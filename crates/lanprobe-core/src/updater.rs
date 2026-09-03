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

/// ⚠️ Deux formes de tag coexistent, et il faut les accepter toutes les deux.
///
/// Le dépôt porte deux produits — l'application et le hub — qui ne sortent pas
/// ensemble. Les tags sont donc passés de `v2.1.0` à `app-v2.1.0`. Mais les
/// versions déjà installées embarquent cette fonction : si elle ne reconnaît
/// que l'ancienne forme, elles cessent de voir les mises à jour **sans rien
/// dire**. On accepte donc les deux, définitivement.
pub fn parse_version(tag: &str) -> Option<(u32, u32, u32)> {
    let s = tag
        .strip_prefix("app-v")
        .or_else(|| tag.strip_prefix('v'))?;
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
    // Les fichiers publiés gardent la forme `v2.1.0`, quel que soit le tag :
    // `lanprobe_app-v2.1.0_amd64.deb` ne servirait personne, et un client
    // ancien qui cherche `lanprobe_v2.1.0_amd64.deb` doit le trouver.
    let tag = &match tag.strip_prefix("app-") {
        Some(rest) => rest.to_string(),
        None => tag.to_string(),
    };
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

// ── Le catalogue vu par le hub ─────────────────────────────────────────────
//
// Le hub distribue des paquets pour des machines qui ne sont PAS la sienne :
// un hub Linux propose l'installeur Windows au technicien qui va l'installer
// ailleurs. `expected_asset_name` ne peut pas répondre à ça — elle est
// compilée pour l'hôte, et c'est très bien pour une sonde qui se met à jour
// elle-même. On dérive donc les mêmes noms, mais depuis une plateforme
// demandée.

pub const PLATFORM_MACOS: &str = "macos";
pub const PLATFORM_WINDOWS: &str = "windows";
pub const PLATFORM_DEBIAN: &str = "debian";
pub const PLATFORM_DEBIAN_HEADLESS: &str = "debian-headless";

/// Les plateformes distribuables, dans l'ordre où on les propose.
pub const PLATFORMS: &[&str] = &[
    PLATFORM_MACOS,
    PLATFORM_WINDOWS,
    PLATFORM_DEBIAN,
    PLATFORM_DEBIAN_HEADLESS,
];

/// Le fichier publié pour une plateforme donnée, ou `None` si on ne sait pas
/// le nommer.
///
/// ⚠️ **Pas de nom inventé.** Chercher un asset construit au jugé rendrait
/// « introuvable » là où la vraie réponse est « on ne sait pas construire ce
/// nom » — deux pannes différentes, deux gestes différents.
///
/// 🔴 Le `.dmg` n'est pas dans la liste, et ce n'est pas un oubli : la CI ne
/// le signe pas (elle ne publie qu'un `.sha256` à côté, sur la même release).
/// Le hub ne sert que ce qu'il peut vérifier — voir la note de `signature_name`.
pub fn asset_name_for(platform: &str, tag: &str) -> Option<String> {
    // Les fichiers publiés gardent la forme `v2.1.0`, quel que soit le préfixe
    // du tag : `lanprobe_app-v2.1.0_amd64.deb` ne servirait personne.
    let v = tag.strip_prefix("app-").unwrap_or(tag);
    Some(match platform {
        PLATFORM_MACOS => format!("lanprobe_{v}_universal.pkg"),
        PLATFORM_WINDOWS => format!("lanprobe_{v}_x64-setup.exe"),
        PLATFORM_DEBIAN => format!("lanprobe_{v}_amd64.deb"),
        PLATFORM_DEBIAN_HEADLESS => format!("lanprobe-server_{v}_amd64.deb"),
        _ => return None,
    })
}

/// Le nom de la signature minisign publiée à côté d'un asset.
///
/// 🔴 Sa **présence dans la liste des assets** est ce qui permet de dire
/// « non distribuable » sans rien télécharger. Un paquet dont la signature
/// n'est pas publiée n'est pas servi par le hub : servir un binaire non
/// vérifié depuis son propre serveur serait moins sûr que de renvoyer vers
/// GitHub, où la personne verrait au moins d'où il vient.
pub fn signature_name(asset: &str) -> String {
    format!("{asset}.sig")
}

/// La dernière release **de l'application**, avec ses assets.
///
/// ⚠️ `hub-x.y.z` n'en est pas une : [`parse_version`] l'écarte déjà, et c'est
/// ce qui évite de proposer le mauvais produit.
#[derive(Debug, Clone)]
pub struct LatestRelease {
    pub tag: String,
    /// `(nom, url de téléchargement)`, tels que GitHub les publie.
    pub assets: Vec<(String, String)>,
    pub notes_url: String,
}

pub async fn latest_app_release() -> Result<LatestRelease, String> {
    let releases = fetch_releases().await?;
    let (_, release) = releases
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| parse_version(&r.tag_name).map(|v| (v, r)))
        .max_by_key(|(v, _)| *v)
        .ok_or_else(|| "aucune release publiée".to_string())?;
    Ok(LatestRelease {
        notes_url: format!(
            "https://github.com/{}/releases/tag/{}",
            GITHUB_REPO, release.tag_name
        ),
        assets: release
            .assets
            .into_iter()
            .map(|a| (a.name, a.browser_download_url))
            .collect(),
        tag: release.tag_name,
    })
}

