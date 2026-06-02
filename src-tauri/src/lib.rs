mod updater;

use updater::UpdateInfo;
use lanprobe_core::interfaces::{get_interface_details, list_interfaces, InterfaceDetails};
use lanprobe_core::configure::{apply_dhcp, apply_static, NetworkConfig};
use lanprobe_core::permissions::{has_permissions, install_permissions};
use lanprobe_core::ping::{self, ping_once};
use lanprobe_core::discovery::{parse_cidr, get_hostname, read_arp_table, get_local_network_cidr, DiscoveredHost};
use lanprobe_core::ping::ping_once_fast_retry;
use lanprobe_core::ports::{scan_ports, scan_udp_ports, PortResult};
use lanprobe_core::sla::{compute_sla, PingSample, SlaStats};
use lanprobe_core::speedtest::{run_speedtest, SpeedResult};
use lanprobe_core::iperf::run_iperf3;
use lanprobe_core::internet::{run_internet_monitor, InternetTick};
use lanprobe_core::public_ip::{get_public_ip, PublicIpInfo};
use lanprobe_server::state::{AppState, BroadcastEvent};
use lanprobe_server::AuthStore;
use tauri::Emitter;
use serde::Deserialize;
use std::net::Ipv4Addr;
use std::time::Duration;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::Mutex as AsyncMutex;

/// Handle du serveur HTTPS embarqué, quand le toggle "Mode serveur" est ON.
/// `None` = serveur arrêté. Stocké dans un `AsyncMutex` parce que
/// `shutdown()` est async.
type ServerModeState = Arc<AsyncMutex<Option<lanprobe_server::ServerHandle>>>;

#[tauri::command]
async fn cmd_check_update() -> Result<UpdateInfo, String> {
    updater::check_update_impl().await
}

#[tauri::command]
async fn cmd_apply_update(url: String, asset_name: String) -> Result<String, String> {
    updater::apply_update_impl(url, asset_name).await
}

/// `AppState` partagé entre les commandes Tauri et le serveur HTTP
/// embarqué quand il tourne. Toutes les opérations réseau (ping, scan,
/// discovery, monitoring, internet) lisent et écrivent les mêmes
/// handles, ce qui permet à un client web connecté de voir l'état
/// courant du desktop et de recevoir les events en live.
type SharedState = Arc<AppState>;

/// Résout l'IPv4 source à utiliser pour toutes les opérations réseau,
/// en lisant l'interface désignée par l'utilisateur. Strict : si
/// l'interface est sélectionnée mais sans IPv4, on refuse au lieu de
/// retomber sur la route par défaut.
fn resolve_src_strict(state: &SharedState) -> Result<Option<Ipv4Addr>, String> {
    let name_opt = state.selected_interface.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let Some(name) = name_opt else { return Ok(None); };
    let details = get_interface_details(&name);
    let ip_str = details.ip.ok_or_else(|| format!("L'interface « {} » n'a pas d'adresse IPv4 — choisissez-en une autre.", name))?;
    let ip: Ipv4Addr = ip_str.parse().map_err(|_| format!("Adresse IPv4 invalide sur « {}» : {}", name, ip_str))?;
    Ok(Some(ip))
}

#[derive(Deserialize, Default)]
struct ApplyStaticArgs {
    interface: String,
    ip: String,
    subnet: String,
    gateway: String,
    dns_primary: String,
    dns_secondary: Option<String>,
}

// Marqueur ASCII embarqué dans la section .rodata : permet au job CI de
// grep la version attendue via `strings` sans dépendre de l'encodage UTF-16
// de la ressource PE VS_VERSION_INFO. #[used] + #[no_mangle] empêchent le
// linker d'optimiser la constante sous prétexte qu'elle semble inutilisée.
#[used]
#[unsafe(no_mangle)]
static LANPROBE_BUILD_VERSION: &[u8] = concat!("LANPROBE_BUILD_VERSION=", env!("CARGO_PKG_VERSION"), "\0").as_bytes();

#[tauri::command]
fn cmd_app_version(app: AppHandle) -> String {
    // On prend la version depuis le package_info Tauri (issu de tauri.conf.json
    // via tauri-build) plutôt que env!("CARGO_PKG_VERSION") directement : en CI
    // Windows, Cargo n'invalidait pas toujours lib.rs lors d'un bump de version,
    // et le binaire embarquait une version périmée côté Settings.
    let _ = LANPROBE_BUILD_VERSION; // force-référence pour que le linker garde le marqueur
    app.package_info().version.to_string()
}

