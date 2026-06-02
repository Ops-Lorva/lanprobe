//! Planificateur automatique — lance des speedtests, découvertes réseau et
//! scans de ports à intervalles configurables.
//!
//! Chaque type de tâche tourne dans une sous-tâche Tokio indépendante. Le
//! planificateur écoute les events `config:update` sur le bus partagé et
//! redémarre ses sous-tâches à chaud si la configuration du scheduler change.
//!
//! Config lue depuis `AppState::config` (clé `"scheduler"` dans
//! `app_config.json`).

use std::net::Ipv4Addr;
use std::sync::atomic::Ordering;

use lanprobe_core::discovery::{
    get_hostname, get_local_network_cidr, parse_cidr, read_arp_table, DiscoveredHost,
};
use lanprobe_core::interfaces::get_interface_details;
use lanprobe_core::ports::scan_ports;
use serde_json::json;

use crate::state::AppState;

// ── Config structs ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct SchedulerConfig {
    /// Intervalle entre deux speedtests automatiques, en minutes.
    /// 0 = désactivé.
    #[serde(default)]
    pub speedtest_interval_min: u64,

    /// Intervalle entre deux découvertes réseau automatiques, en minutes.
    /// 0 = désactivé.
    #[serde(default)]
    pub discovery_interval_min: u64,

    /// CIDR à scanner. Vide = auto-détection via l'interface sélectionnée.
    #[serde(default)]
    pub discovery_cidr: String,

    /// Intervalle entre deux scans de ports automatiques, en minutes.
    /// 0 = désactivé.
    #[serde(default)]
    pub portscan_interval_min: u64,

    /// IPs à scanner. Vide = désactivé même si l'intervalle est > 0.
    #[serde(default)]
    pub portscan_targets: Vec<String>,
}

impl SchedulerConfig {
    pub fn speedtest_enabled(&self) -> bool {
        self.speedtest_interval_min > 0
    }

    pub fn discovery_enabled(&self) -> bool {
        self.discovery_interval_min > 0
    }

    pub fn portscan_enabled(&self) -> bool {
        self.portscan_interval_min > 0 && !self.portscan_targets.is_empty()
    }
}

// ── Config loader ──────────────────────────────────────────────────────────

fn load_config(state: &AppState) -> SchedulerConfig {
    let cfg_value = state.config.get();
    cfg_value
        .get("scheduler")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn config_changed(old: &SchedulerConfig, new: &SchedulerConfig) -> bool {
    old.speedtest_interval_min != new.speedtest_interval_min
        || old.discovery_interval_min != new.discovery_interval_min
        || old.discovery_cidr != new.discovery_cidr
        || old.portscan_interval_min != new.portscan_interval_min
        || old.portscan_targets != new.portscan_targets
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_ip_u32(ip: &str) -> Option<u32> {
    ip.split('.').fold(Some(0u32), |acc, p| {
        acc.and_then(|a| p.parse::<u8>().ok().map(|b| (a << 8) | b as u32))
    })
}

// ── Sub-task launchers ─────────────────────────────────────────────────────

fn start_sub_tasks(cfg: &SchedulerConfig, state: &AppState) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();

    if cfg.speedtest_enabled() {
        let s = state.clone();
        let interval_min = cfg.speedtest_interval_min;
        handles.push(tokio::spawn(run_speedtest_task(s, interval_min)));
    }

    if cfg.discovery_enabled() {
        let s = state.clone();
        let interval_min = cfg.discovery_interval_min;
        let cidr = cfg.discovery_cidr.clone();
        handles.push(tokio::spawn(run_discovery_task(s, interval_min, cidr)));
    }

    if cfg.portscan_enabled() {
        let s = state.clone();
        let interval_min = cfg.portscan_interval_min;
        let targets = cfg.portscan_targets.clone();
        handles.push(tokio::spawn(run_portscan_task(s, interval_min, targets)));
    }

    handles
}

// ── Speedtest sub-task ─────────────────────────────────────────────────────

async fn run_speedtest_task(state: AppState, interval_min: u64) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_min * 60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;

        // Lire l'engine et les paramètres depuis la config courante.
        let engine = {
            let cfg_val = state.config.get();
            cfg_val["speedtestEngine"].as_str().unwrap_or("ookla").to_string()
        };

        tracing::info!("Scheduler: running scheduled speedtest (engine={})", engine);

        state.speedtest.mark_running();
        let _ = state.events.send(crate::state::BroadcastEvent {
            event: "speedtest:running".into(),
            payload: json!({ "running": true }),
        });

        let result = if engine == "iperf3" {
            let server = {
                let cfg_val = state.config.get();
                cfg_val["iperfServer"].as_str().unwrap_or("").to_string()
            };
            // Résoudre l'IP source depuis l'interface sélectionnée.
            let src = resolve_src(&state);
            lanprobe_core::iperf::run_iperf3(&server, src).await
        } else {
            // Ookla — run_speedtest gère l'interface sélectionnée elle-même.
            let src = resolve_src(&state);
            let iface_name = get_selected_iface_name(&state);
            let iface_for_cli = iface_name.as_ref().map(|n| {
                #[cfg(target_os = "macos")]
                { get_interface_details(n).bsd_name.unwrap_or(n.clone()) }
                #[cfg(not(target_os = "macos"))]
                { n.clone() }
            });
            lanprobe_core::speedtest::run_speedtest(src, iface_for_cli).await
        };

        match result {
            Ok(r) => {
                state.speedtest.set(r.clone());
                let _ = state.events.send(crate::state::BroadcastEvent {
                    event: "speedtest:result".into(),
                    payload: serde_json::to_value(&r).unwrap_or(serde_json::Value::Null),
                });
                tracing::info!(
                    "Scheduler: speedtest done — dl={:.1} ul={:.1} lat={}ms",
                    r.download_mbps, r.upload_mbps, r.latency_ms
                );
            }
            Err(e) => {
                state.speedtest.mark_stopped();
                let _ = state.events.send(crate::state::BroadcastEvent {
                    event: "speedtest:running".into(),
                    payload: json!({ "running": false }),
                });
                tracing::warn!("Scheduler: scheduled speedtest failed: {}", e);
            }
        }
    }
}

