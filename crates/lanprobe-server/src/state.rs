//! État partagé du serveur — équivalent headless de ce que la couche
//! Tauri stocke via `.manage()`. Interface sélectionnée, ping map, historique
//! internet, discovery partagée, monitoring partagé, et le broadcaster
//! d'events pour les WebSockets.
//!
//! Un seul `AppState` est instancié par processus et partagé entre la
//! couche Tauri (commandes `#[tauri::command]`) et le serveur HTTP
//! embarqué quand il tourne — cela permet à un client web de voir les
//! scans lancés depuis le desktop, et inversement.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use lanprobe_core::discovery::DiscoveredHost;
use lanprobe_core::internet::InternetHistory;
use lanprobe_core::ping::PingResult;
use lanprobe_core::ports::PortResult;
use lanprobe_core::speedtest::SpeedResult;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::auth::AuthStore;
use crate::config::ConfigStore;

pub type SelectedInterface = Arc<Mutex<Option<String>>>;
pub type PingStopMap = Arc<Mutex<HashMap<String, bool>>>;
pub type ScanCancel = Arc<AtomicBool>;

/// Event broadcast à tous les clients WebSocket connectés et, côté
/// desktop, mirroré vers `app.emit()` pour alimenter le frontend Tauri.
#[derive(Debug, Clone, Serialize)]
pub struct BroadcastEvent {
    pub event: String,
    pub payload: serde_json::Value,
}

/// Merge-state des scans réseau : chaque nouveau host (event `discovery:host`
/// ou mise à jour MAC / latence) est fusionné dans une map {ip → host}.
/// Permet à un client fraîchement connecté (web comme desktop) de
/// récupérer d'un seul coup les résultats en cours via un snapshot.
#[derive(Debug, Default)]
pub struct DiscoveryStateInner {
    hosts: Mutex<HashMap<String, DiscoveredHost>>,
}

impl DiscoveryStateInner {
    pub fn clear(&self) {
        if let Ok(mut g) = self.hosts.lock() {
            g.clear();
        }
    }
    pub fn upsert(&self, host: DiscoveredHost) {
        let Ok(mut g) = self.hosts.lock() else { return };
        let entry = g.entry(host.ip.clone()).or_insert_with(|| host.clone());
        if host.hostname.is_some() {
            entry.hostname = host.hostname.clone();
        }
        if host.mac.is_some() {
            entry.mac = host.mac.clone();
        }
        if host.latency_ms.is_some() {
            entry.latency_ms = host.latency_ms;
        }
    }
    pub fn update_latency(&self, ip: &str, latency_ms: u64) {
        if let Ok(mut g) = self.hosts.lock() {
            if let Some(h) = g.get_mut(ip) {
                h.latency_ms = Some(latency_ms);
            }
        }
    }
    pub fn update_mac(&self, ip: &str, mac: String) {
        if let Ok(mut g) = self.hosts.lock() {
            if let Some(h) = g.get_mut(ip) {
                h.mac = Some(mac);
            }
        }
    }
    pub fn snapshot(&self) -> Vec<DiscoveredHost> {
        self.hosts
            .lock()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }
}

pub type DiscoveryState = Arc<DiscoveryStateInner>;

/// Ring buffer par IP de tous les samples de ping monitoring. Borné pour
/// ne pas exploser la RAM quand un monitoring tourne pendant des heures.
const MONITOR_MAX_SAMPLES: usize = 3600; // 1 h @ 1s

#[derive(Debug, Default)]
pub struct MonitoringStateInner {
    samples: Mutex<HashMap<String, VecDeque<PingResult>>>,
}

impl MonitoringStateInner {
    pub fn push(&self, result: PingResult) {
        if let Ok(mut g) = self.samples.lock() {
            let q = g.entry(result.ip.clone()).or_default();
            q.push_back(result);
            while q.len() > MONITOR_MAX_SAMPLES {
                q.pop_front();
            }
        }
    }
    pub fn snapshot(&self) -> HashMap<String, Vec<PingResult>> {
        self.samples
            .lock()
            .map(|g| g.iter().map(|(k, v)| (k.clone(), v.iter().cloned().collect())).collect())
            .unwrap_or_default()
    }
    pub fn clear_ip(&self, ip: &str) {
        if let Ok(mut g) = self.samples.lock() {
            g.remove(ip);
        }
    }
}

