//! Appairage d'un appareil et identité durable (contrat § 22).
//!
//! L'exigence est une phrase, et elle commande tout le reste : « je ne veux pas
//! devoir taper mon mot de passe ou me ré-enrôler en haut d'une échelle, donc
//! tant que l'app n'est pas révoquée côté hub, elle doit toujours arriver à se
//! reconnecter ».
//!
//! 🔴 **Lue littéralement, cette phrase interdit la session.** Une session est
//! un objet qui expire — c'est sa définition. Tant que l'app s'authentifie par
//! une session, il existe un instant où elle redemande le mot de passe, et cet
//! instant tombe un jour en haut d'une échelle, parce que c'est là qu'on ouvre
//! l'app. D'où trois objets et trois durées de vie :
//!
//! | Objet | Vit où | Durée |
//! |---|---|---|
//! | identité d'appareil (`device_id` + secret) | trousseau du téléphone · `paired_devices` | illimitée — une révocation, et rien d'autre |
//! | jeton d'accès | mémoire vive de l'app | 15 min, sans conséquence visible |
//! | session web | navigateur | inchangée, elle ne concerne pas l'app |
//!
//! Le module vit à part de `web.rs` — qui fait 11 600 lignes — pour que deux
//! tâches parallèles n'aient pas à s'y partager le routeur.
//!
//! ## Ce que ce module tient, et qu'un relecteur ne doit pas « corriger »
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

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Extension, Json, Router,
};

use rusqlite::OptionalExtension;

use crate::db::{Db, DbError, DbResult, Outcome, Role};
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
    /// Le QR, en SVG, rendu **par le hub** — comme celui du second facteur
    /// (`totp.rs`). Le code ne traverse alors aucune dépendance de plus, et la
    /// page reste lisible sur un réseau fermé, sans CDN.
    ///
    /// ⚠️ Rempli par la route, pas par la couche de stockage : le QR porte
    /// l'adresse du hub et l'empreinte de son certificat, que la base ne
    /// connaît pas. La saisie manuelle reste le chemin de secours obligatoire —
    /// la caméra échoue, et le hub web est parfois ouvert sur le téléphone
    /// lui-même, auquel cas il n'y a aucun second écran à photographier.
    pub qr_svg: Option<String>,
    /// Empreinte SHA-256 du certificat du hub, à épingler par le téléphone.
    ///
    /// 🔴 Sans elle, iOS refuse un hub en certificat auto-signé. Elle arrive au
    /// téléphone par un canal que l'utilisateur contrôle physiquement — l'écran
    /// qu'il a sous les yeux — plutôt que par le réseau qu'on cherche
    /// justement à authentifier.
    ///
    /// ⚠️ `None` quand le hub sert en clair derrière un reverse proxy : le
    /// certificat que le téléphone verra est celui du proxy, et annoncer une
    /// empreinte la ferait comparer à un certificat qui n'est pas le sien.
    pub cert_fingerprint: Option<String>,
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
        // Le QR et l'empreinte sont posés par la route : la base ne connaît
        // ni l'adresse du hub ni son certificat.
        Ok(PairCode {
            code,
            expires_at,
            qr_svg: None,
            cert_fingerprint: None,
        })
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
pub(crate) fn identity_from_access_token(state: &AppState, token: &str) -> Option<Identity> {
    // ⚠️ Pas de `panic!` ni d'`unwrap` sur ce chemin : il est parcouru à CHAQUE
    // requête portant un en-tête `Authorization`, y compris celles qui n'ont
    // rien à voir avec l'appairage. Tout ce qui rate rend `None`.
    let plain = state.secrets.open_text(token).ok()?;
    let claim: AccessClaim = serde_json::from_str(&plain).ok()?;
    if claim.expires_at <= crate::db::now() {
        return None;
    }

    // 🔴 **`revoked_at` est relu ICI, à chaque requête, jamais mis en cache.**
    // Le jeton vit quinze minutes ; le porter en cache donnerait donc quinze
    // minutes d'accès résiduel à un téléphone qu'on vient de révoquer — sur le
    // seul mécanisme de sécurité du modèle, puisque rien d'autre n'expire.
    let device = state.db.get_paired_device(&claim.device_id).ok()?;
    if device.revoked_at.is_some() {
        return None;
    }

    // ⚠️ **Rôle et portée sont relus dans `identity_of`, jamais lus dans le
    // jeton** — qui ne les porte d'ailleurs pas. Un jeton qui porterait le rôle
    // survivrait quinze minutes à une rétrogradation, et une portée gelée
    // survivrait à sa révocation. `identity_of` rend aussi `None` quand le
    // compte a été désactivé.
    crate::web::identity_of(state, &device.username, Some(device.device_id))
}

/// Ce que le jeton d'accès transporte, et rien de plus.
///
/// ⚠️ **Ni rôle ni portée.** Ils sont relus à chaque requête : les mettre ici
/// les figerait pour quinze minutes, c'est-à-dire ferait survivre une portée
/// à sa révocation. Le jeton ne dit que « qui », jamais « ce qui est permis ».
#[derive(serde::Serialize, serde::Deserialize)]
struct AccessClaim {
    device_id: String,
    /// Redondant avec la ligne de l'appareil, et gardé quand même : il rend le
    /// sceau lisible en cas d'incident, sans jointure.
    username: String,
    expires_at: i64,
}

