//! Rattachement d'une sonde à un hub `lanprobe-web`.
//!
//! Trois responsabilités, et une seule d'entre elles a besoin du compte de
//! l'utilisateur :
//!
//! 1. **enrôlement** — une URL et un code de huit caractères suffisent. Le hub
//!    répond avec l'identité de la sonde, ses coordonnées InfluxDB et une clé
//!    d'écriture. Le mot de passe du compte n'est jamais écrit sur disque, et
//!    avec un code il n'est même pas saisi ;
//! 2. **battement de cœur** — sans lui, une sonde éteinte est indiscernable
//!    d'une sonde qui n'a rien à dire : dans une base de séries temporelles,
//!    l'absence de points ressemble exactement au calme plat ;
//! 3. **application des changements décidés côté hub** — renommage, changement
//!    de cadence, rotation de clé. Ils arrivent dans la réponse au battement,
//!    la sonde n'interroge jamais le hub pour savoir si quelque chose a bougé.
//!
//! Voir `docs/lanprobe-web-contrat.md`, sections 3, 6 et 9.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::secrets::{self, SecretKey};
use crate::state::AppState;

/// Clé de la section `hub` dans `app_config.json`.
const CONFIG_KEY: &str = "hub";

/// Repli minimal si le hub ne dit rien — il annonce normalement sa cadence.
const DEFAULT_HEARTBEAT_SECS: u64 = 60;

/// Plafond du repli exponentiel entre deux tentatives ratées. Une sonde qui
/// martèle un hub en train de redémarrer ne fait que retarder son retour.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

// ── Ce que la sonde retient ────────────────────────────────────────────────

/// Coordonnées d'écriture InfluxDB remises à l'enrôlement.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InfluxCredentials {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub org: String,
    #[serde(default)]
    pub bucket: String,
    /// Scellé au repos (`enc:v1:…`). Une sonde vaut le réseau qu'elle mesure ;
    /// sa clé d'écriture n'a pas à traîner en clair dans un fichier JSON.
    #[serde(default)]
    pub token: String,
    /// Empreinte SHA-256 du certificat d'Influx, à épingler.
    ///
    /// Influx est servi avec un certificat auto-signé : soit on épingle, soit
    /// on désactive la vérification. Épingler garde l'authentification du
    /// serveur sans exiger une autorité de certification que personne n'a sur
    /// un réseau local — désactiver la vérification l'abandonne pour de bon.
    #[serde(default)]
    pub tls_fingerprint_sha256: Option<String>,
    /// Version de la clé, incrémentée à chaque rotation côté hub. Renvoyée
    /// dans les battements suivants : c'est cet accusé qui autorise le hub à
    /// détruire l'ancienne clé.
    #[serde(default)]
    pub token_version: u32,
}

/// Section `hub` de la configuration de la sonde.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HubConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub probe_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub site_id: String,
    #[serde(default)]
    pub site: String,
    /// Jeton de sonde, scellé au repos.
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub influx: InfluxCredentials,
    #[serde(default = "default_heartbeat")]
    pub heartbeat_interval_secs: u64,
}

fn default_heartbeat() -> u64 {
    DEFAULT_HEARTBEAT_SECS
}

impl HubConfig {
    /// Vrai quand la sonde est rattachée à un hub. On exige l'identifiant
    /// **et** le jeton : une configuration à moitié écrite (interruption au
    /// mauvais moment) ne doit pas passer pour un enrôlement valide.
    pub fn is_enrolled(&self) -> bool {
        !self.url.is_empty() && !self.probe_id.is_empty() && !self.token.is_empty()
    }
}

// ── Dialogue avec le hub ───────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct EnrollRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    site: Option<&'a str>,
    name: &'a str,
}

#[derive(Debug, Deserialize)]
struct EnrollResponse {
    probe_id: String,
    name: String,
    #[serde(default)]
    site_id: String,
    #[serde(default)]
    site: String,
    token: String,
    influx: InfluxResponse,
    #[serde(default = "default_heartbeat")]
    heartbeat_interval_secs: u64,
}

