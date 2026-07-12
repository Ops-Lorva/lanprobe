//! Auth minimaliste stockée dans `users.json` à côté de la config du serveur.
//!
//! - Passwords hashés en argon2id (jamais en clair)
//! - Sessions en mémoire : token 32 bytes aléatoires dans un cookie HttpOnly
//! - Premier lancement : si users.json n'existe pas, le serveur expose une
//!   route `/api/setup` qui crée le premier admin (login + password) ; toutes
//!   les autres routes renvoient 503 avec `{ needs_setup: true }` jusque là.
//!
//! Pas de BDD — tant qu'on reste sur un admin + quelques users c'est suffisant.
//! Si un jour il faut de l'audit log ou du multi-tenant on migrera vers SQLite.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::{Deserialize, Serialize};

const SESSION_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 7);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecord {
    pub username: String,
    pub password_hash: String,
    pub role: String,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
struct UsersFile {
    users: Vec<UserRecord>,
}

pub struct AuthStore {
    path: PathBuf,
    users: Mutex<Vec<UserRecord>>,
    sessions: Mutex<HashMap<String, Session>>,
    /// Token de setup à haute entropie, généré au démarrage tant qu'aucun
    /// admin n'existe. `POST /api/setup` doit le présenter : ça ferme la
    /// fenêtre de course où n'importe qui sur le LAN pouvait devenir admin
    /// entre le `systemctl start` et le setup manuel de l'opérateur.
    setup_token: Mutex<Option<String>>,
}

struct Session {
    username: String,
    expires: Instant,
}

impl AuthStore {
    pub fn load(path: PathBuf) -> std::io::Result<Self> {
        let users = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            let parsed: UsersFile = serde_json::from_str(&raw).unwrap_or_default();
            parsed.users
        } else {
            Vec::new()
        };
        Ok(Self {
            path,
            users: Mutex::new(users),
            sessions: Mutex::new(HashMap::new()),
            setup_token: Mutex::new(None),
        })
    }

    pub fn needs_setup(&self) -> bool {
        self.users.lock().map(|u| u.is_empty()).unwrap_or(true)
    }

    /// Génère un token de setup à haute entropie, le conserve en mémoire et
    /// l'écrit dans `token_path` (permissions `0600` sous Unix). À appeler au
    /// démarrage : tant qu'aucun admin n'existe, `POST /api/setup` exigera ce
    /// token. Retourne `None` (sans écrire de secret) si un admin est déjà
    /// configuré — dans ce cas un éventuel fichier résiduel est neutralisé.
    pub fn init_setup_token(&self, token_path: &Path) -> Result<Option<String>, String> {
        if !self.needs_setup() {
            *self.setup_token.lock().map_err(|_| "setup_token lock poisoned".to_string())? = None;
            // Politique équipe : on ne supprime pas le fichier, on l'écrase
            // avec une valeur inutilisable pour ne pas laisser traîner un token.
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

    /// Crée le premier admin après vérification du token de setup généré au
    /// démarrage. Échoue si le token est absent/incorrect ou si un admin
    /// existe déjà. Le token est invalidé une fois consommé (pas de rejeu).
    pub fn initial_setup_with_token(
        &self,
        token: &str,
        username: &str,
        password: &str,
    ) -> Result<(), String> {
        {
            let guard = self
                .setup_token
                .lock()
                .map_err(|_| "setup_token lock poisoned".to_string())?;
            match guard.as_deref() {
                Some(expected) if constant_time_eq(expected.as_bytes(), token.as_bytes()) => {}
                _ => return Err("invalid or missing setup token".into()),
            }
        }
        self.initial_setup(username, password)?;
        // Token consommé : on l'invalide immédiatement.
        if let Ok(mut guard) = self.setup_token.lock() {
            *guard = None;
        }
        Ok(())
    }

    /// Crée le premier utilisateur admin. Échoue si un user existe déjà —
    /// la route `/api/setup` n'est pas rejouable.
    pub fn initial_setup(&self, username: &str, password: &str) -> Result<(), String> {
        let mut guard = self.users.lock().map_err(|_| "users lock poisoned".to_string())?;
        if !guard.is_empty() {
            return Err("setup already done".into());
        }
        let hash = hash_password(password).map_err(|e| e.to_string())?;
        guard.push(UserRecord {
            username: username.to_string(),
            password_hash: hash,
            role: "admin".into(),
        });
        self.persist(&guard)?;
        Ok(())
    }

    /// Crée ou met à jour les identifiants de l'administrateur.
    /// Contrairement à `initial_setup`, fonctionne même si un compte existe déjà.
    pub fn set_or_update_credentials(&self, username: &str, password: &str) -> Result<(), String> {
        let mut guard = self.users.lock().map_err(|_| "users lock poisoned".to_string())?;
        let hash = hash_password(password).map_err(|e| e.to_string())?;
        if let Some(user) = guard.iter_mut().find(|u| u.role == "admin") {
            user.username = username.to_string();
            user.password_hash = hash;
        } else {
            guard.push(UserRecord {
                username: username.to_string(),
                password_hash: hash,
                role: "admin".into(),
            });
        }
        self.persist(&guard)
    }

    pub fn login(&self, username: &str, password: &str) -> Result<String, String> {
        let guard = self.users.lock().map_err(|_| "users lock poisoned".to_string())?;
        let user = guard
            .iter()
            .find(|u| u.username == username)
            .ok_or_else(|| "invalid credentials".to_string())?;
        verify_password(&user.password_hash, password).map_err(|_| "invalid credentials".to_string())?;
        let token = generate_token();
        let mut sessions = self.sessions.lock().map_err(|_| "sessions lock poisoned".to_string())?;
        sessions.insert(
            token.clone(),
            Session {
                username: user.username.clone(),
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

    fn persist(&self, users: &[UserRecord]) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let data = UsersFile { users: users.to_vec() };
        let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, json).map_err(|e| e.to_string())?;
        // Restreint à lecture user-only — évite d'exposer les hashes au
        // reste du système dans un home partagé.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    // 16 bytes de salt brut → base64 via SaltString (argon2 0.5 ne prend pas
    // directement rand 0.10 en entrée, on contourne avec getrandom).
    let mut salt_bytes = [0u8; 16];
    getrandom::getrandom(&mut salt_bytes).map_err(|_| argon2::password_hash::Error::Crypto)?;
    let salt = SaltString::encode_b64(&salt_bytes)?;
    let argon = Argon2::default();
    let hash = argon.hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

fn verify_password(stored: &str, password: &str) -> Result<(), argon2::password_hash::Error> {
    let parsed = PasswordHash::new(stored)?;
    Argon2::default().verify_password(password.as_bytes(), &parsed)
}

/// Comparaison à temps constant — évite qu'un attaquant devine le token de
/// setup octet par octet en mesurant le temps de réponse.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn generate_token() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("getrandom failed");
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn default_users_path(config_dir: &Path) -> PathBuf {
    config_dir.join("users.json")
}

pub fn default_setup_token_path(config_dir: &Path) -> PathBuf {
    config_dir.join("setup-token")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_in(dir: &Path) -> AuthStore {
        AuthStore::load(default_users_path(dir)).unwrap()
    }

    #[test]
    fn init_setup_token_generates_and_writes_file() {
        let tmp = std::env::temp_dir().join(format!("lanprobe-auth-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let store = store_in(&tmp);
        let token_path = default_setup_token_path(&tmp);

        let token = store.init_setup_token(&token_path).unwrap();
        assert!(token.is_some(), "un token doit être généré tant qu'aucun admin n'existe");
        let token = token.unwrap();
        assert!(token.len() >= 32, "token trop court : {}", token.len());

        let on_disk = std::fs::read_to_string(&token_path).unwrap();
        assert!(on_disk.contains(&token), "le token doit être écrit sur disque");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&token_path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "le fichier token doit être 0600");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn setup_with_wrong_token_is_rejected() {
        let tmp = std::env::temp_dir().join(format!("lanprobe-auth-wrong-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let store = store_in(&tmp);
        store.init_setup_token(&default_setup_token_path(&tmp)).unwrap();

        let err = store
            .initial_setup_with_token("mauvais-token", "admin", "password123")
            .unwrap_err();
        assert!(err.contains("token"), "erreur attendue sur le token : {err}");
        assert!(store.needs_setup(), "aucun admin ne doit avoir été créé");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn setup_without_token_generated_is_rejected() {
        let tmp = std::env::temp_dir().join(format!("lanprobe-auth-none-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let store = store_in(&tmp);
        // Aucun init_setup_token() → pas de token en mémoire.
        let err = store
            .initial_setup_with_token("", "admin", "password123")
            .unwrap_err();
        assert!(err.contains("token"), "erreur attendue : {err}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn setup_with_correct_token_succeeds_then_not_replayable() {
        let tmp = std::env::temp_dir().join(format!("lanprobe-auth-ok-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let store = store_in(&tmp);
        let token = store
            .init_setup_token(&default_setup_token_path(&tmp))
            .unwrap()
            .unwrap();

        store
            .initial_setup_with_token(&token, "admin", "password123")
            .unwrap();
        assert!(!store.needs_setup(), "l'admin doit exister après le setup");

        // Le token est consommé : impossible de rejouer, même avec le bon token.
        let err = store
            .initial_setup_with_token(&token, "pirate", "password123")
            .unwrap_err();
        assert!(!err.is_empty(), "le rejeu doit échouer");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn init_setup_token_is_noop_when_admin_exists() {
        let tmp = std::env::temp_dir().join(format!("lanprobe-auth-exists-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let store = store_in(&tmp);
        store.initial_setup("admin", "password123").unwrap();

        let token = store.init_setup_token(&default_setup_token_path(&tmp)).unwrap();
        assert!(token.is_none(), "aucun token ne doit être généré si un admin existe");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
