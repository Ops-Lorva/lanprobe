//! Réglages du hub, rangés en base et modifiables depuis l'interface.
//!
//! Une valeur qui vit dans un `.env` demande d'éditer un fichier, de
//! redémarrer le conteneur, et se perd à la première migration de machine. On
//! a SQLite : les réglages y vivent, et sont sauvegardés avec le reste.
//!
//! Les variables d'environnement restent acceptées comme **valeurs initiales**
//! au premier démarrage, pour qui préfère préconfigurer. Dès qu'un réglage est
//! écrit en base, la base gagne — sinon un `.env` oublié écraserait
//! silencieusement ce que l'opérateur vient de régler dans l'interface.
//!
//! **Aucun secret ici** : le jeton opérateur Influx vit dans un fichier du
//! volume, pas en base, et n'est exposé par aucune route.

use std::sync::Arc;

use crate::db::{Db, DbError, DbResult};

pub mod keys {
    pub const INFLUX_URL: &str = "influx_url";
    pub const INFLUX_ORG: &str = "influx_org";
    pub const INFLUX_BUCKET: &str = "influx_bucket";
    pub const INFLUX_ADVERTISE_URL: &str = "influx_advertise_url";
    /// Adresse publique du hub, telle que les sondes le joignent.
    pub const HUB_PUBLIC_URL: &str = "hub_public_url";
    pub const RETENTION_DAYS: &str = "retention_days";
    pub const HEARTBEAT_INTERVAL_SECS: &str = "heartbeat_interval_secs";
    /// Silence toléré avant qu'une sonde soit déclarée hors ligne. Ce n'est
    /// pas un secret : il n'y a que ce réglage-là dans les notifications qui
    /// ait le droit d'être en clair.
    pub const NOTIFY_DELAY_SECS: &str = "notify_delay_secs";
    /// Types d'alerte retenus, séparés par des virgules.
    ///
    /// ⚠️ Une liste vide est une valeur valide : « je ne veux rien recevoir ».
    /// La confondre avec « non réglé », donc avec le défaut, rendrait
    /// impossible de tout couper sans démonter les canaux.
    pub const NOTIFY_EVENTS: &str = "notify_events";

    /// La sauvegarde tourne toute seule. Une sauvegarde qu'il faut
    /// déclencher est une sauvegarde qu'on oublie.
    pub const BACKUP_ENABLED: &str = "backup_enabled";
    pub const BACKUP_INTERVAL_HOURS: &str = "backup_interval_hours";
    /// ⚠️ Baisser cette valeur **supprime des archives**.
    pub const BACKUP_KEEP_LAST: &str = "backup_keep_last";

    /// ⚠️ Baisser cette valeur **supprime des scans**. Même garde-fou que la
    /// rétention Influx : rien ne s'efface sans accord explicite.
    ///
    /// L'inventaire s'accumule sinon sans limite : un scan d'un /24 pose une
    /// ligne par machine et une par port ouvert, à chaque passage.
    pub const INVENTORY_DAYS: &str = "inventory_days";

    /// Le hub termine lui-même le TLS, avec son certificat auto-signé.
    ///
    /// ⚠️ Ne prend effet **qu'au redémarrage** : basculer à chaud couperait
    /// la page depuis laquelle on vient de cliquer, et laisserait les sondes
    /// sans hub sans que personne ait vu le message.
    pub const TLS_ENABLED: &str = "tls_enabled";

    /// Reverse proxies dont on accepte le `X-Forwarded-For`, en CIDR séparés
    /// par des virgules.
    ///
    /// ⚠️ **Vide par défaut, et c'est le comportement sûr.** L'en-tête est
    /// alors ignoré : on lit l'adresse de la socket. Le lire « s'il est
    /// présent » laisserait n'importe quelle sonde forger son adresse source.
    pub const TRUSTED_PROXIES: &str = "trusted_proxies";

    pub const ALL: &[&str] = &[
        INFLUX_URL,
        INFLUX_ORG,
        INFLUX_BUCKET,
        INFLUX_ADVERTISE_URL,
        HUB_PUBLIC_URL,
        RETENTION_DAYS,
        HEARTBEAT_INTERVAL_SECS,
        NOTIFY_DELAY_SECS,
        NOTIFY_EVENTS,
        BACKUP_ENABLED,
        BACKUP_INTERVAL_HOURS,
        BACKUP_KEEP_LAST,
        INVENTORY_DAYS,
        TLS_ENABLED,
        TRUSTED_PROXIES,
    ];
}

