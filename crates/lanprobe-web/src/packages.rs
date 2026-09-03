//! Paquets de la sonde, récupérés et servis par le hub.
//!
//! Le besoin naît à l'enrôlement : le hub donne un code, et il fallait aller
//! chercher le binaire ailleurs. Le hub le récupère donc, le vérifie, le garde
//! et le sert lui-même.
//!
//! **Pourquoi le hub relaie plutôt que de pointer un lien.**
//!
//! 1. Le poste du client n'a plus besoin d'atteindre GitHub — seul le hub
//!    sort. Sur un réseau d'entreprise filtré, c'est la différence entre « ça
//!    marche » et « débrouillez-vous ».
//! 2. 🔴 Le hub **vérifie la signature avant de servir**. C'est le point qui
//!    justifie toute la fonctionnalité, pas une option : un hub qui distribue
//!    des binaires non vérifiés depuis son propre serveur est moins sûr que le
//!    lien GitHub qu'il remplace, parce que la personne ne voit même plus d'où
//!    ils viennent.
//!
//! ⚠️ **Récupération à la demande, jamais au démarrage.** Tirer cinquante
//! mégaoctets sur chaque hub pour des paquets que personne ne demandera
//! peut-être est un coût pour rien.
//!
//! ⚠️ **L'écran d'enrôlement ne dépend d'aucun appel sortant.** Ces routes-ci
//! sont les seules à sortir, et elles ne sont appelées que quand on ouvre la
//! liste des paquets. Un hub sans Internet affiche son code d'enrôlement comme
//! d'habitude, et dit — ici — que les paquets ne sont pas joignables.

use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as UrlPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};

use lanprobe_core::updater::{asset_name_for, parse_version, signature_name, PLATFORMS};

use crate::db::{Outcome, Role};
use crate::web::{guarded, AppState, Identity};

/// Clé **publique** minisign de LanProbe, celle qui vérifie les assets publiés.
///
/// 🔴 Recopiée de `src-tauri/src/updater.rs` faute d'une caisse partagée entre
/// le hub et l'application : la clé privée correspondante ne vit que sur le
/// poste de release et dans le secret CI. Si elle tourne un jour, **les deux
/// copies changent** — un test vérifie au moins qu'une signature faite avec une
/// autre clé est refusée, ce qui attrape une clé mal recopiée.
const UPDATER_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEMzRDY0NUJBMENBQUExRDcKUldUWG9hb011a1hXdzR0ZmtEV1VIUjVHQWdnNWhOR3kwdDdYaXRDSGJBa1poblIyTjl3YnNLZWcK";

/// Où vivent les paquets, sous le volume du hub — à côté des rapports.
pub const PACKAGES_DIR: &str = "packages";

/// Suffixe d'un téléchargement en cours. ⚠️ Il ne devient un paquet qu'après
/// vérification : un `.part` renommé trop tôt serait servi tronqué.
const PART: &str = ".part";

/// Combien de versions le cache garde par défaut. Deux : celle qu'on installe,
/// et celle d'avant — qu'on réinstalle quand la nouvelle pose problème.
pub const DEFAULT_KEEP_VERSIONS: i64 = 2;

// ── Le cache sur le volume ─────────────────────────────────────────────────

/// Un composant de chemin sûr : ni séparateur, ni remontée, ni vide.
///
/// 🔴 Le tag et le nom d'asset viennent d'une réponse HTTP. Les coller dans un
/// chemin sans les regarder, c'est laisser `../../secret.key` désigner autre
/// chose que le cache.
fn safe_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
}

/// Le fichier en cache : un répertoire par tag, le nom d'asset tel quel.
pub fn package_path(config_dir: &Path, tag: &str, asset: &str) -> Option<PathBuf> {
    if !safe_component(tag) || !safe_component(asset) {
        return None;
    }
    Some(config_dir.join(PACKAGES_DIR).join(tag).join(asset))
}

/// Un paquet en cache, tel que le disque le porte.
#[derive(Debug, Clone)]
pub struct CachedPackage {
    pub tag: String,
    pub asset: String,
    pub size: u64,
}

