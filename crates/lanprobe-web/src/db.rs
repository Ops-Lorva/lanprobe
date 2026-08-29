//! Stockage SQLite du hub : comptes, sites, sondes.
//!
//! Les migrations sont versionnées par `PRAGMA user_version` et rejouées à
//! chaque ouverture. Pas de framework : le schéma tient en trois tables, et
//! une dépendance de plus à comprendre pour l'auto-hébergeur qui débogue son
//! volume ne se justifie pas.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension};

/// Version cible du schéma. Toute migration ajoutée doit incrémenter cette
/// constante **et** être ajoutée à `MIGRATIONS` — jamais retoucher une
/// migration déjà livrée : une base en production l'a déjà appliquée.
pub const SCHEMA_VERSION: i64 = 10;

/// Migrations dans l'ordre. L'index `n` fait passer de la version `n` à `n+1`.
const MIGRATIONS: &[&str] = &[
    // v0 → v1 : schéma initial du contrat.
    r#"
    CREATE TABLE users (
      id            INTEGER PRIMARY KEY,
      username      TEXT NOT NULL UNIQUE,
      password_hash TEXT NOT NULL,
      role          TEXT NOT NULL DEFAULT 'admin',
      created_at    INTEGER NOT NULL
    );

    CREATE TABLE sites (
      site_id    TEXT PRIMARY KEY,
      name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
      created_at INTEGER NOT NULL
    );

    CREATE TABLE probes (
      probe_id        TEXT PRIMARY KEY,
      site_id         TEXT NOT NULL REFERENCES sites(site_id),
      name            TEXT NOT NULL,
      token_hash      TEXT NOT NULL,
      platform        TEXT,
      version         TEXT,
      last_seen       INTEGER,
      buffered_points INTEGER DEFAULT 0,
      created_at      INTEGER NOT NULL,
      revoked_at      INTEGER
    );

    -- Le nom est unique DANS un site, pas globalement : deux clients ont
    -- chacun le droit d'avoir leur « Paris ».
    CREATE UNIQUE INDEX probes_site_name ON probes(site_id, name COLLATE NOCASE);
    "#,
    // v1 → v2 : réglages modifiables depuis l'interface. Ils priment sur
    // l'environnement — une valeur qui vit dans un `.env` demande d'éditer un
    // fichier, de redémarrer le conteneur, et se perd à la migration suivante.
    r#"
    CREATE TABLE settings (
      key        TEXT PRIMARY KEY,
      value      TEXT NOT NULL,
      updated_at INTEGER NOT NULL
    );
    "#,
    // v2 → v3 : codes d'enrôlement, rotation des jetons d'écriture, et suivi
    // des jetons de lecture émis pour Grafana.
    r#"
    CREATE TABLE enroll_codes (
      code_hash   TEXT PRIMARY KEY,
      site_id     TEXT NOT NULL REFERENCES sites(site_id),
      -- Renseigné pour un code de RÉ-enrôlement : il répare une sonde
      -- existante au lieu d'en créer une nouvelle.
      probe_id    TEXT REFERENCES probes(probe_id),
      created_at  INTEGER NOT NULL,
      expires_at  INTEGER NOT NULL,
      consumed_at INTEGER
    );

    ALTER TABLE probes ADD COLUMN influx_auth_id TEXT;
    ALTER TABLE probes ADD COLUMN influx_token_version INTEGER NOT NULL DEFAULT 1;
    ALTER TABLE probes ADD COLUMN pending_influx_auth_id TEXT;
    ALTER TABLE probes ADD COLUMN pending_influx_token TEXT;
    ALTER TABLE probes ADD COLUMN pending_influx_token_version INTEGER;

    -- On ne garde QUE l'identifiant d'autorisation : la valeur du jeton de
    -- lecture est affichée une fois et n'est jamais conservée.
    CREATE TABLE influx_read_tokens (
      auth_id     TEXT PRIMARY KEY,
      description TEXT NOT NULL,
      created_at  INTEGER NOT NULL,
      revoked_at  INTEGER
    );
    "#,
    // v3 → v4 : le code d'enrôlement en clair, le temps de sa validité.
    //
    // Compromis décidé par le propriétaire, en connaissance du risque : un code
    // perdu au rechargement de la page obligeait à en regénérer un. Il vit donc
    // en clair **pendant ses quinze minutes**, et est effacé dès qu'il est
    // consommé ou expiré — le hachage reste la vérité pour l'authentification.
    r#"
    ALTER TABLE enroll_codes ADD COLUMN code_plain TEXT;
    "#,
    // v4 → v5 : la désactivation d'un compte.
    //
    // Il n'y a pas de suppression de compte, et il n'y en aura pas : les
    // lignes du journal d'audit nomment leur acteur, et un compte effacé
    // emporterait la traçabilité de tout ce qu'il a fait. On désactive.
    r#"
    ALTER TABLE users ADD COLUMN disabled_at INTEGER;
    "#,
    // v5 → v6 : le journal d'audit.
    //
    // **Ajout seul.** Aucune requête de ce fichier n'en supprime une ligne, et
    // aucune route n'y mène : un journal qu'on peut nettoyer ne prouve rien.
    // Pas de purge par ancienneté non plus — quelques dizaines d'octets par
    // geste d'opérateur ne justifient pas d'inventer un chemin d'effacement.
    //
    // `actor` est nullable : une tentative de connexion sur un compte
    // inexistant n'a pas d'acteur connu, et la laisser tomber pour cette
    // raison effacerait précisément les lignes qui comptent.
    r#"
    CREATE TABLE audit_log (
      id      INTEGER PRIMARY KEY AUTOINCREMENT,
      at      INTEGER NOT NULL,
      actor   TEXT,
      action  TEXT NOT NULL,
      target  TEXT,
      outcome TEXT NOT NULL,
      detail  TEXT
    );

    CREATE INDEX audit_log_at     ON audit_log(at DESC);
    CREATE INDEX audit_log_actor  ON audit_log(actor);
    CREATE INDEX audit_log_action ON audit_log(action);
    "#,
    // v6 → v7 : les notifications.
    //
    // `sealed_secrets` tient ce qui ne peut pas aller dans `settings` : mot de
    // passe SMTP et URL de webhook, qui est elle-même un secret porteur. La
    // section 7 du contrat pose que `settings` ne contient aucun secret et que
    // `GET /api/settings` peut donc être rendue telle quelle — les y mettre
    // aurait cassé les deux. Les valeurs sont scellées en AES-256-GCM
    // (`enc:v1:`), et une valeur vide veut dire « non configuré » : rien ne se
    // supprime, on désactive.
    //
    // `notify_subscriptions` porte l'activation par site, héritée par ses
    // sondes, avec exception possible par sonde — `enabled` à NULL sur une
    // sonde signifie « hérite du site ». Sans cet héritage on recocherait
    // vingt cases à chaque nouveau client.
    //
    // `notify_states` retient l'état annoncé pour chaque sonde. C'est lui qui
    // fait qu'on notifie des **transitions** et non des états : sans mémoire,
    // un portable qu'on referme alerterait à chaque passage de la boucle, et
    // trois alertes inutiles suffisent pour que plus personne ne les lise.
    r#"
    CREATE TABLE sealed_secrets (
      key        TEXT PRIMARY KEY,
      value      TEXT NOT NULL,
      updated_at INTEGER NOT NULL
    );

    CREATE TABLE notify_subscriptions (
      scope    TEXT NOT NULL,          -- 'site' | 'probe'
      scope_id TEXT NOT NULL,
      enabled  INTEGER,                -- NULL = hérite
      PRIMARY KEY (scope, scope_id)
    );

    CREATE TABLE notify_states (
      probe_id TEXT PRIMARY KEY REFERENCES probes(probe_id),
      state    TEXT NOT NULL,          -- 'up' | 'down'
      since    INTEGER NOT NULL
    );
    "#,
    // v7 → v8 : ce que la sonde raconte d'elle-même (contrat, section 15).
    //
    // **Ajout de colonnes seul** — rien n'est réécrit, une base peuplée passe
    // sans y toucher, et les sondes déjà enrôlées remplissent ces colonnes à
    // leur premier battement.
    //
    // Aucune de ces valeurs n'est jamais remise à NULL : une sonde qui perd
    // son accès internet bat sans IP publique, et afficher un tiret parce
    // qu'elle ne répond plus effacerait l'information au moment exact où elle
    // sert — quand un site tombe, savoir quelle adresse il avait est le
    // premier élément de diagnostic.
    //
    // `public_ip_at` date le moment où l'adresse **a changé**, pas le dernier
    // battement : « cette adresse depuis mardi » se lit, « vue il y a une
    // minute » ne dit rien de plus que `last_seen`.
    //
    // `local_ips` est un tableau JSON : une machine multi-domiciliée en a
    // plusieurs, et une colonne par adresse serait un plafond arbitraire.
    r#"
    ALTER TABLE probes ADD COLUMN public_ip    TEXT;
    ALTER TABLE probes ADD COLUMN public_ip_at INTEGER;
    ALTER TABLE probes ADD COLUMN interface    TEXT;
    ALTER TABLE probes ADD COLUMN local_ips    TEXT;
    ALTER TABLE probes ADD COLUMN gateway      TEXT;
    "#,
    // v8 → v9 : file de commandes du hub vers la sonde (contrat § 14).
    //
    // ⚠️ Le hub ne joint JAMAIS la sonde : c'est la sonde qui bat. La file
    // voyage donc dans la réponse au battement de cœur, et l'accusé revient
    // au battement suivant. D'où `delivered_count` : une commande remise trois
    // fois sans accusé est déclarée échouée, pas rejouée indéfiniment — un
    // scan qui fait planter la sonde ne doit pas la faire replanter à chaque
    // redémarrage.
    //
    // `created_by` porte le nom du compte : une commande est une action sur le
    // réseau d'un client, on doit pouvoir dire qui l'a lancée même des mois
    // après, y compris si le compte a depuis été désactivé.
    r#"
    CREATE TABLE probe_commands (
      id              INTEGER PRIMARY KEY AUTOINCREMENT,
      probe_id        TEXT    NOT NULL REFERENCES probes(probe_id) ON DELETE CASCADE,
      kind            TEXT    NOT NULL,
      args            TEXT    NOT NULL DEFAULT '{}',
      state           TEXT    NOT NULL DEFAULT 'pending',
      created_at      INTEGER NOT NULL,
      created_by      TEXT,
      delivered_count INTEGER NOT NULL DEFAULT 0,
      settled_at      INTEGER,
      error           TEXT
    );
    CREATE INDEX idx_probe_commands_pending
        ON probe_commands(probe_id, state, id);
    "#,
    // v9 → v10 : état internet vu par la sonde, remonté au battement.
    //
    // ⚠️ Sans lui, le parc affichait « En ligne » pour une sonde dont le lien
    // mesuré était mort : le battement passe, donc le voyant est vert, et rien
    // à l'écran ne dit que plus aucune mesure n'aboutit. La sonde le sait —
    // elle teste internet en permanence — il suffisait de le lui demander.
    r#"
    ALTER TABLE probes ADD COLUMN internet_state    TEXT;
    ALTER TABLE probes ADD COLUMN internet_state_at INTEGER;
    "#,
];

/// Remises d'une même commande avant de la déclarer échouée.
///
/// ⚠️ Trois, pas l'infini. Une commande qui fait planter la sonde reviendrait
/// sinon à chaque redémarrage, et la sonde ne se relèverait jamais.
pub const MAX_DELIVERIES: i64 = 3;

/// Commande telle qu'elle part vers la sonde.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingCommand {
    pub id: i64,
    pub kind: String,
    pub args: serde_json::Value,
}

/// Commande telle qu'elle est montrée dans l'interface.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommandRow {
    pub id: i64,
    pub kind: String,
    pub args: serde_json::Value,
    /// `pending` | `done` | `failed`.
    pub state: String,
    pub created_at: i64,
    pub created_by: Option<String>,
    pub delivered_count: i64,
    pub settled_at: Option<i64>,
    pub error: Option<String>,
}

/// Erreurs remontées jusqu'à la couche HTTP, qui les traduit en codes. Un
/// `String` unique obligerait les handlers à deviner le code d'après le
/// texte du message — ils se tromperaient.
#[derive(Debug)]
pub enum DbError {
    Conflict(String),
    NotFound(String),
    /// L'appelant n'est pas authentifié, ou ses identifiants sont faux.
    Unauthorized(String),
    /// L'appelant est bien authentifié, mais son rôle ne suffit pas. À
    /// distinguer d'`Unauthorized` : répondre 401 le renverrait à l'écran de
    /// connexion, où se reconnecter ne changerait rien.
    Forbidden(String),
    Internal(String),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Conflict(m)
            | DbError::NotFound(m)
            | DbError::Unauthorized(m)
            | DbError::Forbidden(m)
            | DbError::Internal(m) => write!(f, "{m}"),
        }
    }
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Internal(e.to_string())
    }
}

pub type DbResult<T> = Result<T, DbError>;

/// Rôle d'un compte. **L'ordre de déclaration est l'ordre des privilèges** :
/// c'est lui qui permet d'écrire « au moins operator » en une comparaison, au
/// lieu d'énumérer les rôles autorisés à chaque route — une énumération qu'on
/// oublie de compléter le jour où un rôle s'ajoute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Consulte le parc, ne le modifie pas. C'est le rôle qui manque le plus
    /// en entreprise : montrer les sondes à un client sans qu'un clic en
    /// révoque une.
    Viewer,
    /// Enrôle, renomme, déplace, fait tourner les clés.
    Operator,
    /// Tout, y compris les comptes, la rétention et le journal d'audit.
    Admin,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Operator => "operator",
            Role::Admin => "admin",
        }
    }

    /// Lecture stricte, pour une valeur qui vient d'une requête : un rôle mal
    /// orthographié doit être refusé, pas interprété.
    pub fn parse(value: &str) -> Option<Role> {
        match value.trim().to_ascii_lowercase().as_str() {
            "viewer" => Some(Role::Viewer),
            "operator" => Some(Role::Operator),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }

    /// Lecture d'une valeur **déjà en base**. Une valeur inconnue — écriture à
    /// la main, rétrogradation de version — retombe sur le moindre privilège :
    /// une base illisible ne doit jamais ouvrir plus de portes qu'elle n'en
    /// nomme.
    fn from_stored(value: &str) -> Role {
        Role::parse(value).unwrap_or(Role::Viewer)
    }
}

