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

/// Empreinte SHA-256 du certificat du hub, au format `AB:CD:…` — celui
/// qu'affichent les navigateurs et `openssl x509 -fingerprint`, donc celui
/// qu'un opérateur peut comparer à l'œil.
///
/// 🔴 **C'est ce que le QR d'appairage transporte** (contrat § 22). Le
/// téléphone épingle alors le certificat auto-signé du hub : l'empreinte lui
/// arrive par un canal que l'utilisateur contrôle physiquement — l'écran qu'il
/// a sous les yeux — plutôt que par le réseau qu'on cherche justement à
/// authentifier. C'est déjà le motif que le hub emploie pour épingler le
/// certificat d'Influx (contrat § 7).
///
/// ⚠️ **Elle ne bouge pas d'un démarrage à l'autre**, parce que le certificat
/// est conservé. Un certificat régénéré à chaque démarrage invaliderait celui
/// qu'ont épinglé tous les téléphones appairés — c'est-à-dire produirait très
/// exactement la panne que l'appairage existe pour ne jamais produire.
///
/// ⚠️ Elle est lue **sur le certificat servi**, pas sur une valeur en base :
/// une empreinte stockée survivrait au fichier qu'elle décrit.
pub fn fingerprint_sha256(paths: &TlsPaths) -> Result<String, String> {
    ensure_certificate(paths)?;
    let cert_pem = std::fs::read(&paths.cert).map_err(|e| e.to_string())?;
    let der = rustls_pemfile::certs(&mut cert_pem.as_slice())
        .next()
        .ok_or_else(|| "aucun certificat dans le fichier".to_string())?
        .map_err(|e| e.to_string())?;
    let digest = ring::digest::digest(&ring::digest::SHA256, der.as_ref());
    Ok(digest
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":"))
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
    fn the_certificate_carries_a_readable_fingerprint_that_does_not_move() {
        // ⚠️ C'est cette empreinte que le QR d'appairage porte (contrat § 22) :
        // elle arrive au téléphone par un canal que l'utilisateur contrôle
        // physiquement — l'écran qu'il a sous les yeux. Épingler vaut mieux que
        // désactiver la vérification : on garde l'authentification du serveur
        // sans exiger une autorité que personne n'a sur un LAN.
        let dir = tmp_dir("fingerprint");
        let paths = tls_paths(&dir);

        let empreinte = fingerprint_sha256(&paths).unwrap();
        // Le format des navigateurs et d'`openssl x509 -fingerprint` : c'est
        // celui qu'un opérateur peut comparer à l'œil.
        let octets: Vec<&str> = empreinte.split(':').collect();
        assert_eq!(octets.len(), 32, "SHA-256 = 32 octets : {empreinte}");
        assert!(
            octets.iter().all(|o| o.len() == 2 && o.chars().all(|c| c.is_ascii_hexdigit())),
            "{empreinte}"
        );

        // 🔴 Elle ne bouge pas d'un démarrage à l'autre. Sinon chaque
        // redémarrage du conteneur invaliderait le certificat épinglé par tous
        // les téléphones appairés — c'est-à-dire exactement la panne que
        // l'appairage existe pour ne jamais produire.
        assert_eq!(fingerprint_sha256(&paths).unwrap(), empreinte);

        let autre = tmp_dir("fingerprint-2");
        assert_ne!(
            fingerprint_sha256(&tls_paths(&autre)).unwrap(),
            empreinte,
            "deux certificats distincts ne peuvent pas partager une empreinte"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&autre);
    }

    #[test]
    fn the_generated_material_builds_a_usable_server_config() {
        let dir = tmp_dir("config");
        let paths = tls_paths(&dir);
        assert!(server_config(&paths).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
