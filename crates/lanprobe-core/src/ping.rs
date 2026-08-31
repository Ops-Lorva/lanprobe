use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::net::TcpSocket;
use tokio::sync::mpsc;
use tokio::time::timeout;
use ping_async::{IcmpEchoRequestor, IcmpEchoStatus};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use super::proc::async_cmd;

#[derive(Debug, Serialize, Clone, Default)]
pub struct PingResult {
    pub ip: String,
    pub alive: bool,
    pub latency_ms: Option<u64>,
    pub timestamp: u64,
}

const PROBE_PORTS: &[u16] = &[80, 443, 22, 445, 8080, 21, 23, 8443, 139, 9100, 631, 3389];

/// Délai d'attente d'un ping, commun aux quatre chemins de mesure.
pub const PING_TIMEOUT_MS: u64 = 1000;

/// Transforme une latence mesurée en résultat, en écartant l'impossible.
///
/// ⚠️ **Une latence supérieure au délai d'attente ne peut pas venir du réseau** :
/// le ping aurait abandonné avant. Elle vient d'un processus suspendu — un
/// portable qui se met en veille reprend son chronomètre au réveil et compte
/// tout le sommeil comme temps de réponse.
///
/// Constaté en production : `max 1045504 ms`, soit **17 minutes** pour un ping.
/// Le point avait tout du vrai — bonne unité, bon horodatage, bonne cible — mais
/// aucun paquet n'avait mis 17 minutes. Il écrasait l'échelle du graphe et
/// rendait toutes les mesures honnêtes illisibles, sans que rien ne le signale.
///
/// Décision de Benjamin : **au-delà du délai, l'hôte est considéré mort.** C'est
/// d'ailleurs ce qu'il est du point de vue de la sonde, qui n'a rien reçu à
/// temps. Mieux vaut un trou franc qu'une valeur plausible et fausse.
fn measured(ip: &str, latency_ms: u64, timestamp: u64) -> PingResult {
    if latency_ms > PING_TIMEOUT_MS {
        return PingResult { ip: ip.to_string(), alive: false, latency_ms: None, timestamp };
    }
    PingResult { ip: ip.to_string(), alive: true, latency_ms: Some(latency_ms), timestamp }
}

pub async fn ping_once(ip: &str, src: Option<Ipv4Addr>) -> PingResult {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if let Some(lat) = icmp_ping(ip, src, PING_TIMEOUT_MS).await {
        return measured(ip, lat, timestamp);
    }

    #[cfg(target_os = "windows")]
    if let Some(lat) = windows_ping_exe(ip, src, PING_TIMEOUT_MS).await {
        return measured(ip, lat, timestamp);
    }

    #[cfg(target_os = "macos")]
    if let Some(lat) = macos_ping_exe(ip, src, PING_TIMEOUT_MS).await {
        return measured(ip, lat, timestamp);
    }

    let start = Instant::now();
    if tcp_probe(ip, src, PING_TIMEOUT_MS).await {
        measured(ip, start.elapsed().as_millis() as u64, timestamp)
    } else {
        PingResult { ip: ip.to_string(), alive: false, latency_ms: None, timestamp }
    }
}