/// Retourne "pkg", "dmg", ou "unknown" selon le mode d'installation macOS.
/// Sur les autres plateformes retourne toujours "unknown".
#[tauri::command]
fn cmd_install_type() -> String {
    #[cfg(target_os = "macos")]
    {
        // Le PKG inscrit un receipt lisible par pkgutil. On cherche sur tous
        // les packages enregistrés pour éviter les variations d'identifiant
        // (Tauri peut enregistrer io.lanprobe.app, io.lanprobe.app.pkg, etc.)
        let ok = std::process::Command::new("sh")
            .args(["-c", "pkgutil --pkgs 2>/dev/null | grep -qi lanprobe"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok { "pkg".to_string() } else { "dmg".to_string() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        "unknown".to_string()
    }
}

#[tauri::command]
fn cmd_check_permissions() -> bool { has_permissions() }

#[tauri::command]
fn cmd_install_permissions() -> Result<(), String> { install_permissions() }

#[tauri::command]
fn cmd_list_interfaces() -> Vec<String> { list_interfaces() }

#[tauri::command]
fn cmd_get_interface_details(name: String) -> InterfaceDetails { get_interface_details(&name) }

#[tauri::command]
fn cmd_set_selected_interface(name: Option<String>, state: tauri::State<'_, SharedState>) -> Result<(), String> {
    *state.selected_interface.lock().unwrap_or_else(|p| p.into_inner()) = name.clone();
    // Diffuse aux clients (web + desktop) pour que leur Dashboard
    // reflète le changement en live — sinon le dropdown côté web reste
    // sur l'ancienne valeur jusqu'à rechargement.
    state.emit(
        "interface:selected",
        serde_json::json!({ "name": name }),
    );
    Ok(())
}

#[tauri::command]
fn cmd_get_selected_interface(state: tauri::State<'_, SharedState>) -> Option<String> {
    state.selected_interface.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

#[tauri::command]
fn cmd_get_discovery_snapshot(state: tauri::State<'_, SharedState>) -> Vec<DiscoveredHost> {
    state.discovery.snapshot()
}

#[tauri::command]
fn cmd_clear_discovery(state: tauri::State<'_, SharedState>) {
    state.discovery.clear();
}

#[tauri::command]
fn cmd_get_monitoring_snapshot(
    state: tauri::State<'_, SharedState>,
) -> std::collections::HashMap<String, Vec<lanprobe_core::ping::PingResult>> {
    state.monitoring.snapshot()
}

/// Renvoie le snapshot complet de la config frontend (settings, profils
/// réseau, profils portscan). C'est ce que tous les clients — desktop ou
/// web — utilisent pour s'hydrater au démarrage.
#[tauri::command]
fn cmd_config_get(state: tauri::State<'_, SharedState>) -> serde_json::Value {
    state.config.get()
}

/// Écrase le snapshot complet de la config frontend et diffuse un
/// event `config:update` sur le bus partagé pour que les autres clients
/// (WS + Tauri webview) se resynchronisent en temps réel.
#[tauri::command]
fn cmd_config_set(
    value: serde_json::Value,
    state: tauri::State<'_, SharedState>,
) -> Result<(), String> {
    state.config.put(value.clone())?;
    state.emit("config:update", value);
    Ok(())
}

/// Attend que l'interface `iface` ait une IP assignée (poll 500 ms,
/// timeout configurable). Lève le blackout dès que l'IP est disponible
/// pour ne pas allonger inutilement la fenêtre silencieuse.
async fn wait_interface_up(shared: &AppState, iface: &str, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let details = get_interface_details(iface);
        if details.ip.is_some() {
            shared.clear_monitoring_blackout();
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    // Timeout atteint — on lève quand même le blackout pour ne pas
    // laisser le monitoring muet indéfiniment.
    shared.clear_monitoring_blackout();
}

#[tauri::command]
async fn cmd_apply_static(
    args: ApplyStaticArgs,
    state: tauri::State<'_, SharedState>,
) -> Result<(), String> {
    // Blackout généreux : couvre la transition réseau + éventuel délai DHCP/ARP.
    state.set_monitoring_blackout(Duration::from_secs(25));
    let iface = args.interface.clone();
    let result = apply_static(&NetworkConfig {
        interface: args.interface, ip: args.ip, subnet: args.subnet,
        gateway: args.gateway, dns_primary: args.dns_primary, dns_secondary: args.dns_secondary,
    });
    if result.is_err() {
        state.clear_monitoring_blackout();
        return result;
    }
    wait_interface_up(&state, &iface, Duration::from_secs(15)).await;
    Ok(())
}

#[tauri::command]
async fn cmd_apply_dhcp(
    interface: String,
    state: tauri::State<'_, SharedState>,
) -> Result<(), String> {
    // DHCP peut prendre plus longtemps (échange DORA) → blackout plus large.
    state.set_monitoring_blackout(Duration::from_secs(35));
    let result = apply_dhcp(&interface);
    if result.is_err() {
        state.clear_monitoring_blackout();
        return result;
    }
    wait_interface_up(&state, &interface, Duration::from_secs(20)).await;
    Ok(())
}

#[tauri::command]
async fn cmd_start_ping(
    ip: String,
    state: tauri::State<'_, SharedState>,
) -> Result<(), String> {
    {
        let mut map = state.ping_stop.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(ip.clone(), false);
    }
    let shared = state.inner().clone();
    let ip_clone = ip.clone();
    tokio::spawn(async move {
        loop {
            {
                let Ok(map) = shared.ping_stop.lock() else { break };
                if *map.get(&ip_clone).unwrap_or(&true) { break; }
            }
            // Pendant un blackout (changement de profil réseau en cours),
            // on ne fait rien — ni ping ni enregistrement — pour éviter
            // d'inscrire de faux outages dans l'historique.
            if shared.is_monitoring_blackout() {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            // Résolution de l'IP source in-place — on ne peut pas
            // appeler `resolve_src_strict(&shared)` parce que la fonction
            // prend une tauri::State, pas un SharedState direct.
            let strict: Result<Option<Ipv4Addr>, ()> = {
                let name_opt = shared.selected_interface.lock().unwrap_or_else(|p| p.into_inner()).clone();
                match name_opt {
                    None => Ok(None),
                    Some(name) => {
                        let d = get_interface_details(&name);
                        match d.ip.and_then(|s| s.parse::<Ipv4Addr>().ok()) {
                            Some(ip) => Ok(Some(ip)),
                            None => Err(()),
                        }
                    }
                }
            };
            let result = match strict {
                Ok(src) => ping_once(&ip_clone, src).await,
                Err(_) => ping::PingResult {
                    ip: ip_clone.clone(),
                    alive: false,
                    latency_ms: None,
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                },
            };
            shared.monitoring.push(result.clone());
            shared.emit(
                "ping:tick",
                serde_json::to_value(&result).unwrap_or(serde_json::Value::Null),
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    Ok(())
}

#[tauri::command]
async fn cmd_stop_ping(ip: String, state: tauri::State<'_, SharedState>) -> Result<(), String> {
    {
        let mut map = state.ping_stop.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(ip.clone(), true);
    }
    // Purge l'historique pour que cmd_get_monitoring_snapshot ne ré-injecte pas
    // l'hôte au prochain (ré)hydratation du front (sinon il « réapparaît » après
    // suppression du monitoring).
    state.monitoring.clear_ip(&ip);
    Ok(())
}

#[tauri::command]
fn cmd_get_local_network_cidr(
    iface_name: Option<String>,
    state: tauri::State<'_, SharedState>,
) -> Option<String> {
    let iface = &state.selected_interface;
    // On dérive le CIDR de l'interface choisie par l'utilisateur, pas de la
    // route par défaut du système : sur Windows, `ipconfig` renvoie souvent
    // l'adresse du vEthernet WSL en premier, ce qui faisait scanner le
    // mauvais réseau. Le frontend peut passer `iface_name` explicitement
    // pour éviter la race avec la persistance du state backend (Discovery
    // peut monter avant que Dashboard n'ait set l'interface côté Rust).
    let name = iface_name.or_else(|| iface.lock().unwrap_or_else(|p| p.into_inner()).clone());
    if let Some(name) = name {
        let d = get_interface_details(&name);
        if let (Some(ip), Some(mask)) = (d.ip, d.subnet) {
            let ip_parts: Vec<u8> = ip.split('.').filter_map(|p| p.parse().ok()).collect();
            let mask_parts: Vec<u8> = mask.split('.').filter_map(|p| p.parse().ok()).collect();
            if ip_parts.len() == 4 && mask_parts.len() == 4 {
                let ip_int = u32::from_be_bytes([ip_parts[0], ip_parts[1], ip_parts[2], ip_parts[3]]);
                let mask_int = u32::from_be_bytes([mask_parts[0], mask_parts[1], mask_parts[2], mask_parts[3]]);
                let prefix = mask_int.count_ones();
                let net = Ipv4Addr::from(ip_int & mask_int);
                return Some(format!("{}/{}", net, prefix));
            }
        }
    }
    get_local_network_cidr()
}

/// Lance un scan réseau en arrière-plan et émet des events :
/// - "discovery:host"  { ip, hostname, mac, latency_ms }  pour chaque hôte trouvé
/// - "discovery:done"  {}  à la fin du scan
/// Retourne immédiatement pour ne pas bloquer la navigation.
#[tauri::command]
async fn cmd_scan_network(
    cidr: String,
    state: tauri::State<'_, SharedState>,
) -> Result<(), String> {
    let (first, last) = parse_cidr(&cidr)?;
    let src = resolve_src_strict(state.inner())?;
    if state
        .scan_cancel
        .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A network scan is already in progress".to_string());
    }
    state.discovery.clear();
    let shared = state.inner().clone();

    tokio::spawn(async move {
        let cancel = &shared.scan_cancel;
        let arp_initial = read_arp_table().await;
        if cancel.load(Ordering::SeqCst) {
            shared.emit("discovery:done", serde_json::Value::Null);
            return;
        }
        for (ip, mac) in &arp_initial {
            let ip_parsed: Option<u32> = ip.split('.').fold(Some(0u32), |acc, p| {
                acc.and_then(|a| p.parse::<u8>().ok().map(|b| (a << 8) | b as u32))
            });
            if let Some(ip_int) = ip_parsed {
                if ip_int >= first && ip_int <= last {
                    let host = DiscoveredHost {
                        ip: ip.clone(),
                        hostname: None,
                        mac: Some(mac.clone()),
                        vendor: lanprobe_core::oui::vendor_for_mac(mac),
                        latency_ms: None,
                    };
                    shared.discovery.upsert(host.clone());
                    shared.emit(
                        "discovery:host",
                        serde_json::to_value(&host).unwrap_or(serde_json::Value::Null),
                    );
                }
            }
        }

        #[cfg(target_os = "windows")]
        let chunk_size = 32usize;
        #[cfg(not(target_os = "windows"))]
        let chunk_size = 128usize;
        let all_ips: Vec<String> = (first..=last).map(|i| Ipv4Addr::from(i).to_string()).collect();

        for chunk in all_ips.chunks(chunk_size) {
            if cancel.load(Ordering::SeqCst) { break; }
            let mut handles = vec![];
            for ip in chunk {
                let ip = ip.clone();
                let arp_mac = arp_initial.get(&ip).cloned();
                let shared_c = shared.clone();
                handles.push(tokio::spawn(async move {
                    let latency = ping_once_fast_retry(&ip, src, 3).await;
                    if let Some(lat) = latency {
                        if arp_mac.is_none() {
                            let hostname = get_hostname(&ip).await;
                            let host = DiscoveredHost {
                                ip: ip.clone(),
                                hostname,
                                mac: None,
                                vendor: None,
                                latency_ms: Some(lat),
                            };
                            shared_c.discovery.upsert(host.clone());
                            shared_c.emit(
                                "discovery:host",
                                serde_json::to_value(&host).unwrap_or(serde_json::Value::Null),
                            );
                        } else {
                            shared_c.discovery.update_latency(&ip, lat);
                            shared_c.emit(
                                "discovery:host_latency",
                                serde_json::json!({ "ip": ip, "latency_ms": lat }),
                            );
                        }
                    }
                }));
            }
            for h in handles { let _ = h.await; }
        }

        if cancel.load(Ordering::SeqCst) {
            shared.emit("discovery:done", serde_json::Value::Null);
            return;
        }

        let arp_after = read_arp_table().await;
        for (ip, mac) in &arp_after {
            if arp_initial.contains_key(ip) { continue; }
            let ip_parsed: Option<u32> = ip.split('.').fold(Some(0u32), |acc, p| {
                acc.and_then(|a| p.parse::<u8>().ok().map(|b| (a << 8) | b as u32))
            });
            if let Some(ip_int) = ip_parsed {
                if ip_int >= first && ip_int <= last {
                    shared.discovery.update_mac(ip, mac.clone());
                    shared.emit(
                        "discovery:host_mac",
                        serde_json::json!({
                            "ip": ip,
                            "mac": mac,
                            "vendor": lanprobe_core::oui::vendor_for_mac(mac),
                        }),
                    );
                }
            }
        }

        shared.emit("discovery:done", serde_json::Value::Null);
        // Remettre scan_cancel à true (idle) : le scan est terminé normalement.
        shared.scan_cancel.store(true, Ordering::SeqCst);
    });

    Ok(())
}

#[tauri::command]
fn cmd_cancel_scan(state: tauri::State<'_, SharedState>) {
    state.scan_cancel.store(true, Ordering::SeqCst);
    state.emit("discovery:done", serde_json::Value::Null);
}

#[tauri::command]
async fn cmd_scan_ports(ip: String, ports: Option<Vec<u16>>, profile_id: Option<String>, state: tauri::State<'_, SharedState>) -> Result<Vec<PortResult>, String> {
    let src = resolve_src_strict(state.inner())?;
    state.portscan.mark_in_progress(&ip, profile_id.clone());
    state.emit("portscan:update", serde_json::json!({
        "ip": ip, "in_progress": true, "profile_id": profile_id,
    }));
    let results = scan_ports(&ip, src, ports).await;
    let now = now_secs();
    let entry = state.portscan.set_tcp(&ip, results.clone(), now, profile_id);
    state.emit("portscan:update", serde_json::to_value(&entry).unwrap_or(serde_json::json!({})));
    Ok(results)
}

#[tauri::command]
async fn cmd_scan_udp_ports(ip: String, ports: Option<Vec<u16>>, state: tauri::State<'_, SharedState>) -> Result<Vec<PortResult>, String> {
    let src = resolve_src_strict(state.inner())?;
    let results = scan_udp_ports(&ip, src, ports).await;
    let now = now_secs();
    let entry = state.portscan.set_udp(&ip, results.clone(), now);
    state.emit("portscan:update", serde_json::to_value(&entry).unwrap_or(serde_json::json!({})));
    Ok(results)
}

#[tauri::command]
fn cmd_get_portscan_snapshot(state: tauri::State<'_, SharedState>) -> Vec<lanprobe_server::state::PortScanEntry> {
    state.portscan.snapshot()
}

#[tauri::command]
fn cmd_clear_portscan_entry(ip: String, state: tauri::State<'_, SharedState>) -> Result<(), String> {
    state.portscan.remove(&ip);
    state.emit("portscan:removed", serde_json::json!({ "ip": ip }));
    Ok(())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Deserialize, Default)]
struct PingSampleDto { alive: bool, latency_ms: Option<u64> }

#[tauri::command]
fn cmd_compute_sla(ip: String, samples: Vec<PingSampleDto>) -> SlaStats {
    let s: Vec<PingSample> = samples.into_iter().map(|d| PingSample { alive: d.alive, latency_ms: d.latency_ms }).collect();
    compute_sla(&ip, &s)
}

#[tauri::command]
async fn cmd_run_speedtest(state: tauri::State<'_, SharedState>) -> Result<SpeedResult, String> {
    let src = resolve_src_strict(state.inner())?;
    let name = state.selected_interface.lock().unwrap_or_else(|p| p.into_inner()).clone();
    // Sur macOS, `name` est le service networksetup ("Wi-Fi") — or Ookla CLI
    // attend l'ifname BSD ("en0"). Sur Linux/Windows `name` est déjà le bon.
    let iface_for_cli = match name {
        Some(ref n) => {
            #[cfg(target_os = "macos")]
            { get_interface_details(n).bsd_name.or(Some(n.clone())) }
            #[cfg(not(target_os = "macos"))]
            { Some(n.clone()) }
        }
        None => None,
    };
    state.speedtest.mark_running();
    state.emit("speedtest:running", serde_json::json!({ "running": true }));
    let cancel = state.speedtest.cancel_handle();
    let res = tokio::select! {
        r = run_speedtest(src, iface_for_cli) => r,
        _ = cancel.notified() => Err("speedtest.errors.cancelled".to_string()),
    };
    match &res {
        Ok(r) => {
            state.speedtest.set(r.clone());
            state.emit("speedtest:result", serde_json::to_value(r).unwrap_or(serde_json::json!({})));
        }
        Err(_) => {
            state.speedtest.mark_stopped();
            state.emit("speedtest:running", serde_json::json!({ "running": false }));
        }
    }
    res
}

#[tauri::command]
async fn cmd_cancel_speedtest(state: tauri::State<'_, SharedState>) -> Result<(), String> {
    state.speedtest.request_cancel();
    state.emit("speedtest:running", serde_json::json!({ "running": false }));
    Ok(())
}

#[tauri::command]
async fn cmd_run_iperf3(server: String, state: tauri::State<'_, SharedState>) -> Result<SpeedResult, String> {
    let src = resolve_src_strict(state.inner())?;
    state.speedtest.mark_running();
    state.emit("speedtest:running", serde_json::json!({ "running": true }));
    let cancel = state.speedtest.cancel_handle();
    let res = tokio::select! {
        r = run_iperf3(&server, src) => r,
        _ = cancel.notified() => Err("speedtest.errors.cancelled".to_string()),
    };
    match &res {
        Ok(r) => {
            state.speedtest.set(r.clone());
            state.emit("speedtest:result", serde_json::to_value(r).unwrap_or(serde_json::json!({})));
        }
        Err(_) => {
            state.speedtest.mark_stopped();
            state.emit("speedtest:running", serde_json::json!({ "running": false }));
        }
    }
    res
}

#[tauri::command]
fn cmd_get_speedtest_snapshot(state: tauri::State<'_, SharedState>) -> serde_json::Value {
    serde_json::json!({
        "latest": state.speedtest.snapshot(),
        "running": state.speedtest.is_running(),
    })
}

#[tauri::command]
fn cmd_get_internet_status(state: tauri::State<'_, SharedState>) -> Option<InternetTick> {
    state.internet.snapshot()
}

#[tauri::command]
fn cmd_reset_internet_monitor(state: tauri::State<'_, SharedState>) {
    state.internet.reset();
}

#[tauri::command]
async fn cmd_get_public_ip(state: tauri::State<'_, SharedState>) -> Result<PublicIpInfo, String> {
    let src = resolve_src_strict(state.inner())?;
    get_public_ip(src).await
}

/// Ouvre une URL dans le navigateur par défaut. Tauri v2 bloque la
/// navigation externe depuis les `<a target="_blank">`, on passe donc
/// par la commande système.
#[tauri::command]
fn cmd_open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("URL non autorisée".into());
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &url])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&url)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&url)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct ServerStartArgs {
    host: Option<String>,
    port: Option<u16>,
}

#[derive(serde::Serialize)]
struct ServerStatus {
    running: bool,
    addr: Option<String>,
}

#[tauri::command]
async fn cmd_server_mode_status(state: tauri::State<'_, ServerModeState>) -> Result<ServerStatus, String> {
    let guard = state.inner().lock().await;
    Ok(match guard.as_ref() {
        Some(h) => ServerStatus {
            running: true,
            addr: Some(format!("https://{}", h.addr)),
        },
        None => ServerStatus { running: false, addr: None },
    })
}

#[tauri::command]
async fn cmd_server_mode_start(
    args: ServerStartArgs,
    state: tauri::State<'_, ServerModeState>,
    shared: tauri::State<'_, SharedState>,
) -> Result<ServerStatus, String> {
    let mut guard = state.inner().lock().await;
    if guard.is_some() {
        return Err("server already running".into());
    }
    let host = args.host.unwrap_or_else(|| "0.0.0.0".into());
    let port = args.port.unwrap_or(8443);
    let addr: std::net::SocketAddr = format!("{}:{}", host, port)
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let handle = lanprobe_server::start(lanprobe_server::StartConfig {
        addr,
        config_dir: lanprobe_server::default_config_dir(),
        shared_state: Some((**shared.inner()).clone()),
    })
    .await?;
    let status = ServerStatus {
        running: true,
        addr: Some(format!("https://{}", handle.addr)),
    };
    *guard = Some(handle);
    Ok(status)
}

#[tauri::command]
async fn cmd_server_mode_stop(state: tauri::State<'_, ServerModeState>) -> Result<(), String> {
    let mut guard = state.inner().lock().await;
    if let Some(handle) = guard.take() {
        handle.shutdown().await?;
    }
    Ok(())
}

#[tauri::command]
fn cmd_server_mode_has_account(shared: tauri::State<'_, SharedState>) -> bool {
    !shared.auth.needs_setup()
}

#[derive(Deserialize)]
struct SetAccountArgs {
    username: String,
    password: String,
}

#[tauri::command]
fn cmd_server_mode_set_account(
    args: SetAccountArgs,
    shared: tauri::State<'_, SharedState>,
) -> Result<(), String> {
    if args.username.trim().is_empty() {
        return Err("username required".into());
    }
    if args.password.len() < 8 {
        return Err("password must be ≥ 8 characters".into());
    }
    shared.auth.set_or_update_credentials(args.username.trim(), &args.password)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config_dir = lanprobe_server::default_config_dir();
    let _ = std::fs::create_dir_all(&config_dir);
    let auth = AuthStore::load(lanprobe_server::users_file_path(&config_dir))
        .map(Arc::new)
        .expect("failed to load auth store");
    let config = Arc::new(lanprobe_server::config::ConfigStore::load(
        lanprobe_server::config::default_config_path(&config_dir),
    ));
    let shared: SharedState = Arc::new(AppState::new(auth, config));
    let server_mode: ServerModeState = Arc::new(AsyncMutex::new(None));
    let shared_for_setup = shared.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(shared)
        .manage(server_mode)
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let shared = shared_for_setup.clone();
            // Forwarder event bus → app.emit. Chaque event publié sur
            // `shared.events` est relayé à la webview Tauri via IPC, en
            // plus d'être diffusé aux WebSockets quand le serveur embarqué
            // tourne. Ça permet au frontend desktop de rester branché sur
            // le même bus que le frontend web.
            let mut rx = shared.events.subscribe();
            let app_handle_forward = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                while let Ok(ev) = rx.recv().await {
                    let _ = app_handle_forward.emit(&ev.event, ev.payload);
                }
            });
            // Monitoring internet : on passe par le bus partagé pour que
            // les clients web voient les mêmes ticks que le desktop.
            let history = shared.internet.clone();
            let iface = shared.selected_interface.clone();
            let events = shared.events.clone();
            tauri::async_runtime::spawn(run_internet_monitor(history, iface, move |tick| {
                let _ = events.send(BroadcastEvent {
                    event: "internet:tick".into(),
                    payload: serde_json::to_value(tick).unwrap_or(serde_json::Value::Null),
                });
            }));
            let _ = app_handle;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cmd_app_version,
            cmd_install_type,
            cmd_check_permissions,
            cmd_install_permissions,
            cmd_list_interfaces,
            cmd_get_interface_details,
            cmd_set_selected_interface,
            cmd_get_selected_interface,
            cmd_get_discovery_snapshot,
            cmd_clear_discovery,
            cmd_get_monitoring_snapshot,
            cmd_config_get,
            cmd_config_set,
            cmd_apply_static,
            cmd_apply_dhcp,
            cmd_start_ping,
            cmd_stop_ping,
            cmd_get_local_network_cidr,
            cmd_scan_network,
            cmd_cancel_scan,
            cmd_scan_ports,
            cmd_scan_udp_ports,
            cmd_get_portscan_snapshot,
            cmd_clear_portscan_entry,
            cmd_compute_sla,
            cmd_run_speedtest,
            cmd_cancel_speedtest,
            cmd_run_iperf3,
            cmd_get_speedtest_snapshot,
            cmd_check_update,
            cmd_apply_update,
            cmd_get_internet_status,
            cmd_reset_internet_monitor,
            cmd_get_public_ip,
            cmd_open_url,
            cmd_server_mode_status,
            cmd_server_mode_start,
            cmd_server_mode_stop,
            cmd_server_mode_has_account,
            cmd_server_mode_set_account,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