impl serde::Serialize for Role {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// Résultat d'une action journalisée. Les échecs comptent autant que les
/// réussites : un journal qui n'enregistre que les succès ne montre jamais
/// une tentative d'intrusion, il montre celui qui a fini par entrer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::Failure => "failure",
        }
    }

    fn from_stored(value: &str) -> Outcome {
        match value {
            "success" => Outcome::Success,
            _ => Outcome::Failure,
        }
    }
}

impl serde::Serialize for Outcome {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// Une ligne du journal d'audit. **Jamais de secret ici** : ni jeton, ni mot
/// de passe, ni code d'enrôlement. `detail` est une phrase courte destinée à
/// être lue, pas un vidage de la requête.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub at: i64,
    /// `None` pour une tentative anonyme — connexion sur un compte inconnu.
    pub actor: Option<String>,
    pub action: String,
    pub target: Option<String>,
    pub outcome: Outcome,
    pub detail: Option<String>,
}

/// Plafond du nombre de lignes qu'une requête peut ramener. Le journal se lit
/// par pages : une requête qui le viderait d'un coup ferait travailler le hub
/// pour une page que personne ne lit jusqu'au bout.
pub const AUDIT_MAX_LIMIT: i64 = 500;
pub const AUDIT_DEFAULT_LIMIT: i64 = 100;

#[derive(Debug, Clone)]
pub struct AuditFilter {
    pub limit: i64,
    /// Pagination à rebours : la page suivante est « plus ancienne que cet
    /// identifiant ». Un décalage par `OFFSET` glisserait à chaque ligne
    /// ajoutée pendant la lecture, et rejouerait des lignes déjà vues.
    pub before_id: Option<i64>,
    pub actor: Option<String>,
    pub action: Option<String>,
}

impl Default for AuditFilter {
    fn default() -> Self {
        Self {
            limit: AUDIT_DEFAULT_LIMIT,
            before_id: None,
            actor: None,
            action: None,
        }
    }
}

/// État d'alerte annoncé pour une sonde. On notifie les **transitions** de
/// cet état, jamais l'état lui-même : « Paris est passée hors ligne », une
/// fois, pas un rappel par minute tant qu'elle l'est.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertState {
    Up,
    Down,
}

impl AlertState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AlertState::Up => "up",
            AlertState::Down => "down",
        }
    }

    fn from_stored(value: &str) -> AlertState {
        match value {
            "down" => AlertState::Down,
            _ => AlertState::Up,
        }
    }
}

/// Abonnement aux notifications, par site ou par sonde.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NotifySubscription {
    /// `site` ou `probe`.
    pub scope: String,
    pub scope_id: String,
    /// `None` sur une sonde = elle suit son site. Sur un site, `None` équivaut
    /// à « pas d'alerte » — c'est le défaut.
    pub enabled: Option<bool>,
}

/// Un compte, tel que l'interface l'affiche./// Un compte, tel que l'interface l'affiche./// Un compte, tel que l'interface l'affiche. **Le hash du mot de passe n'en
/// fait pas partie** : aucune sérialisation ne l'expose.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UserRecord {
    pub username: String,
    pub role: Role,
    pub created_at: i64,
    /// Renseigné quand le compte est désactivé. On ne supprime pas un compte.
    pub disabled_at: Option<i64>,
}

/// Longueur minimale d'un mot de passe de compte, vérifiée en base et pas
/// seulement dans le handler : une règle qui ne vit que dans la couche HTTP
/// se contourne par le premier appelant qui l'ignore.
const MIN_PASSWORD_LEN: usize = 8;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Site {
    pub site_id: String,
    pub name: String,
    pub created_at: i64,
    pub probe_count: i64,
}

/// `probe_count` ne compte que les sondes actives : une sonde révoquée reste
/// en base pour l'historique, mais l'annoncer dans un décompte de parc
/// donnerait une taille de parc fausse.
const SITE_COLUMNS: &str = "s.site_id, s.name, s.created_at, \
                            (SELECT COUNT(*) FROM probes p \
                              WHERE p.site_id = s.site_id AND p.revoked_at IS NULL)";

fn site_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Site> {
    Ok(Site {
        site_id: r.get(0)?,
        name: r.get(1)?,
        created_at: r.get(2)?,
        probe_count: r.get(3)?,
    })
}

/// Ligne `probes` enrichie du nom de son site. Le `token_hash` ne sort jamais
/// de ce crate : aucune sérialisation ne l'expose.
#[derive(Debug, Clone)]
pub struct ProbeRecord {
    pub probe_id: String,
    pub site_id: String,
    pub site_name: String,
    pub name: String,
    pub token_hash: String,
    pub platform: Option<String>,
    pub version: Option<String>,
    pub last_seen: Option<i64>,
    pub buffered_points: i64,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
    /// Autorisation Influx actuellement valide pour cette sonde.
    pub influx_auth_id: Option<String>,
    pub influx_token_version: i64,
    pub pending_influx_auth_id: Option<String>,
    /// Jeton d'écriture en attente de remise. C'est un **secret de relais** :
    /// il n'est pas hachable puisqu'il doit être retransmis tel quel à la
    /// sonde, il est en écriture seule sur un unique bucket, et il est effacé
    /// dès que la sonde en accuse réception.
    pub pending_influx_token: Option<String>,
    pub pending_influx_token_version: Option<i64>,
    /// Dernière IP publique connue du site, et la date à laquelle elle a
    /// changé. Conservées quand la sonde est hors ligne : c'est justement là
    /// qu'on veut savoir où elle était.
    pub public_ip: Option<String>,
    pub public_ip_at: Option<i64>,
    /// Interface par laquelle la sonde sort, telle qu'elle la nomme.
    pub interface: Option<String>,
    /// Adresses locales au format CIDR. Vide tant qu'aucun battement ne les a
    /// portées — jamais vidée ensuite.
    pub local_ips: Vec<String>,
    pub gateway: Option<String>,
    /// Dernier verdict internet remonté par la sonde, et sa date. Conservé
    /// quand elle ne répond plus : c'est le dernier état connu, pas une
    /// mesure fraîche — l'interface doit le présenter comme tel.
    pub internet_state: Option<String>,
    pub internet_state_at: Option<i64>,
}

const PROBE_COLUMNS: &str = "p.probe_id, p.site_id, s.name, p.name, p.token_hash, p.platform, \
                             p.version, p.last_seen, p.buffered_points, p.created_at, p.revoked_at, \
                             p.influx_auth_id, p.influx_token_version, p.pending_influx_auth_id, \
                             p.pending_influx_token, p.pending_influx_token_version, \
                             p.public_ip, p.public_ip_at, p.interface, p.local_ips, p.gateway, \
                             p.internet_state, p.internet_state_at";

/// Ce qu'une sonde raconte de sa position réseau dans son battement.
///
/// Tout est facultatif : une sonde sans accès internet envoie son battement
/// **sans** IP publique plutôt que d'échouer, et un champ absent ne doit
/// jamais écraser ce que le hub savait déjà.
#[derive(Debug, Clone, Default)]
pub struct ProbeIdentity {
    pub public_ip: Option<String>,
    pub interface: Option<String>,
    pub local_ips: Vec<String>,
    pub gateway: Option<String>,
    /// Verdict internet de la sonde : `online` / `limited` / `offline`.
    ///
    /// ⚠️ C'est la seule chose qui distingue une sonde saine d'une sonde qui
    /// bat très bien mais ne mesure plus rien. Sans elle, le parc affichait un
    /// voyant vert pour un lien mort.
    pub internet_state: Option<String>,
}

/// Bascule d'IP publique observée à un battement — changement d'opérateur,
/// passage sur un lien de secours. C'est ce qui mérite une ligne d'audit ;
/// une adresse inchangée n'en mérite aucune.
#[derive(Debug, Clone)]
pub struct PublicIpChange {
    /// `None` la première fois que la sonde annonce une adresse.
    pub previous: Option<String>,
    pub current: String,
}

#[derive(Debug, Clone)]
pub struct EnrollCode {
    /// Code en clair — la seule fois où il existe hors du navigateur.
    pub code: String,
    pub site_id: String,
    pub probe_id: Option<String>,
    pub expires_at: i64,
}

/// Un enrôlement en attente, tel que l'interface l'affiche dans la liste des
/// sondes du site : une ligne fantôme avec son compte à rebours, remplacée par
/// la vraie sonde dès qu'une machine consomme le code.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingEnrollCode {
    /// Identifiant stable de l'attente. Sans lui, l'interface devait
    /// rapprocher une ligne de son code par `site|sonde|expiration` — deux
    /// codes émis pour le même site à la même seconde se confondaient.
    pub enroll_id: i64,
    pub site_id: String,
    pub site: String,
    /// Renseigné pour un ré-enrôlement : la ligne concerne une sonde existante.
    pub probe_id: Option<String>,
    pub probe: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    /// Le code en clair, tant qu'il est valide. `None` dès qu'il est consommé
    /// ou expiré : il est alors effacé de la base, seul le haché subsiste.
    pub code: Option<String>,
}

/// Ce qu'un code consommé donne le droit de faire.
#[derive(Debug, Clone)]
pub struct EnrollClaim {
    pub site_id: String,
    /// `Some` pour un code de ré-enrôlement : il répare cette sonde-là et ne
    /// peut pas servir à en créer une autre.
    pub probe_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReadTokenRecord {
    pub auth_id: String,
    pub description: String,
    pub created_at: i64,
}

fn probe_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProbeRecord> {
    Ok(ProbeRecord {
        probe_id: r.get(0)?,
        site_id: r.get(1)?,
        site_name: r.get(2)?,
        name: r.get(3)?,
        token_hash: r.get(4)?,
        platform: r.get(5)?,
        version: r.get(6)?,
        last_seen: r.get(7)?,
        buffered_points: r.get(8)?,
        created_at: r.get(9)?,
        revoked_at: r.get(10)?,
        influx_auth_id: r.get(11)?,
        influx_token_version: r.get(12)?,
        pending_influx_auth_id: r.get(13)?,
        pending_influx_token: r.get(14)?,
        pending_influx_token_version: r.get(15)?,
        public_ip: r.get(16)?,
        public_ip_at: r.get(17)?,
        interface: r.get(18)?,
        local_ips: r
            .get::<_, Option<String>>(19)?
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default(),
        gateway: r.get(20)?,
        internet_state: r.get(21)?,
        internet_state_at: r.get(22)?,
    })
}

fn user_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<UserRecord> {
    Ok(UserRecord {
        username: r.get(0)?,
        role: Role::from_stored(&r.get::<_, String>(1)?),
        created_at: r.get(2)?,
        disabled_at: r.get(3)?,
    })
}

fn check_password_length(password: &str) -> DbResult<()> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(DbError::Conflict(format!(
            "le mot de passe doit faire au moins {MIN_PASSWORD_LEN} caractères"
        )));
    }
    Ok(())
}

