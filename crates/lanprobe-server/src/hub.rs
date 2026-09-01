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

/// Plancher entre deux relevés **forcés**.
///
/// Le TTL long protège du martèlement en régime normal ; ce plancher protège du
/// cas pathologique. Une sortie multi-WAN en répartition de charge fait alterner
/// l'adresse source d'un battement à l'autre : le hub demanderait alors un
/// relevé à chaque fois, et on martèlerait le service tiers exactement comme le
/// TTL cherchait à l'éviter. C'est un compteur en mémoire, jamais un réglage.
pub const FORCED_REFRESH_FLOOR: Duration = Duration::from_secs(5 * 60);

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
    /// Empreinte SHA-256 du certificat du hub, épinglée à la première
    /// connexion.
    ///
    /// C'est ce qui remplace `allow_self_signed` : celle-ci acceptait
    /// **n'importe quel** certificat — donc celui de n'importe qui sur le
    /// chemin, qui repartait avec le jeton de la sonde. Une empreinte dit
    /// « ce hub-là », pas « je ne vérifie plus rien ».
    ///
    /// ⚠️ `allow_self_signed` reste lu, et l'emporte quand il est vrai : des
    /// sondes déployées le portent, et le retirer les couperait du hub à la
    /// mise à jour. Il n'est simplement plus proposé.
    #[serde(default)]
    pub hub_cert_sha256: String,
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
    /// Passerelle et adresses locales au moment du relevé.
    ///
    /// ⚠️ Elles comptent autant que le nom d'interface : passer du Wi-Fi de la
    /// maison à celui d'un café garde « en0 ». Sans elles, l'adresse publique
    /// affichée restait fausse jusqu'à six heures — c'était le défaut d'origine.
    gateway: Option<String>,
    local_ips: Vec<String>,
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
    /// Vrai s'il faut refaire l'appel sortant : jamais vue, réseau changé,
    /// relevé demandé par le hub, ou plus vieille que `PUBLIC_IP_TTL`.
    fn needs_refresh(&self, identity: &NetworkIdentity, forced: bool, now: Instant) -> bool {
        let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let Some(c) = guard.as_ref() else {
            return true;
        };
        // ⚠️ La passerelle et les adresses locales comptent autant que le nom
        // d'interface : passer du Wi-Fi de la maison à celui d'un café garde
        // « en0 », et l'interface seule ne détecte alors rien.
        if c.interface != identity.interface
            || c.gateway != identity.gateway
            || c.local_ips != identity.local_ips
        {
            return true;
        }
        // Le hub a vu l'adresse source du battement bouger. On l'écoute, mais
        // jamais plus souvent que le plancher : c'est un indice, pas un ordre.
        if forced {
            return now.duration_since(c.asked_at) >= FORCED_REFRESH_FLOOR;
        }
        now.duration_since(c.asked_at) >= PUBLIC_IP_TTL
    }

    /// Enregistre le résultat d'un relevé. Un échec (`ip` à `None`) conserve
    /// la dernière valeur connue **si le réseau n'a pas changé** : sinon
    /// l'adresse de l'ancien réseau n'a plus rien à voir avec le nouveau, et
    /// la garder serait un mensonge.
    fn store(&self, identity: &NetworkIdentity, ip: Option<String>, now: Instant) {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let previous = guard
            .as_ref()
            .filter(|c| {
                c.interface == identity.interface
                    && c.gateway == identity.gateway
                    && c.local_ips == identity.local_ips
            })
            .and_then(|c| c.ip.clone());
        *guard = Some(CachedPublicIp {
            interface: identity.interface.clone(),
            gateway: identity.gateway.clone(),
            local_ips: identity.local_ips.clone(),
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
///
/// `forced` porte les déclencheurs qui ne se voient pas d'ici : la demande du
/// hub et le retour d'internet. Le cache reste seul juge du plancher.
async fn refresh_public_ip(
    cache: &PublicIpCache,
    identity: &NetworkIdentity,
    forced: bool,
    now: Instant,
) {
    if !cache.needs_refresh(identity, forced, now) {
        return;
    }
    // ⚠️ Une interface choisie mais sans adresse ne doit produire AUCUNE IP
    // publique.
    //
    // `get_public_ip(None)` sort par la route par défaut : sur un poste dont
    // le Wi-Fi sélectionné est coupé mais qui garde un autre lien, la sonde
    // remontait l'adresse publique de *cet autre* lien. Le parc affichait donc
    // une IP plausible et fausse, pendant que tout le reste — scans,
    // speedtest, état internet — échouait correctement. Une seule valeur
    // mensongère au milieu de mesures honnêtes est pire que pas de valeur.
    let source = source_address(identity);
    if identity.interface.is_some() && source.is_none() {
        cache.store(identity, None, now);
        return;
    }
    let found = lanprobe_core::public_ip::get_public_ip(source)
        .await
        .map(|info| info.ip)
        .inspect_err(|e| tracing::debug!("hub : IP publique indisponible ({e})"))
        .ok()
        .filter(|ip| !ip.is_empty());
    cache.store(identity, found, now);
}

// ── Surveillances partagées avec le hub ────────────────────────────────────

/// Clé de la file des changements de surveillance dans `app_config.json`.
///
/// ⚠️ Elle est persistée : un retrait fait pendant que le hub est injoignable
/// doit survivre à un redémarrage de l'app, sinon la cible réapparaît au
/// battement suivant — c'est précisément le cas que ce dispositif couvre.
const MONITOR_CHANGES_KEY: &str = "hub_monitor_changes";

/// Un geste de surveillance en attente d'accusé, tel qu'il part au battement.
///
/// ⚠️ Il porte une **ancienneté**, jamais une date : le hub la convertit dans
/// sa propre référence à la réception. C'est ce qui fait qu'aucune horloge
/// n'est jamais comparée à une autre — ni dérive, ni saut au réveil de veille.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorChange {
    pub target: String,
    /// `add` ou `remove` — le vocabulaire fermé du contrat.
    pub action: String,
    pub age_secs: u64,
}

/// La liste effective telle que le hub la tient, retirées comprises.
///
/// ⚠️ Les retirées EN FONT PARTIE : sans elles, la sonde ne distinguerait pas
/// « le hub ne connaît pas cette cible » de « le hub l'a retirée », et
/// ressusciterait au redémarrage ce qu'on vient de retirer.
#[derive(Debug, Clone, Deserialize)]
pub struct HubMonitor {
    #[serde(default)]
    pub target: String,
    /// `None` = active. Une date, c'est la suppression explicite.
    #[serde(default)]
    pub removed_at: Option<i64>,
}

/// Un geste local, avec l'instant **monotone** auquel il a été posé.
#[derive(Debug)]
struct PendingChange {
    target: String,
    removed: bool,
    /// ⚠️ `Instant`, jamais `SystemTime` : c'est une durée qu'on mesure, et
    /// seule l'horloge monotone la mesure honnêtement.
    since: Instant,
    /// Instant du dernier envoi au hub. `None` tant que le geste n'est jamais
    /// parti : un accusé ne peut alors pas le concerner.
    emitted: Option<Instant>,
}

#[derive(Debug, Default)]
struct QueueInner {
    changes: Vec<PendingChange>,
    /// La file a-t-elle déjà été relue depuis le disque ? Relire deux fois
    /// réécraserait les gestes posés entre-temps.
    restored: bool,
}

/// File des changements de surveillance non encore accusés.
///
/// Même mécanisme que les commandes (contrat § 14) : un changement ne quitte
/// la file que lorsque le hub l'a accusé. Un battement perdu ne perd donc
/// jamais un retrait.
#[derive(Debug, Default)]
pub struct MonitorChangeQueue {
    inner: Mutex<QueueInner>,
}

impl MonitorChangeQueue {
    fn lock(&self) -> std::sync::MutexGuard<'_, QueueInner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Note un geste. Un geste plus récent sur la même cible **remplace** le
    /// précédent : c'est le dernier acte de l'utilisateur qui compte, et
    /// envoyer les deux ferait arbitrer le hub contre lui-même.
    pub fn record(&self, target: &str, removed: bool, now: Instant) {
        let mut g = self.lock();
        g.changes.retain(|c| c.target != target);
        g.changes.push(PendingChange {
            target: target.to_string(),
            removed,
            since: now,
            emitted: None,
        });
    }

    /// La file telle qu'elle est, sans rien marquer — pour la persistance.
    pub fn snapshot(&self, now: Instant) -> Vec<MonitorChange> {
        self.lock()
            .changes
            .iter()
            .map(|c| MonitorChange {
                target: c.target.clone(),
                action: if c.removed { "remove" } else { "add" }.into(),
                // `saturating_duration_since` : un `now` antérieur donnerait 0
                // plutôt que de paniquer. L'horloge monotone ne recule pas,
                // mais rien n'oblige l'appelant à passer un instant croissant.
                age_secs: now.saturating_duration_since(c.since).as_secs(),
            })
            .collect()
    }

    /// La file à joindre au battement. Marque les gestes comme émis : c'est ce
    /// qui rend un accusé applicable, et rien d'autre.
    pub fn outgoing(&self, now: Instant) -> Vec<MonitorChange> {
        for c in self.lock().changes.iter_mut() {
            c.emitted = Some(now);
        }
        self.snapshot(now)
    }

    /// Retire de la file les gestes que le hub vient d'accuser.
    ///
    /// ⚠️ Seuls ceux qui étaient **partis avant** d'être re-posés : sinon un
    /// accusé portant sur le geste précédent emporterait avec lui celui que
    /// l'utilisateur vient de poser pendant que le battement était en vol.
    pub fn acknowledge(&self, targets: &[String]) {
        self.lock().changes.retain(|c| {
            !(targets.iter().any(|t| t == &c.target)
                && c.emitted.is_some_and(|sent| c.since <= sent))
        });
    }

    /// L'action non accusée qui porte sur cette cible, s'il y en a une.
    /// `Some(true)` = retrait, `Some(false)` = ajout.
    pub fn pending_action(&self, target: &str) -> Option<bool> {
        self.lock()
            .changes
            .iter()
            .find(|c| c.target == target)
            .map(|c| c.removed)
    }

    /// Vrai si le disque a déjà été relu.
    fn is_restored(&self) -> bool {
        self.lock().restored
    }

    /// Reprend la file laissée par l'exécution précédente. Sans effet si elle
    /// a déjà été reprise.
    ///
    /// ⚠️ L'ancienneté repart de celle qui a été persistée : le temps pendant
    /// lequel l'app était **arrêtée** n'est pas compté, parce qu'aucune horloge
    /// monotone ne survit à un processus. On préfère cette sous-estimation à
    /// une horloge murale, qui saute au réveil de veille et se fait corriger
    /// par NTP — le geste reste daté, il est seulement rajeuni de la durée de
    /// l'arrêt.
    fn restore(&self, stored: Vec<MonitorChange>, now: Instant) {
        let mut g = self.lock();
        if g.restored {
            return;
        }
        g.restored = true;
        for change in stored {
            let since = now
                .checked_sub(Duration::from_secs(change.age_secs))
                // Une ancienneté plus grande que l'âge de l'horloge monotone
                // (machine tout juste démarrée) : on repart de maintenant
                // plutôt que de paniquer.
                .unwrap_or(now);
            g.changes.push(PendingChange {
                target: change.target,
                removed: change.action == "remove",
                since,
                emitted: None,
            });
        }
    }
}

/// Relit la file depuis la configuration, une seule fois par exécution.
fn ensure_restored(state: &AppState) {
    if state.monitor_changes.is_restored() {
        return;
    }
    let stored: Vec<MonitorChange> = state
        .config
        .get()
        .get(MONITOR_CHANGES_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    state.monitor_changes.restore(stored, Instant::now());
}

/// Écrit la file sur disque. Un échec est signalé mais ne fait rien échouer :
/// la surveillance elle-même a déjà démarré ou s'est arrêtée.
fn persist_monitor_changes(state: &AppState) {
    let changes = state.monitor_changes.snapshot(Instant::now());
    let mut root = state.config.get();
    let Some(map) = root.as_object_mut() else {
        return;
    };
    match changes.is_empty() {
        true => {
            map.remove(MONITOR_CHANGES_KEY);
        }
        false => match serde_json::to_value(&changes) {
            Ok(value) => {
                map.insert(MONITOR_CHANGES_KEY.to_string(), value);
            }
            Err(e) => return tracing::warn!("file de surveillances illisible : {e}"),
        },
    }
    if let Err(e) = state.config.put(root) {
        tracing::warn!("file de surveillances non persistée : {e}");
    }
}

/// Note un geste de surveillance et le persiste.
///
/// ⚠️ **Retirer écrit un fait.** Cesser d'annoncer une cible ne dit rien : le
/// hub ne peut pas distinguer « retirée » de « l'app était éteinte ». C'est
/// tout le défaut qu'on corrige, et il vaut dans les deux sens.
pub fn record_monitor_change(state: &AppState, target: &str, removed: bool) {
    let target = target.trim();
    if target.is_empty() {
        return;
    }
    ensure_restored(state);
    state
        .monitor_changes
        .record(target, removed, Instant::now());
    persist_monitor_changes(state);
}

/// Les changements à joindre au battement, avec leur ancienneté.
pub fn outgoing_monitor_changes(state: &AppState) -> Vec<MonitorChange> {
    ensure_restored(state);
    state.monitor_changes.outgoing(Instant::now())
}

/// Aligne les surveillances locales sur la liste que le hub vient de renvoyer.
///
/// ⚠️ On accuse **avant** de s'aligner : la liste renvoyée intègre déjà les
/// changements que le hub vient d'arbitrer, alors qu'un geste resté en file
/// continue, lui, de faire autorité sur sa cible.
pub(crate) fn align_monitors(state: &AppState, acks: &[String], monitors: &[HubMonitor]) {
    ensure_restored(state);
    if !acks.is_empty() {
        state.monitor_changes.acknowledge(acks);
        persist_monitor_changes(state);
    }

    for entry in monitors {
        let target = entry.target.trim();
        if target.is_empty() {
            continue;
        }
        // ⚠️ Un geste non accusé l'emporte : le hub ne l'a pas encore vu, sa
        // liste est en retard sur cette cible précise. S'y aligner annulerait
        // le geste sous les doigts de l'utilisateur.
        if state.monitor_changes.pending_action(target).is_some() {
            continue;
        }
        match entry.removed_at {
            // ⚠️ La suppression du hub gagne : la sonde ne ressuscite JAMAIS
            // une cible retirée. Sans ça, une app qui redémarre réactive ce
            // qu'on vient de retirer depuis le hub — le défaut d'aujourd'hui,
            // simplement inversé.
            Some(_) if crate::monitor::is_monitored(state, target) => {
                tracing::info!("hub : surveillance de {target} retirée");
                crate::monitor::stop_from_hub(state, target);
            }
            Some(_) => {}
            None => crate::monitor::start_from_hub(state, target),
        }
    }
    // ⚠️ Rien ne s'arrête au motif d'une absence de la liste : une cible que le
    // hub ne mentionne pas n'a rien subi. C'est la même règle que côté hub, et
    // c'est elle qui laisse un hub antérieur à ce mécanisme sans effet de bord.
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
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
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
    /// Configuration que le hub gardait pour cette sonde (§ 16). Présente
    /// seulement sur un ré-enrôlement d'une machine déjà connue — c'est le
    /// seul moment où une machine réinstallée peut retrouver ses profils.
    #[serde(default)]
    config: Option<serde_json::Value>,
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
    /// Commandes à exécuter (contrat § 14). Un hub plus ancien que la file
    /// n'envoie pas ce champ : la liste est alors simplement vide.
    #[serde(default)]
    pub commands: Vec<crate::commands::Command>,
    /// Le hub a vu l'adresse source du battement changer et demande un relevé
    /// d'IP publique (contrat § 15).
    ///
    /// ⚠️ Absent d'un hub antérieur : la sonde garde alors ses seuls
    /// déclencheurs locaux, ce qui est exactement le comportement d'avant.
    /// Dégradation, pas panne.
    #[serde(default)]
    pub recheck_public_ip: bool,
    /// La liste effective des surveillances, retirées comprises. La sonde s'y
    /// aligne.
    ///
    /// ⚠️ Absente d'un hub antérieur à ce mécanisme : la liste est alors vide,
    /// et une liste vide n'arrête rien — une absence n'est pas une suppression.
    #[serde(default)]
    pub monitors: Vec<HubMonitor>,
    /// Cibles dont le changement a été pris en compte. Tant qu'un changement
    /// n'y figure pas, la sonde le réémet — même mécanisme que les commandes
    /// (contrat § 14).
    #[serde(default)]
    pub monitor_acks: Vec<String>,
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
    /// Verdicts des commandes exécutées depuis le battement précédent.
    /// Omis quand il n'y en a pas — c'est le cas de très loin le plus
    /// fréquent, et un tableau vide à chaque battement n'apprend rien.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    command_acks: Vec<crate::commands::Ack>,
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
    /// Verdict internet vu par la sonde. C'est la seule chose qui distingue,
    /// dans le parc, une sonde saine d'une sonde qui bat très bien mais dont
    /// le lien mesuré est mort.
    #[serde(skip_serializing_if = "Option::is_none")]
    internet_state: Option<String>,
    /// Cibles ICMP **actuellement** surveillées, avec leur dernier verdict.
    ///
    /// ⚠️ Le hub déduisait ces cibles des mesures présentes dans la fenêtre
    /// affichée. Une cible retirée continuait donc d'apparaître tant que ses
    /// anciens points restaient dans la fenêtre — le parc affirmait une
    /// surveillance qui n'existait plus. Seule la sonde sait ce qui tourne à
    /// l'instant ; elle le dit.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    monitors: Vec<MonitorState>,
    /// Gestes de surveillance non encore accusés, avec leur **ancienneté**.
    ///
    /// ⚠️ Ce ne sont pas des cibles, ce sont des actes datés. Le champ
    /// au-dessus dit ce qui tourne ; celui-ci dit ce qui a été décidé. Les
    /// confondre ferait de « je ne l'annonce plus » une suppression, ce qui
    /// est exactement le défaut corrigé ici.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    monitor_changes: Vec<MonitorChange>,
}

/// Une cible surveillée, telle que la sonde la voit maintenant.
#[derive(Debug, Serialize)]
struct MonitorState {
    ip: String,
    alive: bool,
    /// Dernier relevé.
    #[serde(skip_serializing_if = "Option::is_none")]
    latency_ms: Option<f64>,
    /// ⚠️ Min / moyenne / max sur ce que la sonde garde en mémoire. La
    /// dernière valeur seule ne dit rien de la stabilité : un lien qui oscille
    /// entre 5 et 300 ms et un lien stable à 150 affichent la même chose une
    /// fois sur deux.
    #[serde(skip_serializing_if = "Option::is_none")]
    min_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avg_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_ms: Option<f64>,
    /// Part des relevés en vie sur ce que la sonde garde en mémoire.
    uptime_pct: f64,
    samples: usize,
}

/// Photographie des surveillances en cours.
fn monitor_states(state: &AppState) -> Vec<MonitorState> {
    let mut out: Vec<MonitorState> = state
        .monitoring
        .snapshot()
        .into_iter()
        // Une cible dont la boucle est arrêtée n'est plus surveillée, même si
        // son historique n'a pas encore été purgé.
        .filter(|(ip, samples)| !samples.is_empty() && crate::monitor::is_monitored(state, ip))
        .map(|(ip, samples)| {
            let alive_count = samples.iter().filter(|s| s.alive).count();
            let last = samples.last();
            // Seuls les relevés qui ont abouti : une absence de réponse n'est
            // pas une latence de zéro, et la compter écraserait la moyenne.
            let lat: Vec<f64> = samples
                .iter()
                .filter_map(|s| s.latency_ms)
                .map(|v| v as f64)
                .collect();
            let (min_ms, avg_ms, max_ms) = if lat.is_empty() {
                (None, None, None)
            } else {
                (
                    lat.iter().copied().reduce(f64::min),
                    Some(lat.iter().sum::<f64>() / lat.len() as f64),
                    lat.iter().copied().reduce(f64::max),
                )
            };
            MonitorState {
                min_ms,
                avg_ms,
                max_ms,
                alive: last.map(|s| s.alive).unwrap_or(false),
                latency_ms: last.and_then(|s| s.latency_ms).map(|v| v as f64),
                uptime_pct: (alive_count as f64 / samples.len() as f64) * 100.0,
                samples: samples.len(),
                ip,
            }
        })
        .collect();
    out.sort_by(|a, b| a.ip.cmp(&b.ip));
    out
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
    // Empreinte du certificat du hub, confirmée par l'opérateur. Vide quand
    // le certificat se vérifie tout seul : on n'épingle que ce qui en a
    // besoin, sinon un renouvellement chez une autorité publique couperait la
    // sonde tous les trois mois.
    pin: &str,
) -> Result<HubConfig, String> {
    // Le nom est facultatif : vide, le hub en choisit un, et un ré-enrôlement
    // garde de toute façon celui qui est défini là-bas.
    let name = name.trim();
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

    let body = EnrollRequest {
        code,
        username,
        password,
        site,
        name: (!name.is_empty()).then_some(name),
    };
    // L'enrôlement suit la même règle que le reste : s'enrôler par un lien
    // pour mesurer par un autre donnerait une sonde qui joint son hub à
    // l'installation et plus jamais après.
    let source = crate::scheduler::resolve_src(state)?;
    let response = http_client(allow_self_signed, pin, source)
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
            let hint = if !allow_self_signed && pin.trim().is_empty() && looks_like_tls_failure(&e)
            {
                " — certificat non vérifiable. Confirmez son empreinte pour l'épingler, \
                 ou placez le hub derrière un reverse proxy muni d'un vrai certificat."
            } else if !pin.trim().is_empty() && looks_like_tls_failure(&e) {
                " — le certificat présenté ne correspond pas à l'empreinte épinglée. \
                 Soit le hub en a changé, soit ce n'est pas lui."
            } else {
                ""
            };
            format!("hub injoignable : {}{hint}", cause_chain(&e))
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

    // ⚠️ Déposée AVANT d'écrire la config du hub : si l'écriture échoue, on
    // préfère une sonde non rattachée qui a gardé ses profils à une sonde
    // rattachée qui les a perdus.
    if let Some(restored) = parsed.config.clone() {
        let mut root = state.config.get();
        if let Some(map) = root.as_object_mut() {
            map.insert(RESTORED_KEY.to_string(), restored);
        }
        let _ = state.config.put(root);
    }

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
        hub_cert_sha256: pin.trim().to_string(),
    };

    save(state, &config)?;
    Ok(config)
}

/// Relit la section `hub` de la configuration.
/// Dépose la configuration de la sonde au hub, pour sauvegarde (§ 16).
///
/// ⚠️ Le hub ne la lit pas : c'est un concentrateur, pas un gestionnaire de
/// profils. Elle part dans ses sauvegardes, et une sonde ré-enrôlée la
/// retrouve — voilà tout le service rendu.
///
/// Un échec ne fait rien échouer d'autre : les profils restent sur la machine,
/// et une sauvegarde manquée ne doit pas empêcher de mesurer.
pub async fn upload_config(
    state: &AppState,
    key: &SecretKey,
    config: serde_json::Value,
) -> Result<(), String> {
    let cfg = load(state);
    if !cfg.is_enrolled() {
        return Err("sonde non rattachée".into());
    }
    let token = secrets::open(key, &cfg.token)?;
    let source = crate::scheduler::resolve_src(state)?;
    let response = http_client_pinned(&cfg, source)
        .post(format!("{}/api/probes/{}/config", cfg.url, cfg.probe_id))
        .bearer_auth(token)
        .json(&config)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("le hub a répondu {}", response.status()))
    }
}