/// Émet un jeton d'accès de quinze minutes pour un appareil.
///
/// ⚠️ **Le secret d'appareil ne bouge PAS ici**, et c'est tout l'arbitrage du
/// § 22. La *refresh token rotation* du métier ferait tourner le secret durable
/// à chaque renouvellement pour détecter un rejeu ; ici, une réponse perdue —
/// une 4G qui coupe entre l'écriture du hub et sa réception — laisserait le
/// téléphone avec un secret mort et obligerait à réappairer. Ce scénario ne se
/// déclenche pas au hasard : il se déclenche là où le réseau est mauvais,
/// c'est-à-dire là où le technicien travaille, en haut d'une échelle.
///
/// 🔴 **Un relecteur « corrigera » ce point en croyant bien faire.** La
/// rotation à l'usage produit exactement la panne que l'appairage existe pour
/// ne jamais produire. La rotation est un geste explicite depuis le hub
/// (`POST /api/devices/{id}/rotate`), et le prix — plus aucune détection
/// automatique d'un secret copié — est payé sciemment : le seul détecteur
/// restant est humain, la dernière vue et la dernière adresse de l'écran
/// « Appareils ».
pub(crate) fn issue_access_token(state: &AppState, device: &PairedDevice) -> Result<String, String> {
    seal_access_token(
        state,
        &device.device_id,
        &device.username,
        crate::db::now() + ACCESS_TOKEN_TTL_SECS,
    )
}

/// Scelle une prétention d'accès à l'échéance demandée. Séparée d'
/// [`issue_access_token`] pour que les tests puissent en fabriquer une périmée
/// sans attendre un quart d'heure.
fn seal_access_token(
    state: &AppState,
    device_id: &str,
    username: &str,
    expires_at: i64,
) -> Result<String, String> {
    let claim = AccessClaim {
        device_id: device_id.to_string(),
        username: username.to_string(),
        expires_at,
    };
    let plain = serde_json::to_string(&claim).map_err(|e| e.to_string())?;
    state.secrets.seal_text(&plain)
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
        .route("/api/pair", post(pair))
        .route("/api/devices/token", post(issue_token));

    // Chacun agit sur SON compte, quel que soit son rôle (contrat § 19).
    // Ouvert à `viewer` : celui qui perd son téléphone à 22 h doit pouvoir le
    // révoquer seul.
    let les_siens = guarded(
        state,
        Role::Viewer,
        Router::new()
            .route("/api/me/pair-codes", post(create_pair_code))
            .route("/api/me/devices", get(list_my_devices))
            .route("/api/me/devices/{id}", patch(rename_my_device))
            .route("/api/me/devices/{id}/revoke", post(revoke_my_device)),
    );

    // Ceux de tous les comptes : un opérateur parti de l'entreprise ne se
    // révoque pas lui-même.
    let ceux_de_tous = guarded(
        state,
        Role::Admin,
        Router::new()
            .route("/api/devices", get(list_all_devices))
            .route("/api/devices/{id}/revoke", post(revoke_any_device))
            .route("/api/devices/{id}/rotate", post(rotate_any_device)),
    );

    ouvert.merge(les_siens).merge(ceux_de_tous)
}

// ── Le code d'appairage ────────────────────────────────────────────────────

/// Émet un code pour **son propre** compte.
///
/// ⚠️ Il ne crée aucun compte et n'élève aucun droit : l'appareil héritera du
/// rôle et de la portée de celui qui l'a émis, jamais plus. Un code volé chez
/// un `viewer` ne donne qu'un `viewer`.
async fn create_pair_code(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    headers: HeaderMap,
) -> Response {
    // Le clair des codes périmés s'en va avant qu'on en émette un de plus :
    // pas de minuterie à surveiller, et c'est fait avant toute lecture.
    if let Err(e) = state.db.purge_stale_pair_codes(crate::db::now()) {
        tracing::warn!("codes d'appairage périmés non purgés : {e}");
    }
    // ⚠️ Le code lui-même n'entre jamais au journal : c'est de quoi appairer
    // un téléphone.
    let code = match crate::web::audited(
        &state,
        Some(&actor.username),
        "device.pair_code",
        Some(&actor.username),
        state
            .db
            .create_pair_code(&actor.username, PAIR_CODE_TTL_SECS),
    ) {
        Ok(code) => code,
        Err(e) => return crate::web::error_response(e),
    };

    let hub = hub_url(&state, &headers);
    let uri = pairing_uri(&hub, &code.code, state.cert_fingerprint.as_deref());
    // Un QR illisible ne fait pas échouer l'émission : le code en clair est le
    // chemin de secours obligatoire, et il est déjà là.
    let qr_svg = match qr_svg(&uri) {
        Ok(svg) => Some(svg),
        Err(e) => {
            tracing::warn!("QR d'appairage non rendu : {e}");
            None
        }
    };
    crate::web::ok_json(serde_json::json!(PairCode {
        qr_svg,
        cert_fingerprint: state.cert_fingerprint.clone(),
        ..code
    }))
}

/// L'adresse du hub telle que le téléphone devra le joindre.
///
/// Le réglage `hub_public_url` gagne quand il est posé — c'est déjà l'adresse
/// que les sondes reçoivent. Sinon l'en-tête `Host` de la requête : celui qui
/// affiche le QR est sur la page du hub, l'adresse de sa barre d'URL est donc
/// celle qui marche depuis son réseau.
fn hub_url(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(url) = state.settings.hub_public_url() {
        return url;
    }
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let schema = if state.tls { "https" } else { "http" };
    format!("{schema}://{host}")
}

/// Ce que le QR transporte.
///
/// ⚠️ **Ni mot de passe, ni jeton de longue durée** — les deux raccourcis que
/// le § 22 refuse : un mot de passe affiché à l'écran ne se révoque pas
/// appareil par appareil, et un jeton durable ferait d'une image un identifiant
/// permanent. Le QR ne porte qu'un code de quinze minutes à usage unique.
fn pairing_uri(hub: &str, code: &str, fingerprint: Option<&str>) -> String {
    let enc = |s: &str| {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                other => format!("%{other:02X}"),
            })
            .collect::<String>()
    };
    let mut uri = format!("lanprobe://pair?hub={}&code={}", enc(hub), enc(code));
    // Absente quand le hub sert en clair : le téléphone n'a alors rien à
    // épingler, et un champ vide le ferait comparer à du néant.
    if let Some(fp) = fingerprint {
        uri.push_str(&format!("&fp={}", enc(fp)));
    }
    uri
}