// ── Discovery sub-task ─────────────────────────────────────────────────────

async fn run_discovery_task(state: AppState, interval_min: u64, cidr: String) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_min * 60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;

        // Guard contre la concurrence : on utilise un CAS pour s'assurer
        // qu'aucun autre scan (déclenché manuellement ou par le scheduler)
        // n'est en cours. `scan_cancel == true` signifie "idle" ; `false`
        // signifie "un scan tourne". On ne procède que si on peut passer
        // atomiquement de `true` (idle) à `false` (scan actif).
        //
        // Si le CAS échoue c'est qu'un scan est déjà en cours → on saute
        // ce tick plutôt que de clobber l'état partagé.
        if state
            .scan_cancel
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::warn!("Scheduler: discovery scan skipped — another scan is in progress");
            continue;
        }

        // Déterminer le CIDR effectif : configuré ou auto-détecté.
        let effective_cidr = if cidr.is_empty() {
            // Même logique que cmd_get_local_network_cidr dans routes.rs :
            // d'abord depuis l'interface sélectionnée, sinon fallback `get_local_network_cidr`.
            let from_iface = try_cidr_from_selected_iface(&state);
            let detected = from_iface.or_else(get_local_network_cidr);
            match detected {
                Some(c) => c,
                None => {
                    tracing::warn!("Scheduler discovery: failed to auto-detect CIDR, skipping");
                    // Remettre scan_cancel à true (idle) puisqu'on n'a pas démarré.
                    state.scan_cancel.store(true, Ordering::SeqCst);
                    continue;
                }
            }
        } else {
            cidr.clone()
        };

        tracing::info!("Scheduler: running scheduled discovery on {}", effective_cidr);

        let (first, last) = match parse_cidr(&effective_cidr) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Scheduler discovery: invalid CIDR {}: {}", effective_cidr, e);
                state.scan_cancel.store(true, Ordering::SeqCst);
                continue;
            }
        };

        let src = resolve_src(&state);

        // Réinitialiser le store de découverte pour ce nouveau scan.
        state.discovery.clear();

        // — La logique de scan tourne directement ici, dans la boucle, sans
        //   inner `tokio::spawn`. Puisque `run_discovery_task` est déjà dans
        //   sa propre sous-tâche, un second spawn créerait une course : la
        //   boucle pourrait avancer au tick suivant avant la fin du scan
        //   précédent et clobberer l'état partagé.

        // Étape 1 : ARP initial.
        let arp_initial = read_arp_table().await;
        if state.scan_cancel.load(Ordering::SeqCst) {
            let _ = state.events.send(done_event(&effective_cidr, 0));
            state.scan_cancel.store(true, Ordering::SeqCst);
            continue;
        }
        for (ip, mac) in &arp_initial {
            if let Some(i) = parse_ip_u32(ip) {
                if i >= first && i <= last {
                    let host = DiscoveredHost {
                        ip: ip.clone(),
                        hostname: None,
                        mac: Some(mac.clone()),
                        vendor: lanprobe_core::oui::vendor_for_mac(mac),
                        latency_ms: None,
                    };
                    state.discovery.upsert(host.clone());
                    let _ = state.events.send(crate::state::BroadcastEvent {
                        event: "discovery:host".into(),
                        payload: serde_json::to_value(&host)
                            .unwrap_or(serde_json::Value::Null),
                    });
                }
            }
        }

        // Étape 2 : ping sweep en chunks parallèles.
        #[cfg(target_os = "windows")]
        let chunk_size = 32usize;
        #[cfg(not(target_os = "windows"))]
        let chunk_size = 128usize;

        let all_ips: Vec<String> = (first..=last)
            .map(|i| Ipv4Addr::from(i).to_string())
            .collect();

        for chunk in all_ips.chunks(chunk_size) {
            if state.scan_cancel.load(Ordering::SeqCst) {
                break;
            }
            let mut handles = vec![];
            for ip in chunk {
                let ip = ip.clone();
                let arp_mac = arp_initial.get(&ip).cloned();
                let events_c = state.events.clone();
                let discovery_c = state.discovery.clone();
                handles.push(tokio::spawn(async move {
                    if let Some(lat) =
                        lanprobe_core::ping::ping_once_fast_retry(&ip, src, 3).await
                    {
                        if arp_mac.is_none() {
                            let hostname = get_hostname(&ip).await;
                            let host = DiscoveredHost {
                                ip: ip.clone(),
                                hostname,
                                mac: None,
                                vendor: None,
                                latency_ms: Some(lat),
                            };
                            discovery_c.upsert(host.clone());
                            let _ = events_c.send(crate::state::BroadcastEvent {
                                event: "discovery:host".into(),
                                payload: serde_json::to_value(&host)
                                    .unwrap_or(serde_json::Value::Null),
                            });
                        } else {
                            discovery_c.update_latency(&ip, lat);
                            let _ = events_c.send(crate::state::BroadcastEvent {
                                event: "discovery:host_latency".into(),
                                payload: json!({ "ip": ip, "latency_ms": lat }),
                            });
                        }
                    }
                }));
            }
            for h in handles {
                let _ = h.await;
            }
        }

        if state.scan_cancel.load(Ordering::SeqCst) {
            let _ = state.events.send(done_event(&effective_cidr, 0));
            state.scan_cancel.store(true, Ordering::SeqCst);
            continue;
        }

        // Étape 3 : ARP final pour récupérer les MACs des hôtes pingés.
        let arp_after = read_arp_table().await;
        for (ip, mac) in &arp_after {
            if arp_initial.contains_key(ip) {
                continue;
            }
            if let Some(i) = parse_ip_u32(ip) {
                if i >= first && i <= last {
                    state.discovery.update_mac(ip, mac.clone());
                    let _ = state.events.send(crate::state::BroadcastEvent {
                        event: "discovery:host_mac".into(),
                        payload: json!({
                            "ip": ip,
                            "mac": mac,
                            "vendor": lanprobe_core::oui::vendor_for_mac(mac),
                        }),
                    });
                }
            }
        }

        let hosts_found = state.discovery.snapshot().len();
        tracing::info!(
            "Scheduler: discovery done on {} — {} hosts found",
            effective_cidr,
            hosts_found
        );
        let _ = state.events.send(done_event(&effective_cidr, hosts_found));

        // Remettre scan_cancel à true (idle) une fois le scan terminé.
        state.scan_cancel.store(true, Ordering::SeqCst);
    }
}

