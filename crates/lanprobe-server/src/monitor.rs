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
use lanprobe_core::ping::ping_once_checked;

use crate::state::AppState;

/// Adresse source imposée par l'interface sélectionnée.
///
/// ⚠️ `Err` quand une interface est choisie mais n'a pas d'IPv4 — câble
/// débranché, lien tombé, bail DHCP perdu. **On ne mesure pas ce tour-ci**, et
/// on n'écrit rien : c'est un fait sur NOTRE machine, pas sur la cible.
///
/// ⚠️ Cette consigne disait « le ping doit alors être compté comme un échec ».
/// Son propre argument la contredisait : *un résultat pris par le mauvais lien
/// est pire qu'aucun résultat — il est faux et on ne le sait pas*. Sauter
/// évite le mauvais lien tout autant, sans inventer de panne. Écrire
/// `alive: false` fabriquait un faux rouge une fois par seconde et par cible,
/// jusque dans le rapport remis au client.
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

/// Démarre la surveillance d'une cible **à la demande d'ici** — la fenêtre,
/// ou une commande du hub. Le geste est noté dans la file des changements pour
/// que le hub l'apprenne au battement suivant.
pub fn start(state: &AppState, ip: &str) {
    // ⚠️ Noté même si la boucle tournait déjà : le hub a pu retirer la cible
    // de son côté, et c'est ce geste-ci, plus récent, qui doit gagner
    // l'arbitrage. Sans lui, la cible s'arrêterait au battement suivant.
    crate::hub::record_monitor_change(state, ip, false);
    start_from_hub(state, ip);
}

/// Arrête la surveillance à la demande d'ici, et **écrit le retrait**.
///
/// ⚠️ Une absence n'est pas une suppression : sans ce fait écrit, le hub ne
/// verrait que « cette cible ne m'est plus annoncée », ce qui ne dit rien —
/// une app éteinte n'annonce rien non plus.
pub fn stop(state: &AppState, ip: &str) {
    crate::hub::record_monitor_change(state, ip, true);
    stop_from_hub(state, ip);
}