/// QR du code d'appairage, en SVG.
///
/// Même motif que celui du second facteur (`totp.rs`) : rendu côté hub, sans
/// bibliothèque JavaScript. Le code ne traverse aucune dépendance de plus, et
/// la page reste lisible sur un réseau fermé, sans CDN — ce qui compte pour un
/// produit auto-hébergé.
fn qr_svg(uri: &str) -> Result<String, String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(uri.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render()
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

// ── L'appairage ────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct PairBody {
    code: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    app_version: Option<String>,
}

/// Échange un code contre une identité d'appareil.
///
/// ⚠️ **Non authentifiée par session** : c'est le code qui authentifie. Le
/// téléphone n'a rien d'autre à présenter, et lui demander un cookie
/// reviendrait à lui demander de se connecter — c'est-à-dire à taper un mot de
/// passe en haut d'une échelle.
async fn pair(State(state): State<AppState>, Json(body): Json<PairBody>) -> Response {
    let username = match state.db.consume_pair_code(body.code.trim(), crate::db::now()) {
        Ok(username) => username,
        Err(e) => {
            // 🔴 **Bruyamment.** Un code réessayé — parce qu'il a déjà servi,
            // ou parce que quelqu'un en essaie au hasard — est exactement ce
            // qu'un journal doit montrer. L'acteur est inconnu : personne ne
            // s'est authentifié.
            crate::web::audit(
                &state,
                None,
                "device.refused",
                Some("pair"),
                Outcome::Failure,
                Some(&e.to_string()),
            );
            return crate::web::error_response(e);
        }
    };

    // ⚠️ Le compte peut avoir été désactivé entre l'émission du code et son
    // usage. Le vérifier ici évite un appareil né mort, qui échouerait au
    // premier renouvellement sans que personne comprenne pourquoi.
    let Some(identity) = crate::web::identity_of(&state, &username, None) else {
        crate::web::audit(
            &state,
            Some(&username),
            "device.refused",
            Some("pair"),
            Outcome::Failure,
            Some(Refusal::AccountDisabled.as_str()),
        );
        return refusal_response(Refusal::AccountDisabled);
    };

    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("Appareil");
    let (device, secret) = match crate::web::audited(
        &state,
        Some(&username),
        "device.pair",
        Some(name),
        state.db.pair_device(
            &username,
            name,
            body.platform.as_deref(),
            body.app_version.as_deref(),
        ),
    ) {
        Ok(pair) => pair,
        Err(e) => return crate::web::error_response(e),
    };

    // 🔴 `device_secret` ne sort **qu'ici, une fois**. La base n'en garde que
    // le condensat : rien, nulle part, ne permettra de le relire.
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "device_id": device.device_id,
            "device_secret": secret,
            "user": identity.username,
            // Rendus pour que l'app sache quoi afficher dès le premier écran —
            // jamais pour qu'elle s'en serve comme d'une autorisation. Le hub
            // les relit à chaque requête, quoi que l'app en fasse.
            "role": identity.role.as_str(),
            "scope": match &identity.scope {
                crate::db::Scope::All => serde_json::Value::Null,
                crate::db::Scope::Sites(ids) => serde_json::json!(ids),
            },
            "cert_fingerprint": state.cert_fingerprint,
        })),
    )
        .into_response()
}

// ── Le jeton d'accès ───────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct TokenBody {
    device_id: String,
}

/// Échange l'identité durable contre un jeton de quinze minutes.
///
/// 🔴 **C'est ce qui répare un 401 tout seul, sans écran de connexion.** L'app
/// n'a jamais à redemander un mot de passe : tant que l'appareil n'est pas
/// révoqué, cette route lui rend un jeton.
///
/// ⚠️ **Le secret d'appareil ne tourne PAS ici.** Voir [`issue_access_token`] :
/// une rotation à l'usage perd le secret quand la réponse se perd, c'est-à-dire
/// là où le réseau est mauvais — là où le technicien travaille.
///
/// ⚠️ **Aucune ligne d'audit sur le succès.** Quatre-vingt-seize par jour et par
/// téléphone, dans un journal en ajout seul et sans purge, noieraient les
/// lignes qui comptent. C'est `last_seen` qui rend l'usage observable. Les
/// refus, eux, s'écrivent.
async fn issue_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    // ⚠️ L'adresse de la socket est lue dans les extensions plutôt qu'extraite
    // par `ConnectInfo` : cette route est celle qui ne doit JAMAIS échouer, et
    // un extracteur qui rejette parce que la couche `connect_info` manque
    // couperait tous les téléphones pour un champ d'affichage.
    extensions: axum::http::Extensions,
    Json(body): Json<TokenBody>,
) -> Response {
    let Some(secret) = bearer(&headers) else {
        return refusal_response(Refusal::Unknown);
    };
    let device = match state.db.authenticate_device(body.device_id.trim(), &secret) {
        Ok(device) => device,
        Err(refusal) => {
            crate::web::audit(
                &state,
                None,
                "device.refused",
                Some(body.device_id.trim()),
                Outcome::Failure,
                Some(refusal.as_str()),
            );
            return refusal_response(refusal);
        }
    };

    let token = match issue_access_token(&state, &device) {
        Ok(token) => token,
        Err(e) => {
            tracing::error!("jeton d'accès non scellé : {e}");
            return crate::web::error_response(crate::db::DbError::Internal(
                "jeton d'accès non émis".into(),
            ));
        }
    };

    // ⚠️ La dernière vue et la dernière adresse sont le SEUL détecteur d'un
    // secret copié, puisque le secret ne tourne pas à l'usage. Un échec
    // d'écriture ne fait pas échouer le renouvellement — couper l'accès pour un
    // relevé manqué serait exactement la panne interdite.
    let ip = extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|axum::extract::ConnectInfo(addr)| {
            crate::client_ip::resolve(
                addr.ip(),
                headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()),
                &state.settings.trusted_proxies(),
            )
            .to_string()
        });
    if let Err(e) = state
        .db
        .touch_paired_device(&device.device_id, crate::db::now(), ip.as_deref())
    {
        tracing::warn!("dernière vue de {} non notée : {e}", device.device_id);
    }

    crate::web::ok_json(serde_json::json!({
        "access_token": token,
        "expires_in": ACCESS_TOKEN_TTL_SECS,
    }))
}

