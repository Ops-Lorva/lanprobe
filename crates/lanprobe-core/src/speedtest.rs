use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr};
use super::proc::async_cmd;

#[derive(Debug, Serialize, Clone, Default)]
pub struct SpeedResult {
    pub engine: String,
    pub download_mbps: f64,
    pub upload_mbps: f64,
    pub latency_ms: u64,
    pub jitter_ms: Option<f64>,
    pub server_name: String,
    pub result_url: Option<String>,
    pub timestamp: u64,
}

pub async fn run_speedtest(
    src: Option<Ipv4Addr>,
    iface_name: Option<String>,
) -> Result<SpeedResult, String> {
    if let Some(ref name) = iface_name {
        // Étape 1 — vérification de connectivité avant Ookla.
        // Le CLI Ookla traite `-I` comme un hint : si l'interface sélectionnée
        // n'a pas internet, il peut router silencieusement via la route par
        // défaut. On ouvre une connexion TCP sur 1.1.1.1:443 bindée sur l'IP
        // source — si ça échoue, l'interface est hors ligne → erreur immédiate.
        if let Some(ip) = src {
            check_interface_reachability(ip, name).await?;
        }

        // Étape 2 — lancement Ookla avec bind.
        // Sur Windows, `-I <friendly-name>` échoue avec "Failed binding local
        // connection end" même quand l'interface a internet (confirmé en prod).
        // La cause : Ookla CLI Windows ne résout pas correctement le nom en
        // index pour IP_UNICAST_IF. On utilise `-i <ip>` (minuscule) qui bind
        // par adresse source — équivalent fonctionnel, universellement supporté.
        // Sur macOS/Linux, `-I <name>` fonctionne (BSD name / ifname).
        #[cfg(target_os = "windows")]
        {
            if let Some(ip) = src {
                return run_ookla_cli(Some(&ip.to_string()), "-i", Some(name)).await;
            }
            return run_ookla_cli(Some(name), "-I", Some(name)).await;
        }
        #[cfg(not(target_os = "windows"))]
        return run_ookla_cli(Some(name), "-I", Some(name)).await;
    }
    // Pas d'interface sélectionnée → route par défaut.
    run_ookla_cli(None, "-I", None).await
}

/// Vérifie qu'on peut joindre internet via l'IP de l'interface sélectionnée.
/// Ouvre une connexion TCP sur 1.1.1.1:443 bindée sur `src` avec un timeout
/// de 4 secondes. Si le bind échoue → interface sans IP ou down.
/// Si le connect timeout → interface up mais pas de route internet.
async fn check_interface_reachability(src: Ipv4Addr, iface: &str) -> Result<(), String> {
    use std::net::SocketAddr;
    use std::time::Duration;

    let local: SocketAddr = SocketAddr::new(IpAddr::V4(src), 0);
    let remote: SocketAddr = "1.1.1.1:443".parse().unwrap();

    let sock = tokio::net::TcpSocket::new_v4()
        .map_err(|e| format!("speedtest.errors.interface_unavailable|iface={iface}|detail={e}"))?;

    sock.bind(local)
        .map_err(|_| format!("speedtest.errors.interface_unavailable|iface={iface}"))?;

    tokio::time::timeout(Duration::from_secs(4), sock.connect(remote))
        .await
        .map_err(|_| format!("speedtest.errors.network_unreachable|iface={iface}"))?
        .map_err(|_| format!("speedtest.errors.network_unreachable|iface={iface}"))?;

    Ok(())
}

// ── CLI Ookla speedtest ────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct OoklaResult {
    #[serde(default)]
    ping: OoklaPing,
    #[serde(default)]
    download: OoklaBandwidth,
    #[serde(default)]
    upload: OoklaBandwidth,
    #[serde(default)]
    server: OoklaServer,
    #[serde(default)]
    result: OoklaResultInfo,
}

#[derive(Deserialize, Default)]
struct OoklaPing {
    #[serde(default)]
    latency: f64,
    #[serde(default)]
    jitter: f64,
}

#[derive(Deserialize, Default)]
struct OoklaBandwidth {
    #[serde(default)]
    bandwidth: u64, // en octets/s
}