/// Démarre la surveillance d'une cible. Sans effet si elle tourne déjà :
/// deux boucles sur la même adresse doubleraient chaque point.
///
/// ⚠️ Ne note **rien** dans la file : c'est l'alignement sur la liste du hub.
/// Lui renvoyer sa propre décision datée de maintenant la ferait gagner contre
/// l'ordre qui vient de l'émettre, et les deux côtés se répondraient sans fin.
pub fn start_from_hub(state: &AppState, ip: &str) {
    if is_monitored(state, ip) {
        return;
    }
    {
        let mut map = state.ping_stop.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(ip.to_string(), false);
    }
    // ⚠️ La fenêtre n'apprend l'état des surveillances que par les événements :
    // sans celui-ci, une cible ajoutée depuis le hub n'apparaissait qu'au
    // redémarrage de l'app. Émis ici et pas plus haut : la sortie anticipée
    // ci-dessus veut dire « rien n'a changé », et un événement l'annoncerait.
    state.emit("monitor:started", serde_json::json!({ "ip": ip }));
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
            // ⚠️ Interface choisie sans IPv4 : on ne mesure pas. C'est le
            // TROISIÈME échec local de cette boucle, et il se traite comme les
            // deux autres — la bascule de profil juste au-dessus, et les
            // sockets ICMP épuisés juste en dessous. Il était le seul à
            // inventer une réponse : `alive: false` une fois par seconde et
            // par cible, sur la foi d'un câble débranché chez nous.
            let Ok(src) = source_address(&state) else {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            };
            // ⚠️ `ping_once_checked` rend `None` quand la machine n'a pas pu
            // ÉMETTRE — sockets ICMP épuisés, typiquement par un scan réseau
            // en cours. On saute alors le relevé au lieu d'écrire « hôte
            // mort » : c'est un fait sur nous, pas sur la cible. Sans ça, un
            // scan d'un /16 fabriquait 22 % de fausse indisponibilité sur des
            // cibles parfaitement joignables.
            let Some(result) = ping_once_checked(&ip, src).await else {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
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
///
/// Ne note rien dans la file, pour la même raison que `start_from_hub`.
pub fn stop_from_hub(state: &AppState, ip: &str) {
    {
        let mut map = state.ping_stop.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(ip.to_string(), true);
    }
    state.monitoring.clear_ip(ip);
    // 🔴 Purger l'historique ne suffit PAS à faire partir la ligne : la
    // fenêtre garde en mémoire ce qu'elle a déjà reçu, et ne relit
    // l'instantané qu'au démarrage. Constaté le 29/09 — une cible retirée
    // depuis le téléphone n'était plus pinguée, mais restait affichée avec ses
    // derniers chiffres jusqu'au redémarrage de l'app.
    state.emit("monitor:stopped", serde_json::json!({ "ip": ip }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigStore;
    use std::sync::Arc;

    /// Un fichier de configuration par test **et par processus** : les gestes
    /// de surveillance sont désormais persistés, deux tests qui partageraient
    /// le fichier se marcheraient dessus.
    fn state(tag: &str) -> AppState {
        AppState::new_headless(Arc::new(ConfigStore::load(
            std::env::temp_dir().join(format!(
                "lanprobe-monitor-{tag}-{}.json",
                std::process::id()
            )),
        )))
    }

    /// Les événements vus depuis l'abonnement, sous la forme `(nom, ip)`.
    fn drained(rx: &mut tokio::sync::broadcast::Receiver<crate::state::BroadcastEvent>) -> Vec<(String, String)> {
        let mut seen = Vec::new();
        while let Ok(event) = rx.try_recv() {
            seen.push((
                event.event.clone(),
                event.payload["ip"].as_str().unwrap_or_default().to_string(),
            ));
        }
        seen
    }

    #[tokio::test]
    async fn a_removal_decided_by_the_hub_is_announced_to_the_window() {
        // 🔴 Le défaut que ça corrige, constaté le 29/09 sur le Mac de
        // Benjamin : une cible retirée depuis le téléphone cessait bien d'être
        // pinguée — la sonde l'annonçait au hub — mais sa ligne RESTAIT à
        // l'écran avec ses derniers chiffres, jusqu'au redémarrage de l'app.
        // La fenêtre n'apprend l'état des surveillances que par ces
        // événements ; sans celui-ci, elle affichait une mesure qui n'existait
        // plus.
        let state = state("retrait-du-hub");
        let mut events = state.events.subscribe();
        // 192.0.2.0/24 est le réseau de documentation : aucun paquet n'a de
        // destinataire, et la boucle n'a rien à mesurer.
        start_from_hub(&state, "192.0.2.7");
        stop_from_hub(&state, "192.0.2.7");
        let seen = drained(&mut events);
        assert!(
            seen.contains(&("monitor:stopped".to_string(), "192.0.2.7".to_string())),
            "le retrait n'a rien annoncé : {seen:?}"
        );
    }

    #[tokio::test]
    async fn an_addition_decided_by_the_hub_is_announced_to_the_window() {
        // Le même défaut, à l'envers : une cible ajoutée depuis le téléphone
        // n'apparaissait dans la fenêtre qu'au redémarrage.
        let state = state("ajout-du-hub");
        let mut events = state.events.subscribe();
        start_from_hub(&state, "192.0.2.8");
        let seen = drained(&mut events);
        assert!(
            seen.contains(&("monitor:started".to_string(), "192.0.2.8".to_string())),
            "l'ajout n'a rien annoncé : {seen:?}"
        );
    }

    #[tokio::test]
    async fn a_selected_interface_without_ipv4_writes_no_sample_at_all() {
        // 🔴 L'interface choisie n'a pas d'IPv4 — câble débranché, lien tombé,
        // bail DHCP perdu. C'est un fait sur NOTRE machine, pas sur la cible.
        // La boucle écrivait pourtant `alive: false`, une fois par seconde,
        // pour CHAQUE cible surveillée — jusque dans Influx, donc jusque dans
        // le rapport SLA remis au client. Un faux rouge fabrique une panne, et
        // fait déplacer quelqu'un pour rien.
        //
        // Les deux autres échecs locaux de cette même boucle sautent déjà le
        // relevé — sockets ICMP épuisés, et bascule de profil réseau. Celui-ci
        // était le seul à inventer une réponse.
        //
        // ⚠️ Aucun paquet ne part : le nom d'interface n'existe pas, donc
        // `source_address` échoue avant tout ping. 192.0.2.0/24 est de toute
        // façon la plage de documentation, jamais routée.
        let state = state("no-ipv4");
        *state
            .selected_interface
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some("interface-qui-nexiste-pas".into());

        start(&state, "192.0.2.99");
        tokio::time::sleep(Duration::from_millis(1_500)).await;

        // ⚠️ On lit AVANT d'arrêter : `stop` purge l'historique de la cible
        // (`clear_ip`). Lire après rendrait ce test vert quoi qu'il arrive —
        // il l'était, d'ailleurs, à la première écriture.
        let ecrits = state
            .monitoring
            .snapshot()
            .get("192.0.2.99")
            .map(|v| v.len())
            .unwrap_or(0);
        stop(&state, "192.0.2.99");
        assert_eq!(ecrits, 0, "aucun échantillon ne doit être écrit, surtout pas un `alive: false`");
    }

    #[tokio::test]
    async fn stopping_a_target_removes_it_from_the_monitored_set() {
        let state = state("stop");
        start(&state, "10.0.0.1");
        assert!(is_monitored(&state, "10.0.0.1"));
        stop(&state, "10.0.0.1");
        assert!(!is_monitored(&state, "10.0.0.1"));
    }

    #[tokio::test]
    async fn an_unknown_target_is_not_monitored() {
        assert!(!is_monitored(&state("unknown"), "10.0.0.9"));
    }

    #[tokio::test]
    async fn removing_a_target_writes_an_explicit_removal() {
        // ⚠️ Une absence n'est PAS une suppression : sans ce changement écrit,
        // le hub ne saurait jamais que la cible a été retirée — il verrait
        // seulement qu'on ne la lui annonce plus, ce qui ne veut rien dire et
        // ferait revenir la cible au battement suivant.
        let state = state("removal-is-written");
        start(&state, "192.0.2.20");
        stop(&state, "192.0.2.20");

        let changes = crate::hub::outgoing_monitor_changes(&state);
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].target, "192.0.2.20");
        assert_eq!(changes[0].action, "remove", "le geste le plus récent");
    }

    #[tokio::test]
    async fn aligning_on_the_hub_writes_nothing() {
        // Le hub n'a pas à s'entendre répéter sa propre décision : le geste
        // repartirait daté de maintenant et gagnerait l'arbitrage contre
        // l'ordre qui vient de l'émettre.
        let state = state("align-is-silent");
        start_from_hub(&state, "192.0.2.21");
        stop_from_hub(&state, "192.0.2.21");
        assert!(crate::hub::outgoing_monitor_changes(&state).is_empty());
    }
}
