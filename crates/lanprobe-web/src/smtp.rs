//! Client SMTP minimal — juste de quoi poster une alerte.
//!
//! Pourquoi pas une bibliothèque : le hub a besoin d'envoyer un texte court à
//! quelques destinataires, une fois de temps en temps. Le protocole tient en
//! une dizaine de commandes, et `rustls` est déjà dans l'arbre pour Influx.
//! Une dépendance de plus à suivre, à auditer et à mettre à jour ne se
//! justifie pas pour ça.
//!
//! Ce qui est couvert : `EHLO`, `STARTTLS`, TLS implicite (port 465),
//! `AUTH LOGIN` et `AUTH PLAIN`, plusieurs destinataires, un corps UTF-8.
//! Ce qui ne l'est pas : pièces jointes, `AUTH` par OAuth, `DSN`, pipelining.
//! Le jour où l'un manque, il s'ajoute — mais pas avant qu'il manque.

use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::notify::{SmtpConfig, SmtpSecurity};

/// Un serveur qui ne répond pas ne doit pas immobiliser la boucle d'alerte.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Envoie un message. Rend une erreur lisible : elle est destinée à remonter
/// telle quelle dans le résultat du bouton de test.
pub async fn send(cfg: &SmtpConfig, subject: &str, body: &str) -> Result<(), String> {
    if cfg.host.trim().is_empty() {
        return Err("aucun serveur SMTP configuré".into());
    }
    if cfg.to.iter().all(|a| a.trim().is_empty()) {
        return Err("aucun destinataire configuré".into());
    }
    let message = compose(cfg, subject, body);
    tokio::time::timeout(TIMEOUT, deliver(cfg, message))
        .await
        .map_err(|_| format!("{}:{} n'a pas répondu à temps", cfg.host, cfg.port))?
}

async fn deliver(cfg: &SmtpConfig, message: String) -> Result<(), String> {
    let host = cfg.host.trim().trim_start_matches('[').trim_end_matches(']');
    let tcp = TcpStream::connect((host, cfg.port))
        .await
        .map_err(|e| format!("connexion à {host}:{} impossible : {e}", cfg.port))?;

    match cfg.security {
        SmtpSecurity::None => {
            let mut session = Session::new(tcp);
            session.greet().await?;
            session.ehlo().await?;
            session.deliver(cfg, &message).await
        }
        SmtpSecurity::Tls => {
            let tls = upgrade(tcp, host).await?;
            let mut session = Session::new(tls);
            session.greet().await?;
            session.ehlo().await?;
            session.deliver(cfg, &message).await
        }
        SmtpSecurity::Starttls => {
            let mut session = Session::new(tcp);
            session.greet().await?;
            session.ehlo().await?;
            session.command("STARTTLS", &[220]).await?;
            let tcp = session.into_inner()?;
            let tls = upgrade(tcp, host).await?;
            let mut session = Session::new(tls);
            // `EHLO` est rejoué obligatoirement : tout ce que le serveur avait
            // annoncé en clair est réputé non fiable.
            session.ehlo().await?;
            session.deliver(cfg, &message).await
        }
    }
}

async fn upgrade(
    tcp: TcpStream,
    host: &str,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, String> {
    use rustls_platform_verifier::BuilderVerifierExt;

    // Contrairement à Influx — joint en local sur un certificat auto-signé
    // dont le hub épingle l'empreinte — un relais SMTP est une machine
    // distante quelconque. On vérifie son certificat contre le magasin de la
    // plateforme : accepter n'importe quel certificat livrerait le mot de
    // passe SMTP au premier intermédiaire venu.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_platform_verifier()
        .map_err(|e| format!("magasin de certificats indisponible : {e}"))?
        .with_no_client_auth();

    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| format!("nom de serveur SMTP invalide : {e}"))?;
    tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(server_name, tcp)
        .await
        .map_err(|e| format!("handshake TLS avec {host} échoué : {e}"))
}

// ── La conversation ────────────────────────────────────────────────────────

