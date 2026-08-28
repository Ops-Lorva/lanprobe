//! Certificat TLS du hub.
//!
//! Le hub sert en HTTPS avec un certificat auto-signé généré dans son volume.
//! Sans TLS, le cookie de session `Secure` ne serait jamais renvoyé par le
//! navigateur — et les identifiants d'enrôlement circuleraient en clair sur le
//! LAN. Le certificat est **conservé** entre deux démarrages : le régénérer
//! obligerait l'utilisateur à l'accepter de nouveau à chaque redémarrage du
//! conteneur, et l'habituerait à cliquer sans lire.

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct TlsPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
}

pub fn tls_paths(config_dir: &Path) -> TlsPaths {
    TlsPaths {
        cert: config_dir.join("hub-cert.pem"),
        key: config_dir.join("hub-key.pem"),
    }
}

/// Génère le couple certificat/clé s'il manque. Ne touche à rien s'il existe.
pub fn ensure_certificate(paths: &TlsPaths) -> Result<(), String> {
    if paths.cert.exists() && paths.key.exists() {
        return Ok(());
    }
    if let Some(parent) = paths.cert.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let key_pair = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    // Les sondes joignent le hub par son nom ou par son IP selon le réseau :
    // on couvre les deux, sinon la moitié des déploiements échoue au SAN.
    let params = rcgen::CertificateParams::new(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ])
    .map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key_pair).map_err(|e| e.to_string())?;

    std::fs::write(&paths.cert, cert.pem()).map_err(|e| e.to_string())?;
    std::fs::write(&paths.key, key_pair.serialize_pem()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&paths.key, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Charge — en le générant au besoin — la configuration TLS du serveur.
pub fn server_config(paths: &TlsPaths) -> Result<Arc<rustls::ServerConfig>, String> {
    ensure_certificate(paths)?;

    let cert_pem = std::fs::read(&paths.cert).map_err(|e| e.to_string())?;
    let key_pem = std::fs::read(&paths.key).map_err(|e| e.to_string())?;
    let certs = rustls_pemfile::certs(&mut cert_pem.as_slice())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_slice())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "clé privée illisible".to_string())?;

    // Provider explicite : `ring` et `aws-lc-rs` sont tous deux dans l'arbre
    // via reqwest, rustls refuse alors de choisir seul.
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|e| e.to_string())?;
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lanprobe-web-tls-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn a_self_signed_certificate_is_generated_then_reused() {
        let dir = tmp_dir("generate");
        let paths = tls_paths(&dir);

        ensure_certificate(&paths).unwrap();
        assert!(paths.cert.exists() && paths.key.exists());
        let first = std::fs::read_to_string(&paths.cert).unwrap();

        // Un redémarrage ne doit pas régénérer : le navigateur redemanderait
        // à l'utilisateur d'accepter le certificat à chaque fois.
        ensure_certificate(&paths).unwrap();
        assert_eq!(std::fs::read_to_string(&paths.cert).unwrap(), first);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_private_key_is_not_world_readable() {
        let dir = tmp_dir("perms");
        let paths = tls_paths(&dir);
        ensure_certificate(&paths).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&paths.key).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "la clé privée doit être 0600");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_generated_material_builds_a_usable_server_config() {
        let dir = tmp_dir("config");
        let paths = tls_paths(&dir);
        assert!(server_config(&paths).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
