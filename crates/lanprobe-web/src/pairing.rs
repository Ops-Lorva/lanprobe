//! Appairage d'un appareil et identité durable (contrat § 22).
//!
//! ⚠️ **Souche.** Les signatures sont réelles et le module compile ; les corps
//! restent à écrire. Elles sont posées ici, et pas laissées à la tâche qui les
//! remplira, parce que `web.rs` fait 11 600 lignes et que deux tâches
//! parallèles ne peuvent pas s'y partager le routeur sans se marcher dessus.
//!
//! 🔴 **Les routes rendent `501` et non `todo!()`.** Un `todo!()` sur une route
//! branchée fait paniquer la tâche qui sert la requête et laisse le client
//! devant une connexion coupée, sans rien à lire. Un `501` nommé dit ce qui se
//! passe. Les fonctions de domaine, elles, portent bien un `todo!()` : personne
//! ne les appelle encore.
//!
//! ## Ce que la tâche d'appairage doit tenir
//!
//! - **Le secret d'appareil ne tourne PAS à l'usage.** Faire tourner à chaque
//!   renouvellement perd le secret quand la réponse se perd — c'est-à-dire là
//!   où le réseau est mauvais, donc là où le technicien travaille. La rotation
//!   est un geste explicite depuis le hub.
//! - **Un appareil n'a pas de droits propres** : il hérite du rôle et de la
//!   portée **courants** de son propriétaire, relus à chaque requête.
//! - **Aucune route ne supprime un appareil.** On révoque, la ligne reste.
//! - **Le renouvellement du jeton ne s'écrit PAS au journal d'audit** :
//!   quatre-vingt-seize lignes par jour et par téléphone, dans un journal en
//!   ajout seul et sans purge, noieraient les lignes qui comptent. Les refus,
//!   eux, s'écrivent.

// Une souche n'a pas encore d'appelant : sans cette autorisation, chaque
// signature posée d'avance produirait un avertissement, et le bruit finirait
// par masquer celui qui compte.
#![allow(dead_code)]

use axum::{
    routing::{get, patch, post},
    Router,
};

use rusqlite::OptionalExtension;

use crate::db::{Db, DbError, DbResult, Role};
use crate::web::{guarded, AppState, Identity};

/// Durée de vie d'un jeton d'accès. Quinze minutes : il vit en mémoire vive et
/// se renouvelle en silence, son expiration n'a aucune conséquence visible.
pub(crate) const ACCESS_TOKEN_TTL_SECS: i64 = 900;

/// Durée de vie d'un code d'appairage — la même que celle d'un code
/// d'enrôlement. Pas 24 h : le confort d'une fois par appareil ne paie pas une
/// fenêtre d'attaque d'une journée.
pub(crate) const PAIR_CODE_TTL_SECS: i64 = 15 * 60;

/// Pourquoi le hub refuse de renouveler un jeton d'accès.
///
/// 🔴 **Trois causes, trois conduites** — d'où trois motifs et non un
/// « session expirée » unique. Le mot « session » est d'ailleurs à ne plus
/// employer ici : il fait attendre une reconnexion qui n'arrivera pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// « Cet appareil a été révoqué depuis le hub. » Réappairer serait un
    /// contournement : la personne appelle celui qui l'a révoqué.
    Revoked,
    /// « Ce hub ne connaît pas cet appareil — il a peut-être été restauré
    /// depuis une sauvegarde antérieure à l'appairage. » Elle réappaire,
    /// légitimement.
    Unknown,
    /// « Le compte a été désactivé. » Rien de ce qu'elle fera sur ce téléphone
    /// n'y changera quelque chose.
    AccountDisabled,
}

impl Refusal {
    /// Le motif tel qu'il part dans le corps de la réponse — c'est sur lui que
    /// l'app décide de s'arrêter ou de réappairer.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Refusal::Revoked => "revoked",
            Refusal::Unknown => "unknown",
            Refusal::AccountDisabled => "account_disabled",
        }
    }
}

