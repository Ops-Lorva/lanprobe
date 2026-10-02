//! Exécution des commandes venues du hub (contrat § 14).
//!
//! ⚠️ Le hub ne joint jamais la sonde : il n'a aucune route vers son réseau,
//! et c'est le but. Les commandes voyagent donc dans la réponse au battement
//! de cœur, et l'accusé repart au battement suivant. La latence est d'au plus
//! un intervalle — invisible pour un scan ou un test de débit qui durent
//! plusieurs minutes.
//!
//! Chaque commande est une **action sur le réseau d'un client**. On n'invente
//! donc rien ici : le `kind` est déjà validé par le hub contre une liste
//! fermée, et une commande inconnue est refusée plutôt qu'ignorée — un accusé
//! d'échec explicite vaut mieux qu'une commande qui reste « en attente »
//! jusqu'à l'abandon.

use serde::{Deserialize, Serialize};

use crate::state::AppState;
use crate::{monitor, scheduler};

/// Commande telle qu'elle arrive dans la réponse au battement.
#[derive(Debug, Clone, Deserialize)]
pub struct Command {
    pub id: i64,
    pub kind: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// Verdict renvoyé au hub au battement suivant.
#[derive(Debug, Clone, Serialize)]
pub struct Ack {
    pub id: i64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Une liste de ports d'un argument de commande.
///
/// ⚠️ **Vide est rendu comme absent** : une liste vide veut dire « la sonde
/// garde la sienne », jamais « ne scanne rien ». Les confondre ferait un scan
/// complet là où on croyait restreindre (§ 25).
fn port_list(command: &Command, name: &str) -> Option<Vec<u16>> {
    command
        .args
        .get(name)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|p| p.as_u64())
                .filter(|p| *p >= 1 && *p <= 65_535)
                .map(|p| p as u16)
                .collect()
        })
        .filter(|v: &Vec<u16>| !v.is_empty())
}

