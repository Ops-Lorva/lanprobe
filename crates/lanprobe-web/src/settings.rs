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

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::db::{Db, DbError, DbResult};

pub mod keys {
    pub const INFLUX_URL: &str = "influx_url";
    pub const INFLUX_ORG: &str = "influx_org";
    pub const INFLUX_BUCKET: &str = "influx_bucket";
    pub const INFLUX_ADVERTISE_URL: &str = "influx_advertise_url";
    pub const RETENTION_DAYS: &str = "retention_days";
    pub const HEARTBEAT_INTERVAL_SECS: &str = "heartbeat_interval_secs";

    pub const ALL: &[&str] = &[
        INFLUX_URL,
        INFLUX_ORG,
        INFLUX_BUCKET,
        INFLUX_ADVERTISE_URL,
        RETENTION_DAYS,
        HEARTBEAT_INTERVAL_SECS,
    ];
}

pub const DEFAULT_INFLUX_URL: &str = "https://127.0.0.1:8086";
pub const DEFAULT_INFLUX_ORG: &str = "lanprobe";
pub const DEFAULT_INFLUX_BUCKET: &str = "lanprobe";
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: i64 = 60;

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
    pub fn stored_advertise_url(&self) -> Option<String> {
        self.db.get_setting(keys::INFLUX_ADVERTISE_URL).ok().flatten()
    }

    /// Rétention du bucket en jours. `0` = illimitée.
    pub fn retention_days(&self) -> i64 {
        self.get_or(keys::RETENTION_DAYS, "0").parse().unwrap_or(0)
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
    pub fn all(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        out.insert(keys::INFLUX_URL.into(), self.influx_url());
        out.insert(keys::INFLUX_ORG.into(), self.influx_org());
        out.insert(keys::INFLUX_BUCKET.into(), self.influx_bucket());
        out.insert(
            keys::INFLUX_ADVERTISE_URL.into(),
            self.stored_advertise_url().unwrap_or_default(),
        );
        out.insert(keys::RETENTION_DAYS.into(), self.retention_days().to_string());
        out.insert(
            keys::HEARTBEAT_INTERVAL_SECS.into(),
            self.heartbeat_interval_secs().to_string(),
        );
        out
    }

    /// Écrit un réglage après validation. `confirm_data_loss` n'a d'effet que
    /// sur `retention_days` : raccourcir une rétention efface des mesures, et
    /// sur ce projet rien ne s'efface sans accord explicite.
    pub fn put(&self, key: &str, value: &str, confirm_data_loss: bool) -> DbResult<()> {
        let value = value.trim();
        match key {
            keys::INFLUX_URL | keys::INFLUX_ADVERTISE_URL => validate_absolute_url(value)?,
            keys::INFLUX_ORG | keys::INFLUX_BUCKET => {
                if value.is_empty() {
                    return Err(DbError::Conflict(format!("{key} ne peut pas être vide")));
                }
            }
            keys::HEARTBEAT_INTERVAL_SECS => {
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
            other => return Err(DbError::Conflict(format!("réglage inconnu : {other}"))),
        }
        self.db.set_setting(key, value)
    }

    /// Vrai si passer à `new_days` raccourcit la fenêtre conservée. `0` vaut
    /// l'infini : quitter l'illimité est la réduction la plus destructrice,
    /// pas une augmentation.
    fn retention_shortens_to(&self, new_days: i64) -> bool {
        let current = self.retention_days();
        match (current, new_days) {
            (_, 0) => false,
            (0, _) => true,
            (c, n) => n < c,
        }
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
    fn leaving_unlimited_retention_is_a_reduction_too() {
        // 0 = illimité : passer à 90 jours efface tout ce qui est plus vieux.
        // C'est la réduction la plus destructrice, pas une augmentation.
        let s = settings();
        assert_eq!(s.retention_days(), 0);

        assert!(s.put(keys::RETENTION_DAYS, "90", false).is_err());
        assert!(s.put(keys::RETENTION_DAYS, "90", true).is_ok());
    }

    #[test]
    fn lengthening_the_retention_needs_no_confirmation() {
        let s = settings();
        s.put(keys::RETENTION_DAYS, "30", true).unwrap();
        assert!(s.put(keys::RETENTION_DAYS, "90", false).is_ok());
        assert!(s.put(keys::RETENTION_DAYS, "0", false).is_ok(), "revenir à illimité n'efface rien");
    }
}
