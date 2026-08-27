//! Chiffrement au repos des secrets applicatifs de `app_config.json`.
//!
//! Ce qui est chiffré ici : les **secrets applicatifs** (jeton InfluxDB v2,
//! mot de passe InfluxDB v1). Les **mots de passe utilisateur** ne passent
//! jamais par ce module : ils sont hashés en argon2id dans `auth.rs`, et un
//! mot de passe déchiffrable serait une faille, pas une fonctionnalité.
//!
//! ## Où vit la clé — la mesure exacte
//!
//! Par défaut la clé est un fichier `secret.key` (0600) posé **dans le même
//! config-dir** que `app_config.json`. Il faut le dire franchement : une clé
//! rangée à côté du fichier qu'elle protège ne défend que contre la copie du
//! fichier seul — sauvegarde qui fuit, volume exporté, `app_config.json`
//! collé dans un ticket. Elle ne défend **pas** contre quelqu'un qui a déjà
//! accès à la machine ou au volume complet : il lit les deux fichiers.
//!
//! Pour un vrai gain, l'opérateur passe la clé par l'environnement
//! (`LANPROBE_SECRET_KEY`, 32 octets en base64) — Docker secret, systemd
//! `LoadCredential`, gestionnaire de secrets — et la clé ne touche alors
//! jamais le volume. Quand cette variable est définie, aucun `secret.key`
//! n'est écrit sur disque.

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};

/// Préfixe des valeurs chiffrées sur disque. Sert aussi de marqueur de
/// migration : une valeur sans ce préfixe est une valeur en clair héritée
/// d'une version antérieure, qu'on relit telle quelle puis qu'on rechiffre
/// à la première écriture.
pub const SEALED_PREFIX: &str = "enc:v1:";

/// Nom de la variable d'environnement qui permet de garder la clé hors du
/// volume de données (base64 de 32 octets).
pub const KEY_ENV: &str = "LANPROBE_SECRET_KEY";

pub struct SecretKey {
    bytes: [u8; 32],
}

/// Charge la clé depuis `LANPROBE_SECRET_KEY` si elle est définie, sinon
/// depuis `<config_dir>/secret.key` — en la générant (0600) au premier appel.
pub fn load_or_create_key(config_dir: &Path) -> Result<SecretKey, String> {
    if let Ok(encoded) = std::env::var(KEY_ENV) {
        let trimmed = encoded.trim();
        if !trimmed.is_empty() {
            return decode_key(trimmed)
                .map_err(|e| format!("{KEY_ENV} illisible : {e}"));
        }
    }

    let path = default_key_path(config_dir);
    if path.exists() {
        let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        return decode_key(raw.trim())
            .map_err(|e| format!("{} illisible : {e}", path.display()));
    }

    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, format!("{}\n", B64.encode(bytes))).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(SecretKey { bytes })
}

/// Chiffre `plaintext` en AES-256-GCM et rend `enc:v1:<base64(nonce||sceau)>`.
pub fn seal(key: &SecretKey, plaintext: &str) -> Result<String, String> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce_bytes).map_err(|e| e.to_string())?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);

    let mut buffer = plaintext.as_bytes().to_vec();
    key.aead()?
        .seal_in_place_append_tag(nonce, Aad::empty(), &mut buffer)
        .map_err(|_| "échec du chiffrement".to_string())?;

    let mut out = Vec::with_capacity(NONCE_LEN + buffer.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&buffer);
    Ok(format!("{SEALED_PREFIX}{}", B64.encode(&out)))
}

/// Déchiffre une valeur produite par [`seal`]. Rend une erreur si la valeur
/// n'est pas scellée, si la clé est mauvaise ou si le sceau a été altéré.
pub fn open(key: &SecretKey, sealed: &str) -> Result<String, String> {
    let body = sealed
        .strip_prefix(SEALED_PREFIX)
        .ok_or_else(|| "valeur non scellée".to_string())?;
    let raw = B64.decode(body).map_err(|e| e.to_string())?;
    if raw.len() <= NONCE_LEN {
        return Err("sceau tronqué".into());
    }
    let (nonce_bytes, cipher) = raw.split_at(NONCE_LEN);
    let nonce = Nonce::try_assume_unique_for_key(nonce_bytes)
        .map_err(|_| "nonce invalide".to_string())?;

    let mut buffer = cipher.to_vec();
    let plain = key
        .aead()?
        .open_in_place(nonce, Aad::empty(), &mut buffer)
        .map_err(|_| "sceau invalide ou altéré".to_string())?;
    String::from_utf8(plain.to_vec()).map_err(|e| e.to_string())
}

