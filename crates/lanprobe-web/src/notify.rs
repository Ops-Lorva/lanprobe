//! Notifications : deux canaux, et un déclencheur qui se tait.
//!
//! ## Deux canaux, pas cinq
//!
//! SMTP et un **webhook générique**. Slack, Discord, Teams, ntfy, Gotify
//! acceptent tous un `POST` JSON : un webhook paramétrable avec deux modèles
//! préremplis couvre l'écosystème entier, et le cas maison ou Mattermost
//! marche sans toucher au code. Cinq intégrations dédiées auraient cinq fois
//! plus de surface à maintenir pour le même résultat.
//!
//! ## Le difficile n'est pas le canal, c'est le déclencheur
//!
//! Notifier « sonde hors ligne » en continu condamne la fonctionnalité : un
//! portable qu'on referme alerterait à chaque passage de la boucle, et trois
//! alertes inutiles suffisent pour que plus personne ne les lise — le jour où
//! un vrai site tombe, personne ne le voit.
//!
//! 1. **Transitions, pas états.** L'état annoncé est mémorisé en base ; on
//!    n'émet que lorsqu'il change.
//! 2. **Délai avant alerte**, cinq minutes par défaut, réglable. Une sonde qui
//!    redémarre ne réveille personne.
//! 3. **Le retour est notifié aussi.** Une alerte sans son « c'est revenu »
//!    oblige à aller vérifier à la main, donc on cesse de les lire.
//! 4. **Activable par site, héritée par ses sondes, avec exception par sonde.**
//!
//! ## Les secrets ne sortent pas d'ici
//!
//! Mot de passe SMTP et URL de webhook vivent scellés (`secrets.rs`) et ne
//! sont rendus par aucune route. L'interface sait « configuré » ou « non
//! configuré », et dispose d'un bouton de test qui envoie un vrai message.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::db::{AlertState, Db, DbResult};
use crate::secrets::{keys, Secrets};

/// Silence toléré avant de déclarer une sonde hors ligne. Cinq minutes : une
/// sonde qui redémarre, ou un lot de mesures qui prend du retard, ne réveille
/// personne.
pub const DEFAULT_DELAY_SECS: i64 = 300;

// ── Ce qu'on annonce ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    Down,
    Up,
}

impl AlertKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AlertKind::Down => "probe.down",
            AlertKind::Up => "probe.up",
        }
    }
}

/// Une transition à annoncer. Jamais un état : un état se lit dans
/// l'interface, une notification sert à dire ce qui a changé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub kind: AlertKind,
    pub probe_id: String,
    pub probe: String,
    pub site_id: String,
    pub site: String,
    /// Date du dernier signe de vie.
    pub last_seen: i64,
    /// Durée du silence au moment où l'alerte part. `0` pour un retour.
    pub silence_secs: i64,
}

impl Alert {
    pub fn subject(&self) -> String {
        match self.kind {
            AlertKind::Down => format!("LanProbe — {} ({}) hors ligne", self.probe, self.site),
            AlertKind::Up => format!("LanProbe — {} ({}) de retour", self.probe, self.site),
        }
    }

    pub fn message(&self) -> String {
        match self.kind {
            AlertKind::Down => format!(
                "{} ({}) est passée hors ligne — dernier contact il y a {}.",
                self.probe,
                self.site,
                humanise(self.silence_secs)
            ),
            AlertKind::Up => format!("{} ({}) est revenue en ligne.", self.probe, self.site),
        }
    }
}

/// Durée en français, arrondie. On ne parle pas de secondes à quelqu'un qu'on
/// réveille : « il y a 7 min » se lit, « il y a 431 s » se recalcule.
fn humanise(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 120 {
        return format!("{secs} s");
    }
    let minutes = secs / 60;
    if minutes < 120 {
        return format!("{minutes} min");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{hours} h");
    }
    format!("{} j", hours / 24)
}

// ── Le déclencheur ─────────────────────────────────────────────────────────

