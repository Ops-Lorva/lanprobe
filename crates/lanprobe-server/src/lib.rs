//! `lanprobe-server` exposé comme bibliothèque : permet à `lanprobe` (client
//! Tauri desktop) d'embarquer le même serveur HTTP et de le démarrer/arrêter
//! depuis un toggle Settings. Binaire standalone et lib partagent les
//! mêmes modules (routes, auth, TLS, WS bridge).

pub mod assets;
pub mod auth;
pub mod config;
pub mod influxdb;
pub mod routes;
pub mod scheduler;
pub mod secrets;
pub mod state;
pub mod tls;
pub mod web;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub use auth::AuthStore;
pub use state::AppState;

/// Handle retourné par `start`. `shutdown()` signale au serveur de couper
/// proprement et attend la fin de la task axum.
pub struct ServerHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<(), String>>>,
    pub addr: SocketAddr,
}

impl ServerHandle {
    pub async fn shutdown(mut self) -> Result<(), String> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.await.map_err(|e| e.to_string())??;
        }
        Ok(())
    }
}

pub struct StartConfig {
    pub addr: SocketAddr,
    pub config_dir: PathBuf,
    /// Si fourni, le serveur réutilise cet `AppState` au lieu d'en créer
    /// un nouveau. C'est ce qui permet à un client web de voir en temps
    /// réel les scans, pings et monitorings lancés depuis le desktop
    /// Tauri (et inversement). Quand cette option est `Some`, le serveur
    /// NE spawn PAS un second internet monitor — celui de Tauri est déjà
    /// en train de publier sur le même bus.
    pub shared_state: Option<AppState>,
}

/// Démarre le serveur HTTPS en arrière-plan et rend la main immédiatement.
/// Le caller garde la `ServerHandle` pour pouvoir l'arrêter plus tard
/// (ex : toggle OFF dans les Settings du client desktop).
pub async fn start(cfg: StartConfig) -> Result<ServerHandle, String> {
    std::fs::create_dir_all(&cfg.config_dir).map_err(|e| e.to_string())?;

    let state = if let Some(s) = cfg.shared_state {
        s
    } else {
        let auth = AuthStore::load(auth::default_users_path(&cfg.config_dir))
            .map(Arc::new)
            .map_err(|e| e.to_string())?;
        let config_store = Arc::new(config::ConfigStore::load(config::default_config_path(
            &cfg.config_dir,
        )));
        let state = AppState::new_headless(auth, config_store);
        // Standalone : on démarre le monitoring internet dans ce processus.
        let history = state.internet.clone();
        let iface = state.selected_interface.clone();
        let events = state.events.clone();
        tokio::spawn(lanprobe_core::internet::run_internet_monitor(
            history,
            iface,
            move |tick| {
                let _ = events.send(state::BroadcastEvent {
                    event: "internet:tick".into(),
                    payload: serde_json::to_value(tick).unwrap_or(serde_json::Value::Null),
                });
            },
        ));
        state
    };

    // Tant qu'aucun admin n'existe, on génère un token de setup à haute
    // entropie que `POST /api/setup` devra présenter. Ça ferme la fenêtre de
    // course où n'importe qui sur le LAN pouvait devenir l'unique admin entre
    // le `systemctl start` et le setup manuel de l'opérateur.
    let token_path = auth::default_setup_token_path(&cfg.config_dir);
    match state.auth.init_setup_token(&token_path) {
        Ok(Some(token)) => {
            tracing::warn!(
                "aucun admin configuré — token de setup requis pour POST /api/setup.\n\
                 Token : {token}\n\
                 (aussi lisible dans {} — permissions 0600)",
                token_path.display()
            );
        }
        Ok(None) => {}
        Err(e) => tracing::error!("impossible de générer le token de setup : {e}"),
    }

    let influx_state = state.clone();
    tokio::spawn(crate::influxdb::run(influx_state));
    let sched_state = state.clone();
    tokio::spawn(crate::scheduler::run(sched_state));

    let tls_paths = tls::tls_paths(&cfg.config_dir);
    let tls_server_config = tls::ensure_rustls_config(&tls_paths).await?;
    let tls_acceptor = tokio_rustls::TlsAcceptor::from(tls_server_config);
    let router = web::build_router(state);

    let (tx, rx) = oneshot::channel::<()>();
    let addr = cfg.addr;
    let task = tokio::spawn(async move {
        use tower::ServiceExt;

        let listener = tokio::net::TcpListener::bind(addr).await
            .map_err(|e| e.to_string())?;
        let https_port = addr.port();
        let mut rx = rx;

        loop {
            tokio::select! {
                _ = &mut rx => break,
                accept = listener.accept() => {
                    let Ok((stream, _peer)) = accept else { continue };

                    // Protocol sniffing : TLS ClientHello commence toujours
                    // par l'octet 0x16 (record type = Handshake). Toute autre
                    // valeur indique du HTTP clair → redirection 301 vers HTTPS.
                    let mut first = [0u8; 1];
                    let peeked = stream.peek(&mut first).await;
                    if peeked.is_err() { continue; }

                    if first[0] == 0x16 {
                        // TLS — handshake puis service axum
                        let tls_acceptor = tls_acceptor.clone();
                        let router = router.clone();
                        tokio::spawn(async move {
                            let Ok(tls_stream) = tls_acceptor.accept(stream).await else { return };
                            let svc = router.into_service();
                            let _ = hyper_util::server::conn::auto::Builder::new(
                                hyper_util::rt::TokioExecutor::new(),
                            )
                            .serve_connection_with_upgrades(
                                hyper_util::rt::TokioIo::new(tls_stream),
                                hyper::service::service_fn(move |req| {
                                    let svc = svc.clone();
                                    async move { svc.oneshot(req).await }
                                }),
                            )
                            .await;
                        });
                    } else {
                        // HTTP — on lit la ligne Host: pour reconstituer l'URL
                        // et on renvoie un 301 Moved Permanently vers HTTPS.
                        tokio::spawn(async move {
                            let _ = send_http_redirect(stream, https_port).await;
                        });
                    }
                }
            }
        }
        Ok(())
    });

    Ok(ServerHandle {
        shutdown_tx: Some(tx),
        task: Some(task),
        addr,
    })
}

