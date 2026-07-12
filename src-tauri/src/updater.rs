use base64::Engine;
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

// Sources de mises à jour : GitHub Releases est maintenant la référence
// officielle pour les utilisateurs finaux (repo public, pas de SSO/firewall
// interne, moins anxiogène qu'un instance GitLab self-hosted). Les builds
// GitLab continuent de pousser leurs assets sur cette release via un job CI.
const GITHUB_REPO: &str = "Ops-Lorva/lanprobe";
const GITHUB_API: &str = "https://api.github.com/repos/Ops-Lorva/lanprobe";

// Clé PUBLIQUE minisign de LanProbe (base64 du fichier .pub, 2 lignes).
// La clé PRIVÉE correspondante vit uniquement dans ~/.tauri/lanprobe.key
// (poste de release) et dans le secret CI TAURI_SIGNING_PRIVATE_KEY. Chaque
// asset publié est signé avec elle ; l'updater refuse d'exécuter un installeur
// dont la signature `.sig` ne correspond pas à cette clé. Paire DÉDIÉE à
// LanProbe (jamais partagée avec lorva-drive / taskori-sync).
const UPDATER_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEMzRDY0NUJBMENBQUExRDcKUldUWG9hb011a1hXdzR0ZmtEV1VIUjVHQWdnNWhOR3kwdDdYaXRDSGJBa1poblIyTjl3YnNLZWcK";

/// Vérifie une signature minisign produite par `tauri signer sign`.
///
/// - `data` : octets exacts de l'installeur téléchargé.
/// - `sig_b64` : contenu BRUT du fichier `.sig` (déjà en base64, tel quel).
/// - `pub_key_b64` : clé publique base64 (celle de tauri.conf.json / .pub).
///
/// Réplique la logique de `tauri-plugin-updater` : chaque valeur base64
/// enveloppe le texte minisign (2 lignes pour la clé, 4 pour la signature).
fn verify_signature(data: &[u8], sig_b64: &str, pub_key_b64: &str) -> Result<(), String> {
    let pub_key_decoded = base64_to_string(pub_key_b64)?;
    let public_key = PublicKey::decode(&pub_key_decoded).map_err(|e| e.to_string())?;
    let signature_decoded = base64_to_string(sig_b64)?;
    let signature = Signature::decode(&signature_decoded).map_err(|e| e.to_string())?;
    public_key
        .verify(data, &signature, true)
        .map_err(|e| e.to_string())
}

fn base64_to_string(b64: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

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

/// Détermine le flavour Linux courant : AppImage (env var APPIMAGE posée
/// par le runtime) ou .deb (binaire dans /usr). Retourne None sur une
/// install inconnue pour éviter de proposer une mise à jour cassée.
#[cfg(target_os = "linux")]
fn linux_flavour() -> Option<&'static str> {
    if std::env::var("APPIMAGE").is_ok() {
        return Some("appimage");
    }
    if let Ok(exe) = std::env::current_exe() {
        let s = exe.to_string_lossy();
        if s.contains("/.mount_") || s.to_lowercase().contains("appimage") {
            return Some("appimage");
        }
        if s.starts_with("/usr/") {
            return Some("deb");
        }
    }
    None
}

/// Parse un tag semver `vX.Y.Z` en tuple comparable. Ignore les suffixes
/// comme `-linux` / `-windows` / `latest` — on ne veut que les tags full.
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

/// Retourne le nom d'asset attendu pour la plateforme courante (sans URL
/// — l'URL réelle vient de la réponse GitHub qui connaît l'ID public).
fn expected_asset_name(tag: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    { return Some(format!("lanprobe_{}_x64-setup.exe", tag)); }
    #[cfg(target_os = "linux")]
    {
        match linux_flavour()? {
            "appimage" => Some(format!("lanprobe_{}_amd64.AppImage", tag)),
            "deb"      => Some(format!("lanprobe_{}_amd64.deb", tag)),
            _ => None,
        }
    }
    #[cfg(target_os = "macos")]
    { Some(format!("lanprobe_{}_universal.pkg", tag)) }
}

/// Cherche l'asset correspondant à la plateforme dans les assets GitHub.
fn pick_asset(assets: &[GithubAsset], tag: &str) -> Option<(String, String)> {
    let expected = expected_asset_name(tag)?;
    assets.iter()
        .find(|a| a.name == expected)
        .map(|a| (a.browser_download_url.clone(), a.name.clone()))
}

pub async fn check_update_impl() -> Result<UpdateInfo, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let current_tuple = parse_version(&format!("v{}", current));

    let url = format!("{}/releases?per_page=20", GITHUB_API);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("LanProbe-Updater")
        .build()
        .map_err(|e| e.to_string())?;
    // GitHub exige un User-Agent + version d'API explicite. Pas de token :
    // le repo est public, les releases et assets sont servis sans auth.
    let resp = client.get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send().await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API HTTP {}", resp.status()));
    }
    let releases: Vec<GithubRelease> = resp.json().await.map_err(|e| e.to_string())?;

    // On prend la plus grande release non-draft non-prerelease dont le tag
    // parse en semver strict (vX.Y.Z) — `latest`, hotfixes `-linux`, etc.
    // sont ignorés par parse_version.
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
            pick_asset(&r.assets, &r.tag_name)
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

