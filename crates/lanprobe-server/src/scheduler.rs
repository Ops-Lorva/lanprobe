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
    get_hostname, get_local_network_cidr, parse_cidr, read_arp_table, scan_interface,
    DiscoveredHost,
};
use lanprobe_core::interfaces::get_interface_details;
use lanprobe_core::ports::{scan_ports, scan_udp_ports};
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

/// Lance un test de débit **maintenant**, avec le moteur configuré.
///
/// Extraite de la tâche planifiée pour que le hub puisse la déclencher
/// (contrat § 14) : deux implémentations d'un même test finiraient par
/// diverger sur la contrainte d'interface, qui est justement ce qui rend la
/// mesure honnête.
/// Clé de scellement, pour publier l'inventaire au hub.
///
/// Relue à chaque scan plutôt que gardée : les scans sont rares, et une clé
/// tenue en mémoire pour la durée du processus survivrait à un ré-enrôlement.
/// Envoie au hub la découverte qui vient de se terminer.
///
/// 🔴 **Appelée par les DEUX chemins, et c'est tout l'objet de cette
/// fonction.** Un balayage lancé depuis l'interface de la sonde ne partait
/// nulle part : seuls ceux venus d'un ordre du hub ou de l'ordonnanceur étaient
/// publiés. On voyait donc, depuis le téléphone, un inventaire vieux de deux
/// heures pendant que l'écran de la sonde affichait le bon — sans rien qui
/// explique l'écart. Constaté le 30/09.
///
/// ⚠️ Sans clé de scellement, il n'y a pas de hub à qui parler : la sonde
/// travaille alors seule, et ce n'est pas une erreur.
pub async fn publish_discovery(state: &AppState, cidr: &str) {
    let Some(key) = sealing_key(state) else { return };
    // ⚠️ Le compte est journalisé : l'écart entre ce qu'une fenêtre affiche et
    // ce que le hub reçoit a demandé une enquête en base de données le 01/10.
    // Il tient en une ligne de journal.
    tracing::info!(
        "découverte publiée au hub : {} machines",
        state.discovery.snapshot().len()
    );
    let hosts = state
        .discovery
        .snapshot()
        .into_iter()
        .map(|h| crate::inventory::ScanHost {
            ip: h.ip,
            hostname: h.hostname,
            mac: h.mac,
            vendor: h.vendor,
            latency_ms: h.latency_ms.map(|v| v as i64),
            // ⚠️ Une découverte date son lot et rien d'autre : toutes ses
            // machines ont été vues dans le même balayage. Mettre ici le
            // `started_at` du rapport n'apprendrait rien, et inventer une date
            // par machine serait faux.
            scanned_at: None,
        })
        .collect();
    crate::inventory::publish(
        state,
        &key,
        crate::inventory::ScanReport {
            kind: "discovery".into(),
            started_at: crate::inventory::now(),
            cidr: Some(cidr.to_string()),
            hosts,
            ports: Vec::new(),
            speedtest: None,
        },
    )
    .await;
}

/// Envoie au hub les ports ouverts d'une machine.
///
/// 🔴 **Appelée par les deux chemins**, comme `publish_discovery` : un scan
/// lancé depuis la fenêtre de la sonde ne partait nulle part, et le hub — donc
/// le téléphone — montrait l'inventaire précédent sans dire qu'il était vieux.
///
/// ⚠️ Seuls les ports OUVERTS partent : publier les milliers de ports fermés
/// d'un scan complet gonflerait l'inventaire sans rien apprendre.
///
/// ⚠️ Sans clé de scellement, il n'y a pas de hub à qui parler.
pub async fn publish_ports(state: &AppState, _just_scanned: &str) {
    let Some(key) = sealing_key(state) else { return };
    // 🔴 **Toutes les machines connues, pas seulement celle qu'on vient de
    // scanner.** Chaque publication crée un scan ENTIER côté hub, et le hub
    // n'affiche que le dernier : ne publier qu'une machine faisait disparaître
    // toutes les précédentes à chaque nouveau scan. Deux machines scannées sur
    // la sonde, une seule visible sur le hub — constaté le 02/10.
    //
    // ⚠️ Seuls les ports OUVERTS partent : publier les milliers de ports fermés
    // d'un scan complet gonflerait l'inventaire sans rien apprendre.
    let entries = state.portscan.snapshot();
    let (hosts, ports) = inventory_from_entries(&entries);
    if hosts.is_empty() {
        return;
    }
    tracing::info!(
        "scan de ports publié au hub : {} machines, {} ports ouverts",
        hosts.len(),
        ports.len()
    );
    crate::inventory::publish(
        state,
        &key,
        crate::inventory::ScanReport {
            kind: "ports".into(),
            started_at: crate::inventory::now(),
            cidr: None,
            hosts,
            ports,
            speedtest: None,
        },
    )
    .await;
}