/// Variante avec retries — utilisée pour le scan réseau où la perte
/// occasionnelle d'un paquet ICMP fait disparaître des hôtes pourtant joignables.
/// Seul le sondage ICMP est retenté (le moins cher) ; les fallbacks ne tournent
/// qu'une fois pour ne pas pénaliser les IPs réellement mortes.
pub async fn ping_once_fast_retry(ip: &str, src: Option<Ipv4Addr>, attempts: usize) -> Option<u64> {
    // Sur macOS, le binaire /sbin/ping (setuid root, raw socket) est beaucoup
    // plus fiable pour les sweeps que ping-async via SOCK_DGRAM. On le tente
    // en premier avant le crate, pour capturer un maximum d'hôtes.
    #[cfg(target_os = "macos")]
    if let Some(lat) = macos_ping_exe(ip, src, 600).await {
        return Some(lat);
    }

    for _ in 0..attempts.max(1) {
        if let Some(lat) = icmp_ping(ip, src, 600).await {
            return Some(lat);
        }
    }
    // Si l'utilisateur n'a PAS sélectionné d'interface on peut élargir la
    // recherche (ping.exe, tcp_probe, fallback no-src). Si au contraire il
    // en a choisi une, on reste strict : tout fallback sans src enverrait
    // le trafic par la route par défaut et ferait apparaître des hôtes qui
    // ne sont PAS joignables depuis l'interface choisie — ce qui induit
    // l'utilisateur en erreur (bug rapporté quand le Wi-Fi sélectionné est
    // désactivé mais la découverte continuait de trouver des hôtes via
    // l'Ethernet par défaut).
    if src.is_none() {
        #[cfg(target_os = "windows")]
        if let Some(lat) = windows_ping_exe(ip, None, 600).await {
            return Some(lat);
        }
        let start = Instant::now();
        if tcp_probe(ip, None, 600).await {
            return Some(start.elapsed().as_millis() as u64);
        }
        return None;
    }
    #[cfg(target_os = "windows")]
    if let Some(lat) = windows_ping_exe(ip, src, 600).await {
        return Some(lat);
    }
    let start = Instant::now();
    if tcp_probe(ip, src, 600).await {
        Some(start.elapsed().as_millis() as u64)
    } else {
        None
    }
}

/// Unprivileged ICMP ping via the `ping-async` crate.
/// `src` (Some) force l'adresse source — utile pour router le ping via une
/// interface spécifique au lieu de la route par défaut.
async fn icmp_ping(ip: &str, src: Option<Ipv4Addr>, timeout_ms: u64) -> Option<u64> {
    let addr: IpAddr = ip.parse().ok()?;
    let src_addr: Option<IpAddr> = src.map(IpAddr::V4);
    let requestor = IcmpEchoRequestor::new(
        addr,
        src_addr,
        None,
        Some(Duration::from_millis(timeout_ms)),
    ).ok()?;
    let reply = requestor.send().await.ok()?;
    match reply.status() {
        IcmpEchoStatus::Success => {
            let rtt = reply.round_trip_time();
            Some(rtt.as_millis() as u64)
        }
        _ => None,
    }
}

/// Parallel TCP probe on common ports — fallback when ICMP is blocked.
/// Si `src` est fourni, chaque socket est bind sur cette adresse avant le
/// connect → le paquet sort par l'interface qui possède cette IP.
async fn tcp_probe(ip: &str, src: Option<Ipv4Addr>, timeout_ms: u64) -> bool {
    // JoinSet pour pouvoir abort toutes les connexions TCP en cours dès
    // qu'on a une réponse (ou au timeout). Sans ça, sur un scan /24 avec
    // beaucoup d'IPs mortes, Windows accumule des milliers de SYN en vol
    // qui saturent la pile réseau et faisaient crasher l'app.
    let (tx, mut rx) = mpsc::channel::<()>(1);
    let mut set = tokio::task::JoinSet::new();
    for &port in PROBE_PORTS {
        let tx = tx.clone();
        let addr = format!("{}:{}", ip, port);
        let src = src;
        set.spawn(async move {
            if let Ok(target) = addr.parse::<SocketAddr>() {
                let connect = async {
                    let socket = TcpSocket::new_v4().ok()?;
                    if let Some(s) = src {
                        socket.bind(SocketAddr::new(IpAddr::V4(s), 0)).ok()?;
                    }
                    socket.connect(target).await.ok()
                };
                if timeout(Duration::from_millis(timeout_ms), connect)
                    .await
                    .ok()
                    .flatten()
                    .is_some()
                {
                    let _ = tx.send(()).await;
                }
            }
        });
    }
    drop(tx);
    let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms + 50));
    tokio::pin!(deadline);
    let alive = tokio::select! {
        Some(_) = rx.recv() => true,
        _ = &mut deadline => false,
    };
    set.abort_all();
    alive
}