/// Lit les premières octets d'une connexion HTTP, extrait le Host, et renvoie
/// une réponse HTTP/1.1 301 vers l'URL HTTPS équivalente sur `https_port`.
/// Les navigateurs suivent automatiquement cette redirection.
async fn send_http_redirect(
    mut stream: tokio::net::TcpStream,
    https_port: u16,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    // Lit suffisamment pour trouver l'en-tête Host + la Request-Line.
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    let req = std::str::from_utf8(&buf[..n]).unwrap_or("");

    // Extrait le chemin depuis la Request-Line (ex: "GET /foo HTTP/1.1\r\n")
    let path = req.lines().next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/");

    // Extrait l'hostname depuis l'en-tête Host: (sans le port)
    let host = req.lines()
        .find(|l| l.to_lowercase().starts_with("host:"))
        .and_then(|l| l.splitn(2, ':').nth(1))
        .map(str::trim)
        .map(|h| h.split(':').next().unwrap_or(h)) // retire le port si présent
        .unwrap_or("localhost");

    let location = format!("https://{}:{}{}", host, https_port, path);
    let response = format!(
        "HTTP/1.1 301 Moved Permanently\r\n\
         Location: {location}\r\n\
         Connection: close\r\n\
         Content-Length: 0\r\n\
         \r\n"
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

pub fn default_config_dir() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lanprobe")
}

pub fn users_file_path(config_dir: &Path) -> PathBuf {
    auth::default_users_path(config_dir)
}

/// Vrai si aucun utilisateur n'est configuré. Utilisé côté desktop pour
/// décider s'il faut prompter user/password dans Settings avant de
/// démarrer le serveur.
pub fn has_account(config_dir: &Path) -> bool {
    let path = auth::default_users_path(config_dir);
    match AuthStore::load(path) {
        Ok(store) => !store.needs_setup(),
        Err(_) => false,
    }
}

/// Crée l'utilisateur admin initial. Ne rejoue pas si un compte existe
/// déjà — dans ce cas, retourne une erreur.
pub fn set_initial_account(
    config_dir: &Path,
    username: &str,
    password: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let store = AuthStore::load(auth::default_users_path(config_dir))
        .map_err(|e| e.to_string())?;
    store.initial_setup(username, password)
}