/// Les machines et les ports ouverts à publier, depuis l'état local des scans.
///
/// 🔴 **Chaque machine porte SA date.** Depuis que la sonde publie toutes les
/// machines qu'elle connaît à chaque scan — sans quoi le hub, qui n'affiche que
/// le dernier scan, perdait les précédentes —, le `started_at` du rapport est
/// celui du LOT. S'en servir pour dater les machines ferait passer une machine
/// scannée il y a une heure pour scannée à l'instant : une valeur plausible et
/// fausse, c'est-à-dire la pire.
///
/// ⚠️ Une machine sans horodatage (entrée créée par un scan en cours) part
/// **sans date**. Publier `0` la daterait du 1er janvier 1970 ; l'écran du hub
/// doit pouvoir dire « date inconnue », et c'est l'absence qui le permet.
fn inventory_from_entries(
    entries: &[crate::state::PortScanEntry],
) -> (Vec<crate::inventory::ScanHost>, Vec<crate::inventory::ScanPort>) {
    let mut hosts = Vec::new();
    let mut ports = Vec::new();
    for entry in entries {
        hosts.push(crate::inventory::ScanHost {
            ip: entry.ip.clone(),
            hostname: None,
            mac: None,
            vendor: None,
            latency_ms: None,
            scanned_at: (entry.timestamp > 0).then_some(entry.timestamp as i64),
        });
        // ⚠️ Seuls les ports OUVERTS : publier les milliers de ports fermés
        // d'un scan complet gonflerait l'inventaire sans rien apprendre.
        for p in entry.tcp.iter().chain(entry.udp.iter()).filter(|p| p.open) {
            ports.push(crate::inventory::ScanPort {
                ip: entry.ip.clone(),
                port: p.port,
                proto: p.proto.clone(),
                service: (!p.service.is_empty()).then(|| p.service.clone()),
            });
        }
    }
    (hosts, ports)
}

fn sealing_key(state: &AppState) -> Option<crate::secrets::SecretKey> {
    crate::secrets::load_or_create_key(&state.config.dir()).ok()
}

pub async fn speedtest_once(state: &AppState) {
    speedtest_with(state, None, None).await
}

/// Idem, en imposant le moteur et le serveur.
///
/// ⚠️ Les surcharges viennent d'une commande du hub, et ne touchent PAS la
/// configuration de la sonde : un test lancé à distance ne doit pas changer ce
/// que fera le prochain test planifié. Le hub demande un test, il ne
/// reconfigure pas la sonde.
pub async fn speedtest_with(
    state: &AppState,
    engine_override: Option<&str>,
    server_override: Option<&str>,
) {
    let state = state.clone();
    let engine_override = engine_override.map(str::to_string);
    let server_override = server_override.map(str::to_string);

    // Lire l'engine et les paramètres depuis la config courante.
    let engine = engine_override.unwrap_or_else(|| {
        let cfg_val = state.config.get();
        cfg_val["speedtestEngine"].as_str().unwrap_or("ookla").to_string()
    });

    tracing::info!("Scheduler: running scheduled speedtest (engine={})", engine);

    state.speedtest.mark_running();
    let _ = state.events.send(crate::state::BroadcastEvent {
        event: "speedtest:running".into(),
        payload: json!({ "running": true }),
    });

    let result = if engine == "iperf3" {
        let server = server_override.unwrap_or_else(|| {
            let cfg_val = state.config.get();
            cfg_val["iperfServer"].as_str().unwrap_or("").to_string()
        });
        if server.trim().is_empty() {
            // Dire lequel manque : « échec » enverrait chercher du côté du
            // réseau un problème qui est dans la commande.
            tracing::warn!("iperf3 demandé sans serveur — test abandonné");
            state.speedtest.mark_stopped();
            return;
        }
        // Résoudre l'IP source depuis l'interface sélectionnée.
        match resolve_src(&state) {
            Ok(src) => lanprobe_core::iperf::run_iperf3(&server, src).await,
            Err(e) => Err(e),
        }
    } else {
        // Ookla — run_speedtest gère l'interface sélectionnée elle-même.
        //
        // ⚠️ Abandonner plutôt que mesurer sans source : Ookla traite `-I`
        // comme une indication contournable, et le pré-check de
        // connectivité qui l'en empêche ne tourne que si une IP source est
        // connue. Sans elle, le test partirait par la route par défaut et
        // rendrait le débit du mauvais lien.
        match resolve_src(&state) {
            Ok(src) => {
                let iface_name = get_selected_iface_name(&state);
                let iface_for_cli = iface_name.as_ref().map(|n| {
                    #[cfg(target_os = "macos")]
                    { get_interface_details(n).bsd_name.unwrap_or(n.clone()) }
                    #[cfg(not(target_os = "macos"))]
                    { n.clone() }
                });
                lanprobe_core::speedtest::run_speedtest(src, iface_for_cli).await
            }
            Err(e) => Err(e),
        }
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
            // Le détail part au hub ; le compteur reste écrit dans Influx par
            // le chemin habituel. Deux chemins pour la même valeur finiraient
            // par diverger, et on ne saurait plus lequel croire.
            if let Some(key) = sealing_key(&state) {
                crate::inventory::publish(
                    &state,
                    &key,
                    crate::inventory::ScanReport {
                        kind: "speedtest".into(),
                        started_at: crate::inventory::now(),
                        cidr: None,
                        hosts: Vec::new(),
                        ports: Vec::new(),
                        speedtest: Some(crate::inventory::SpeedtestReport {
                            engine: r.engine.clone(),
                            server_name: r.server_name.clone(),
                            download_mbps: r.download_mbps,
                            upload_mbps: r.upload_mbps,
                            latency_ms: r.latency_ms,
                            jitter_ms: r.jitter_ms,
                            result_url: r.result_url.clone(),
                        }),
                    },
                )
                .await;
            }
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

async fn run_speedtest_task(state: AppState, interval_min: u64) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_min * 60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        speedtest_once(&state).await;
    }
}

