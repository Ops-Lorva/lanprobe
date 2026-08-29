//! Export de métriques vers InfluxDB v1 ou v2.
//!
//! Le module écoute le bus `broadcast::Sender<BroadcastEvent>` et convertit
//! chaque event en lignes InfluxDB Line Protocol. Les lignes sont bufférisées
//! et flushées toutes les secondes en une seule requête POST.
//!
//! Config lue depuis `AppState::config` (clé `"influxdb"` dans
//! `app_config.json`). Le module attend silencieusement un `config:update`
//! valide avant de démarrer l'envoi.

use base64::{engine::general_purpose::STANDARD as B64_STANDARD, Engine};

use crate::export_buffer::{flush_once, Backoff, ExportBuffer, PointWriter};
use crate::state::{AppState, BroadcastEvent};

// ── Config structs ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct InfluxConfig {
    #[serde(default)]
    pub enabled: bool,
    /// "v1" or "v2"
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub url: String,
    /// Surcharge le hostname détecté automatiquement pour le tag `host=`.
    #[serde(default)]
    pub instance_label: String,
    #[serde(default)]
    pub v1: V1Config,
    #[serde(default)]
    pub v2: V2Config,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct V1Config {
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct V2Config {
    #[serde(default)]
    pub org: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub token: String,
}

impl InfluxConfig {
    pub fn is_ready(&self) -> bool {
        if !self.enabled || self.url.is_empty() {
            return false;
        }
        match self.version.as_str() {
            "v1" => !self.v1.database.is_empty(),
            "v2" | "" => {
                !self.v2.org.is_empty()
                    && !self.v2.bucket.is_empty()
                    && !self.v2.token.is_empty()
            }
            _ => false,
        }
    }
}

// ── HTTP client ────────────────────────────────────────────────────────────

struct InfluxClient {
    http: reqwest::Client,
    base_url: String,
    auth_header: Option<String>,
    host_tag: String,
}

impl InfluxClient {
    /// Client visant le **hub** plutôt qu'InfluxDB.
    ///
    /// Quand la sonde est rattachée à un hub, elle ne touche plus à Influx :
    /// elle envoie au hub, qui relaie. Une seule adresse, une seule
    /// authentification, un seul certificat — et Influx n'a pas besoin d'être
    /// joignable depuis le réseau de la sonde.
    ///
    /// L'export direct vers Influx reste entièrement fonctionnel pour qui
    /// n'utilise pas de hub : on ajoute un chemin, on n'en retire pas.
    async fn for_hub(
        hub_url: &str,
        probe_id: &str,
        token: &str,
        host_tag: String,
        allow_self_signed: bool,
    ) -> Self {
        Self {
            // Le hub peut porter un certificat auto-signé quand l'utilisateur
            // l'a explicitement déclaré : le relais des mesures doit suivre le
            // même réglage que le battement de cœur, sinon la sonde se
            // rattache mais n'écrit jamais rien.
            http: {
                let builder = reqwest::Client::builder();
                let builder = if allow_self_signed {
                    builder.danger_accept_invalid_certs(true)
                } else {
                    builder
                };
                builder.build().unwrap_or_default()
            },
            base_url: format!(
                "{}/api/probes/{}/write",
                hub_url.trim_end_matches('/'),
                probe_id
            ),
            auth_header: Some(format!("Bearer {token}")),
            host_tag,
        }
    }

    async fn new(cfg: &InfluxConfig) -> Self {
        let host_tag = resolve_host_tag_async(cfg).await;

        let (base_url, auth_header) = if cfg.version == "v1" {
            let mut url = reqwest::Url::parse(&format!("{}/write", cfg.url.trim_end_matches('/')))
                .expect("invalid InfluxDB URL");
            url.query_pairs_mut()
                .append_pair("db", &cfg.v1.database)
                .append_pair("precision", "ns");
            let auth = if !cfg.v1.username.is_empty() {
                let encoded =
                    B64_STANDARD.encode(format!("{}:{}", cfg.v1.username, cfg.v1.password));
                Some(format!("Basic {}", encoded))
            } else {
                None
            };
            (url.to_string(), auth)
        } else {
            let mut url = reqwest::Url::parse(&format!("{}/api/v2/write", cfg.url.trim_end_matches('/')))
                .expect("invalid InfluxDB URL");
            url.query_pairs_mut()
                .append_pair("org", &cfg.v2.org)
                .append_pair("bucket", &cfg.v2.bucket)
                .append_pair("precision", "ns");
            let auth = if !cfg.v2.token.is_empty() {
                Some(format!("Token {}", cfg.v2.token))
            } else {
                None
            };
            (url.to_string(), auth)
        };

        Self {
            http: reqwest::Client::new(),
            base_url,
            auth_header,
            host_tag,
        }
    }

    async fn write(&self, body: String) -> Result<(), String> {
        let mut req = self.http
            .post(&self.base_url)
            .timeout(std::time::Duration::from_secs(10))
            .body(body);
        if let Some(auth) = &self.auth_header {
            req = req.header("Authorization", auth);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(format!("écriture refusée ({status})"))
        }
    }
}

impl PointWriter for InfluxClient {
    async fn write_points(&self, body: String) -> Result<(), String> {
        self.write(body).await
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

async fn resolve_host_tag_async(cfg: &InfluxConfig) -> String {
    if !cfg.instance_label.is_empty() {
        return cfg.instance_label.clone();
    }
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    tokio::process::Command::new("hostname")
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Escapes spaces, commas and equals signs as required by InfluxDB Line Protocol.
fn escape_tag(s: &str) -> String {
    s.replace(',', "\\,").replace('=', "\\=").replace(' ', "\\ ")
}

/// Escapes backslashes and double-quotes for InfluxDB Line Protocol string field values.
fn escape_field_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Converts a `BroadcastEvent` to zero or more InfluxDB line protocol strings.
fn event_to_points(event: &BroadcastEvent, host: &str) -> Vec<String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let host_tag = escape_tag(host);
    let p = &event.payload;

    match event.event.as_str() {
        "ping:tick" => {
            let ip = p["ip"].as_str().unwrap_or("unknown");
            let alive = p["alive"].as_bool().unwrap_or(false);
            let alive_str = if alive { "true" } else { "false" };
            // latency_ms n'est inclus que quand l'hôte est joignable — écrire
            // 0 pour un hôte unreachable serait trompeur.
            let latency_part = if alive {
                format!("latency_ms={}i,", p["latency_ms"].as_u64().unwrap_or(0))
            } else {
                String::new()
            };
            vec![format!(
                "ping_latency,host={},ip={} {}alive={} {}",
                host_tag,
                escape_tag(ip),
                latency_part,
                alive_str,
                ts
            )]
        }

        "internet:tick" => {
            // `state` est sérialisé comme une string (snake_case via serde).
            let state = p["state"].as_str().unwrap_or("unknown");
            let icmp = p["icmp_ms"].as_u64().unwrap_or(0);
            let http = p["http_ms"].as_u64().unwrap_or(0);
            let dns = p["dns_ms"].as_u64().unwrap_or(0);
            let uptime = p["uptime_pct"].as_f64().unwrap_or(0.0);
            let icmp_ok = if p["icmp_ok"].as_bool().unwrap_or(false) {
                "true"
            } else {
                "false"
            };
            let http_ok = if p["http_ok"].as_bool().unwrap_or(false) {
                "true"
            } else {
                "false"
            };
            let dns_ok = if p["dns_ok"].as_bool().unwrap_or(false) {
                "true"
            } else {
                "false"
            };
            vec![format!(
                "internet_status,host={} state=\"{}\",icmp_ms={}i,http_ms={}i,dns_ms={}i,uptime_pct={},icmp_ok={},http_ok={},dns_ok={} {}",
                host_tag,
                escape_field_str(state),
                icmp,
                http,
                dns,
                uptime,
                icmp_ok,
                http_ok,
                dns_ok,
                ts
            )]
        }

        "speedtest:result" => {
            let engine = p["engine"].as_str().unwrap_or("unknown");
            let dl = p["download_mbps"].as_f64().unwrap_or(0.0);
            let ul = p["upload_mbps"].as_f64().unwrap_or(0.0);
            let lat = p["latency_ms"].as_u64().unwrap_or(0);
            // jitter_ms est Option<f64> dans SpeedResult.
            let jitter = p["jitter_ms"].as_f64().unwrap_or(0.0);
            vec![format!(
                "speedtest,host={},engine={} download_mbps={},upload_mbps={},latency_ms={}i,jitter_ms={} {}",
                host_tag,
                escape_tag(engine),
                dl,
                ul,
                lat,
                jitter,
                ts
            )]
        }

        "discovery:done" => {
            let cidr = p["cidr"].as_str().unwrap_or("");
            let hosts = p["hosts_found"].as_i64().unwrap_or(0);
            if cidr.is_empty() {
                return vec![];
            }
            vec![format!(
                "discovery,host={},cidr={} hosts_found={}i {}",
                host_tag,
                escape_tag(cidr),
                hosts,
                ts
            )]
        }

        "portscan:update" => {
            let ip = p["ip"].as_str().unwrap_or("unknown");
            // Le payload est un PortScanEntry sérialisé :
            // { ip, tcp: [...], udp: [...], timestamp, profile_id, in_progress }
            // On ne log pas les scans en cours — attendre la fin.
            if p["in_progress"].as_bool().unwrap_or(false) {
                return vec![];
            }
            let open_tcp = p["tcp"]
                .as_array()
                .map(|a| a.len() as i64)
                .unwrap_or(0);
            let open_udp = p["udp"]
                .as_array()
                .map(|a| a.len() as i64)
                .unwrap_or(0);
            vec![format!(
                "port_scan,host={},ip={} open_tcp={}i,open_udp={}i {}",
                host_tag,
                escape_tag(ip),
                open_tcp,
                open_udp,
                ts
            )]
        }

        _ => vec![],
    }
}

// ── Config loader ──────────────────────────────────────────────────────────

fn load_config(state: &AppState) -> InfluxConfig {
    let cfg_value = state.config.get();
    cfg_value
        .get("influxdb")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Teste la connectivité vers l'endpoint InfluxDB configuré.
/// Retourne `Ok(())` si le serveur répond avec 200 ou 204, une erreur
/// descriptive sinon.
pub async fn test_connection(state: AppState) -> Result<(), String> {
    let cfg = load_config(&state);
    if !cfg.enabled {
        return Err("InfluxDB is not enabled".to_string());
    }
    if cfg.url.is_empty() {
        return Err("InfluxDB URL is not configured".to_string());
    }
    let ping_url = if cfg.version == "v1" {
        format!("{}/ping", cfg.url.trim_end_matches('/'))
    } else {
        format!("{}/api/v2/ping", cfg.url.trim_end_matches('/'))
    };
    let client = InfluxClient::new(&cfg).await;
    let mut req = client.http.get(&ping_url);
    if let Some(auth) = &client.auth_header {
        req = req.header("Authorization", auth);
    }
    let resp = req
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(format!("Unexpected status: {}", status))
    }
}

/// Construit le client visant le hub, si la sonde y est rattachée.
async fn hub_client(state: &AppState, key: &crate::secrets::SecretKey) -> Option<InfluxClient> {
    let cfg = crate::hub::load(state);
    if !cfg.is_enrolled() {
        return None;
    }
    let token = crate::secrets::open(key, &cfg.token).ok()?;
    // Le tag lisible reste le nom donné à la sonde dans le hub : c'est celui
    // que l'utilisateur reconnaîtra dans Grafana.
    let host_tag = if cfg.name.is_empty() {
        resolve_host_tag_async(&InfluxConfig::default()).await
    } else {
        cfg.name.clone()
    };
    tracing::info!("mesures envoyées au hub {} (sonde « {} »)", cfg.url, host_tag);
    Some(
        InfluxClient::for_hub(
            &cfg.url,
            &cfg.probe_id,
            &token,
            host_tag,
            cfg.allow_self_signed,
        )
        .await,
    )
}

/// Le tampon n'est réécrit qu'une fois toutes les 10 s : réécrire 100 000
/// lignes à chaque seconde coûterait plus cher que le service lui-même. Le
/// prix de ce délai est un éventuel rejeu de quelques secondes déjà écrites
/// après un arrêt brutal — sans conséquence, Influx écrasant un point de
/// même mesure, mêmes tags et même horodatage.
const PERSIST_EVERY_TICKS: u32 = 10;

/// Horodatage courant en nanosecondes — même base que celle posée sur les
/// points par `event_to_points`.
fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

fn persist_buffer(buffer: &mut ExportBuffer, ticks: &mut u32, force: bool) {
    *ticks += 1;
    if !buffer.is_dirty() || (!force && *ticks < PERSIST_EVERY_TICKS) {
        return;
    }
    *ticks = 0;
    if let Err(e) = buffer.persist() {
        tracing::warn!("tampon d'export non persisté ({e}) — un redémarrage perdrait les points en attente");
    }
}

/// Une seconde de la boucle d'envoi : tentative d'écriture, remontée d'état,
/// et persistance périodique du tampon.
async fn flush_tick(
    client: &InfluxClient,
    buffer: &mut ExportBuffer,
    backoff: &mut Backoff,
    status: Option<&std::sync::Arc<crate::hub::WriteStatus>>,
    ticks: &mut u32,
) {
    // ⚠️ On ne vide le tampon qu'APRÈS confirmation. Le vider avant
    // l'écriture — ce que faisait la 1.1.5 — perd les points exactement
    // pendant l'incident qu'on voudrait analyser.
    flush_once(client, buffer, backoff, std::time::Instant::now()).await;
    // Remonter la profondeur du tampon : sans ça, l'app affiche « tout va
    // bien » pendant qu'elle accumule sans rien livrer, et le hub reçoit un
    // `buffered_points` figé à zéro qui ne signale jamais rien. Tant que le
    // repli court, la dernière écriture est en échec : on ne prétend pas le
    // contraire.
    if let Some(status) = status {
        status.set(buffer.len(), backoff.delay().is_zero());
    }
    persist_buffer(buffer, ticks, false);
}

/// Boucle d'envoi commune aux deux destinations.
async fn run_with(
    _state: AppState,
    mut rx: tokio::sync::broadcast::Receiver<BroadcastEvent>,
    client: InfluxClient,
    status: Option<std::sync::Arc<crate::hub::WriteStatus>>,
    mut buffer: ExportBuffer,
) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut backoff = Backoff::new();
    let mut ticks = 0u32;

    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok(event) => {
                    let now = now_ns();
                    for line in event_to_points(&event, &client.host_tag) {
                        buffer.push_at(line, now);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    // Arrêt propre : on ne part pas en laissant sur le disque
                    // une photo périmée du tampon.
                    persist_buffer(&mut buffer, &mut ticks, true);
                    return;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("bus saturé, {n} events perdus");
                }
            },
            _ = ticker.tick() => {
                flush_tick(&client, &mut buffer, &mut backoff, status.as_ref(), &mut ticks).await;
            }
        }
    }
}