pub type MonitoringState = Arc<MonitoringStateInner>;

/// Résultat d'un scan de ports (TCP + UDP) pour une IP donnée, partagé entre
/// le desktop et les clients web : un nouveau client récupère l'état complet
/// au démarrage via `cmd_get_portscan_snapshot`, puis écoute `portscan:update`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct PortScanEntry {
    pub ip: String,
    pub tcp: Vec<PortResult>,
    pub udp: Vec<PortResult>,
    pub timestamp: u64,
    pub profile_id: Option<String>,
    pub in_progress: bool,
}

#[derive(Debug, Default)]
pub struct PortScanStateInner {
    entries: Mutex<HashMap<String, PortScanEntry>>,
}

impl PortScanStateInner {
    pub fn upsert(&self, entry: PortScanEntry) {
        if let Ok(mut g) = self.entries.lock() {
            g.insert(entry.ip.clone(), entry);
        }
    }
    pub fn mark_in_progress(&self, ip: &str, profile_id: Option<String>) {
        if let Ok(mut g) = self.entries.lock() {
            let e = g.entry(ip.to_string()).or_insert_with(|| PortScanEntry {
                ip: ip.to_string(),
                ..Default::default()
            });
            e.in_progress = true;
            if profile_id.is_some() {
                e.profile_id = profile_id;
            }
        }
    }
    pub fn set_tcp(&self, ip: &str, tcp: Vec<PortResult>, timestamp: u64, profile_id: Option<String>) -> PortScanEntry {
        let mut out = PortScanEntry::default();
        if let Ok(mut g) = self.entries.lock() {
            let e = g.entry(ip.to_string()).or_insert_with(|| PortScanEntry {
                ip: ip.to_string(),
                ..Default::default()
            });
            e.tcp = tcp;
            e.timestamp = timestamp;
            e.in_progress = false;
            if profile_id.is_some() {
                e.profile_id = profile_id;
            }
            out = e.clone();
        }
        out
    }
    pub fn set_udp(&self, ip: &str, udp: Vec<PortResult>, timestamp: u64) -> PortScanEntry {
        let mut out = PortScanEntry::default();
        if let Ok(mut g) = self.entries.lock() {
            let e = g.entry(ip.to_string()).or_insert_with(|| PortScanEntry {
                ip: ip.to_string(),
                ..Default::default()
            });
            e.udp = udp;
            e.timestamp = timestamp;
            out = e.clone();
        }
        out
    }
    pub fn remove(&self, ip: &str) {
        if let Ok(mut g) = self.entries.lock() {
            g.remove(ip);
        }
    }
    pub fn snapshot(&self) -> Vec<PortScanEntry> {
        self.entries
            .lock()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }
}

pub type PortScanState = Arc<PortScanStateInner>;

/// Dernier résultat de speedtest (Ookla ou iperf3). Un seul slot : les tests
/// sont mutuellement exclusifs et on n'a pas besoin d'historique côté backend
/// (le frontend garde son propre historique local).
#[derive(Debug, Default)]
pub struct SpeedTestStateInner {
    latest: Mutex<Option<SpeedResult>>,
    running: Mutex<bool>,
    cancel: Arc<tokio::sync::Notify>,
}

impl SpeedTestStateInner {
    /// Handle à attendre dans un `tokio::select!` côté commande : quand il est
    /// notifié, on abandonne le future du speedtest (le process est tué via
    /// `kill_on_drop`).
    pub fn cancel_handle(&self) -> Arc<tokio::sync::Notify> {
        self.cancel.clone()
    }
    /// Demande l'annulation du test en cours.
    pub fn request_cancel(&self) {
        self.cancel.notify_waiters();
        self.mark_stopped();
    }
    pub fn set(&self, result: SpeedResult) {
        if let Ok(mut g) = self.latest.lock() {
            *g = Some(result);
        }
        if let Ok(mut r) = self.running.lock() {
            *r = false;
        }
    }
    pub fn mark_running(&self) {
        if let Ok(mut r) = self.running.lock() {
            *r = true;
        }
    }
    pub fn mark_stopped(&self) {
        if let Ok(mut r) = self.running.lock() {
            *r = false;
        }
    }
    pub fn snapshot(&self) -> Option<SpeedResult> {
        self.latest.lock().ok().and_then(|g| g.clone())
    }
    pub fn is_running(&self) -> bool {
        self.running.lock().map(|r| *r).unwrap_or(false)
    }
}

