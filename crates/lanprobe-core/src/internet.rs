use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ping_async::{IcmpEchoRequestor, IcmpEchoStatus};
use serde::Serialize;
use tokio::time::sleep;

use crate::interfaces::get_interface_details;
use crate::resolver::{dns_plan, DnsPlan};

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum InternetState {
    Online,
    Limited,
    Offline,
}

pub fn derive_state(icmp_ok: bool, http_ok: bool) -> InternetState {
    // HTTP est le test principal : si la requête HTTPS passe → Online.
    // ICMP seul → Limited (ex : portail captif, pare-feu bloque HTTP).
    // Sur Linux desktop sans CAP_NET_RAW, ICMP échoue toujours ; sans
    // ce fallback l'état serait Offline alors que HTTP fonctionne.
    if http_ok {
        InternetState::Online
    } else if icmp_ok {
        InternetState::Limited
    } else {
        InternetState::Offline
    }
}

const UPTIME_WINDOW: usize = 720; // ~1h @ 5s tick
const ICMP_TARGET: &str = "1.1.1.1";
// Endpoint de détection de connexion (HTTP simple, 204 No Content, pas de TLS requis).
// Utilisé par Android/Chrome — fiable, lightweight, ne redirige pas.
const HTTP_TARGET: &str = "http://connectivitycheck.gstatic.com/generate_204";
const DNS_TARGET: &str = "cloudflare.com";
const TICK_SECS: u64 = 5;

#[derive(Debug, Serialize, Clone)]
pub struct InternetTick {
    pub state: InternetState,
    pub icmp_ok: bool,
    pub icmp_ms: Option<u64>,
    pub http_ok: bool,
    pub http_ms: Option<u64>,
    pub dns_ok: bool,
    pub dns_ms: Option<u64>,
    pub dns_target: &'static str,
    pub icmp_target: &'static str,
    pub http_target: &'static str,
    pub timestamp: u64,
    pub uptime_pct: f64,
    pub samples: usize,
}

#[derive(Default)]
pub struct InternetHistory {
    buf: Mutex<VecDeque<bool>>,
    last: Mutex<Option<InternetTick>>,
}

impl InternetHistory {
    pub fn push(&self, icmp_ok: bool) -> (f64, usize) {
        let Ok(mut buf) = self.buf.lock() else { return (100.0, 0); };
        if buf.len() >= UPTIME_WINDOW { buf.pop_front(); }
        buf.push_back(icmp_ok);
        let total = buf.len();
        let ok = buf.iter().filter(|b| **b).count();
        let pct = if total == 0 { 100.0 } else { (ok as f64 / total as f64) * 100.0 };
        (pct, total)
    }

    pub fn set_last(&self, tick: InternetTick) {
        if let Ok(mut guard) = self.last.lock() {
            *guard = Some(tick);
        }
    }

    pub fn snapshot(&self) -> Option<InternetTick> {
        self.last.lock().ok().and_then(|g| g.clone())
    }

    /// Vide la fenêtre d'uptime et le dernier tick — utilisé quand on change
    /// de profil réseau (static/DHCP) pour que l'historique de l'interface
    /// précédente ne contamine pas le pourcentage affiché après le switch.
    pub fn reset(&self) {
        if let Ok(mut buf) = self.buf.lock() { buf.clear(); }
        if let Ok(mut last) = self.last.lock() { *last = None; }
    }
}

/// Mesure la résolution DNS **de l'interface auditée**.
///
/// `getaddrinfo` passait par le résolveur système et donc par la route par
/// défaut : le DNS du VLAN client pouvait être mort, le Wi-Fi du technicien
/// répondait et la pastille restait verte. Le plan décide désormais d'où
/// part la requête et vers quel serveur — et quand aucune mesure honnête
/// n'est possible, elle échoue au lieu de mentir.
pub async fn probe_dns(plan: &DnsPlan) -> (bool, Option<u64>) {
    let binding = match plan {
        DnsPlan::System => None,
        DnsPlan::Bound(b) => Some(b),
        DnsPlan::Impossible => return (false, None),
    };
    let start = std::time::Instant::now();
    match crate::resolver::lookup(DNS_TARGET, binding, Duration::from_secs(3)).await {
        Ok(addrs) if !addrs.is_empty() => (true, Some(start.elapsed().as_millis() as u64)),
        _ => (false, None),
    }
}

