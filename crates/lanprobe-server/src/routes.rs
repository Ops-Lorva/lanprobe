//! Toutes les commandes Tauri ré-exposées en HTTP POST /api/invoke/<cmd>.
//!
//! Le shim `window.__TAURI_INTERNALS__` côté navigateur fait un POST avec
//! le body JSON = arguments, et lit la réponse JSON. Les signatures ici
//! doivent rester alignées avec `src-tauri/src/lib.rs` pour que le même
//! frontend marche dans les deux modes.

use std::net::Ipv4Addr;
use std::sync::atomic::Ordering;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use lanprobe_core::configure::{apply_dhcp, apply_static, NetworkConfig};
use lanprobe_core::discovery::{
    get_hostname, get_local_network_cidr, parse_cidr, read_arp_table, DiscoveredHost,
};
use lanprobe_core::interfaces::{get_interface_details, list_interfaces};
use lanprobe_core::iperf::run_iperf3;
use lanprobe_core::permissions::{has_permissions, install_permissions};
use lanprobe_core::ping::{self, ping_once, ping_once_fast_retry};
use lanprobe_core::ports::{scan_ports, scan_udp_ports};
use lanprobe_core::public_ip::get_public_ip;
use lanprobe_core::sla::{compute_sla, PingSample};
use lanprobe_core::speedtest::run_speedtest;
use lanprobe_core::updater;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::state::AppState;

/// Résout l'IP source de l'interface sélectionnée — strict (erreur si
/// interface choisie mais pas d'IPv4) — exactement comme le backend Tauri.
fn resolve_src_strict(state: &AppState) -> Result<Option<Ipv4Addr>, String> {
    let name_opt = state.selected_interface.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let Some(name) = name_opt else { return Ok(None); };
    let details = get_interface_details(&name);
    let ip_str = details.ip.ok_or_else(|| {
        format!("L'interface « {} » n'a pas d'adresse IPv4 — choisissez-en une autre.", name)
    })?;
    let ip: Ipv4Addr = ip_str
        .parse()
        .map_err(|_| format!("Adresse IPv4 invalide sur « {}» : {}", name, ip_str))?;
    Ok(Some(ip))
}

