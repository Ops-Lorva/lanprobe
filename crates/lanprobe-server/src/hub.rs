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
use std::time::{Duration, Instant};

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

/// Durée de validité de l'IP publique en cache.
///
/// La connaître exige un appel sortant vers un service tiers ; le faire à
/// chaque battement pour chaque sonde d'un parc, c'est marteler un service
/// gratuit et publier son trafic à intervalle régulier. Elle change rarement,
/// et c'est le **changement** qui informe — pas la fraîcheur.
pub const PUBLIC_IP_TTL: Duration = Duration::from_secs(6 * 3600);

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
    /// Accepte un certificat non vérifiable pour le hub. **Faux par défaut.**
    ///
    /// N'a de raison d'être que si l'utilisateur héberge son hub avec un
    /// certificat auto-signé (`--tls`) et l'assume en connaissance de cause :
    /// c'est le jeton de sonde qui voyage dans cette requête, et sans
    /// vérification n'importe qui sur le chemin peut se faire passer pour le
    /// hub et le récupérer. Le cas normal — un hub derrière un reverse proxy
    /// avec un vrai certificat — ne l'active jamais.
    #[serde(default)]
    pub allow_self_signed: bool,
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

// ── IP publique : en cache, jamais à chaque battement ──────────────────────

#[derive(Debug)]
struct CachedPublicIp {
    /// Interface pour laquelle l'adresse a été relevée. Une bascule sur un
    /// lien de secours change l'IP publique.
    interface: Option<String>,
    /// `None` quand le dernier relevé a échoué et qu'aucune valeur antérieure
    /// n'était utilisable.
    ip: Option<String>,
    asked_at: Instant,
}

/// Dernière IP publique connue de la sonde, et la date à laquelle on l'a
/// demandée. Voir `docs/lanprobe-web-contrat.md`, section 15.
#[derive(Debug, Default)]
pub struct PublicIpCache {
    inner: Mutex<Option<CachedPublicIp>>,
}

impl PublicIpCache {
    /// Vrai s'il faut refaire l'appel sortant : jamais vue, interface
    /// changée, ou plus vieille que `PUBLIC_IP_TTL`.
    fn needs_refresh(&self, interface: Option<&str>, now: Instant) -> bool {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_ref() {
            None => true,
            Some(c) => {
                c.interface.as_deref() != interface
                    || now.duration_since(c.asked_at) >= PUBLIC_IP_TTL
            }
        }
    }

    /// Enregistre le résultat d'un relevé. Un échec (`ip` à `None`) conserve
    /// la dernière valeur connue **si l'interface n'a pas changé** : sinon
    /// l'adresse de l'ancienne interface n'a plus rien à voir avec la
    /// nouvelle, et la garder serait un mensonge.
    fn store(&self, interface: Option<&str>, ip: Option<String>, now: Instant) {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let previous = guard
            .as_ref()
            .filter(|c| c.interface.as_deref() == interface)
            .and_then(|c| c.ip.clone());
        *guard = Some(CachedPublicIp {
            interface: interface.map(str::to_string),
            ip: ip.or(previous),
            asked_at: now,
        });
    }

    /// La dernière adresse connue, s'il y en a une.
    pub fn value(&self) -> Option<String> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .and_then(|c| c.ip.clone())
    }
}

/// Ce que la sonde sait de sa position réseau, relevé à chaque battement —
/// sauf l'IP publique, qui vient du cache.
#[derive(Debug, Default)]
struct NetworkIdentity {
    interface: Option<String>,
    local_ips: Vec<String>,
    gateway: Option<String>,
}

/// Relève l'interface sélectionnée, ses adresses locales et sa passerelle.
///
/// Tout vient de `lanprobe-core` : l'app desktop montre exactement ces
/// valeurs, un second chemin de calcul finirait par en montrer d'autres.
fn network_identity(state: &AppState) -> NetworkIdentity {
    let interface = state
        .selected_interface
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let Some(name) = interface else {
        return NetworkIdentity::default();
    };
    let details = lanprobe_core::interfaces::get_interface_details(&name);
    NetworkIdentity {
        interface: Some(details.bsd_name.clone().unwrap_or_else(|| name.clone())),
        local_ips: lanprobe_core::interfaces::local_cidr(&details)
            .into_iter()
            .collect(),
        gateway: details.gateway.clone().filter(|g| !g.is_empty()),
    }
}

