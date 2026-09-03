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

use crate::db::{Db, DbResult, Role};
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

/// Le stockage de l'appairage, hors de `db.rs` — qui fait déjà 5 200 lignes et
/// qu'aucune tâche parallèle ne peut se partager.
impl Db {
    /// Émet un code court pour un compte. Usage unique, 15 minutes.
    pub(crate) fn create_pair_code(&self, _username: &str, _ttl_secs: i64) -> DbResult<PairCode> {
        todo!("tâche « appairage » : calque de create_enroll_code, table pair_codes")
    }

    /// Consomme un code et rend le compte auquel il appartient.
    ///
    /// ⚠️ **Consommé au premier succès**, dans la même transaction que sa
    /// lecture : deux téléphones qui présentent le même code au même instant
    /// doivent laisser exactement un gagnant.
    pub(crate) fn consume_pair_code(&self, _code: &str, _now: i64) -> DbResult<String> {
        todo!("tâche « appairage » : calque de consume_enroll_code")
    }

    /// Enregistre un appareil et rend `(la ligne, le secret en clair)`.
    ///
    /// ⚠️ Le secret n'est rendu **qu'ici, une fois**. La base n'en garde que le
    /// condensat argon2id, comme pour un jeton de sonde.
    pub(crate) fn pair_device(
        &self,
        _username: &str,
        _name: &str,
        _platform: Option<&str>,
        _app_version: Option<&str>,
    ) -> DbResult<(PairedDevice, String)> {
        todo!("tâche « appairage » : calque de enroll_probe")
    }

    /// Vérifie un secret d'appareil.
    ///
    /// ⚠️ **Le compte désactivé se vérifie ICI**, pas seulement à la connexion
    /// web : un appareil qui survivrait à la désactivation de son propriétaire
    /// serait un trou dans la seule procédure de départ d'un salarié.
    pub(crate) fn authenticate_device(
        &self,
        _device_id: &str,
        _secret: &str,
    ) -> Result<PairedDevice, Refusal> {
        todo!("tâche « appairage » : trois motifs de refus distincts")
    }

    /// Les appareils d'un compte, ou de tous quand `username` est `None`.
    /// Les révoqués **en font partie** : la ligne reste, l'audit la nomme.
    pub(crate) fn list_paired_devices(
        &self,
        _username: Option<&str>,
    ) -> DbResult<Vec<PairedDevice>> {
        todo!("tâche « appairage »")
    }

    /// « iPhone » et « iPhone » ne se distinguent pas, et on ne révoque pas au
    /// hasard dans une liste.
    pub(crate) fn rename_paired_device(&self, _device_id: &str, _name: &str) -> DbResult<()> {
        todo!("tâche « appairage »")
    }

    /// Révoque — **ne supprime pas**. `revoked_at` daté, la ligne reste.
    pub(crate) fn revoke_paired_device(&self, _device_id: &str) -> DbResult<()> {
        todo!("tâche « appairage »")
    }

    /// Fait tourner le secret sur un geste explicite, et rend le nouveau.
    ///
    /// ⚠️ **Jamais appelée depuis le renouvellement de jeton.** C'est tout
    /// l'arbitrage du § 22 : une rotation à l'usage perd le secret quand la
    /// réponse se perd, là où le réseau est mauvais.
    pub(crate) fn rotate_paired_device_secret(&self, _device_id: &str) -> DbResult<String> {
        todo!("tâche « appairage » : calque de rotate_probe_token")
    }

    /// Note la dernière vue et la dernière adresse.
    ///
    /// ⚠️ Écrite au renouvellement du jeton, qui ne laisse **aucune** ligne
    /// d'audit : c'est cette colonne, et elle seule, qui rend l'usage d'un
    /// appareil observable.
    pub(crate) fn touch_paired_device(
        &self,
        _device_id: &str,
        _now: i64,
        _ip: Option<&str>,
    ) -> DbResult<()> {
        todo!("tâche « appairage »")
    }
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