/// Le refus, avec son motif.
///
/// 🔴 **Jamais « session expirée ».** Rien n'expire dans ce modèle : le mot
/// serait faux, et il ferait attendre une reconnexion qui n'arrivera pas. Trois
/// causes, trois conduites — et c'est sur `reason` que l'app décide de
/// s'arrêter ou de réappairer.
fn refusal_response(refusal: Refusal) -> Response {
    let message = match refusal {
        Refusal::Revoked => "cet appareil a été révoqué depuis le hub",
        Refusal::Unknown => {
            "ce hub ne connaît pas cet appareil — il a peut-être été restauré \
             depuis une sauvegarde antérieure à l'appairage"
        }
        Refusal::AccountDisabled => "le compte propriétaire de cet appareil a été désactivé",
    };
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": message, "reason": refusal.as_str() })),
    )
        .into_response()
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ── L'écran « Appareils » ──────────────────────────────────────────────────

fn devices_json(devices: Vec<PairedDevice>) -> Response {
    crate::web::ok_json(serde_json::json!({ "devices": devices }))
}

async fn list_my_devices(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
) -> Response {
    match state.db.list_paired_devices(Some(&actor.username)) {
        Ok(devices) => devices_json(devices),
        Err(e) => crate::web::error_response(e),
    }
}

#[derive(serde::Deserialize)]
struct DeviceFilter {
    #[serde(default)]
    user: Option<String>,
}

async fn list_all_devices(
    State(state): State<AppState>,
    Query(filter): Query<DeviceFilter>,
) -> Response {
    match state.db.list_paired_devices(filter.user.as_deref()) {
        Ok(devices) => devices_json(devices),
        Err(e) => crate::web::error_response(e),
    }
}

/// Le sien, ou rien.
///
/// ⚠️ **404 et non 403** quand l'appareil est celui d'un autre : « ça existe
/// mais pas pour vous » apprendrait déjà l'existence du téléphone de quelqu'un
/// d'autre. C'est la règle que le § 17 applique aux sites et aux sondes.
fn mine(state: &AppState, actor: &Identity, device_id: &str) -> Result<PairedDevice, Response> {
    match state.db.get_paired_device(device_id) {
        Ok(d) if d.username == actor.username => Ok(d),
        _ => Err(crate::web::error_response(crate::db::DbError::NotFound(
            "appareil inconnu".into(),
        ))),
    }
}

#[derive(serde::Deserialize)]
struct RenameBody {
    name: String,
}

async fn rename_my_device(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<RenameBody>,
) -> Response {
    let device = match mine(&state, &actor, &id) {
        Ok(device) => device,
        Err(refus) => return refus,
    };
    match crate::web::audited(
        &state,
        Some(&actor.username),
        "device.rename",
        Some(&device.device_id),
        state.db.rename_paired_device(&device.device_id, &body.name),
    ) {
        Ok(()) => crate::web::ok_json(serde_json::json!({ "ok": true })),
        Err(e) => crate::web::error_response(e),
    }
}

async fn revoke_my_device(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    let device = match mine(&state, &actor, &id) {
        Ok(device) => device,
        Err(refus) => return refus,
    };
    revoke(&state, &actor, &device.device_id)
}

async fn revoke_any_device(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    revoke(&state, &actor, &id)
}

/// Révoque — **ne supprime pas**. La ligne reste, parce que le journal d'audit
/// la nomme, et qu'une ligne d'audit qui pointe une cible disparue ne prouve
/// rien.
fn revoke(state: &AppState, actor: &Identity, device_id: &str) -> Response {
    match crate::web::audited(
        state,
        Some(&actor.username),
        "device.revoke",
        Some(device_id),
        state.db.revoke_paired_device(device_id),
    ) {
        Ok(()) => crate::web::ok_json(serde_json::json!({ "ok": true })),
        Err(e) => crate::web::error_response(e),
    }
}