// ── Discovery sub-task ─────────────────────────────────────────────────────

/// Lance une découverte réseau **maintenant** sur le CIDR donné (vide =
/// déduit de l'interface sélectionnée).
///
/// Extraite pour que le hub puisse la déclencher (contrat § 14). Le garde
/// anti-concurrence est conservé tel quel : deux découvertes simultanées se
/// piétinent dans l'état partagé.
pub async fn discovery_once(state: &AppState, cidr: String) {
    let state = state.clone();

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
        return;
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
                return;
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
            return;
        }
    };

    let src = match resolve_src(&state) {
        Ok(src) => src,
        Err(e) => {
            tracing::warn!("découverte planifiée abandonnée : {e}");
            return;
        }
    };

    // Réinitialiser le store de découverte pour ce nouveau scan.
    state.discovery.clear();

    // — La logique de scan tourne directement ici, dans la boucle, sans
    //   inner `tokio::spawn`. Puisque `run_discovery_task` est déjà dans
    //   sa propre sous-tâche, un second spawn créerait une course : la
    //   boucle pourrait avancer au tick suivant avant la fin du scan
    //   précédent et clobberer l'état partagé.

    // Étape 1 : ARP initial, vu par l'interface choisie — une table ARP
    // globale ferait passer les voisins d'un autre lien pour des hôtes du
    // réseau planifié.
    let scan_iface = scan_interface(get_selected_iface_name(&state).as_deref());
    let arp_initial = read_arp_table(scan_iface.as_ref()).await;
    if state.scan_cancel.load(Ordering::SeqCst) {
        let _ = state.events.send(done_event(&effective_cidr, 0));
        state.scan_cancel.store(true, Ordering::SeqCst);
        return;
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
        return;
    }

    // Étape 3 : ARP final pour récupérer les MACs des hôtes pingés.
    let arp_after = read_arp_table(scan_iface.as_ref()).await;
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

    publish_discovery(&state, &effective_cidr).await;

    // Remettre scan_cancel à true (idle) une fois le scan terminé.
    state.scan_cancel.store(true, Ordering::SeqCst);
}

async fn run_discovery_task(state: AppState, interval_min: u64, cidr: String) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_min * 60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        discovery_once(&state, cidr.clone()).await;
    }
}

// ── Port scan sub-task ─────────────────────────────────────────────────────

/// Scanne les ports d'une cible **maintenant**.
///
/// Rend une erreur lisible plutôt que de scanner en silence par la mauvaise
/// interface : un scan pris par le mauvais lien ne dit rien du réseau qu'on
/// croit observer.
pub async fn portscan_once(state: &AppState, ip: &str) -> Result<usize, String> {
    portscan_with(state, ip, None, None, None).await
}

