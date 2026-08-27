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
pub const SCHEMA_VERSION: i64 = 1;

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
];

/// Erreurs remontées jusqu'à la couche HTTP, qui les traduit en codes. Un
/// `String` unique obligerait les handlers à deviner le code d'après le
/// texte du message — ils se tromperaient.
#[derive(Debug)]
pub enum DbError {
    Conflict(String),
    NotFound(String),
    Internal(String),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Conflict(m) | DbError::NotFound(m) | DbError::Internal(m) => write!(f, "{m}"),
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

    pub fn create_site(&self, name: &str) -> DbResult<Site> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DbError::Conflict("le nom du site est requis".into()));
        }
        let site_id = uuid::Uuid::new_v4().to_string();
        let created_at = now();
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO sites (site_id, name, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![site_id, name, created_at],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(err, _)
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                DbError::Conflict(format!("le site « {name} » existe déjà"))
            }
            other => DbError::Internal(other.to_string()),
        })?;
        Ok(Site {
            site_id,
            name: name.to_string(),
            created_at,
            probe_count: 0,
        })
    }

    pub fn list_sites(&self) -> DbResult<Vec<Site>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT s.site_id, s.name, s.created_at,
                    (SELECT COUNT(*) FROM probes p
                      WHERE p.site_id = s.site_id AND p.revoked_at IS NULL)
             FROM sites s ORDER BY s.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Site {
                site_id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                probe_count: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
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

    /// Chaque test travaille dans son propre dossier — les tests Rust
    /// tournent en parallèle dans le même processus.
    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lanprobe-web-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }
}
