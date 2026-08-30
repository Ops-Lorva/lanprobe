//! Épinglage du certificat du hub.
//!
//! Remplace la case « accepter un certificat auto-signé », qui acceptait
//! **n'importe quel** certificat : n'importe qui sur le chemin pouvait se
//! faire passer pour le hub et récupérer le jeton de la sonde. Cocher cette
//! case ne disait pas « je fais confiance à MON hub », elle disait « je ne
//! vérifie plus rien ».
//!
//! L'épinglage dit la première chose. À la première connexion, la sonde
//! relève l'empreinte du certificat présenté, l'opérateur la confirme, et
//! toute connexion ultérieure n'accepte que celui-là.
//!
//! ⚠️ Une seule empreinte, celle du certificat exact. Un hub qui change de
//! certificat — passage d'un auto-signé à un vrai, ou régénération — cessera
//! d'être joignable jusqu'à ce que l'empreinte soit remise à jour. C'est le
//! prix de l'épinglage, et c'est le comportement voulu : accepter un
//! certificat nouveau sans rien demander reviendrait à ne rien épingler du
//! tout. Le hub, lui, conserve son certificat auto-signé entre deux
//! démarrages, précisément pour que ce cas reste rare.

use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

/// Empreinte SHA-256 au format `AB:CD:…` — celui qu'affichent les navigateurs
/// et `openssl x509 -fingerprint`, donc celui qu'un opérateur peut comparer à
/// l'œil sans outil.
pub fn fingerprint_of(der: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    digest
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Compare deux empreintes en ignorant la casse et les séparateurs.
///
/// L'opérateur recopie parfois l'empreinte depuis son navigateur, qui les
/// affiche tantôt en minuscules, tantôt sans les deux-points. Refuser sur la
/// forme plutôt que sur le contenu serait un refus incompréhensible.
pub fn same_fingerprint(a: &str, b: &str) -> bool {
    let normalize = |s: &str| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .flat_map(|c| c.to_uppercase())
            .collect::<String>()
    };
    let (a, b) = (normalize(a), normalize(b));
    !a.is_empty() && a == b
}

/// Vérificateur qui n'accepte qu'une empreinte connue.
///
/// Aucune logique d'autorité, de nom d'hôte ou de date : ce n'est pas un
/// oubli. Un certificat épinglé est reconnu par lui-même, et lui appliquer en
/// plus une vérification de chaîne rejetterait tous les auto-signés — c'est-à-
/// dire exactement le cas que l'épinglage existe pour couvrir.
#[derive(Debug)]
struct PinnedVerifier {
    expected: String,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let seen = fingerprint_of(end_entity.as_ref());
        if same_fingerprint(&seen, &self.expected) {
            return Ok(ServerCertVerified::assertion());
        }
        Err(rustls::Error::General(format!(
            "certificat inattendu : empreinte {seen}, attendue {}",
            self.expected
        )))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // ⚠️ La signature EST vérifiée, contrairement à l'ancienne case. Sans
        // ce contrôle, un intermédiaire qui rejoue le certificat du hub — il
        // est public — passerait l'épinglage sans posséder la clé privée.
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Vérificateur qui accepte tout et **retient** ce qu'il a vu.
///
/// Sert uniquement à la première connexion, pour pouvoir MONTRER l'empreinte à
/// l'opérateur. Aucune requête applicative ne passe par lui : le handshake est
/// ouvert puis abandonné.
#[derive(Debug)]
struct CaptureVerifier {
    seen: Mutex<Option<String>>,
}

impl ServerCertVerifier for CaptureVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Ok(mut guard) = self.seen.lock() {
            *guard = Some(fingerprint_of(end_entity.as_ref()));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _m: &[u8],
        _c: &CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _m: &[u8],
        _c: &CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn default_provider() -> Arc<rustls::crypto::CryptoProvider> {
    // `reqwest` installe le fournisseur par défaut ; s'il ne l'a pas encore
    // fait, on prend celui de la caisse plutôt que de paniquer au premier
    // handshake.
    rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
}

/// Configuration TLS qui n'accepte que le certificat épinglé.
pub fn pinned_tls_config(fingerprint: &str) -> rustls::ClientConfig {
    let mut config = rustls::ClientConfig::builder_with_provider(default_provider())
        .with_safe_default_protocol_versions()
        .expect("versions TLS par défaut")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedVerifier {
            expected: fingerprint.to_string(),
        }))
        .with_no_client_auth();
    // `reqwest` négocie HTTP/2 quand le serveur le propose ; sans ces
    // protocoles annoncés, la connexion retombe en HTTP/1.1 sans le dire.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    config
}