/// Un appareil appairé, tel que l'écran « Appareils » le montre.
///
/// ⚠️ Le secret n'y figure pas et n'y figurera jamais : il est haché en base
/// et rendu **une seule fois**, à l'appairage.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PairedDevice {
    pub device_id: String,
    pub username: String,
    pub name: String,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    pub created_at: i64,
    /// ⚠️ Affichée telle quelle (« vu il y a 41 j »), jamais convertie en
    /// verdict : il n'y a **aucune** révocation par inactivité. Une révocation
    /// « non vu depuis N jours » est une expiration déguisée qui se déclenche
    /// au retour de congés.
    pub last_seen: Option<i64>,
    /// La dernière adresse vue. Avec `last_seen`, c'est le **seul** détecteur
    /// d'un secret copié, puisque le secret ne tourne pas à l'usage.
    pub last_ip: Option<String>,
    pub revoked_at: Option<i64>,
}

/// Un code d'appairage fraîchement émis, tel que le hub web l'affiche.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PairCode {
    /// En clair : il est lu à l'écran et recopié à la main. Il n'est rendu que
    /// pendant sa validité.
    pub code: String,
    pub expires_at: i64,
}

/// Colonnes de `paired_devices`, dans l'ordre où [`device_from_row`] les lit.
/// Une seule liste : deux `SELECT` qui divergent d'une colonne se paient en
/// panique au premier appel de la seule route qui l'emploie.
const DEVICE_COLUMNS: &str = "device_id, username, name, platform, app_version, \
                              created_at, last_seen, last_ip, revoked_at";

fn device_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PairedDevice> {
    Ok(PairedDevice {
        device_id: row.get(0)?,
        username: row.get(1)?,
        name: row.get(2)?,
        platform: row.get(3)?,
        app_version: row.get(4)?,
        created_at: row.get(5)?,
        last_seen: row.get(6)?,
        last_ip: row.get(7)?,
        revoked_at: row.get(8)?,
    })
}

/// Le stockage de l'appairage, hors de `db.rs` — qui fait déjà 5 200 lignes et
/// qu'aucune tâche parallèle ne peut se partager.
impl Db {
    /// Émet un code court pour un compte. Usage unique, 15 minutes.
    pub(crate) fn create_pair_code(&self, username: &str, ttl_secs: i64) -> DbResult<PairCode> {
        // Le même générateur que le code d'enrôlement, et pas une copie :
        // l'alphabet sans `0`/`O` ni `1`/`I` est ce qui rend le code dictable
        // au téléphone.
        let code = crate::db::generate_enroll_code();
        let code_hash = lanprobe_core::passwords::hash_password(&code)
            .map_err(|e| DbError::Internal(e.to_string()))?;
        let created_at = crate::db::now();
        let expires_at = created_at + ttl_secs;
        {
            let conn = self.conn().lock().map_err(|_| poisoned())?;
            conn.execute(
                "INSERT INTO pair_codes (code_hash, username, created_at, expires_at, code_plain)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![code_hash, username, created_at, expires_at, code],
            )?;
        }
        Ok(PairCode { code, expires_at })
    }

    /// Consomme un code et rend le compte auquel il appartient.
    ///
    /// ⚠️ **Consommé au premier succès**, dans la même transaction que sa
    /// lecture : deux téléphones qui présentent le même code au même instant
    /// doivent laisser exactement un gagnant.
    pub(crate) fn consume_pair_code(&self, code: &str, now: i64) -> DbResult<String> {
        let mut conn = self.conn().lock().map_err(|_| poisoned())?;
        // `IMMEDIATE` prend le verrou d'écriture dès l'ouverture : la lecture
        // des candidats et la marque de consommation ne peuvent pas être
        // entrelacées avec celles d'une autre transaction.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

        // Le code est haché : on ne peut pas l'indexer, il faut vérifier les
        // candidats vivants un par un. Ils se comptent sur les doigts d'une
        // main — un code vit 15 minutes.
        let candidates: Vec<(String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT code_hash, username FROM pair_codes
                 WHERE consumed_at IS NULL AND expires_at > ?1",
            )?;
            let rows = stmt.query_map([now], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        for (code_hash, username) in candidates {
            if lanprobe_core::passwords::verify_password(&code_hash, code).is_err() {
                continue;
            }
            let changed = tx.execute(
                "UPDATE pair_codes SET code_plain = NULL, consumed_at = ?2
                 WHERE code_hash = ?1 AND consumed_at IS NULL",
                rusqlite::params![code_hash, now],
            )?;
            if changed == 0 {
                break;
            }
            tx.commit()?;
            return Ok(username);
        }
        // ⚠️ Un seul message pour les trois cas — inconnu, expiré, déjà
        // utilisé. Distinguer « déjà utilisé » apprendrait à qui essaie au
        // hasard qu'il a touché un code réel.
        Err(DbError::Unauthorized(
            "code d'appairage inconnu, expiré ou déjà utilisé".into(),
        ))
    }

    /// Les codes de ce compte encore ouverts, purge du clair expiré comprise.
    /// Sert à ce que réémettre un code n'en laisse pas trois valables.
    pub(crate) fn purge_stale_pair_codes(&self, now: i64) -> DbResult<()> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        // Le clair ne survit ni à l'expiration ni à la consommation. On purge
        // ici plutôt que par une tâche de fond : pas de minuterie à
        // surveiller, et c'est fait avant toute lecture.
        conn.execute(
            "UPDATE pair_codes SET code_plain = NULL
             WHERE code_plain IS NOT NULL AND (expires_at <= ?1 OR consumed_at IS NOT NULL)",
            [now],
        )?;
        Ok(())
    }