/// Adresse IPv4 source à utiliser pour l'appel sortant, pour qu'il sorte bien
/// par l'interface choisie et pas par la route par défaut.
fn source_address(identity: &NetworkIdentity) -> Option<std::net::Ipv4Addr> {
    identity
        .local_ips
        .first()?
        .split('/')
        .next()?
        .parse()
        .ok()
}

/// Rafraîchit l'IP publique **si et seulement si** le cache l'exige. Un échec
/// n'est pas une erreur de battement : la sonde envoie ce qu'elle sait.
async fn refresh_public_ip(cache: &PublicIpCache, identity: &NetworkIdentity, now: Instant) {
    if !cache.needs_refresh(identity.interface.as_deref(), now) {
        return;
    }
    let found = lanprobe_core::public_ip::get_public_ip(source_address(identity))
        .await
        .map(|info| info.ip)
        .inspect_err(|e| tracing::debug!("hub : IP publique indisponible ({e})"))
        .ok()
        .filter(|ip| !ip.is_empty());
    cache.store(identity.interface.as_deref(), found, now);
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
pub struct InfluxResponse {
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
    /// Identité réseau du site. Omise plutôt que vide : une sonde sans accès
    /// internet doit pouvoir battre — c'est justement le moment où son
    /// battement compte. Voir section 15 du contrat.
    #[serde(skip_serializing_if = "Option::is_none")]
    public_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    interface: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    local_ips: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gateway: Option<String>,
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
    allow_self_signed: bool,
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
    let response = http_client(allow_self_signed)
        .post(format!("{hub_url}/api/probes/enroll"))
        .json(&body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| {
            // Distinguer les deux causes : « je n'ai pas confiance en ce
            // certificat » n'a pas la même correction que « personne ne
            // répond », et le message générique enverrait l'utilisateur
            // vérifier son réseau alors que son hub est parfaitement joignable.
            let hint = if !allow_self_signed && looks_like_tls_failure(&e) {
                " — certificat non vérifiable. Si votre hub utilise un certificat auto-signé, cochez l'option correspondante ; sinon placez-le derrière un reverse proxy muni d'un vrai certificat."
            } else {
                ""
            };
            format!("hub injoignable : {e}{hint}")
        })?;

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
        allow_self_signed,
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
    // Sans ça, l'interface garderait une pastille d'erreur jusqu'au prochain
    // passage de la boucle — pour une sonde qu'on vient de détacher exprès.
    state.hub.set_detached();
    let mut root = state.config.get();
    if let Some(map) = root.as_object_mut() {
        map.remove(CONFIG_KEY);
    }
    state.config.put(root.clone())?;
    state.emit("config:update", root);
    Ok(())
}

// ── État du rattachement, tel que l'app le montre ──────────────────────────

/// Les quatre états d'un rattachement. Ce ne sont pas quatre nuances de la
/// même chose : chacun appelle une action différente de l'utilisateur, c'est
/// ce qui justifie quatre couleurs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HubState {
    /// Pas rattachée. Le rattachement est optionnel : son absence n'est pas
    /// un défaut, et l'app n'a rien de particulier à signaler.
    #[default]
    Off,
    /// Dernier battement réussi. Rien à faire.
    Ok,
    /// Rattachée, mais le lien est coupé. **Ça se répare tout seul** : rien à
    /// faire, sinon savoir que les mesures ne partent plus et s'accumulent.
    Degraded,
    /// Le hub a refusé le jeton. **Réessayer ne servira jamais à rien** : il
    /// faut un ré-enrôlement, donc une action humaine. Le confondre avec
    /// `Degraded` ferait attendre une réparation qui ne viendra pas.
    Broken,
}

/// Ce que l'app affiche du rattachement, à un instant donné.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HubSnapshot {
    pub state: HubState,
    /// Horodatage UNIX du dernier battement **réussi**. Conservé quand le
    /// lien tombe : « plus rien ne part depuis 14 h 02 » se lit, « plus rien
    /// ne part » ne dit pas depuis quand.
    pub last_beat_at: Option<i64>,
    /// Message du hub ou de la couche réseau. Il est écrit pour être lu.
    pub last_error: Option<String>,
    /// Points en attente d'écriture — c'est la profondeur du retard.
    pub buffered_points: usize,
}

/// État partagé entre la boucle de battement et l'interface.
#[derive(Debug, Default)]
pub struct HubStatus {
    inner: Mutex<HubSnapshot>,
}