struct Session<S> {
    stream: BufReader<S>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Session<S> {
    fn new(stream: S) -> Self {
        Self {
            stream: BufReader::new(stream),
        }
    }

    /// Récupère le flux nu pour le passer en TLS.
    ///
    /// **Le tampon doit être vide.** Des octets déjà reçus au moment du
    /// passage en TLS seraient des commandes injectées avant le handshake, et
    /// exécutées comme si elles venaient du canal chiffré — la faille
    /// classique de `STARTTLS`.
    fn into_inner(self) -> Result<S, String> {
        if !self.stream.buffer().is_empty() {
            return Err(
                "le serveur a envoyé des données avant le passage en TLS — connexion abandonnée"
                    .into(),
            );
        }
        Ok(self.stream.into_inner())
    }

    async fn read_reply(&mut self) -> Result<(u16, String), String> {
        let mut text = String::new();
        loop {
            let mut line = String::new();
            let read = self
                .stream
                .read_line(&mut line)
                .await
                .map_err(|e| format!("lecture SMTP interrompue : {e}"))?;
            if read == 0 {
                return Err("le serveur SMTP a coupé la connexion".into());
            }
            let line = line.trim_end_matches(['\r', '\n']).to_string();
            if line.len() < 3 {
                return Err(format!("réponse SMTP illisible : {line}"));
            }
            let code: u16 = line[..3]
                .parse()
                .map_err(|_| format!("réponse SMTP illisible : {line}"))?;
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(line[3..].trim_start_matches(['-', ' ']));
            // Une réponse multiligne sépare le code du texte par `-` ; le
            // dernier tour utilise une espace.
            if line.as_bytes().get(3) != Some(&b'-') {
                return Ok((code, text));
            }
        }
    }

    async fn write_line(&mut self, line: &str) -> Result<(), String> {
        self.stream
            .get_mut()
            .write_all(format!("{line}\r\n").as_bytes())
            .await
            .map_err(|e| format!("écriture SMTP impossible : {e}"))
    }

    async fn greet(&mut self) -> Result<(), String> {
        expect(self.read_reply().await?, &[220], "accueil")
    }

    async fn ehlo(&mut self) -> Result<(), String> {
        // Le nom annoncé n'a pas à être résoluble ; les relais qui l'exigent
        // se configurent avec un compte, pas avec un `HELO` crédible.
        self.command("EHLO lanprobe", &[250]).await
    }

    async fn command(&mut self, line: &str, expected: &[u16]) -> Result<(), String> {
        self.write_line(line).await?;
        // Le mot de commande seul : `AUTH LOGIN` ne doit pas faire figurer le
        // mot de passe dans un message d'erreur.
        let label = line.split(' ').next().unwrap_or(line).to_string();
        expect(self.read_reply().await?, expected, &label)
    }

    async fn deliver(&mut self, cfg: &SmtpConfig, message: &str) -> Result<(), String> {
        if let Some(username) = cfg.username.as_deref().filter(|u| !u.trim().is_empty()) {
            let password = cfg.password.clone().unwrap_or_default();
            self.authenticate(username, &password).await?;
        }
        self.command(&format!("MAIL FROM:<{}>", cfg.from.trim()), &[250])
            .await?;
        for recipient in cfg.to.iter().filter(|a| !a.trim().is_empty()) {
            self.command(&format!("RCPT TO:<{}>", recipient.trim()), &[250, 251])
                .await?;
        }
        self.command("DATA", &[354]).await?;
        self.stream
            .get_mut()
            .write_all(message.as_bytes())
            .await
            .map_err(|e| format!("écriture SMTP impossible : {e}"))?;
        self.write_line(".").await?;
        expect(self.read_reply().await?, &[250], "corps du message")?;
        // Un `QUIT` qui échoue ne remet pas en cause un message déjà accepté.
        let _ = self.command("QUIT", &[221]).await;
        Ok(())
    }

    /// `AUTH LOGIN` d'abord — le plus répandu — puis `AUTH PLAIN` en repli.
    async fn authenticate(&mut self, username: &str, password: &str) -> Result<(), String> {
        self.write_line("AUTH LOGIN").await?;
        let (code, _) = self.read_reply().await?;
        if code == 334 {
            self.write_line(&B64.encode(username)).await?;
            let (code, text) = self.read_reply().await?;
            if code != 334 {
                return Err(format!("authentification SMTP refusée ({code}) : {text}"));
            }
            self.write_line(&B64.encode(password)).await?;
            return expect(self.read_reply().await?, &[235], "authentification");
        }
        // `\0user\0password`, tel que le veut la RFC 4616.
        let plain = format!("\0{username}\0{password}");
        self.write_line(&format!("AUTH PLAIN {}", B64.encode(plain)))
            .await?;
        expect(self.read_reply().await?, &[235], "authentification")
    }
}

fn expect((code, text): (u16, String), expected: &[u16], step: &str) -> Result<(), String> {
    if expected.contains(&code) {
        return Ok(());
    }
    Err(format!("{step} : le serveur SMTP a répondu {code} — {text}"))
}

// ── Le message ─────────────────────────────────────────────────────────────

/// Compose le message complet, corps en base64.
///
/// Encoder le corps évite deux pièges d'un coup : les accents, qu'un relais
/// 7 bits mutilerait, et le « dot stuffing » — une ligne réduite à un point
/// terminerait le message au milieu. Une sortie base64 ne contient ni l'un ni
/// l'autre.
fn compose(cfg: &SmtpConfig, subject: &str, body: &str) -> String {
    let recipients: Vec<&str> = cfg
        .to
        .iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .collect();
    let mut message = String::new();
    message.push_str(&format!("From: {}\r\n", cfg.from.trim()));
    message.push_str(&format!("To: {}\r\n", recipients.join(", ")));
    message.push_str(&format!("Subject: {}\r\n", encode_header(subject)));
    message.push_str(&format!("Date: {}\r\n", rfc5322_date(crate::db::now())));
    message.push_str(&format!(
        "Message-ID: <{}@lanprobe>\r\n",
        lanprobe_core::passwords::generate_token()
    ));
    message.push_str("MIME-Version: 1.0\r\n");
    message.push_str("Content-Type: text/plain; charset=utf-8\r\n");
    message.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
    for chunk in wrap(&B64.encode(body), 76) {
        message.push_str(&chunk);
        message.push_str("\r\n");
    }
    message
}

/// Un en-tête ne transporte que de l'ASCII : « Paris est passée hors ligne »
/// arriverait haché. RFC 2047, mot encodé en base64.
fn encode_header(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    format!("=?UTF-8?B?{}?=", B64.encode(value))
}

fn wrap(value: &str, width: usize) -> Vec<String> {
    value
        .as_bytes()
        .chunks(width)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect()
}

/// Époque Unix → date RFC 5322, en UTC.
///
/// Le hub n'a pas de bibliothèque de dates et n'en veut pas pour un seul
/// en-tête. Un message sans `Date` est mal formé et se fait classer en
/// indésirable par une partie des relais — c'est trop cher payé pour une
/// ligne.
fn rfc5322_date(epoch: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    // Algorithme des jours civils de Howard Hinnant, dans l'autre sens que
    // celui de `web.rs`.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} +0000",
        DAYS[days.rem_euclid(7) as usize],
        d,
        MONTHS[(m - 1) as usize],
        y,
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    /// Faux relais SMTP en clair. Il rejoue une conversation complète et garde
    /// tout ce qu'il a reçu : les tests assertent sur les commandes réellement
    /// émises, pas sur un bouchon qui se contenterait de dire oui.
    pub(crate) struct FakeSmtp {
        pub(crate) host: String,
        pub(crate) port: u16,
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl FakeSmtp {
        pub(crate) async fn start(require_auth: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let seen = Arc::new(Mutex::new(Vec::new()));

            let recorded = seen.clone();
            tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    let recorded = recorded.clone();
                    tokio::spawn(async move { serve(socket, recorded, require_auth).await });
                }
            });

            Self {
                host: "127.0.0.1".into(),
                port,
                seen,
            }
        }