    /// Enregistre un appareil et rend `(la ligne, le secret en clair)`.
    ///
    /// ⚠️ Le secret n'est rendu **qu'ici, une fois**. La base n'en garde que le
    /// condensat argon2id, comme pour un jeton de sonde.
    pub(crate) fn pair_device(
        &self,
        username: &str,
        name: &str,
        platform: Option<&str>,
        app_version: Option<&str>,
    ) -> DbResult<(PairedDevice, String)> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DbError::Conflict("le nom de l'appareil est requis".into()));
        }
        let secret = lanprobe_core::passwords::generate_token();
        let secret_hash = lanprobe_core::passwords::hash_password(&secret)
            .map_err(|e| DbError::Internal(e.to_string()))?;
        let device_id = uuid::Uuid::new_v4().to_string();
        {
            let conn = self.conn().lock().map_err(|_| poisoned())?;
            conn.execute(
                "INSERT INTO paired_devices
                   (device_id, username, name, secret_hash, platform, app_version, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    device_id,
                    username,
                    name,
                    secret_hash,
                    platform,
                    app_version,
                    crate::db::now()
                ],
            )?;
        }
        Ok((self.get_paired_device(&device_id)?, secret))
    }

    /// Un appareil, révoqué ou non. La révocation ne cache pas la ligne : on
    /// ne supprime rien, et l'écran « Appareils » montre l'état.
    pub(crate) fn get_paired_device(&self, device_id: &str) -> DbResult<PairedDevice> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        conn.query_row(
            &format!("SELECT {DEVICE_COLUMNS} FROM paired_devices WHERE device_id = ?1"),
            [device_id],
            device_from_row,
        )
        .optional()?
        .ok_or_else(|| DbError::NotFound("appareil inconnu".into()))
    }

    /// Vérifie un secret d'appareil.
    ///
    /// ⚠️ **Le compte désactivé se vérifie ICI**, pas seulement à la connexion
    /// web : un appareil qui survivrait à la désactivation de son propriétaire
    /// serait un trou dans la seule procédure de départ d'un salarié.
    pub(crate) fn authenticate_device(
        &self,
        device_id: &str,
        secret: &str,
    ) -> Result<PairedDevice, Refusal> {
        let device = self.get_paired_device(device_id).map_err(|_| Refusal::Unknown)?;
        let stored: String = {
            let conn = self.conn().lock().map_err(|_| Refusal::Unknown)?;
            conn.query_row(
                "SELECT secret_hash FROM paired_devices WHERE device_id = ?1",
                [device_id],
                |r| r.get(0),
            )
            .map_err(|_| Refusal::Unknown)?
        };
        // 🔴 **Le secret se vérifie AVANT de nommer le motif.** Répondre
        // « révoqué » à qui présente un `device_id` sans son secret
        // apprendrait l'existence de l'appareil à qui ne la connaissait pas.
        // Un mauvais secret est donc `Unknown`, comme un appareil absent.
        if lanprobe_core::passwords::verify_password(&stored, secret).is_err() {
            return Err(Refusal::Unknown);
        }
        // ⚠️ **`revoked_at` est relu ICI, à chaque renouvellement**, jamais
        // mis en cache : la révocation est le seul mécanisme de sécurité du
        // modèle, et un cache de quinze minutes laisserait un accès résiduel
        // sur un téléphone perdu.
        if device.revoked_at.is_some() {
            return Err(Refusal::Revoked);
        }
        let owner = self
            .get_user(&device.username)
            .map_err(|_| Refusal::AccountDisabled)?;
        if owner.disabled_at.is_some() {
            return Err(Refusal::AccountDisabled);
        }
        Ok(device)
    }

    /// Les appareils d'un compte, ou de tous quand `username` est `None`.
    /// Les révoqués **en font partie** : la ligne reste, l'audit la nomme.
    pub(crate) fn list_paired_devices(
        &self,
        username: Option<&str>,
    ) -> DbResult<Vec<PairedDevice>> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {DEVICE_COLUMNS} FROM paired_devices
             WHERE (?1 IS NULL OR username = ?1)
             ORDER BY created_at DESC, device_id"
        ))?;
        let rows = stmt.query_map([username], device_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// « iPhone » et « iPhone » ne se distinguent pas, et on ne révoque pas au
    /// hasard dans une liste.
    pub(crate) fn rename_paired_device(&self, device_id: &str, name: &str) -> DbResult<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DbError::Conflict("le nom de l'appareil est requis".into()));
        }
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        let changed = conn.execute(
            "UPDATE paired_devices SET name = ?2 WHERE device_id = ?1",
            rusqlite::params![device_id, name],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound("appareil inconnu".into()));
        }
        Ok(())
    }

    /// Révoque — **ne supprime pas**. `revoked_at` daté, la ligne reste.
    pub(crate) fn revoke_paired_device(&self, device_id: &str) -> DbResult<()> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        let changed = conn.execute(
            "UPDATE paired_devices SET revoked_at = ?2
             WHERE device_id = ?1 AND revoked_at IS NULL",
            rusqlite::params![device_id, crate::db::now()],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound("appareil inconnu ou déjà révoqué".into()));
        }
        Ok(())
    }

    /// Fait tourner le secret sur un geste explicite, et rend le nouveau.
    ///
    /// ⚠️ **Jamais appelée depuis le renouvellement de jeton.** C'est tout
    /// l'arbitrage du § 22 : une rotation à l'usage perd le secret quand la
    /// réponse se perd, là où le réseau est mauvais.
    pub(crate) fn rotate_paired_device_secret(&self, device_id: &str) -> DbResult<String> {
        let secret = lanprobe_core::passwords::generate_token();
        let secret_hash = lanprobe_core::passwords::hash_password(&secret)
            .map_err(|e| DbError::Internal(e.to_string()))?;
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        // ⚠️ Sans recouvrement : l'ancien secret meurt à l'instant. C'est la
        // variante écartée en v1 du § 22 — elle règle la réponse perdue, coûte
        // deux secrets à tenir de chaque côté, et l'écran « Appareils » couvre
        // déjà ce qu'elle apporterait. Écartée, pas condamnée.
        let changed = conn.execute(
            "UPDATE paired_devices SET secret_hash = ?2 WHERE device_id = ?1",
            rusqlite::params![device_id, secret_hash],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound("appareil inconnu".into()));
        }
        Ok(secret)
    }

    /// Note la dernière vue et la dernière adresse.
    ///
    /// ⚠️ Écrite au renouvellement du jeton, qui ne laisse **aucune** ligne
    /// d'audit : c'est cette colonne, et elle seule, qui rend l'usage d'un
    /// appareil observable.
    pub(crate) fn touch_paired_device(
        &self,
        device_id: &str,
        now: i64,
        ip: Option<&str>,
    ) -> DbResult<()> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        conn.execute(
            "UPDATE paired_devices SET last_seen = ?2, last_ip = COALESCE(?3, last_ip)
             WHERE device_id = ?1",
            rusqlite::params![device_id, now, ip],
        )?;
        Ok(())
    }
}