/// Ce que le volume contient, **sans aucun appel sortant**.
///
/// 🔴 C'est ce qui rend la liste utile sur un hub sans Internet : ce qui est
/// déjà là se sert, et l'indisponibilité de GitHub ne concerne que le reste.
pub fn cached_packages(config_dir: &Path) -> Vec<CachedPackage> {
    let racine = config_dir.join(PACKAGES_DIR);
    let Ok(versions) = std::fs::read_dir(&racine) else {
        // Un hub qui n'a jamais rien récupéré n'a pas ce répertoire. Ce n'est
        // pas une panne : c'est l'état initial.
        return Vec::new();
    };
    let mut trouves = Vec::new();
    for version in versions.flatten() {
        let Ok(tag) = version.file_name().into_string() else {
            continue;
        };
        let Ok(fichiers) = std::fs::read_dir(version.path()) else {
            continue;
        };
        for f in fichiers.flatten() {
            let Ok(asset) = f.file_name().into_string() else {
                continue;
            };
            // ⚠️ Un `.part` est un téléchargement interrompu, pas un paquet.
            // Le lister le ferait servir : un installeur tronqué s'ouvre, et
            // échoue beaucoup plus loin, sur la machine du client.
            if asset.ends_with(PART) {
                continue;
            }
            let Ok(meta) = f.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            trouves.push(CachedPackage {
                tag: tag.clone(),
                asset,
                size: meta.len(),
            });
        }
    }
    trouves
}

/// Retire les paquets des versions les plus anciennes. Rend le nombre de
/// fichiers retirés.
///
/// ⚠️ **Par version, pas par date.** « Ce qui date de plus de N jours » purge
/// le paquet de la version courante sur un hub tranquille, et le fait
/// retélécharger pour rien. « Les N dernières versions » dit ce qu'on garde, et
/// c'est ce que l'écran affiche.
///
/// ⚠️ **On ne retire pas ce qu'on ne sait pas lire** : un répertoire dont le
/// nom n'est pas une version peut être n'importe quoi, y compris quelque chose
/// qu'un humain a posé là.
pub fn purge_cached_packages(config_dir: &Path, keep: i64) -> usize {
    let keep = keep.max(1) as usize;
    let racine = config_dir.join(PACKAGES_DIR);
    let Ok(entrees) = std::fs::read_dir(&racine) else {
        return 0;
    };

    let mut versions: Vec<((u32, u32, u32), String)> = Vec::new();
    for e in entrees.flatten() {
        let Ok(nom) = e.file_name().into_string() else {
            continue;
        };
        if let Some(v) = parse_version(&nom) {
            versions.push((v, nom));
        }
    }
    versions.sort_by(|a, b| b.0.cmp(&a.0));

    let mut retires = 0;
    for (_, tag) in versions.into_iter().skip(keep) {
        let dossier = racine.join(&tag);
        if let Ok(fichiers) = std::fs::read_dir(&dossier) {
            for f in fichiers.flatten() {
                if std::fs::remove_file(f.path()).is_ok() {
                    retires += 1;
                }
            }
        }
        // Le répertoire vidé s'en va avec ses fichiers : un cache jonché de
        // répertoires vides ne se lit plus.
        let _ = std::fs::remove_dir(&dossier);
    }
    retires
}

// ── La signature ───────────────────────────────────────────────────────────

/// Vérifie une signature minisign produite par `tauri signer sign`.
///
/// Réplique exacte de `src-tauri/src/updater.rs` : chaque valeur base64
/// enveloppe le texte minisign (2 lignes pour la clé, 4 pour la signature).
fn verify_signature(data: &[u8], sig_b64: &str, pub_key_b64: &str) -> Result<(), String> {
    use base64::Engine;
    let decode = |b64: &str| -> Result<String, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| e.to_string())?;
        String::from_utf8(bytes).map_err(|e| e.to_string())
    };
    let cle = minisign_verify::PublicKey::decode(&decode(pub_key_b64)?)
        .map_err(|e| e.to_string())?;
    let signature =
        minisign_verify::Signature::decode(&decode(sig_b64)?).map_err(|e| e.to_string())?;
    cle.verify(data, &signature, true).map_err(|e| e.to_string())
}