/// Mesure ICMP de l'interface auditée. `iface` sert au liage par
/// périphérique là où il est possible (voir `ping_args`).
///
/// ⚠️ Le chemin socket brute (`ping_async`) ne sait lier que l'adresse
/// source : le liage par périphérique n'existe que sur le repli `ping`.
/// Comme la socket brute n'est disponible qu'avec CAP_NET_RAW, c'est bien le
/// repli qui sert sur un poste non privilégié — celui où le recouvrement de
/// réseaux se produit le plus souvent.
pub async fn probe_icmp(src: Option<Ipv4Addr>, iface: Option<&str>) -> (bool, Option<u64>) {
    let Ok(addr) = ICMP_TARGET.parse::<IpAddr>() else { return (false, None); };
    let src_addr: Option<IpAddr> = src.map(IpAddr::V4);
    match IcmpEchoRequestor::new(addr, src_addr, None, Some(Duration::from_millis(2000))) {
        Ok(req) => match req.send().await {
            Ok(r) if r.status() == IcmpEchoStatus::Success => {
                (true, Some(r.round_trip_time().as_millis() as u64))
            }
            _ => (false, None),
        },
        // Raw socket unavailable (no CAP_NET_RAW on Linux) → use system ping binary.
        Err(_) => probe_icmp_subprocess(ICMP_TARGET, src, iface).await,
    }
}

/// Arguments du `ping` système.
///
/// Sous Linux, lier l'IP source ne suffit pas : le *weak host model* laisse
/// un paquet portant l'adresse de l'interface A sortir par B dès que la
/// table de routage le préfère. `-I <iface>` lie la socket au périphérique
/// (SO_BINDTODEVICE) et ferme cette porte. Ailleurs on garde le liage par
/// adresse source : `-I` n'y désigne pas la même chose et casser le fallback
/// ICMP coûterait plus cher que le défaut qu'on corrige.
fn ping_args(target: &str, src: Option<Ipv4Addr>, iface: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-c".into(), "1".into(),
        "-W".into(), "2".into(),
        "-q".into(),
    ];
    #[cfg(target_os = "linux")]
    let bind = iface.map(|i| i.to_string()).or_else(|| src.map(|ip| ip.to_string()));
    #[cfg(not(target_os = "linux"))]
    let bind = {
        let _ = iface;
        src.map(|ip| ip.to_string())
    };
    if let Some(bind) = bind {
        args.push("-I".into());
        args.push(bind);
    }
    args.push(target.into());
    args
}

/// Fallback ICMP via the system `ping` binary (already has cap_net_raw/setuid).
/// Used on Linux when the process lacks CAP_NET_RAW.
async fn probe_icmp_subprocess(
    target: &str,
    src: Option<Ipv4Addr>,
    iface: Option<&str>,
) -> (bool, Option<u64>) {
    let args = ping_args(target, src, iface);

    let result = tokio::time::timeout(
        Duration::from_millis(3000),
        tokio::process::Command::new("ping")
            .args(&args)
            .output(),
    ).await;

    match result {
        Ok(Ok(out)) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            (true, parse_ping_rtt_ms(&text))
        }
        _ => (false, None),
    }
}

/// Parses the avg RTT from `ping -q` output:
/// `rtt min/avg/max/mdev = 1.234/2.345/3.456/0.100 ms`
fn parse_ping_rtt_ms(output: &str) -> Option<u64> {
    for line in output.lines() {
        if line.contains("rtt") || line.contains("round-trip") {
            if let Some(eq) = line.find('=') {
                let values: Vec<&str> = line[eq + 1..].trim().splitn(4, '/').collect();
                if values.len() >= 2 {
                    if let Ok(avg) = values[1].trim().parse::<f64>() {
                        return Some(avg.round() as u64);
                    }
                }
            }
        }
    }
    None
}