pub async fn apply_update_impl(url: String, asset_name: String) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Download HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;

    // Vérification de signature AVANT toute exécution. La CI signe chaque asset
    // avec la clé privée LanProbe ; l'asset `.sig` est publié à côté de
    // l'installeur (même nom + `.sig`). Sans TLS-seul-vers-GitHub : un asset de
    // release compromis (token fuité, compte pris) ne peut plus déclencher de
    // RCE silencieuse, car la clé privée reste hors de GitHub. On échoue fermé :
    // pas de `.sig` valide → pas de lancement d'installeur.
    let sig_url = format!("{}.sig", url);
    let sig_resp = client.get(&sig_url).send().await.map_err(|e| e.to_string())?;
    if !sig_resp.status().is_success() {
        return Err(format!(
            "Signature introuvable ({}.sig) : HTTP {} — mise à jour refusée",
            asset_name,
            sig_resp.status()
        ));
    }
    let sig_b64 = sig_resp.text().await.map_err(|e| e.to_string())?;
    verify_signature(&bytes, &sig_b64, UPDATER_PUBKEY)
        .map_err(|e| format!("Signature invalide, mise à jour refusée : {}", e))?;

    let tmp_dir = std::env::temp_dir().join("lanprobe-update");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let dest: PathBuf = tmp_dir.join(&asset_name);
    std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if asset_name.ends_with(".AppImage") {
            let mut perms = std::fs::metadata(&dest).map_err(|e| e.to_string())?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&dest, perms).map_err(|e| e.to_string())?;
        }
    }

    launch_installer(&dest, &asset_name)?;
    Ok(dest.to_string_lossy().to_string())
}

#[cfg(target_os = "windows")]
fn launch_installer(path: &std::path::Path, _name: &str) -> Result<(), String> {
    // Spawn détaché pour que l'installeur NSIS survive à l'exit de lanprobe.exe.
    // `cmd /C start "" <path>` — le premier "" est le titre obligatoire de
    // `start` (sans ça le prochain argument entre guillemets serait avalé
    // comme titre et l'installeur ne démarrerait jamais). Il faut UN SEUL
    // "" : en mettre deux fait interpréter le second comme le fichier à
    // lancer, et le vrai chemin devient un argument → start ouvre le dossier
    // courant au lieu d'exécuter le .exe.
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const DETACHED_PROCESS: u32 = 0x00000008;
    Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(path)
        .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn launch_installer(path: &std::path::Path, name: &str) -> Result<(), String> {
    if name.ends_with(".AppImage") {
        // Si on tourne déjà en AppImage, on remplace le fichier d'origine
        // quand c'est possible puis on exec la nouvelle version. Sinon on
        // spawn simplement l'AppImage téléchargée en /tmp et on quitte.
        if let Ok(orig) = std::env::var("APPIMAGE") {
            let _ = std::fs::copy(path, &orig);
            let _ = Command::new("chmod").arg("+x").arg(&orig).status();
            Command::new(&orig).spawn().map_err(|e| e.to_string())?;
            return Ok(());
        }
        Command::new(path).spawn().map_err(|e| e.to_string())?;
        return Ok(());
    }
    if name.ends_with(".deb") {
        // xdg-open délègue au gestionnaire graphique (GNOME Software,
        // Discover, gdebi…) qui gère l'authentification et l'install.
        // Si aucun n'est dispo, on retombe sur pkexec apt.
        if Command::new("xdg-open").arg(path).spawn().is_ok() {
            return Ok(());
        }
        Command::new("pkexec")
            .arg("apt")
            .arg("install")
            .arg("-y")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    Err(format!("Unsupported Linux asset: {}", name))
}

#[cfg(target_os = "macos")]
fn launch_installer(path: &std::path::Path, name: &str) -> Result<(), String> {
    if name.ends_with(".pkg") {
        // Le PKG est signé + notarisé — `open` délègue à macOS Installer.app
        // qui gère l'authentification (mot de passe admin) et l'installation
        // dans /Applications sans qu'on ait besoin de sudo nous-mêmes.
        Command::new("open").arg(path).spawn().map_err(|e| e.to_string())?;
        return Ok(());
    }
    Err(format!("Unsupported macOS asset: {}", name))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Clé de TEST jetable (générée par `tauri signer generate`, sans mot de
    // passe) — n'a RIEN à voir avec la clé de prod embarquée dans UPDATER_PUBKEY.
    const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEQ1NEJBRENBMkI3NTYwNzYKUldSMllIVXJ5cTFMMVl0aWp0VHdKYUY2UkJvVkFsSWttUmQrYjJiRXV5VVExVnk0dmFmL0RWRlUK";
    // Contenu d'un `.sig` produit par `tauri signer sign` sur les octets ci-dessous.
    const TEST_SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSMllIVXJ5cTFMMVROQk9iRXpBU3RmU1lRVGorY0NUaE9NVmt1UDNSMTZ2SkQ5NXcrUDNiTERKaVQyd3VrWUh2NmdwWDB0WFZYWTFJWnhkdkh6VFdYNjJWb1JaVVZjcUFrPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzgzODcwMDQ4CWZpbGU6Zml4dHVyZS5iaW4KZ0VDd0gyZFhBTWNBajZUWjh1b05oUjVSQVptMUNzMHQwYTdTQk5zWkg3bk8vOVZsSWgrRmlYaURtMWYreXRHU1ZxS0NDTTN0aDZ3UUNpOGszYnVCQkE9PQo=";
    const TEST_DATA: &[u8] = b"lanprobe-updater-signature-fixture-v1";

    #[test]
    fn verifies_a_valid_signature() {
        assert!(verify_signature(TEST_DATA, TEST_SIG, TEST_PUBKEY).is_ok());
    }

    #[test]
    fn rejects_tampered_payload() {
        let mut tampered = TEST_DATA.to_vec();
        tampered.push(b'!');
        assert!(verify_signature(&tampered, TEST_SIG, TEST_PUBKEY).is_err());
    }

    #[test]
    fn rejects_signature_from_another_key() {
        // La clé de prod ne doit pas valider une signature faite avec la clé de test.
        assert!(verify_signature(TEST_DATA, TEST_SIG, UPDATER_PUBKEY).is_err());
    }
}