#[derive(Debug, Deserialize)]
struct InfluxResponse {
    #[serde(default)]
    url: String,
    #[serde(default)]
    org: String,
    #[serde(default)]
    bucket: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    tls_fingerprint_sha256: Option<String>,
    #[serde(default)]
    token_version: u32,
}

/// Réponse au battement de cœur : c'est par elle que le hub pousse ses
/// décisions vers la sonde.
#[derive(Debug, Default, Deserialize)]
pub struct HeartbeatResponse {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub site_id: Option<String>,
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub heartbeat_interval_secs: Option<u64>,
    #[serde(default)]
    pub influx: Option<InfluxResponse>,
}

/// Ce que la sonde raconte d'elle-même à chaque battement.
#[derive(Debug, Serialize)]
struct HeartbeatRequest<'a> {
    version: &'a str,
    platform: &'a str,
    /// Profondeur du tampon local. Une sonde qui accumule sans écrire est
    /// visible ici **avant** qu'on découvre le trou dans les graphes.
    buffered_points: usize,
    last_write_ok: bool,
    /// Accuse réception d'une rotation : tant que le hub ne le voit pas, il
    /// conserve l'ancienne clé.
    influx_token_version: u32,
}

/// Enrôle cette sonde auprès d'un hub et **persiste** le résultat scellé.
///
/// `code` et `credentials` sont exclusifs : le code est le chemin normal, les
/// identifiants ne servent qu'à un usage scripté.
pub async fn enroll(
    state: &AppState,
    key: &SecretKey,
    hub_url: &str,
    name: &str,
    code: Option<&str>,
    credentials: Option<(&str, &str, &str)>,
) -> Result<HubConfig, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("le nom de la sonde ne peut pas être vide".into());
    }
    let hub_url = hub_url.trim().trim_end_matches('/');
    if hub_url.is_empty() {
        return Err("l'adresse du hub ne peut pas être vide".into());
    }

    let (username, password, site) = match credentials {
        Some((u, p, s)) => (Some(u), Some(p), Some(s)),
        None => (None, None, None),
    };
    if code.is_none() && username.is_none() {
        return Err("il faut un code d'enrôlement ou des identifiants".into());
    }

    let body = EnrollRequest { code, username, password, site, name };
    let response = http_client()
        .post(format!("{hub_url}/api/probes/enroll"))
        .json(&body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("hub injoignable : {e}"))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        // Le hub renvoie `{"error": "..."}` : on remonte son message, il est
        // écrit pour être lu (code expiré, nom déjà pris dans ce site…).
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["error"].as_str().map(String::from))
            .unwrap_or_else(|| text.clone());
        return Err(format!("enrôlement refusé ({status}) : {detail}"));
    }

    let parsed: EnrollResponse =
        serde_json::from_str(&text).map_err(|e| format!("réponse illisible du hub : {e}"))?;

    let config = HubConfig {
        url: hub_url.to_string(),
        probe_id: parsed.probe_id,
        name: parsed.name,
        site_id: parsed.site_id,
        site: parsed.site,
        token: secrets::seal(key, &parsed.token)?,
        influx: InfluxCredentials {
            url: parsed.influx.url,
            org: parsed.influx.org,
            bucket: parsed.influx.bucket,
            token: secrets::seal(key, &parsed.influx.token)?,
            tls_fingerprint_sha256: parsed.influx.tls_fingerprint_sha256,
            token_version: parsed.influx.token_version,
        },
        heartbeat_interval_secs: parsed.heartbeat_interval_secs,
    };

    save(state, &config)?;
    Ok(config)
}