#[derive(Deserialize, Default)]
struct OoklaServer {
    #[serde(default)]
    name: String,
    #[serde(default)]
    location: String,
    #[serde(default)]
    country: String,
}

#[derive(Deserialize, Default)]
struct OoklaResultInfo {
    #[serde(default)]
    url: String,
}

fn ookla_binary_path() -> std::path::PathBuf {
    // Chemin dans le répertoire de données de l'app
    let data_dir = dirs_next::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    data_dir.join("lanprobe").join("speedtest")
}

async fn ensure_ookla_cli() -> Result<std::path::PathBuf, String> {
    // 1. Binaire embarqué dans l'installeur (Windows : speedtest.exe livré
    //    dans les ressources Tauri à côté de l'exécutable de l'app).
    if let Some(bundled) = bundled_speedtest_path() {
        if bundled.exists() {
            return Ok(bundled);
        }
    }

    // 2. Cherche dans les emplacements système courants (Linux/macOS via brew/apt)
    let system_candidates = [
        "/usr/local/bin/speedtest",
        "/opt/homebrew/bin/speedtest",
        "/usr/bin/speedtest",
    ];
    for p in &system_candidates {
        if std::path::Path::new(p).exists() {
            return Ok(std::path::PathBuf::from(p));
        }
    }

    // 3. Cherche dans le répertoire de données de l'app
    let local_bin = ookla_binary_path();
    if local_bin.exists() {
        return Ok(local_bin);
    }

    // 4. Télécharge automatiquement depuis Ookla (filet de sécurité si le
    //    bundle est absent — typiquement sur Linux/macOS hors brew).
    download_ookla_cli(&local_bin).await?;
    Ok(local_bin)
}

/// Retourne le chemin attendu du binaire `speedtest` embarqué par l'installeur.
/// Tauri pose les ressources à côté de l'exécutable selon la plateforme.
fn bundled_speedtest_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;

    #[cfg(target_os = "windows")]
    {
        // Tauri v2 NSIS peut placer les ressources à plusieurs endroits
        // selon la version — on teste tous les emplacements plausibles.
        let candidates = [
            exe_dir.join("resources").join("win").join("speedtest.exe"),
            exe_dir.join("resources").join("speedtest.exe"),
            exe_dir.join("speedtest.exe"),
            exe_dir.join("_up_").join("resources").join("win").join("speedtest.exe"),
        ];
        for p in &candidates {
            if p.exists() {
                eprintln!("[speedtest] bundled binary found at: {}", p.display());
                return Some(p.clone());
            }
        }
        eprintln!("[speedtest] bundled binary NOT found. Checked:");
        for p in &candidates {
            eprintln!("  - {}", p.display());
        }
    }

    #[cfg(target_os = "macos")]
    {
        // .app/Contents/MacOS/lanprobe → .app/Contents/Resources/
        if let Some(contents) = exe_dir.parent() {
            let p = contents.join("Resources").join("speedtest");
            if p.exists() { return Some(p); }
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::path::PathBuf;
        // Tauri copie les resources dans usr/lib/<productName>/resources/...
        // dans le bundle deb.
        let candidates = [
            exe_dir.join("resources").join("linux").join("speedtest"),
            exe_dir.join("speedtest"),
            exe_dir.join("../lib/LanProbe/resources/linux/speedtest"),
            exe_dir.join("../lib/lanprobe/resources/linux/speedtest"),
            PathBuf::from("/usr/lib/LanProbe/resources/linux/speedtest"),
            PathBuf::from("/usr/lib/lanprobe/resources/linux/speedtest"),
            // Chemin du deb headless (lanprobe-server)
            PathBuf::from("/usr/lib/lanprobe/speedtest"),
        ];
        for p in &candidates {
            if p.exists() { return Some(p.clone()); }
        }
    }

    None
}