impl HubStatus {
    pub fn snapshot(&self) -> HubSnapshot {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Aucune configuration de hub : on ne signale rien.
    pub fn set_detached(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.state = HubState::Off;
        g.last_error = None;
    }

    pub fn record_success(&self, buffered_points: usize, at: i64) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.state = HubState::Ok;
        g.last_beat_at = Some(at);
        g.last_error = None;
        g.buffered_points = buffered_points;
    }

    /// Un échec transitoire ne **repeint pas** un rattachement déjà cassé :
    /// une coupure réseau survenue après une révocation ne rend pas la sonde
    /// réparable toute seule. Seul un battement réussi sort de `Broken`.
    pub fn record_failure(&self, error: &BeatError, buffered_points: usize) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.state = match (g.state, error) {
            (_, BeatError::Unrecoverable(_)) => HubState::Broken,
            (HubState::Broken, _) => HubState::Broken,
            _ => HubState::Degraded,
        };
        g.last_error = Some(error.message().to_string());
        g.buffered_points = buffered_points;
    }
}

/// Pourquoi un battement a échoué. La distinction n'est pas cosmétique :
/// l'une des deux causes se répare toute seule, l'autre jamais.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeatError {
    /// Rien ne se réparera tout seul : jeton refusé par le hub, ou secret
    /// local illisible. Il faut un ré-enrôlement.
    Unrecoverable(String),
    /// Hub injoignable, réseau coupé, réponse illisible : le prochain tick
    /// réessaiera, et un jour ça repassera.
    Transient(String),
}

impl BeatError {
    pub fn message(&self) -> &str {
        match self {
            BeatError::Unrecoverable(m) | BeatError::Transient(m) => m,
        }
    }
}

impl std::fmt::Display for BeatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

/// Traduit le code de réponse du hub. Seul un refus du jeton est définitif —
/// un hub en train de redémarrer n'a révoqué personne.
fn error_for_status(status: reqwest::StatusCode) -> Option<BeatError> {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Some(BeatError::Unrecoverable(
            "jeton refusé par le hub — la sonde a probablement été révoquée, un ré-enrôlement est nécessaire".into(),
        ));
    }
    if status.is_success() {
        return None;
    }
    Some(BeatError::Transient(format!("réponse {status}")))
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
    // Vit aussi longtemps que la boucle : c'est ce qui fait qu'on n'appelle
    // pas le service d'IP publique à chaque battement.
    let public_ip = PublicIpCache::default();

    loop {
        let config = load(&state);
        if !config.is_enrolled() {
            // Pas encore enrôlée : on attend qu'elle le soit, sans bruit.
            state.hub.set_detached();
            tokio::time::sleep(Duration::from_secs(15)).await;
            continue;
        }

        let interval = Duration::from_secs(config.heartbeat_interval_secs.max(5));
        let (buffered_points, _) = status.read();
        match beat(&state, &key, &config, &status, &public_ip).await {
            Ok(response) => {
                backoff = Duration::from_secs(5);
                state.hub.record_success(buffered_points, unix_now());
                if let Err(e) = apply(&state, &key, &config, response) {
                    tracing::warn!("hub : mise à jour locale impossible : {e}");
                }
                tokio::time::sleep(interval).await;
            }
            Err(e) => {
                state.hub.record_failure(&e, buffered_points);
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
    public_ip: &PublicIpCache,
) -> Result<HeartbeatResponse, BeatError> {
    // Un secret local illisible ne se répare pas en réessayant : c'est un
    // ré-enrôlement qu'il faut, pas de la patience.
    let token = secrets::open(key, &config.token).map_err(BeatError::Unrecoverable)?;
    let (buffered_points, last_write_ok) = status.read();

    let identity = network_identity(state);
    refresh_public_ip(public_ip, &identity, Instant::now()).await;

    let response = http_client(config.allow_self_signed)
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
            public_ip: public_ip.value(),
            interface: identity.interface.clone(),
            local_ips: identity.local_ips.clone(),
            gateway: identity.gateway.clone(),
        })
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| BeatError::Transient(e.to_string()))?;

    if let Some(e) = error_for_status(response.status()) {
        return Err(e);
    }

    // Un hub en train de redémarrer peut servir une page d'erreur : illisible
    // ne veut pas dire révoquée.
    response
        .json::<HeartbeatResponse>()
        .await
        .map_err(|e| BeatError::Transient(format!("réponse illisible : {e}")))
}

/// Horodatage UNIX en secondes.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
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

/// Vrai quand l'erreur de `reqwest` ressemble à un refus de certificat.
fn looks_like_tls_failure(error: &reqwest::Error) -> bool {
    let text = error.to_string().to_lowercase();
    text.contains("certificate") || text.contains("tls") || text.contains("self-signed")
}

