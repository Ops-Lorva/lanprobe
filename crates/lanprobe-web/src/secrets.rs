//! Secrets du hub, scellés au repos.
//!
//! **Ce qui vit ici n'a pas sa place dans `settings`.** La section 7 du
//! contrat pose que la table `settings` ne contient aucun secret, et que
//! `GET /api/settings` peut donc être rendue telle quelle. Un mot de passe
//! SMTP et une URL de webhook — qui est elle-même un secret porteur : qui la
//! connaît peut écrire dans le salon — violent les deux.
//!
//! Le scellement est celui de `lanprobe_core::secrets` : AES-256-GCM, marqueur
//! `enc:v1:`, clé dans `<config-dir>/secret.key` (0600) ou dans
//! `LANPROBE_SECRET_KEY`. Le même que le serveur headless, pour qu'il n'y ait
//! qu'un format à comprendre le jour où quelqu'un ouvre le volume.
//!
//! **Aucune de ces valeurs ne sort par l'API.** L'interface sait « configuré »
//! ou « non configuré », et dispose d'un bouton de test qui envoie un vrai
//! message — c'est la seule vérification qui prouve quelque chose.

use std::path::Path;
use std::sync::Arc;

use lanprobe_core::secrets::{self, SecretKey};

use crate::db::{Db, DbResult};

pub mod keys {
    /// Configuration SMTP complète, scellée en un bloc.
    pub const SMTP: &str = "notify_smtp";
    /// Webhook générique : URL et modèle.
    pub const WEBHOOK: &str = "notify_webhook";
}

#[derive(Clone)]
pub struct Secrets {
    db: Arc<Db>,
    key: Arc<SecretKey>,
}

impl Secrets {
    /// Charge la clé du volume — ou de `LANPROBE_SECRET_KEY` si elle est
    /// définie, auquel cas rien n'est écrit sur disque.
    pub fn load(db: Arc<Db>, config_dir: &Path) -> Result<Self, String> {
        Ok(Self {
            db,
            key: Arc::new(secrets::load_or_create_key(config_dir)?),
        })
    }

    /// Clé tirée en mémoire, jamais écrite. Sert aux tests, qui travaillent
    /// sur une base en mémoire et n'ont aucun volume.
    pub fn ephemeral(db: Arc<Db>) -> Result<Self, String> {
        Ok(Self {
            db,
            key: Arc::new(secrets::generate_key()?),
        })
    }

    /// Scelle une chaîne **sans la stocker**, et rend le sceau.
    ///
    /// 🔴 C'est ce qui rend croyable le jeton d'accès d'un appareil appairé
    /// (contrat § 22) : le jeton est *sans état*, rien ne le retrouve en base,
    /// c'est le sceau seul qui prouve que le hub l'a émis. Une table de jetons
    /// de quinze minutes demanderait un balayage de nettoyage que personne ne
    /// surveille, pour n'apporter qu'une révocation immédiate que la
    /// révocation de l'appareil couvre déjà.
    ///
    /// ⚠️ La clé est celle du volume : elle survit aux redémarrages, et un
    /// jeton scellé par un autre hub ne s'ouvre pas ici.
    pub(crate) fn seal_text(&self, plain: &str) -> Result<String, String> {
        secrets::seal(&self.key, plain)
    }

    /// Ouvre un sceau produit par [`Secrets::seal_text`]. Échoue si la valeur
    /// n'est pas scellée, si la clé a changé, ou si le sceau a été altéré.
    pub(crate) fn open_text(&self, sealed: &str) -> Result<String, String> {
        secrets::open(&self.key, sealed)
    }

    pub fn put_json<T: serde::Serialize>(&self, key: &str, value: &T) -> DbResult<()> {
        let plain = serde_json::to_string(value)
            .map_err(|e| crate::db::DbError::Internal(e.to_string()))?;
        let sealed = secrets::seal(&self.key, &plain)
            .map_err(|e| crate::db::DbError::Internal(e.to_string()))?;
        self.db.set_sealed(key, &sealed)
    }