/// Idem, avec une liste de ports imposée (profil choisi depuis le hub).
///
/// ⚠️ La liste est **résolue par l'appelant**, pas nommée ici : les profils
/// vivent dans l'interface, et faire connaître « common » ou « web » à la
/// sonde obligerait à la mettre à jour pour ajouter un profil.
pub async fn portscan_with(
    state: &AppState,
    ip: &str,
    ports: Option<Vec<u16>>,
    // ⚠️ Les ports UDP d'un profil. Le hub les modélise depuis le 02/10
    // (§ 25) : sans eux, un « Common » lancé du hub ne rendait que la moitié de
    // ce que le même profil rend depuis la fenêtre de la sonde.
    udp_ports: Option<Vec<u16>>,
    // 🔴 L'étiquette du profil employé, telle que le hub l'a donnée. Rangée
    // avec le résultat, comme pour un scan local : sans elle, un scan lancé
    // depuis le hub ou le téléphone s'affichait sans aucun profil sur l'écran
    // de la sonde — défaut constaté le 02/10.
    //
    // ⚠️ Jamais validée : les ports font foi, le profil n'est qu'une étiquette.
    // La sonde n'a peut-être pas encore reçu le profil que le hub vient de
    // créer, et l'écran saura l'afficher dès qu'il descendra.
    profile_id: Option<String>,
) -> Result<usize, String> {
    if ip.parse::<std::net::Ipv4Addr>().is_err() {
        return Err(format!("« {ip} » n'est pas une adresse IPv4"));
    }
    let src = resolve_src(state)?;

    state.portscan.mark_in_progress(ip, profile_id.clone());
    let _ = state.events.send(crate::state::BroadcastEvent {
        event: "portscan:update".into(),
        payload: json!({ "ip": ip, "in_progress": true, "profile_id": profile_id }),
    });

    let results = scan_ports(ip, src, ports).await;
    let mut entry = state.portscan.set_tcp(ip, results, now_secs(), profile_id.clone());
    // ⚠️ L'UDP seulement s'il est demandé. Sans liste, on ne lance PAS le scan
    // UDP par défaut du cœur : une commande venue d'un hub antérieur à ce champ
    // se mettrait à scanner des ports que personne n'a demandés.
    if let Some(udp) = udp_ports.filter(|list| !list.is_empty()) {
        let udp_results = scan_udp_ports(ip, src, Some(udp)).await;
        entry = state.portscan.set_udp(ip, udp_results, now_secs());
    }
    let _ = state.events.send(crate::state::BroadcastEvent {
        event: "portscan:update".into(),
        payload: serde_json::to_value(&entry).unwrap_or(serde_json::Value::Null),
    });
    tracing::info!(
        "scan de ports terminé sur {ip} — {} TCP, {} UDP",
        entry.tcp.len(),
        entry.udp.len()
    );

    publish_ports(state, ip).await;

    Ok(entry.tcp.len())
}

async fn run_portscan_task(state: AppState, interval_min: u64, targets: Vec<String>) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_min * 60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        tracing::info!("scan de ports planifié sur {} cibles", targets.len());
        for target in &targets {
            if let Err(e) = portscan_once(&state, target).await {
                tracing::warn!("scan de ports planifié sur {target} abandonné : {e}");
            }
        }
    }
}

// ── Interface resolution helpers ───────────────────────────────────────────