        pub(crate) fn seen(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }

        pub(crate) fn config(&self) -> SmtpConfig {
            SmtpConfig {
                host: self.host.clone(),
                port: self.port,
                security: SmtpSecurity::None,
                username: None,
                password: None,
                from: "hub@lanprobe.test".into(),
                to: vec!["ops@example.org".into()],
            }
        }
    }

    async fn serve(socket: TcpStream, seen: Arc<Mutex<Vec<String>>>, require_auth: bool) {
        let mut stream = BufReader::new(socket);
        let _ = stream
            .get_mut()
            .write_all(b"220 fake.lanprobe.test ESMTP\r\n")
            .await;

        let mut in_data = false;
        loop {
            let mut line = String::new();
            match stream.read_line(&mut line).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let line = line.trim_end_matches(['\r', '\n']).to_string();
            seen.lock().unwrap().push(line.clone());

            if in_data {
                if line == "." {
                    in_data = false;
                    let _ = stream.get_mut().write_all(b"250 OK\r\n").await;
                }
                continue;
            }

            let reply: &[u8] = if line.starts_with("EHLO") {
                if require_auth {
                    b"250-fake.lanprobe.test\r\n250 AUTH LOGIN PLAIN\r\n"
                } else {
                    b"250 fake.lanprobe.test\r\n"
                }
            } else if line == "AUTH LOGIN" {
                b"334 VXNlcm5hbWU6\r\n"
            } else if line.starts_with("AUTH PLAIN") {
                b"235 OK\r\n"
            } else if line.starts_with("MAIL FROM") || line.starts_with("RCPT TO") {
                b"250 OK\r\n"
            } else if line == "DATA" {
                in_data = true;
                b"354 Go ahead\r\n"
            } else if line == "QUIT" {
                let _ = stream.get_mut().write_all(b"221 Bye\r\n").await;
                return;
            } else if line == "RSET" {
                b"250 OK\r\n"
            } else {
                // Les deux tours en base64 de `AUTH LOGIN` : identifiant puis
                // mot de passe.
                let previous = {
                    let guard = seen.lock().unwrap();
                    guard.iter().rev().nth(1).cloned().unwrap_or_default()
                };
                if previous == "AUTH LOGIN" {
                    b"334 UGFzc3dvcmQ6\r\n"
                } else {
                    b"235 OK\r\n"
                }
            };
            let _ = stream.get_mut().write_all(reply).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeSmtp;
    use super::*;

    #[tokio::test]
    async fn a_message_walks_the_whole_conversation() {
        let fake = FakeSmtp::start(false).await;
        send(&fake.config(), "Sujet", "Corps du message").await.unwrap();

        let seen = fake.seen();
        let commands: Vec<&String> = seen.iter().collect();
        assert!(commands.iter().any(|l| l.starts_with("EHLO")), "{seen:?}");
        assert!(
            commands
                .iter()
                .any(|l| l.as_str() == "MAIL FROM:<hub@lanprobe.test>"),
            "{seen:?}"
        );
        assert!(
            commands
                .iter()
                .any(|l| l.as_str() == "RCPT TO:<ops@example.org>"),
            "{seen:?}"
        );
        assert!(commands.iter().any(|l| l.as_str() == "DATA"), "{seen:?}");
        assert!(commands.iter().any(|l| l.as_str() == "."), "{seen:?}");
        assert!(commands.iter().any(|l| l.as_str() == "QUIT"), "{seen:?}");
    }

    #[tokio::test]
    async fn every_recipient_gets_its_own_rcpt() {
        let fake = FakeSmtp::start(false).await;
        let mut cfg = fake.config();
        cfg.to = vec![
            "un@example.org".into(),
            "  ".into(),
            "deux@example.org".into(),
        ];
        send(&cfg, "Sujet", "Corps").await.unwrap();

        let seen = fake.seen();
        let rcpts: Vec<&String> = seen.iter().filter(|l| l.starts_with("RCPT TO")).collect();
        assert_eq!(rcpts.len(), 2, "l'adresse vide ne doit pas être envoyée : {seen:?}");
    }

    #[tokio::test]
    async fn credentials_trigger_an_auth_exchange() {
        let fake = FakeSmtp::start(true).await;
        let mut cfg = fake.config();
        cfg.username = Some("hub".into());
        cfg.password = Some("secret-smtp".into());
        send(&cfg, "Sujet", "Corps").await.unwrap();

        let seen = fake.seen();
        assert!(seen.iter().any(|l| l == "AUTH LOGIN"), "{seen:?}");
        // Le mot de passe voyage en base64 — ce n'est pas du chiffrement, et
        // c'est précisément pourquoi le défaut du réglage est STARTTLS.
        assert!(
            seen.iter().any(|l| l == &B64.encode("secret-smtp")),
            "{seen:?}"
        );
        assert!(
            !seen.iter().any(|l| l.contains("secret-smtp")),
            "le mot de passe ne doit pas partir en clair : {seen:?}"
        );
    }

    #[tokio::test]
    async fn a_missing_recipient_is_refused_before_connecting() {
        let mut cfg = FakeSmtp::start(false).await.config();
        cfg.to = vec![];
        let err = send(&cfg, "Sujet", "Corps").await.unwrap_err();
        assert!(err.contains("destinataire"), "{err}");
    }

    #[tokio::test]
    async fn an_unreachable_server_gives_a_readable_error() {
        // Le message remonte tel quel dans le résultat du bouton de test.
        let cfg = SmtpConfig {
            host: "127.0.0.1".into(),
            // Port réservé, rien n'écoute.
            port: 1,
            security: SmtpSecurity::None,
            username: None,
            password: None,
            from: "hub@lanprobe.test".into(),
            to: vec!["ops@example.org".into()],
        };
        let err = send(&cfg, "Sujet", "Corps").await.unwrap_err();
        assert!(err.contains("127.0.0.1:1"), "{err}");
    }

    #[test]
    fn the_body_is_encoded_so_accents_and_lone_dots_survive() {
        let cfg = SmtpConfig {
            host: "x".into(),
            port: 25,
            security: SmtpSecurity::None,
            username: None,
            password: None,
            from: "hub@lanprobe.test".into(),
            to: vec!["ops@example.org".into()],
        };
        let message = compose(&cfg, "Sujet", "Paris est passée hors ligne\r\n.\r\nsuite");

        assert!(message.contains("Content-Transfer-Encoding: base64"));
        // Une ligne réduite à un point terminerait le message au milieu.
        assert!(
            !message
                .split("\r\n\r\n")
                .nth(1)
                .unwrap()
                .lines()
                .any(|l| l == "."),
            "{message}"
        );
        let body = message.split("\r\n\r\n").nth(1).unwrap().replace("\r\n", "");
        let decoded = String::from_utf8(B64.decode(body).unwrap()).unwrap();
        assert!(decoded.contains("passée"));
    }

    #[test]
    fn an_accented_subject_is_encoded_for_the_header() {
        let cfg = SmtpConfig {
            host: "x".into(),
            port: 25,
            security: SmtpSecurity::None,
            username: None,
            password: None,
            from: "hub@lanprobe.test".into(),
            to: vec!["ops@example.org".into()],
        };
        let message = compose(&cfg, "Sonde arrêtée", "Corps");
        assert!(message.contains("Subject: =?UTF-8?B?"), "{message}");
        assert!(!message.contains("Subject: Sonde arrêtée"), "{message}");

        assert_eq!(encode_header("Tout en ASCII"), "Tout en ASCII");
    }

    #[test]
    fn the_date_header_matches_known_epochs() {
        assert_eq!(rfc5322_date(0), "Thu, 01 Jan 1970 00:00:00 +0000");
        assert_eq!(rfc5322_date(1_709_208_000), "Thu, 29 Feb 2024 12:00:00 +0000");
        assert_eq!(rfc5322_date(1_787_788_800), "Thu, 27 Aug 2026 00:00:00 +0000");
    }

    #[tokio::test]
    async fn a_multiline_reply_is_read_to_its_last_line() {
        // Un `EHLO` répond en plusieurs lignes ; s'arrêter à la première
        // désynchroniserait tout le reste de la conversation.
        let script = b"250-fake.lanprobe.test\r\n250-PIPELINING\r\n250 AUTH LOGIN\r\n".to_vec();
        let mut session = Session::new(std::io::Cursor::new(script));
        let (code, text) = session.read_reply().await.unwrap();
        assert_eq!(code, 250);
        assert!(text.contains("AUTH LOGIN"), "{text}");
    }
}