fn arg<'a>(command: &'a Command, name: &str) -> Option<&'a str> {
    command
        .args
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// Exécute une commande et rend son verdict.
///
/// Toute erreur remonte telle quelle dans l'accusé : c'est ce texte que
/// l'opérateur lira dans le hub, il doit dire ce qui s'est passé et non
/// « échec ».
pub async fn execute(state: &AppState, command: &Command) -> Ack {
    let result = match command.kind.as_str() {
        "speedtest" => {
            // Le hub peut imposer le moteur et le serveur. Sans surcharge, la
            // sonde utilise sa propre configuration — c'est le cas du test
            // planifié, qu'une commande ne doit pas modifier.
            let engine = arg(command, "engine");
            let server = arg(command, "server");
            if engine == Some("iperf3") && server.is_none() {
                return Ack {
                    id: command.id,
                    ok: false,
                    error: Some("iperf3 demandé sans serveur".into()),
                };
            }
            scheduler::speedtest_with(state, engine, server).await;
            Ok(())
        }
        "port_scan" => match arg(command, "ip") {
            Some(ip) => {
                // Le hub peut imposer les listes de ports. Vides ou absentes,
                // la sonde reprend les siennes — une liste vide ne veut jamais
                // dire « ne scanne rien » (§ 25).
                let ports = port_list(command, "ports");
                let udp_ports = port_list(command, "udp_ports");
                // 🔴 **L'étiquette du profil employé.** Sans elle, un scan
                // lancé depuis le hub ou depuis le téléphone s'affichait sur
                // l'écran de la sonde SANS aucun profil, alors qu'un scan
                // lancé dans sa fenêtre montrait le sien — constaté le 02/10.
                //
                // ⚠️ Elle n'est pas validée, et c'est volontaire : les ports
                // font foi, le profil n'est qu'une étiquette. La sonde n'a
                // peut-être pas encore reçu le profil que le hub vient de
                // créer, et un scan qui marche ne doit pas tomber pour ça.
                let profile_id = arg(command, "profile_id").map(str::to_string);
                scheduler::portscan_with(state, ip, ports, udp_ports, profile_id)
                    .await
                    .map(|_| ())
            }
            None => Err("adresse manquante".into()),
        },
        "discovery" => {
            // CIDR vide : la découverte le déduit de l'interface sélectionnée.
            // C'est le cas courant depuis le hub, qui ne connaît pas le plan
            // d'adressage du site.
            scheduler::discovery_once(state, arg(command, "cidr").unwrap_or("").to_string()).await;
            Ok(())
        }
        "add_monitor" => match arg(command, "ip") {
            Some(ip) => {
                monitor::start(state, ip);
                Ok(())
            }
            None => Err("adresse manquante".into()),
        },
        "remove_monitor" => match arg(command, "ip") {
            Some(ip) => {
                monitor::stop(state, ip);
                Ok(())
            }
            None => Err("adresse manquante".into()),
        },
        other => Err(format!("commande inconnue : {other}")),
    };

    match result {
        Ok(()) => Ack { id: command.id, ok: true, error: None },
        Err(e) => {
            tracing::warn!("commande {} ({}) échouée : {e}", command.id, command.kind);
            Ack { id: command.id, ok: false, error: Some(e) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigStore;
    use std::sync::Arc;

    fn state() -> AppState {
        AppState::new_headless(Arc::new(ConfigStore::load(
            std::env::temp_dir().join("lanprobe-commands-test.json"),
        )))
    }

    fn command(kind: &str, args: serde_json::Value) -> Command {
        Command { id: 1, kind: kind.into(), args }
    }

    #[tokio::test]
    async fn un_scan_commande_range_le_profil_qui_a_servi() {
        // 🔴 Défaut constaté le 02/10 : un scan lancé depuis le hub ou depuis
        // le téléphone n'affichait AUCUN profil sur l'écran de la sonde, alors
        // qu'un scan lancé dans la fenêtre de la sonde montrait le sien. La
        // commande ne portait que la liste de ports : personne ne disait de
        // quel profil elle venait, et le résultat était donc rangé sans
        // étiquette.
        //
        // Scan sur 127.0.0.1, un seul port : local et instantané.
        let state = state();
        let ack = execute(
            &state,
            &command(
                "port_scan",
                serde_json::json!({ "ip": "127.0.0.1", "ports": [1], "profile_id": "web" }),
            ),
        )
        .await;
        assert!(ack.ok, "{ack:?}");

        let entry = state
            .portscan
            .snapshot()
            .into_iter()
            .find(|e| e.ip == "127.0.0.1")
            .expect("le scan doit avoir laissé une entrée");
        assert_eq!(entry.profile_id.as_deref(), Some("web"));
    }

    #[tokio::test]
    async fn un_profil_inconnu_de_la_sonde_ne_fait_pas_echouer_le_scan() {
        // ⚠️ **Les ports font foi, le profil n'est qu'une étiquette.** La sonde
        // n'a pas forcément reçu le profil que le hub vient de créer — il
        // descendra au battement suivant. Un scan qui marche ne doit pas tomber
        // pour une étiquette qu'on ne sait pas encore traduire : elle est
        // rangée telle quelle, et l'écran l'affichera dès qu'il la connaîtra.
        let state = state();
        let ack = execute(
            &state,
            &command(
                "port_scan",
                serde_json::json!({ "ip": "127.0.0.1", "ports": [1], "profile_id": "jamais-vu" }),
            ),
        )
        .await;
        assert!(ack.ok, "{ack:?}");

        let entry = state
            .portscan
            .snapshot()
            .into_iter()
            .find(|e| e.ip == "127.0.0.1")
            .expect("le scan doit avoir laissé une entrée");
        assert_eq!(entry.profile_id.as_deref(), Some("jamais-vu"));
    }

    #[tokio::test]
    async fn un_scan_commande_fait_aussi_l_udp_quand_le_hub_en_demande() {
        // Le hub modélise désormais les ports UDP d'un profil (§ 25) : une
        // commande qui en porte doit les scanner, sinon un profil « Common »
        // lancé depuis le hub ne rendrait que la moitié de ce que le même
        // profil rend depuis la fenêtre de la sonde.
        let state = state();
        let ack = execute(
            &state,
            &command(
                "port_scan",
                serde_json::json!({ "ip": "127.0.0.1", "ports": [1], "udp_ports": [1] }),
            ),
        )
        .await;
        assert!(ack.ok, "{ack:?}");

        let entry = state
            .portscan
            .snapshot()
            .into_iter()
            .find(|e| e.ip == "127.0.0.1")
            .expect("le scan doit avoir laissé une entrée");
        assert_eq!(entry.udp.len(), 1, "{entry:?}");
    }

    #[tokio::test]
    async fn an_unknown_command_fails_explicitly() {
        // Ignorer une commande inconnue la laisserait « en attente » jusqu'à
        // l'abandon au bout de trois remises, sans jamais dire pourquoi.
        let ack = execute(&state(), &command("efface_tout", serde_json::json!({}))).await;
        assert!(!ack.ok);
        assert!(ack.error.unwrap().contains("inconnue"));
    }

    #[tokio::test]
    async fn a_scan_without_an_address_says_so() {
        let ack = execute(&state(), &command("port_scan", serde_json::json!({}))).await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_deref(), Some("adresse manquante"));
    }

    #[tokio::test]
    async fn an_empty_address_counts_as_missing() {
        // `{"ip": ""}` n'est pas une adresse : sans ce filtre, le scan
        // partirait sur une chaîne vide et rendrait « tous les ports fermés ».
        let ack = execute(&state(), &command("port_scan", serde_json::json!({"ip": "  "}))).await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_deref(), Some("adresse manquante"));
    }

    #[tokio::test]
    async fn adding_then_removing_a_monitor_is_acknowledged() {
        let state = state();
        let add = execute(&state, &command("add_monitor", serde_json::json!({"ip": "10.0.0.1"}))).await;
        assert!(add.ok, "{:?}", add.error);
        assert!(monitor::is_monitored(&state, "10.0.0.1"));

        let remove =
            execute(&state, &command("remove_monitor", serde_json::json!({"ip": "10.0.0.1"}))).await;
        assert!(remove.ok);
        assert!(!monitor::is_monitored(&state, "10.0.0.1"));
    }

    #[tokio::test]
    async fn iperf_without_a_server_says_which_argument_is_missing() {
        // ⚠️ « Échec » enverrait chercher du côté du réseau un problème qui
        // est dans la commande. Et lancer iperf3 sans serveur n'aurait rien à
        // mesurer : autant refuser tout de suite.
        let ack = execute(
            &state(),
            &command("speedtest", serde_json::json!({ "engine": "iperf3" })),
        )
        .await;
        assert!(!ack.ok);
        assert!(ack.error.unwrap().contains("serveur"));
    }

    #[tokio::test]
    async fn a_scan_on_a_malformed_address_reports_the_address() {
        let ack =
            execute(&state(), &command("port_scan", serde_json::json!({"ip": "pas-une-ip"}))).await;
        assert!(!ack.ok);
        assert!(ack.error.unwrap().contains("pas-une-ip"));
    }
}