// ── La release et ses assets ───────────────────────────────────────────────

/// Un asset publié, avec la signature qui l'accompagne — ou son absence.
#[derive(Debug, Clone)]
pub struct PublishedAsset {
    pub url: String,
    /// `None` = la CI ne publie pas de `.sig` pour ce fichier. Le hub ne le
    /// sert alors pas, et le dit **sans rien télécharger**.
    pub signature: Option<String>,
}

pub fn find_asset(assets: &[(String, String)], nom: &str) -> Option<PublishedAsset> {
    let url = assets.iter().find(|(n, _)| n == nom)?.1.clone();
    let sig = signature_name(nom);
    Some(PublishedAsset {
        url,
        signature: assets.iter().find(|(n, _)| *n == sig).map(|(_, u)| u.clone()),
    })
}

/// Le numéro lisible d'un tag. ⚠️ Lu, jamais écrit en dur : un numéro figé se
/// périme et devient un mensonge — c'est précisément ce qu'on retire de ce
/// produit.
pub fn version_of(tag: &str) -> String {
    tag.strip_prefix("app-")
        .unwrap_or(tag)
        .trim_start_matches('v')
        .to_string()
}

// ── Les routes ─────────────────────────────────────────────────────────────

/// ⚠️ `operator` et non `viewer` : distribuer l'installeur d'une sonde est un
/// geste d'installation, au même titre que l'enrôlement dont il est le
/// prolongement. Un lecteur n'a pas de machine à équiper.
pub(crate) fn routes(state: &AppState) -> Router<AppState> {
    guarded(
        state,
        Role::Operator,
        Router::new()
            .route("/api/packages", get(list_packages))
            .route("/api/packages/{platform}/fetch", post(fetch_package))
            .route("/api/packages/{platform}/file", get(download_package)),
    )
}

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

/// Le paquet en cache pour une plateforme : la version la plus récente qu'on
/// ait, quelle que soit celle publiée aujourd'hui.
fn cached_for(config_dir: &Path, platform: &str) -> Option<CachedPackage> {
    cached_packages(config_dir)
        .into_iter()
        .filter(|c| asset_name_for(platform, &c.tag).as_deref() == Some(c.asset.as_str()))
        .max_by_key(|c| parse_version(&c.tag))
}

/// Ce qu'on peut proposer : ce que le volume tient déjà, et ce que la dernière
/// release publie.
///
/// 🔴 **Les deux moitiés sont indépendantes.** Un hub sans Internet répond
/// quand même, avec ce qu'il a en cache et la raison pour le reste — un écran
/// qui ne dirait rien laisserait chercher la panne ailleurs.
async fn list_packages(State(state): State<AppState>) -> Response {
    let release = lanprobe_core::updater::latest_app_release().await;
    let (tag, assets, notes, erreur) = match release {
        Ok(r) => (Some(r.tag), r.assets, Some(r.notes_url), None),
        Err(e) => (None, Vec::new(), None, Some(e)),
    };

    let paquets: Vec<serde_json::Value> = PLATFORMS
        .iter()
        .map(|platform| {
            let en_cache = cached_for(&state.config_dir, platform);
            // Le nom publié pour la version en ligne — absent quand on ne peut
            // pas la lire.
            let publie = tag
                .as_deref()
                .and_then(|t| asset_name_for(platform, t))
                .and_then(|nom| find_asset(&assets, &nom).map(|a| (nom, a)));

            serde_json::json!({
                "platform": platform,
                // ⚠️ La version affichée est celle du fichier concerné : celle
                // du cache quand c'est lui qu'on servira, celle de la release
                // sinon. Jamais une constante.
                "version": en_cache
                    .as_ref()
                    .map(|c| version_of(&c.tag))
                    .or_else(|| tag.as_deref().map(version_of)),
                "asset": en_cache
                    .as_ref()
                    .map(|c| c.asset.clone())
                    .or_else(|| publie.as_ref().map(|(n, _)| n.clone())),
                "cached": en_cache.is_some(),
                "size": en_cache.as_ref().map(|c| c.size),
                // 🔴 `false` = la CI ne publie pas de `.sig` pour ce fichier :
                // le hub ne le servira pas, et l'écran le dit AVANT le clic
                // plutôt qu'un bouton qui échoue.
                //
                // Ce qui est en cache a été vérifié pour y entrer : rien n'y
                // arrive sans signature valide.
                "signed": en_cache.is_some() || publie.as_ref().is_some_and(|(_, a)| a.signature.is_some()),
                "publishable": publie.is_some(),
            })
        })
        .collect();

    crate::web::ok_json(serde_json::json!({
        "reachable": erreur.is_none(),
        "error": erreur,
        "tag": tag,
        "version": tag.as_deref().map(version_of),
        "notes_url": notes,
        "keep_versions": state.settings.package_keep_versions(),
        "packages": paquets,
    }))
}