/// Mesure HTTP de l'interface auditée.
///
/// La connexion part de `src`, et le nom de la cible est résolu par le même
/// chemin : sans résolveur lié, la moitié de la mesure (la résolution)
/// décrirait encore le poste du technicien.
///
/// ⚠️ Limite connue : la connexion est liée par **adresse source**, pas par
/// périphérique. Sous Linux (weak host model), si deux interfaces ont une
/// route vers la même destination, le paquet peut sortir par l'autre.
/// `reqwest` sait lier la socket au périphérique (`.interface()`,
/// SO_BINDTODEVICE / IP_BOUND_IF), mais SO_BINDTODEVICE exige CAP_NET_RAW :
/// l'activer sans condition ferait échouer toute mesure HTTP sur une
/// installation non privilégiée. À faire quand la sonde saura si elle
/// dispose de la capacité.
pub async fn probe_http(src: Option<Ipv4Addr>, plan: &DnsPlan) -> (bool, Option<u64>) {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .user_agent("LanProbe-Internet-Probe");
    if let Some(s) = src {
        builder = builder.local_address(IpAddr::V4(s));
    }
    match plan {
        DnsPlan::System => {}
        DnsPlan::Bound(b) => {
            match crate::resolver::BoundHttpResolver::new(b, Duration::from_secs(3)) {
                Some(r) => builder = builder.dns_resolver(std::sync::Arc::new(r)),
                // Résolveur lié impossible à construire : on ne se rabat pas
                // sur le résolveur système, on rate la mesure.
                None => return (false, None),
            }
        }
        DnsPlan::Impossible => return (false, None),
    }
    let client = match builder.build() {
        Ok(c) => c,
        Err(_) => return (false, None),
    };
    let start = std::time::Instant::now();
    match client.head(HTTP_TARGET).send().await {
        Ok(r) if r.status().is_success() => {
            (true, Some(start.elapsed().as_millis() as u64))
        }
        _ => (false, None),
    }
}

pub type InternetStateHandle = Arc<InternetHistory>;
pub type SelectedInterfaceHandle = Arc<Mutex<Option<String>>>;

/// D'où partent les mesures d'un tick.
#[derive(Debug, Default)]
struct ProbeRoute {
    /// IP source imposée aux paquets.
    src: Option<Ipv4Addr>,
    /// Nom système de l'interface, pour le liage par périphérique.
    iface: Option<String>,
    dns: DnsPlan,
}

/// Résout d'où partent les mesures. Retour :
/// - Ok(route par défaut) : aucune interface choisie
/// - Ok(route liée)       : interface résolue
/// - Err : interface choisie mais pas d'IP → les probes doivent échouer
///   (offline) au lieu d'emprunter la route par défaut.
fn resolve_src_from_iface(iface: &SelectedInterfaceHandle) -> Result<ProbeRoute, ()> {
    let Ok(guard) = iface.lock() else { return Ok(ProbeRoute::default()); };
    let Some(name) = guard.clone() else { return Ok(ProbeRoute::default()); };
    drop(guard);
    let details = get_interface_details(&name);
    let plan = dns_plan(Some(&details));
    let Some(ip_str) = details.ip.as_deref() else { return Err(()); };
    let ip = ip_str.parse::<Ipv4Addr>().map_err(|_| ())?;
    Ok(ProbeRoute {
        src: Some(ip),
        // Nom système : sur macOS l'interface sélectionnée est un service
        // networksetup, que les CLI ne connaissent pas.
        iface: Some(details.bsd_name.clone().unwrap_or(name)),
        dns: plan,
    })
}