fn poisoned() -> DbError {
    DbError::Internal("verrou de la base empoisonné".into())
}

/// Relit un jeton d'accès et rend l'identité de son propriétaire.
///
/// 🔴 **C'est le crochet que `session_guard` appelle.** Il rend `None`
/// aujourd'hui : aucun jeton n'est encore reconnu, et un porteur inconnu se
/// solde donc par le même `401` qu'une session absente.
///
/// ⚠️ **Rôle et portée sont relus dans `identity_of`, jamais lus dans le
/// jeton.** Un jeton qui porterait le rôle survivrait quinze minutes à une
/// rétrogradation, et une portée gelée survivrait à sa révocation.
///
/// ⚠️ Le jeton est **sans état** : signé avec la clé scellée du hub, jamais
/// stocké. Une table de jetons de quinze minutes demanderait un balayage de
/// nettoyage que personne ne surveille, pour n'apporter qu'une révocation
/// immédiate que la révocation de l'appareil couvre déjà.
pub(crate) fn identity_from_access_token(_state: &AppState, _token: &str) -> Option<Identity> {
    // ⚠️ Pas de `todo!()` : cette fonction est appelée à CHAQUE requête portant
    // un en-tête `Authorization`. Une panique ici couperait le hub sur une
    // requête quelconque, avant même que l'appairage existe.
    None
}