pub type SpeedTestState = Arc<SpeedTestStateInner>;

#[derive(Clone)]
pub struct AppState {
    pub selected_interface: SelectedInterface,
    pub ping_stop: PingStopMap,
    pub scan_cancel: ScanCancel,
    pub internet: Arc<InternetHistory>,
    pub discovery: DiscoveryState,
    pub monitoring: MonitoringState,
    pub portscan: PortScanState,
    pub speedtest: SpeedTestState,
    pub events: broadcast::Sender<BroadcastEvent>,
    pub auth: Arc<AuthStore>,
    pub config: Arc<ConfigStore>,
    /// `true` quand l'AppState est créé par le binaire `lanprobe-server`
    /// standalone (mode headless). `false` quand il est créé par le shell
    /// Tauri et partagé entre le desktop et un serveur web embarqué —
    /// dans ce cas l'update est gérée par l'app desktop elle-même.
    pub is_headless: bool,
    /// Fenêtre pendant laquelle les pings ne sont pas enregistrés dans
    /// l'historique de monitoring — évite les faux outages lors d'un
    /// changement de profil réseau (l'interface est brièvement down).
    pub monitoring_blackout_until: Arc<Mutex<Option<Instant>>>,
}

impl AppState {
    pub fn new(auth: Arc<AuthStore>, config: Arc<ConfigStore>) -> Self {
        Self::new_with_mode(auth, config, false)
    }
    pub fn new_headless(auth: Arc<AuthStore>, config: Arc<ConfigStore>) -> Self {
        Self::new_with_mode(auth, config, true)
    }
    fn new_with_mode(auth: Arc<AuthStore>, config: Arc<ConfigStore>, is_headless: bool) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            selected_interface: Arc::new(Mutex::new(None)),
            ping_stop: Arc::new(Mutex::new(HashMap::new())),
            // Initialisé à `true` (idle) : `false` signifie "un scan est en
            // cours", `true` signifie "idle / annulation demandée". Ainsi le
            // scheduler peut utiliser un CAS(true→false) pour détecter
            // atomiquement qu'aucun scan concurrent n'est en cours.
            scan_cancel: Arc::new(AtomicBool::new(true)),
            internet: Arc::new(InternetHistory::default()),
            discovery: Arc::new(DiscoveryStateInner::default()),
            monitoring: Arc::new(MonitoringStateInner::default()),
            portscan: Arc::new(PortScanStateInner::default()),
            speedtest: Arc::new(SpeedTestStateInner::default()),
            events,
            auth,
            config,
            is_headless,
            monitoring_blackout_until: Arc::new(Mutex::new(None)),
        }
    }

    /// Pose un blackout sur le monitoring pendant `duration`.
    /// Les pings continuent de tourner mais n'écrivent pas dans l'historique.
    pub fn set_monitoring_blackout(&self, duration: std::time::Duration) {
        if let Ok(mut guard) = self.monitoring_blackout_until.lock() {
            *guard = Some(Instant::now() + duration);
        }
    }

    /// Lève le blackout immédiatement (appelé dès que l'interface est up).
    pub fn clear_monitoring_blackout(&self) {
        if let Ok(mut guard) = self.monitoring_blackout_until.lock() {
            *guard = None;
        }
    }

    /// Retourne `true` si on est dans la fenêtre de blackout.
    pub fn is_monitoring_blackout(&self) -> bool {
        self.monitoring_blackout_until
            .lock()
            .ok()
            .and_then(|g| *g)
            .map(|until| Instant::now() < until)
            .unwrap_or(false)
    }

    /// Envoie un event JSON sur le bus partagé. Aucun effet si plus
    /// aucun abonné — on avale silencieusement l'erreur.
    pub fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = self.events.send(BroadcastEvent {
            event: event.into(),
            payload,
        });
    }
}