/// L'appel sortant, en un seul endroit : deux implémentations de la même
/// requête finiraient par ne plus lire la même liste.
async fn fetch_releases() -> Result<Vec<GithubRelease>, String> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("LanProbe-Updater")
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/releases?per_page=20", GITHUB_API);
    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API HTTP {}", resp.status()));
    }
    resp.json().await.map_err(|e| e.to_string())
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

    let releases = fetch_releases().await?;

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


#[cfg(test)]
mod hub_catalog_tests {
    use super::*;

    /// Le hub distribue des paquets pour des machines qui ne sont PAS la
    /// sienne : un hub Linux propose l'installeur Windows. `expected_asset_name`
    /// ne peut pas répondre — elle est compilée pour l'hôte.
    #[test]
    fn every_platform_is_named_without_asking_the_host() {
        assert_eq!(
            asset_name_for(PLATFORM_MACOS, "app-v2.1.1").as_deref(),
            Some("lanprobe_v2.1.1_universal.pkg")
        );
        assert_eq!(
            asset_name_for(PLATFORM_WINDOWS, "app-v2.1.1").as_deref(),
            Some("lanprobe_v2.1.1_x64-setup.exe")
        );
        assert_eq!(
            asset_name_for(PLATFORM_DEBIAN, "app-v2.1.1").as_deref(),
            Some("lanprobe_v2.1.1_amd64.deb")
        );
        assert_eq!(
            asset_name_for(PLATFORM_DEBIAN_HEADLESS, "app-v2.1.1").as_deref(),
            Some("lanprobe-server_v2.1.1_amd64.deb")
        );
    }

    #[test]
    fn an_unknown_platform_is_not_guessed() {
        // Mieux vaut ne rien proposer qu'un nom de fichier inventé : le hub
        // irait chercher un asset qui n'existe pas et rendrait « introuvable »
        // là où la vraie réponse est « on ne sait pas construire ce nom ».
        assert_eq!(asset_name_for("solaris", "v2.1.1"), None);
    }

    #[test]
    fn the_tag_prefix_never_reaches_the_file_name() {
        // Les fichiers publiés gardent la forme `v2.1.0`, quel que soit le tag.
        assert_eq!(
            asset_name_for(PLATFORM_DEBIAN, "app-v2.1.1"),
            asset_name_for(PLATFORM_DEBIAN, "v2.1.1")
        );
    }

    /// 🔴 Un seul jeu de règles de nommage. Deux tables qui divergent, c'est un
    /// hub qui cherche `lanprobe_v2.1.1_amd64.deb` pendant que la sonde
    /// installée cherche autre chose, et personne ne le voit avant la release.
    #[test]
    fn the_hub_names_the_same_file_as_the_probe_itself() {
        let tag = "app-v2.1.1";
        assert_eq!(
            asset_name_for(PLATFORM_DEBIAN_HEADLESS, tag),
            expected_asset_name(tag, true).or_else(|| asset_name_for(PLATFORM_DEBIAN_HEADLESS, tag)),
        );
    }

