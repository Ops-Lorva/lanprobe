//! Limitation de débit des routes publiques (audit RSSI du 16/11, F3 et F4).
//!
//! Le hub n'avait **aucun** frein : un dictionnaire sur `/api/login` ou un
//! code TOTP essayé million par million passaient sans ralentir, et chaque
//! `POST /api/pair` faux coûtait un argon2. Ce module compte, en mémoire, ce
//! qu'une adresse ou un compte a déjà raté, et fait répondre `429` au-delà
//! d'un budget.
//!
//! ⚠️ **On compte les ÉCHECS, pas les requêtes**, sauf pour les routes qui ne
//! savent pas ce qu'est un échec (le début d'une connexion par clé d'accès).
//! Un technicien qui se connecte vingt fois dans la journée ne doit jamais
//! voir un `429` ; celui qui se trompe dix fois de suite, si.
//!
//! 🔴 **Réservé avant, rendu après** (revue RSSI, N1) : chaque essai prend
//! son jeton AVANT la vérification ([`Limits::take`]) et le rend s'il réussit
//! ([`Limits::refund`]). Compter l'échec après argon2 laissait une rafale
//! simultanée passer tout entière avant le premier décompte.
//!
//! ⚠️ **En mémoire, et c'est assumé.** Un redémarrage remet les compteurs à
//! zéro : pour qu'il serve à un attaquant, il faudrait qu'il sache redémarrer
//! le hub. Une table SQLite aurait ajouté une écriture par échec — c'est-à-dire
//! le coût que la limite existe pour borner.
//!
//! ⚠️ **L'adresse est celle de `client_ip`**, qui ne lit `X-Forwarded-For` que
//! depuis un proxy de confiance. Sans `trusted_proxies` derrière un proxy,
//! tous les clients partagent l'adresse du proxy — et donc le même budget :
//! c'est le réglage à poser, pas une raison d'écouter l'en-tête de n'importe
//! qui.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Ce qu'on limite. Chaque classe a son budget : se tromper de mot de passe et
/// présenter un code d'appairage faux ne se paient pas pareil.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    /// Mot de passe, second facteur, clé d'accès, `Basic` — par adresse.
    AuthIp,
    /// Les mêmes, par compte visé : un dictionnaire réparti sur cent adresses
    /// bute quand même sur le compte.
    AuthAccount,
    /// `POST /api/login/passkey/start` : pas d'échec à compter, on compte les
    /// appels.
    PasskeyStart,
    /// `POST /api/pair` — code d'appairage faux.
    Pair,
    /// `POST /api/devices/token` — secret d'appareil refusé.
    DeviceToken,
    /// `POST /api/probes/enroll` — code ou identifiants refusés.
    Enroll,
}

impl Class {
    /// `(nombre, fenêtre)` : au-delà de `nombre` dans la fenêtre, `429`.
    ///
    /// ⚠️ Le budget par compte est plus serré dans le temps que celui par
    /// adresse : c'est lui qui borne le TOTP, six chiffres qu'aucune adresse
    /// isolée ne doit pouvoir parcourir.
    fn budget(self) -> (u32, Duration) {
        match self {
            Class::AuthIp => (10, Duration::from_secs(5 * 60)),
            Class::AuthAccount => (10, Duration::from_secs(15 * 60)),
            Class::PasskeyStart => (30, Duration::from_secs(5 * 60)),
            Class::Pair => (10, Duration::from_secs(5 * 60)),
            // Un téléphone renouvelle son jeton toutes les quinze minutes ;
            // un refus y est rare et se répète peu. Le budget est large pour
            // qu'une app en boucle sur un appareil révoqué ne prive pas un
            // bureau entier, derrière la même adresse, du renouvellement.
            Class::DeviceToken => (30, Duration::from_secs(5 * 60)),
            Class::Enroll => (10, Duration::from_secs(5 * 60)),
        }
    }
}

/// Au-delà de ce nombre d'entrées, les fenêtres échues sont balayées avant
/// d'en ajouter une. Un flot d'adresses toutes différentes ne doit pas faire
/// grossir la table sans fin.
const SWEEP_ABOVE: usize = 10_000;