pub const DEFAULT_INFLUX_URL: &str = "https://127.0.0.1:8086";
pub const DEFAULT_INFLUX_ORG: &str = "lanprobe";
pub const DEFAULT_INFLUX_BUCKET: &str = "lanprobe";
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: i64 = 60;
/// ⚠️ **Illimitée par défaut, et ça n'est pas un oubli.**
///
/// Un défaut à 90 jours se serait appliqué aux hubs déjà installés : la
/// première purge aurait effacé des scans que personne n'a accepté de perdre,
/// à la faveur d'une mise à jour. Sur ce projet rien ne s'efface sans accord
/// explicite — la rétention de l'inventaire s'active donc à la main, et
/// l'interface explique pourquoi on voudrait le faire.
pub const DEFAULT_INVENTORY_DAYS: i64 = 0;

/// Une rétention passe-t-elle de `current` à `new` en effaçant quelque chose ?
/// `0` vaut « illimitée » : y aller n'efface rien, en partir efface tout ce
/// qui dépasse.
fn shortens(current: i64, new: i64) -> bool {
    match (current, new) {
        (_, 0) => false,
        (0, _) => true,
        (c, n) => n < c,
    }
}

#[derive(Clone)]
pub struct Settings {
    db: Arc<Db>,
}

impl Settings {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    pub fn influx_url(&self) -> String {
        self.get_or(keys::INFLUX_URL, DEFAULT_INFLUX_URL)
    }

    pub fn influx_org(&self) -> String {
        self.get_or(keys::INFLUX_ORG, DEFAULT_INFLUX_ORG)
    }

    pub fn influx_bucket(&self) -> String {
        self.get_or(keys::INFLUX_BUCKET, DEFAULT_INFLUX_BUCKET)
    }

    /// URL annoncée telle qu'elle est réglée dans l'interface. `None` laisse
    /// la main aux niveaux suivants (variable d'environnement, en-tête `Host`).
    /// Adresse publique du hub. C'est **le seul réglage d'adresse à renseigner
    /// normalement** : l'URL d'Influx en découle. L'utilisateur ne devrait pas
    /// avoir à distinguer deux URL voisines — il dit par où on le joint, le hub
    /// calcule le reste.
    pub fn hub_public_url(&self) -> Option<String> {
        self.db
            .get_setting(keys::HUB_PUBLIC_URL)
            .ok()
            .flatten()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
    }

    pub fn stored_advertise_url(&self) -> Option<String> {
        self.db.get_setting(keys::INFLUX_ADVERTISE_URL).ok().flatten()
    }

    /// Rétention du bucket en jours. `0` = illimitée.
    pub fn retention_days(&self) -> i64 {
        self.get_or(keys::RETENTION_DAYS, "0").parse().unwrap_or(0)
    }

    /// Délai avant alerte. Cinq minutes par défaut : une sonde qui redémarre
    /// ne réveille personne.
    pub fn notify_delay_secs(&self) -> i64 {
        self.get_or(
            keys::NOTIFY_DELAY_SECS,
            &crate::notify::DEFAULT_DELAY_SECS.to_string(),
        )
        .parse()
        .unwrap_or(crate::notify::DEFAULT_DELAY_SECS)
    }

    /// Vrai tant que personne ne l'a explicitement coupée. Le défaut est
    /// « activée » : c'est tout l'intérêt du modèle.
    /// Types d'alerte retenus. Défaut : ce qui existait avant, à savoir la
    /// sonde hors ligne et son retour.
    pub fn notify_events(&self) -> Vec<String> {
        match self.db.get_setting(keys::NOTIFY_EVENTS) {
            Ok(Some(raw)) => raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            _ => vec!["probe.down".into(), "probe.up".into()],
        }
    }

    pub fn backup_enabled(&self) -> bool {
        self.get_or(keys::BACKUP_ENABLED, "true") != "false"
    }

    pub fn backup_interval_hours(&self) -> i64 {
        self.get_or(
            keys::BACKUP_INTERVAL_HOURS,
            &crate::backup::DEFAULT_INTERVAL_HOURS.to_string(),
        )
        .parse()
        .unwrap_or(crate::backup::DEFAULT_INTERVAL_HOURS)
    }