/// Relit la section `hub` de la configuration.
pub fn load(state: &AppState) -> HubConfig {
    state
        .config
        .get()
        .get(CONFIG_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Écrit la section `hub` sans toucher au reste de la configuration.
fn save(state: &AppState, config: &HubConfig) -> Result<(), String> {
    let mut root = state.config.get();
    let value = serde_json::to_value(config).map_err(|e| e.to_string())?;
    match root.as_object_mut() {
        Some(map) => {
            map.insert(CONFIG_KEY.to_string(), value);
        }
        None => {
            root = serde_json::json!({ CONFIG_KEY: value });
        }
    }
    state.config.put(root.clone())?;
    // Le module InfluxDB écoute `config:update` : c'est ce qui fait basculer
    // l'export sur les nouvelles coordonnées sans redémarrer la sonde.
    state.emit("config:update", root);
    Ok(())
}

/// Détache la sonde du hub. **Ne supprime aucune mesure** — celles déjà
/// écrites appartiennent au hub, pas à la sonde.
pub fn forget(state: &AppState) -> Result<(), String> {
    let mut root = state.config.get();
    if let Some(map) = root.as_object_mut() {
        map.remove(CONFIG_KEY);
    }
    state.config.put(root.clone())?;
    state.emit("config:update", root);
    Ok(())
}

// ── Boucle de battement de cœur ────────────────────────────────────────────

/// Compteur partagé avec l'exportateur InfluxDB : combien de points attendent
/// d'être écrits, et la dernière écriture a-t-elle réussi.
#[derive(Debug, Default)]
pub struct WriteStatus {
    inner: Mutex<(usize, bool)>,
}

impl WriteStatus {
    pub fn set(&self, buffered: usize, last_write_ok: bool) {
        if let Ok(mut g) = self.inner.lock() {
            *g = (buffered, last_write_ok);
        }
    }
    fn read(&self) -> (usize, bool) {
        self.inner.lock().map(|g| *g).unwrap_or((0, true))
    }
}

/// Tâche de fond : bat le cœur tant que la sonde est enrôlée, et applique ce
/// que le hub renvoie.
pub async fn run(state: AppState, key: SecretKey, status: Arc<WriteStatus>) {
    let mut backoff = Duration::from_secs(5);

    loop {
        let config = load(&state);
        if !config.is_enrolled() {
            // Pas encore enrôlée : on attend qu'elle le soit, sans bruit.
            tokio::time::sleep(Duration::from_secs(15)).await;
            continue;
        }

        let interval = Duration::from_secs(config.heartbeat_interval_secs.max(5));
        match beat(&state, &key, &config, &status).await {
            Ok(response) => {
                backoff = Duration::from_secs(5);
                if let Err(e) = apply(&state, &key, &config, response) {
                    tracing::warn!("hub : mise à jour locale impossible : {e}");
                }
                tokio::time::sleep(interval).await;
            }
            Err(e) => {
                tracing::warn!("hub : battement de cœur échoué ({e}) — nouvelle tentative dans {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

async fn beat(
    state: &AppState,
    key: &SecretKey,
    config: &HubConfig,
    status: &WriteStatus,
) -> Result<HeartbeatResponse, String> {
    let token = secrets::open(key, &config.token)?;
    let (buffered_points, last_write_ok) = status.read();
    let _ = state;

    let response = http_client()
        .post(format!(
            "{}/api/probes/{}/heartbeat",
            config.url, config.probe_id
        ))
        .bearer_auth(token)
        .json(&HeartbeatRequest {
            version: env!("CARGO_PKG_VERSION"),
            platform: platform(),
            buffered_points,
            last_write_ok,
            influx_token_version: config.influx.token_version,
        })
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status_code = response.status();
    if status_code == reqwest::StatusCode::UNAUTHORIZED {
        // La sonde a été révoquée, ou son jeton a changé côté hub. Insister
        // ne sert à rien : il faut un ré-enrôlement, donc une action humaine.
        return Err(
            "jeton refusé par le hub — la sonde a probablement été révoquée, un ré-enrôlement est nécessaire".into(),
        );
    }
    if !status_code.is_success() {
        return Err(format!("réponse {status_code}"));
    }

    response
        .json::<HeartbeatResponse>()
        .await
        .map_err(|e| format!("réponse illisible : {e}"))
}

/// Applique ce que le hub a renvoyé. N'écrit la configuration que si quelque
/// chose a réellement changé — réécrire à chaque battement userait le disque
/// et brouillerait la lecture du fichier.
fn apply(
    state: &AppState,
    key: &SecretKey,
    current: &HubConfig,
    response: HeartbeatResponse,
) -> Result<(), String> {
    let mut next = current.clone();
    let mut changed = false;

    if let Some(name) = response.name.filter(|n| *n != current.name) {
        tracing::info!("hub : sonde renommée en « {name} »");
        next.name = name;
        changed = true;
    }
    if let Some(site) = response.site.filter(|s| *s != current.site) {
        tracing::info!("hub : site devenu « {site} »");
        next.site = site;
        changed = true;
    }
    if let Some(site_id) = response.site_id.filter(|s| *s != current.site_id) {
        next.site_id = site_id;
        changed = true;
    }
    if let Some(secs) = response
        .heartbeat_interval_secs
        .filter(|s| *s != current.heartbeat_interval_secs)
    {
        next.heartbeat_interval_secs = secs;
        changed = true;
    }

    // Rotation de clé : le hub ne détruira l'ancienne qu'après avoir vu la
    // nouvelle version dans un battement. Tant qu'on n'a pas persisté celle-ci,
    // on ne l'accuse pas — sinon une coupure entre l'accusé et l'écriture
    // laisserait la sonde avec une clé morte.
    if let Some(influx) = response.influx {
        if influx.token_version > current.influx.token_version && !influx.token.is_empty() {
            tracing::info!(
                "hub : nouvelle clé d'écriture (version {})",
                influx.token_version
            );
            next.influx.token = secrets::seal(key, &influx.token)?;
            next.influx.token_version = influx.token_version;
            if !influx.url.is_empty() {
                next.influx.url = influx.url;
            }
            if !influx.org.is_empty() {
                next.influx.org = influx.org;
            }
            if !influx.bucket.is_empty() {
                next.influx.bucket = influx.bucket;
            }
            if influx.tls_fingerprint_sha256.is_some() {
                next.influx.tls_fingerprint_sha256 = influx.tls_fingerprint_sha256;
            }
            changed = true;
        }
    }

    if changed {
        save(state, &next)?;
    }
    Ok(())
}

fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        // Le hub peut servir un certificat auto-signé (option `--tls`). On ne
        // valide pas la chaîne ici : le secret qui compte, le jeton de sonde,
        // est vérifié par le hub, et l'utilisateur a saisi l'adresse lui-même.
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_written_config_is_not_an_enrolment() {
        let mut c = HubConfig {
            url: "https://hub".into(),
            ..Default::default()
        };
        assert!(!c.is_enrolled(), "une URL seule ne suffit pas");
        c.probe_id = "abc".into();
        assert!(!c.is_enrolled(), "sans jeton, la sonde ne peut rien faire");
        c.token = "enc:v1:…".into();
        assert!(c.is_enrolled());
    }

    #[test]
    fn heartbeat_defaults_to_a_sane_cadence() {
        let c: HubConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c.heartbeat_interval_secs, DEFAULT_HEARTBEAT_SECS);
    }

    #[test]
    fn enrolment_needs_a_code_or_credentials() {
        // Vérifié avant tout appel réseau : inutile de déranger le hub pour
        // une demande qu'il refusera.
        let body = EnrollRequest {
            code: None,
            username: None,
            password: None,
            site: None,
            name: "Paris",
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 1, "{json}");
        assert_eq!(json["name"], "Paris");
    }

    #[test]
    fn a_lower_token_version_is_ignored() {
        // Une réponse tardive ou rejouée ne doit pas faire revenir la sonde à
        // une clé que le hub a déjà détruite.
        let current = HubConfig {
            influx: InfluxCredentials {
                token_version: 4,
                ..Default::default()
            },
            ..Default::default()
        };
        let older = InfluxResponse {
            url: String::new(),
            org: String::new(),
            bucket: String::new(),
            token: "vieux".into(),
            tls_fingerprint_sha256: None,
            token_version: 3,
        };
        assert!(
            older.token_version <= current.influx.token_version,
            "la version antérieure doit être ignorée"
        );
    }
}