// ── Port scan sub-task ─────────────────────────────────────────────────────

async fn run_portscan_task(state: AppState, interval_min: u64, targets: Vec<String>) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_min * 60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;

        tracing::info!("Scheduler: running scheduled port scan on {} targets", targets.len());

        for target in &targets {
            // Valider que la cible est une adresse IPv4 valide avant de lancer
            // le scan. Une entrée malformée dans la config ferait échouer
            // `scan_ports` silencieusement (tous les ports retournés open=false) ;
            // mieux vaut loguer et ignorer explicitement.
            if target.parse::<std::net::Ipv4Addr>().is_err() {
                tracing::warn!("Scheduler: invalid portscan target {:?}, skipping", target);
                continue;
            }

            let ip = target.clone();
            let src = resolve_src(&state);

            // Marquer le scan comme en cours et envoyer l'event.
            state.portscan.mark_in_progress(&ip, None);
            let _ = state.events.send(crate::state::BroadcastEvent {
                event: "portscan:update".into(),
                payload: json!({ "ip": ip, "in_progress": true, "profile_id": null }),
            });

            // Scanner les ports TCP (ports par défaut = None → profil par défaut).
            let results = scan_ports(&ip, src, None).await;
            let entry = state
                .portscan
                .set_tcp(&ip, results, now_secs(), None);
            let _ = state.events.send(crate::state::BroadcastEvent {
                event: "portscan:update".into(),
                payload: serde_json::to_value(&entry).unwrap_or(serde_json::Value::Null),
            });

            tracing::info!(
                "Scheduler: port scan done on {} — {} TCP open",
                ip,
                entry.tcp.len()
            );
        }
    }
}