impl SecretKey {
    fn aead(&self) -> Result<LessSafeKey, String> {
        let unbound = UnboundKey::new(&AES_256_GCM, &self.bytes)
            .map_err(|_| "clé AES-256 invalide".to_string())?;
        Ok(LessSafeKey::new(unbound))
    }
}

fn decode_key(encoded: &str) -> Result<SecretKey, String> {
    let raw = B64.decode(encoded).map_err(|e| e.to_string())?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| "la clé doit faire 32 octets une fois décodée".to_string())?;
    Ok(SecretKey { bytes })
}

/// Vrai si la valeur porte le marqueur de chiffrement.
pub fn is_sealed(value: &str) -> bool {
    value.starts_with(SEALED_PREFIX)
}

pub fn default_key_path(config_dir: &Path) -> PathBuf {
    config_dir.join("secret.key")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chaque test travaille dans son propre dossier — les tests Rust
    /// tournent en parallèle dans le même processus.
    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lanprobe-secrets-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn seal_then_open_round_trips() {
        let dir = tmp_dir("roundtrip");
        let key = load_or_create_key(&dir).unwrap();

        let sealed = seal(&key, "mon-token-influx").unwrap();
        assert_eq!(open(&key, &sealed).unwrap(), "mon-token-influx");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sealed_value_hides_the_plaintext() {
        let dir = tmp_dir("hides");
        let key = load_or_create_key(&dir).unwrap();

        let sealed = seal(&key, "super-secret-token").unwrap();
        assert!(is_sealed(&sealed), "la valeur doit porter le marqueur : {sealed}");
        assert!(
            !sealed.contains("super-secret-token"),
            "le clair ne doit pas apparaître : {sealed}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_plaintext_seals_differently() {
        let dir = tmp_dir("nonce");
        let key = load_or_create_key(&dir).unwrap();

        // Nonce aléatoire par scellement : deux chiffrés identiques
        // laisseraient fuiter « ces deux instances ont le même token ».
        let a = seal(&key, "identique").unwrap();
        let b = seal(&key, "identique").unwrap();
        assert_ne!(a, b, "le nonce doit être tiré à chaque scellement");
        assert_eq!(open(&key, &a).unwrap(), open(&key, &b).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tampered_seal_is_rejected() {
        let dir = tmp_dir("tamper");
        let key = load_or_create_key(&dir).unwrap();

        let sealed = seal(&key, "token-intact").unwrap();
        let body = &sealed[SEALED_PREFIX.len()..];
        let mut raw = B64.decode(body).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff; // altère le tag GCM
        let forged = format!("{SEALED_PREFIX}{}", B64.encode(&raw));

        assert!(open(&key, &forged).is_err(), "un sceau altéré doit être rejeté");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_rejects_a_plaintext_value() {
        let dir = tmp_dir("plain");
        let key = load_or_create_key(&dir).unwrap();

        assert!(!is_sealed("token-en-clair"));
        assert!(open(&key, "token-en-clair").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn key_file_is_created_0600_and_reused() {
        let dir = tmp_dir("keyfile");
        let key_path = default_key_path(&dir);
        let _ = std::fs::remove_file(&key_path);

        let first = load_or_create_key(&dir).unwrap();
        assert!(key_path.exists(), "la clé doit être écrite au premier appel");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "secret.key doit être 0600");
        }

        // Rechargement : la même clé doit ouvrir ce que la première a scellé,
        // sinon un redémarrage rendrait la config illisible.
        let sealed = seal(&first, "persistant").unwrap();
        let second = load_or_create_key(&dir).unwrap();
        assert_eq!(open(&second, &sealed).unwrap(), "persistant");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_key_takes_precedence_and_writes_nothing() {
        let dir = tmp_dir("envkey");
        let key_path = default_key_path(&dir);
        let _ = std::fs::remove_file(&key_path);

        let raw = [7u8; 32];
        // SAFETY : test mono-thread sur cette variable, restaurée avant de rendre.
        unsafe { std::env::set_var(KEY_ENV, B64.encode(raw)) };
        let key = load_or_create_key(&dir).unwrap();
        let sealed = seal(&key, "via-env").unwrap();
        unsafe { std::env::remove_var(KEY_ENV) };

        assert!(
            !key_path.exists(),
            "aucune clé ne doit être posée sur le volume quand {KEY_ENV} est défini"
        );
        assert_eq!(open(&key, &sealed).unwrap(), "via-env");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