async fn download_ookla_cli(dest: &std::path::Path) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    // Crée le répertoire parent si nécessaire
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "macos")]
    let url = "https://install.speedtest.net/app/cli/ookla-speedtest-1.2.0-macosx-universal.tgz";

    #[cfg(target_os = "linux")]
    let url = "https://install.speedtest.net/app/cli/ookla-speedtest-1.2.0-linux-x86_64.tgz";

    #[cfg(target_os = "windows")]
    let url = "https://install.speedtest.net/app/cli/ookla-speedtest-1.2.0-win64.zip";

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent("LanProbe-App")
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;

    #[cfg(not(target_os = "windows"))]
    {
        // Décompresse le .tgz et extrait le binaire `speedtest`
        let tmp_tgz = dest.with_extension("tgz");
        let mut f = tokio::fs::File::create(&tmp_tgz).await.map_err(|e| e.to_string())?;
        f.write_all(&bytes).await.map_err(|e| e.to_string())?;
        f.flush().await.map_err(|e| e.to_string())?;
        drop(f);

        // Extrait avec tar
        let out = async_cmd("tar")
            .args(["-xzf", &tmp_tgz.to_string_lossy(), "-C",
                &dest.parent().unwrap().to_string_lossy(), "--strip-components=1"])
            .output().await.map_err(|e| e.to_string())?;
        let _ = tokio::fs::remove_file(&tmp_tgz).await;

        if !out.status.success() {
            return Err(format!("Extraction échouée: {}", String::from_utf8_lossy(&out.stderr)));
        }

        // Cherche le binaire extrait (peut s'appeler speedtest ou ooklaserver)
        let parent = dest.parent().unwrap();
        for name in &["speedtest", "ooklaserver"] {
            let candidate = parent.join(name);
            if candidate.exists() {
                // Rend exécutable
                use std::os::unix::fs::PermissionsExt;
                tokio::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o755))
                    .await.map_err(|e| e.to_string())?;

                // macOS : supprime l'attribut de quarantaine ajouté par le système
                // quand un binaire est téléchargé programmatiquement. Sans ça,
                // Gatekeeper peut bloquer l'exécution avec des erreurs socket cryptiques.
                #[cfg(target_os = "macos")]
                let _ = async_cmd("xattr")
                    .args(["-d", "com.apple.quarantine", &candidate.to_string_lossy()])
                    .output().await;

                if candidate != dest {
                    tokio::fs::rename(&candidate, dest).await.map_err(|e| e.to_string())?;
                }
                return Ok(());
            }
        }
        return Err("Binaire speedtest introuvable dans l'archive".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let parent = dest.parent().ok_or("dest sans parent")?;
        let tmp_zip = parent.join("speedtest-cli.zip");
        let mut f = tokio::fs::File::create(&tmp_zip).await.map_err(|e| e.to_string())?;
        f.write_all(&bytes).await.map_err(|e| e.to_string())?;
        f.flush().await.map_err(|e| e.to_string())?;
        drop(f);

        // Extraction via tar.exe (livré avec Windows 10+).
        let out = async_cmd("tar")
            .args(["-xf", &tmp_zip.to_string_lossy(), "-C", &parent.to_string_lossy()])
            .output().await.map_err(|e| e.to_string())?;
        let _ = tokio::fs::remove_file(&tmp_zip).await;
        if !out.status.success() {
            return Err(format!("Extraction zip échouée: {}", String::from_utf8_lossy(&out.stderr)));
        }

        let extracted = parent.join("speedtest.exe");
        if !extracted.exists() {
            return Err("speedtest.exe introuvable après extraction".to_string());
        }
        if extracted != dest {
            tokio::fs::rename(&extracted, dest).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// Traduit une sortie d'erreur Ookla CLI en une clé i18n que le frontend
/// va traduire. Format : `speedtest.errors.<code>` ou
/// `speedtest.errors.<code>|iface=<name>` quand le nom d'interface est
/// utile pour localiser le message. Les patterns reconnus correspondent
/// aux cas qu'on voit vraiment en prod ; on tombe sur un code générique
/// avec le détail brut en dernier recours.
fn summarize_ookla_error(stderr: &str, stdout: &str, iface: Option<&str>) -> String {
    let blob = format!("{stderr}\n{stdout}");
    let lower = blob.to_lowercase();
    let code = if lower.contains("failed binding local connection end")
        || lower.contains("cannot assign requested address")
        || lower.contains("unavailable or has no routable")
        || lower.contains("no routable ip")
    {
        "interface_unavailable"
    } else if lower.contains("no such interface") || lower.contains("interface not found") {
        "interface_unknown"
    } else if lower.contains("cannot retrieve configuration")
        || lower.contains("could not retrieve or read configuration")
        || lower.contains("cannot reach")
        || lower.contains("unable to connect")
        || lower.contains("cannot open socket")
        || lower.contains("connection refused")
        || lower.contains("network is unreachable")
        || lower.contains("timed out")
    {
        "network_unreachable"
    } else if lower.contains("no servers") || lower.contains("no matching servers") {
        "no_servers"
    } else {
        // Extrait le premier message d'erreur lisible :
        // - Ligne JSON Ookla ({"type":"log","message":"..."}) → champ `message`
        // - Ligne texte classique → strip timestamps/prefixes
        let detail = blob.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .find_map(|line| {
                if line.starts_with('{') {
                    // Tente d'extraire le champ "message" du JSON Ookla
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                        if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
                            let msg = msg.trim();
                            if !msg.is_empty() { return Some(msg.to_string()); }
                        }
                    }
                    None
                } else {
                    let a = line.splitn(2, ']').nth(1).map(str::trim).unwrap_or(line);
                    let b = a.splitn(2, ']').nth(1).map(str::trim).unwrap_or(a);
                    let s = b.splitn(2, " - ").nth(1).unwrap_or(b).trim();
                    if s.is_empty() { None } else { Some(s.to_string()) }
                }
            })
            .unwrap_or_default();
        return format!("speedtest.errors.unknown|detail={}", detail);
    };
    match iface {
        Some(name) => format!("speedtest.errors.{code}|iface={name}"),
        None => format!("speedtest.errors.{code}"),
    }
}