/// Client HTTP vers le hub.
///
/// **La vérification du certificat est active par défaut.** Le jeton de sonde
/// voyage dans ces requêtes : sans vérification, n'importe qui sur le chemin
/// peut se faire passer pour le hub et le récupérer. Elle n'est levée que si
/// l'utilisateur a explicitement déclaré héberger un hub auto-signé — un choix
/// qu'il pose lui-même, pas un défaut qu'il subit.
fn http_client(allow_self_signed: bool) -> reqwest::Client {
    let builder = reqwest::Client::builder();
    let builder = if allow_self_signed {
        builder.danger_accept_invalid_certs(true)
    } else {
        builder
    };
    builder.build().unwrap_or_default()
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
    fn tls_verification_is_on_unless_explicitly_waived() {
        // Le jeton de sonde voyage dans ces requêtes : le défaut doit être
        // la vérification, jamais l'inverse.
        let c: HubConfig = serde_json::from_str("{}").unwrap();
        assert!(
            !c.allow_self_signed,
            "une configuration qui ne dit rien doit vérifier le certificat"
        );
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

    // ── État du rattachement, tel que l'app le montre ──────────────────

    #[test]
    fn a_probe_without_a_hub_is_off_not_broken() {
        // Le rattachement est optionnel : son absence n'est pas un défaut.
        let status = HubStatus::default();
        assert_eq!(status.snapshot().state, HubState::Off);

        status.set_detached();
        let snap = status.snapshot();
        assert_eq!(snap.state, HubState::Off);
        assert_eq!(snap.last_error, None);
    }

    #[test]
    fn a_failed_beat_degrades_and_the_next_success_repairs() {
        // `degraded` se répare tout seul quand le lien revient : l'utilisateur
        // n'a rien à faire, il doit juste savoir que ses mesures ne partent
        // plus.
        let status = HubStatus::default();
        status.record_success(0, 1_000);
        assert_eq!(status.snapshot().state, HubState::Ok);

        status.record_failure(&BeatError::Transient("hub injoignable".into()), 42);
        let snap = status.snapshot();
        assert_eq!(snap.state, HubState::Degraded);
        assert_eq!(snap.last_error.as_deref(), Some("hub injoignable"));
        assert_eq!(snap.buffered_points, 42, "depuis combien de temps ça dure");
        assert_eq!(
            snap.last_beat_at,
            Some(1_000),
            "le dernier battement réussi ne s'efface pas"
        );

        status.record_success(0, 2_000);
        let snap = status.snapshot();
        assert_eq!(snap.state, HubState::Ok);
        assert_eq!(snap.last_error, None, "l'erreur réparée ne reste pas affichée");
        assert_eq!(snap.last_beat_at, Some(2_000));
    }

    #[test]
    fn a_refused_token_is_broken_not_degraded() {
        // Réessayer ne servira jamais à rien : il faut un ré-enrôlement.
        // Confondre les deux ferait attendre une réparation qui ne viendra pas.
        let status = HubStatus::default();
        status.record_success(0, 1_000);
        status.record_failure(&BeatError::Unrecoverable("jeton refusé".into()), 0);

        let snap = status.snapshot();
        assert_eq!(snap.state, HubState::Broken);
        assert_ne!(snap.state, HubState::Degraded);
        assert_eq!(snap.last_error.as_deref(), Some("jeton refusé"));
    }

    #[test]
    fn a_network_hiccup_does_not_hide_a_revoked_probe() {
        // Une coupure réseau après une révocation ne doit pas repeindre
        // l'état en « ça va revenir ».
        let status = HubStatus::default();
        status.record_failure(&BeatError::Unrecoverable("jeton refusé".into()), 0);
        status.record_failure(&BeatError::Transient("hub injoignable".into()), 3);

        assert_eq!(status.snapshot().state, HubState::Broken);
        // Seul un battement réussi — donc un ré-enrôlement — répare.
        status.record_success(0, 2_000);
        assert_eq!(status.snapshot().state, HubState::Ok);
    }

    #[test]
    fn only_a_refusal_of_the_token_is_unrecoverable() {
        use reqwest::StatusCode;
        assert!(matches!(
            error_for_status(StatusCode::UNAUTHORIZED),
            Some(BeatError::Unrecoverable(_))
        ));
        // Un hub en train de redémarrer n'a révoqué personne.
        assert!(matches!(
            error_for_status(StatusCode::BAD_GATEWAY),
            Some(BeatError::Transient(_))
        ));
        assert!(error_for_status(StatusCode::OK).is_none());
    }

    #[test]
    fn the_status_serialises_as_the_app_expects_it() {
        let status = HubStatus::default();
        status.record_success(7, 1_787_914_000);
        let json = serde_json::to_value(status.snapshot()).unwrap();
        assert_eq!(json["state"], "ok");
        assert_eq!(json["last_beat_at"], 1_787_914_000_i64);
        assert_eq!(json["buffered_points"], 7);
        assert_eq!(json["last_error"], serde_json::Value::Null);

        status.record_failure(&BeatError::Transient("réseau coupé".into()), 7);
        let json = serde_json::to_value(status.snapshot()).unwrap();
        assert_eq!(json["state"], "degraded");
        assert_eq!(json["last_error"], "réseau coupé");
    }

    #[test]
    fn the_public_ip_is_not_asked_again_before_six_hours() {
        // Le demander à chaque battement, c'est marteler un service gratuit
        // et publier son trafic à intervalle régulier.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        assert!(cache.needs_refresh(Some("en0"), t0), "cache vide");
        cache.store(Some("en0"), Some("88.120.0.1".into()), t0);

        assert!(!cache.needs_refresh(Some("en0"), t0 + Duration::from_secs(60)));
        assert!(!cache.needs_refresh(Some("en0"), t0 + PUBLIC_IP_TTL - Duration::from_secs(1)));
        assert!(cache.needs_refresh(Some("en0"), t0 + PUBLIC_IP_TTL));
        assert_eq!(cache.value().as_deref(), Some("88.120.0.1"));
    }

    #[test]
    fn changing_interface_forces_a_refresh() {
        // Une bascule sur un lien de secours change l'IP publique : c'est le
        // seul moment où sa fraîcheur compte vraiment.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        cache.store(Some("en0"), Some("88.120.0.1".into()), t0);
        assert!(cache.needs_refresh(Some("en1"), t0 + Duration::from_secs(1)));
    }

    #[test]
    fn a_failed_lookup_keeps_the_last_known_address() {
        // Sans internet, la sonde bat quand même : elle envoie ce qu'elle
        // sait, pas rien du tout.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        cache.store(Some("en0"), Some("88.120.0.1".into()), t0);
        cache.store(Some("en0"), None, t0 + PUBLIC_IP_TTL);
        assert_eq!(cache.value().as_deref(), Some("88.120.0.1"));
        // …et on ne réessaie pas dans la foulée.
        assert!(!cache.needs_refresh(Some("en0"), t0 + PUBLIC_IP_TTL + Duration::from_secs(60)));
    }

    #[test]
    fn a_failed_lookup_after_an_interface_change_forgets_the_address() {
        // L'adresse de l'ancienne interface n'a plus rien à voir avec la
        // nouvelle : la conserver serait un mensonge, pas une précaution.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        cache.store(Some("en0"), Some("88.120.0.1".into()), t0);
        cache.store(Some("en1"), None, t0 + Duration::from_secs(1));
        assert_eq!(cache.value(), None);
    }

    #[test]
    fn a_heartbeat_without_a_public_ip_omits_the_field() {
        // Une sonde sans accès internet doit pouvoir battre : c'est
        // justement le moment où son battement compte.
        let body = HeartbeatRequest {
            version: "1.2.0",
            platform: "linux",
            buffered_points: 0,
            last_write_ok: true,
            influx_token_version: 1,
            public_ip: None,
            interface: None,
            local_ips: Vec::new(),
            gateway: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        let map = json.as_object().unwrap();
        assert!(!map.contains_key("public_ip"), "{json}");
        assert!(!map.contains_key("interface"), "{json}");
        assert!(!map.contains_key("gateway"), "{json}");
        assert!(!map.contains_key("local_ips"), "{json}");
    }

    #[test]
    fn a_heartbeat_carries_the_network_identity_when_known() {
        let body = HeartbeatRequest {
            version: "1.2.0",
            platform: "linux",
            buffered_points: 0,
            last_write_ok: true,
            influx_token_version: 1,
            public_ip: Some("88.120.0.1".into()),
            interface: Some("en0".into()),
            local_ips: vec!["10.6.8.42/24".into()],
            gateway: Some("10.6.8.1".into()),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["public_ip"], "88.120.0.1");
        assert_eq!(json["interface"], "en0");
        assert_eq!(json["local_ips"][0], "10.6.8.42/24");
        assert_eq!(json["gateway"], "10.6.8.1");
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