/// Les compteurs. Un seul verrou : chaque opération est un accès de table, et
/// les routes concernées sont rares devant le reste du trafic.
pub struct Limits {
    windows: Mutex<HashMap<(Class, String), (Instant, u32)>>,
    /// Le portail argon2 : voir [`Limits::argon2_permit`].
    argon2: std::sync::Arc<tokio::sync::Semaphore>,
    /// Les échecs anonymes du journal d'audit, regroupés. Voir
    /// [`FailureDigest`].
    pub digest: FailureDigest,
}

/// Refus : combien de secondes attendre. Va dans `Retry-After`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryAfter(pub u64);

/// Argon2 simultanés, pour tout le hub. Quatre fois 19 Mio : borné, et
/// largement assez pour les connexions d'une équipe.
pub const ARGON2_CONCURRENCY: usize = 4;

impl Default for Limits {
    fn default() -> Self {
        Self {
            windows: Mutex::default(),
            argon2: std::sync::Arc::new(tokio::sync::Semaphore::new(ARGON2_CONCURRENCY)),
            digest: FailureDigest::default(),
        }
    }
}

impl Limits {
    pub fn new() -> Self {
        Self::default()
    }

    /// Refuse si l'une des clés a épuisé son budget. Ne compte rien.
    pub fn check(&self, keys: &[(Class, &str)]) -> Result<(), RetryAfter> {
        self.check_at(keys, Instant::now())
    }

    /// Compte un échec pour chacune des clés.
    pub fn fail(&self, keys: &[(Class, &str)]) {
        self.fail_at(keys, Instant::now());
    }

    /// Réserve un essai : refuse au-delà du budget, sinon le compte tout de
    /// suite — **avant** la vérification.
    ///
    /// 🔴 Contrôle et décompte sous le MÊME verrou (revue RSSI, N1). Avec
    /// `check` puis `fail` après argon2, K requêtes simultanées passaient
    /// toutes le contrôle avant qu'un seul échec soit compté : le budget de
    /// dix devenait K. Une route qui réussit rend son jeton par
    /// [`Limits::refund`] ; c'est ce qui garde « on compte les échecs ».
    ///
    /// Sert aussi, tel quel, aux routes qui n'ont pas d'échec à compter.
    pub fn take(&self, keys: &[(Class, &str)]) -> Result<(), RetryAfter> {
        let now = Instant::now();
        let Ok(mut windows) = self.windows.lock() else {
            return Err(RetryAfter(60));
        };
        Self::refusal(&windows, keys, now)?;
        Self::count(&mut windows, keys, now);
        Ok(())
    }

    /// Rend le jeton réservé par [`Limits::take`] : l'essai a réussi, ou il
    /// n'a rien essayé (portail argon2 plein). Jamais en dessous de zéro, et
    /// rien si la fenêtre a changé entre-temps.
    pub fn refund(&self, keys: &[(Class, &str)]) {
        let now = Instant::now();
        let Ok(mut windows) = self.windows.lock() else {
            return;
        };
        for (class, key) in keys {
            if let Some((start, count)) = windows.get_mut(&(*class, key.to_string()))
                && now.saturating_duration_since(*start) < class.budget().1
            {
                *count = count.saturating_sub(1);
            }
        }
    }

    /// Une place pour un calcul argon2, ou un refus immédiat.
    ///
    /// 🔴 Revue RSSI, N1 : `spawn_blocking` laissait jusqu'à 512 argon2
    /// tourner ensemble, 19 Mio chacun — de quoi mettre la VM du hub à genoux
    /// sans s'authentifier. Le plafond vaut pour tout le hub ; au-delà, on
    /// répond `429` plutôt que d'empiler. La place est rendue quand le
    /// calcul se termine, pas quand le client raccroche.
    pub fn argon2_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, RetryAfter> {
        self.argon2.clone().try_acquire_owned().map_err(|_| RetryAfter(1))
    }

    fn check_at(&self, keys: &[(Class, &str)], now: Instant) -> Result<(), RetryAfter> {
        let Ok(windows) = self.windows.lock() else {
            // 🔴 Fermé, pas ouvert : un verrou empoisonné ne doit pas lever le
            // seul frein des routes publiques.
            return Err(RetryAfter(60));
        };
        Self::refusal(&windows, keys, now)
    }