/// Ouvre un handshake vers `host:port` et rend l'empreinte présentée.
///
/// Le certificat n'est pas *validé* — c'est tout l'intérêt : on veut voir
/// celui d'un hub auto-signé, précisément celui qu'aucune autorité ne signe.
pub async fn capture_fingerprint(
    host: &str,
    port: u16,
    source: Option<std::net::Ipv4Addr>,
) -> Result<String, String> {
    let verifier = Arc::new(CaptureVerifier {
        seen: Mutex::new(None),
    });
    let config = rustls::ClientConfig::builder_with_provider(default_provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();

    let server_name = ServerName::try_from(host.to_string())
        .map_err(|_| format!("nom d'hôte invalide : {host}"))?;

    // ⚠️ Le lien choisi vaut aussi ici. Relever l'empreinte par une autre
    // interface que celle qui servira ensuite pourrait viser une machine
    // différente — et épingler le certificat de quelqu'un d'autre.
    let stream = match source {
        Some(ip) => {
            let socket = tokio::net::TcpSocket::new_v4().map_err(|e| e.to_string())?;
            socket
                .bind(std::net::SocketAddr::new(std::net::IpAddr::V4(ip), 0))
                .map_err(|e| format!("liaison à {ip} impossible : {e}"))?;
            let addr = tokio::net::lookup_host((host, port))
                .await
                .map_err(|e| e.to_string())?
                .find(|a| a.is_ipv4())
                .ok_or_else(|| format!("aucune adresse IPv4 pour {host}"))?;
            socket.connect(addr).await.map_err(|e| e.to_string())?
        }
        None => tokio::net::TcpStream::connect((host, port))
            .await
            .map_err(|e| e.to_string())?,
    };

    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    // Le handshake suffit : on n'envoie aucune requête, donc aucun secret.
    let _ = connector
        .connect(server_name, stream)
        .await
        .map_err(|e| format!("handshake TLS : {e}"))?;

    verifier
        .seen
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .ok_or_else(|| "aucun certificat présenté".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_reads_like_the_one_a_browser_shows() {
        let fp = fingerprint_of(b"nimporte quoi");
        assert_eq!(fp.len(), 32 * 3 - 1, "32 octets en hexa, séparés par « : »");
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit() || c == ':'));
    }

    #[test]
    fn comparing_ignores_case_and_separators() {
        // L'opérateur recopie depuis son navigateur, qui n'affiche pas tous la
        // même forme. Refuser sur la forme serait un refus incompréhensible.
        assert!(same_fingerprint("AB:CD:EF", "abcdef"));
        assert!(same_fingerprint("ab cd ef", "AB:CD:EF"));
        assert!(!same_fingerprint("AB:CD:EF", "AB:CD:E0"));
    }

    #[test]
    fn an_empty_fingerprint_never_matches() {
        // 🔴 Sans cette garde, une configuration où l'épingle a été perdue
        // accepterait le premier certificat venu : deux chaînes vides sont
        // égales.
        assert!(!same_fingerprint("", ""));
        assert!(!same_fingerprint("", "AB:CD"));
        assert!(!same_fingerprint("AB:CD", ""));
    }
}