    /// Nombre d'archives conservées. `0` = toutes.
    pub fn backup_keep_last(&self) -> i64 {
        self.get_or(
            keys::BACKUP_KEEP_LAST,
            &crate::backup::DEFAULT_KEEP_LAST.to_string(),
        )
        .parse()
        .unwrap_or(crate::backup::DEFAULT_KEEP_LAST)
    }

    /// Rétention de l'inventaire réseau, en jours. `0` = illimitée.
    pub fn inventory_days(&self) -> i64 {
        self.get_or(keys::INVENTORY_DAYS, &DEFAULT_INVENTORY_DAYS.to_string())
            .parse()
            .unwrap_or(DEFAULT_INVENTORY_DAYS)
    }

    /// Le hub doit-il servir en HTTPS lui-même.
    ///
    /// ⚠️ Lu **au démarrage seulement**. `--tls` (ou `LANPROBE_WEB_TLS`)
    /// force l'activation quoi qu'en dise la base : une option passée
    /// explicitement à la ligne de commande ne doit pas pouvoir être
    /// désactivée depuis l'interface — sinon un opérateur qui n'a pas de
    /// proxy peut se retrouver à servir en clair sans l'avoir voulu.
    pub fn tls_enabled(&self) -> bool {
        self.get_or(keys::TLS_ENABLED, "false") == "true"
    }

    /// Proxies de confiance, en CIDR séparés par des virgules. Vide par
    /// défaut : `X-Forwarded-For` est alors ignoré, ce qui est le comportement
    /// sûr pour un hub exposé directement.
    pub fn trusted_proxies(&self) -> Vec<crate::client_ip::IpNet> {
        crate::client_ip::parse_cidrs(&self.get_or(keys::TRUSTED_PROXIES, ""))
    }

    pub fn heartbeat_interval_secs(&self) -> i64 {
        self.get_or(
            keys::HEARTBEAT_INTERVAL_SECS,
            &DEFAULT_HEARTBEAT_INTERVAL_SECS.to_string(),
        )
        .parse()
        .unwrap_or(DEFAULT_HEARTBEAT_INTERVAL_SECS)
    }

    /// Tous les réglages, valeurs effectives (défaut compris). Aucun secret
    /// n'y figure : la table `settings` n'en contient aucun, par construction.
    ///
    /// **Les types sont respectés** : les entiers sortent en nombres JSON, et
    /// une URL annoncée non réglée sort en `null`, pas en chaîne vide. La base
    /// stocke tout en texte, mais le laisser transparaître obligeait
    /// l'interface à comparer `60` à `"60"` — jamais égaux, donc un formulaire
    /// perpétuellement « modifié » dès l'ouverture, et un bouton
    /// « Enregistrer » actif sans que personne n'ait rien touché.
    pub fn all(&self) -> serde_json::Value {
        serde_json::json!({
            keys::INFLUX_URL: self.influx_url(),
            keys::INFLUX_ORG: self.influx_org(),
            keys::INFLUX_BUCKET: self.influx_bucket(),
            keys::INFLUX_ADVERTISE_URL: self.stored_advertise_url(),
            keys::HUB_PUBLIC_URL: self.hub_public_url(),
            keys::RETENTION_DAYS: self.retention_days(),
            keys::HEARTBEAT_INTERVAL_SECS: self.heartbeat_interval_secs(),
            keys::NOTIFY_DELAY_SECS: self.notify_delay_secs(),
            keys::NOTIFY_EVENTS: self.notify_events(),
            keys::BACKUP_ENABLED: self.backup_enabled(),
            keys::BACKUP_INTERVAL_HOURS: self.backup_interval_hours(),
            keys::BACKUP_KEEP_LAST: self.backup_keep_last(),
            keys::INVENTORY_DAYS: self.inventory_days(),
            keys::TLS_ENABLED: self.tls_enabled(),
        })
    }

