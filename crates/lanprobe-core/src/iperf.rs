use serde::Deserialize;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use super::proc::async_cmd;
use super::speedtest::SpeedResult;

#[derive(Deserialize, Default)]
struct IperfJson {
    #[serde(default)]
    end: IperfEnd,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize, Default)]
struct IperfEnd {
    #[serde(default)]
    sum_received: IperfSum,
    #[serde(default)]
    streams: Vec<IperfStream>,
}

#[derive(Deserialize, Default)]
struct IperfSum {
    #[serde(default)]
    bits_per_second: f64,
}

#[derive(Deserialize, Default)]
struct IperfStream {
    #[serde(default)]
    udp: Option<IperfUdp>,
}

#[derive(Deserialize, Default)]
struct IperfUdp {
    #[serde(default)]
    jitter_ms: f64,
}

/// Cherche le binaire `iperf3` livré avec l'installeur, juste à côté
/// de l'exécutable de l'app. Mirror exact de `bundled_speedtest_path`.
fn bundled_iperf_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;

    #[cfg(target_os = "windows")]
    {
        let candidates = [
            exe_dir.join("resources").join("win").join("iperf3.exe"),
            exe_dir.join("resources").join("iperf3.exe"),
            exe_dir.join("iperf3.exe"),
            exe_dir.join("_up_").join("resources").join("win").join("iperf3.exe"),
        ];
        for p in &candidates {
            if p.exists() {
                return Some(p.clone());
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(contents) = exe_dir.parent() {
            let candidates = [
                contents.join("Resources").join("resources").join("macos").join("iperf3"),
                contents.join("Resources").join("iperf3"),
            ];
            for p in &candidates {
                if p.exists() { return Some(p.clone()); }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Tauri copie les resources dans `usr/lib/<productName>/resources/...`
        // dans le bundle. Pour .deb c'est `/usr/lib/LanProbe/...` en absolu ;
        // pour AppImage c'est `$MOUNT/usr/lib/LanProbe/...` et le binaire est
        // à `$MOUNT/usr/bin/lanprobe`, donc `exe_dir/../lib/LanProbe/...`.
        // Linux est case-sensitive : `LanProbe` (productName) ≠ `lanprobe`.
        let candidates = [
            exe_dir.join("resources").join("linux").join("iperf3"),
            exe_dir.join("iperf3"),
            exe_dir.join("../lib/LanProbe/resources/linux/iperf3"),
            exe_dir.join("../lib/lanprobe/resources/linux/iperf3"),
            PathBuf::from("/usr/lib/LanProbe/resources/linux/iperf3"),
            PathBuf::from("/usr/lib/lanprobe/resources/linux/iperf3"),
        ];
        for p in &candidates {
            if p.exists() { return Some(p.clone()); }
        }
    }

    None
}

/// Résout le binaire iperf3 à invoquer : bundled → système → PATH.
fn resolve_iperf_bin() -> PathBuf {
    if let Some(b) = bundled_iperf_path() {
        return b;
    }
    let system = [
        "/usr/local/bin/iperf3",
        "/opt/homebrew/bin/iperf3",
        "/usr/bin/iperf3",
    ];
    for p in &system {
        if std::path::Path::new(p).exists() {
            return PathBuf::from(p);
        }
    }
    #[cfg(target_os = "windows")]
    { PathBuf::from("iperf3.exe") }
    #[cfg(not(target_os = "windows"))]
    { PathBuf::from("iperf3") }
}

async fn run_one(server: &str, src: Option<Ipv4Addr>, reverse: bool) -> Result<IperfJson, String> {
    let bin = resolve_iperf_bin();
    let mut cmd = async_cmd(&bin);
    cmd.kill_on_drop(true); // annulation : drop du future → process iperf3 tué
    cmd.args(["-c", server, "-J", "-t", "10"]);
    if reverse { cmd.arg("-R"); }
    if let Some(s) = src {
        let src_str = s.to_string();
        cmd.args(["-B", &src_str]);
    }
    let out = cmd.output().await
        .map_err(|e| format!("iperf3 introuvable : {e}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        return Err(format!("iperf3 a échoué : {}", String::from_utf8_lossy(&out.stderr)));
    }
    let parsed: IperfJson = serde_json::from_str(&stdout)
        .map_err(|e| format!("JSON iperf3 invalide : {e}"))?;
    if let Some(err) = parsed.error.as_ref() {
        return Err(format!("iperf3 : {err}"));
    }
    Ok(parsed)
}

/// Valide que `s` ressemble à `host` ou `host:port` et ne contient pas de
/// flags CLI (ex: `"1.2.3.4 -p 9999"`). Protège contre l'injection de flags.
fn validate_server_addr(s: &str) -> Result<(), String> {
    let host = match s.rsplit_once(':') {
        Some((h, port_str)) => {
            port_str.parse::<u16>()
                .map_err(|_| format!("port iperf3 invalide : '{s}'"))?;
            h
        }
        None => s,
    };
    if host.is_empty() || host.contains(' ') || host.starts_with('-') {
        return Err(format!("adresse iperf3 invalide : '{s}'"));
    }
    Ok(())
}

pub async fn run_iperf3(server: &str, src: Option<Ipv4Addr>) -> Result<SpeedResult, String> {
    let server = server.trim();
    if server.is_empty() {
        return Err("Adresse du serveur iperf3 vide".into());
    }
    validate_server_addr(server)?;

    let upload = run_one(server, src, false).await?;
    let download = run_one(server, src, true).await?;

    let upload_mbps = upload.end.sum_received.bits_per_second / 1_000_000.0;
    let download_mbps = download.end.sum_received.bits_per_second / 1_000_000.0;

    let jitter = download.end.streams.iter()
        .filter_map(|s| s.udp.as_ref().map(|u| u.jitter_ms))
        .next();

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Ok(SpeedResult {
        engine: "iperf3".to_string(),
        download_mbps,
        upload_mbps,
        latency_ms: 0,
        jitter_ms: jitter,
        server_name: format!("iperf3 — {}", server),
        result_url: None,
        timestamp,
    })
}
