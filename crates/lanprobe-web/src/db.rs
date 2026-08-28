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
pub const SCHEMA_VERSION: i64 = 4;

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
];

/// Erreurs remontées jusqu'à la couche HTTP, qui les traduit en codes. Un
/// `String` unique obligerait les handlers à deviner le code d'après le
/// texte du message — ils se tromperaient.
#[derive(Debug)]
pub enum DbError {
    Conflict(String),
    NotFound(String),
    Unauthorized(String),
    Internal(String),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Conflict(m)
            | DbError::NotFound(m)
            | DbError::Unauthorized(m)
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
}

const PROBE_COLUMNS: &str = "p.probe_id, p.site_id, s.name, p.name, p.token_hash, p.platform, \
                             p.version, p.last_seen, p.buffered_points, p.created_at, p.revoked_at, \
                             p.influx_auth_id, p.influx_token_version, p.pending_influx_auth_id, \
                             p.pending_influx_token, p.pending_influx_token_version";

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
    })
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

    /// Rend le nom d'utilisateur canonique si les identifiants sont bons.
    /// L'erreur ne distingue jamais « compte inconnu » de « mauvais mot de
    /// passe » : ce serait un oracle à noms de comptes.
    pub fn verify_credentials(&self, username: &str, password: &str) -> DbResult<String> {
        let Some(hash) = self.password_hash_of(username)? else {
            return Err(DbError::Unauthorized("identifiants invalides".into()));
        };
        lanprobe_core::passwords::verify_password(&hash, password)
            .map_err(|_| DbError::Unauthorized("identifiants invalides".into()))?;
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

    pub fn record_heartbeat(
        &self,
        probe_id: &str,
        version: Option<&str>,
        platform: Option<&str>,
        buffered_points: i64,
    ) -> DbResult<()> {
        let conn = self.lock()?;
        // COALESCE : une sonde qui omet `version` ne doit pas effacer celle
        // qu'elle avait annoncée au battement précédent.
        let changed = conn.execute(
            "UPDATE probes SET last_seen = ?2,
                    version = COALESCE(?3, version),
                    platform = COALESCE(?4, platform),
                    buffered_points = ?5
             WHERE probe_id = ?1",
            rusqlite::params![probe_id, now(), version, platform, buffered_points],
        )?;
        if changed == 0 {
            return Err(DbError::NotFound("sonde inconnue".into()));
        }
        Ok(())
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

        db.record_heartbeat(&probe.probe_id, Some("1.2.0"), Some("linux"), 42)
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

    /// Chaque test travaille dans son propre dossier — les tests Rust
    /// tournent en parallèle dans le même processus.
    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lanprobe-web-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}