    fn refusal(
        windows: &HashMap<(Class, String), (Instant, u32)>,
        keys: &[(Class, &str)],
        now: Instant,
    ) -> Result<(), RetryAfter> {
        let mut wait = 0;
        for (class, key) in keys {
            let (max, period) = class.budget();
            if let Some((start, count)) = windows.get(&(*class, key.to_string())) {
                let elapsed = now.saturating_duration_since(*start);
                if elapsed < period && *count >= max {
                    wait = wait.max((period - elapsed).as_secs().max(1));
                }
            }
        }
        if wait > 0 { Err(RetryAfter(wait)) } else { Ok(()) }
    }

    fn fail_at(&self, keys: &[(Class, &str)], now: Instant) {
        let Ok(mut windows) = self.windows.lock() else {
            return;
        };
        Self::count(&mut windows, keys, now);
    }

    fn count(
        windows: &mut HashMap<(Class, String), (Instant, u32)>,
        keys: &[(Class, &str)],
        now: Instant,
    ) {
        if windows.len() > SWEEP_ABOVE {
            windows.retain(|(class, _), (start, _)| {
                now.saturating_duration_since(*start) < class.budget().1
            });
        }
        for (class, key) in keys {
            let period = class.budget().1;
            let entry = windows
                .entry((*class, key.to_string()))
                .or_insert((now, 0));
            if now.saturating_duration_since(entry.0) >= period {
                *entry = (now, 0);
            }
            entry.1 = entry.1.saturating_add(1);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.windows.lock().unwrap().len()
    }
}

// ── Échecs anonymes regroupés au journal d'audit ──────────────────────────

/// Durée d'une fenêtre de regroupement, en secondes.
pub const DIGEST_WINDOW_SECS: i64 = 60;

/// Regroupe les échecs **anonymes** du journal d'audit (F4).
///
/// 🔴 Le journal est en ajout seul et sans purge, et chaque `POST /api/pair`
/// faux, chaque connexion sur un compte inexistant y écrivait une ligne : un
/// flot anonyme remplissait le disque sans même avoir besoin de passer la
/// limite de débit, puisque chaque adresse neuve repart avec un budget.
///
/// La première ligne d'une fenêtre s'écrit telle quelle — c'est elle qui dit
/// ce qui se passe. Les suivantes, pendant [`DIGEST_WINDOW_SECS`], ne font que
/// compter ; une ligne de synthèse dit combien il y en a eu quand la fenêtre se
/// ferme. Rien n'est effacé : on écrit moins, on ne retire rien.
///
/// ⚠️ **Seuls les échecs SANS acteur passent par ici.** Un échec qui nomme un
/// compte — mot de passe faux sur `benjamin` — reste une ligne à part entière :
/// c'est celle qu'on cherche après coup.
#[derive(Default)]
pub struct FailureDigest {
    /// Par action : `(début de la fenêtre, échecs absorbés)`. Clé = action
    /// seule, jamais la cible : la cible vient de l'attaquant (un `device_id`
    /// inventé), et en faire une clé ferait grossir la table à sa demande.
    windows: Mutex<HashMap<String, (i64, u32)>>,
}

/// Une synthèse à écrire : `n` échecs absorbés depuis `since`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    pub action: String,
    pub absorbed: u32,
    pub since: i64,
}

impl Digest {
    pub fn detail(&self) -> String {
        format!(
            "{} échec(s) anonyme(s) de plus, regroupés — fenêtre ouverte à {} (epoch)",
            self.absorbed, self.since
        )
    }
}

impl FailureDigest {
    /// Admet un échec anonyme. Rend la synthèse de la fenêtre précédente s'il
    /// y en a une à écrire, et `true` si la ligne elle-même doit s'écrire.
    pub fn admit(&self, action: &str, now: i64) -> (Option<Digest>, bool) {
        let Ok(mut windows) = self.windows.lock() else {
            // Verrou empoisonné : on écrit. Perdre une ligne serait pire
            // qu'en écrire trop.
            return (None, true);
        };
        let closed = match windows.get_mut(action) {
            Some((since, absorbed)) if now - *since < DIGEST_WINDOW_SECS => {
                *absorbed = absorbed.saturating_add(1);
                return (None, false);
            }
            Some((since, absorbed)) => (*absorbed > 0).then(|| Digest {
                action: action.to_string(),
                absorbed: *absorbed,
                since: *since,
            }),
            None => None,
        };
        windows.insert(action.to_string(), (now, 0));
        (closed, true)
    }