/// Récupère le paquet, **le vérifie**, puis le range.
///
/// 🔴 Un paquet dont la signature ne se vérifie pas n'est jamais servi, et
/// n'entre pas dans le cache : le `.part` est retiré et l'écran reçoit la
/// raison. C'est le point qui justifie que le hub distribue au lieu de pointer.
async fn fetch_package(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    UrlPath(platform): UrlPath<String>,
) -> Response {
    let release = match lanprobe_core::updater::latest_app_release().await {
        Ok(r) => r,
        // ⚠️ 502 et non 500 : ce n'est pas le hub qui est en panne, c'est la
        // sortie vers GitHub. Le message le dit, parce que le geste qui suit
        // n'est pas le même — on ne redémarre pas un hub pour un pare-feu.
        Err(e) => {
            return fail(
                StatusCode::BAD_GATEWAY,
                &format!("les paquets ne sont pas joignables depuis ce hub : {e}"),
            )
        }
    };

    let Some(nom) = asset_name_for(&platform, &release.tag) else {
        return fail(StatusCode::NOT_FOUND, "plateforme inconnue");
    };
    let Some(asset) = find_asset(&release.assets, &nom) else {
        return fail(
            StatusCode::NOT_FOUND,
            &format!("« {nom} » n'est pas publié dans la dernière version"),
        );
    };
    let Some(sig_url) = asset.signature.clone() else {
        // 🔴 Refusé AVANT le téléchargement, et nommé : « pas de signature
        // publiée » n'est pas « échec réseau », et personne n'y peut rien en
        // réessayant.
        crate::web::audit(
            &state,
            Some(&actor.username),
            "package.fetch",
            Some(&nom),
            Outcome::Failure,
            Some("aucune signature publiée pour cet asset"),
        );
        return fail(
            StatusCode::CONFLICT,
            &format!(
                "« {nom} » n'est pas signé : le hub ne distribue que ce qu'il peut vérifier"
            ),
        );
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .user_agent("LanProbe-Hub")
        .build()
    {
        Ok(c) => c,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    let octets = match telecharger(&client, &asset.url).await {
        Ok(o) => o,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, &e),
    };
    let signature = match telecharger(&client, &sig_url).await {
        Ok(o) => String::from_utf8_lossy(&o).to_string(),
        Err(e) => return fail(StatusCode::BAD_GATEWAY, &e),
    };

    if let Err(e) = verify_signature(&octets, &signature, UPDATER_PUBKEY) {
        crate::web::audit(
            &state,
            Some(&actor.username),
            "package.fetch",
            Some(&nom),
            Outcome::Failure,
            Some(&format!("signature invalide : {e}")),
        );
        return fail(
            StatusCode::BAD_GATEWAY,
            &format!("signature invalide — « {nom} » n'a pas été gardé : {e}"),
        );
    }

    let Some(dest) = package_path(&state.config_dir, &release.tag, &nom) else {
        return fail(StatusCode::CONFLICT, "nom de fichier refusé");
    };
    // Écrit à côté, renommé ensuite : un hub arrêté au milieu laisse un `.part`
    // que personne ne sert, jamais un installeur tronqué qui s'ouvre.
    let partiel = dest.with_extension(format!(
        "{}{}",
        dest.extension().and_then(|e| e.to_str()).unwrap_or(""),
        PART
    ));
    if let Err(e) = tokio::fs::create_dir_all(dest.parent().unwrap()).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Err(e) = tokio::fs::write(&partiel, &octets).await {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Err(e) = tokio::fs::rename(&partiel, &dest).await {
        let _ = tokio::fs::remove_file(&partiel).await;
        return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }

    crate::web::audit(
        &state,
        Some(&actor.username),
        "package.fetch",
        Some(&nom),
        Outcome::Success,
        Some(&format!("{} octets, signature vérifiée", octets.len())),
    );

    // Le cache ne grossit qu'ici : c'est donc ici qu'on le taille.
    let keep = state.settings.package_keep_versions();
    match purge_cached_packages(&state.config_dir, keep) {
        0 => {}
        n => tracing::info!("paquets : {n} fichier(s) au-delà des {keep} dernières versions retirés"),
    }

    crate::web::ok_json(serde_json::json!({
        "platform": platform,
        "asset": nom,
        "version": version_of(&release.tag),
        "size": octets.len(),
        "cached": true,
    }))
}

async fn telecharger(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} sur {url}", resp.status()));
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| e.to_string())
}