/// Passe le parc en revue et rend les **transitions** à annoncer, en mettant
/// à jour l'état mémorisé au passage.
///
/// Trois cas se taisent volontairement :
///
/// - une sonde **jamais vue** : on ne revient pas d'un état où l'on n'a jamais
///   été, et une machine enrôlée qu'on n'a pas encore branchée n'est pas une
///   panne ;
/// - une sonde **révoquée** : elle a cessé d'émettre parce qu'on le lui a
///   demandé ;
/// - la **première évaluation** d'une sonde : activer les alertes ne rejoue
///   pas les pannes en cours. L'interface les montre déjà ; ce qu'on attend
///   d'une notification, c'est ce qui change après.
pub fn evaluate(db: &Db, now: i64, delay_secs: i64) -> DbResult<Vec<Alert>> {
    let delay = delay_secs.max(1);
    let mut alerts = Vec::new();

    for probe in db.list_probes(None)? {
        if probe.revoked_at.is_some() {
            continue;
        }
        let Some(last_seen) = probe.last_seen else {
            continue;
        };
        if !db.notify_enabled_for_probe(&probe.probe_id)? {
            continue;
        }

        let silence = (now - last_seen).max(0);
        let observed = if silence <= delay {
            AlertState::Up
        } else {
            AlertState::Down
        };

        match db.notify_state(&probe.probe_id)? {
            // Première évaluation : on retient l'état, on n'annonce rien.
            None => db.set_notify_state(&probe.probe_id, observed, now)?,
            Some((known, _)) if known == observed => {}
            Some(_) => {
                db.set_notify_state(&probe.probe_id, observed, now)?;
                alerts.push(Alert {
                    kind: match observed {
                        AlertState::Up => AlertKind::Up,
                        AlertState::Down => AlertKind::Down,
                    },
                    probe_id: probe.probe_id.clone(),
                    probe: probe.name.clone(),
                    site_id: probe.site_id.clone(),
                    site: probe.site_name.clone(),
                    last_seen,
                    silence_secs: match observed {
                        AlertState::Down => silence,
                        AlertState::Up => 0,
                    },
                });
            }
        }
    }
    Ok(alerts)
}

// ── Les canaux ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookTemplate {
    /// Corps JSON complet : le cas maison, Mattermost, ntfy, Gotify.
    #[default]
    Generic,
    /// `{"text": "…"}`
    Slack,
    /// `{"content": "…"}`
    Discord,
}

impl WebhookTemplate {
    pub fn as_str(&self) -> &'static str {
        match self {
            WebhookTemplate::Generic => "generic",
            WebhookTemplate::Slack => "slack",
            WebhookTemplate::Discord => "discord",
        }
    }
}

/// **L'URL est un secret porteur** : qui la connaît peut écrire dans le salon.
/// Elle vit scellée et ne sort par aucune route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub url: String,
    #[serde(default)]
    pub template: WebhookTemplate,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurity {
    /// Aucun chiffrement — un relais local, et rien d'autre.
    None,
    /// Port 587 : on ouvre en clair puis on passe en TLS. Le cas courant.
    #[default]
    Starttls,
    /// Port 465 : TLS dès la connexion.
    Tls,
}

impl SmtpSecurity {
    pub fn as_str(&self) -> &'static str {
        match self {
            SmtpSecurity::None => "none",
            SmtpSecurity::Starttls => "starttls",
            SmtpSecurity::Tls => "tls",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub security: SmtpSecurity,
    #[serde(default)]
    pub username: Option<String>,
    /// Le seul champ vraiment secret de la configuration SMTP — et la raison
    /// pour laquelle elle est scellée en entier plutôt que rangée dans
    /// `settings`.
    #[serde(default)]
    pub password: Option<String>,
    pub from: String,
    pub to: Vec<String>,
}

/// Corps du webhook. Le modèle décide de la forme, jamais du contenu : le même
/// texte part partout, seule son enveloppe change.
pub fn webhook_payload(template: WebhookTemplate, alert: &Alert) -> serde_json::Value {
    let message = alert.message();
    match template {
        WebhookTemplate::Slack => serde_json::json!({ "text": message }),
        WebhookTemplate::Discord => serde_json::json!({ "content": message }),
        WebhookTemplate::Generic => serde_json::json!({
            "event": alert.kind.as_str(),
            "probe_id": alert.probe_id,
            "probe": alert.probe,
            "site_id": alert.site_id,
            "site": alert.site,
            "last_seen": alert.last_seen,
            "silence_secs": alert.silence_secs,
            "message": message,
        }),
    }
}

// ── L'émetteur ─────────────────────────────────────────────────────────────

/// Résultat d'un envoi, canal par canal. Un canal en panne ne doit pas
/// empêcher l'autre de parler : c'est précisément le jour où l'un tombe qu'on
/// a besoin de l'autre.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelOutcome {
    pub channel: &'static str,
    pub attempted: bool,
    pub ok: bool,
    pub error: Option<String>,
}