// ── Interface resolution helpers ───────────────────────────────────────────

/// Retourne l'IP source de l'interface sélectionnée, ou `None`.
///
/// # Comportement quand `None` est retourné
///
/// Aucune interface n'est sélectionnée (ou l'interface sélectionnée n'a pas
/// d'adresse IPv4). Les appelants passent cette valeur directement à
/// `scan_ports` / `scan_udp_ports` / `run_iperf3` / `run_speedtest`.
///
/// - `scan_ports` / `scan_udp_ports` : si `src` est `None`, le socket TCP/UDP
///   n'est pas bindé à une adresse source particulière — le système
///   d'exploitation choisit l'interface de sortie automatiquement (comportement
///   équivalent à `bind("0.0.0.0:0")`). Cela ne provoque pas de panique ni
///   d'erreur ; les scans fonctionnent mais partent possiblement par une autre
///   interface que celle attendue par l'utilisateur.
/// - `run_iperf3` / `run_speedtest` : même sémantique — `None` est interprété
///   comme "laisser l'OS choisir".
///
/// En résumé : `None` est sûr, mais peut donner des résultats mesurés sur une
/// interface non désirée si plusieurs interfaces sont présentes.
fn resolve_src(state: &AppState) -> Option<Ipv4Addr> {
    let name = state
        .selected_interface
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()?;
    let details = get_interface_details(&name);
    details.ip?.parse::<Ipv4Addr>().ok()
}