/// Les routes de l'appairage (contrat § 22).
///
/// ⚠️ **`POST /api/pair` et `POST /api/devices/token` sont publiques** — c'est
/// le code, puis le secret d'appareil, qui authentifient. Les ranger derrière
/// la garde de session mettrait `/api/devices/token` derrière ce qu'elle existe
/// précisément pour remplacer.
pub(crate) fn routes(state: &AppState) -> Router<AppState> {
    // Le code, puis le secret d'appareil, authentifient. Aucune session.
    let ouvert = Router::new()
        .route("/api/pair", post(not_implemented))
        .route("/api/devices/token", post(not_implemented));

    // Chacun agit sur SON compte, quel que soit son rôle (contrat § 19).
    // Ouvert à `viewer` : celui qui perd son téléphone à 22 h doit pouvoir le
    // révoquer seul.
    let les_siens = guarded(
        state,
        Role::Viewer,
        Router::new()
            .route("/api/me/pair-codes", post(not_implemented))
            .route("/api/me/devices", get(not_implemented))
            .route("/api/me/devices/{id}", patch(not_implemented))
            .route("/api/me/devices/{id}/revoke", post(not_implemented)),
    );

    // Ceux de tous les comptes : un opérateur parti de l'entreprise ne se
    // révoque pas lui-même.
    let ceux_de_tous = guarded(
        state,
        Role::Admin,
        Router::new()
            .route("/api/devices", get(not_implemented))
            .route("/api/devices/{id}/revoke", post(not_implemented))
            .route("/api/devices/{id}/rotate", post(not_implemented)),
    );

    ouvert.merge(les_siens).merge(ceux_de_tous)
}