    /// Écrit un réglage après validation. `confirm_data_loss` porte sur les
    /// deux réglages dont la baisse **efface** quelque chose :
    /// `retention_days` (des mesures) et `backup_keep_last` (des archives).
    /// Sur ce projet rien ne s'efface sans accord explicite, et le garde-fou
    /// est ici — côté serveur — pas dans l'interface.
    pub fn put(&self, key: &str, value: &str, confirm_data_loss: bool) -> DbResult<()> {
        let value = value.trim();
        match key {
            keys::INFLUX_URL | keys::INFLUX_ADVERTISE_URL | keys::HUB_PUBLIC_URL => {
                validate_absolute_url(value)?
            }
            keys::INFLUX_ORG | keys::INFLUX_BUCKET => {
                if value.is_empty() {
                    return Err(DbError::Conflict(format!("{key} ne peut pas être vide")));
                }
            }
            keys::NOTIFY_EVENTS => {
                // Chaque entrée doit être un type connu : une faute de frappe
                // qui passe donnerait un réglage qui n'alerte sur rien, sans
                // que rien ne le dise.
                for kind in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    if crate::notify::AlertKind::parse(kind).is_none() {
                        return Err(DbError::Conflict(format!("type d'alerte inconnu : {kind}")));
                    }
                }
            }
            keys::BACKUP_ENABLED | keys::TLS_ENABLED => {
                if !matches!(value, "true" | "false") {
                    return Err(DbError::Conflict(format!(
                        "{key} attend true ou false"
                    )));
                }
            }
            keys::BACKUP_KEEP_LAST => {
                let parsed: i64 = value
                    .parse()
                    .map_err(|_| DbError::Conflict(format!("{key} doit être un entier")))?;
                if parsed < 0 {
                    return Err(DbError::Conflict(format!("{key} ne peut pas être négatif")));
                }
                if self.kept_archives_shrink_to(parsed) && !confirm_data_loss {
                    return Err(DbError::Conflict(format!(
                        "ne garder que {parsed} archive(s) supprime les plus anciennes — \
                         renvoyer la requête avec confirm_data_loss: true"
                    )));
                }
            }
            keys::HEARTBEAT_INTERVAL_SECS
            | keys::NOTIFY_DELAY_SECS
            | keys::BACKUP_INTERVAL_HOURS => {
                let parsed: i64 = value
                    .parse()
                    .map_err(|_| DbError::Conflict(format!("{key} doit être un entier")))?;
                if parsed <= 0 {
                    return Err(DbError::Conflict(format!("{key} doit être supérieur à 0")));
                }
            }
            keys::RETENTION_DAYS => {
                let parsed: i64 = value
                    .parse()
                    .map_err(|_| DbError::Conflict(format!("{key} doit être un entier")))?;
                if parsed < 0 {
                    return Err(DbError::Conflict(format!("{key} ne peut pas être négatif")));
                }
                if self.retention_shortens_to(parsed) && !confirm_data_loss {
                    return Err(DbError::Conflict(format!(
                        "raccourcir la rétention efface les mesures plus anciennes que \
                         {parsed} jour(s) — renvoyer la requête avec confirm_data_loss: true"
                    )));
                }
            }
            keys::INVENTORY_DAYS => {
                let parsed: i64 = value
                    .parse()
                    .map_err(|_| DbError::Conflict(format!("{key} doit être un entier")))?;
                if parsed < 0 {
                    return Err(DbError::Conflict(format!("{key} ne peut pas être négatif")));
                }
                // Même garde que la rétention Influx, pour la même raison :
                // raccourcir efface, et rien ne s'efface ici sans accord.
                if shortens(self.inventory_days(), parsed) && !confirm_data_loss {
                    return Err(DbError::Conflict(format!(
                        "raccourcir la rétention de l'inventaire supprime les scans plus \
                         anciens que {parsed} jour(s) — renvoyer la requête avec \
                         confirm_data_loss: true"
                    )));
                }
            }
            other => return Err(DbError::Conflict(format!("réglage inconnu : {other}"))),
        }
        self.db.set_setting(key, value)
    }

    /// Vrai si passer à `new_keep` réduit le nombre d'archives conservées.
    /// `0` vaut « toutes » : y aller n'efface rien, en partir efface.
    fn kept_archives_shrink_to(&self, new_keep: i64) -> bool {
        let current = self.backup_keep_last();
        match (current, new_keep) {
            (_, 0) => false,
            (0, _) => true,
            (c, n) => n < c,
        }
    }

    /// Vrai si passer à `new_days` raccourcit la fenêtre conservée. `0` vaut
    /// l'infini : quitter l'illimité est la réduction la plus destructrice,
    /// pas une augmentation.
    fn retention_shortens_to(&self, new_days: i64) -> bool {
        shortens(self.retention_days(), new_days)
    }

    /// Amorce les réglages absents depuis l'environnement. Appelé une fois au
    /// démarrage ; les clés déjà réglées ne bougent pas.
    ///
    /// `INFLUX_ADVERTISE_URL` n'est **pas** amorcée ici : elle reste un niveau
    /// de repli distinct pour que le diagnostic puisse dire si la valeur vient
    /// de l'interface ou de l'environnement.
    pub fn seed_from_env(&self) -> DbResult<()> {
        for (var, key) in [
            ("INFLUX_URL", keys::INFLUX_URL),
            ("INFLUX_ORG", keys::INFLUX_ORG),
            ("INFLUX_BUCKET", keys::INFLUX_BUCKET),
        ] {
            if let Ok(value) = std::env::var(var)
                && !value.trim().is_empty()
            {
                self.db.seed_setting(key, value.trim())?;
            }
        }
        Ok(())
    }

    fn get_or(&self, key: &str, default: &str) -> String {
        self.db
            .get_setting(key)
            .ok()
            .flatten()
            .unwrap_or_else(|| default.to_string())
    }
}