impl ChannelOutcome {
    fn skipped(channel: &'static str) -> Self {
        Self {
            channel,
            attempted: false,
            ok: false,
            error: None,
        }
    }

    fn from(channel: &'static str, result: Result<(), String>) -> Self {
        Self {
            channel,
            attempted: true,
            ok: result.is_ok(),
            error: result.err(),
        }
    }
}

#[derive(Clone)]
pub struct Notifier {
    db: Arc<Db>,
    secrets: Secrets,
    settings: crate::settings::Settings,
    http: reqwest::Client,
}

impl Notifier {
    pub fn new(db: Arc<Db>, secrets: Secrets, settings: crate::settings::Settings) -> Self {
        Self {
            db,
            secrets,
            settings,
            // Un webhook qui ne répond pas ne doit pas immobiliser la boucle :
            // sans délai maximal, un salon injoignable retiendrait le hub
            // jusqu'au timeout du système.
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn webhook_config(&self) -> Option<WebhookConfig> {
        self.secrets.get_json(keys::WEBHOOK)
    }

    pub fn smtp_config(&self) -> Option<SmtpConfig> {
        self.secrets.get_json(keys::SMTP)
    }

    /// Ce que l'API a le droit de dire d'une configuration : qu'elle existe.
    /// **Ni l'URL du webhook, ni le mot de passe SMTP n'en font partie** — les
    /// champs SMTP non secrets sont rendus pour que l'interface puisse
    /// afficher le formulaire sans obliger à tout retaper.
    pub fn status(&self) -> serde_json::Value {
        let smtp = self.smtp_config();
        let webhook = self.webhook_config();
        serde_json::json!({
            "delay_secs": self.settings.notify_delay_secs(),
            "smtp": match &smtp {
                Some(cfg) => serde_json::json!({
                    "configured": true,
                    "host": cfg.host,
                    "port": cfg.port,
                    "security": cfg.security.as_str(),
                    "username": cfg.username,
                    "password_set": cfg.password.as_deref().is_some_and(|p| !p.is_empty()),
                    "from": cfg.from,
                    "to": cfg.to,
                }),
                None => serde_json::json!({
                    "configured": false,
                    "unreadable": self.secrets.is_set(keys::SMTP),
                }),
            },
            "webhook": match &webhook {
                // L'URL n'est jamais rendue : elle porte à elle seule le droit
                // d'écrire dans le salon.
                Some(cfg) => serde_json::json!({
                    "configured": true,
                    "template": cfg.template.as_str(),
                }),
                None => serde_json::json!({
                    "configured": false,
                    "unreadable": self.secrets.is_set(keys::WEBHOOK),
                }),
            },
        })
    }

    /// Envoie une alerte sur tous les canaux configurés.
    pub async fn dispatch(&self, alert: &Alert) -> Vec<ChannelOutcome> {
        let mut outcomes = Vec::with_capacity(2);

        outcomes.push(match self.webhook_config() {
            Some(cfg) => ChannelOutcome::from("webhook", self.post_webhook(&cfg, alert).await),
            None => ChannelOutcome::skipped("webhook"),
        });
        outcomes.push(match self.smtp_config() {
            Some(cfg) => ChannelOutcome::from(
                "smtp",
                crate::smtp::send(&cfg, &alert.subject(), &alert.message()).await,
            ),
            None => ChannelOutcome::skipped("smtp"),
        });
        outcomes
    }

    async fn post_webhook(&self, cfg: &WebhookConfig, alert: &Alert) -> Result<(), String> {
        let response = self
            .http
            .post(&cfg.url)
            .json(&webhook_payload(cfg.template, alert))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        // Le corps de l'erreur est celui du salon distant : il dit « channel
        // not found » là où le code seul ne dirait que « 404 ».
        let body = response.text().await.unwrap_or_default();
        Err(format!(
            "le webhook a répondu {status}{}",
            if body.trim().is_empty() {
                String::new()
            } else {
                format!(" : {}", body.trim().chars().take(200).collect::<String>())
            }
        ))
    }

    /// Un tour de boucle : évaluer, annoncer, journaliser. Rend le nombre de
    /// transitions annoncées.
    pub async fn run_once(&self, now: i64) -> usize {
        let delay = self.settings.notify_delay_secs();
        let alerts = match evaluate(&self.db, now, delay) {
            Ok(alerts) => alerts,
            Err(e) => {
                tracing::error!("évaluation des alertes impossible : {e}");
                return 0;
            }
        };
        for alert in &alerts {
            let outcomes = self.dispatch(alert).await;
            // L'audit dit qu'une alerte est partie et sur quel canal, jamais
            // vers quelle adresse : la destination est le secret.
            let detail = outcomes
                .iter()
                .filter(|o| o.attempted)
                .map(|o| format!("{}:{}", o.channel, if o.ok { "ok" } else { "échec" }))
                .collect::<Vec<_>>()
                .join(" ");
            let outcome = if outcomes.iter().any(|o| o.attempted && o.ok) {
                crate::db::Outcome::Success
            } else {
                crate::db::Outcome::Failure
            };
            let detail = if detail.is_empty() {
                "aucun canal configuré".to_string()
            } else {
                detail
            };
            if let Err(e) = self.db.record_audit(
                None,
                alert.kind.as_str(),
                Some(&alert.probe_id),
                outcome,
                Some(&detail),
            ) {
                tracing::error!("ligne d'audit d'alerte non écrite : {e}");
            }
        }
        alerts.len()
    }

    /// Message de test, envoyé pour de vrai. Une configuration qu'on ne peut
    /// pas essayer se découvre fausse le jour de la première panne.
    pub async fn send_test(&self) -> Vec<ChannelOutcome> {
        let alert = Alert {
            kind: AlertKind::Up,
            probe_id: "test".into(),
            probe: "Sonde de test".into(),
            site_id: "test".into(),
            site: "LanProbe".into(),
            last_seen: crate::db::now(),
            silence_secs: 0,
        };
        self.dispatch(&alert).await
    }
}

/// Boucle de fond. Le pas est court devant le délai d'alerte : c'est lui qui
/// décide de la réactivité, pas la cadence de la boucle.
pub async fn run(notifier: Notifier, step: std::time::Duration) {
    loop {
        tokio::time::sleep(step).await;
        notifier.run_once(crate::db::now()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELAY: i64 = 300;

    /// Une base avec un site abonné et une sonde qui vient de battre.
    fn parc() -> (Arc<Db>, String, i64) {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        db.set_notify_subscription("site", &site.site_id, Some(true))
            .unwrap();
        db.record_heartbeat(&probe.probe_id, None, None, 0).unwrap();
        let seen = db.get_probe(&probe.probe_id).unwrap().last_seen.unwrap();
        (db, probe.probe_id, seen)
    }

    #[test]
    fn the_first_pass_records_the_state_without_announcing_it() {
        // Activer les alertes ne rejoue pas ce qui est déjà à l'écran.
        let (db, probe_id, seen) = parc();
        assert!(evaluate(&db, seen, DELAY).unwrap().is_empty());
        assert_eq!(
            db.notify_state(&probe_id).unwrap().map(|(s, _)| s),
            Some(AlertState::Up)
        );
    }

    #[test]
    fn the_delay_is_respected_before_calling_a_probe_offline() {
        // Une sonde qui redémarre ne réveille personne.
        let (db, _, seen) = parc();
        evaluate(&db, seen, DELAY).unwrap();

        assert!(
            evaluate(&db, seen + DELAY, DELAY).unwrap().is_empty(),
            "au délai exact, la sonde est encore en ligne"
        );
        assert!(
            evaluate(&db, seen + DELAY - 1, DELAY).unwrap().is_empty(),
            "avant le délai, rien"
        );
    }

    #[test]
    fn going_offline_is_announced_once_and_only_once() {
        let (db, probe_id, seen) = parc();
        evaluate(&db, seen, DELAY).unwrap();

        let alerts = evaluate(&db, seen + DELAY + 1, DELAY).unwrap();
        assert_eq!(alerts.len(), 1, "{alerts:?}");
        assert_eq!(alerts[0].kind, AlertKind::Down);
        assert_eq!(alerts[0].probe, "Paris");
        assert_eq!(alerts[0].site, "Durand");

        // Tant que l'état ne change pas, on se tait : un rappel par minute et
        // l'utilisateur coupe tout.
        for step in [2, 60, 3_600, 86_400] {
            assert!(
                evaluate(&db, seen + DELAY + step, DELAY).unwrap().is_empty(),
                "rappel émis au bout de {step} s"
            );
        }
        assert_eq!(
            db.notify_state(&probe_id).unwrap().map(|(s, _)| s),
            Some(AlertState::Down)
        );
    }

    #[test]
    fn coming_back_is_announced_too() {
        // Une alerte sans son « c'est revenu » oblige à aller vérifier à la
        // main, donc on cesse de les lire.
        let (db, probe_id, seen) = parc();
        evaluate(&db, seen, DELAY).unwrap();
        evaluate(&db, seen + DELAY + 1, DELAY).unwrap();

        db.record_heartbeat(&probe_id, None, None, 0).unwrap();
        let back = db.get_probe(&probe_id).unwrap().last_seen.unwrap();

        let alerts = evaluate(&db, back, DELAY).unwrap();
        assert_eq!(alerts.len(), 1, "{alerts:?}");
        assert_eq!(alerts[0].kind, AlertKind::Up);
        assert_eq!(alerts[0].silence_secs, 0);

        assert!(
            evaluate(&db, back + 1, DELAY).unwrap().is_empty(),
            "le retour ne se répète pas non plus"
        );
    }

    #[test]
    fn a_site_that_is_not_subscribed_stays_silent() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        db.record_heartbeat(&probe.probe_id, None, None, 0).unwrap();
        let seen = db.get_probe(&probe.probe_id).unwrap().last_seen.unwrap();

        evaluate(&db, seen, DELAY).unwrap();
        assert!(evaluate(&db, seen + DELAY + 1, DELAY).unwrap().is_empty());
    }

    #[test]
    fn a_probe_exception_silences_it_inside_a_subscribed_site() {
        // Un poste de test n'alerte pas, la baie du client si.
        let (db, probe_id, seen) = parc();
        db.set_notify_subscription("probe", &probe_id, Some(false))
            .unwrap();

        evaluate(&db, seen, DELAY).unwrap();
        assert!(evaluate(&db, seen + DELAY + 1, DELAY).unwrap().is_empty());
    }

    #[test]
    fn a_probe_never_seen_raises_nothing() {
        // On ne revient pas d'un état où l'on n'a jamais été, et une machine
        // enrôlée qu'on n'a pas encore branchée n'est pas une panne.
        let db = Arc::new(Db::open_in_memory().unwrap());
        let site = db.create_site("Durand").unwrap();
        let (probe, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        db.set_notify_subscription("site", &site.site_id, Some(true))
            .unwrap();

        let now = crate::db::now();
        assert!(evaluate(&db, now, DELAY).unwrap().is_empty());
        assert!(evaluate(&db, now + 86_400, DELAY).unwrap().is_empty());
        assert!(db.notify_state(&probe.probe_id).unwrap().is_none());
    }

    #[test]
    fn a_revoked_probe_raises_nothing() {
        // Elle a cessé d'émettre parce qu'on le lui a demandé.
        let (db, probe_id, seen) = parc();
        evaluate(&db, seen, DELAY).unwrap();
        db.revoke_probe(&probe_id).unwrap();

        assert!(evaluate(&db, seen + DELAY + 1, DELAY).unwrap().is_empty());
    }

    #[test]
    fn each_probe_keeps_its_own_state() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let site = db.create_site("Durand").unwrap();
        let (paris, _) = db.enroll_probe(&site.site_id, "Paris").unwrap();
        let (lyon, _) = db.enroll_probe(&site.site_id, "Lyon").unwrap();
        db.set_notify_subscription("site", &site.site_id, Some(true))
            .unwrap();
        db.record_heartbeat(&paris.probe_id, None, None, 0).unwrap();
        db.record_heartbeat(&lyon.probe_id, None, None, 0).unwrap();
        let seen = db.get_probe(&paris.probe_id).unwrap().last_seen.unwrap();
        evaluate(&db, seen, DELAY).unwrap();

        // Les deux battent au même instant, mais Lyon était mémorisée hors
        // ligne : seule elle a une transition à annoncer.
        db.set_notify_state(&lyon.probe_id, AlertState::Down, seen).unwrap();

        let alerts = evaluate(&db, seen, DELAY).unwrap();
        assert_eq!(alerts.len(), 1, "{alerts:?}");
        assert_eq!(alerts[0].probe, "Lyon");
        assert_eq!(alerts[0].kind, AlertKind::Up);
        assert_eq!(
            db.notify_state(&paris.probe_id).unwrap().map(|(s, _)| s),
            Some(AlertState::Up),
            "l'état de Paris ne doit pas avoir bougé"
        );
    }

    #[test]
    fn the_message_says_what_changed_and_how_long_ago() {
        let alert = Alert {
            kind: AlertKind::Down,
            probe_id: "p-1".into(),
            probe: "Paris".into(),
            site_id: "s-1".into(),
            site: "Durand".into(),
            last_seen: 1_000,
            silence_secs: 7 * 60,
        };
        assert_eq!(
            alert.message(),
            "Paris (Durand) est passée hors ligne — dernier contact il y a 7 min."
        );
        assert_eq!(alert.subject(), "LanProbe — Paris (Durand) hors ligne");

        let back = Alert {
            kind: AlertKind::Up,
            silence_secs: 0,
            ..alert
        };
        assert_eq!(back.message(), "Paris (Durand) est revenue en ligne.");
        assert_eq!(back.subject(), "LanProbe — Paris (Durand) de retour");
    }

    #[test]
    fn durations_are_written_for_someone_being_woken_up() {
        assert_eq!(humanise(45), "45 s");
        assert_eq!(humanise(7 * 60), "7 min");
        assert_eq!(humanise(3 * 3600), "3 h");
        assert_eq!(humanise(5 * 86_400), "5 j");
        assert_eq!(humanise(-1), "0 s", "une horloge en avance ne doit rien casser");
    }

    fn sample_alert() -> Alert {
        Alert {
            kind: AlertKind::Down,
            probe_id: "p-1".into(),
            probe: "Paris".into(),
            site_id: "s-1".into(),
            site: "Durand".into(),
            last_seen: 1_000,
            silence_secs: 420,
        }
    }

    #[test]
    fn the_slack_template_is_a_text_field() {
        let payload = webhook_payload(WebhookTemplate::Slack, &sample_alert());
        assert_eq!(payload["text"], sample_alert().message());
        assert_eq!(payload.as_object().unwrap().len(), 1, "Slack n'attend que ça");
    }

    #[test]
    fn the_discord_template_is_a_content_field() {
        let payload = webhook_payload(WebhookTemplate::Discord, &sample_alert());
        assert_eq!(payload["content"], sample_alert().message());
        assert_eq!(payload.as_object().unwrap().len(), 1);
    }

    #[test]
    fn the_generic_template_carries_the_facts_not_just_a_sentence() {
        // C'est ce qui permet à un destinataire maison de router sur le site
        // ou de filtrer sur la sonde sans analyser du français.
        let payload = webhook_payload(WebhookTemplate::Generic, &sample_alert());
        assert_eq!(payload["event"], "probe.down");
        assert_eq!(payload["probe_id"], "p-1");
        assert_eq!(payload["site"], "Durand");
        assert_eq!(payload["silence_secs"], 420);
        assert!(payload["message"].is_string());
    }
}