/// Réponse JSON uniforme : les erreurs remontent en texte brut avec code
/// 500, les succès sont serialisés en JSON. Le shim du navigateur
/// `throw text` sur !ok et `JSON.parse` sinon — pareil que Tauri `invoke`.
pub async fn invoke(
    Path(cmd): Path<String>,
    State(state): State<AppState>,
    Json(args): Json<Value>,
) -> impl IntoResponse {
    match dispatch(&cmd, args, &state).await {
        Ok(v) => (StatusCode::OK, v.to_string()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

async fn dispatch(cmd: &str, args: Value, state: &AppState) -> Result<Value, String> {
    match cmd {
        "cmd_app_version" => Ok(json!(env!("CARGO_PKG_VERSION"))),

        "cmd_check_permissions" => Ok(json!(has_permissions())),
        "cmd_install_permissions" => install_permissions().map(|_| Value::Null),

        "cmd_list_interfaces" => Ok(json!(list_interfaces())),
        "cmd_get_interface_details" => {
            let name = args.get("name").and_then(|v| v.as_str()).ok_or("missing name")?;
            serde_json::to_value(get_interface_details(name)).map_err(|e| e.to_string())
        }

        "cmd_get_platform" => {
            let os = if cfg!(target_os = "windows") { "windows" }
                else if cfg!(target_os = "macos") { "macos" }
                else { "linux" };
            Ok(json!(os))
        }

        "cmd_install_type" => Ok(json!("headless-server")),

        "cmd_set_selected_interface" => {
            let name: Option<String> = args.get("name")
                .and_then(|v| if v.is_null() { None } else { v.as_str().map(String::from) });
            *state.selected_interface.lock().unwrap_or_else(|p| p.into_inner()) = name.clone();
            let _ = state.events.send(crate::state::BroadcastEvent {
                event: "interface:selected".into(),
                payload: json!({ "name": name }),
            });
            Ok(Value::Null)
        }
        "cmd_get_selected_interface" => {
            Ok(json!(state.selected_interface.lock().unwrap_or_else(|p| p.into_inner()).clone()))
        }
        "cmd_get_discovery_snapshot" => {
            Ok(serde_json::to_value(state.discovery.snapshot()).unwrap_or(Value::Null))
        }
        "cmd_clear_discovery" => {
            state.discovery.clear();
            Ok(Value::Null)
        }
        "cmd_get_monitoring_snapshot" => {
            Ok(serde_json::to_value(state.monitoring.snapshot()).unwrap_or(Value::Null))
        }

        "cmd_config_get" => Ok(state.config.get()),
        "cmd_config_set" => {
            // Autorisé depuis le web : le desktop reçoit config:update
            // et se resynchronise. Permet de piloter l'engine speedtest
            // et le serveur iperf3 depuis la vue web.
            let value = args.get("value").cloned().unwrap_or(Value::Null);
            state.config.put(value.clone())?;
            let _ = state.events.send(crate::state::BroadcastEvent {
                event: "config:update".into(),
                payload: value,
            });
            Ok(Value::Null)
        }

        "cmd_apply_static" => {
            let dto: ApplyStaticArgs = serde_json::from_value(args.get("args").cloned().unwrap_or(args))
                .map_err(|e| e.to_string())?;
            apply_static(&NetworkConfig {
                interface: dto.interface,
                ip: dto.ip,
                subnet: dto.subnet,
                gateway: dto.gateway,
                dns_primary: dto.dns_primary,
                dns_secondary: dto.dns_secondary,
            })
            .map(|_| Value::Null)
        }
        "cmd_apply_dhcp" => {
            let iface = args.get("interface").and_then(|v| v.as_str()).ok_or("missing interface")?;
            apply_dhcp(iface).map(|_| Value::Null)
        }

        "cmd_start_ping" => {
            let ip = args.get("ip").and_then(|v| v.as_str()).ok_or("missing ip")?.to_string();
            {
                let mut map = state.ping_stop.lock().unwrap_or_else(|p| p.into_inner());
                map.insert(ip.clone(), false);
            }
            let stop_map = state.ping_stop.clone();
            let iface = state.selected_interface.clone();
            let events = state.events.clone();
            let monitoring = state.monitoring.clone();
            let ip_clone = ip.clone();
            tokio::spawn(async move {
                loop {
                    {
                        let Ok(map) = stop_map.lock() else { break };
                        if *map.get(&ip_clone).unwrap_or(&true) { break; }
                    }
                    let src = resolve_src_from(&iface);
                    let result = match src {
                        Ok(s) => ping_once(&ip_clone, s).await,
                        Err(_) => ping::PingResult {
                            ip: ip_clone.clone(),
                            alive: false,
                            latency_ms: None,
                            timestamp: now_secs(),
                        },
                    };
                    monitoring.push(result.clone());
                    let _ = events.send(crate::state::BroadcastEvent {
                        event: "ping:tick".into(),
                        payload: serde_json::to_value(&result).unwrap_or(Value::Null),
                    });
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
            Ok(Value::Null)
        }
        "cmd_stop_ping" => {
            let ip = args.get("ip").and_then(|v| v.as_str()).ok_or("missing ip")?.to_string();
            state.ping_stop.lock().unwrap_or_else(|p| p.into_inner()).insert(ip, true);
            Ok(Value::Null)
        }

        "cmd_get_local_network_cidr" => {
            let explicit = args.get("ifaceName").and_then(|v| v.as_str()).map(String::from);
            let name = explicit.or_else(|| state.selected_interface.lock().unwrap_or_else(|p| p.into_inner()).clone());
            let res = if let Some(name) = name {
                let d = get_interface_details(&name);
                if let (Some(ip), Some(mask)) = (d.ip, d.subnet) {
                    cidr_from_ip_mask(&ip, &mask)
                } else {
                    None
                }
            } else {
                None
            }
            .or_else(get_local_network_cidr);
            Ok(json!(res))
        }

        "cmd_scan_network" => {
            let cidr = args.get("cidr").and_then(|v| v.as_str()).ok_or("missing cidr")?.to_string();
            let (first, last) = parse_cidr(&cidr)?;
            let src = resolve_src_strict(state)?;
            if state
                .scan_cancel
                .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                return Err("A network scan is already in progress".to_string());
            }
            state.discovery.clear();
            let cancel = state.scan_cancel.clone();
            let events = state.events.clone();
            let discovery = state.discovery.clone();
            let cidr_for_spawn = cidr.clone();

            tokio::spawn(async move {
                let cidr = cidr_for_spawn;
                let arp_initial = read_arp_table().await;
                if cancel.load(Ordering::SeqCst) {
                    let _ = events.send(done_event(&cidr, 0));
                    return;
                }
                for (ip, mac) in &arp_initial {
                    let ip_int: Option<u32> = parse_ip_u32(ip);
                    if let Some(i) = ip_int {
                        if i >= first && i <= last {
                            let host = DiscoveredHost {
                                ip: ip.clone(),
                                hostname: None,
                                mac: Some(mac.clone()),
                                vendor: lanprobe_core::oui::vendor_for_mac(mac),
                                latency_ms: None,
                            };
                            discovery.upsert(host.clone());
                            let _ = events.send(crate::state::BroadcastEvent {
                                event: "discovery:host".into(),
                                payload: serde_json::to_value(&host).unwrap_or(Value::Null),
                            });
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
                        let events_c = events.clone();
                        let discovery_c = discovery.clone();
                        handles.push(tokio::spawn(async move {
                            if let Some(lat) = ping_once_fast_retry(&ip, src, 3).await {
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
                                        payload: serde_json::to_value(&host).unwrap_or(Value::Null),
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
                    for h in handles { let _ = h.await; }
                }
                if cancel.load(Ordering::SeqCst) {
                    let _ = events.send(done_event(&cidr, 0));
                    return;
                }
                let arp_after = read_arp_table().await;
                for (ip, mac) in &arp_after {
                    if arp_initial.contains_key(ip) { continue; }
                    if let Some(i) = parse_ip_u32(ip) {
                        if i >= first && i <= last {
                            discovery.update_mac(ip, mac.clone());
                            let _ = events.send(crate::state::BroadcastEvent {
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
                let hosts_found = discovery.snapshot().len();
                let _ = events.send(done_event(&cidr, hosts_found));
                // Remettre scan_cancel à true (idle) : le scan est terminé
                // normalement, on libère le verrou pour le scheduler.
                cancel.store(true, Ordering::SeqCst);
            });
            Ok(Value::Null)
        }
        "cmd_cancel_scan" => {
            state.scan_cancel.store(true, Ordering::SeqCst);
            let _ = state.events.send(done_event("", 0));
            Ok(Value::Null)
        }

        "cmd_scan_ports" => {
            let ip = args.get("ip").and_then(|v| v.as_str()).ok_or("missing ip")?.to_string();
            let ports = args.get("ports").and_then(|v| serde_json::from_value(v.clone()).ok());
            let profile_id = args.get("profileId").and_then(|v| v.as_str()).map(String::from);
            let src = resolve_src_strict(state)?;
            state.portscan.mark_in_progress(&ip, profile_id.clone());
            let _ = state.events.send(crate::state::BroadcastEvent {
                event: "portscan:update".into(),
                payload: json!({ "ip": ip, "in_progress": true, "profile_id": profile_id }),
            });
            let results = scan_ports(&ip, src, ports).await;
            let entry = state.portscan.set_tcp(&ip, results.clone(), now_secs(), profile_id);
            let _ = state.events.send(crate::state::BroadcastEvent {
                event: "portscan:update".into(),
                payload: serde_json::to_value(&entry).unwrap_or(Value::Null),
            });
            Ok(json!(results))
        }
        "cmd_scan_udp_ports" => {
            let ip = args.get("ip").and_then(|v| v.as_str()).ok_or("missing ip")?.to_string();
            let ports = args.get("ports").and_then(|v| serde_json::from_value(v.clone()).ok());
            let src = resolve_src_strict(state)?;
            let results = scan_udp_ports(&ip, src, ports).await;
            let entry = state.portscan.set_udp(&ip, results.clone(), now_secs());
            let _ = state.events.send(crate::state::BroadcastEvent {
                event: "portscan:update".into(),
                payload: serde_json::to_value(&entry).unwrap_or(Value::Null),
            });
            Ok(json!(results))
        }
        "cmd_get_portscan_snapshot" => {
            Ok(serde_json::to_value(state.portscan.snapshot()).unwrap_or(Value::Null))
        }
        "cmd_clear_portscan_entry" => {
            let ip = args.get("ip").and_then(|v| v.as_str()).ok_or("missing ip")?.to_string();
            state.portscan.remove(&ip);
            let _ = state.events.send(crate::state::BroadcastEvent {
                event: "portscan:removed".into(),
                payload: json!({ "ip": ip }),
            });
            Ok(Value::Null)
        }

        "cmd_compute_sla" => {
            let ip = args.get("ip").and_then(|v| v.as_str()).ok_or("missing ip")?;
            let samples: Vec<PingSampleDto> =
                serde_json::from_value(args.get("samples").cloned().unwrap_or(Value::Null))
                    .map_err(|e| e.to_string())?;
            let s: Vec<PingSample> = samples
                .into_iter()
                .map(|d| PingSample { alive: d.alive, latency_ms: d.latency_ms })
                .collect();
            serde_json::to_value(compute_sla(ip, &s)).map_err(|e| e.to_string())
        }

        "cmd_run_speedtest" => {
            let src = resolve_src_strict(state)?;
            let name = state.selected_interface.lock().unwrap_or_else(|p| p.into_inner()).clone();
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
            let _ = state.events.send(crate::state::BroadcastEvent {
                event: "speedtest:running".into(),
                payload: json!({ "running": true }),
            });
            let res = run_speedtest(src, iface_for_cli).await;
            match &res {
                Ok(r) => {
                    state.speedtest.set(r.clone());
                    let _ = state.events.send(crate::state::BroadcastEvent {
                        event: "speedtest:result".into(),
                        payload: serde_json::to_value(r).unwrap_or(Value::Null),
                    });
                }
                Err(_) => {
                    state.speedtest.mark_stopped();
                    let _ = state.events.send(crate::state::BroadcastEvent {
                        event: "speedtest:running".into(),
                        payload: json!({ "running": false }),
                    });
                }
            }
            res.and_then(|r| serde_json::to_value(r).map_err(|e| e.to_string()))
        }
        "cmd_run_iperf3" => {
            let server = args.get("server").and_then(|v| v.as_str()).ok_or("missing server")?;
            let src = resolve_src_strict(state)?;
            state.speedtest.mark_running();
            let _ = state.events.send(crate::state::BroadcastEvent {
                event: "speedtest:running".into(),
                payload: json!({ "running": true }),
            });
            let res = run_iperf3(server, src).await;
            match &res {
                Ok(r) => {
                    state.speedtest.set(r.clone());
                    let _ = state.events.send(crate::state::BroadcastEvent {
                        event: "speedtest:result".into(),
                        payload: serde_json::to_value(r).unwrap_or(Value::Null),
                    });
                }
                Err(_) => {
                    state.speedtest.mark_stopped();
                    let _ = state.events.send(crate::state::BroadcastEvent {
                        event: "speedtest:running".into(),
                        payload: json!({ "running": false }),
                    });
                }
            }
            res.and_then(|r| serde_json::to_value(r).map_err(|e| e.to_string()))
        }
        "cmd_get_speedtest_snapshot" => {
            Ok(json!({
                "latest": state.speedtest.snapshot(),
                "running": state.speedtest.is_running(),
            }))
        }

        "cmd_get_internet_status" => Ok(serde_json::to_value(state.internet.snapshot()).unwrap_or(Value::Null)),
        "cmd_reset_internet_monitor" => {
            state.internet.reset();
            Ok(Value::Null)
        }

        "cmd_get_public_ip" => {
            let src = resolve_src_strict(state)?;
            get_public_ip(src).await.and_then(|r| serde_json::to_value(r).map_err(|e| e.to_string()))
        }

        "cmd_open_url" => {
            // En mode serveur, ouvrir une URL côté hôte n'a pas de sens —
            // le client (navigateur) peut juste faire `window.open`. On
            // répond OK pour ne pas casser l'UI mais on ne fait rien.
            Ok(Value::Null)
        }

        "cmd_check_update" => {
            // En mode headless, on vérifie la dispo d'une mise à jour mais on
            // ne propose pas l'installation automatique (le service tourne sans
            // droits root). Le bandeau affiche juste un lien vers la release.
            let mut info = updater::check_update(true).await?;
            info.asset_url = None;   // pas de bouton "installer"
            info.asset_name = None;
            info.platform_supported = false; // → bannière "voir" au lieu de "installer"
            serde_json::to_value(info).map_err(|e| e.to_string())
        }
        "cmd_apply_update" => {
            Err("use install-server.sh to update the headless server".into())
        }

        "cmd_test_influxdb" => {
            match crate::influxdb::test_connection(state.clone()).await {
                Ok(()) => Ok(serde_json::json!({ "ok": true })),
                Err(e) => Ok(serde_json::json!({ "ok": false, "error": e })),
            }
        }

        _ => Err(format!("unknown command: {cmd}")),
    }
}

fn resolve_src_from(iface: &std::sync::Arc<std::sync::Mutex<Option<String>>>) -> Result<Option<Ipv4Addr>, ()> {
    let Ok(guard) = iface.lock() else { return Ok(None); };
    let Some(name) = guard.clone() else { return Ok(None); };
    drop(guard);
    let d = get_interface_details(&name);
    let Some(ip_str) = d.ip else { return Err(()); };
    ip_str.parse::<Ipv4Addr>().map(Some).map_err(|_| ())
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
        payload: serde_json::json!({ "cidr": cidr, "hosts_found": hosts_found }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_done_event_payload() {
        let event = done_event("192.168.1.0/24", 42);
        assert_eq!(event.event, "discovery:done");
        assert_eq!(event.payload["cidr"], "192.168.1.0/24");
        assert_eq!(event.payload["hosts_found"], 42);
    }
}

#[derive(Deserialize)]
struct ApplyStaticArgs {
    interface: String,
    ip: String,
    subnet: String,
    gateway: String,
    dns_primary: String,
    #[serde(default)]
    dns_secondary: Option<String>,
}

#[derive(Deserialize)]
struct PingSampleDto {
    alive: bool,
    latency_ms: Option<u64>,
}
