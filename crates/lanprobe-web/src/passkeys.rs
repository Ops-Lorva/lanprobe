//! Clés d'accès (WebAuthn / FIDO2).
//!
//! ⚠️ **Rien de tout cela ne fonctionne sans contexte sécurisé.** Le
//! navigateur refuse l'API WebAuthn avant même d'atteindre le hub dès que la
//! page n'est ni en HTTPS ni sur `localhost`. Ce n'est pas une limite du
//! produit et aucun code ici ne peut la lever : l'interrupteur HTTPS des
//! réglages, ou un reverse proxy, sont les deux seules réponses.
//!
//! Le hub étant auto-hébergé, il n'a pas de domaine connu à l'avance : le
//! `rp_id` se déduit de l'hôte par lequel la page a été servie. C'est le seul
//! choix qui marche pour un produit qu'on joint tantôt par un nom, tantôt par
//! une IP, sans rien demander à l'utilisateur.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use webauthn_rs::prelude::*;

/// Durée de vie d'une cérémonie entamée.
///
/// Deux minutes : le temps de sortir son téléphone, de le déverrouiller et de
/// valider. Plus long laisserait s'accumuler des défis en attente, ce qui est
/// une file qu'on peut remplir de l'extérieur ; plus court ferait échouer
/// quiconque cherche sa clé au fond d'un sac.
const CEREMONY_TTL: Duration = Duration::from_secs(120);

/// Le nom montré par le navigateur au moment de choisir la clé.
const RP_NAME: &str = "LanProbe Hub";

pub struct Ceremonies {
    registrations: Mutex<HashMap<String, (PasskeyRegistration, Instant)>>,
    authentications: Mutex<HashMap<String, (PasskeyAuthentication, Instant)>>,
}

impl Default for Ceremonies {
    fn default() -> Self {
        Self::new()
    }
}

impl Ceremonies {
    pub fn new() -> Self {
        Self {
            registrations: Mutex::new(HashMap::new()),
            authentications: Mutex::new(HashMap::new()),
        }
    }

    pub fn put_registration(&self, username: &str, state: PasskeyRegistration) {
        if let Ok(mut map) = self.registrations.lock() {
            prune(&mut map);
            map.insert(username.to_string(), (state, Instant::now()));
        }
    }

    /// Reprend et **consomme** la cérémonie. Un défi qui resterait après usage
    /// pourrait être rejoué.
    pub fn take_registration(&self, username: &str) -> Option<PasskeyRegistration> {
        let mut map = self.registrations.lock().ok()?;
        prune(&mut map);
        map.remove(username).map(|(state, _)| state)
    }

    pub fn put_authentication(&self, username: &str, state: PasskeyAuthentication) {
        if let Ok(mut map) = self.authentications.lock() {
            prune(&mut map);
            map.insert(username.to_string(), (state, Instant::now()));
        }
    }

    pub fn take_authentication(&self, username: &str) -> Option<PasskeyAuthentication> {
        let mut map = self.authentications.lock().ok()?;
        prune(&mut map);
        map.remove(username).map(|(state, _)| state)
    }
}

fn prune<T>(map: &mut HashMap<String, (T, Instant)>) {
    let now = Instant::now();
    map.retain(|_, (_, started)| now.duration_since(*started) < CEREMONY_TTL);
}

/// Construit le contexte WebAuthn pour l'hôte par lequel la page est servie.
///
/// ⚠️ Le `rp_id` est le **domaine seul**, sans port ni schéma ; l'origine, elle,
/// les porte. Les confondre fait échouer toutes les cérémonies avec un message
/// que le navigateur ne détaille pas.
///
/// ⚠️ Une clé est liée au `rp_id` qui l'a créée. Joindre le hub par son IP
/// après l'avoir enrôlée par son nom donnera « aucune clé pour ce site » — ce
/// n'est pas une panne, c'est la protection contre l'hameçonnage qui fait
/// exactement son travail. C'est aussi pourquoi l'adresse publique du hub
/// mérite d'être fixée avant d'enrôler des clés.
pub fn context(host: &str, https: bool) -> Result<Webauthn, String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("hôte inconnu : impossible de déterminer le domaine WebAuthn".into());
    }
    let rp_id = rp_id_of(host);

    let scheme = if https { "https" } else { "http" };
    let origin = Url::parse(&format!("{scheme}://{host}"))
        .map_err(|e| format!("origine WebAuthn invalide ({scheme}://{host}) : {e}"))?;

    WebauthnBuilder::new(&rp_id, &origin)
        .map_err(|e| format!("contexte WebAuthn : {e}"))?
        .rp_name(RP_NAME)
        .build()
        .map_err(|e| format!("contexte WebAuthn : {e}"))
}

/// Le domaine seul, sans port.
///
/// `[::1]:8080` autant que `hub.exemple.fr:8443` : on coupe le port sans
/// casser une adresse IPv6 littérale, dont les deux-points ne sont pas des
/// séparateurs de port.
pub fn rp_id_of(host: &str) -> String {
    let host = host.trim();
    let without_port = match host.rsplit_once(':') {
        Some((before, after))
            if after.chars().all(|c| c.is_ascii_digit())
                && !after.is_empty()
                && !before.is_empty()
                && (before.ends_with(']') || !before.contains(':')) =>
        {
            before
        }
        _ => host,
    };
    without_port.trim_matches(|c| c == '[' || c == ']').to_string()
}

/// Le navigateur autorisera-t-il l'API sur cette origine ?
///
/// Reproduit la règle du contexte sécurisé, pour pouvoir le DIRE à l'écran
/// plutôt que de laisser l'utilisateur cliquer sur un bouton qui échouera
/// silencieusement.
pub fn secure_context(host: &str, https: bool) -> bool {
    if https {
        return true;
    }
    let bare = host.rsplit_once(':').map_or(host, |(h, _)| h);
    matches!(bare, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relying_party_is_the_host_without_its_port() {
        // ⚠️ Confondre `rp_id` et origine fait échouer toutes les cérémonies
        // avec un message que le navigateur ne détaille pas.
        assert_eq!(rp_id_of("hub.exemple.fr:8443"), "hub.exemple.fr");
        assert_eq!(rp_id_of("hub.exemple.fr"), "hub.exemple.fr");
        assert!(context("hub.exemple.fr:8443", true).is_ok());
    }

    #[test]
    fn an_ipv6_literal_keeps_its_address_and_loses_its_port() {
        assert_eq!(rp_id_of("[::1]:8080"), "::1");
        assert_eq!(rp_id_of("[fe80::1]"), "fe80::1");
    }

    #[test]
    fn an_empty_host_is_refused_rather_than_guessed() {
        assert!(context("   ", true).is_err());
    }

    #[test]
    fn plain_http_is_only_a_secure_context_on_the_loopback() {
        assert!(secure_context("hub.exemple.fr", true));
        assert!(secure_context("localhost:18080", false));
        assert!(secure_context("127.0.0.1:18080", false));
        assert!(
            !secure_context("10.0.30.12:18080", false),
            "c'est le cas courant d'un hub de LAN — l'écran doit pouvoir le dire"
        );
    }

    #[test]
    fn a_ceremony_is_consumed_by_its_first_use() {
        // Un défi qui survit à son usage peut être rejoué.
        let c = Ceremonies::new();
        assert!(c.take_registration("claire").is_none());
        assert!(c.take_authentication("claire").is_none());
    }
}