/// Validation volontairement stricte : schéma **et** hôte. Une URL relative
/// ou tronquée donnerait une sonde qui s'enrôle et n'écrit jamais rien.
fn validate_absolute_url(value: &str) -> DbResult<()> {
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err(DbError::Conflict(
            "URL absolue attendue, avec son schéma (https://…)".into(),
        ));
    };
    if !matches!(scheme, "http" | "https") {
        return Err(DbError::Conflict("schéma attendu : http ou https".into()));
    }
    let host = rest.split(['/', '?']).next().unwrap_or("");
    if host.trim().is_empty() {
        return Err(DbError::Conflict("l'URL doit comporter un hôte".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Les variables d'environnement sont globales au processus alors que les
    /// tests Rust tournent en parallèle : tout test qui en manipule une prend
    /// ce verrou.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn settings() -> Settings {
        Settings::new(Arc::new(crate::db::Db::open_in_memory().unwrap()))
    }

    #[test]
    fn defaults_apply_when_nothing_is_configured() {
        let s = settings();
        assert_eq!(s.influx_url(), "https://127.0.0.1:8086");
        assert_eq!(s.influx_org(), "lanprobe");
        assert_eq!(s.influx_bucket(), "lanprobe");
        assert_eq!(s.retention_days(), 0, "0 = rétention illimitée");
        assert_eq!(s.heartbeat_interval_secs(), 60);
        assert!(s.stored_advertise_url().is_none());
    }

    #[test]
    fn the_database_wins_over_the_environment() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let s = settings();
        // SAFETY : sérialisé par ENV_LOCK, restauré avant de rendre la main.
        unsafe { std::env::set_var("INFLUX_ORG", "depuis-env") };
        s.seed_from_env().unwrap();
        s.put(keys::INFLUX_ORG, "depuis-la-base", false).unwrap();
        // Un second amorçage — c'est ce qui se passe à chaque redémarrage —
        // ne doit pas écraser ce que l'opérateur a réglé dans l'interface.
        s.seed_from_env().unwrap();
        unsafe { std::env::remove_var("INFLUX_ORG") };

        assert_eq!(s.influx_org(), "depuis-la-base");
    }

    #[test]
    fn the_environment_only_seeds_the_first_boot() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let s = settings();
        unsafe { std::env::set_var("INFLUX_BUCKET", "premier") };
        s.seed_from_env().unwrap();
        unsafe { std::env::set_var("INFLUX_BUCKET", "second") };
        s.seed_from_env().unwrap();
        unsafe { std::env::remove_var("INFLUX_BUCKET") };

        assert_eq!(
            s.influx_bucket(),
            "premier",
            "modifier la variable après le premier démarrage ne doit rien changer"
        );
    }

    #[test]
    fn listing_settings_exposes_no_secret() {
        let s = settings();
        s.put(keys::INFLUX_URL, "https://influx.example:8086", false).unwrap();

        let all = s.all();
        let rendered = serde_json::to_string(&all).unwrap();
        for forbidden in ["token", "password", "secret"] {
            assert!(
                !rendered.to_lowercase().contains(forbidden),
                "aucun secret ne doit figurer dans GET /api/settings : {rendered}"
            );
        }
        assert!(rendered.contains("influx_url"));
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let s = settings();
        let err = s.put("influx_master_key", "peu-importe", false).unwrap_err();
        assert!(matches!(err, crate::db::DbError::Conflict(_)), "{err:?}");
    }

    #[test]
    fn a_url_setting_must_be_absolute() {
        let s = settings();
        for bad in ["influx.example:8086", "/relatif", "https://", ""] {
            assert!(
                s.put(keys::INFLUX_ADVERTISE_URL, bad, false).is_err(),
                "« {bad} » doit être refusé"
            );
        }
        assert!(s.put(keys::INFLUX_ADVERTISE_URL, "https://a.example:9086", false).is_ok());
    }

    #[test]
    fn the_alert_delay_defaults_to_five_minutes_and_is_adjustable() {
        let s = settings();
        assert_eq!(s.notify_delay_secs(), 300);

        assert!(s.put(keys::NOTIFY_DELAY_SECS, "0", false).is_err());
        assert!(s.put(keys::NOTIFY_DELAY_SECS, "beaucoup", false).is_err());
        s.put(keys::NOTIFY_DELAY_SECS, "600", false).unwrap();
        assert_eq!(s.notify_delay_secs(), 600);
    }

    #[test]
    fn a_numeric_setting_must_be_a_positive_number() {
        let s = settings();
        assert!(s.put(keys::HEARTBEAT_INTERVAL_SECS, "soixante", false).is_err());
        assert!(s.put(keys::HEARTBEAT_INTERVAL_SECS, "0", false).is_err());
        assert!(s.put(keys::HEARTBEAT_INTERVAL_SECS, "-1", false).is_err());
        assert!(s.put(keys::HEARTBEAT_INTERVAL_SECS, "30", false).is_ok());
        assert_eq!(s.heartbeat_interval_secs(), 30);
    }

    #[test]
    fn shortening_the_retention_is_refused_without_explicit_confirmation() {
        let s = settings();
        s.put(keys::RETENTION_DAYS, "90", true).unwrap();

        let err = s.put(keys::RETENTION_DAYS, "30", false).unwrap_err();
        assert!(
            err.to_string().contains("confirm"),
            "le refus doit nommer la confirmation attendue : {err}"
        );
        assert_eq!(s.retention_days(), 90, "rien ne doit avoir changé");

        s.put(keys::RETENTION_DAYS, "30", true).unwrap();
        assert_eq!(s.retention_days(), 30);
    }

    #[test]
    fn inventory_retention_defaults_to_unlimited_so_an_update_erases_nothing() {
        // 🔴 Le point du test : un défaut à 90 jours se serait appliqué aux
        // hubs déjà installés, et la première purge aurait effacé des scans
        // que personne n'a accepté de perdre — à la faveur d'une mise à jour.
        let s = settings();
        assert_eq!(s.inventory_days(), 0);
    }

    #[test]
    fn shortening_the_inventory_retention_is_refused_without_confirmation() {
        let s = settings();
        // Partir d'illimité EST une réduction : tout ce qui dépasse s'efface.
        assert!(s.put(keys::INVENTORY_DAYS, "90", false).is_err());
        s.put(keys::INVENTORY_DAYS, "90", true).unwrap();

        let err = s.put(keys::INVENTORY_DAYS, "30", false).unwrap_err();
        assert!(
            err.to_string().contains("confirm"),
            "le refus doit nommer la confirmation attendue : {err}"
        );
        assert_eq!(s.inventory_days(), 90, "rien ne doit avoir changé");

        s.put(keys::INVENTORY_DAYS, "30", true).unwrap();
        assert_eq!(s.inventory_days(), 30);

        // Rallonger ne détruit rien : aucune confirmation exigée.
        s.put(keys::INVENTORY_DAYS, "365", false).unwrap();
        assert_eq!(s.inventory_days(), 365);
    }

    #[test]
    fn the_tls_switch_only_accepts_a_boolean() {
        let s = settings();
        assert!(!s.tls_enabled(), "le hub sert en clair par défaut");
        assert!(s.put(keys::TLS_ENABLED, "oui", false).is_err());
        s.put(keys::TLS_ENABLED, "true", false).unwrap();
        assert!(s.tls_enabled());
    }

    #[test]
    fn leaving_unlimited_retention_is_a_reduction_too() {
        // 0 = illimité : passer à 90 jours efface tout ce qui est plus vieux.
        // C'est la réduction la plus destructrice, pas une augmentation.
        let s = settings();
        assert_eq!(s.retention_days(), 0);

        assert!(s.put(keys::RETENTION_DAYS, "90", false).is_err());
        assert!(s.put(keys::RETENTION_DAYS, "90", true).is_ok());
    }

    #[test]
    fn the_backup_settings_have_sensible_defaults() {
        let s = settings();
        assert!(s.backup_enabled(), "une sauvegarde qu'il faut déclencher est oubliée");
        assert_eq!(s.backup_interval_hours(), 24);
        assert_eq!(s.backup_keep_last(), 7, "sept jours : de quoi passer un week-end");
    }

    #[test]
    fn the_backup_settings_are_adjustable_and_validated() {
        let s = settings();
        s.put(keys::BACKUP_ENABLED, "false", false).unwrap();
        assert!(!s.backup_enabled());
        s.put(keys::BACKUP_ENABLED, "true", false).unwrap();
        assert!(s.backup_enabled());
        assert!(s.put(keys::BACKUP_ENABLED, "peut-être", false).is_err());

        assert!(s.put(keys::BACKUP_INTERVAL_HOURS, "0", false).is_err());
        assert!(s.put(keys::BACKUP_INTERVAL_HOURS, "beaucoup", false).is_err());
        s.put(keys::BACKUP_INTERVAL_HOURS, "6", false).unwrap();
        assert_eq!(s.backup_interval_hours(), 6);

        assert!(s.put(keys::BACKUP_KEEP_LAST, "-1", false).is_err());
    }

    #[test]
    fn keeping_fewer_archives_is_refused_without_explicit_confirmation() {
        // Baisser cette valeur supprime des archives : même règle que la
        // rétention Influx, confirmation exigée côté serveur et pas seulement
        // dans l'interface.
        let s = settings();
        assert_eq!(s.backup_keep_last(), 7);

        let err = s.put(keys::BACKUP_KEEP_LAST, "3", false).unwrap_err();
        assert!(
            err.to_string().contains("confirm"),
            "le refus doit nommer la confirmation attendue : {err}"
        );
        assert_eq!(s.backup_keep_last(), 7, "rien ne doit avoir changé");

        s.put(keys::BACKUP_KEEP_LAST, "3", true).unwrap();
        assert_eq!(s.backup_keep_last(), 3);
    }

    #[test]
    fn keeping_more_archives_needs_no_confirmation() {
        let s = settings();
        assert!(s.put(keys::BACKUP_KEEP_LAST, "30", false).is_ok());
        // 0 = tout garder. C'est l'augmentation maximale, pas une purge :
        // confondre les deux ferait de la valeur la plus prudente la plus
        // destructrice.
        assert!(s.put(keys::BACKUP_KEEP_LAST, "0", false).is_ok());
        assert_eq!(s.backup_keep_last(), 0);
        // Et quitter « tout garder » redevient une réduction.
        assert!(s.put(keys::BACKUP_KEEP_LAST, "7", false).is_err());
        assert!(s.put(keys::BACKUP_KEEP_LAST, "7", true).is_ok());
    }

    #[test]
    fn the_backup_settings_carry_no_secret_and_are_listed() {
        let s = settings();
        let all = s.all();
        assert_eq!(all[keys::BACKUP_ENABLED], serde_json::json!(true));
        assert_eq!(all[keys::BACKUP_INTERVAL_HOURS], serde_json::json!(24));
        assert_eq!(all[keys::BACKUP_KEEP_LAST], serde_json::json!(7));
    }

    #[test]
    fn lengthening_the_retention_needs_no_confirmation() {
        let s = settings();
        s.put(keys::RETENTION_DAYS, "30", true).unwrap();
        assert!(s.put(keys::RETENTION_DAYS, "90", false).is_ok());
        assert!(s.put(keys::RETENTION_DAYS, "0", false).is_ok(), "revenir à illimité n'efface rien");
    }
}