/// ⚠️ Une route annoncée mais pas encore écrite dit `501`, pas `404` : `404`
/// laisserait croire à un client mal versionné qu'il s'est trompé de chemin.
async fn not_implemented() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::NOT_IMPLEMENTED,
        axum::Json(serde_json::json!({ "error": "appairage non encore implémenté" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{now, Scope};

    /// Un hub installé, un compte admin, rien d'autre. La couche de stockage
    /// n'a besoin ni d'Influx ni du routeur.
    fn db_with_admin() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.create_initial_admin("admin", "mot-de-passe-de-test").unwrap();
        db
    }

    // ── Le code d'appairage ────────────────────────────────────────────────

    #[test]
    fn a_pair_code_names_the_account_that_issued_it() {
        // Le code ne crée aucun compte et n'élève aucun droit : il désigne
        // celui qui l'a émis, et l'appareil héritera de LUI (contrat § 22).
        let db = db_with_admin();

        let code = db.create_pair_code("admin", PAIR_CODE_TTL_SECS).unwrap();

        assert!(code.expires_at > now(), "un code émis est vivant");
        assert_eq!(db.consume_pair_code(&code.code, now()).unwrap(), "admin");
    }

    #[test]
    fn a_pair_code_consumed_twice_fails_the_second_time() {
        // 🔴 « Un second essai échouera — et c'est le signal. » Un code
        // réutilisable serait un identifiant permanent posé dans une image.
        let db = db_with_admin();
        let code = db.create_pair_code("admin", PAIR_CODE_TTL_SECS).unwrap();

        db.consume_pair_code(&code.code, now()).unwrap();

        let second = db.consume_pair_code(&code.code, now());
        assert!(second.is_err(), "le second essai passe : {second:?}");
    }

    #[test]
    fn an_expired_pair_code_opens_nothing() {
        // Pas 24 h : le confort d'une fois par appareil ne paie pas une
        // fenêtre d'attaque d'une journée.
        let db = db_with_admin();
        let code = db.create_pair_code("admin", -1).unwrap();

        assert!(db.consume_pair_code(&code.code, now()).is_err());
    }

    #[test]
    fn the_stored_pair_code_is_hashed_and_the_plaintext_dies_with_it() {
        // Une base volée ne doit pas livrer de quoi appairer un téléphone.
        let db = db_with_admin();
        let code = db.create_pair_code("admin", PAIR_CODE_TTL_SECS).unwrap();

        let hashes: Vec<String> = {
            let conn = db.conn().lock().unwrap();
            let mut stmt = conn.prepare("SELECT code_hash FROM pair_codes").unwrap();
            let rows = stmt.query_map([], |r| r.get(0)).unwrap();
            rows.collect::<Result<Vec<String>, _>>().unwrap()
        };
        assert_eq!(hashes.len(), 1);
        assert!(hashes[0].starts_with("$argon2id$"), "{}", hashes[0]);
        assert!(!hashes[0].contains(&code.code));

        // Le clair ne survit pas à la consommation : il n'était là que pour
        // l'écran qui l'affiche.
        db.consume_pair_code(&code.code, now()).unwrap();
        let restant: Option<String> = db
            .conn()
            .lock()
            .unwrap()
            .query_row("SELECT code_plain FROM pair_codes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(restant, None);
    }

    // ── L'identité d'appareil ──────────────────────────────────────────────

    #[test]
    fn pairing_returns_the_secret_once_and_stores_only_its_hash() {
        let db = db_with_admin();

        let (device, secret) = db
            .pair_device("admin", "iPhone 15 — Benjamin", Some("iOS 18.2"), Some("1.0.0"))
            .unwrap();

        assert_eq!(device.username, "admin");
        assert_eq!(device.name, "iPhone 15 — Benjamin");
        assert!(device.revoked_at.is_none());
        assert!(device.last_seen.is_none(), "jamais vu avant sa première requête");

        let hash: String = db
            .conn()
            .lock()
            .unwrap()
            .query_row("SELECT secret_hash FROM paired_devices", [], |r| r.get(0))
            .unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        assert!(!hash.contains(&secret));

        // Il n'existe aucune lecture qui rende le secret : il ne sort qu'ici.
        let rendu = serde_json::to_string(&device).unwrap();
        assert!(!rendu.contains(&secret), "{rendu}");
        assert!(!rendu.contains("$argon2"), "{rendu}");
    }

    #[test]
    fn a_paired_device_authenticates_on_its_secret() {
        let db = db_with_admin();
        let (device, secret) = db.pair_device("admin", "iPhone", None, None).unwrap();

        let vu = db.authenticate_device(&device.device_id, &secret).unwrap();
        assert_eq!(vu.device_id, device.device_id);

        assert_eq!(
            db.authenticate_device(&device.device_id, "mauvais-secret").unwrap_err(),
            Refusal::Unknown,
            "un mauvais secret ne dit pas si l'appareil existe"
        );
        assert_eq!(
            db.authenticate_device("appareil-jamais-vu", &secret).unwrap_err(),
            Refusal::Unknown
        );
    }

    #[test]
    fn a_revoked_device_is_refused_with_its_own_reason() {
        // 🔴 Trois causes, trois conduites. « Session expirée » ferait attendre
        // une reconnexion qui n'arrivera pas.
        let db = db_with_admin();
        let (device, secret) = db.pair_device("admin", "iPhone", None, None).unwrap();

        db.revoke_paired_device(&device.device_id).unwrap();

        assert_eq!(
            db.authenticate_device(&device.device_id, &secret).unwrap_err(),
            Refusal::Revoked
        );
        // La ligne reste : le journal d'audit la nomme, et une ligne d'audit
        // qui pointe une cible disparue ne prouve rien.
        let restant = db.list_paired_devices(Some("admin")).unwrap();
        assert_eq!(restant.len(), 1);
        assert!(restant[0].revoked_at.is_some());
    }

    #[test]
    fn a_device_falls_with_the_account_that_owns_it() {
        // Un appareil qui survivrait à la désactivation de son propriétaire
        // serait un trou dans la seule procédure de départ d'un salarié.
        let db = db_with_admin();
        db.create_user("dupont", "mot-de-passe-de-test", Role::Operator)
            .unwrap();
        let (device, secret) = db.pair_device("dupont", "iPhone", None, None).unwrap();

        db.set_user_disabled("admin", "dupont", true).unwrap();

        assert_eq!(
            db.authenticate_device(&device.device_id, &secret).unwrap_err(),
            Refusal::AccountDisabled
        );
    }

    #[test]
    fn rotating_the_secret_is_an_explicit_gesture_that_kills_the_old_one() {
        // ⚠️ Le secret ne tourne PAS à l'usage : faire tourner à chaque
        // renouvellement perd le secret quand la réponse se perd, c'est-à-dire
        // là où le réseau est mauvais — là où le technicien travaille. Ceci
        // n'arrive donc que sur un geste depuis le hub.
        let db = db_with_admin();
        let (device, ancien) = db.pair_device("admin", "iPhone", None, None).unwrap();

        let nouveau = db.rotate_paired_device_secret(&device.device_id).unwrap();

        assert_ne!(nouveau, ancien);
        assert!(db.authenticate_device(&device.device_id, &nouveau).is_ok());
        assert_eq!(
            db.authenticate_device(&device.device_id, &ancien).unwrap_err(),
            Refusal::Unknown
        );
    }

    #[test]
    fn the_last_seen_and_last_address_are_the_only_human_detector() {
        // Faute de rotation à l'usage, il n'y a plus de rejeu à repérer : la
        // dernière vue et la dernière adresse sont ce qui reste. C'est ce qui
        // rend l'écran « Appareils » obligatoire plutôt que confortable.
        let db = db_with_admin();
        let (device, _) = db.pair_device("admin", "iPhone", None, None).unwrap();

        db.touch_paired_device(&device.device_id, 1_700_000_000, Some("10.0.0.42"))
            .unwrap();

        let vu = &db.list_paired_devices(Some("admin")).unwrap()[0];
        assert_eq!(vu.last_seen, Some(1_700_000_000));
        assert_eq!(vu.last_ip.as_deref(), Some("10.0.0.42"));
    }

    #[test]
    fn devices_are_listed_per_account_or_for_everyone() {
        let db = db_with_admin();
        db.create_user("dupont", "mot-de-passe-de-test", Role::Viewer)
            .unwrap();
        db.pair_device("admin", "iPhone d'admin", None, None).unwrap();
        db.pair_device("dupont", "iPhone de Dupont", None, None).unwrap();

        assert_eq!(db.list_paired_devices(Some("dupont")).unwrap().len(), 1);
        assert_eq!(db.list_paired_devices(None).unwrap().len(), 2);
    }

    #[test]
    fn a_device_can_be_renamed_because_two_iphones_look_alike() {
        // « iPhone » et « iPhone » ne se distinguent pas, et on ne révoque pas
        // au hasard dans une liste.
        let db = db_with_admin();
        let (device, _) = db.pair_device("admin", "iPhone", None, None).unwrap();

        db.rename_paired_device(&device.device_id, "iPhone — atelier")
            .unwrap();

        assert_eq!(
            db.list_paired_devices(Some("admin")).unwrap()[0].name,
            "iPhone — atelier"
        );
    }

    #[test]
    fn a_device_inherits_the_current_role_and_scope_of_its_owner() {
        // ⚠️ Un appareil n'a AUCUN droit propre. Rien dans sa ligne ne porte un
        // rôle ni une portée : il n'y a donc rien à figer, et rien qui puisse
        // survivre à une rétrogradation.
        let db = db_with_admin();
        let site = db.create_site("Durand").unwrap();
        db.create_user("dupont", "mot-de-passe-de-test", Role::Operator)
            .unwrap();
        db.pair_device("dupont", "iPhone", None, None).unwrap();

        assert_eq!(db.user_scope("dupont", Role::Operator).unwrap(), Scope::All);

        db.set_user_sites("dupont", &[site.site_id.clone()]).unwrap();
        assert_eq!(
            db.user_scope("dupont", Role::Operator).unwrap(),
            Scope::Sites(vec![site.site_id])
        );
    }
}