    /// Rend la valeur, ou `None` si elle n'est pas configurée **ou** si le
    /// sceau ne s'ouvre pas.
    ///
    /// Un sceau illisible — clé perdue, volume restauré sans son `secret.key`
    /// — est journalisé bruyamment puis traité comme « non configuré » : un
    /// hub qui refuse de démarrer parce qu'un webhook ne se déchiffre plus
    /// serait une panne totale pour une fonctionnalité accessoire.
    pub fn get_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        let sealed = self.db.get_sealed(key).ok().flatten()?;
        match secrets::open(&self.key, &sealed) {
            Ok(plain) => match serde_json::from_str(&plain) {
                Ok(value) => Some(value),
                Err(e) => {
                    tracing::error!("secret « {key} » illisible après déchiffrement : {e}");
                    None
                }
            },
            Err(e) => {
                tracing::error!(
                    "secret « {key} » non déchiffrable ({e}) — \
                     la clé du volume a-t-elle changé ? Le canal est traité comme non configuré."
                );
                None
            }
        }
    }

    /// Vrai si la clé porte une valeur — sans dire laquelle. C'est tout ce que
    /// l'API a le droit d'annoncer.
    pub fn is_set(&self, key: &str) -> bool {
        self.db.get_sealed(key).ok().flatten().is_some()
    }

    /// Désactive un canal. La ligne reste, vidée : rien ne se supprime.
    pub fn clear(&self, key: &str) -> DbResult<()> {
        self.db.clear_sealed(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Fake {
        url: String,
    }

    fn store() -> (Arc<Db>, Secrets) {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let secrets = Secrets::ephemeral(db.clone()).unwrap();
        (db, secrets)
    }

    #[test]
    fn a_secret_round_trips() {
        let (_db, secrets) = store();
        assert!(!secrets.is_set(keys::WEBHOOK));

        let value = Fake {
            url: "https://hooks.example/T000/B000/xoxb-secret".into(),
        };
        secrets.put_json(keys::WEBHOOK, &value).unwrap();

        assert!(secrets.is_set(keys::WEBHOOK));
        assert_eq!(secrets.get_json::<Fake>(keys::WEBHOOK), Some(value));
    }

    #[test]
    fn the_stored_row_carries_no_plaintext() {
        // Une base volée ne doit pas livrer l'URL du webhook : la connaître,
        // c'est pouvoir écrire dans le salon du client.
        let (db, secrets) = store();
        secrets
            .put_json(
                keys::WEBHOOK,
                &Fake {
                    url: "https://hooks.example/xoxb-secret".into(),
                },
            )
            .unwrap();

        let stored = db.get_sealed(keys::WEBHOOK).unwrap().unwrap();
        assert!(stored.starts_with("enc:v1:"), "valeur non scellée : {stored}");
        assert!(!stored.contains("xoxb-secret"), "{stored}");
        assert!(!stored.contains("hooks.example"), "{stored}");
    }

    #[test]
    fn clearing_unconfigures_the_channel() {
        let (_db, secrets) = store();
        secrets
            .put_json(keys::SMTP, &Fake { url: "x".into() })
            .unwrap();

        secrets.clear(keys::SMTP).unwrap();

        assert!(!secrets.is_set(keys::SMTP));
        assert_eq!(secrets.get_json::<Fake>(keys::SMTP), None);
    }

    #[test]
    fn a_seal_from_another_key_is_treated_as_unconfigured() {
        // Volume restauré sans son `secret.key` : le hub doit continuer de
        // servir, pas refuser de démarrer pour une fonctionnalité accessoire.
        let db = Arc::new(Db::open_in_memory().unwrap());
        let first = Secrets::ephemeral(db.clone()).unwrap();
        first
            .put_json(keys::WEBHOOK, &Fake { url: "x".into() })
            .unwrap();

        let second = Secrets::ephemeral(db.clone()).unwrap();

        assert_eq!(second.get_json::<Fake>(keys::WEBHOOK), None);
        // `is_set` reste vrai : la ligne existe. C'est ce qui permet à
        // l'interface de dire « configuré mais illisible » plutôt que de
        // faire croire que personne n'avait rien réglé.
        assert!(second.is_set(keys::WEBHOOK));
    }
}
