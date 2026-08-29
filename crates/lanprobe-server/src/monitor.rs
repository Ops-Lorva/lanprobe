//! Surveillance ICMP d'une cible, en continu.
//!
//! ⚠️ La boucle vivait dans la couche Tauri, donc **uniquement dans
//! l'application de bureau**. Une sonde headless ne pouvait surveiller aucune
//! cible, et le hub n'avait rien à piloter. Elle vit ici pour que les deux —
//! fenêtre et commande venue du hub (contrat § 14) — démarrent exactement la
//! même surveillance, avec la même contrainte d'interface.

use std::net::Ipv4Addr;
use std::time::Duration;

use lanprobe_core::interfaces::get_interface_details;
use lanprobe_core::ping::{self, ping_once};

use crate::state::AppState;

/// Adresse source imposée par l'interface sélectionnée.
///
/// ⚠️ `Err` quand une interface est choisie mais n'a pas d'IPv4 : le ping
/// doit alors être compté comme un échec, jamais reporté sur une autre
/// interface. Un résultat pris par le mauvais lien est pire qu'aucun
/// résultat — il est faux et on ne le sait pas.
fn source_address(state: &AppState) -> Result<Option<Ipv4Addr>, ()> {
    let name = state
        .selected_interface
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let Some(name) = name else { return Ok(None) };
    match get_interface_details(&name)
        .ip
        .and_then(|s| s.parse::<Ipv4Addr>().ok())
    {
        Some(ip) => Ok(Some(ip)),
        None => Err(()),
    }
}

/// Vrai si cette cible est déjà surveillée.
pub fn is_monitored(state: &AppState, ip: &str) -> bool {
    state
        .ping_stop
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(ip)
        .is_some_and(|stopped| !*stopped)
}

/// Démarre la surveillance d'une cible. Sans effet si elle tourne déjà :
/// deux boucles sur la même adresse doubleraient chaque point.
pub fn start(state: &AppState, ip: &str) {
    if is_monitored(state, ip) {
        return;
    }
    {
        let mut map = state.ping_stop.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(ip.to_string(), false);
    }
    let state = state.clone();
    let ip = ip.to_string();
    tokio::spawn(async move {
        loop {
            {
                let Ok(map) = state.ping_stop.lock() else { break };
                if *map.get(&ip).unwrap_or(&true) {
                    break;
                }
            }
            // Pendant un changement de profil réseau, l'interface est
            // brièvement absente : ne rien inscrire vaut mieux qu'inscrire une
            // panne qui n'a pas eu lieu.
            if state.is_monitoring_blackout() {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            let result = match source_address(&state) {
                Ok(src) => ping_once(&ip, src).await,
                Err(()) => ping::PingResult {
                    ip: ip.clone(),
                    alive: false,
                    latency_ms: None,
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                },
            };
            state.monitoring.push(result.clone());
            state.emit(
                "ping:tick",
                serde_json::to_value(&result).unwrap_or(serde_json::Value::Null),
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

/// Arrête la surveillance et purge l'historique de la cible.
///
/// ⚠️ La purge n'est pas un détail : sans elle, l'instantané renvoyé à
/// l'interface réintroduit l'hôte, qui « réapparaît » après suppression.
pub fn stop(state: &AppState, ip: &str) {
    {
        let mut map = state.ping_stop.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(ip.to_string(), true);
    }
    state.monitoring.clear_ip(ip);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigStore;
    use std::sync::Arc;

    fn state() -> AppState {
        AppState::new_headless(Arc::new(ConfigStore::load(
            std::env::temp_dir().join("lanprobe-monitor-test.json"),
        )))
    }

    #[tokio::test]
    async fn stopping_a_target_removes_it_from_the_monitored_set() {
        let state = state();
        start(&state, "10.0.0.1");
        assert!(is_monitored(&state, "10.0.0.1"));
        stop(&state, "10.0.0.1");
        assert!(!is_monitored(&state, "10.0.0.1"));
    }

    #[tokio::test]
    async fn an_unknown_target_is_not_monitored() {
        assert!(!is_monitored(&state(), "10.0.0.9"));
    }
}