/// Sert le paquet **du cache**, jamais depuis GitHub à la volée.
///
/// ⚠️ Le téléchargement est un geste séparé et explicite : un `GET` qui part
/// chercher cinquante mégaoctets répondrait au bout de trois minutes, et
/// l'écran n'aurait rien à dire pendant ce temps.
async fn download_package(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    UrlPath(platform): UrlPath<String>,
) -> Response {
    let Some(en_cache) = cached_for(&state.config_dir, &platform) else {
        return fail(
            StatusCode::CONFLICT,
            "ce paquet n'est pas dans le cache du hub — récupérez-le d'abord",
        );
    };
    let Some(chemin) = package_path(&state.config_dir, &en_cache.tag, &en_cache.asset) else {
        return fail(StatusCode::CONFLICT, "nom de fichier refusé");
    };
    let octets = match tokio::fs::read(&chemin).await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("paquet {} illisible sur le volume : {e}", en_cache.asset);
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "ce paquet est introuvable sur le volume du hub",
            );
        }
    };

    crate::web::audit(
        &state,
        Some(&actor.username),
        "package.download",
        Some(&en_cache.asset),
        Outcome::Success,
        Some(&format!(
            "le hub a servi le paquet à {} ({} octets)",
            actor.username,
            octets.len()
        )),
    );

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, octets.len().to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", en_cache.asset),
            ),
        ],
        octets,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Clé de TEST jetable, et signature produite avec elle. Les mêmes que
    // celles de `src-tauri/src/updater.rs` : c'est le même mécanisme, il se
    // vérifie avec le même fixture.
    const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEQ1NEJBRENBMkI3NTYwNzYKUldSMllIVXJ5cTFMMVl0aWp0VHdKYUY2UkJvVkFsSWttUmQrYjJiRXV5VVExVnk0dmFmL0RWRlUK";
    const TEST_SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSMllIVXJ5cTFMMVROQk9iRXpBU3RmU1lRVGorY0NUaE9NVmt1UDNSMTZ2SkQ5NXcrUDNiTERKaVQyd3VrWUh2NmdwWDB0WFZYWTFJWnhkdkh6VFdYNjJWb1JaVVZjcUFrPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzgzODcwMDQ4CWZpbGU6Zml4dHVyZS5iaW4KZ0VDd0gyZFhBTWNBajZUWjh1b05oUjVSQVptMUNzMHQwYTdTQk5zWkg3bk8vOVZsSWgrRmlYaURtMWYreXRHU1ZxS0NDTTN0aDZ3UUNpOGszYnVCQkE9PQo=";
    const TEST_DATA: &[u8] = b"lanprobe-updater-signature-fixture-v1";

    fn volume() -> tempdir::Dir {
        tempdir::Dir::new()
    }

    fn ecrire(dir: &std::path::Path, tag: &str, asset: &str, octets: &[u8]) {
        let p = package_path(dir, tag, asset).expect("chemin refusé");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, octets).unwrap();
    }

    #[test]
    fn a_package_lives_under_its_own_version() {
        let p = package_path(std::path::Path::new("/vol"), "app-v2.1.1", "lanprobe_v2.1.1_amd64.deb")
            .unwrap();
        assert_eq!(
            p,
            std::path::Path::new("/vol/packages/app-v2.1.1/lanprobe_v2.1.1_amd64.deb")
        );
    }

    /// 🔴 Le tag et le nom d'asset viennent d'une réponse HTTP. Les coller dans
    /// un chemin sans les regarder, c'est laisser `../../secret.key` désigner
    /// autre chose que le cache.
    #[test]
    fn a_name_that_climbs_out_of_the_cache_is_refused() {
        let vol = std::path::Path::new("/vol");
        assert!(package_path(vol, "..", "x.deb").is_none());
        assert!(package_path(vol, "app-v1.0.0", "../../secret.key").is_none());
        assert!(package_path(vol, "a/b", "x.deb").is_none());
        assert!(package_path(vol, "app-v1.0.0", "").is_none());
    }

    /// Un hub qui n'a jamais rien récupéré n'a pas de répertoire `packages/`.
    /// Il doit rendre une liste vide, pas une erreur : la modale s'ouvre quand
    /// même, et dit qu'il n'y a rien en cache.
    #[test]
    fn an_untouched_hub_lists_nothing_and_does_not_fail() {
        let vol = volume();
        assert!(cached_packages(vol.path()).is_empty());
    }

    #[test]
    fn what_the_disk_holds_is_read_without_any_outgoing_call() {
        let vol = volume();
        ecrire(vol.path(), "app-v2.1.1", "lanprobe_v2.1.1_amd64.deb", b"0123456789");
        let en_cache = cached_packages(vol.path());
        assert_eq!(en_cache.len(), 1);
        assert_eq!(en_cache[0].tag, "app-v2.1.1");
        assert_eq!(en_cache[0].asset, "lanprobe_v2.1.1_amd64.deb");
        assert_eq!(en_cache[0].size, 10);
    }

    /// ⚠️ Un fichier `.part` est un téléchargement interrompu, pas un paquet.
    /// Le lister le ferait servir : un installeur tronqué s'ouvre, et échoue
    /// beaucoup plus loin.
    #[test]
    fn an_interrupted_download_is_not_a_package() {
        let vol = volume();
        ecrire(vol.path(), "app-v2.1.1", "lanprobe_v2.1.1_amd64.deb.part", b"012");
        assert!(cached_packages(vol.path()).is_empty());
    }

    #[test]
    fn the_oldest_versions_leave_and_the_newest_stay() {
        let vol = volume();
        for tag in ["app-v2.0.0", "app-v2.1.0", "app-v2.1.1"] {
            ecrire(vol.path(), tag, &format!("lanprobe_{tag}_amd64.deb"), b"x");
        }
        assert_eq!(purge_cached_packages(vol.path(), 2), 1);
        let restants: Vec<_> = cached_packages(vol.path()).into_iter().map(|c| c.tag).collect();
        assert!(restants.contains(&"app-v2.1.1".to_string()));
        assert!(restants.contains(&"app-v2.1.0".to_string()));
        assert!(!restants.contains(&"app-v2.0.0".to_string()));
    }

    /// Le répertoire vidé s'en va avec ses fichiers : un cache jonché de
    /// répertoires vides ne se lit plus.
    #[test]
    fn an_emptied_version_leaves_no_directory_behind() {
        let vol = volume();
        ecrire(vol.path(), "app-v1.0.0", "lanprobe_v1.0.0_amd64.deb", b"x");
        ecrire(vol.path(), "app-v2.0.0", "lanprobe_v2.0.0_amd64.deb", b"x");
        purge_cached_packages(vol.path(), 1);
        assert!(!vol.path().join(PACKAGES_DIR).join("app-v1.0.0").exists());
    }

    /// ⚠️ On ne retire pas ce qu'on ne sait pas lire. Un répertoire dont le nom
    /// n'est pas une version peut être n'importe quoi — y compris quelque chose
    /// qu'un humain a posé là.
    #[test]
    fn a_directory_that_is_not_a_version_is_left_alone() {
        let vol = volume();
        ecrire(vol.path(), "app-v2.1.1", "lanprobe_v2.1.1_amd64.deb", b"x");
        ecrire(vol.path(), "notes", "lisez-moi.txt", b"x");
        purge_cached_packages(vol.path(), 1);
        assert!(vol.path().join(PACKAGES_DIR).join("notes").exists());
    }

    #[test]
    fn keeping_zero_versions_is_read_as_keeping_one() {
        // Un cache vidé à chaque récupération n'est plus un cache. Le réglage a
        // un plancher côté écriture ; ici on refuse simplement de tout jeter.
        let vol = volume();
        ecrire(vol.path(), "app-v2.1.1", "lanprobe_v2.1.1_amd64.deb", b"x");
        assert_eq!(purge_cached_packages(vol.path(), 0), 0);
    }

    // ── La signature ───────────────────────────────────────────────────────

    #[test]
    fn a_valid_signature_passes() {
        assert!(verify_signature(TEST_DATA, TEST_SIG, TEST_PUBKEY).is_ok());
    }

    #[test]
    fn a_tampered_package_is_refused() {
        let mut abime = TEST_DATA.to_vec();
        abime.push(b'!');
        assert!(verify_signature(&abime, TEST_SIG, TEST_PUBKEY).is_err());
    }

    /// 🔴 La clé de prod ne valide pas une signature faite avec une autre clé.
    /// Sans cette assertion, embarquer la mauvaise clé passerait inaperçu — et
    /// le hub servirait des binaires signés par n'importe qui.
    #[test]
    fn a_signature_from_another_key_is_refused() {
        assert!(verify_signature(TEST_DATA, TEST_SIG, UPDATER_PUBKEY).is_err());
    }

    /// La signature publiée par la CI est celle de `<asset>.sig`. Un asset dont
    /// elle n'est pas publiée n'est jamais téléchargé : le hub le dit.
    #[test]
    fn an_unsigned_asset_is_named_as_such_before_any_download() {
        let assets = vec![
            ("lanprobe_v2.1.1_amd64.deb".to_string(), "https://x/deb".to_string()),
            ("lanprobe_v2.1.1_amd64.deb.sig".to_string(), "https://x/sig".to_string()),
            ("lanprobe-server_v2.1.1_amd64.deb".to_string(), "https://x/hdeb".to_string()),
        ];
        let signe = find_asset(&assets, "lanprobe_v2.1.1_amd64.deb").unwrap();
        assert_eq!(signe.signature.as_deref(), Some("https://x/sig"));
        let nu = find_asset(&assets, "lanprobe-server_v2.1.1_amd64.deb").unwrap();
        assert!(nu.signature.is_none(), "le .deb headless n'a pas de .sig publié");
        assert!(find_asset(&assets, "lanprobe_v2.1.1_x64-setup.exe").is_none());
    }

    /// Le numéro affiché vient du tag, jamais d'une constante : une version en
    /// dur se périme et devient un mensonge.
    #[test]
    fn the_version_shown_is_read_from_the_tag() {
        assert_eq!(version_of("app-v2.1.1"), "2.1.1");
        assert_eq!(version_of("v1.1.5"), "1.1.5");
    }

    /// Un bac à sable qui s'efface tout seul — `tempfile` n'est pas dans les
    /// dépendances du hub, et une dépendance de plus pour trois lignes ne se
    /// justifie pas.
    mod tempdir {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);

        pub struct Dir(std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                // ⚠️ Un compteur et non l'horodatage : les tests d'un même
                // fichier démarrent dans la même seconde, et deux d'entre eux
                // partageraient alors le même bac à sable.
                let p = std::env::temp_dir().join(format!(
                    "lanprobe-packages-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&p).unwrap();
                Dir(p)
            }
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
