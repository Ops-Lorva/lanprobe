use reqwest::Client;
use serde::{Deserialize, Serialize};

const GITHUB_REPO: &str = "Ops-Lorva/lanprobe";
const GITHUB_API: &str = "https://api.github.com/repos/Ops-Lorva/lanprobe";

#[derive(Debug, Serialize, Clone)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub has_update: bool,
    pub asset_url: Option<String>,
    pub asset_name: Option<String>,
    pub platform_supported: bool,
    pub release_notes_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

fn parse_version(tag: &str) -> Option<(u32, u32, u32)> {
    let s = tag.strip_prefix('v')?;
    if s.contains('-') { return None; }
    let mut parts = s.split('.');
    let a = parts.next()?.parse().ok()?;
    let b = parts.next()?.parse().ok()?;
    let c = parts.next()?.parse().ok()?;
    if parts.next().is_some() { return None; }
    Some((a, b, c))
}

/// Retourne le nom d'asset selon la cible de build.
/// `is_server` distingue le binaire headless (`lanprobe-server_vX.Y.Z_amd64.deb`)
/// du desktop (`lanprobe_vX.Y.Z_amd64.deb` / `.exe` / `.pkg`).
pub fn expected_asset_name(tag: &str, is_server: bool) -> Option<String> {
    if is_server {
        // Le headless tourne sur Linux (Debian/Ubuntu) — asset .deb uniquement.
        #[cfg(target_os = "linux")]
        return Some(format!("lanprobe-server_{}_amd64.deb", tag));
        #[allow(unreachable_code)]
        return None;
    }
    #[cfg(target_os = "windows")]
    { return Some(format!("lanprobe_{}_x64-setup.exe", tag)); }
    #[cfg(target_os = "linux")]
    {
        // AppImage si lancé via APPIMAGE ou chemin /.mount_, .deb si /usr/.
        let flavour = if std::env::var("APPIMAGE").is_ok() { Some("appimage") }
            else if let Ok(exe) = std::env::current_exe() {
                let s = exe.to_string_lossy().to_string();
                if s.contains("/.mount_") || s.to_lowercase().contains("appimage") { Some("appimage") }
                else if s.starts_with("/usr/") { Some("deb") }
                else { None }
            } else { None };
        return match flavour? {
            "appimage" => Some(format!("lanprobe_{}_amd64.AppImage", tag)),
            "deb"      => Some(format!("lanprobe_{}_amd64.deb", tag)),
            _ => None,
        };
    }
    #[cfg(target_os = "macos")]
    { Some(format!("lanprobe_{}_universal.pkg", tag)) }
}

fn pick_asset(assets: &[GithubAsset], tag: &str, is_server: bool) -> Option<(String, String)> {
    let expected = expected_asset_name(tag, is_server)?;
    assets.iter()
        .find(|a| a.name == expected)
        .map(|a| (a.browser_download_url.clone(), a.name.clone()))
}

pub async fn check_update(is_server: bool) -> Result<UpdateInfo, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let current_tuple = parse_version(&format!("v{}", current));

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("LanProbe-Updater")
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/releases?per_page=20", GITHUB_API);
    let resp = client.get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API HTTP {}", resp.status()));
    }
    let releases: Vec<GithubRelease> = resp.json().await.map_err(|e| e.to_string())?;

    let latest = releases.into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| parse_version(&r.tag_name).map(|v| (v, r)))
        .max_by_key(|(v, _)| *v);

    let (latest_tuple, latest_release) = match latest {
        Some((v, r)) => (Some(v), Some(r)),
        None => (None, None),
    };

    let has_update = match (current_tuple, latest_tuple) {
        (Some(cur), Some(lat)) => lat > cur,
        _ => false,
    };

    let (asset_url, asset_name) = if has_update {
        if let Some(ref r) = latest_release {
            pick_asset(&r.assets, &r.tag_name, is_server)
                .map(|(u, n)| (Some(u), Some(n)))
                .unwrap_or((None, None))
        } else { (None, None) }
    } else { (None, None) };

    let latest_tag = latest_release.as_ref().map(|r| r.tag_name.clone());
    let platform_supported = asset_url.is_some() || !has_update;

    let release_notes_url = latest_tag.as_ref().map(|t|
        format!("https://github.com/{}/releases/tag/{}", GITHUB_REPO, t)
    );

    Ok(UpdateInfo {
        current_version: current,
        latest_version: latest_tag.map(|t| t.trim_start_matches('v').to_string()),
        has_update: has_update && asset_url.is_some(),
        asset_url,
        asset_name,
        platform_supported,
        release_notes_url,
    })
}
