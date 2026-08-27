//! `lanprobe-web` — le hub auto-hébergé.
//!
//! Le démarrage suit une règle : **rien de ce qui peut attendre ne bloque le
//! service**. Influx pas encore prêt, jeton opérateur pas encore écrit par
//! l'entrypoint — le hub écoute quand même et réessaie en tâche de fond. Un
//! conteneur qui refuse de démarrer parce qu'un autre n'est pas là est
//! ingérable en compose, puisqu'on ne contrôle pas l'ordre de disponibilité.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use lanprobe_web::{auth::Auth, db::Db, influx, settings::Settings, tls, web};

#[derive(Parser)]
#[command(name = "lanprobe-web", about = "LanProbe Web — hub auto-hébergé")]
struct Args {
    /// Adresse d'écoute.
    #[arg(long, env = "LANPROBE_WEB_HOST", default_value = "0.0.0.0")]
    host: String,

    #[arg(long, env = "LANPROBE_WEB_PORT", default_value_t = 8443)]
    port: u16,

    /// Volume du hub : base SQLite, certificat, jeton opérateur, token de setup.
    #[arg(long, env = "LANPROBE_WEB_CONFIG_DIR", default_value = "/data/lanprobe")]
    config_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    std::fs::create_dir_all(&args.config_dir).map_err(|e| e.to_string())?;

    let db = Arc::new(Db::open(&args.config_dir.join("hub.sqlite")).map_err(|e| e.to_string())?);
    let settings = Settings::new(db.clone());
    // L'environnement n'amorce que ce qui n'a jamais été réglé : ensuite,
    // c'est l'interface qui décide.
    if let Err(e) = settings.seed_from_env() {
        tracing::warn!("amorçage des réglages depuis l'environnement : {e}");
    }

    let auth = Arc::new(Auth::new(db.clone()));
    let token_path = lanprobe_web::auth::default_setup_token_path(&args.config_dir);
    match auth.init_setup_token(&token_path) {
        Ok(Some(token)) => tracing::warn!(
            "aucun compte configuré — POST /api/setup exige ce token de setup.\n\
             Token : {token}\n\
             (aussi lisible dans {} — permissions 0600)",
            token_path.display()
        ),
        Ok(None) => {}
        Err(e) => tracing::error!("token de setup non généré : {e}"),
    }

    // Jeton opérateur absent : on démarre quand même. L'interface est alors
    // joignable, ce qui est exactement ce qu'il faut pour diagnostiquer.
    let operator_token = match influx::load_operator_token(&args.config_dir) {
        Ok(token) => token,
        Err(e) => {
            tracing::error!("{e} — le provisionnement InfluxDB échouera jusqu'à correction");
            String::new()
        }
    };
    let influx = Arc::new(influx::Influx::new(settings.clone(), operator_token));

    {
        let influx = influx.clone();
        let retention_days = settings.retention_days();
        tokio::spawn(async move {
            influx::run_provisioning(
                influx.clone(),
                std::time::Duration::from_secs(2),
                std::time::Duration::from_secs(60),
            )
            .await;
            // Recalculée à chaque démarrage : un certificat Influx renouvelé
            // se propage ainsi aux sondes au battement de cœur suivant, sans
            // que personne n'intervienne.
            match influx.ensure_tls_fingerprint().await {
                Ok(f) if !f.is_empty() => tracing::info!("empreinte TLS d'InfluxDB : {f}"),
                Ok(_) => tracing::warn!("InfluxDB est servi en HTTP clair : rien à épingler"),
                Err(e) => tracing::warn!("empreinte TLS d'InfluxDB non calculée : {e}"),
            }
            if let Err(e) = influx.apply_retention(retention_days).await {
                tracing::warn!("rétention non appliquée : {e}");
            }
        });
    }

    let state = web::AppState {
        db,
        auth,
        settings,
        influx,
    };
    let router = web::build_router(state);

    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .map_err(|e| format!("adresse d'écoute invalide : {e}"))?;
    let tls_paths = tls::tls_paths(&args.config_dir);
    let tls_config = tls::server_config(&tls_paths)?;

    tracing::info!("hub à l'écoute sur https://{addr}");
    axum_server::bind_rustls(
        addr,
        axum_server::tls_rustls::RustlsConfig::from_config(tls_config),
    )
    .serve(router.into_make_service())
    .await
    .map_err(|e| e.to_string())
}