/// Retourne le nom de l'interface sélectionnée, ou `None`.
fn get_selected_iface_name(state: &AppState) -> Option<String> {
    state
        .selected_interface
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

/// Calcule le CIDR de l'interface sélectionnée (IP + masque).
/// Retourne `None` si aucune interface n'est sélectionnée ou si elle n'a
/// pas d'adresse IPv4 + masque.
fn try_cidr_from_selected_iface(state: &AppState) -> Option<String> {
    let name = get_selected_iface_name(state)?;
    let d = get_interface_details(&name);
    let ip = d.ip?;
    let mask = d.subnet?;
    cidr_from_ip_mask(&ip, &mask)
}

fn cidr_from_ip_mask(ip: &str, mask: &str) -> Option<String> {
    let ip_parts: Vec<u8> = ip.split('.').filter_map(|p| p.parse().ok()).collect();
    let mask_parts: Vec<u8> = mask.split('.').filter_map(|p| p.parse().ok()).collect();
    if ip_parts.len() != 4 || mask_parts.len() != 4 {
        return None;
    }
    let ip_int = u32::from_be_bytes([ip_parts[0], ip_parts[1], ip_parts[2], ip_parts[3]]);
    let mask_int = u32::from_be_bytes([mask_parts[0], mask_parts[1], mask_parts[2], mask_parts[3]]);
    let prefix = mask_int.count_ones();
    let net = Ipv4Addr::from(ip_int & mask_int);
    Some(format!("{}/{}", net, prefix))
}

fn done_event(cidr: &str, hosts_found: usize) -> crate::state::BroadcastEvent {
    crate::state::BroadcastEvent {
        event: "discovery:done".into(),
        payload: json!({ "cidr": cidr, "hosts_found": hosts_found }),
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Tâche de fond — orchestre les sous-tâches planifiées et écoute les
/// `config:update` pour recharger à chaud.
pub async fn run(state: AppState) {
    // S'abonner AVANT de lire la config pour ne rater aucun event.
    let mut rx = state.events.subscribe();

    // Charger la config initiale et démarrer les sous-tâches.
    let mut cfg = load_config(&state);
    let mut handles = start_sub_tasks(&cfg, &state);

    tracing::info!(
        "Scheduler started — speedtest={} discovery={} portscan={}",
        cfg.speedtest_enabled(),
        cfg.discovery_enabled(),
        cfg.portscan_enabled()
    );

    loop {
        match rx.recv().await {
            Ok(event) if event.event == "config:update" => {
                let new_cfg = load_config(&state);
                if config_changed(&cfg, &new_cfg) {
                    tracing::info!("Scheduler: config changed, restarting sub-tasks");
                    for h in handles.drain(..) {
                        h.abort();
                    }
                    cfg = new_cfg;
                    handles = start_sub_tasks(&cfg, &state);
                    tracing::info!(
                        "Scheduler restarted — speedtest={} discovery={} portscan={}",
                        cfg.speedtest_enabled(),
                        cfg.discovery_enabled(),
                        cfg.portscan_enabled()
                    );
                }
            }
            Ok(_) => {} // ignorer les autres events
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                tracing::info!("Scheduler: broadcast channel closed, shutting down");
                for h in handles.drain(..) {
                    h.abort();
                }
                return;
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("Scheduler: broadcast lagged, {} events dropped", n);
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduler_config_default() {
        let cfg = SchedulerConfig::default();
        assert_eq!(cfg.speedtest_interval_min, 0);
        assert_eq!(cfg.discovery_interval_min, 0);
        assert_eq!(cfg.portscan_interval_min, 0);
        assert!(cfg.discovery_cidr.is_empty());
        assert!(cfg.portscan_targets.is_empty());

        assert!(!cfg.speedtest_enabled());
        assert!(!cfg.discovery_enabled());
        assert!(!cfg.portscan_enabled());
    }

    #[test]
    fn test_scheduler_config_enabled() {
        let cfg = SchedulerConfig {
            speedtest_interval_min: 60,
            discovery_interval_min: 30,
            discovery_cidr: "192.168.1.0/24".to_string(),
            portscan_interval_min: 120,
            portscan_targets: vec!["192.168.1.1".to_string()],
        };
        assert!(cfg.speedtest_enabled());
        assert!(cfg.discovery_enabled());
        assert!(cfg.portscan_enabled());
    }

    #[test]
    fn test_portscan_disabled_empty_targets() {
        let cfg = SchedulerConfig {
            portscan_interval_min: 60,
            portscan_targets: vec![],
            ..Default::default()
        };
        // Intervalle > 0 mais pas de cibles → désactivé.
        assert!(!cfg.portscan_enabled());
    }

    #[test]
    fn test_config_changed() {
        let base = SchedulerConfig {
            speedtest_interval_min: 60,
            discovery_interval_min: 30,
            discovery_cidr: "192.168.1.0/24".to_string(),
            portscan_interval_min: 120,
            portscan_targets: vec!["192.168.1.1".to_string()],
        };

        // Pas de changement.
        assert!(!config_changed(&base, &base.clone()));

        // Changement sur speedtest_interval_min.
        let mut changed = base.clone();
        changed.speedtest_interval_min = 120;
        assert!(config_changed(&base, &changed));

        // Changement sur discovery_interval_min.
        let mut changed = base.clone();
        changed.discovery_interval_min = 60;
        assert!(config_changed(&base, &changed));

        // Changement sur discovery_cidr.
        let mut changed = base.clone();
        changed.discovery_cidr = "10.0.0.0/8".to_string();
        assert!(config_changed(&base, &changed));

        // Changement sur portscan_interval_min.
        let mut changed = base.clone();
        changed.portscan_interval_min = 60;
        assert!(config_changed(&base, &changed));

        // Changement sur portscan_targets.
        let mut changed = base.clone();
        changed.portscan_targets = vec!["10.0.0.1".to_string()];
        assert!(config_changed(&base, &changed));
    }
}