/// Boucle de monitoring internet, runtime-agnostique. Le caller choisit où
/// la faire tourner (tauri::async_runtime::spawn côté desktop, tokio::spawn
/// côté serveur headless) et fournit un callback `emit` qui publie chaque
/// tick (websocket, event Tauri, …).
pub async fn run_internet_monitor<F>(
    history: InternetStateHandle,
    iface: SelectedInterfaceHandle,
    mut emit: F,
) where
    F: FnMut(&InternetTick) + Send + 'static,
{
    loop {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            // On relit la source à chaque tick : si l'utilisateur change
            // d'interface, le prochain probe sortira par la nouvelle.
            // Si l'interface sélectionnée n'a pas d'IPv4 (Wintun, tunnel
            // sans config), on reporte offline directement au lieu de
            // retomber sur la route par défaut — c'est exactement ce que
            // l'utilisateur veut tester : "est-ce que CETTE interface a
            // accès à internet".
            let ((icmp_ok, icmp_ms), (http_ok, http_ms), (dns_ok, dns_ms)) =
                match resolve_src_from_iface(&iface) {
                    Ok(route) => {
                        let dev = route.iface.as_deref();
                        tokio::join!(
                            probe_icmp(route.src, dev),
                            probe_http(route.src, &route.dns),
                            probe_dns(&route.dns)
                        )
                    }
                    Err(_) => ((false, None), (false, None), (false, None)),
                };
            let state = derive_state(icmp_ok, http_ok);
            // L'uptime reflète la connectivité réelle : Online si HTTP OU ICMP
            // passe. Sur Linux sans socket RAW (ex. Ubuntu sans CAP_NET_RAW),
            // l'ICMP échoue systématiquement mais HTTP fonctionne → utiliser
            // icmp_ok seul ferait chuter l'uptime même quand on est Online.
            let (uptime_pct, samples) = history.push(icmp_ok || http_ok);
            let tick = InternetTick {
                state,
                icmp_ok, icmp_ms,
                http_ok, http_ms,
                dns_ok, dns_ms,
                dns_target: DNS_TARGET,
                icmp_target: ICMP_TARGET,
                http_target: HTTP_TARGET,
                timestamp,
                uptime_pct,
                samples,
            };
            history.set_last(tick.clone());
            emit(&tick);
            sleep(Duration::from_secs(TICK_SECS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_when_both_ok() {
        assert_eq!(derive_state(true, true), InternetState::Online);
    }

    #[test]
    fn online_when_http_only() {
        // Linux desktop sans CAP_NET_RAW : ICMP échoue mais HTTP passe.
        assert_eq!(derive_state(false, true), InternetState::Online);
    }

    #[test]
    fn limited_when_icmp_only() {
        // Portail captif : ICMP passe, HTTP bloqué.
        assert_eq!(derive_state(true, false), InternetState::Limited);
    }

    #[test]
    fn offline_when_both_down() {
        assert_eq!(derive_state(false, false), InternetState::Offline);
    }

    #[test]
    fn sans_interface_choisie_ping_ne_lie_rien() {
        assert_eq!(
            ping_args("1.1.1.1", None, None),
            vec!["-c", "1", "-W", "2", "-q", "1.1.1.1"]
        );
    }

    /// Sous Linux (weak host model), un paquet portant l'IP de l'interface A
    /// peut sortir par B si la table de routage le décide. `ping -I <iface>`
    /// lie la socket au périphérique (SO_BINDTODEVICE), pas seulement à
    /// l'adresse.
    #[cfg(target_os = "linux")]
    #[test]
    fn sous_linux_ping_est_lie_au_peripherique() {
        assert_eq!(
            ping_args("1.1.1.1", Some(Ipv4Addr::new(10, 0, 30, 12)), Some("eth0")),
            vec!["-c", "1", "-W", "2", "-q", "-I", "eth0", "1.1.1.1"]
        );
    }

    #[test]
    fn sans_nom_d_interface_ping_retombe_sur_l_ip_source() {
        assert_eq!(
            ping_args("1.1.1.1", Some(Ipv4Addr::new(10, 0, 30, 12)), None),
            vec!["-c", "1", "-W", "2", "-q", "-I", "10.0.30.12", "1.1.1.1"]
        );
    }

    #[tokio::test]
    async fn le_http_ne_resout_pas_par_le_resolveur_du_technicien() {
        // Interface choisie mais sans serveur DNS exploitable : la cible HTTP
        // ne peut pas être résolue honnêtement. La résoudre quand même par le
        // système afficherait « Online » sur le réseau du technicien.
        let (ok, ms) = probe_http(Some(Ipv4Addr::new(127, 0, 0, 1)), &DnsPlan::Impossible).await;
        assert!(!ok);
        assert_eq!(ms, None);
    }

    #[tokio::test]
    async fn une_interface_sans_resolveur_ne_verdit_pas_la_pastille_dns() {
        // Interface choisie mais aucun serveur DNS à interroger : la mesure
        // échoue. Se rabattre sur le résolveur système afficherait vert alors
        // que le réseau audité, lui, ne résout rien.
        assert_eq!(probe_dns(&DnsPlan::Impossible).await, (false, None));
    }

    #[test]
    fn history_uptime_empty() {
        let h = InternetHistory::default();
        let (pct, n) = h.push(true);
        assert_eq!(pct, 100.0);
        assert_eq!(n, 1);
    }

    #[test]
    fn history_uptime_mixed() {
        let h = InternetHistory::default();
        h.push(true);
        h.push(false);
        let (pct, n) = h.push(true);
        assert!((pct - 66.666).abs() < 0.1);
        assert_eq!(n, 3);
    }
}