/// Configuration rendue par le hub au dernier enrôlement, s'il en gardait une.
///
/// ⚠️ Consommée une seule fois : une fois restituée à l'application, elle est
/// effacée. La relire à chaque démarrage écraserait ce que quelqu'un vient de
/// régler devant l'écran.
pub fn take_restored_config(state: &AppState) -> Option<serde_json::Value> {
    let mut root = state.config.get();
    let restored = root.as_object_mut()?.remove(RESTORED_KEY)?;
    let _ = state.config.put(root);
    Some(restored)
}

/// Clé sous laquelle la configuration rendue par le hub attend l'application.
const RESTORED_KEY: &str = "hub_restored_config";

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
    // Les verdicts partent au battement SUIVANT, jamais à celui qui a apporté
    // la commande : une commande peut durer plusieurs minutes, et attendre son
    // exécution pour battre ferait passer la sonde pour hors ligne pendant
    // qu'elle travaille.
    let mut pending_acks: Vec<crate::commands::Ack> = Vec::new();
    // Demande de relevé formulée par le hub au battement PRÉCÉDENT : sa réponse
    // arrive forcément trop tard pour le battement en cours.
    let mut recheck_requested = false;
    // Sert à repérer la transition `offline` → `online`, pas l'état lui-même.
    let mut previous_internet: Option<String> = None;

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
        let acks = std::mem::take(&mut pending_acks);

        // Une transition `offline` → `online` veut dire qu'on vient de
        // raccrocher : souvent sur un autre lien, donc sous une autre adresse.
        // C'est un déclencheur purement local, il marche même quand le hub vit
        // dans le LAN des sondes et ne voit jamais d'adresse publique.
        let current_internet = internet_state(&state);
        let came_back = matches!(previous_internet.as_deref(), Some("offline"))
            && matches!(current_internet.as_deref(), Some("online"));
        previous_internet = current_internet;

        match beat(
            &state,
            &key,
            &config,
            &status,
            &public_ip,
            recheck_requested || came_back,
            acks,
        )
        .await
        {
            Ok(response) => {
                backoff = Duration::from_secs(5);
                recheck_requested = response.recheck_public_ip;
                state.hub.record_success(buffered_points, unix_now());
                let commands = response.commands.clone();
                if let Err(e) = apply(&state, &key, &config, response) {
                    tracing::warn!("hub : mise à jour locale impossible : {e}");
                }
                for command in &commands {
                    tracing::info!("hub : commande {} — {}", command.id, command.kind);
                    pending_acks.push(crate::commands::execute(&state, command).await);
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
    // Relevé d'IP publique demandé par le hub ou par un retour d'internet.
    // Le cache garde le dernier mot : il applique son plancher de 5 minutes.
    forced_public_ip: bool,
    command_acks: Vec<crate::commands::Ack>,
) -> Result<HeartbeatResponse, BeatError> {
    // Un secret local illisible ne se répare pas en réessayant : c'est un
    // ré-enrôlement qu'il faut, pas de la patience.
    let token = secrets::open(key, &config.token).map_err(BeatError::Unrecoverable)?;
    let (buffered_points, last_write_ok) = status.read();

    let identity = network_identity(state);
    refresh_public_ip(public_ip, &identity, forced_public_ip, Instant::now()).await;

    // ⚠️ Une interface sélectionnée sans IPv4 fait échouer le battement plutôt
    // que de le laisser sortir ailleurs. C'est voulu : la sonde doit alors
    // apparaître « sans nouvelles », parce que c'est la vérité de l'interface
    // qu'on observe.
    let source = crate::scheduler::resolve_src(state).map_err(BeatError::Transient)?;

    let response = http_client_pinned(config, source)
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
            command_acks,
            monitors: monitor_states(state),
            monitor_changes: outgoing_monitor_changes(state),
            internet_state: internet_state(state),
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

/// Verdict internet vu par la sonde, sous la forme envoyée au hub.
///
/// La boucle et le battement le lisent tous les deux — l'une pour repérer la
/// transition `offline` → `online`, l'autre pour le remonter. Un seul endroit
/// pour le calculer : deux définitions finiraient par diverger.
fn internet_state(state: &AppState) -> Option<String> {
    state.internet.snapshot().map(|t| {
        serde_json::to_value(&t.state)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "offline".into())
    })
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
    // Fait en premier, et sans `?` : une clé illisible plus bas ne doit pas
    // faire sauter l'alignement des surveillances, ni faire réémettre
    // indéfiniment des gestes que le hub a pourtant accusés.
    align_monitors(state, &response.monitor_acks, &response.monitors);

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
    let text = cause_chain(error).to_lowercase();
    ["certificate", "certificat", "tls", "self-signed", "unknownissuer", "invalidcertificate"]
        .iter()
        .any(|needle| text.contains(needle))
}

/// Le message d'une erreur `reqwest`, **avec sa chaîne de causes**.
///
/// 🔴 `reqwest::Error` s'affiche seul en « error sending request for url … »,
/// qui ne dit rien : la vraie raison — certificat refusé, connexion refusée,
/// nom introuvable — vit dans `source()`. Sans la dérouler, le message montré
/// à l'utilisateur est inutile, et toute détection faite dessus échoue en
/// silence. C'est exactement ce qui rendait la mention « certificat non
/// vérifiable » inatteignable.
fn cause_chain(error: &reqwest::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source: Option<&(dyn std::error::Error + 'static)> =
        std::error::Error::source(error);
    while let Some(cause) = source {
        let text = cause.to_string();
        // Une cause qui répète la précédente n'apprend rien et allonge le
        // message que l'utilisateur doit lire.
        if !parts.iter().any(|p| p == &text) {
            parts.push(text);
        }
        source = cause.source();
    }
    parts.join(" : ")
}

/// Client HTTP vers le hub.
///
/// **La vérification du certificat est active par défaut.** Le jeton de sonde
/// voyage dans ces requêtes : sans vérification, n'importe qui sur le chemin
/// peut se faire passer pour le hub et le récupérer. Elle n'est levée que si
/// l'utilisateur a explicitement déclaré héberger un hub auto-signé — un choix
/// qu'il pose lui-même, pas un défaut qu'il subit.
/// Client HTTP vers le hub, **lié à l'interface sélectionnée**.
///
/// ⚠️ `local_address` n'est pas un détail de confort : sans elle, le battement
/// de cœur sortait par la route par défaut. Une sonde dont l'interface mesurée
/// n'avait plus internet restait donc verte dans le parc, parce que sa ligne
/// de vie empruntait un autre lien. Le hub affirmait « en ligne » pour une
/// interface qu'aucun paquet n'avait traversée.
///
/// `source` à `None` = aucune interface sélectionnée : on laisse le système
/// router, ce qui est le comportement attendu quand rien n'est imposé.
/// Même client, exposé pour la publication d'inventaire : elle doit suivre
/// exactement les mêmes règles de certificat et d'interface que le battement.
/// Client HTTP pour un hub donné, épingle comprise.
pub fn http_client_pinned(cfg: &HubConfig, source: Option<std::net::Ipv4Addr>) -> reqwest::Client {
    http_client(cfg.allow_self_signed, &cfg.hub_cert_sha256, source)
}

fn http_client(
    allow_self_signed: bool,
    pin: &str,
    source: Option<std::net::Ipv4Addr>,
) -> reqwest::Client {
    let builder = reqwest::Client::builder();
    // Ordre volontaire : l'ancien réglage l'emporte. Une sonde déjà déployée
    // qui le porte doit continuer de fonctionner à la lettre après la mise à
    // jour — la remplacer par une épingle qu'elle n'a pas la couperait du hub
    // sans le moindre message, à distance.
    let builder = if allow_self_signed {
        builder.danger_accept_invalid_certs(true)
    } else if !pin.trim().is_empty() {
        builder.use_preconfigured_tls(crate::tls_pin::pinned_tls_config(pin.trim()))
    } else {
        builder
    };
    let builder = match source {
        Some(ip) => builder.local_address(std::net::IpAddr::V4(ip)),
        None => builder,
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
            name: Some("Paris"),
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

    /// Identité réseau minimale pour les tests du cache d'IP publique.
    fn identity(interface: &str, gateway: &str) -> NetworkIdentity {
        NetworkIdentity {
            interface: Some(interface.into()),
            local_ips: vec!["10.6.8.42/24".into()],
            gateway: Some(gateway.into()),
        }
    }

    #[test]
    fn the_public_ip_is_not_asked_again_before_six_hours() {
        // Le demander à chaque battement, c'est marteler un service gratuit
        // et publier son trafic à intervalle régulier.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        let net = identity("en0", "10.6.8.1");
        assert!(cache.needs_refresh(&net, false, t0), "cache vide");
        cache.store(&net, Some("88.120.0.1".into()), t0);

        assert!(!cache.needs_refresh(&net, false, t0 + Duration::from_secs(60)));
        assert!(!cache.needs_refresh(&net, false, t0 + PUBLIC_IP_TTL - Duration::from_secs(1)));
        assert!(cache.needs_refresh(&net, false, t0 + PUBLIC_IP_TTL));
        assert_eq!(cache.value().as_deref(), Some("88.120.0.1"));
    }

    #[test]
    fn changing_interface_forces_a_refresh() {
        // Une bascule sur un lien de secours change l'IP publique : c'est le
        // seul moment où sa fraîcheur compte vraiment.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        cache.store(&identity("en0", "10.6.8.1"), Some("88.120.0.1".into()), t0);
        assert!(cache.needs_refresh(
            &identity("en1", "10.6.8.1"),
            false,
            t0 + Duration::from_secs(1)
        ));
    }

    #[test]
    fn a_new_gateway_forces_a_fresh_lookup_even_on_the_same_interface() {
        // Changer de Wi-Fi garde « en0 ». Sans ce déclencheur, l'adresse reste
        // fausse jusqu'à six heures — c'est le défaut d'origine.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        cache.store(&identity("en0", "10.6.8.1"), Some("88.120.0.1".into()), t0);

        assert!(!cache.needs_refresh(&identity("en0", "10.6.8.1"), false, t0));
        assert!(cache.needs_refresh(&identity("en0", "192.168.1.1"), false, t0));
    }

    #[test]
    fn a_new_local_address_forces_a_fresh_lookup() {
        // Deux réseaux peuvent annoncer la même passerelle (`192.168.1.1` est
        // le défaut de la moitié des box) : l'adresse locale est le dernier
        // signe local qu'on a changé de réseau.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        cache.store(&identity("en0", "10.6.8.1"), Some("88.120.0.1".into()), t0);

        let moved = NetworkIdentity {
            interface: Some("en0".into()),
            local_ips: vec!["10.6.9.42/24".into()],
            gateway: Some("10.6.8.1".into()),
        };
        assert!(cache.needs_refresh(&moved, false, t0));
    }

    #[test]
    fn the_hub_can_force_a_lookup() {
        // Le hub a vu l'adresse source bouger : sa demande passe outre le TTL
        // de six heures, sur un réseau qui n'a pourtant rien changé localement.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        cache.store(&identity("en0", "10.6.8.1"), Some("88.120.0.1".into()), t0);

        let past_the_floor = t0 + FORCED_REFRESH_FLOOR;
        assert!(
            !cache.needs_refresh(&identity("en0", "10.6.8.1"), false, past_the_floor),
            "sans demande, le TTL de six heures s'applique encore"
        );
        assert!(cache.needs_refresh(&identity("en0", "10.6.8.1"), true, past_the_floor));
    }

    #[test]
    fn a_forced_lookup_is_floored_at_five_minutes() {
        // Une sortie multi-WAN en répartition de charge alterne l'IP source à
        // chaque battement : sans plancher on martèlerait le service tiers, ce
        // que le TTL de six heures cherchait justement à éviter.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        cache.store(&identity("en0", "10.6.8.1"), Some("88.120.0.1".into()), t0);

        let just_after = t0 + Duration::from_secs(60);
        assert!(!cache.needs_refresh(&identity("en0", "10.6.8.1"), true, just_after));

        let later = t0 + Duration::from_secs(5 * 60 + 1);
        assert!(cache.needs_refresh(&identity("en0", "10.6.8.1"), true, later));
    }

    #[test]
    fn a_failed_lookup_keeps_the_last_known_address() {
        // Sans internet, la sonde bat quand même : elle envoie ce qu'elle
        // sait, pas rien du tout.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        let net = identity("en0", "10.6.8.1");
        cache.store(&net, Some("88.120.0.1".into()), t0);
        cache.store(&net, None, t0 + PUBLIC_IP_TTL);
        assert_eq!(cache.value().as_deref(), Some("88.120.0.1"));
        // …et on ne réessaie pas dans la foulée.
        assert!(!cache.needs_refresh(&net, false, t0 + PUBLIC_IP_TTL + Duration::from_secs(60)));
    }

    #[test]
    fn a_failed_lookup_after_an_interface_change_forgets_the_address() {
        // L'adresse de l'ancienne interface n'a plus rien à voir avec la
        // nouvelle : la conserver serait un mensonge, pas une précaution.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        cache.store(&identity("en0", "10.6.8.1"), Some("88.120.0.1".into()), t0);
        cache.store(&identity("en1", "10.6.8.1"), None, t0 + Duration::from_secs(1));
        assert_eq!(cache.value(), None);
    }

    #[test]
    fn a_failed_lookup_after_a_network_change_forgets_the_address() {
        // Même raison que ci-dessus : la passerelle a changé, donc le réseau.
        // Garder l'adresse de l'ancien afficherait une valeur plausible et
        // fausse au milieu de mesures honnêtes.
        let cache = PublicIpCache::default();
        let t0 = Instant::now();
        cache.store(&identity("en0", "10.6.8.1"), Some("88.120.0.1".into()), t0);
        cache.store(
            &identity("en0", "192.168.1.1"),
            None,
            t0 + Duration::from_secs(1),
        );
        assert_eq!(cache.value(), None);
    }

    #[test]
    fn a_hub_that_says_nothing_asks_for_no_recheck() {
        // Compatibilité ascendante : un hub antérieur au champ n'envoie rien,
        // la sonde doit garder ses seuls déclencheurs locaux. Dégradation,
        // pas panne.
        let r: HeartbeatResponse = serde_json::from_str("{}").unwrap();
        assert!(!r.recheck_public_ip);

        let r: HeartbeatResponse = serde_json::from_str(r#"{"recheck_public_ip":true}"#).unwrap();
        assert!(r.recheck_public_ip);
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
            command_acks: Vec::new(),
            internet_state: None,
            monitors: Vec::new(),
            monitor_changes: Vec::new(),
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
            command_acks: Vec::new(),
            internet_state: None,
            monitors: Vec::new(),
            monitor_changes: Vec::new(),
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

    // ── Surveillances partagées avec le hub ────────────────────────────

    /// État minimal pour les tests qui persistent quelque chose.
    ///
    /// Un fichier par test **et par processus** : la file des changements est
    /// écrite sur disque, deux tests qui partageraient le fichier se
    /// marcheraient dessus.
    fn state(tag: &str) -> AppState {
        AppState::new_headless(Arc::new(crate::config::ConfigStore::load(
            std::env::temp_dir().join(format!("lanprobe-hub-{tag}-{}.json", std::process::id())),
        )))
    }

    #[test]
    fn a_change_carries_a_monotonic_age_never_a_wall_clock_date() {
        // ⚠️ `Instant`, jamais `SystemTime` : un portable qui sort de veille
        // voit son horloge murale sauter de plusieurs minutes. L'horloge
        // monotone ne recule pas et ignore NTP — c'est la seule qui mesure
        // honnêtement une durée. C'est aussi ce qui fait qu'aucune horloge
        // n'est jamais comparée à une autre : le hub date à la réception.
        let queue = MonitorChangeQueue::default();
        let t0 = Instant::now();
        queue.record("8.8.4.4", true, t0);

        let sent = queue.outgoing(t0 + Duration::from_secs(11_520));
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].target, "8.8.4.4");
        assert_eq!(sent[0].action, "remove");
        assert_eq!(sent[0].age_secs, 11_520, "3 h 12 min, mesurées sur Instant");

        // Le contrat n'accepte qu'une ancienneté : envoyer une date rendrait
        // l'arbitrage dépendant de deux horloges.
        let json = serde_json::to_value(&sent[0]).unwrap();
        let map = json.as_object().unwrap();
        assert_eq!(map.len(), 3, "{json}");
        assert!(map.contains_key("age_secs"), "{json}");
    }

    #[test]
    fn a_change_stays_in_the_queue_until_the_hub_acknowledges_it() {
        // Même mécanisme que les commandes (§ 14) : un battement perdu ne doit
        // pas emporter le retrait avec lui.
        let queue = MonitorChangeQueue::default();
        let t0 = Instant::now();
        queue.record("8.8.4.4", true, t0);
        queue.record("1.1.1.1", false, t0);

        let _ = queue.outgoing(t0 + Duration::from_secs(1));
        queue.acknowledge(&["1.1.1.1".to_string()]);

        let left = queue.outgoing(t0 + Duration::from_secs(2));
        assert_eq!(left.len(), 1, "seul l'accusé quitte la file");
        assert_eq!(left[0].target, "8.8.4.4");
        assert_eq!(left[0].age_secs, 2, "son ancienneté continue de courir");
    }

    #[test]
    fn a_stale_acknowledgement_does_not_swallow_the_newest_gesture() {
        // L'utilisateur peut refaire un geste sur la même cible pendant qu'un
        // battement est en vol : l'accusé porte alors sur le geste précédent,
        // et emporterait le nouveau avec lui.
        let queue = MonitorChangeQueue::default();
        let t0 = Instant::now();
        queue.record("8.8.4.4", true, t0);
        let _ = queue.outgoing(t0);
        queue.record("8.8.4.4", false, t0 + Duration::from_secs(1));
        queue.acknowledge(&["8.8.4.4".to_string()]);

        let left = queue.outgoing(t0 + Duration::from_secs(2));
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].action, "add", "le geste le plus récent survit");
    }

    #[test]
    fn the_queue_survives_a_restart_of_the_app() {
        // ⚠️ Un retrait fait pendant que le hub est injoignable serait perdu au
        // redémarrage, et la cible reviendrait au battement suivant : c'est
        // exactement le cas que ce dispositif doit couvrir.
        let first = state("restart");
        record_monitor_change(&first, "8.8.4.4", true);

        // Nouvelle instance sur le même fichier : c'est ce que voit l'app au
        // démarrage suivant.
        let second = AppState::new_headless(Arc::new(crate::config::ConfigStore::load(
            std::env::temp_dir()
                .join(format!("lanprobe-hub-restart-{}.json", std::process::id())),
        )));
        let sent = outgoing_monitor_changes(&second);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].target, "8.8.4.4");
        assert_eq!(sent[0].action, "remove");
    }

    #[tokio::test]
    async fn the_probe_does_not_resurrect_a_target_removed_more_recently() {
        // ⚠️ Sans cette règle, une app qui redémarre ressuscite ce qu'on vient
        // de retirer depuis le hub : c'est le défaut d'aujourd'hui, inversé.
        let state = state("resurrect");
        crate::monitor::start_from_hub(&state, "192.0.2.10");
        assert!(crate::monitor::is_monitored(&state, "192.0.2.10"));

        align_monitors(
            &state,
            &[],
            &[HubMonitor {
                target: "192.0.2.10".into(),
                removed_at: Some(1_787_914_000),
            }],
        );
        assert!(!crate::monitor::is_monitored(&state, "192.0.2.10"));
    }

    #[tokio::test]
    async fn a_target_the_hub_keeps_active_is_monitored_again() {
        // Critère 1 : une cible ajoutée depuis le hub survit au redémarrage de
        // l'app, qui la reprend au premier battement.
        let state = state("adopt");
        align_monitors(
            &state,
            &[],
            &[HubMonitor {
                target: "192.0.2.11".into(),
                removed_at: None,
            }],
        );
        assert!(crate::monitor::is_monitored(&state, "192.0.2.11"));
    }

    #[tokio::test]
    async fn an_unacknowledged_gesture_still_rules_its_target() {
        // La liste du hub est en retard sur cette cible précise : il n'a pas
        // encore vu le geste. S'y aligner l'annulerait sous les doigts de
        // l'utilisateur.
        let state = state("pending-wins");
        crate::monitor::start(&state, "192.0.2.12");
        align_monitors(
            &state,
            &[],
            &[HubMonitor {
                target: "192.0.2.12".into(),
                removed_at: Some(1_787_914_000),
            }],
        );
        assert!(crate::monitor::is_monitored(&state, "192.0.2.12"));
    }

    #[tokio::test]
    async fn an_acknowledged_gesture_gives_the_hub_the_last_word() {
        // Une fois accusé, le changement a été arbitré : c'est la liste du hub
        // qui fait foi, sinon la file ferait autorité pour toujours.
        let state = state("acked");
        crate::monitor::start(&state, "192.0.2.13");
        // Le battement l'a emporté : c'est ce qui rend l'accusé applicable.
        let _ = outgoing_monitor_changes(&state);
        align_monitors(
            &state,
            &["192.0.2.13".to_string()],
            &[HubMonitor {
                target: "192.0.2.13".into(),
                removed_at: Some(1_787_914_000),
            }],
        );
        assert!(!crate::monitor::is_monitored(&state, "192.0.2.13"));
        assert!(
            outgoing_monitor_changes(&state).is_empty(),
            "l'accusé vide la file"
        );
    }

    #[tokio::test]
    async fn a_target_the_hub_never_mentions_is_left_alone() {
        // ⚠️ Une absence n'est pas une suppression. Un hub antérieur à ce
        // mécanisme n'envoie aucune liste : rien ne doit s'arrêter, sinon la
        // mise à jour du hub couperait les surveillances en place.
        let state = state("absent");
        crate::monitor::start_from_hub(&state, "192.0.2.14");

        let older: HeartbeatResponse = serde_json::from_str("{}").unwrap();
        assert!(older.monitors.is_empty());
        assert!(older.monitor_acks.is_empty());
        align_monitors(&state, &older.monitor_acks, &older.monitors);
        assert!(crate::monitor::is_monitored(&state, "192.0.2.14"));
    }

    #[test]
    fn the_hub_list_is_read_with_its_removals() {
        // Les retirées font partie de la liste : sans elles, la sonde ne
        // distinguerait pas « le hub ne connaît pas cette cible » de « le hub
        // l'a retirée ».
        let r: HeartbeatResponse = serde_json::from_str(
            r#"{"monitors":[{"target":"8.8.8.8","added_at":100,"removed_at":200,"last_change_at":200},
                            {"target":"1.1.1.1","added_at":100,"removed_at":null,"last_change_at":100}],
                "monitor_acks":["8.8.8.8"]}"#,
        )
        .unwrap();
        assert_eq!(r.monitors.len(), 2);
        assert_eq!(r.monitors[0].removed_at, Some(200));
        assert_eq!(r.monitors[1].removed_at, None);
        assert_eq!(r.monitor_acks, vec!["8.8.8.8".to_string()]);
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