    #[test]
    fn the_signature_sits_next_to_its_package() {
        // La CI publie `<asset>.sig` à côté de l'asset. Le hub le cherche sous
        // ce nom exact — c'est ce qui lui permet de dire « non signé » sans
        // rien télécharger.
        assert_eq!(signature_name("lanprobe_v2.1.1_amd64.deb"), "lanprobe_v2.1.1_amd64.deb.sig");
    }
}

#[cfg(test)]
mod updater_tag_tests {
    use super::*;

    #[test]
    fn both_tag_forms_are_understood() {
        // ⚠️ Le jour où l'on est passé de `v2.1.0` à `app-v2.1.0`, les
        // installations existantes auraient cessé de voir les mises à jour
        // sans le moindre message. Les deux formes restent valides.
        assert_eq!(parse_version("v2.1.0"), Some((2, 1, 0)));
        assert_eq!(parse_version("app-v2.1.0"), Some((2, 1, 0)));
    }

    #[test]
    fn a_hub_tag_is_not_an_application_version() {
        // Le hub sort sous `hub-1.0.0`. Le prendre pour une version de
        // l'application proposerait une mise à jour vers le mauvais produit.
        assert_eq!(parse_version("hub-1.0.0"), None);
        assert_eq!(parse_version("hub-v1.0.0"), None);
    }

    #[test]
    fn a_prerelease_is_not_offered() {
        assert_eq!(parse_version("v2.1.0-rc1"), None);
        assert_eq!(parse_version("app-v2.1.0-rc1"), None);
    }

    #[test]
    fn published_files_keep_the_plain_v_form() {
        // Un client ancien cherche `lanprobe_v2.1.0_amd64.deb` : le nom des
        // fichiers ne doit pas suivre le préfixe du tag.
        let from_new = expected_asset_name("app-v2.1.0", true);
        let from_old = expected_asset_name("v2.1.0", true);
        assert_eq!(from_new, from_old);
        assert_eq!(from_new.as_deref(), Some("lanprobe-server_v2.1.0_amd64.deb"));
    }
}

#[cfg(test)]
mod real_release_list_tests {
    use super::*;

    /// Les tags réellement publiés au 30/08/2026, dans l'ordre rendu par
    /// l'API. Le tri doit trouver 2.1.1 et ignorer le hub.
    const PUBLISHED: &[&str] = &[
        "hub-1.0.0",
        "app-v2.1.1",
        "app-v2.1.0",
        "v2.0.0",
        "v1.1.5",
        "v1.1.4",
    ];

    #[test]
    fn a_2_1_0_install_finds_2_1_1_in_the_real_list() {
        let latest = PUBLISHED
            .iter()
            .filter_map(|t| parse_version(t).map(|v| (v, *t)))
            .max_by_key(|(v, _)| *v);
        assert_eq!(latest, Some(((2, 1, 1), "app-v2.1.1")));

        let current = parse_version("v2.1.0").unwrap();
        assert!(latest.unwrap().0 > current, "2.1.1 doit être vue comme plus récente");
    }

    #[test]
    fn the_hub_release_is_never_taken_for_an_application_version() {
        // `hub-1.0.0` est en tête de liste. Le prendre pour une version de
        // l'application proposerait une mise à jour vers le mauvais produit —
        // et, pire, ferait redescendre de 2.1.x à 1.0.0.
        assert_eq!(parse_version("hub-1.0.0"), None);
    }

    #[test]
    fn the_asset_looked_for_is_the_one_actually_published() {
        // Le fichier publié s'appelle `lanprobe-server_v2.1.1_amd64.deb`,
        // pas `lanprobe-server_app-v2.1.1_amd64.deb`.
        assert_eq!(
            expected_asset_name("app-v2.1.1", true).as_deref(),
            Some("lanprobe-server_v2.1.1_amd64.deb")
        );
    }
}