    /// Les synthèses des fenêtres échues — à écrire par une tâche de fond,
    /// pour qu'une rafale qui s'arrête net ne reste pas sans sa ligne.
    pub fn due(&self, now: i64) -> Vec<Digest> {
        let Ok(mut windows) = self.windows.lock() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        windows.retain(|action, (since, absorbed)| {
            if now - *since < DIGEST_WINDOW_SECS {
                return true;
            }
            if *absorbed > 0 {
                out.push(Digest {
                    action: action.clone(),
                    absorbed: *absorbed,
                    since: *since,
                });
            }
            false
        });
        out
    }
}

/// La clé d'une adresse.
///
/// ⚠️ **Une IPv6 compte pour son /64**, pas pour elle-même : un abonné reçoit
/// couramment un /64 entier, et une adresse par essai lui coûterait zéro.
pub fn ip_key(ip: IpAddr) -> String {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        },
        v4 => v4.to_string(),
    }
}

/// La clé d'un compte : le nom tel qu'on le tape, sans casse ni blancs. Un
/// dictionnaire ne contourne pas le budget en alternant `Admin` et `admin`.
pub fn account_key(username: &str) -> String {
    username.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_budget_se_consomme_par_echec_puis_refuse() {
        let limits = Limits::new();
        let keys = [(Class::Pair, "203.0.113.7")];
        for _ in 0..10 {
            assert!(limits.check(&keys).is_ok());
            limits.fail(&keys);
        }
        let refus = limits.check(&keys).unwrap_err();
        assert!(refus.0 >= 1 && refus.0 <= 300, "{refus:?}");
        // Une autre adresse n'est pas concernée.
        assert!(limits.check(&[(Class::Pair, "203.0.113.8")]).is_ok());
    }

    #[test]
    fn une_cle_epuisee_suffit_a_refuser() {
        let limits = Limits::new();
        for _ in 0..10 {
            limits.fail(&[(Class::AuthAccount, "admin")]);
        }
        // Nouvelle adresse, même compte : refusé quand même.
        assert!(limits
            .check(&[(Class::AuthIp, "198.51.100.1"), (Class::AuthAccount, "admin")])
            .is_err());
    }

    #[test]
    fn la_fenetre_echue_rend_le_budget() {
        let limits = Limits::new();
        let keys = [(Class::Enroll, "203.0.113.7")];
        let t0 = Instant::now();
        for _ in 0..10 {
            limits.fail_at(&keys, t0);
        }
        assert!(limits.check_at(&keys, t0).is_err());
        let after = t0 + Duration::from_secs(5 * 60 + 1);
        assert!(limits.check_at(&keys, after).is_ok());
        limits.fail_at(&keys, after);
        assert!(limits.check_at(&keys, after).is_ok(), "le compteur repart de un");
    }

    #[test]
    fn take_compte_les_appels() {
        let limits = Limits::new();
        let keys = [(Class::PasskeyStart, "203.0.113.7")];
        for _ in 0..30 {
            assert!(limits.take(&keys).is_ok());
        }
        assert!(limits.take(&keys).is_err());
    }

    #[test]
    fn take_reserve_sous_le_meme_verrou_que_le_controle() {
        // 🔴 N1 : `check` puis `fail` laissaient passer K requêtes
        // simultanées avant qu'un seul échec soit compté.
        let limits = std::sync::Arc::new(Limits::new());
        let accordes = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let fils: Vec<_> = (0..8)
            .map(|_| {
                let (limits, accordes) = (limits.clone(), accordes.clone());
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        if limits.take(&[(Class::AuthIp, "203.0.113.7")]).is_ok() {
                            accordes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                })
            })
            .collect();
        for fil in fils {
            fil.join().unwrap();
        }
        assert_eq!(accordes.load(std::sync::atomic::Ordering::SeqCst), 10);
    }

    #[test]
    fn take_ne_compte_rien_quand_il_refuse() {
        let limits = Limits::new();
        for _ in 0..10 {
            limits.fail(&[(Class::AuthAccount, "admin")]);
        }
        let keys = [(Class::AuthIp, "203.0.113.7"), (Class::AuthAccount, "admin")];
        assert!(limits.take(&keys).is_err());
        // L'adresse n'a rien perdu au refus du compte.
        for _ in 0..10 {
            assert!(limits.take(&[(Class::AuthIp, "203.0.113.7")]).is_ok());
        }
    }

    #[test]
    fn refund_rend_le_jeton_d_un_succes() {
        let limits = Limits::new();
        let keys = [(Class::AuthIp, "203.0.113.7")];
        for _ in 0..10 {
            limits.take(&keys).unwrap();
        }
        assert!(limits.take(&keys).is_err());
        limits.refund(&keys);
        assert!(limits.take(&keys).is_ok());
        assert!(limits.take(&keys).is_err());
        // Rendre plus qu'on n'a pris ne crée pas de crédit.
        let autre = [(Class::Pair, "198.51.100.1")];
        limits.refund(&autre);
        limits.refund(&autre);
        for _ in 0..10 {
            limits.take(&autre).unwrap();
        }
        assert!(limits.take(&autre).is_err());
    }

    #[test]
    fn le_portail_argon2_plafonne_les_calculs_simultanes() {
        let limits = Limits::new();
        let tenus: Vec<_> = (0..ARGON2_CONCURRENCY)
            .map(|_| limits.argon2_permit().expect("sous le plafond"))
            .collect();
        assert!(limits.argon2_permit().is_err(), "au-delà du plafond : refus, pas d'attente");
        drop(tenus);
        assert!(limits.argon2_permit().is_ok());
    }

    #[test]
    fn la_table_ne_grossit_pas_sans_fin() {
        let limits = Limits::new();
        let t0 = Instant::now();
        for i in 0..(SWEEP_ABOVE + 5) {
            limits.fail_at(&[(Class::Pair, &i.to_string())], t0);
        }
        let later = t0 + Duration::from_secs(10 * 60);
        limits.fail_at(&[(Class::Pair, "tard")], later);
        assert_eq!(limits.len(), 1, "les fenêtres échues sont balayées");
    }

    #[test]
    fn les_echecs_anonymes_d_une_rafale_tiennent_en_deux_lignes() {
        let digest = FailureDigest::default();
        // La première s'écrit.
        assert_eq!(digest.admit("device.refused", 1000), (None, true));
        // Les 49 suivantes, dans la minute, ne font que compter.
        for i in 0..49 {
            assert_eq!(digest.admit("device.refused", 1001 + i % 50), (None, false));
        }
        // Une autre action a sa propre fenêtre.
        assert_eq!(digest.admit("auth.login", 1002), (None, true));
        // Rien d'échu avant la fin de la fenêtre…
        assert!(digest.due(1059).is_empty());
        // … puis la synthèse, une fois.
        let due = digest.due(1060);
        assert_eq!(
            due,
            vec![Digest { action: "device.refused".into(), absorbed: 49, since: 1000 }]
        );
        assert!(digest.due(1200).is_empty(), "rien deux fois");
    }

    #[test]
    fn la_fenetre_suivante_rend_la_synthese_de_la_precedente() {
        let digest = FailureDigest::default();
        digest.admit("auth.login", 1000);
        digest.admit("auth.login", 1010);
        digest.admit("auth.login", 1020);
        let (synthese, ecrire) = digest.admit("auth.login", 1100);
        assert!(ecrire);
        assert_eq!(
            synthese,
            Some(Digest { action: "auth.login".into(), absorbed: 2, since: 1000 })
        );
    }

    #[test]
    fn une_ipv6_compte_pour_son_slash_64() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        assert_eq!(ip_key(a), ip_key(b));
        let mapped: IpAddr = "::ffff:203.0.113.7".parse().unwrap();
        assert_eq!(ip_key(mapped), "203.0.113.7");
    }

    #[test]
    fn la_casse_du_compte_ne_contourne_rien() {
        assert_eq!(account_key(" Admin "), account_key("admin"));
    }
}