/// Retourne l'IP source de l'interface sélectionnée, ou `None`.
///
/// # Comportement quand `None` est retourné
/// Résout l'IP source de l'interface sélectionnée, **strictement**.
///
/// Trois cas, et le troisième est celui qui fabriquait de fausses mesures :
///
/// - aucune interface choisie → `Ok(None)`, l'OS décide. Légitime : personne
///   n'a exprimé de préférence.
/// - interface choisie, avec une IPv4 → `Ok(Some(ip))`.
/// - **interface choisie, sans IPv4** → `Err`. Le lien est tombé, ou le bail
///   DHCP a expiré.
///
/// Ce dernier cas rendait `None` auparavant, donc « que l'OS décide » — et la
/// mesure planifiée partait par l'interface de management. Un speedtest
/// mesurait alors le débit du mauvais lien, et le point arrivait dans Influx
/// avec une valeur parfaitement plausible que personne ne remettait en cause.
/// C'est le seul chemin qui tourne sans humain devant l'écran : c'est donc
/// celui où une mesure fausse dure le plus longtemps.
///
/// Mieux vaut une mesure absente, visible comme telle, qu'une mesure crédible
/// prise sur le mauvais réseau.
/// Adresse source imposée par l'interface sélectionnée.
///
/// ⚠️ **Tout** le trafic de la sonde passe par cette adresse — mesures comme
/// battement de cœur. C'est une décision du propriétaire : on peut avoir
/// internet sur `eth1` et pas sur `eth0`, et `eth0` est justement celui qu'on
/// cherche à éprouver. Une ligne de vie qui sortirait par l'autre lien
/// affirmerait que tout va bien sur une interface qu'elle n'a jamais empruntée.
///
/// `Err` quand une interface est choisie mais n'a pas d'IPv4 : il n'y a alors
/// pas de source légitime, et il vaut mieux échouer que sortir par ailleurs.
pub fn resolve_src(state: &AppState) -> Result<Option<Ipv4Addr>, String> {
    let name = state
        .selected_interface
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let Some(name) = name else { return Ok(None) };
    let details = get_interface_details(&name);
    let Some(ip) = details.ip else {
        return Err(format!(
            "l'interface « {name} » n'a pas d'adresse IPv4 — mesure abandonnée \
             plutôt que prise par une autre interface"
        ));
    };
    ip.parse::<Ipv4Addr>()
        .map(Some)
        .map_err(|_| format!("adresse IPv4 invalide sur « {name} » : {ip}"))
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
    use crate::state::PortScanEntry;
    use lanprobe_core::ports::PortResult;

    fn port(port: u16, open: bool) -> PortResult {
        PortResult { port, service: "x".into(), proto: "tcp".into(), open }
    }

    #[test]
    fn chaque_machine_publiee_porte_sa_propre_date_de_scan() {
        // 🔴 Depuis que la sonde publie TOUTES les machines qu'elle connaît à
        // chaque scan (sans quoi le hub, qui n'affiche que le dernier scan,
        // perdait les précédentes), `scans.started_at` est unique pour tout le
        // lot : une machine scannée il y a une heure s'affichait comme scannée
        // à l'instant. Une valeur plausible et fausse.
        let entries = vec![
            PortScanEntry {
                ip: "10.0.0.1".into(),
                tcp: vec![port(22, true), port(23, false)],
                timestamp: 1_790_000_000,
                ..Default::default()
            },
            PortScanEntry {
                ip: "10.0.0.2".into(),
                tcp: vec![port(80, true)],
                timestamp: 1_790_003_600,
                ..Default::default()
            },
        ];

        let (hosts, ports) = super::inventory_from_entries(&entries);
        assert_eq!(hosts[0].scanned_at, Some(1_790_000_000));
        assert_eq!(hosts[1].scanned_at, Some(1_790_003_600));
        // ⚠️ Et seuls les ports OUVERTS partent : publier les milliers de ports
        // fermés d'un scan complet gonflerait l'inventaire sans rien apprendre.
        assert_eq!(ports.len(), 2, "{ports:?}");
    }

    #[test]
    fn une_machine_jamais_scannee_part_sans_date_plutot_qu_avec_zero() {
        // ⚠️ Une entrée créée par un scan en cours n'a pas encore d'horodatage.
        // Publier `0` la daterait du 1er janvier 1970 ; l'écran doit pouvoir
        // dire « date inconnue », et c'est `null` qui le lui permet.
        let entries = vec![PortScanEntry {
            ip: "10.0.0.3".into(),
            in_progress: true,
            ..Default::default()
        }];
        let (hosts, _) = super::inventory_from_entries(&entries);
        assert_eq!(hosts[0].scanned_at, None);
    }

    use crate::config::ConfigStore;
    use std::sync::Arc;

    fn state_on(interface: Option<&str>) -> AppState {
        let state = AppState::new_headless(Arc::new(ConfigStore::load(
            std::env::temp_dir().join("lanprobe-resolve-src-test.json"),
        )));
        *state.selected_interface.lock().unwrap() = interface.map(str::to_string);
        state
    }

    #[test]
    fn no_selected_interface_lets_the_system_route() {
        // Rien d'imposé : on ne force aucune adresse source. C'est le
        // comportement attendu tant que l'utilisateur n'a rien choisi.
        assert_eq!(resolve_src(&state_on(None)), Ok(None));
    }

    #[test]
    fn an_interface_without_an_address_fails_instead_of_falling_back() {
        // ⚠️ Le cœur de la règle : tout le trafic de la sonde — mesures ET
        // battement de cœur — passe par l'interface choisie. Retomber sur la
        // route par défaut ferait mesurer un lien et répondre par un autre,
        // et le hub affirmerait « en ligne » pour une interface qu'aucun
        // paquet n'a traversée.
        let err = resolve_src(&state_on(Some("interface-qui-nexiste-pas"))).unwrap_err();
        assert!(err.contains("interface-qui-nexiste-pas"), "{err}");
    }

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
