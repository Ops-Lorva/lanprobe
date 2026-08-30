//! Export de métriques vers InfluxDB v1 ou v2.
//!
//! Le module écoute le bus `broadcast::Sender<BroadcastEvent>` et convertit
//! chaque event en lignes InfluxDB Line Protocol. Les lignes sont bufférisées
//! et flushées toutes les secondes en une seule requête POST.
//!
//! Config lue depuis `AppState::config` (clé `"influxdb"` dans
//! `app_config.json`). Le module attend silencieusement un `config:update`
//! valide avant de démarrer l'envoi.


use crate::export_buffer::{flush_once, Backoff, ExportBuffer, PointWriter};
use crate::state::{AppState, BroadcastEvent};

// ── Config structs ─────────────────────────────────────────────────────────

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
    /// ⚠️ C'est désormais le SEUL client. L'export direct vers Influx —
    /// URL, organisation, bucket et jeton saisis dans l'application — a été
    /// retiré : il demandait à chaque sonde de connaître Influx et de porter
    /// un jeton d'écriture, exactement ce que le hub évite.
    async fn for_hub(
        hub_url: &str,
        probe_id: &str,
        token: &str,
        host_tag: String,
        hub_cfg: &crate::hub::HubConfig,
        source: Option<std::net::Ipv4Addr>,
    ) -> Self {
        Self {
            // 🔴 **Le même client que le battement de cœur**, épingle
            // comprise. Un client construit ici à part est exactement ce qui
            // s'était produit : la sonde se rattachait, battait, et n'écrivait
            // jamais rien — le tampon grossissait en silence pendant que
            // l'interface annonçait « rattachée ». Passer la configuration
            // entière plutôt que des bouts rend cette divergence impossible.
            //
            // La contrainte d'interface vaut ici aussi : sans elle, une sonde
            // pouvait mesurer un lien mort et livrer ses relevés par un lien
            // sain.
            http: crate::hub::http_client_pinned(hub_cfg, source),
            base_url: format!(
                "{}/api/probes/{}/write",
                hub_url.trim_end_matches('/'),
                probe_id
            ),
            auth_header: Some(format!("Bearer {token}")),
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

/// Nom de machine, utilisé comme étiquette `host=` quand le hub n'a pas
/// encore nommé la sonde.
async fn resolve_host_tag_async() -> String {
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
            // L'URL du résultat Ookla est un CHAMP, jamais une étiquette :
            // elle est unique à chaque test, et une étiquette unique fait
            // exploser la cardinalité de la série. iperf3 n'en produit pas.
            // Le serveur mesuré est une information de lecture, pas un axe :
            // « 216 Mbit/s » ne veut rien dire sans savoir contre quoi. Pour
            // iperf3 c'est l'adresse du serveur, pour Ookla le datacenter.
            // En CHAMP, comme l'URL : il change à chaque test Ookla, et une
            // étiquette qui change fait exploser la cardinalité.
            let server = p["server_name"]
                .as_str()
                .filter(|v| !v.trim().is_empty())
                .map(|v| format!(",server=\"{}\"", escape_field_str(v)))
                .unwrap_or_default();
            let result_url = p["result_url"]
                .as_str()
                .filter(|u| !u.is_empty())
                .map(|u| format!(",result_url=\"{}\"", escape_field_str(u)))
                .unwrap_or_default();
            vec![format!(
                "speedtest,host={},engine={} download_mbps={},upload_mbps={},latency_ms={}i,jitter_ms={}{}{} {}",
                host_tag,
                escape_tag(engine),
                dl,
                ul,
                lat,
                jitter,
                server,
                result_url,
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

async fn hub_client(state: &AppState, key: &crate::secrets::SecretKey) -> Option<InfluxClient> {
    let cfg = crate::hub::load(state);
    if !cfg.is_enrolled() {
        return None;
    }
    let token = crate::secrets::open(key, &cfg.token).ok()?;
    // Le tag lisible reste le nom donné à la sonde dans le hub : c'est celui
    // que l'utilisateur reconnaîtra dans Grafana.
    let host_tag = if cfg.name.is_empty() {
        resolve_host_tag_async().await
    } else {
        cfg.name.clone()
    };
    // Une interface sans IPv4 : on ne construit pas de client plutôt que d'en
    // faire un qui sortirait par ailleurs. L'appelant réessaiera au prochain
    // changement d'interface, et le tampon garde les points en attendant.
    let source = match crate::scheduler::resolve_src(state) {
        Ok(source) => source,
        Err(e) => {
            tracing::warn!("export suspendu : {e}");
            return None;
        }
    };
    tracing::info!("mesures envoyées au hub {} (sonde « {} »)", cfg.url, host_tag);
    Some(
        InfluxClient::for_hub(&cfg.url, &cfg.probe_id, &token, host_tag, &cfg, source).await,
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
    state: AppState,
    key: crate::secrets::SecretKey,
    mut rx: tokio::sync::broadcast::Receiver<BroadcastEvent>,
    client: InfluxClient,
    status: Option<std::sync::Arc<crate::hub::WriteStatus>>,
    mut buffer: ExportBuffer,
) {
    let mut client = client;
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut backoff = Backoff::new();
    let mut ticks = 0u32;

    loop {
        tokio::select! {
            event = rx.recv() => match event {
                // ⚠️ Le client porte l'adresse source de l'interface : il est
                // périmé dès qu'on change d'interface. Sans cette
                // reconstruction, les mesures continuaient de partir par
                // l'ancien lien — y compris après l'avoir débranché.
                Ok(event) if event.event == "interface:selected" || event.event == "config:update" => {
                    match hub_client(&state, &key).await {
                        Some(fresh) => client = fresh,
                        None => tracing::warn!(
                            "interface sans adresse source : l'export attend, le tampon garde les points"
                        ),
                    }
                }
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
    // S'abonner AVANT de lire la configuration : entre les deux, une mesure
    // pourrait naître et se perdre.
    let mut rx = state.events.subscribe();

    // Reprendre ce qu'une exécution précédente n'a pas pu livrer. Le tampon
    // survit à la coupure de courant : sinon, la panne efface la trace de la
    // panne.
    let buffer = ExportBuffer::open_at(
        ExportBuffer::default_path(&state.config.dir()),
        now_ns(),
    );

    let Some(key) = key else {
        tracing::error!("clé de scellement absente : les mesures ne peuvent pas partir");
        return;
    };

    // ⚠️ Il faut guetter le rattachement, pas seulement le lire au démarrage.
    // Une sonde enrôlée alors que l'application tournait déjà n'envoyait
    // jamais rien : la destination était choisie une seule fois. Le battement
    // de cœur, lui, relit sa configuration à chaque tour — d'où une sonde qui
    // apparaît en ligne dans le parc sans qu'aucune mesure n'arrive, et qu'il
    // fallait redémarrer pour débloquer.
    //
    // Pendant cette attente les mesures continuent d'être empilées : c'est
    // tout l'intérêt du tampon, et c'est ce qui fait qu'un enrôlement tardif
    // ne perd pas la matinée.
    let mut buffer = buffer;
    loop {
        if let Some(client) = hub_client(&state, &key).await {
            run_with(state, key, rx, client, status, buffer).await;
            return;
        }
        match rx.recv().await {
            Ok(event) if event.event == "config:update" => continue,
            Ok(event) => {
                // Pas encore de nom de sonde : `host=` sera réécrit par le hub
                // à la réception, qui étiquette de toute façon chaque point.
                let now = now_ns();
                let host = state.config.get()
                    .get("hub")
                    .and_then(|h| h.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                for line in event_to_points(&event, &host) {
                    buffer.push_at(line, now);
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("export : {n} événements perdus dans le bus");
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

    #[tokio::test]
    async fn the_hub_url_is_the_probe_write_endpoint() {
        // Le seul client qui reste. Si cette URL change de forme, la sonde
        // écrit dans le vide sans que rien ne le dise : le hub répondrait 404
        // et le tampon grossirait en silence.
        let client = InfluxClient::for_hub(
            "https://hub.exemple.fr/",
            "13f1af69-6f84-41c5-a1c4-cf742b02b71d",
            "jeton",
            "Windows".into(),
            &crate::hub::HubConfig::default(),
            None,
        )
        .await;
        assert_eq!(
            client.base_url,
            "https://hub.exemple.fr/api/probes/13f1af69-6f84-41c5-a1c4-cf742b02b71d/write"
        );
        assert_eq!(client.auth_header.as_deref(), Some("Bearer jeton"));
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
    fn a_speedtest_result_url_travels_as_a_field_not_a_tag() {
        // L'interface propose un lien vers le résultat Ookla ; il doit donc
        // arriver jusqu'à Influx. En étiquette, chaque test créerait une série
        // nouvelle — la cardinalité exploserait pour un lien qu'on ne filtre
        // jamais.
        let event = make_event(
            "speedtest:result",
            json!({
                "engine": "ookla",
                "download_mbps": 910.7,
                "upload_mbps": 360.2,
                "latency_ms": 12,
                "jitter_ms": 1.5,
                "server_name": "Paris — Île-de-France, FR",
                "result_url": "https://www.speedtest.net/result/c/abc-123"
            }),
        );
        let points = event_to_points(&event, "testhost");
        let (tags, fields) = points[0].split_once(' ').unwrap();
        assert!(!tags.contains("result_url"), "l'URL ne doit pas être une étiquette : {tags}");
        assert!(!tags.contains("server="), "le serveur non plus : {tags}");
        assert!(
            fields.contains(r#"result_url="https://www.speedtest.net/result/c/abc-123""#),
            "URL absente des champs : {fields}"
        );
        // « 216 Mbit/s » ne veut rien dire sans savoir contre quoi.
        assert!(
            fields.contains(r#"server="Paris — Île-de-France, FR""#),
            "serveur absent des champs : {fields}"
        );
    }

    #[test]
    fn iperf_has_no_result_url_and_the_field_is_simply_absent() {
        // Un champ vide n'est pas un champ à chaîne vide : l'interface doit
        // pouvoir distinguer « pas de lien » de « lien vide ».
        let event = make_event(
            "speedtest:result",
            json!({
                "engine": "iperf3",
                "download_mbps": 940.0,
                "upload_mbps": 930.0,
                "latency_ms": 1,
                "jitter_ms": 0.2
            }),
        );
        let points = event_to_points(&event, "testhost");
        assert!(!points[0].contains("result_url"), "{}", points[0]);
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