async fn run_ookla_cli(bind_arg: Option<&str>, bind_flag: &str, iface_for_err: Option<&str>) -> Result<SpeedResult, String> {
    let bin = ensure_ookla_cli().await?;

    let mut cmd = async_cmd(&bin);
    // Permet l'annulation : si le future est abandonné (tokio::select! côté
    // serveur sur cmd_cancel_speedtest), le Child est droppé et le process tué.
    cmd.kill_on_drop(true);
    cmd.args(["--format=json", "--accept-license", "--accept-gdpr"]);
    if let Some(arg) = bind_arg {
        // Sur Windows : `-i <ip>` (IP source, bind par adresse).
        // Sur macOS/Linux : `-I <name>` (nom d'interface BSD/ifname).
        cmd.args([bind_flag, arg]);
    }
    let out = cmd.output().await.map_err(|e| e.to_string())?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(summarize_ookla_error(&stderr, &stdout, iface_for_err));
    }

    // Le CLI émet plusieurs lignes JSON (progress), on veut la dernière avec "type":"result"
    let stdout = String::from_utf8_lossy(&out.stdout);
    let result_line = stdout.lines()
        .filter(|l| l.contains("\"type\":\"result\""))
        .last()
        .ok_or("Pas de ligne résultat dans la sortie")?;

    let r: OoklaResult = serde_json::from_str(result_line)
        .map_err(|e| e.to_string())?;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let server_name = format!("{} — {}, {}", r.server.name, r.server.location, r.server.country);

    Ok(SpeedResult {
        engine: "ookla".to_string(),
        download_mbps: (r.download.bandwidth as f64 * 8.0) / 1_000_000.0,
        upload_mbps: (r.upload.bandwidth as f64 * 8.0) / 1_000_000.0,
        latency_ms: if r.ping.latency.is_finite() && r.ping.latency >= 0.0 { r.ping.latency.round() as u64 } else { 0 },
        jitter_ms: Some(r.ping.jitter),
        server_name,
        result_url: if r.result.url.is_empty() { None } else { Some(r.result.url) },
        timestamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_result_has_engine_field() {
        let r = SpeedResult {
            engine: "ookla".to_string(),
            download_mbps: 100.0,
            upload_mbps: 50.0,
            latency_ms: 10,
            jitter_ms: None,
            server_name: "test".to_string(),
            result_url: None,
            timestamp: 0,
        };
        assert_eq!(r.engine, "ookla");
    }
}