/// Fait tourner le secret — **sur ce geste, et sur lui seul**.
///
/// 🔴 L'appareil devra être réappairé. C'est le prix assumé du § 22 : le seul
/// détecteur d'un secret copié étant humain — la dernière vue et la dernière
/// adresse — c'est un humain qui décide de couper.
async fn rotate_any_device(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    match crate::web::audited(
        &state,
        Some(&actor.username),
        "device.rotate",
        Some(&id),
        state.db.rotate_paired_device_secret(&id),
    ) {
        // Rendu une seule fois, comme à l'appairage.
        Ok(secret) => crate::web::ok_json(serde_json::json!({ "device_secret": secret })),
        Err(e) => crate::web::error_response(e),
    }
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

    /// Un hub complet, mais **sans Influx ni faux serveur** : l'appairage n'en
    /// touche aucun. Le harnais de `web.rs` monte un Influx factice à chaque
    /// test — ici ce serait un serveur HTTP par cas pour rien.
    pub(super) fn state_with_admin() -> AppState {
        use std::sync::Arc;
        let db = Arc::new(db_with_admin());
        let settings = crate::settings::Settings::new(db.clone());
        let secrets = crate::secrets::Secrets::ephemeral(db.clone()).unwrap();
        AppState {
            auth: Arc::new(crate::auth::Auth::new(db.clone())),
            influx: Arc::new(crate::influx::Influx::new(
                settings.clone(),
                "jeton-operateur-de-test".into(),
            )),
            notifier: crate::notify::Notifier::new(db.clone(), secrets.clone(), settings.clone()),
            ceremonies: Arc::new(crate::passkeys::Ceremonies::new()),
            db,
            settings,
            secrets,
            // Hub en clair derrière un proxy : aucun certificat du hub à
            // épingler, donc aucune empreinte à annoncer.
            tls: false,
            cert_fingerprint: None,
            restart_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            config_dir: std::env::temp_dir().join("lanprobe-pairing-tests"),
            backup_dir: std::env::temp_dir().join("lanprobe-pairing-tests"),
            influx_cli: "influx-absent-des-tests".into(),
        }
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

#[cfg(test)]
mod access_token_tests {
    use super::tests::state_with_admin;
    use super::*;
    use crate::db::{now, Role, Scope};

    /// Appaire un appareil et rend `(l'appareil, son jeton d'accès)`.
    fn paired(state: &AppState, username: &str) -> (PairedDevice, String) {
        let (device, _secret) = state
            .db
            .pair_device(username, "iPhone", Some("iOS 18.2"), Some("1.0.0"))
            .unwrap();
        let token = issue_access_token(state, &device).unwrap();
        (device, token)
    }

    #[test]
    fn an_access_token_names_the_device_it_came_from() {
        // L'audit dirait « admin », ce qui est vrai et trompeur le jour où le
        // téléphone est entre d'autres mains (contrat § 23).
        let state = state_with_admin();
        let (device, token) = paired(&state, "admin");

        let identity = identity_from_access_token(&state, &token).expect("jeton valide");

        assert_eq!(identity.username, "admin");
        assert_eq!(identity.device_id.as_deref(), Some(device.device_id.as_str()));
    }

    #[test]
    fn an_access_token_reads_the_current_role_and_scope_at_every_request() {
        // 🔴 Écrit avec un compte `Scope::All` — règle du § 17.3. Un test écrit
        // avec un compte restreint reste vert quoi qu'on casse sur ce chemin :
        // c'est l'angle mort qui a laissé le même défaut revenir trois fois.
        let state = state_with_admin();
        state
            .db
            .create_user("second-admin", "mot-de-passe-de-test", Role::Admin)
            .unwrap();
        let (_device, token) = paired(&state, "admin");

        let avant = identity_from_access_token(&state, &token).expect("jeton valide");
        assert_eq!(avant.role, Role::Admin);
        assert_eq!(avant.scope, Scope::All);

        // Rétrogradation du COMPTE : l'accès tient, ce qu'il permet rétrécit.
        // Le jeton n'a pas bougé — c'est bien la relecture qui décide.
        state
            .db
            .set_user_role("second-admin", "admin", Role::Viewer)
            .unwrap();
        let site = state.db.create_site("Durand").unwrap();
        state
            .db
            .set_user_sites("admin", &[site.site_id.clone()])
            .unwrap();

        let apres = identity_from_access_token(&state, &token).expect("l'accès tient");
        assert_eq!(apres.role, Role::Viewer, "le rôle du jeton a survécu");
        assert_eq!(
            apres.scope,
            Scope::Sites(vec![site.site_id]),
            "une portée figée survivrait à sa révocation"
        );
    }

    #[test]
    fn revoking_a_device_refuses_the_very_next_request() {
        // 🔴 Sans attendre aucune expiration. `revoked_at` mis en cache pour la
        // durée du jeton laisserait quinze minutes d'accès résiduel sur un
        // téléphone perdu — sur le seul mécanisme de sécurité du modèle.
        let state = state_with_admin();
        let (device, token) = paired(&state, "admin");
        assert!(identity_from_access_token(&state, &token).is_some());

        state.db.revoke_paired_device(&device.device_id).unwrap();

        assert!(
            identity_from_access_token(&state, &token).is_none(),
            "accès résiduel après révocation"
        );
    }

    #[test]
    fn a_disabled_account_takes_its_devices_with_it() {
        let state = state_with_admin();
        state
            .db
            .create_user("dupont", "mot-de-passe-de-test", Role::Operator)
            .unwrap();
        let (_device, token) = paired(&state, "dupont");
        assert!(identity_from_access_token(&state, &token).is_some());

        state.db.set_user_disabled("admin", "dupont", true).unwrap();

        assert!(identity_from_access_token(&state, &token).is_none());
    }

    #[test]
    fn an_expired_access_token_is_no_identity() {
        // Son expiration n'a aucune conséquence visible : l'app en redemande
        // un. C'est le seul objet des trois qui meurt du temps qui passe.
        let state = state_with_admin();
        let (device, _) = state.db.pair_device("admin", "iPhone", None, None).unwrap();
        let perime = seal_access_token(&state, &device.device_id, "admin", now() - 1).unwrap();

        assert!(identity_from_access_token(&state, &perime).is_none());
    }

    #[test]
    fn a_token_sealed_with_another_key_is_no_identity() {
        // Le jeton est SANS ÉTAT : rien ne le retrouve en base, c'est le sceau
        // seul qui le rend croyable. Un sceau d'un autre hub — ou un jeton
        // bricolé — ne doit donc rien ouvrir.
        let state = state_with_admin();
        let (_device, token) = paired(&state, "admin");

        let autre = state_with_admin();
        assert!(identity_from_access_token(&autre, &token).is_none());

        for bricolage in ["", "n-importe-quoi", "enc:v1:AAAA"] {
            assert!(
                identity_from_access_token(&state, bricolage).is_none(),
                "{bricolage} ouvre une session"
            );
        }
    }

    #[test]
    fn the_access_token_is_never_stored() {
        // ⚠️ Aucune table de jetons de quinze minutes : elle demanderait un
        // balayage de nettoyage que personne ne surveille, pour n'apporter
        // qu'une révocation immédiate que la révocation de l'appareil couvre
        // déjà.
        let state = state_with_admin();
        let (_device, token) = paired(&state, "admin");

        let tables: Vec<String> = {
            let conn = state.db.conn().lock().unwrap();
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get(0)).unwrap();
            rows.collect::<Result<Vec<String>, _>>().unwrap()
        };
        assert!(
            !tables.iter().any(|t| t.contains("access_token")),
            "{tables:?}"
        );

        // Et il ne traîne dans aucune colonne texte de la base.
        let conn = state.db.conn().lock().unwrap();
        for table in &tables {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM \"{table}\""))
                .unwrap();
            let colonnes = stmt.column_count();
            let mut rows = stmt.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for i in 0..colonnes {
                    if let Ok(v) = row.get::<_, String>(i) {
                        assert!(!v.contains(&token), "jeton retrouvé dans {table}");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod routes_tests {
    use super::tests::state_with_admin;
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    const MOT_DE_PASSE: &str = "mot-de-passe-de-test";

    struct Harness {
        state: AppState,
        router: axum::Router,
    }

    impl Harness {
        fn new() -> Self {
            let state = state_with_admin();
            let router = crate::web::build_router(state.clone());
            Self { state, router }
        }

        async fn call(&self, req: Request<Body>) -> (StatusCode, serde_json::Value) {
            let response = self.router.clone().oneshot(req).await.unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
            let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
            (status, body)
        }

        /// Un cookie de session pour un compte, sans passer par l'écran.
        fn session(&self, username: &str) -> String {
            let token = self.state.auth.start_session(username.to_string()).unwrap();
            format!("lanprobe_hub_session={token}")
        }

        async fn get(&self, path: &str, cookie: &str) -> (StatusCode, serde_json::Value) {
            self.call(
                Request::builder()
                    .uri(path)
                    .header(axum::http::header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
        }

        async fn post(
            &self,
            path: &str,
            cookie: &str,
            body: serde_json::Value,
        ) -> (StatusCode, serde_json::Value) {
            self.call(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(axum::http::header::COOKIE, cookie)
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
        }

        /// Une requête d'appareil : porteur, jamais de cookie.
        async fn bearer(&self, method: &str, path: &str, token: &str, body: serde_json::Value)
            -> (StatusCode, serde_json::Value)
        {
            self.call(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
        }

        /// Le parcours complet : un code, puis l'appairage, puis un jeton
        /// d'accès. Rend `(device_id, secret, access_token)`.
        async fn pair(&self, username: &str) -> (String, String, String) {
            let cookie = self.session(username);
            let (_, code) = self.post("/api/me/pair-codes", &cookie, serde_json::json!({})).await;
            let (status, paired) = self
                .call(
                    Request::builder()
                        .method("POST")
                        .uri("/api/pair")
                        .header(axum::http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({
                                "code": code["code"],
                                "name": "iPhone 15",
                                "platform": "iOS 18.2",
                                "app_version": "1.0.0",
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{paired}");
            let device_id = paired["device_id"].as_str().unwrap().to_string();
            let secret = paired["device_secret"].as_str().unwrap().to_string();
            let token = self.refresh(&device_id, &secret).await.1["access_token"]
                .as_str()
                .unwrap()
                .to_string();
            (device_id, secret, token)
        }

        async fn refresh(&self, device_id: &str, secret: &str) -> (StatusCode, serde_json::Value) {
            self.bearer(
                "POST",
                "/api/devices/token",
                secret,
                serde_json::json!({ "device_id": device_id }),
            )
            .await
        }

        fn audit_actions(&self) -> Vec<String> {
            self.state
                .db
                .list_audit(&crate::db::AuditFilter {
                    limit: 100,
                    before_id: None,
                    actor: None,
                    action: None,
                })
                .unwrap()
                .into_iter()
                .map(|e| e.action)
                .collect()
        }
    }

    #[tokio::test]
    async fn a_pair_code_comes_with_its_qr_and_the_fingerprint_to_pin() {
        // 🔴 Sans QR il n'y a que la saisie à la main, c'est-à-dire exactement
        // ce que le QR existe pour supprimer. Et sans empreinte, iOS refuse un
        // hub en certificat auto-signé : c'est le champ qui débloque le
        // parcours.
        let h = Harness::new();
        let cookie = h.session("admin");

        let (status, body) = h.post("/api/me/pair-codes", &cookie, serde_json::json!({})).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["code"].as_str().map(str::len), Some(9), "{body}");
        assert!(body["expires_at"].as_i64().unwrap() > crate::db::now());
        let qr = body["qr_svg"].as_str().expect("un QR est rendu par le hub");
        assert!(qr.contains("<svg"), "{qr}");
        // Le QR porte le code : c'est ce qui évite la saisie à la main.
        assert!(qr.len() > 200, "QR suspicieusement court");
        // Hub en clair derrière un proxy : aucun certificat DU HUB à épingler.
        // Annoncer une empreinte la ferait comparer à celle du proxy.
        assert!(body["cert_fingerprint"].is_null(), "{body}");
        assert!(h.audit_actions().contains(&"device.pair_code".to_string()));
    }

    #[tokio::test]
    async fn the_pair_code_never_leaves_the_account_that_issued_it() {
        // Un code volé chez un `viewer` ne donne qu'un `viewer` : l'appareil
        // hérite de celui qui l'a émis, jamais plus.
        let h = Harness::new();
        h.state
            .db
            .create_user("lecteur", MOT_DE_PASSE, Role::Viewer)
            .unwrap();
        let cookie = h.session("lecteur");
        let (_, code) = h.post("/api/me/pair-codes", &cookie, serde_json::json!({})).await;

        let (status, body) = h
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/api/pair")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "code": code["code"], "name": "iPhone" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await;

        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["user"], "lecteur");
        assert_eq!(body["role"], "viewer");
        // Le secret n'est rendu QU'ICI, une seule fois.
        assert!(body["device_secret"].as_str().is_some_and(|s| s.len() > 20));
        assert!(h.audit_actions().contains(&"device.pair".to_string()));
    }

    #[tokio::test]
    async fn a_pair_code_used_twice_is_refused_loudly_the_second_time() {
        // « Un second essai échouera — et c'est le signal. » Le refus laisse une
        // ligne : un code réessayé est ce qu'un journal doit montrer.
        let h = Harness::new();
        let cookie = h.session("admin");
        let (_, code) = h.post("/api/me/pair-codes", &cookie, serde_json::json!({})).await;
        let corps = serde_json::json!({ "code": code["code"], "name": "iPhone" });

        let premier = h
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/api/pair")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(corps.to_string()))
                    .unwrap(),
            )
            .await;
        assert_eq!(premier.0, StatusCode::CREATED);

        let second = h
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/api/pair")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(corps.to_string()))
                    .unwrap(),
            )
            .await;

        assert_eq!(second.0, StatusCode::UNAUTHORIZED, "{}", second.1);
        assert_eq!(
            h.state.db.list_paired_devices(None).unwrap().len(),
            1,
            "le second essai a créé un appareil"
        );
        assert!(
            h.audit_actions().contains(&"device.refused".to_string()),
            "un code réessayé doit laisser une ligne : {:?}",
            h.audit_actions()
        );
    }

    #[tokio::test]
    async fn a_device_trades_its_lasting_identity_for_a_short_token() {
        // 🔴 C'est ce qui répare un 401 tout seul, SANS écran de connexion.
        let h = Harness::new();
        let (device_id, secret, _) = h.pair("admin").await;

        let (status, body) = h.refresh(&device_id, &secret).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["expires_in"], ACCESS_TOKEN_TTL_SECS);
        let token = body["access_token"].as_str().unwrap();

        // Le jeton ouvre une route protégée, sans le moindre cookie.
        let (status, moi) = h
            .bearer("GET", "/api/me", token, serde_json::Value::Null)
            .await;
        assert_eq!(status, StatusCode::OK, "{moi}");
        assert_eq!(moi["username"], "admin");

        // Le renouvellement n'écrit AUCUNE ligne d'audit — quatre-vingt-seize
        // par jour et par téléphone noieraient celles qui comptent. C'est
        // `last_seen` qui rend l'usage observable, et elle seule.
        let vu = h.state.db.get_paired_device(&device_id).unwrap();
        assert!(vu.last_seen.is_some(), "le renouvellement note la dernière vue");
        assert!(
            !h.audit_actions().contains(&"device.token".to_string()),
            "{:?}",
            h.audit_actions()
        );

        // ⚠️ Et le secret durable n'a PAS bougé : il resert au renouvellement
        // suivant. Une rotation à l'usage perdrait le secret quand la réponse
        // se perd, là où le réseau est mauvais.
        assert_eq!(h.refresh(&device_id, &secret).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn revoking_a_device_refuses_the_next_request_without_waiting() {
        // 🔴 Sans attendre aucune expiration : le jeton déjà émis vaut encore
        // quatorze minutes, et il ne doit plus rien ouvrir.
        let h = Harness::new();
        let (device_id, secret, token) = h.pair("admin").await;
        assert_eq!(
            h.bearer("GET", "/api/me", &token, serde_json::Value::Null).await.0,
            StatusCode::OK
        );

        let cookie = h.session("admin");
        let (status, _) = h
            .post(
                &format!("/api/me/devices/{device_id}/revoke"),
                &cookie,
                serde_json::json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);

        assert_eq!(
            h.bearer("GET", "/api/me", &token, serde_json::Value::Null).await.0,
            StatusCode::UNAUTHORIZED,
            "accès résiduel après révocation"
        );
        let (status, refus) = h.refresh(&device_id, &secret).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(refus["reason"], "revoked", "{refus}");
        assert!(h.audit_actions().contains(&"device.revoke".to_string()));
        assert!(h.audit_actions().contains(&"device.refused".to_string()));
    }

    #[tokio::test]
    async fn the_refusal_carries_a_reason_and_never_the_word_session() {
        // ⚠️ Trois causes, trois conduites. « Session expirée » ferait attendre
        // une reconnexion qui n'arrivera jamais : rien n'expire dans ce modèle,
        // le mot serait faux.
        let h = Harness::new();
        h.state
            .db
            .create_user("dupont", MOT_DE_PASSE, Role::Operator)
            .unwrap();
        let (device_id, secret, _) = h.pair("dupont").await;

        // 1. Inconnu — le hub a peut-être été restauré depuis une sauvegarde
        //    antérieure à l'appairage. Elle réappaire, légitimement.
        let (status, inconnu) = h.refresh("appareil-jamais-vu", &secret).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(inconnu["reason"], "unknown", "{inconnu}");

        // 2. Compte désactivé — rien de ce qu'elle fera sur ce téléphone n'y
        //    changera quoi que ce soit.
        h.state.db.set_user_disabled("admin", "dupont", true).unwrap();
        let (status, desactive) = h.refresh(&device_id, &secret).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(desactive["reason"], "account_disabled", "{desactive}");

        for corps in [&inconnu, &desactive] {
            let rendu = corps.to_string().to_lowercase();
            assert!(!rendu.contains("session"), "{rendu}");
            assert!(!rendu.contains("expir"), "{rendu}");
        }
    }

    #[tokio::test]
    async fn a_device_inherits_the_role_of_its_account_and_loses_it_at_the_next_request() {
        // 🔴 Écrit avec un compte `Scope::All` — règle du § 17.3. Un test écrit
        // avec un compte restreint reste vert quoi qu'on casse : c'est l'angle
        // mort qui a fait revenir le même défaut trois fois.
        let h = Harness::new();
        h.state
            .db
            .create_user("second-admin", MOT_DE_PASSE, Role::Admin)
            .unwrap();
        let (_device_id, _secret, token) = h.pair("admin").await;

        // Le téléphone d'un administrateur atteint une route d'administration.
        let (status, comptes) = h
            .bearer("GET", "/api/users", &token, serde_json::Value::Null)
            .await;
        assert_eq!(status, StatusCode::OK, "{comptes}");

        // Rétrogradation du COMPTE. L'accès tient, ce qu'il permet rétrécit —
        // et à la requête SUIVANTE, pas au prochain renouvellement.
        h.state
            .db
            .set_user_role("second-admin", "admin", Role::Viewer)
            .unwrap();

        assert_eq!(
            h.bearer("GET", "/api/users", &token, serde_json::Value::Null).await.0,
            StatusCode::FORBIDDEN,
            "le rôle du jeton a survécu à la rétrogradation"
        );
        // L'accès, lui, n'est pas coupé : une rétrogradation n'est pas une
        // révocation.
        assert_eq!(
            h.bearer("GET", "/api/me", &token, serde_json::Value::Null).await.0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn everyone_manages_his_own_phones_and_no_one_else_s() {
        // Ouvert à `viewer` : celui qui perd son téléphone à 22 h doit pouvoir
        // le révoquer seul, sans réveiller un administrateur.
        let h = Harness::new();
        h.state
            .db
            .create_user("lecteur", MOT_DE_PASSE, Role::Viewer)
            .unwrap();
        let (sien, _, _) = h.pair("lecteur").await;
        let (celui_d_admin, _, _) = h.pair("admin").await;

        let cookie = h.session("lecteur");
        let (status, liste) = h.get("/api/me/devices", &cookie).await;
        assert_eq!(status, StatusCode::OK, "{liste}");
        let devices = liste["devices"].as_array().expect("une enveloppe `devices`");
        assert_eq!(devices.len(), 1, "{liste}");
        assert_eq!(devices[0]["device_id"], sien);

        // ⚠️ 404 et non 403 : « ça existe mais pas pour vous » apprendrait déjà
        // l'existence de l'appareil d'un autre.
        let (status, _) = h
            .post(
                &format!("/api/me/devices/{celui_d_admin}/revoke"),
                &cookie,
                serde_json::json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(
            h.state
                .db
                .get_paired_device(&celui_d_admin)
                .unwrap()
                .revoked_at
                .is_none()
        );

        // Le sien, en revanche, il le renomme et le révoque.
        let (status, _) = h
            .call(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/me/devices/{sien}"))
                    .header(axum::http::header::COOKIE, &cookie)
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "name": "iPhone — atelier" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            h.state.db.get_paired_device(&sien).unwrap().name,
            "iPhone — atelier"
        );
    }

    #[tokio::test]
    async fn an_admin_reaches_the_devices_of_every_account() {
        // Un opérateur parti de l'entreprise ne se révoque pas lui-même.
        let h = Harness::new();
        h.state
            .db
            .create_user("parti", MOT_DE_PASSE, Role::Operator)
            .unwrap();
        let (le_sien, secret, _) = h.pair("parti").await;
        let cookie = h.session("admin");

        let (status, tous) = h.get("/api/devices", &cookie).await;
        assert_eq!(status, StatusCode::OK, "{tous}");
        assert_eq!(tous["devices"].as_array().unwrap().len(), 1);

        // La rotation est un GESTE, jamais un effet de bord du renouvellement.
        let (status, tourne) = h
            .post(
                &format!("/api/devices/{le_sien}/rotate"),
                &cookie,
                serde_json::json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{tourne}");
        assert_ne!(tourne["device_secret"].as_str().unwrap(), secret);
        assert_eq!(
            h.refresh(&le_sien, &secret).await.0,
            StatusCode::UNAUTHORIZED,
            "l'ancien secret ouvre encore"
        );

        let (status, _) = h
            .post(
                &format!("/api/devices/{le_sien}/revoke"),
                &cookie,
                serde_json::json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        // La ligne reste : aucune route ne supprime un appareil.
        assert!(
            h.state
                .db
                .get_paired_device(&le_sien)
                .unwrap()
                .revoked_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_viewer_cannot_reach_the_devices_of_every_account() {
        let h = Harness::new();
        h.state
            .db
            .create_user("lecteur", MOT_DE_PASSE, Role::Viewer)
            .unwrap();
        let cookie = h.session("lecteur");

        assert_eq!(h.get("/api/devices", &cookie).await.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_open_routes_need_no_session_at_all() {
        // ⚠️ `POST /api/devices/token` vit hors de `/api/me/*` : la ranger
        // derrière la garde de session la mettrait derrière ce qu'elle existe
        // précisément pour remplacer.
        let h = Harness::new();
        let (device_id, secret, _) = h.pair("admin").await;

        let (status, body) = h.refresh(&device_id, &secret).await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["access_token"].as_str().is_some());
    }
}
