//! Hachage argon2id et primitives de jeton, partagés par la sonde
//! (`lanprobe-server`) et le hub (`lanprobe-web`).
//!
//! Les deux crates ont le même besoin : ranger un secret sans jamais pouvoir
//! le relire, et comparer un secret présenté sans laisser fuir le nombre
//! d'octets corrects. Dupliquer ce code, c'est accepter que les deux copies
//! divergent le jour où l'une durcit ses paramètres.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    // 16 bytes de salt brut → base64 via SaltString (argon2 0.5 ne prend pas
    // directement rand 0.10 en entrée, on contourne avec getrandom).
    let mut salt_bytes = [0u8; 16];
    getrandom::getrandom(&mut salt_bytes).map_err(|_| argon2::password_hash::Error::Crypto)?;
    let salt = SaltString::encode_b64(&salt_bytes)?;
    let argon = Argon2::default();
    let hash = argon.hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

pub fn verify_password(stored: &str, password: &str) -> Result<(), argon2::password_hash::Error> {
    let parsed = PasswordHash::new(stored)?;
    Argon2::default().verify_password(password.as_bytes(), &parsed)
}

/// Comparaison à temps constant — évite qu'un attaquant devine le token de
/// setup octet par octet en mesurant le temps de réponse.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub fn generate_token() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("getrandom failed");
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_round_trips() {
        let hash = hash_password("mot-de-passe-solide").unwrap();
        assert!(verify_password(&hash, "mot-de-passe-solide").is_ok());
    }

    #[test]
    fn verify_rejects_a_wrong_password() {
        let hash = hash_password("le-bon").unwrap();
        assert!(verify_password(&hash, "le-mauvais").is_err());
    }

    #[test]
    fn hash_never_contains_the_plaintext() {
        let hash = hash_password("secret-en-clair").unwrap();
        assert!(
            !hash.contains("secret-en-clair"),
            "le clair ne doit jamais apparaître dans le hash : {hash}"
        );
        assert!(hash.starts_with("$argon2id$"), "argon2id attendu : {hash}");
    }

    #[test]
    fn same_password_hashes_differently() {
        // Sel tiré à chaque hachage : deux comptes au même mot de passe ne
        // doivent pas se reconnaître dans un dump de la base.
        let a = hash_password("identique").unwrap();
        let b = hash_password("identique").unwrap();
        assert_ne!(a, b, "le sel doit être tiré à chaque hachage");
    }

    #[test]
    fn verify_rejects_a_malformed_hash() {
        assert!(verify_password("pas-un-hash-argon2", "peu-importe").is_err());
    }

    #[test]
    fn generate_token_is_long_and_unique() {
        let a = generate_token();
        let b = generate_token();
        assert!(a.len() >= 32, "jeton trop court : {}", a.len());
        assert_ne!(a, b, "deux jetons consécutifs ne doivent pas coïncider");
    }

    #[test]
    fn constant_time_eq_matches_byte_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }
}