/// Traduit une violation de contrainte SQLite en `Conflict`. Sans ça, un nom
/// déjà pris ressortirait en 500 et l'utilisateur ne saurait pas quoi corriger.
fn conflict_on_constraint(e: rusqlite::Error, message: String) -> DbError {
    match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            DbError::Conflict(message)
        }
        other => DbError::Internal(other.to_string()),
    }
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| DbError::Internal(e.to_string()))?;
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// Base éphémère — les tests, et rien d'autre.
    pub fn open_in_memory() -> DbResult<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> DbResult<Self> {
        // `REFERENCES` ne contraint rien tant que les clés étrangères sont
        // désactivées — c'est le défaut de SQLite, et ça se paie en sondes
        // orphelines pointant vers un site inexistant.
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        migrate(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> DbResult<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| DbError::Internal("verrou SQLite empoisonné".into()))
    }

    pub fn user_version(&self) -> DbResult<i64> {
        let conn = self.lock()?;
        Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    /// Écrit une copie **cohérente** de la base dans `dest`, base ouverte et
    /// en cours d'écriture.
    ///
    /// C'est le seul moyen correct de sauvegarder du SQLite. `cp` sur le
    /// fichier pendant qu'une transaction court produit une archive qui a
    /// l'air parfaitement valide et refuse de s'ouvrir le jour où on en a
    /// besoin — le pire mode de panne possible pour une sauvegarde.
    ///
    /// SQLite refuse d'écraser un fichier existant, et on ne contourne pas ce
    /// refus : il protège une archive déjà écrite.
    pub fn vacuum_into(&self, dest: &Path) -> DbResult<()> {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| DbError::Internal(e.to_string()))?;
        }
        let dest = dest
            .to_str()
            .ok_or_else(|| DbError::Internal("chemin de destination non UTF-8".into()))?;
        let conn = self.lock()?;
        conn.execute("VACUUM INTO ?1", [dest])?;
        Ok(())
    }

    pub fn table_exists(&self, name: &str) -> DbResult<bool> {
        let conn = self.lock()?;
        let found: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [name],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    // ── Codes d'enrôlement ─────────────────────────────────────────────────

    /// Émet un code court, à usage unique. `probe_id` renseigné en fait un
    /// code de **ré-enrôlement** : il répare la sonde nommée au lieu d'en
    /// créer une nouvelle.
    ///
    /// Taper l'identifiant et le mot de passe du compte admin sur chaque
    /// machine sonde est pénible *et* dangereux — un poste compromis livrerait
    /// l'accès à l'interface. Un code expiré ou consommé n'ouvre rien.
    pub fn create_enroll_code(
        &self,
        site_id: &str,
        probe_id: Option<&str>,
        ttl_secs: i64,
    ) -> DbResult<EnrollCode> {
        let code = generate_enroll_code();
        let code_hash = lanprobe_core::passwords::hash_password(&code)
            .map_err(|e| DbError::Internal(e.to_string()))?;
        let created_at = now();
        let expires_at = created_at + ttl_secs;
        {
            let conn = self.lock()?;
            conn.execute(
                "INSERT INTO enroll_codes (code_hash, site_id, probe_id, created_at, expires_at, code_plain)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![code_hash, site_id, probe_id, created_at, expires_at, code],
            )?;
        }
        Ok(EnrollCode {
            code,
            site_id: site_id.to_string(),
            probe_id: probe_id.map(str::to_string),
            expires_at,
        })
    }

    /// Consomme un code. Atomique : deux sondes lancées sur le même code, une
    /// seule passe.
    pub fn consume_enroll_code(&self, code: &str) -> DbResult<EnrollClaim> {
        let mut conn = self.lock()?;
        // `IMMEDIATE` prend le verrou d'écriture dès l'ouverture : la lecture
        // des candidats et la marque de consommation ne peuvent pas être
        // entrelacées avec celles d'une autre transaction.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let now = now();

        // Le code est haché : on ne peut pas l'indexer, il faut vérifier les
        // candidats vivants un par un. Ils se comptent sur les doigts d'une
        // main — un code vit 15 minutes.
        let candidates: Vec<(String, String, Option<String>)> = {
            let mut stmt = tx.prepare(
                "SELECT code_hash, site_id, probe_id FROM enroll_codes
                 WHERE consumed_at IS NULL AND expires_at > ?1",
            )?;
            let rows = stmt.query_map([now], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        for (code_hash, site_id, probe_id) in candidates {
            if lanprobe_core::passwords::verify_password(&code_hash, code).is_err() {
                continue;
            }
            let changed = tx.execute(
                "UPDATE enroll_codes SET code_plain = NULL, consumed_at = ?2
                 WHERE code_hash = ?1 AND consumed_at IS NULL",
                rusqlite::params![code_hash, now],
            )?;
            if changed == 0 {
                break;
            }
            tx.commit()?;
            return Ok(EnrollClaim { site_id, probe_id });
        }
        Err(DbError::Unauthorized(
            "code d'enrôlement inconnu, expiré ou déjà utilisé".into(),
        ))
    }

    /// Empreintes stockées — sert aux tests à vérifier qu'aucun code en clair
    /// ne touche la base.
    /// Les codes encore ouverts : ni consommés, ni expirés.
    ///
    /// **Le code lui-même n'en fait pas partie** — il est stocké haché, comme
    /// un mot de passe, et n'existe en clair qu'une fois, à sa création. Un
    /// code perdu se regénère ; il ne se relit pas. C'est le prix de ne pas
    /// laisser traîner en base de quoi enrôler une sonde.
    pub fn pending_enroll_codes(&self, now: i64) -> DbResult<Vec<PendingEnrollCode>> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        // Le clair ne survit pas à l'expiration. On purge ici plutôt que par
        // une tâche de fond : pas de minuterie à surveiller, et la purge est
        // garantie d'avoir eu lieu avant qu'on puisse lire quoi que ce soit.
        conn.execute(
            "UPDATE enroll_codes SET code_plain = NULL
             WHERE code_plain IS NOT NULL AND (expires_at <= ?1 OR consumed_at IS NOT NULL)",
            [now],
        )?;
        let mut stmt = conn.prepare(
            "SELECT c.rowid, c.site_id, s.name, c.probe_id, p.name, c.created_at, c.expires_at, c.code_plain
             FROM enroll_codes c
             JOIN sites s ON s.site_id = c.site_id
             LEFT JOIN probes p ON p.probe_id = c.probe_id
             WHERE c.consumed_at IS NULL AND c.expires_at > ?1
             ORDER BY c.expires_at",
        )?;
        let rows = stmt
            .query_map([now], |row| {
                Ok(PendingEnrollCode {
                    enroll_id: row.get(0)?,
                    site_id: row.get(1)?,
                    site: row.get(2)?,
                    probe_id: row.get(3)?,
                    probe: row.get(4)?,
                    created_at: row.get(5)?,
                    expires_at: row.get(6)?,
                    code: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn enroll_code_hashes(&self) -> DbResult<Vec<String>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT code_hash FROM enroll_codes")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ── Réglages ───────────────────────────────────────────────────────────

    pub fn get_setting(&self, key: &str) -> DbResult<Option<String>> {
        let conn = self.lock()?;
        let value: Option<String> = conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()?;
        // Une chaîne vide n'est pas un réglage : ce serait une valeur qui
        // gagne la priorité sans rien valoir.
        Ok(value.filter(|v| !v.trim().is_empty()))
    }

    pub fn set_setting(&self, key: &str, value: &str) -> DbResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            rusqlite::params![key, value.trim(), now()],
        )?;
        Ok(())
    }

    /// Écrit `value` seulement si la clé n'a jamais été réglée. Sert à
    /// l'amorçage depuis l'environnement au premier démarrage : ensuite, la
    /// base gagne, et modifier la variable n'a plus d'effet.
    pub fn seed_setting(&self, key: &str, value: &str) -> DbResult<()> {
        if self.get_setting(key)?.is_some() {
            return Ok(());
        }
        self.set_setting(key, value)
    }

    // ── Comptes ────────────────────────────────────────────────────────────

    pub fn has_user(&self) -> DbResult<bool> {
        let conn = self.lock()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        Ok(count > 0)
    }

    /// Crée le compte admin initial. Non rejouable : dès qu'un compte existe,
    /// `/api/setup` ne doit plus être une porte d'entrée.
    pub fn create_initial_admin(&self, username: &str, password: &str) -> DbResult<()> {
        let username = username.trim();
        if username.is_empty() {
            return Err(DbError::Conflict("le nom d'utilisateur est requis".into()));
        }
        if self.has_user()? {
            return Err(DbError::Conflict("le compte initial existe déjà".into()));
        }
        let hash = lanprobe_core::passwords::hash_password(password)
            .map_err(|e| DbError::Internal(e.to_string()))?;
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO users (username, password_hash, role, created_at) VALUES (?1, ?2, 'admin', ?3)",
            rusqlite::params![username, hash, now()],
        )
        .map_err(|e| conflict_on_constraint(e, "le compte initial existe déjà".into()))?;
        Ok(())
    }

    /// Les comptes, par ordre alphabétique. Un compte désactivé y figure —
    /// on doit pouvoir le voir pour le réactiver.
    pub fn list_users(&self) -> DbResult<Vec<UserRecord>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT username, role, created_at, disabled_at FROM users
             ORDER BY username COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], user_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_user(&self, username: &str) -> DbResult<UserRecord> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT username, role, created_at, disabled_at FROM users WHERE username = ?1",
            [username],
            user_from_row,
        )
        .optional()?
        .ok_or_else(|| DbError::NotFound("compte inconnu".into()))
    }

    pub fn role_of(&self, username: &str) -> DbResult<Option<Role>> {
        let conn = self.lock()?;
        Ok(conn
            .query_row("SELECT role FROM users WHERE username = ?1", [username], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .map(|raw| Role::from_stored(&raw)))
    }

    pub fn create_user(&self, username: &str, password: &str, role: Role) -> DbResult<UserRecord> {
        let username = username.trim();
        if username.is_empty() {
            return Err(DbError::Conflict("le nom d'utilisateur est requis".into()));
        }
        check_password_length(password)?;
        let hash = lanprobe_core::passwords::hash_password(password)
            .map_err(|e| DbError::Internal(e.to_string()))?;
        {
            let conn = self.lock()?;
            conn.execute(
                "INSERT INTO users (username, password_hash, role, created_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![username, hash, role.as_str(), now()],
            )
            .map_err(|e| {
                conflict_on_constraint(e, format!("le compte « {username} » existe déjà"))
            })?;
        }
        self.get_user(username)
    }

    /// Change le rôle d'un compte. Deux refus, pour la même raison : un hub
    /// sans administrateur ne se répare que par accès au conteneur.
    ///
    /// - on ne se retire pas son propre rôle d'administrateur — c'est le geste
    ///   qu'on fait par erreur, et il n'a aucun usage légitime ;
    /// - on ne rétrograde pas le dernier administrateur actif.
    pub fn set_user_role(&self, actor: &str, username: &str, role: Role) -> DbResult<UserRecord> {
        let target = self.get_user(username)?;
        if target.role == Role::Admin && role != Role::Admin {
            if actor == target.username {
                return Err(DbError::Conflict(
                    "un administrateur ne peut pas se retirer son propre rôle".into(),
                ));
            }
            self.refuse_if_last_active_admin(&target)?;
        }
        {
            let conn = self.lock()?;
            conn.execute(
                "UPDATE users SET role = ?2 WHERE username = ?1",
                rusqlite::params![target.username, role.as_str()],
            )?;
        }
        self.get_user(&target.username)
    }

    /// Désactive ou réactive un compte. **Aucune suppression** : la ligne
    /// reste, parce que le journal d'audit la nomme.
    pub fn set_user_disabled(
        &self,
        actor: &str,
        username: &str,
        disabled: bool,
    ) -> DbResult<UserRecord> {
        let target = self.get_user(username)?;
        if disabled && target.disabled_at.is_none() {
            // Se désactiver soi-même, c'est se fermer la porte : même geste
            // par erreur que l'auto-rétrogradation, et pas davantage d'usage
            // légitime — pour partir, on se déconnecte.
            if actor == target.username {
                return Err(DbError::Conflict(
                    "un compte ne peut pas se désactiver lui-même".into(),
                ));
            }
            if target.role == Role::Admin {
                self.refuse_if_last_active_admin(&target)?;
            }
        }
        let stamp = disabled.then(now);
        {
            let conn = self.lock()?;
            conn.execute(
                "UPDATE users SET disabled_at = ?2 WHERE username = ?1",
                rusqlite::params![target.username, stamp],
            )?;
        }
        self.get_user(&target.username)
    }

    pub fn reset_user_password(&self, username: &str, password: &str) -> DbResult<()> {
        let target = self.get_user(username)?;
        check_password_length(password)?;
        let hash = lanprobe_core::passwords::hash_password(password)
            .map_err(|e| DbError::Internal(e.to_string()))?;
        let conn = self.lock()?;
        conn.execute(
            "UPDATE users SET password_hash = ?2 WHERE username = ?1",
            rusqlite::params![target.username, hash],
        )?;
        Ok(())
    }

    /// Administrateurs **actifs**. Un compte admin désactivé ne répare rien :
    /// le compter reviendrait à autoriser la fermeture de la dernière porte.
    pub fn active_admin_count(&self) -> DbResult<i64> {
        let conn = self.lock()?;
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled_at IS NULL",
            [],
            |r| r.get(0),
        )?)
    }

    fn refuse_if_last_active_admin(&self, target: &UserRecord) -> DbResult<()> {
        if target.disabled_at.is_none() && self.active_admin_count()? <= 1 {
            return Err(DbError::Conflict(
                "il doit rester au moins un administrateur actif".into(),
            ));
        }
        Ok(())
    }

    /// Rend le nom d'utilisateur canonique si les identifiants sont bons.
    /// L'erreur ne distingue jamais « compte inconnu » de « mauvais mot de
    /// passe » : ce serait un oracle à noms de comptes.
    pub fn verify_credentials(&self, username: &str, password: &str) -> DbResult<String> {
        let Some(hash) = self.password_hash_of(username)? else {
            return Err(DbError::Unauthorized("identifiants invalides".into()));
        };
        lanprobe_core::passwords::verify_password(&hash, password)
            .map_err(|_| DbError::Unauthorized("identifiants invalides".into()))?;
        // Un compte désactivé est traité comme un compte inconnu, et le mot
        // de passe est vérifié d'abord : répondre plus vite pour un compte
        // désactivé dirait à qui essaie que le nom existe.
        if self.get_user(username)?.disabled_at.is_some() {
            return Err(DbError::Unauthorized("identifiants invalides".into()));
        }
        Ok(username.to_string())
    }

    pub fn password_hash_of(&self, username: &str) -> DbResult<Option<String>> {
        let conn = self.lock()?;
        Ok(conn
            .query_row(
                "SELECT password_hash FROM users WHERE username = ?1",
                [username],
                |r| r.get(0),
            )
            .optional()?)
    }

    // ── Journal d'audit ────────────────────────────────────────────────────

    /// Ajoute une ligne. **Il n'existe aucune opération inverse** : pas de
    /// suppression, pas de purge, pas même pour un administrateur.
    ///
    /// L'appelant est responsable de ne jamais passer de secret — `detail`
    /// sert à dire « rétention portée à 90 jours », pas à recopier un corps
    /// de requête.
    pub fn record_audit(
        &self,
        actor: Option<&str>,
        action: &str,
        target: Option<&str>,
        outcome: Outcome,
        detail: Option<&str>,
    ) -> DbResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO audit_log (at, actor, action, target, outcome, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![now(), actor, action, target, outcome.as_str(), detail],
        )?;
        Ok(())
    }

    /// Les lignes, de la plus récente à la plus ancienne.
    pub fn list_audit(&self, filter: &AuditFilter) -> DbResult<Vec<AuditEntry>> {
        let limit = filter.limit.clamp(1, AUDIT_MAX_LIMIT);
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, at, actor, action, target, outcome, detail FROM audit_log
             WHERE (?1 IS NULL OR id < ?1)
               AND (?2 IS NULL OR actor = ?2)
               AND (?3 IS NULL OR action = ?3)
             ORDER BY id DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![filter.before_id, filter.actor, filter.action, limit],
            |r| {
                Ok(AuditEntry {
                    id: r.get(0)?,
                    at: r.get(1)?,
                    actor: r.get(2)?,
                    action: r.get(3)?,
                    target: r.get(4)?,
                    outcome: Outcome::from_stored(&r.get::<_, String>(5)?),
                    detail: r.get(6)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ── Secrets scellés ────────────────────────────────────────────────────
    //
    // Ce que la table `settings` ne peut pas porter : mot de passe SMTP, URL
    // de webhook. Le hub ne stocke ici que du chiffré (`enc:v1:`), et ne rend
    // jamais ces valeurs par l'API — l'interface sait « configuré » ou « non
    // configuré », plus un bouton de test.

    pub fn get_sealed(&self, key: &str) -> DbResult<Option<String>> {
        let conn = self.lock()?;
        let value: Option<String> = conn
            .query_row("SELECT value FROM sealed_secrets WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?;
        // Une valeur vide veut dire « non configuré » : c'est ainsi qu'on
        // désactive un canal sans supprimer de ligne.
        Ok(value.filter(|v| !v.trim().is_empty()))
    }

    pub fn set_sealed(&self, key: &str, sealed: &str) -> DbResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO sealed_secrets (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            rusqlite::params![key, sealed, now()],
        )?;
        Ok(())
    }

    /// Désactive un canal. **Pas de `DELETE`** : la ligne reste, vidée, avec
    /// la date à laquelle on l'a vidée.
    pub fn clear_sealed(&self, key: &str) -> DbResult<()> {
        self.set_sealed(key, "")
    }

    // ── Notifications : abonnements et états ───────────────────────────────

    /// Active, désactive, ou remet à l'héritage (`None`). Aucune suppression :
    /// « hérite » est une valeur, pas une absence de ligne.
    pub fn set_notify_subscription(
        &self,
        scope: &str,
        scope_id: &str,
        enabled: Option<bool>,
    ) -> DbResult<()> {
        if !matches!(scope, "site" | "probe") {
            return Err(DbError::Conflict(format!(
                "portée inconnue : {scope} (attendu : site ou probe)"
            )));
        }
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO notify_subscriptions (scope, scope_id, enabled) VALUES (?1, ?2, ?3)
             ON CONFLICT(scope, scope_id) DO UPDATE SET enabled = excluded.enabled",
            rusqlite::params![scope, scope_id, enabled],
        )?;
        Ok(())
    }

    pub fn list_notify_subscriptions(&self) -> DbResult<Vec<NotifySubscription>> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare("SELECT scope, scope_id, enabled FROM notify_subscriptions ORDER BY scope, scope_id")?;
        let rows = stmt.query_map([], |r| {
            Ok(NotifySubscription {
                scope: r.get(0)?,
                scope_id: r.get(1)?,
                enabled: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// L'exception de la sonde gagne ; sinon elle suit son site ; sinon rien
    /// n'alerte. Le défaut est le silence : une fonctionnalité de notification
    /// qui parle avant qu'on le lui demande se fait couper le premier jour.
    pub fn notify_enabled_for_probe(&self, probe_id: &str) -> DbResult<bool> {
        let probe = self.get_probe(probe_id)?;
        let conn = self.lock()?;
        let resolve = |scope: &str, id: &str| -> DbResult<Option<bool>> {
            Ok(conn
                .query_row(
                    "SELECT enabled FROM notify_subscriptions WHERE scope = ?1 AND scope_id = ?2",
                    [scope, id],
                    |r| r.get::<_, Option<bool>>(0),
                )
                .optional()?
                .flatten())
        };
        if let Some(exception) = resolve("probe", probe_id)? {
            return Ok(exception);
        }
        Ok(resolve("site", &probe.site_id)?.unwrap_or(false))
    }

    /// L'état annoncé pour une sonde, et depuis quand. `None` tant qu'on n'a
    /// rien annoncé : c'est ce qui distingue « première évaluation » de
    /// « toujours en ligne ».
    pub fn notify_state(&self, probe_id: &str) -> DbResult<Option<(AlertState, i64)>> {
        let conn = self.lock()?;
        Ok(conn
            .query_row(
                "SELECT state, since FROM notify_states WHERE probe_id = ?1",
                [probe_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?
            .map(|(state, since)| (AlertState::from_stored(&state), since)))
    }

    pub fn set_notify_state(&self, probe_id: &str, state: AlertState, since: i64) -> DbResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO notify_states (probe_id, state, since) VALUES (?1, ?2, ?3)
             ON CONFLICT(probe_id) DO UPDATE SET state = excluded.state, since = excluded.since",
            rusqlite::params![probe_id, state.as_str(), since],
        )?;
        Ok(())
    }

    // ── Sites ──────────────────────────────────────────────────────────────

    pub fn create_site(&self, name: &str) -> DbResult<Site> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DbError::Conflict("le nom du site est requis".into()));
        }
        let site_id = uuid::Uuid::new_v4().to_string();
        let created_at = now();
        {
            let conn = self.lock()?;
            conn.execute(
                "INSERT INTO sites (site_id, name, created_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![site_id, name, created_at],
            )
            .map_err(|e| conflict_on_constraint(e, format!("le site « {name} » existe déjà")))?;
        }
        Ok(Site {
            site_id,
            name: name.to_string(),
            created_at,
            probe_count: 0,
        })
    }

    /// Résout un site par son nom (insensible à la casse, cf. `COLLATE
    /// NOCASE`) et le crée s'il manque. Le booléen rendu alimente
    /// `site_created` : sans lui, un « Durnad » créé par faute de frappe
    /// passerait inaperçu jusqu'à ce qu'on cherche la sonde dans le mauvais
    /// parc.
    pub fn resolve_or_create_site(&self, name: &str) -> DbResult<(Site, bool)> {
        if let Some(existing) = self.find_site_by_name(name)? {
            return Ok((existing, false));
        }
        Ok((self.create_site(name)?, true))
    }

    pub fn find_site_by_name(&self, name: &str) -> DbResult<Option<Site>> {
        self.site_query("s.name = ?1 COLLATE NOCASE", name.trim())
    }

    pub fn get_site(&self, site_id: &str) -> DbResult<Site> {
        self.site_query("s.site_id = ?1", site_id)?
            .ok_or_else(|| DbError::NotFound("site inconnu".into()))
    }

    fn site_query(&self, predicate: &str, value: &str) -> DbResult<Option<Site>> {
        let conn = self.lock()?;
        Ok(conn
            .query_row(
                &format!("SELECT {SITE_COLUMNS} FROM sites s WHERE {predicate}"),
                [value],
                site_from_row,
            )
            .optional()?)
    }

    pub fn list_sites(&self) -> DbResult<Vec<Site>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {SITE_COLUMNS} FROM sites s ORDER BY s.name COLLATE NOCASE"
        ))?;
        let rows = stmt.query_map([], site_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn rename_site(&self, site_id: &str, name: &str) -> DbResult<Site> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DbError::Conflict("le nom du site est requis".into()));
        }
        {
            let conn = self.lock()?;
            let changed = conn
                .execute(
                    "UPDATE sites SET name = ?2 WHERE site_id = ?1",
                    rusqlite::params![site_id, name],
                )
                .map_err(|e| conflict_on_constraint(e, format!("le site « {name} » existe déjà")))?;
            if changed == 0 {
                return Err(DbError::NotFound("site inconnu".into()));
            }
        }
        self.get_site(site_id)
    }

    /// Refuse tant que le site porte des sondes — y compris révoquées, dont
    /// les mesures restent jointes par `site_id`. Aucune cascade dans ce
    /// projet : on ne fait pas disparaître un parc par un clic.
    pub fn delete_site(&self, site_id: &str) -> DbResult<()> {
        let conn = self.lock()?;
        let carried: i64 = conn.query_row(
            "SELECT COUNT(*) FROM probes WHERE site_id = ?1",
            [site_id],
            |r| r.get(0),
        )?;
        if carried > 0 {
            return Err(DbError::Conflict(format!(
                "le site porte encore {carried} sonde(s)"
            )));
        }
        let changed = conn.execute("DELETE FROM sites WHERE site_id = ?1", [site_id])?;
        if changed == 0 {
            return Err(DbError::NotFound("site inconnu".into()));
        }
        Ok(())
    }

    // ── Sondes ─────────────────────────────────────────────────────────────

    /// Enrôle une sonde et rend son jeton **en clair une seule fois** — seul
    /// le hash argon2id est conservé. Une base volée ne donne pas le droit
    /// d'écrire au nom des sondes.
    pub fn enroll_probe(&self, site_id: &str, name: &str) -> DbResult<(ProbeRecord, String)> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DbError::Conflict("le nom de la sonde est requis".into()));
        }
        let token = lanprobe_core::passwords::generate_token();
        let token_hash = lanprobe_core::passwords::hash_password(&token)
            .map_err(|e| DbError::Internal(e.to_string()))?;
        let probe_id = uuid::Uuid::new_v4().to_string();
        {
            let conn = self.lock()?;
            conn.execute(
                "INSERT INTO probes (probe_id, site_id, name, token_hash, buffered_points, created_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                rusqlite::params![probe_id, site_id, name, token_hash, now()],
            )
            .map_err(|e| {
                conflict_on_constraint(e, format!("une sonde « {name} » existe déjà dans ce site"))
            })?;
        }
        Ok((self.get_probe(&probe_id)?, token))
    }

    pub fn get_probe(&self, probe_id: &str) -> DbResult<ProbeRecord> {
        let conn = self.lock()?;
        conn.query_row(
            &format!(
                "SELECT {PROBE_COLUMNS} FROM probes p JOIN sites s ON s.site_id = p.site_id
                 WHERE p.probe_id = ?1"
            ),
            [probe_id],
            probe_from_row,
        )
        .optional()?
        .ok_or_else(|| DbError::NotFound("sonde inconnue".into()))
    }

    pub fn list_probes(&self, site_id: Option<&str>) -> DbResult<Vec<ProbeRecord>> {
        let conn = self.lock()?;
        let base = format!(
            "SELECT {PROBE_COLUMNS} FROM probes p JOIN sites s ON s.site_id = p.site_id"
        );
        let (sql, params) = match site_id {
            Some(id) => (
                format!("{base} WHERE p.site_id = ?1 ORDER BY p.name COLLATE NOCASE"),
                vec![id],
            ),
            None => (
                format!("{base} ORDER BY s.name COLLATE NOCASE, p.name COLLATE NOCASE"),
                vec![],
            ),
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), probe_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Renomme et/ou déplace une sonde. Les deux champs sont facultatifs ;
    /// le `probe_id` ne bouge jamais, c'est lui qui joint l'historique Influx.
    pub fn update_probe(
        &self,
        probe_id: &str,
        name: Option<&str>,
        site_id: Option<&str>,
    ) -> DbResult<ProbeRecord> {
        let current = self.get_probe(probe_id)?;
        let name = match name {
            Some(n) if n.trim().is_empty() => {
                return Err(DbError::Conflict("le nom de la sonde est requis".into()))
            }
            Some(n) => n.trim().to_string(),
            None => current.name.clone(),
        };
        let site_id_target = site_id.unwrap_or(&current.site_id).to_string();
        // Vérifié explicitement : la contrainte `REFERENCES` rendrait un
        // « constraint violation » indistinguable d'un doublon de nom.
        self.get_site(&site_id_target)?;
        {
            let conn = self.lock()?;
            conn.execute(
                "UPDATE probes SET name = ?2, site_id = ?3 WHERE probe_id = ?1",
                rusqlite::params![probe_id, name, site_id_target],
            )
            .map_err(|e| {
                conflict_on_constraint(e, format!("une sonde « {name} » existe déjà dans ce site"))
            })?;
        }
        self.get_probe(probe_id)
    }

    /// Révoque le jeton d'une sonde. **Ne supprime aucune mesure** et garde
    /// la ligne : les points Influx sont tagués `probe_id`, et un parc qui
    /// perd la résolution de ses noms perd la lecture de son historique.
    pub fn revoke_probe(&self, probe_id: &str) -> DbResult<()> {
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE probes SET revoked_at = ?2 WHERE probe_id = ?1 AND revoked_at IS NULL",
            rusqlite::params![probe_id, now()],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound("sonde inconnue ou déjà révoquée".into()));
        }
        Ok(())
    }

    /// Authentifie une sonde sur son jeton. Une sonde révoquée est traitée
    /// comme inconnue — révoquer doit fermer la porte tout de suite.
    pub fn authenticate_probe(&self, probe_id: &str, token: &str) -> DbResult<ProbeRecord> {
        let probe = self
            .get_probe(probe_id)
            .map_err(|_| DbError::Unauthorized("jeton de sonde invalide".into()))?;
        if probe.revoked_at.is_some() {
            return Err(DbError::Unauthorized("jeton de sonde invalide".into()));
        }
        lanprobe_core::passwords::verify_password(&probe.token_hash, token)
            .map_err(|_| DbError::Unauthorized("jeton de sonde invalide".into()))?;
        Ok(probe)
    }

    /// Ré-enrôle une sonde existante : nouveau jeton de sonde, `revoked_at`
    /// levé, tout le reste conservé. Le `probe_id` **doit** survivre — un
    /// `DELETE` suivi d'un nouvel enrôlement créerait une seconde sonde et
    /// couperait les courbes en deux à la date de l'incident.
    pub fn reenroll_probe(&self, probe_id: &str) -> DbResult<(ProbeRecord, String)> {
        let token = lanprobe_core::passwords::generate_token();
        let token_hash = lanprobe_core::passwords::hash_password(&token)
            .map_err(|e| DbError::Internal(e.to_string()))?;
        {
            let conn = self.lock()?;
            let changed = conn.execute(
                "UPDATE probes SET token_hash = ?2, revoked_at = NULL WHERE probe_id = ?1",
                rusqlite::params![probe_id, token_hash],
            )?;
            if changed == 0 {
                return Err(DbError::NotFound("sonde inconnue".into()));
            }
        }
        Ok((self.get_probe(probe_id)?, token))
    }

    /// Enregistre l'autorisation Influx active d'une sonde, juste après
    /// l'avoir frappée.
    pub fn set_probe_influx_auth(&self, probe_id: &str, auth_id: &str) -> DbResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE probes SET influx_auth_id = ?2 WHERE probe_id = ?1",
            rusqlite::params![probe_id, auth_id],
        )?;
        Ok(())
    }

    /// Prépare une rotation : le nouveau jeton attend d'être remis, l'ancien
    /// **reste actif**. Détruire avant l'accusé couperait toute sonde éteinte
    /// au mauvais moment — elle se réveillerait avec un jeton mort et sans
    /// moyen d'en obtenir un autre. Rend le numéro de version à annoncer.
    pub fn stage_token_rotation(
        &self,
        probe_id: &str,
        new_auth_id: &str,
        new_token: &str,
    ) -> DbResult<i64> {
        let current = self.get_probe(probe_id)?;
        let version = current.influx_token_version + 1;
        let conn = self.lock()?;
        conn.execute(
            "UPDATE probes SET pending_influx_auth_id = ?2,
                    pending_influx_token = ?3,
                    pending_influx_token_version = ?4
             WHERE probe_id = ?1",
            rusqlite::params![probe_id, new_auth_id, new_token, version],
        )?;
        Ok(version)
    }

    /// Enregistre l'accusé de réception de la sonde. Rend l'autorisation
    /// désormais inutile, **à détruire dans Influx par l'appelant**. `None`
    /// si la version accusée n'est pas celle qui attendait.
    pub fn acknowledge_token_version(
        &self,
        probe_id: &str,
        acknowledged: i64,
    ) -> DbResult<Option<String>> {
        let probe = self.get_probe(probe_id)?;
        if probe.pending_influx_token_version != Some(acknowledged) {
            return Ok(None);
        }
        let retired = probe.influx_auth_id.clone();
        let conn = self.lock()?;
        conn.execute(
            "UPDATE probes SET influx_auth_id = pending_influx_auth_id,
                    influx_token_version = ?2,
                    pending_influx_auth_id = NULL,
                    pending_influx_token = NULL,
                    pending_influx_token_version = NULL
             WHERE probe_id = ?1",
            rusqlite::params![probe_id, acknowledged],
        )?;
        Ok(retired)
    }

    /// Compromission : l'ancienne autorisation est retirée immédiatement, le
    /// remplaçant attend le prochain contact. La sonde ne peut pas écrire
    /// dans l'intervalle — c'est le comportement voulu. Son jeton **de
    /// sonde** reste valide, sans quoi la réparation à distance serait
    /// impossible.
    pub fn replace_token_immediately(
        &self,
        probe_id: &str,
        new_auth_id: &str,
        new_token: &str,
    ) -> DbResult<(Option<String>, i64)> {
        let current = self.get_probe(probe_id)?;
        let version = current.influx_token_version + 1;
        let retired = current.influx_auth_id.clone();
        let conn = self.lock()?;
        conn.execute(
            "UPDATE probes SET influx_auth_id = NULL,
                    pending_influx_auth_id = ?2,
                    pending_influx_token = ?3,
                    pending_influx_token_version = ?4
             WHERE probe_id = ?1",
            rusqlite::params![probe_id, new_auth_id, new_token, version],
        )?;
        Ok((retired, version))
    }

    // ── Jetons de lecture (Grafana) ────────────────────────────────────────

    /// N'enregistre que l'identifiant d'autorisation : la valeur du jeton est
    /// affichée une seule fois et n'est jamais conservée.
    pub fn record_read_token(&self, auth_id: &str, description: &str) -> DbResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO influx_read_tokens (auth_id, description, created_at)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![auth_id, description, now()],
        )?;
        Ok(())
    }

    pub fn list_read_tokens(&self) -> DbResult<Vec<ReadTokenRecord>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT auth_id, description, created_at FROM influx_read_tokens
             WHERE revoked_at IS NULL ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ReadTokenRecord {
                auth_id: r.get(0)?,
                description: r.get(1)?,
                created_at: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Marque le jeton révoqué. La ligne reste : on doit pouvoir dire qu'un
    /// jeton a existé et quand il a cessé de valoir.
    pub fn revoke_read_token(&self, auth_id: &str) -> DbResult<()> {
        let conn = self.lock()?;
        let changed = conn.execute(
            "UPDATE influx_read_tokens SET revoked_at = ?2
             WHERE auth_id = ?1 AND revoked_at IS NULL",
            rusqlite::params![auth_id, now()],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound("jeton de lecture inconnu".into()));
        }
        Ok(())
    }

    pub fn read_token_revoked_at(&self, auth_id: &str) -> DbResult<Option<i64>> {
        let conn = self.lock()?;
        Ok(conn
            .query_row(
                "SELECT revoked_at FROM influx_read_tokens WHERE auth_id = ?1",
                [auth_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }

    /// Enregistre un battement. Rend la bascule d'IP publique quand il y en a
    /// une, pour que l'appelant en tire une ligne d'audit — et **seulement**
    /// quand il y en a une : une ligne par battement noierait le journal.
    pub fn record_heartbeat(
        &self,
        probe_id: &str,
        version: Option<&str>,
        platform: Option<&str>,
        buffered_points: i64,
        identity: &ProbeIdentity,
    ) -> DbResult<Option<PublicIpChange>> {
        let conn = self.lock()?;
        // Sérialisé une fois : une liste vide vaut « je n'ai rien à dire »,
        // pas « je n'ai plus d'adresse ».
        let local_ips = if identity.local_ips.is_empty() {
            None
        } else {
            serde_json::to_string(&identity.local_ips).ok()
        };
        let previous: Option<String> = conn
            .query_row(
                "SELECT public_ip FROM probes WHERE probe_id = ?1",
                [probe_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        // COALESCE : une sonde qui omet un champ ne doit pas effacer celui
        // qu'elle avait annoncé au battement précédent. `public_ip_at` ne
        // bouge qu'à un vrai changement — `IS NOT` compare aussi les NULL.
        let changed = conn.execute(
            "UPDATE probes SET last_seen = ?2,
                    version = COALESCE(?3, version),
                    platform = COALESCE(?4, platform),
                    buffered_points = ?5,
                    public_ip = COALESCE(?6, public_ip),
                    public_ip_at = CASE WHEN ?6 IS NOT NULL AND ?6 IS NOT public_ip
                                        THEN ?2 ELSE public_ip_at END,
                    interface = COALESCE(?7, interface),
                    local_ips = COALESCE(?8, local_ips),
                    gateway = COALESCE(?9, gateway),
                    internet_state = COALESCE(?10, internet_state),
                    internet_state_at = CASE WHEN ?10 IS NOT NULL
                                             THEN ?2 ELSE internet_state_at END
             WHERE probe_id = ?1",
            rusqlite::params![
                probe_id,
                now(),
                version,
                platform,
                buffered_points,
                identity.public_ip,
                identity.interface,
                local_ips,
                identity.gateway,
                identity.internet_state,
            ],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound("sonde inconnue".into()));
        }
        Ok(match &identity.public_ip {
            Some(current) if Some(current) != previous.as_ref() => Some(PublicIpChange {
                previous,
                current: current.clone(),
            }),
            _ => None,
        })
    }

    /// Recule le dernier battement d'une sonde. **Tests seulement** : aucun
    /// chemin de production ne remonte le temps, un statut dérivé ne vaut que
    /// si personne ne peut le maquiller.
    #[cfg(test)]
    pub fn backdate_last_seen(&self, probe_id: &str, at: i64) -> DbResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE probes SET last_seen = ?2 WHERE probe_id = ?1",
            rusqlite::params![probe_id, at],
        )?;
        Ok(())
    }


    // ── File de commandes (contrat § 14) ───────────────────────────────────

    /// Empile une commande pour une sonde. Rend son identifiant.
    pub fn enqueue_command(
        &self,
        probe_id: &str,
        kind: &str,
        args: &serde_json::Value,
        actor: &str,
    ) -> DbResult<i64> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        // La sonde doit exister : une commande orpheline attendrait pour
        // toujours un battement qui ne viendra jamais.
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM probes WHERE probe_id = ?1 AND revoked_at IS NULL",
            rusqlite::params![probe_id],
            |r| r.get(0),
        )?;
        if exists == 0 {
            return Err(DbError::NotFound("sonde inconnue ou révoquée".into()));
        }
        conn.execute(
            "INSERT INTO probe_commands (probe_id, kind, args, created_at, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![probe_id, kind, args.to_string(), now(), actor],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Commandes à remettre à la sonde, en incrémentant leur compteur de
    /// remise. Au-delà de [`MAX_DELIVERIES`] sans accusé, la commande est
    /// déclarée échouée plutôt que remise une fois de plus.
    pub fn take_pending_commands(&self, probe_id: &str) -> DbResult<Vec<PendingCommand>> {
        let mut conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let tx = conn.transaction()?;

        tx.execute(
            "UPDATE probe_commands
                SET state = 'failed',
                    settled_at = ?2,
                    error = 'aucun accusé après ' || delivered_count || ' remises'
              WHERE probe_id = ?1 AND state = 'pending' AND delivered_count >= ?3",
            rusqlite::params![probe_id, now(), MAX_DELIVERIES],
        )?;

        let rows: Vec<PendingCommand> = {
            let mut stmt = tx.prepare(
                "SELECT id, kind, args FROM probe_commands
                  WHERE probe_id = ?1 AND state = 'pending'
                  ORDER BY id",
            )?;
            let mapped = stmt.query_map(rusqlite::params![probe_id], |r| {
                Ok(PendingCommand {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    args: serde_json::from_str(&r.get::<_, String>(2)?)
                        .unwrap_or(serde_json::Value::Null),
                })
            })?;
            mapped.collect::<Result<_, _>>()?
        };

        if !rows.is_empty() {
            tx.execute(
                "UPDATE probe_commands
                    SET delivered_count = delivered_count + 1
                  WHERE probe_id = ?1 AND state = 'pending'",
                rusqlite::params![probe_id],
            )?;
        }
        tx.commit()?;
        Ok(rows)
    }

    /// Enregistre l'accusé d'une commande. Une commande déjà réglée n'est pas
    /// retouchée : la sonde peut accuser deux fois si un battement se perd, et
    /// écraser le premier verdict effacerait l'erreur qui l'explique.
    pub fn settle_command(
        &self,
        probe_id: &str,
        id: i64,
        ok: bool,
        error: Option<&str>,
    ) -> DbResult<()> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        conn.execute(
            "UPDATE probe_commands
                SET state = ?3, settled_at = ?4, error = ?5
              WHERE id = ?1 AND probe_id = ?2 AND state = 'pending'",
            rusqlite::params![
                id,
                probe_id,
                if ok { "done" } else { "failed" },
                now(),
                error,
            ],
        )?;
        Ok(())
    }

    /// Historique récent, du plus neuf au plus ancien.
    pub fn list_commands(&self, probe_id: &str, limit: i64) -> DbResult<Vec<CommandRow>> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut stmt = conn.prepare(
            "SELECT id, kind, args, state, created_at, created_by,
                    delivered_count, settled_at, error
               FROM probe_commands
              WHERE probe_id = ?1
              ORDER BY id DESC
              LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![probe_id, limit], |r| {
            Ok(CommandRow {
                id: r.get(0)?,
                kind: r.get(1)?,
                args: serde_json::from_str(&r.get::<_, String>(2)?)
                    .unwrap_or(serde_json::Value::Null),
                state: r.get(3)?,
                created_at: r.get(4)?,
                created_by: r.get(5)?,
                delivered_count: r.get(6)?,
                settled_at: r.get(7)?,
                error: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// Applique les migrations manquantes. Idempotent : à chaque démarrage du
/// conteneur on rejoue, et une base déjà à niveau ne bouge pas.
fn migrate(conn: &Connection) -> DbResult<()> {
    let mut version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    while (version as usize) < MIGRATIONS.len() {
        conn.execute_batch(MIGRATIONS[version as usize])?;
        version += 1;
        // `PRAGMA user_version` n'accepte pas de paramètre lié.
        conn.execute_batch(&format!("PRAGMA user_version = {version}"))?;
    }
    Ok(())
}

/// Code court en `XXXX-XXXX`, sur un alphabet sans `0`/`O` ni `1`/`I` — il
/// est lu à voix haute et recopié à la main, une confusion de caractère se
/// paie en enrôlement raté sans message utile.
fn generate_enroll_code() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).expect("getrandom failed");
    let chars: String = bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_memory() -> Db {
        Db::open_in_memory().unwrap()
    }

    // ── File de commandes (contrat § 14) ───────────────────────────────────

    fn db_with_probe() -> (Db, String) {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        (db, probe.probe_id)
    }

    #[test]
    fn a_command_is_delivered_once_then_settled() {
        let (db, probe_id) = db_with_probe();
        let id = db
            .enqueue_command(&probe_id, "speedtest", &serde_json::json!({}), "claire")
            .unwrap();

        let first = db.take_pending_commands(&probe_id).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, id);
        assert_eq!(first[0].kind, "speedtest");

        db.settle_command(&probe_id, id, true, None).unwrap();
        assert!(
            db.take_pending_commands(&probe_id).unwrap().is_empty(),
            "une commande accusée ne doit plus repartir"
        );
        let row = &db.list_commands(&probe_id, 10).unwrap()[0];
        assert_eq!(row.state, "done");
        assert_eq!(row.created_by.as_deref(), Some("claire"));
    }

    #[test]
    fn a_command_never_acknowledged_fails_instead_of_looping_forever() {
        // ⚠️ Le cas qui compte : une commande qui fait planter la sonde
        // repartirait à chaque redémarrage, et la sonde ne se relèverait
        // jamais. Trois remises, puis on abandonne.
        let (db, probe_id) = db_with_probe();
        db.enqueue_command(&probe_id, "port_scan", &serde_json::json!({"ip": "10.0.0.1"}), "claire")
            .unwrap();

        for beat in 1..=MAX_DELIVERIES {
            assert_eq!(
                db.take_pending_commands(&probe_id).unwrap().len(),
                1,
                "elle doit être remise au battement {beat}"
            );
        }
        assert!(
            db.take_pending_commands(&probe_id).unwrap().is_empty(),
            "au-delà de {MAX_DELIVERIES} remises, la commande est abandonnée"
        );
        let row = &db.list_commands(&probe_id, 10).unwrap()[0];
        assert_eq!(row.state, "failed");
        assert!(row.error.as_deref().unwrap_or("").contains("aucun accusé"));
    }

    #[test]
    fn settling_twice_keeps_the_first_verdict() {
        // Un battement perdu fait accuser deux fois. Écraser le premier
        // verdict effacerait l'erreur qui explique l'échec.
        let (db, probe_id) = db_with_probe();
        let id = db
            .enqueue_command(&probe_id, "discovery", &serde_json::json!({}), "claire")
            .unwrap();
        db.take_pending_commands(&probe_id).unwrap();
        db.settle_command(&probe_id, id, false, Some("interface absente")).unwrap();
        db.settle_command(&probe_id, id, true, None).unwrap();

        let row = &db.list_commands(&probe_id, 10).unwrap()[0];
        assert_eq!(row.state, "failed");
        assert_eq!(row.error.as_deref(), Some("interface absente"));
    }

    #[test]
    fn a_command_for_an_unknown_probe_is_refused() {
        // Sans ça, la commande attendrait pour toujours un battement qui ne
        // viendra jamais, et l'interface la montrerait « en attente ».
        let db = open_memory();
        assert!(matches!(
            db.enqueue_command("inconnue", "speedtest", &serde_json::json!({}), "claire"),
            Err(DbError::NotFound(_))
        ));
    }

    #[test]
    fn a_revoked_probe_accepts_no_command() {
        let (db, probe_id) = db_with_probe();
        db.revoke_probe(&probe_id).unwrap();
        assert!(matches!(
            db.enqueue_command(&probe_id, "speedtest", &serde_json::json!({}), "claire"),
            Err(DbError::NotFound(_))
        ));
    }

    #[test]
    fn a_probe_never_sees_the_commands_of_another() {
        let (db, first) = db_with_probe();
        let site = db.create_site("Martin").unwrap();
        let (other, _) = db.enroll_probe(&site.site_id, "Lyon").unwrap();
        db.enqueue_command(&first, "speedtest", &serde_json::json!({}), "claire")
            .unwrap();
        assert!(db.take_pending_commands(&other.probe_id).unwrap().is_empty());
    }

    #[test]
    fn migrations_create_the_contract_schema() {
        let db = open_memory();
        for table in ["users", "sites", "probes"] {
            assert!(
                db.table_exists(table).unwrap(),
                "la table {table} doit exister après migration"
            );
        }
    }

    #[test]
    fn the_plaintext_code_dies_with_the_timer() {
        let db = Db::open_in_memory().unwrap();
        let site = db.create_site("Durand").unwrap();

        let code = db.create_enroll_code(&site.site_id, None, 900).unwrap();
        let pending = db.pending_enroll_codes(now()).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].code.as_deref(),
            Some(code.code.as_str()),
            "le code doit être lisible tant qu'il est valide"
        );

        // Consommé : le clair disparaît, le haché reste pour l'authentification.
        db.consume_enroll_code(&code.code).unwrap();
        assert!(
            db.pending_enroll_codes(now()).unwrap().is_empty(),
            "un code consommé ne figure plus dans les attentes"
        );
        assert!(
            !db.enroll_code_hashes().unwrap().is_empty(),
            "le haché survit : c'est lui qui empêche le rejeu"
        );
    }

    #[test]
    fn an_expired_code_is_stripped_of_its_plaintext() {
        let db = Db::open_in_memory().unwrap();
        let site = db.create_site("Durand").unwrap();
        db.create_enroll_code(&site.site_id, None, -1).unwrap();

        // La lecture purge : pas de minuterie à surveiller, et rien ne peut
        // être lu avant que la purge ait eu lieu.
        assert!(db.pending_enroll_codes(now()).unwrap().is_empty());

        let conn = db.conn.lock().unwrap();
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM enroll_codes WHERE code_plain IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0, "aucun code en clair ne doit survivre à son délai");
    }

    #[test]
    fn migrations_stamp_the_user_version() {
        let db = open_memory();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn migrations_are_replayable_on_an_existing_database() {
        // Rejouer les migrations sur une base déjà à niveau ne doit ni
        // échouer, ni retoucher les données : c'est ce qui se passe à chaque
        // redémarrage du conteneur.
        let dir = tmp_dir("replay");
        let path = dir.join("hub.sqlite");

        let db = Db::open(&path).unwrap();
        let site = db.create_site("Durand").unwrap();
        drop(db);

        let db = Db::open(&path).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(
            db.list_sites().unwrap().len(),
            1,
            "les données doivent survivre au rejeu des migrations"
        );
        assert_eq!(db.list_sites().unwrap()[0].site_id, site.site_id);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_v2_adds_settings_without_touching_existing_data() {
        // Une base restée en v1 doit se hisser en v2 sans perdre son parc :
        // c'est exactement ce que fait la mise à jour d'un conteneur déjà en
        // service.
        let dir = tmp_dir("v1-to-v2");
        let path = dir.join("hub.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.execute_batch("PRAGMA user_version = 1").unwrap();
            conn.execute(
                "INSERT INTO sites (site_id, name, created_at) VALUES ('s-1', 'Durand', 0)",
                [],
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        assert!(db.table_exists("settings").unwrap());
        assert_eq!(db.list_sites().unwrap().len(), 1, "le parc doit survivre");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_setting_round_trips_and_can_be_overwritten() {
        let db = open_memory();
        assert!(db.get_setting("influx_advertise_url").unwrap().is_none());

        db.set_setting("influx_advertise_url", "https://a.example:8086").unwrap();
        assert_eq!(
            db.get_setting("influx_advertise_url").unwrap().as_deref(),
            Some("https://a.example:8086")
        );

        db.set_setting("influx_advertise_url", "https://b.example:9086").unwrap();
        assert_eq!(
            db.get_setting("influx_advertise_url").unwrap().as_deref(),
            Some("https://b.example:9086")
        );
    }

    #[test]
    fn clearing_a_setting_falls_back_to_absent() {
        // Vider le réglage doit rendre la main à l'environnement, pas laisser
        // une chaîne vide qui gagnerait la priorité en ne valant rien.
        let db = open_memory();
        db.set_setting("influx_advertise_url", "https://a.example:8086").unwrap();
        db.set_setting("influx_advertise_url", "   ").unwrap();
        assert!(db.get_setting("influx_advertise_url").unwrap().is_none());
    }

    // ── Codes d'enrôlement ─────────────────────────────────────────────────

    #[test]
    fn an_enrollment_code_is_stored_hashed_only() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();

        let code = db.create_enroll_code(&site.site_id, None, 900).unwrap();
        assert!(code.code.len() >= 8, "code trop court : {}", code.code);

        let stored = db.enroll_code_hashes().unwrap();
        assert_eq!(stored.len(), 1);
        assert!(!stored[0].contains(&code.code), "le code en clair ne va pas en base");
        assert!(stored[0].starts_with("$argon2id$"));
    }

    #[test]
    fn an_enrollment_code_resolves_to_its_site_once() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let code = db.create_enroll_code(&site.site_id, None, 900).unwrap();

        let claim = db.consume_enroll_code(&code.code).unwrap();
        assert_eq!(claim.site_id, site.site_id);
        assert!(claim.probe_id.is_none(), "code de site : aucune sonde visée");

        // Usage unique : le rejeu ne doit rien ouvrir.
        assert!(matches!(
            db.consume_enroll_code(&code.code).unwrap_err(),
            DbError::Unauthorized(_)
        ));
    }

    #[test]
    fn an_expired_enrollment_code_opens_nothing() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        // TTL négatif : déjà expiré à la seconde où il est créé.
        let code = db.create_enroll_code(&site.site_id, None, -1).unwrap();

        assert!(matches!(
            db.consume_enroll_code(&code.code).unwrap_err(),
            DbError::Unauthorized(_)
        ));
    }

    #[test]
    fn an_unknown_enrollment_code_opens_nothing() {
        let db = open_memory();
        db.create_site("Durand").unwrap();
        assert!(matches!(
            db.consume_enroll_code("XXXX-XXXX").unwrap_err(),
            DbError::Unauthorized(_)
        ));
    }

    #[test]
    fn two_concurrent_uses_of_a_code_leave_exactly_one_winner() {
        // Deux sondes lancées ensemble sur le même code : la consommation est
        // atomique, une seule doit passer.
        let db = std::sync::Arc::new(open_memory());
        let site = db.create_site("Durand").unwrap();
        let code = db.create_enroll_code(&site.site_id, None, 900).unwrap();

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let db = db.clone();
                let code = code.code.clone();
                std::thread::spawn(move || db.consume_enroll_code(&code).is_ok())
            })
            .collect();
        let winners = handles.into_iter().filter(|_| true).fold(0, |acc, h| {
            acc + i32::from(h.join().unwrap())
        });

        assert_eq!(winners, 1, "exactement une sonde doit obtenir le code");
    }

    #[test]
    fn a_reenrollment_code_names_the_probe_it_repairs() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();

        let code = db
            .create_enroll_code(&site.site_id, Some(&probe.probe_id), 900)
            .unwrap();
        let claim = db.consume_enroll_code(&code.code).unwrap();

        assert_eq!(claim.probe_id.as_deref(), Some(probe.probe_id.as_str()));
        assert_eq!(claim.site_id, site.site_id);
    }

    #[test]
    fn reenrolling_reuses_the_probe_id_and_its_history() {
        // Un DELETE suivi d'un nouvel enrôlement créerait une seconde sonde et
        // couperait les courbes en deux à la date de l'incident — précisément
        // le moment où l'on veut comparer l'avant et l'après.
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, first_token) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        db.set_probe_influx_auth(&probe.probe_id, "auth-v1").unwrap();
        db.revoke_probe(&probe.probe_id).unwrap();

        let (repaired, new_token) = db.reenroll_probe(&probe.probe_id).unwrap();

        assert_eq!(repaired.probe_id, probe.probe_id, "le probe_id doit survivre");
        assert_eq!(repaired.created_at, probe.created_at);
        assert_eq!(repaired.site_id, site.site_id);
        assert_eq!(repaired.name, "Paris");
        assert!(repaired.revoked_at.is_none(), "la sonde redevient active");

        assert_ne!(new_token, first_token);
        assert!(
            db.authenticate_probe(&probe.probe_id, &first_token).is_err(),
            "l'ancien jeton de sonde ne doit plus valoir"
        );
        assert!(db.authenticate_probe(&probe.probe_id, &new_token).is_ok());
    }

    // ── Rotation des jetons d'écriture ─────────────────────────────────────

    #[test]
    fn a_rotation_keeps_the_old_token_until_the_probe_acknowledges() {
        // Détruire avant l'accusé couperait toute sonde éteinte au mauvais
        // moment : elle se réveillerait avec un jeton mort.
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        db.set_probe_influx_auth(&probe.probe_id, "auth-v1").unwrap();

        let version = db
            .stage_token_rotation(&probe.probe_id, "auth-v2", "jeton-v2")
            .unwrap();
        assert_eq!(version, 2, "la version doit progresser");

        let probe = db.get_probe(&probe.probe_id).unwrap();
        assert_eq!(
            probe.influx_auth_id.as_deref(),
            Some("auth-v1"),
            "l'ancien jeton doit rester actif tant qu'il n'y a pas d'accusé"
        );
        assert_eq!(probe.pending_influx_token.as_deref(), Some("jeton-v2"));
    }

    #[test]
    fn acknowledging_the_new_version_retires_the_old_authorization() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        db.set_probe_influx_auth(&probe.probe_id, "auth-v1").unwrap();
        let version = db
            .stage_token_rotation(&probe.probe_id, "auth-v2", "jeton-v2")
            .unwrap();

        let retired = db.acknowledge_token_version(&probe.probe_id, version).unwrap();
        assert_eq!(
            retired.as_deref(),
            Some("auth-v1"),
            "l'appelant reçoit l'autorisation à détruire dans Influx"
        );

        let probe = db.get_probe(&probe.probe_id).unwrap();
        assert_eq!(probe.influx_auth_id.as_deref(), Some("auth-v2"));
        assert_eq!(probe.influx_token_version, 2);
        assert!(
            probe.pending_influx_token.is_none(),
            "le jeton relayé est effacé de la base dès qu'il est accusé"
        );
    }

    #[test]
    fn acknowledging_a_stale_version_retires_nothing() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        db.set_probe_influx_auth(&probe.probe_id, "auth-v1").unwrap();
        db.stage_token_rotation(&probe.probe_id, "auth-v2", "jeton-v2").unwrap();

        // La sonde accuse encore l'ancienne version : rien ne doit bouger.
        assert!(db.acknowledge_token_version(&probe.probe_id, 1).unwrap().is_none());
        assert_eq!(
            db.get_probe(&probe.probe_id).unwrap().influx_auth_id.as_deref(),
            Some("auth-v1")
        );
    }

    #[test]
    fn an_immediate_revocation_retires_the_old_authorization_at_once() {
        // Compromission : on coupe d'abord, on rétablit ensuite.
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, hub_token) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        db.set_probe_influx_auth(&probe.probe_id, "auth-v1").unwrap();

        let (retired, version) = db
            .replace_token_immediately(&probe.probe_id, "auth-v2", "jeton-v2")
            .unwrap();
        assert_eq!(retired.as_deref(), Some("auth-v1"));
        assert_eq!(version, 2);

        let probe = db.get_probe(&probe.probe_id).unwrap();
        assert!(
            probe.influx_auth_id.is_none(),
            "plus aucun jeton d'écriture actif avant le prochain contact"
        );
        assert_eq!(probe.pending_influx_token.as_deref(), Some("jeton-v2"));
        assert!(
            db.authenticate_probe(&probe.probe_id, &hub_token).is_ok(),
            "le jeton de sonde reste valide : c'est ce qui permet la réparation à distance"
        );
    }

    // ── Jetons de lecture ──────────────────────────────────────────────────

    #[test]
    fn read_tokens_are_tracked_by_id_never_by_value() {
        let db = open_memory();
        db.record_read_token("auth-lecture-1", "Grafana du bureau").unwrap();

        let listed = db.list_read_tokens().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].auth_id, "auth-lecture-1");
        assert_eq!(listed[0].description, "Grafana du bureau");

        let rendered = serde_json::to_string(&listed).unwrap();
        assert!(
            !rendered.contains("\"token\""),
            "aucune valeur de jeton n'est conservée : {rendered}"
        );
    }

    #[test]
    fn revoking_a_read_token_keeps_the_trace_of_its_existence() {
        let db = open_memory();
        db.record_read_token("auth-lecture-1", "Grafana").unwrap();
        db.revoke_read_token("auth-lecture-1").unwrap();

        assert!(db.list_read_tokens().unwrap().is_empty());
        assert!(db.read_token_revoked_at("auth-lecture-1").unwrap().is_some());
    }

    // ── Comptes ────────────────────────────────────────────────────────────

    #[test]
    fn initial_admin_is_not_replayable() {
        let db = open_memory();
        assert!(!db.has_user().unwrap());

        db.create_initial_admin("admin", "password123").unwrap();
        assert!(db.has_user().unwrap());

        let err = db.create_initial_admin("pirate", "password123").unwrap_err();
        assert!(
            matches!(err, DbError::Conflict(_)),
            "un second setup doit être refusé : {err:?}"
        );
    }

    #[test]
    fn password_is_never_stored_in_clear() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        let hash = db.password_hash_of("admin").unwrap().unwrap();
        assert!(!hash.contains("password123"));
        assert!(hash.starts_with("$argon2id$"), "argon2id attendu : {hash}");
    }

    #[test]
    fn credentials_are_verified() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();

        assert!(db.verify_credentials("admin", "password123").is_ok());
        assert!(matches!(
            db.verify_credentials("admin", "mauvais").unwrap_err(),
            DbError::Unauthorized(_)
        ));
        assert!(matches!(
            db.verify_credentials("inconnu", "password123").unwrap_err(),
            DbError::Unauthorized(_)
        ));
    }

    // ── Sites ──────────────────────────────────────────────────────────────

    #[test]
    fn duplicate_site_name_is_a_conflict_whatever_the_case() {
        let db = open_memory();
        db.create_site("Durand").unwrap();
        let err = db.create_site("durand").unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");
    }

    #[test]
    fn resolving_a_site_is_case_insensitive_and_does_not_create() {
        let db = open_memory();
        let created = db.create_site("Durand").unwrap();

        let (found, was_created) = db.resolve_or_create_site("DURAND").unwrap();
        assert_eq!(found.site_id, created.site_id);
        assert_eq!(found.name, "Durand", "le nom d'origine est conservé");
        assert!(!was_created, "site_created doit être faux sur un site existant");
    }

    #[test]
    fn resolving_an_unknown_site_creates_it_and_says_so() {
        let db = open_memory();
        let (site, was_created) = db.resolve_or_create_site("Martin").unwrap();
        assert!(was_created, "site_created doit être vrai sur un site neuf");
        assert_eq!(site.name, "Martin");
        assert_eq!(db.list_sites().unwrap().len(), 1);
    }

    #[test]
    fn renaming_a_site_keeps_its_site_id() {
        let db = open_memory();
        let site = db.create_site("Durnad").unwrap();

        let renamed = db.rename_site(&site.site_id, "Durand").unwrap();
        assert_eq!(renamed.site_id, site.site_id, "le site_id est l'identité");
        assert_eq!(renamed.name, "Durand");
    }

    #[test]
    fn deleting_a_site_that_still_carries_probes_is_refused() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        db.enroll_probe(&site.site_id, "Paris").unwrap();

        let err = db.delete_site(&site.site_id).unwrap_err();
        assert!(
            matches!(err, DbError::Conflict(_)),
            "aucune suppression en cascade : {err:?}"
        );
        assert_eq!(db.list_sites().unwrap().len(), 1);
    }

    // ── Sondes ─────────────────────────────────────────────────────────────

    #[test]
    fn enrolling_yields_a_token_only_stored_hashed() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();

        let (probe, token) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        assert!(token.len() >= 32, "jeton trop court : {}", token.len());
        assert!(!probe.token_hash.contains(&token), "le jeton en clair ne va pas en base");
        assert!(probe.token_hash.starts_with("$argon2id$"));
        assert_eq!(probe.site_id, site.site_id);
    }

    #[test]
    fn duplicate_probe_name_within_a_site_is_a_conflict() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        db.enroll_probe(&site.site_id, "Paris").unwrap();

        let err = db.enroll_probe(&site.site_id, "paris").unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");
    }

    #[test]
    fn the_same_probe_name_in_two_sites_is_allowed() {
        // Deux clients ont chacun leur « Paris » — c'est le cas nominal, pas
        // une exception.
        let db = open_memory();
        let durand = db.create_site("Durand").unwrap();
        let martin = db.create_site("Martin").unwrap();

        db.enroll_probe(&durand.site_id, "Paris").unwrap();
        db.enroll_probe(&martin.site_id, "Paris").unwrap();

        assert_eq!(db.list_probes(None).unwrap().len(), 2);
    }

    #[test]
    fn authenticating_a_probe_accepts_only_its_own_token() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, token) = db.enroll_probe(&site.site_id, "Paris").unwrap();

        assert!(db.authenticate_probe(&probe.probe_id, &token).is_ok());
        assert!(matches!(
            db.authenticate_probe(&probe.probe_id, "faux-jeton").unwrap_err(),
            DbError::Unauthorized(_)
        ));
        assert!(matches!(
            db.authenticate_probe("probe-inexistante", &token).unwrap_err(),
            DbError::Unauthorized(_)
        ));
    }

    #[test]
    fn a_revoked_probe_can_no_longer_authenticate() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, token) = db.enroll_probe(&site.site_id, "Paris").unwrap();

        db.revoke_probe(&probe.probe_id).unwrap();

        assert!(matches!(
            db.authenticate_probe(&probe.probe_id, &token).unwrap_err(),
            DbError::Unauthorized(_)
        ));
    }

    #[test]
    fn revoking_a_probe_keeps_its_row() {
        // « Supprimer » une sonde révoque son jeton. La ligne reste : les
        // mesures Influx sont taguées `probe_id` et doivent rester lisibles.
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();

        db.revoke_probe(&probe.probe_id).unwrap();

        let kept = db.get_probe(&probe.probe_id).unwrap();
        assert!(kept.revoked_at.is_some(), "la révocation est horodatée");
        assert_eq!(kept.name, "Paris", "le nom reste résoluble pour l'historique");
    }

    #[test]
    fn renaming_a_probe_preserves_its_probe_id() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Pariss").unwrap();

        let renamed = db.update_probe(&probe.probe_id, Some("Paris"), None).unwrap();
        assert_eq!(renamed.probe_id, probe.probe_id, "le probe_id ne bouge jamais");
        assert_eq!(renamed.name, "Paris");
        assert_eq!(renamed.created_at, probe.created_at);
    }

    #[test]
    fn a_probe_can_be_moved_to_another_site() {
        let db = open_memory();
        let durand = db.create_site("Durand").unwrap();
        let martin = db.create_site("Martin").unwrap();
        let (probe, _) = db.enroll_probe(&durand.site_id, "Paris").unwrap();

        let moved = db.update_probe(&probe.probe_id, None, Some(&martin.site_id)).unwrap();
        assert_eq!(moved.site_id, martin.site_id);
        assert_eq!(moved.probe_id, probe.probe_id);

        assert_eq!(db.list_probes(Some(&martin.site_id)).unwrap().len(), 1);
        assert_eq!(db.list_probes(Some(&durand.site_id)).unwrap().len(), 0);
    }

    #[test]
    fn moving_a_probe_onto_a_taken_name_is_refused() {
        let db = open_memory();
        let durand = db.create_site("Durand").unwrap();
        let martin = db.create_site("Martin").unwrap();
        let (probe, _) = db.enroll_probe(&durand.site_id, "Paris").unwrap();
        db.enroll_probe(&martin.site_id, "Paris").unwrap();

        let err = db.update_probe(&probe.probe_id, None, Some(&martin.site_id)).unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");
    }

    #[test]
    fn heartbeat_records_what_the_probe_reports() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        assert!(db.get_probe(&probe.probe_id).unwrap().last_seen.is_none());

        db.record_heartbeat(&probe.probe_id, Some("1.2.0"), Some("linux"), 42, &ProbeIdentity::default())
            .unwrap();

        let seen = db.get_probe(&probe.probe_id).unwrap();
        assert_eq!(seen.version.as_deref(), Some("1.2.0"));
        assert_eq!(seen.platform.as_deref(), Some("linux"));
        assert_eq!(seen.buffered_points, 42);
        assert!(seen.last_seen.is_some());
    }

    #[test]
    fn listing_probes_carries_the_site_name() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        db.enroll_probe(&site.site_id, "Paris").unwrap();

        let probes = db.list_probes(None).unwrap();
        assert_eq!(probes[0].site_name, "Durand");
        assert_eq!(probes[0].site_id, site.site_id);
    }

    // ── Comptes et rôles ───────────────────────────────────────────────

    #[test]
    fn migration_v5_adds_the_disabled_column_without_touching_the_accounts() {
        // Une base en v4 tourne déjà en production : la colonne s'ajoute, les
        // comptes restent, et personne ne se retrouve désactivé au réveil.
        let dir = tmp_dir("v4-to-v5");
        let path = dir.join("hub.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            for migration in &MIGRATIONS[..4] {
                conn.execute_batch(migration).unwrap();
            }
            conn.execute_batch("PRAGMA user_version = 4").unwrap();
            conn.execute(
                "INSERT INTO users (username, password_hash, role, created_at)
                 VALUES ('ancien', 'peu-importe', 'admin', 0)",
                [],
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        let users = db.list_users().unwrap();
        assert_eq!(users.len(), 1, "le compte doit survivre");
        assert_eq!(users[0].username, "ancien");
        assert_eq!(users[0].role, Role::Admin);
        assert!(users[0].disabled_at.is_none(), "personne ne se réveille désactivé");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_initial_account_is_an_admin() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        assert_eq!(db.role_of("admin").unwrap(), Some(Role::Admin));
    }

    #[test]
    fn roles_are_ordered_from_the_least_to_the_most_privileged() {
        // C'est cet ordre qui permet d'écrire « au moins operator » en un
        // comparateur, au lieu d'énumérer les rôles à chaque route.
        assert!(Role::Viewer < Role::Operator);
        assert!(Role::Operator < Role::Admin);
    }

    #[test]
    fn an_unreadable_stored_role_falls_back_to_the_least_privilege() {
        // Une valeur inconnue en base — rétrogradation de version, écriture à
        // la main — ne doit jamais ouvrir plus de portes qu'elle n'en nomme.
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        {
            let conn = db.lock().unwrap();
            conn.execute("UPDATE users SET role = 'sorcier' WHERE username = 'admin'", [])
                .unwrap();
        }
        assert_eq!(db.role_of("admin").unwrap(), Some(Role::Viewer));
    }

    #[test]
    fn an_account_is_created_with_the_role_it_was_given() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();

        let created = db.create_user("claire", "password123", Role::Viewer).unwrap();
        assert_eq!(created.username, "claire");
        assert_eq!(created.role, Role::Viewer);
        assert!(created.disabled_at.is_none());
        assert_eq!(db.list_users().unwrap().len(), 2);
    }

    #[test]
    fn a_duplicate_username_is_a_conflict_not_a_silent_overwrite() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        let err = db.create_user("admin", "password123", Role::Viewer).unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");
    }

    #[test]
    fn a_disabled_account_can_no_longer_log_in() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("claire", "password123", Role::Operator).unwrap();
        assert!(db.verify_credentials("claire", "password123").is_ok());

        db.set_user_disabled("admin", "claire", true).unwrap();

        let err = db.verify_credentials("claire", "password123").unwrap_err();
        assert!(matches!(err, DbError::Unauthorized(_)), "{err:?}");
    }

    #[test]
    fn disabling_keeps_the_row_because_the_audit_trail_points_at_it() {
        // On ne supprime pas un compte : ses lignes d'audit le nomment, et un
        // compte disparu emporterait la traçabilité de ce qu'il a fait.
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("claire", "password123", Role::Operator).unwrap();

        db.set_user_disabled("admin", "claire", true).unwrap();

        let claire = db.get_user("claire").unwrap();
        assert!(claire.disabled_at.is_some(), "la désactivation doit être datée");
        assert!(
            db.list_users().unwrap().iter().any(|u| u.username == "claire"),
            "la ligne doit rester listée"
        );
    }

    #[test]
    fn a_disabled_account_can_be_brought_back() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("claire", "password123", Role::Operator).unwrap();
        db.set_user_disabled("admin", "claire", true).unwrap();

        db.set_user_disabled("admin", "claire", false).unwrap();

        assert!(db.get_user("claire").unwrap().disabled_at.is_none());
        assert!(db.verify_credentials("claire", "password123").is_ok());
    }

    #[test]
    fn the_last_admin_cannot_be_disabled() {
        // Un hub sans administrateur ne se répare que par accès au conteneur.
        // On passe par un autre acteur : se désactiver soi-même est refusé
        // plus tôt, pour une autre raison.
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("claire", "password123", Role::Operator).unwrap();

        let err = db.set_user_disabled("claire", "admin", true).unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");
        assert!(db.get_user("admin").unwrap().disabled_at.is_none());
    }

    #[test]
    fn an_account_cannot_disable_itself() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("claire", "password123", Role::Operator).unwrap();

        let err = db.set_user_disabled("claire", "claire", true).unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");
        assert!(db.get_user("claire").unwrap().disabled_at.is_none());
    }

    #[test]
    fn the_last_admin_cannot_be_demoted() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("claire", "password123", Role::Operator).unwrap();

        let err = db.set_user_role("admin", "admin", Role::Viewer).unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");
        assert_eq!(db.role_of("admin").unwrap(), Some(Role::Admin));
    }

    #[test]
    fn an_admin_cannot_take_their_own_admin_role_away() {
        // Même à deux administrateurs : se rétrograder soi-même est le geste
        // qu'on fait par erreur, et il n'a aucun usage légitime.
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("bertrand", "password123", Role::Admin).unwrap();

        let err = db.set_user_role("admin", "admin", Role::Operator).unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");
        assert_eq!(db.role_of("admin").unwrap(), Some(Role::Admin));
    }

    #[test]
    fn another_admin_can_be_demoted_once_a_second_one_remains() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("bertrand", "password123", Role::Admin).unwrap();

        db.set_user_role("admin", "bertrand", Role::Viewer).unwrap();
        assert_eq!(db.role_of("bertrand").unwrap(), Some(Role::Viewer));
    }

    #[test]
    fn a_disabled_admin_does_not_count_as_a_remaining_admin() {
        // Deux comptes admin dont un désactivé, c'est un seul administrateur.
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("bertrand", "password123", Role::Admin).unwrap();
        db.set_user_disabled("admin", "bertrand", true).unwrap();

        let err = db.set_user_disabled("bertrand", "admin", true).unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");
    }

    #[test]
    fn resetting_a_password_replaces_the_old_one() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("claire", "password123", Role::Viewer).unwrap();

        db.reset_user_password("claire", "nouveau-mot-de-passe").unwrap();

        assert!(db.verify_credentials("claire", "password123").is_err());
        assert!(db.verify_credentials("claire", "nouveau-mot-de-passe").is_ok());
    }

    #[test]
    fn a_password_shorter_than_eight_characters_is_refused() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();

        assert!(db.create_user("claire", "court", Role::Viewer).is_err());
        assert!(db.reset_user_password("admin", "court").is_err());
    }

    #[test]
    fn acting_on_an_unknown_account_is_a_not_found() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();

        assert!(matches!(
            db.get_user("fantôme").unwrap_err(),
            DbError::NotFound(_)
        ));
        assert!(matches!(
            db.set_user_role("admin", "fantôme", Role::Viewer).unwrap_err(),
            DbError::NotFound(_)
        ));
        assert!(matches!(
            db.set_user_disabled("admin", "fantôme", true).unwrap_err(),
            DbError::NotFound(_)
        ));
        assert!(matches!(
            db.reset_user_password("fantôme", "password123").unwrap_err(),
            DbError::NotFound(_)
        ));
    }

    #[test]
    fn listing_accounts_never_carries_a_password_hash() {
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();

        let rendered = serde_json::to_string(&db.list_users().unwrap()).unwrap();
        assert!(!rendered.contains("$argon2"), "hash exposé : {rendered}");
        for forbidden in ["hash", "password", "mot_de_passe"] {
            assert!(!rendered.contains(forbidden), "{forbidden} exposé : {rendered}");
        }
    }

    // ── Journal d'audit ────────────────────────────────────────────────

    #[test]
    fn migration_v6_adds_the_audit_log_to_an_existing_database() {
        let dir = tmp_dir("v5-to-v6");
        let path = dir.join("hub.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            for migration in &MIGRATIONS[..5] {
                conn.execute_batch(migration).unwrap();
            }
            conn.execute_batch("PRAGMA user_version = 5").unwrap();
            conn.execute(
                "INSERT INTO sites (site_id, name, created_at) VALUES ('s-1', 'Durand', 0)",
                [],
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        assert!(db.table_exists("audit_log").unwrap());
        assert_eq!(db.list_sites().unwrap().len(), 1, "le parc doit survivre");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_audit_line_carries_its_actor_action_target_and_outcome() {
        let db = open_memory();
        db.record_audit(Some("admin"), "probe.revoke", Some("p-1"), Outcome::Success, None)
            .unwrap();

        let entries = db.list_audit(&AuditFilter::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].actor.as_deref(), Some("admin"));
        assert_eq!(entries[0].action, "probe.revoke");
        assert_eq!(entries[0].target.as_deref(), Some("p-1"));
        assert_eq!(entries[0].outcome, Outcome::Success);
        assert!(entries[0].at > 0, "la ligne doit être datée");
    }

    #[test]
    fn failures_are_recorded_as_much_as_successes() {
        // Un journal qui n'enregistre que les succès ne montre jamais une
        // tentative d'intrusion : il montre celui qui a fini par entrer.
        let db = open_memory();
        db.record_audit(Some("pirate"), "auth.login", None, Outcome::Failure, None)
            .unwrap();
        db.record_audit(Some("admin"), "auth.login", None, Outcome::Success, None)
            .unwrap();

        let entries = db.list_audit(&AuditFilter::default()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.outcome == Outcome::Failure));
    }

    #[test]
    fn an_anonymous_attempt_is_recorded_without_an_actor() {
        // Une tentative sur un compte inexistant n'a pas d'acteur connu ; la
        // laisser tomber au motif qu'on ne sait pas qui c'était effacerait
        // précisément les lignes qui comptent.
        let db = open_memory();
        db.record_audit(None, "auth.login", None, Outcome::Failure, Some("compte inconnu"))
            .unwrap();

        let entries = db.list_audit(&AuditFilter::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].actor.is_none());
        assert_eq!(entries[0].detail.as_deref(), Some("compte inconnu"));
    }

    #[test]
    fn audit_lines_come_back_newest_first_and_paginate_backwards() {
        let db = open_memory();
        for n in 0..5 {
            db.record_audit(Some("admin"), &format!("action.{n}"), None, Outcome::Success, None)
                .unwrap();
        }

        let page = db
            .list_audit(&AuditFilter {
                limit: 2,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].action, "action.4", "la plus récente d'abord");
        assert_eq!(page[1].action, "action.3");

        let next = db
            .list_audit(&AuditFilter {
                limit: 2,
                before_id: Some(page[1].id),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(next[0].action, "action.2");
        assert_eq!(next[1].action, "action.1");
    }

    #[test]
    fn the_audit_log_filters_by_actor_and_by_action() {
        let db = open_memory();
        db.record_audit(Some("admin"), "auth.login", None, Outcome::Success, None).unwrap();
        db.record_audit(Some("claire"), "auth.login", None, Outcome::Failure, None).unwrap();
        db.record_audit(Some("claire"), "probe.enroll", None, Outcome::Success, None).unwrap();

        let by_actor = db
            .list_audit(&AuditFilter {
                actor: Some("claire".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_actor.len(), 2);

        let by_action = db
            .list_audit(&AuditFilter {
                action: Some("auth.login".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_action.len(), 2);
    }

    #[test]
    fn the_audit_limit_is_capped_so_one_request_cannot_drain_the_log() {
        let db = open_memory();
        for n in 0..3 {
            db.record_audit(Some("admin"), &format!("a{n}"), None, Outcome::Success, None)
                .unwrap();
        }
        let all = db
            .list_audit(&AuditFilter {
                limit: 100_000,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(all.len(), 3, "le plafond ne doit pas tronquer un journal court");
        // Vérifié à la compilation : un plafond qui ne plafonne rien ne sert à rien.
        const { assert!(AUDIT_MAX_LIMIT <= 1000) };
    }

    #[test]
    fn disabling_an_account_leaves_its_audit_trail_intact() {
        // C'est la raison même pour laquelle on ne supprime pas un compte.
        let db = open_memory();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("claire", "password123", Role::Operator).unwrap();
        db.record_audit(Some("claire"), "probe.revoke", Some("p-1"), Outcome::Success, None)
            .unwrap();

        db.set_user_disabled("admin", "claire", true).unwrap();

        let entries = db
            .list_audit(&AuditFilter {
                actor: Some("claire".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(entries.len(), 1, "les lignes de l'acteur doivent rester");
    }

    #[test]
    fn the_audit_log_survives_a_restart() {
        let dir = tmp_dir("audit-restart");
        let path = dir.join("hub.sqlite");

        let db = Db::open(&path).unwrap();
        db.record_audit(Some("admin"), "auth.login", None, Outcome::Success, None)
            .unwrap();
        drop(db);

        let db = Db::open(&path).unwrap();
        assert_eq!(db.list_audit(&AuditFilter::default()).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Notifications : stockage ───────────────────────────────────────

    #[test]
    fn migration_v7_adds_the_notification_tables_to_an_existing_database() {
        let dir = tmp_dir("v6-to-v7");
        let path = dir.join("hub.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            for migration in &MIGRATIONS[..6] {
                conn.execute_batch(migration).unwrap();
            }
            conn.execute_batch("PRAGMA user_version = 6").unwrap();
            conn.execute(
                "INSERT INTO sites (site_id, name, created_at) VALUES ('s-1', 'Durand', 0)",
                [],
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        for table in ["sealed_secrets", "notify_subscriptions", "notify_states"] {
            assert!(db.table_exists(table).unwrap(), "{table} doit exister");
        }
        assert_eq!(db.list_sites().unwrap().len(), 1, "le parc doit survivre");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sealed_value_round_trips_and_is_never_stored_in_the_clear() {
        let db = open_memory();
        assert!(db.get_sealed("notify_webhook").unwrap().is_none());

        db.set_sealed("notify_webhook", "enc:v1:AAAA").unwrap();
        assert_eq!(db.get_sealed("notify_webhook").unwrap().as_deref(), Some("enc:v1:AAAA"));
    }

    #[test]
    fn clearing_a_sealed_value_unconfigures_without_deleting_the_row() {
        // Aucune suppression dans ce projet : désactiver un canal écrit une
        // valeur vide, la ligne reste et sa date de mise à jour avec.
        let db = open_memory();
        db.set_sealed("notify_smtp", "enc:v1:AAAA").unwrap();

        db.clear_sealed("notify_smtp").unwrap();

        assert!(db.get_sealed("notify_smtp").unwrap().is_none());
        let rows: i64 = {
            let conn = db.lock().unwrap();
            conn.query_row("SELECT COUNT(*) FROM sealed_secrets", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(rows, 1, "la ligne doit rester");
    }

    #[test]
    fn a_probe_inherits_the_notification_setting_of_its_site() {
        // Sinon on recoche vingt cases à chaque nouveau client.
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        assert!(
            !db.notify_enabled_for_probe(&probe.probe_id).unwrap(),
            "par défaut, rien n'alerte"
        );

        db.set_notify_subscription("site", &site.site_id, Some(true)).unwrap();

        assert!(db.notify_enabled_for_probe(&probe.probe_id).unwrap());
    }

    #[test]
    fn a_probe_exception_overrides_its_site() {
        // Le poste de test de la baie du client n'alerte pas, le reste si.
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (paris, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        let (banc, _) = db.enroll_probe(&site.site_id, "Banc de test").unwrap();
        db.set_notify_subscription("site", &site.site_id, Some(true)).unwrap();

        db.set_notify_subscription("probe", &banc.probe_id, Some(false)).unwrap();

        assert!(db.notify_enabled_for_probe(&paris.probe_id).unwrap());
        assert!(!db.notify_enabled_for_probe(&banc.probe_id).unwrap());
    }

    #[test]
    fn an_exception_can_also_switch_a_probe_on_in_a_silent_site() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        db.set_notify_subscription("site", &site.site_id, Some(false)).unwrap();

        db.set_notify_subscription("probe", &probe.probe_id, Some(true)).unwrap();

        assert!(db.notify_enabled_for_probe(&probe.probe_id).unwrap());
    }

    #[test]
    fn clearing_an_exception_gives_the_probe_back_to_its_site() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        db.set_notify_subscription("site", &site.site_id, Some(true)).unwrap();
        db.set_notify_subscription("probe", &probe.probe_id, Some(false)).unwrap();

        // `None` n'efface pas la ligne : elle dit « hérite ».
        db.set_notify_subscription("probe", &probe.probe_id, None).unwrap();

        assert!(db.notify_enabled_for_probe(&probe.probe_id).unwrap());
    }

    #[test]
    fn an_unknown_subscription_scope_is_refused() {
        let db = open_memory();
        let err = db.set_notify_subscription("planète", "x", Some(true)).unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");
    }

    #[test]
    fn the_alert_state_of_a_probe_is_remembered_across_restarts() {
        // C'est ce qui empêche un redémarrage du hub de réannoncer une panne
        // déjà annoncée.
        let dir = tmp_dir("notify-state");
        let path = dir.join("hub.sqlite");

        let db = Db::open(&path).unwrap();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        assert!(db.notify_state(&probe.probe_id).unwrap().is_none());
        db.set_notify_state(&probe.probe_id, AlertState::Down, 1_000).unwrap();
        drop(db);

        let db = Db::open(&path).unwrap();
        assert_eq!(
            db.notify_state(&probe.probe_id).unwrap(),
            Some((AlertState::Down, 1_000))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Chaque test travaille dans son propre dossier — les tests Rust
    /// tournent en parallèle dans le même processus.
    // ── Identité réseau remontée par la sonde ──────────────────────────

    #[test]
    fn migration_v8_adds_the_network_identity_to_a_populated_database() {
        let dir = tmp_dir("v7-to-v8");
        let path = dir.join("hub.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            for migration in &MIGRATIONS[..7] {
                conn.execute_batch(migration).unwrap();
            }
            conn.execute_batch("PRAGMA user_version = 7").unwrap();
            conn.execute(
                "INSERT INTO sites (site_id, name, created_at) VALUES ('s-1', 'Durand', 0)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO probes (probe_id, site_id, name, token_hash, created_at)
                 VALUES ('p-1', 's-1', 'Paris', 'x', 0)",
                [],
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        // Le parc survit — une migration additive n'emporte rien.
        let probe = db.get_probe("p-1").unwrap();
        assert_eq!(probe.name, "Paris");
        assert_eq!(probe.public_ip, None);
        assert_eq!(probe.public_ip_at, None);
        assert!(probe.local_ips.is_empty());

        // Et rejouer l'ouverture ne casse rien.
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(db.user_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(db.get_probe("p-1").unwrap().name, "Paris");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_heartbeat_records_the_network_identity() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();

        let identity = ProbeIdentity {
            public_ip: Some("88.120.0.1".into()),
            interface: Some("en0".into()),
            local_ips: vec!["10.6.8.42/24".into()],
            gateway: Some("10.6.8.1".into()),
            internet_state: None,
        };
        let change = db
            .record_heartbeat(&probe.probe_id, Some("1.2.0"), Some("linux"), 0, &identity)
            .unwrap();

        let seen = db.get_probe(&probe.probe_id).unwrap();
        assert_eq!(seen.public_ip.as_deref(), Some("88.120.0.1"));
        assert_eq!(seen.interface.as_deref(), Some("en0"));
        assert_eq!(seen.local_ips, vec!["10.6.8.42/24".to_string()]);
        assert_eq!(seen.gateway.as_deref(), Some("10.6.8.1"));
        assert!(seen.public_ip_at.is_some(), "la date de l'adresse doit être posée");
        let change = change.expect("la première adresse connue est un changement");
        assert_eq!(change.previous, None);
        assert_eq!(change.current, "88.120.0.1");
    }

    #[test]
    fn a_probe_that_says_nothing_keeps_what_it_had_said() {
        // Une sonde qui perd son accès internet bat quand même, sans IP
        // publique. Remplacer la valeur connue par du vide effacerait
        // l'information au moment exact où elle sert.
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();

        db.record_heartbeat(
            &probe.probe_id,
            None,
            None,
            0,
            &ProbeIdentity {
                public_ip: Some("88.120.0.1".into()),
                interface: Some("en0".into()),
                local_ips: vec!["10.6.8.42/24".into()],
                gateway: Some("10.6.8.1".into()),
                internet_state: None,
            },
        )
        .unwrap();
        let before = db.get_probe(&probe.probe_id).unwrap().public_ip_at;

        let change = db
            .record_heartbeat(&probe.probe_id, None, None, 7, &ProbeIdentity::default())
            .unwrap();

        let seen = db.get_probe(&probe.probe_id).unwrap();
        assert_eq!(seen.public_ip.as_deref(), Some("88.120.0.1"));
        assert_eq!(seen.interface.as_deref(), Some("en0"));
        assert_eq!(seen.local_ips, vec!["10.6.8.42/24".to_string()]);
        assert_eq!(seen.gateway.as_deref(), Some("10.6.8.1"));
        assert_eq!(seen.public_ip_at, before, "une adresse inchangée ne se re-date pas");
        assert_eq!(seen.buffered_points, 7, "le reste du battement est bien pris");
        assert!(change.is_none(), "rien n'a changé, rien à signaler");
    }

    #[test]
    fn only_a_real_change_of_public_ip_is_reported() {
        let db = open_memory();
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();

        let first = ProbeIdentity {
            public_ip: Some("88.120.0.1".into()),
            ..Default::default()
        };
        assert!(db
            .record_heartbeat(&probe.probe_id, None, None, 0, &first)
            .unwrap()
            .is_some());
        // Le même battement, cent fois : aucun changement.
        for _ in 0..3 {
            assert!(
                db.record_heartbeat(&probe.probe_id, None, None, 0, &first)
                    .unwrap()
                    .is_none(),
                "une adresse identique n'est pas une bascule"
            );
        }

        let change = db
            .record_heartbeat(
                &probe.probe_id,
                None,
                None,
                0,
                &ProbeIdentity {
                    public_ip: Some("88.120.0.2".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .expect("une bascule d'opérateur doit se voir");
        assert_eq!(change.previous.as_deref(), Some("88.120.0.1"));
        assert_eq!(change.current, "88.120.0.2");
    }

    #[test]
    fn vacuum_into_writes_a_consistent_copy_while_the_base_is_open() {
        // Le seul point qui justifie `VACUUM INTO` : copier le fichier
        // pendant qu'il est écrit produit une archive qui a l'air valide et
        // ne se restaure pas. Ici la base reste ouverte pendant la copie.
        let dir = tmp_dir("vacuum-into");
        let db = Db::open(&dir.join("hub.sqlite")).unwrap();
        db.create_site("Durand").unwrap();
        db.create_initial_admin("admin", "password123").unwrap();

        let copy = dir.join("copie.sqlite");
        db.vacuum_into(&copy).unwrap();

        // La base d'origine sert toujours.
        db.create_site("Martin").unwrap();

        let restored = Db::open(&copy).unwrap();
        assert_eq!(restored.user_version().unwrap(), SCHEMA_VERSION);
        let names: Vec<String> = restored.list_sites().unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["Durand".to_string()], "la copie fige l'instant du VACUUM");
        assert!(restored.get_user("admin").is_ok());
    }

    #[test]
    fn vacuum_into_refuses_to_overwrite_an_existing_file() {
        // SQLite refuse d'écraser : on ne veut surtout pas contourner ce
        // refus, il protège une archive déjà écrite.
        let dir = tmp_dir("vacuum-existing");
        let db = Db::open(&dir.join("hub.sqlite")).unwrap();
        let copy = dir.join("copie.sqlite");
        std::fs::write(&copy, b"deja la").unwrap();

        assert!(db.vacuum_into(&copy).is_err());
        assert_eq!(std::fs::read(&copy).unwrap(), b"deja la");
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lanprobe-web-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}
