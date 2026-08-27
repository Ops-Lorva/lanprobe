//! Sessions du navigateur et token de setup du premier lancement.
//!
//! Les comptes vivent en SQLite (`db.rs`) ; ce module ne tient que ce qui est
//! volatile — les sessions — et la fenêtre de premier démarrage.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lanprobe_core::passwords::{constant_time_eq, generate_token};

use crate::db::{Db, DbError, DbResult};

const SESSION_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 7);

struct Session {
    username: String,
    expires: Instant,
}

pub struct Auth {
    db: Arc<Db>,
    /// Sessions en mémoire : un redémarrage du conteneur déconnecte tout le
    /// monde, ce qui est le comportement souhaitable pour un hub qu'on
    /// redémarre rarement.
    sessions: Mutex<HashMap<String, Session>>,
    /// Token de setup à haute entropie, généré au démarrage tant qu'aucun
    /// compte n'existe. Il ferme la fenêtre de course où le premier venu
    /// devenait l'unique admin entre le `docker compose up` et le setup.
    setup_token: Mutex<Option<String>>,
}

impl Auth {
    pub fn new(db: Arc<Db>) -> Self {
        Self {
            db,
            sessions: Mutex::new(HashMap::new()),
            setup_token: Mutex::new(None),
        }
    }

    pub fn db(&self) -> &Db {
        &self.db
    }

    pub fn needs_setup(&self) -> bool {
        self.db.has_user().map(|has| !has).unwrap_or(true)
    }

    /// Génère le token de setup, le garde en mémoire et l'écrit en `0600`
    /// dans le volume. Rend `None` — sans rien écrire de secret — si un
    /// compte existe déjà ; un fichier résiduel est alors neutralisé plutôt
    /// que supprimé.
    pub fn init_setup_token(&self, token_path: &Path) -> Result<Option<String>, String> {
        if !self.needs_setup() {
            *self.setup_token.lock().map_err(|_| "setup_token lock poisoned".to_string())? = None;
            if token_path.exists() {
                let _ = std::fs::write(token_path, b"# setup already completed\n");
            }
            return Ok(None);
        }
        let token = generate_token();
        *self.setup_token.lock().map_err(|_| "setup_token lock poisoned".to_string())? =
            Some(token.clone());
        if let Some(parent) = token_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(token_path, format!("{token}\n")).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(token_path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(Some(token))
    }

    /// Crée le compte admin initial après vérification du token de setup.
    /// Le token est consommé au premier succès : pas de rejeu.
    pub fn setup(&self, token: &str, username: &str, password: &str) -> DbResult<()> {
        {
            let guard = self
                .setup_token
                .lock()
                .map_err(|_| DbError::Internal("setup_token lock poisoned".into()))?;
            match guard.as_deref() {
                Some(expected) if constant_time_eq(expected.as_bytes(), token.as_bytes()) => {}
                _ => return Err(DbError::Unauthorized("token de setup absent ou invalide".into())),
            }
        }
        self.db.create_initial_admin(username, password)?;
        if let Ok(mut guard) = self.setup_token.lock() {
            *guard = None;
        }
        Ok(())
    }

    pub fn login(&self, username: &str, password: &str) -> DbResult<String> {
        let username = self.db.verify_credentials(username, password)?;
        let token = generate_token();
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| DbError::Internal("sessions lock poisoned".into()))?;
        sessions.insert(
            token.clone(),
            Session {
                username,
                expires: Instant::now() + SESSION_TTL,
            },
        );
        Ok(token)
    }

    pub fn validate(&self, token: &str) -> Option<String> {
        let mut sessions = self.sessions.lock().ok()?;
        let session = sessions.get(token)?;
        if session.expires < Instant::now() {
            sessions.remove(token);
            return None;
        }
        Some(session.username.clone())
    }

    pub fn logout(&self, token: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(token);
        }
    }
}

pub fn default_setup_token_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("setup-token")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn auth() -> Auth {
        Auth::new(Arc::new(crate::db::Db::open_in_memory().unwrap()))
    }

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lanprobe-web-auth-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn setup_token_is_written_0600_while_no_account_exists() {
        let dir = tmp_dir("token");
        let path = dir.join("setup-token");
        let auth = auth();

        let token = auth.init_setup_token(&path).unwrap().expect("un token est attendu");
        assert!(token.len() >= 32, "token trop court : {}", token.len());
        assert!(std::fs::read_to_string(&path).unwrap().contains(&token));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "le fichier token doit être 0600");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_setup_token_is_generated_once_an_account_exists() {
        let dir = tmp_dir("existing");
        let path = dir.join("setup-token");
        let auth = auth();
        auth.db().create_initial_admin("admin", "password123").unwrap();

        assert!(auth.init_setup_token(&path).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn setup_with_a_wrong_token_is_rejected() {
        let dir = tmp_dir("wrong");
        let auth = auth();
        auth.init_setup_token(&dir.join("setup-token")).unwrap();

        let err = auth.setup("mauvais-token", "admin", "password123").unwrap_err();
        assert!(matches!(err, crate::db::DbError::Unauthorized(_)), "{err:?}");
        assert!(auth.needs_setup(), "aucun admin ne doit avoir été créé");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn setup_without_any_token_generated_is_rejected() {
        let auth = auth();
        let err = auth.setup("", "admin", "password123").unwrap_err();
        assert!(matches!(err, crate::db::DbError::Unauthorized(_)), "{err:?}");
    }

    #[test]
    fn setup_succeeds_once_then_is_not_replayable() {
        let dir = tmp_dir("replay");
        let auth = auth();
        let token = auth.init_setup_token(&dir.join("setup-token")).unwrap().unwrap();

        auth.setup(&token, "admin", "password123").unwrap();
        assert!(!auth.needs_setup());

        // Le token est consommé : même le bon token ne rejoue pas.
        assert!(auth.setup(&token, "pirate", "password123").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn login_opens_a_session_that_logout_closes() {
        let auth = auth();
        auth.db().create_initial_admin("admin", "password123").unwrap();

        let session = auth.login("admin", "password123").unwrap();
        assert_eq!(auth.validate(&session).as_deref(), Some("admin"));

        auth.logout(&session);
        assert!(auth.validate(&session).is_none(), "la session doit être fermée");
    }

    #[test]
    fn login_with_a_wrong_password_opens_nothing() {
        let auth = auth();
        auth.db().create_initial_admin("admin", "password123").unwrap();

        assert!(matches!(
            auth.login("admin", "mauvais").unwrap_err(),
            crate::db::DbError::Unauthorized(_)
        ));
    }

    #[test]
    fn an_unknown_session_token_is_not_valid() {
        let auth = auth();
        assert!(auth.validate("jeton-inventé").is_none());
    }
}