/// Fallback Windows : ping.exe natif (toujours présent, pas d'admin requis).
/// `-S <src>` force l'adresse source pour router via l'interface choisie.
#[cfg(target_os = "windows")]
async fn windows_ping_exe(ip: &str, src: Option<Ipv4Addr>, timeout_ms: u64) -> Option<u64> {
    let timeout_str = timeout_ms.to_string();
    let mut cmd = async_cmd("ping");
    cmd.args(["-n", "1", "-w", &timeout_str]);
    let src_str;
    if let Some(s) = src {
        src_str = s.to_string();
        cmd.args(["-S", &src_str]);
    }
    cmd.arg(ip);
    let out = cmd.output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    if !text.to_lowercase().contains("ttl=") {
        return None;
    }
    let lower = text.to_lowercase();
    for marker in ["time=", "tiempo=", "tempo=", "temps=", "zeit="] {
        if let Some(pos) = lower.find(marker) {
            let tail = &text[pos + marker.len()..];
            let digits: String = tail.chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = digits.parse::<u64>() { return Some(n); }
        }
    }
    let bytes = lower.as_bytes();
    for i in 0..bytes.len().saturating_sub(2) {
        if &bytes[i..i + 2] == b"ms" {
            let mut j = i;
            while j > 0 && bytes[j - 1].is_ascii_digit() { j -= 1; }
            if j < i {
                if let Ok(n) = lower[j..i].parse::<u64>() { return Some(n); }
            }
        }
    }
    Some(0)
}

/// Fallback macOS : `/sbin/ping` (setuid root, raw socket, pas de password).
/// Beaucoup plus fiable que ping-async pour les sweeps réseau — le
/// `SOCK_DGRAM` ICMP de macOS limite le débit/fiabilité.
/// `-S <src>` force l'adresse source pour router via l'interface choisie.
#[cfg(target_os = "macos")]
async fn macos_ping_exe(ip: &str, src: Option<Ipv4Addr>, timeout_ms: u64) -> Option<u64> {
    // -c 1      : un seul echo request
    // -W <ms>   : timeout en millisecondes (macOS ping accepte des ms)
    // -n        : pas de résolution DNS
    // -q        : mode silencieux — on parse juste les stats de fin
    let timeout_str = timeout_ms.to_string();
    let mut cmd = async_cmd("/sbin/ping");
    cmd.args(["-c", "1", "-n", "-W", &timeout_str]);
    let src_str;
    if let Some(s) = src {
        src_str = s.to_string();
        cmd.args(["-S", &src_str]);
    }
    cmd.arg(ip);
    let out = cmd.output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Ligne type : "64 bytes from 192.168.1.1: icmp_seq=0 ttl=64 time=1.234 ms"
    for line in text.lines() {
        if let Some(pos) = line.find("time=") {
            let tail = &line[pos + 5..];
            let num: String = tail.chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(ms) = num.parse::<f64>() {
                return Some(ms.round() as u64);
            }
        }
    }
    None
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_latency_within_the_timeout_is_kept() {
        let r = measured("10.0.0.1", 42, 1000);
        assert!(r.alive);
        assert_eq!(r.latency_ms, Some(42));
    }

    #[test]
    fn a_latency_at_the_timeout_is_still_a_measurement() {
        // La borne est inclusive : une réponse arrivée pile à temps EST arrivée.
        let r = measured("10.0.0.1", PING_TIMEOUT_MS, 1000);
        assert!(r.alive);
        assert_eq!(r.latency_ms, Some(PING_TIMEOUT_MS));
    }

    #[test]
    fn a_latency_beyond_the_timeout_is_a_dead_host_not_a_slow_one() {
        // ⚠️ Le cas réel : 1 045 504 ms — 17 minutes — relevés sur un portable
        // qui s'était endormi pendant la mesure. Aucun paquet n'a mis 17
        // minutes ; le chronomètre a compté le sommeil. Un tel point écrasait
        // l'échelle du graphe et rendait toutes les vraies mesures illisibles.
        let r = measured("10.0.0.1", 1_045_504, 1000);
        assert!(!r.alive, "au-delà du délai, l'hôte est considéré mort");
        assert_eq!(r.latency_ms, None, "aucune latence ne doit être publiée");
    }

    #[test]
    fn just_past_the_timeout_is_already_dead() {
        let r = measured("10.0.0.1", PING_TIMEOUT_MS + 1, 1000);
        assert!(!r.alive);
        assert_eq!(r.latency_ms, None);
    }
}