/// Tâche de fond — souscrit aux events et pousse les métriques vers
/// InfluxDB. Bloque jusqu'à la fermeture du canal broadcast.
///
/// Le flux de contrôle :
/// 1. On souscrit AVANT de lire la config (évite de rater des events).
/// 2. On reprend le tampon laissé par l'exécution précédente.
/// 3. Si la config n'est pas prête, on attend un `config:update` valide.
/// 4. On tourne ensuite dans une boucle select : on bufférise les points et
///    on tente une écriture toutes les secondes, avec repli exponentiel.
/// 5. Sur `config:update`, on recharge. Si InfluxDB est désactivé, on
///    abandonne le tampon et on attend la réactivation.
pub async fn run(
    state: AppState,
    key: Option<crate::secrets::SecretKey>,
    status: Option<std::sync::Arc<crate::hub::WriteStatus>>,
) {
    // 1. S'abonner avant de lire la config.
    let mut rx = state.events.subscribe();

    // 2. Reprendre ce qu'une exécution précédente n'a pas pu livrer. Le
    //    tampon survit à la coupure de courant : sinon, la panne efface la
    //    trace de la panne.
    let mut buffer = ExportBuffer::open_at(
        ExportBuffer::default_path(&state.config.dir()),
        now_ns(),
    );

    // Rattachée à un hub : c'est lui qui reçoit les mesures, et il n'y a rien
    // d'autre à configurer. Ce chemin court-circuite l'export direct, qui
    // reste disponible pour qui n'utilise pas de hub.
    if let Some(key) = key.as_ref() {
        if let Some(client) = hub_client(&state, key).await {
            run_with(state, rx, client, status, buffer).await;
            return;
        }
    }

    // 3. Charger la config initiale et attendre qu'elle soit valide.
    //
    // ⚠️ On guette aussi le **rattachement à un hub** pendant cette attente.
    // Sans ça, une sonde enrôlée alors que l'app tournait déjà n'envoyait
    // jamais rien : le choix de destination était fait une seule fois au
    // démarrage. Le battement de cœur, lui, relit sa configuration à chaque
    // tour — d'où une sonde qui apparaît en ligne dans le parc sans qu'aucune
    // mesure n'arrive. Il fallait redémarrer l'app pour que l'export parte.
    let mut cfg = load_config(&state);
    while !cfg.is_ready() {
        match rx.recv().await {
            Ok(event) if event.event == "config:update" => {
                if let Some(key) = key.as_ref()
                    && let Some(client) = hub_client(&state, key).await
                {
                    run_with(state, rx, client, status, buffer).await;
                    return;
                }
                cfg = load_config(&state);
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("InfluxDB: broadcast lagged, {} events dropped", n);
            }
            _ => {}
        }
    }

    // 4. Construire le client.
    let mut client = InfluxClient::new(&cfg).await;

    // 5. Ticker de flush à 1 s. Le repli, lui, espace les tentatives quand
    //    la destination ne répond plus.
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut backoff = Backoff::new();
    let mut ticks = 0u32;

    loop {
        tokio::select! {
            event_result = rx.recv() => {
                match event_result {
                    Ok(event) => {
                        if event.event == "config:update" {
                            let new_cfg = load_config(&state);
                            if new_cfg.is_ready() {
                                client = InfluxClient::new(&new_cfg).await;
                            } else {
                                // InfluxDB désactivé ou config incomplète → on
                                // abandonne le tampon (c'est une décision de
                                // l'utilisateur, pas une panne) et on attend la
                                // réactivation.
                                buffer.discard_all();
                                persist_buffer(&mut buffer, &mut ticks, true);
                                cfg = new_cfg;
                                while !cfg.is_ready() {
                                    match rx.recv().await {
                                        Ok(e) if e.event == "config:update" => {
                                            cfg = load_config(&state);
                                        }
                                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                            tracing::warn!("InfluxDB: broadcast lagged, {} events dropped", n);
                                        }
                                        _ => {}
                                    }
                                }
                                client = InfluxClient::new(&cfg).await;
                            }
                        } else {
                            let now = now_ns();
                            for line in event_to_points(&event, &client.host_tag) {
                                buffer.push_at(line, now);
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        persist_buffer(&mut buffer, &mut ticks, true);
                        return;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("InfluxDB: broadcast lagged, {} events dropped", n);
                    }
                }
            }
            _ = ticker.tick() => {
                flush_tick(&client, &mut buffer, &mut backoff, status.as_ref(), &mut ticks).await;
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_event(event: &str, payload: serde_json::Value) -> BroadcastEvent {
        BroadcastEvent {
            event: event.to_string(),
            payload,
        }
    }

    #[test]
    fn test_influx_config_default() {
        assert!(!InfluxConfig::default().is_ready());
    }

    #[test]
    fn test_influx_config_v1_ready() {
        let cfg = InfluxConfig {
            enabled: true,
            version: "v1".to_string(),
            url: "http://localhost:8086".to_string(),
            instance_label: String::new(),
            v1: V1Config {
                database: "mydb".to_string(),
                username: String::new(),
                password: String::new(),
            },
            v2: V2Config::default(),
        };
        assert!(cfg.is_ready());
    }

    #[test]
    fn test_influx_config_v2_ready() {
        let cfg = InfluxConfig {
            enabled: true,
            version: "v2".to_string(),
            url: "http://localhost:8086".to_string(),
            instance_label: String::new(),
            v1: V1Config::default(),
            v2: V2Config {
                org: "myorg".to_string(),
                bucket: "mybucket".to_string(),
                token: "mytoken".to_string(),
            },
        };
        assert!(cfg.is_ready());
    }

    #[test]
    fn test_escape_tag() {
        assert_eq!(
            escape_tag("hello world,foo=bar"),
            "hello\\ world\\,foo\\=bar"
        );
    }

    #[test]
    fn test_escape_field_str() {
        assert_eq!(escape_field_str("hello"), "hello");
        assert_eq!(escape_field_str("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_field_str("path\\to"), "path\\\\to");
    }

    #[test]
    fn test_event_to_points_ping() {
        let event = make_event(
            "ping:tick",
            json!({ "ip": "192.168.1.1", "alive": true, "latency_ms": 5 }),
        );
        let points = event_to_points(&event, "testhost");
        assert_eq!(points.len(), 1);
        assert!(points[0].starts_with("ping_latency,host=testhost,ip=192.168.1.1 "));
    }

    #[test]
    fn test_event_to_points_internet() {
        let event = make_event(
            "internet:tick",
            json!({
                "state": "up",
                "icmp_ms": 5,
                "http_ms": 10,
                "dns_ms": 8,
                "uptime_pct": 99.9,
                "icmp_ok": true,
                "http_ok": true,
                "dns_ok": true
            }),
        );
        let points = event_to_points(&event, "testhost");
        assert_eq!(points.len(), 1);
        assert!(points[0].starts_with("internet_status,host=testhost "));
    }

    #[test]
    fn test_event_to_points_speedtest() {
        let event = make_event(
            "speedtest:result",
            json!({
                "engine": "ookla",
                "download_mbps": 100.5,
                "upload_mbps": 50.2,
                "latency_ms": 12,
                "jitter_ms": 1.5,
                "server_name": "Paris, FR"
            }),
        );
        let points = event_to_points(&event, "testhost");
        assert_eq!(points.len(), 1);
        assert!(points[0].contains(",engine=ookla "), "Expected ',engine=ookla ' in: {}", points[0]);
    }

    #[test]
    fn test_event_to_points_discovery() {
        let event = make_event(
            "discovery:done",
            json!({ "cidr": "10.0.0.0/24", "hosts_found": 3 }),
        );
        let points = event_to_points(&event, "testhost");
        assert_eq!(points.len(), 1);
        assert!(points[0].contains("cidr=10.0.0.0/24"), "Expected cidr tag in: {}", points[0]);
        assert!(points[0].contains("hosts_found=3i"), "Expected hosts_found in: {}", points[0]);
    }

    #[test]
    fn test_event_to_points_discovery_empty_cidr() {
        let event = BroadcastEvent {
            event: "discovery:done".to_string(),
            payload: serde_json::json!({ "cidr": "", "hosts_found": 0 }),
        };
        let points = event_to_points(&event, "testhost");
        assert!(points.is_empty(), "empty cidr should produce no points");
    }

    #[test]
    fn test_event_to_points_portscan_in_progress() {
        let event = BroadcastEvent {
            event: "portscan:update".to_string(),
            payload: serde_json::json!({ "ip": "192.168.1.1", "tcp": [], "udp": [], "in_progress": true }),
        };
        let points = event_to_points(&event, "testhost");
        assert!(points.is_empty(), "in-progress scan should produce no points");
    }

    #[test]
    fn test_event_to_points_portscan() {
        let event = make_event(
            "portscan:update",
            json!({
                "ip": "192.168.1.100",
                "tcp": [
                    { "port": 22, "service": "SSH", "proto": "tcp", "open": true },
                    { "port": 80, "service": "HTTP", "proto": "tcp", "open": true }
                ],
                "udp": [],
                "timestamp": 1700000000u64,
                "profile_id": null,
                "in_progress": false
            }),
        );
        let points = event_to_points(&event, "testhost");
        assert_eq!(points.len(), 1);
        assert!(points[0].contains("open_tcp=2i"), "Expected open_tcp=2i in: {}", points[0]);
        assert!(points[0].contains("open_udp=0i"), "Expected open_udp=0i in: {}", points[0]);
    }

    #[test]
    fn test_event_to_points_unknown() {
        let event = make_event("unknown:event", json!({}));
        let points = event_to_points(&event, "testhost");
        assert!(points.is_empty());
    }
}
