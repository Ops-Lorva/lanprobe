//! Sauvegarde et restauration du hub.
//!
//! ## Pourquoi ce module existe plutôt qu'une ligne de README
//!
//! Tout le hub vit dans deux volumes Docker, et la réponse naïve — « copiez
//! le volume » — est un mauvais conseil, pas une solution. **Copier un
//! fichier SQLite pendant qu'il est écrit produit une archive potentiellement
//! corrompue, qui a l'air parfaitement valide jusqu'au jour où on tente de la
//! restaurer.** C'est le pire mode de panne possible pour une sauvegarde.
//! Même chose pour le répertoire `engine` d'InfluxDB.
//!
//! Donc : `VACUUM INTO` pour SQLite, `influx backup` pour les séries, et rien
//! d'autre.
//!
//! ## Ce que contient une archive
//!
//! Une archive par sauvegarde, nommée comme chez Sonarr pour qu'on sache ce
//! qu'on tient sans l'ouvrir :
//!
//! ```text
//! /backup/lanprobe_backup_v1.0.0_2026.08.28_14.30.00.zip
//!     manifest.json     version du hub, version de schéma, date, tailles, sommes
//!     hub.sqlite        copie cohérente (VACUUM INTO)
//!     secrets/          secret.key, jeton opérateur, certificats
//!     influx/           sortie de `influx backup`
//! ```
//!
//! ⚠️ **L'archive contient `secret.key`.** Sans lui, une base restaurée rend
//! les canaux de notification illisibles — le hub sait déjà signaler cet état
//! (`unreadable: true`), et la sauvegarde doit éviter d'y conduire. La
//! conséquence directe est qu'une archive est un secret : qui la détient peut
//! ouvrir les identifiants SMTP et l'URL de webhook du hub.
//!
//! ## ZIP et pas tar.zst
//!
//! Le critère est l'utilisateur, pas le producteur : il doit pouvoir ouvrir
//! l'archive avec les outils de son système, sans rien installer.
//! L'explorateur de Windows et le Finder de macOS lisent le ZIP `deflate`
//! nativement ; ni l'un ni l'autre ne lit zstd. Le surcoût de taille est payé
//! une fois, l'installation d'un outil se paierait à chaque restauration —
//! c'est-à-dire le jour le plus mauvais.
//!
//! ## Le manifeste décide
//!
//! Rien n'est extrait qui ne soit **nommé dans le manifeste**, et chaque
//! entrée est vérifiée par sa somme SHA-256 avant d'être posée. Une archive
//! est un fichier de confiance : elle porte `secret.key` et écrase la base.
//! Un fichier qui n'est pas une archive LanProbe est refusé avant qu'on
//! déroule quoi que ce soit.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use crate::db::{Db, SCHEMA_VERSION};

/// Marqueur du manifeste. Sa présence est la première chose vérifiée : c'est
/// ce qui distingue une archive LanProbe d'un ZIP quelconque téléversé par
/// erreur (ou pas par erreur).
pub const MARKER: &str = "lanprobe-hub-backup";

/// Version du **format d'archive**, distincte de la version du hub et de
/// celle du schéma. Elle change si la disposition interne change ; elle n'a
/// pas bougé depuis la première livraison.
pub const FORMAT_VERSION: u32 = 1;

pub const MANIFEST_NAME: &str = "manifest.json";
pub const DB_ENTRY: &str = "hub.sqlite";
pub const SECRETS_PREFIX: &str = "secrets/";
pub const INFLUX_PREFIX: &str = "influx/";

const ARCHIVE_PREFIX: &str = "lanprobe_backup_v";
const ARCHIVE_EXT: &str = ".zip";

/// Répertoire d'archives par défaut, monté en volume dans le conteneur.
pub const DEFAULT_BACKUP_DIR: &str = "/backup";

/// Nombre d'archives conservées par défaut. Sept jours de sauvegardes
/// quotidiennes : de quoi revenir avant un week-end sans surveillance.
pub const DEFAULT_KEEP_LAST: i64 = 7;

/// Cadence par défaut. Quotidienne — c'est tout l'intérêt du modèle : une
/// sauvegarde qu'il faut déclencher est une sauvegarde qu'on oublie.
pub const DEFAULT_INTERVAL_HOURS: i64 = 24;

/// Fichiers du volume repris sous `secrets/`. Liste **explicite** : un
/// ramassage de tout le répertoire embarquerait la base elle-même, les
/// sauvegardes en cours d'écriture, et ce que l'utilisateur y aura déposé.
///
/// `users.json` n'est pas un fichier du hub — les comptes sont en base — mais
/// il est repris s'il traîne dans le volume : c'est celui du serveur headless,
/// et le perdre à la restauration serait une surprise désagréable.
const SECRET_FILES: &[&str] = &[
    "secret.key",
    "influx-operator-token",
    "hub-cert.pem",
    "hub-key.pem",
    "influx-cert.pem",
    "influx-key.pem",
    "setup-token",
    "users.json",
];

#[derive(Debug)]
pub enum BackupError {
    /// Le fichier n'est pas une archive LanProbe : ni ZIP lisible, ni
    /// manifeste, ni marqueur. On refuse avant de dérouler l'extraction.
    NotAnArchive(String),
    /// L'archive a été produite par un hub dont le schéma est **plus récent**
    /// que ce binaire. La restaurer donnerait une base que ce hub ne sait pas
    /// lire, et dont les migrations ne peuvent pas revenir en arrière.
    SchemaTooNew { archive: i64, binary: i64 },
    /// L'archive écraserait des données en place et personne ne l'a confirmé.
    ConfirmationRequired(String),
    /// Une entrée du manifeste ne correspond pas à ce que porte l'archive.
    Corrupt(String),
    Io(String),
    Internal(String),
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackupError::NotAnArchive(m) => {
                write!(f, "ce fichier n'est pas une archive de sauvegarde LanProbe : {m}")
            }
            BackupError::SchemaTooNew { archive, binary } => write!(
                f,
                "archive au schéma {archive}, ce hub n'en connaît que {binary} — \
                 elle vient d'une version plus récente de LanProbe. Mettez le hub à \
                 jour avant de restaurer : rien n'a été touché."
            ),
            BackupError::ConfirmationRequired(m) => write!(f, "{m}"),
            BackupError::Corrupt(m) => write!(f, "archive incohérente : {m}"),
            BackupError::Io(m) => write!(f, "{m}"),
            BackupError::Internal(m) => write!(f, "{m}"),
        }
    }
}

impl From<std::io::Error> for BackupError {
    fn from(e: std::io::Error) -> Self {
        BackupError::Io(e.to_string())
    }
}

impl From<crate::db::DbError> for BackupError {
    fn from(e: crate::db::DbError) -> Self {
        BackupError::Internal(e.to_string())
    }
}

pub type BackupResult<T> = Result<T, BackupError>;

/// Une entrée de l'archive, telle que le manifeste la décrit. La somme est ce
/// qui permet de refuser une archive tronquée **avant** d'écraser quoi que ce
/// soit.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ManifestEntry {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// Ce qu'il faut savoir d'une archive sans l'ouvrir en entier.
///
/// `schema_version` est la raison d'être du manifeste : sans elle on ne sait
/// pas si l'archive est restaurable par la version courante, et on l'apprend
/// au pire moment.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub marker: String,
    pub format: u32,
    pub hub_version: String,
    pub schema_version: i64,
    /// Date de la sauvegarde, en UTC, lisible sans outil.
    pub created_at: String,
    pub created_at_unix: i64,
    pub influx_included: bool,
    /// Pourquoi le volet Influx manque, quand il manque. Une archive sans
    /// séries qui ne dit pas pourquoi est un piège.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub influx_error: Option<String>,
    pub entries: Vec<ManifestEntry>,
}

impl Manifest {
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.bytes).sum()
    }
}

/// De quoi appeler la CLI `influx`. Absente en test : les tests n'ont pas
/// d'instance, et le volet Influx est optionnel par construction.
#[derive(Debug, Clone)]
pub struct InfluxTarget {
    /// Chemin de la CLI. `influx` dans l'image du hub.
    pub cli: PathBuf,
    pub host: String,
    pub org: String,
    pub token: String,
    /// Le certificat d'Influx est auto-signé dans le conteneur : sans ceci la
    /// CLI refuse sa propre instance.
    pub skip_verify: bool,
}

pub struct BackupRequest<'a> {
    pub db: &'a Db,
    pub config_dir: &'a Path,
    pub backup_dir: &'a Path,
    pub influx: Option<InfluxTarget>,
    /// Horodatage de la sauvegarde. Injecté pour que les tests soient
    /// reproductibles.
    pub now: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupReport {
    pub file: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub created_at: String,
    pub influx_included: bool,
    pub influx_error: Option<String>,
    pub schema_version: i64,
    pub hub_version: String,
}

/// Une archive présente dans le répertoire de sauvegarde, lue **d'après son
/// nom**. On n'ouvre pas les archives pour dresser la liste : une archive
/// abîmée doit apparaître dans la liste — c'est le moment où on veut la voir
/// — et non faire échouer la lecture du répertoire.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ArchiveInfo {
    pub file: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub hub_version: String,
    /// Instant déduit du nom, en secondes UTC.
    pub created_at_unix: i64,
    pub created_at: String,
}

pub struct RestoreRequest<'a> {
    pub archive: &'a Path,
    pub config_dir: &'a Path,
    /// Sans ceci, une restauration qui écraserait des données en place est
    /// refusée. Rien ne s'écrase par accident sur ce projet.
    pub confirm_overwrite: bool,
    pub influx: Option<InfluxTarget>,
    pub now: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RestoreReport {
    pub hub_version: String,
    pub schema_version: i64,
    pub created_at: String,
    pub restored: Vec<String>,
    /// Où sont partis les fichiers qui étaient en place. **Ils ne sont pas
    /// supprimés** : une restauration sur la mauvaise archive doit rester
    /// réparable.
    pub moved_aside: Option<PathBuf>,
    pub influx_restored: bool,
    pub influx_error: Option<String>,
    /// Toujours vrai quand la restauration passe par l'API : le processus en
    /// cours tient encore l'ancienne base ouverte, et continuera de servir
    /// ses données jusqu'au redémarrage.
    pub restart_required: bool,
}

// ── Sauvegarde ─────────────────────────────────────────────────────────────

/// Produit une archive cohérente dans `backup_dir`.
///
/// L'échec du volet Influx **ne fait pas échouer la sauvegarde** : une archive
/// sans séries reste ce qui porte les comptes, le parc et les secrets, et la
/// perdre parce qu'Influx ne répondait pas serait absurde. Mais elle le dit,
/// dans le manifeste et dans le rapport.
pub fn create(req: BackupRequest<'_>) -> BackupResult<BackupReport> {
    let hub_version = env!("CARGO_PKG_VERSION").to_string();
    let schema_version = req.db.user_version()?;

    std::fs::create_dir_all(req.backup_dir)?;
    let file = archive_name(&hub_version, req.now);
    let path = req.backup_dir.join(&file);
    if path.exists() {
        return Err(BackupError::Io(format!(
            "{} existe déjà — deux sauvegardes dans la même seconde",
            path.display()
        )));
    }

    // Zone de travail hors du répertoire d'archives : une extraction en cours
    // ne doit jamais ressembler à une archive prête.
    let staging = req.backup_dir.join(format!(".en-cours-{}", req.now));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;

    let result = build_archive(&req, &staging, &path, &hub_version, schema_version);
    let _ = std::fs::remove_dir_all(&staging);
    let (manifest, bytes) = match result {
        Ok(v) => v,
        Err(e) => {
            // Une archive à moitié écrite n'est pas une archive : on la
            // déplace hors du champ des restaurations plutôt que de la
            // laisser traîner sous un nom valide.
            if path.exists() {
                let _ = std::fs::rename(&path, path.with_extension("zip.incomplet"));
            }
            return Err(e);
        }
    };

    Ok(BackupReport {
        file,
        path,
        bytes,
        created_at: manifest.created_at.clone(),
        influx_included: manifest.influx_included,
        influx_error: manifest.influx_error.clone(),
        schema_version,
        hub_version,
    })
}

fn build_archive(
    req: &BackupRequest<'_>,
    staging: &Path,
    path: &Path,
    hub_version: &str,
    schema_version: i64,
) -> BackupResult<(Manifest, u64)> {
    // 1. SQLite, par VACUUM INTO. Jamais une copie de fichier.
    let db_copy = staging.join(DB_ENTRY);
    req.db.vacuum_into(&db_copy)?;

    // 2. InfluxDB, par sa commande dédiée. Jamais une copie du répertoire
    //    `engine`.
    let influx_dir = staging.join("influx");
    let influx_error = match &req.influx {
        None => Some("aucune instance InfluxDB configurée pour la sauvegarde".to_string()),
        Some(target) => match run_influx_backup(target, &influx_dir) {
            Ok(()) => None,
            Err(e) => {
                tracing::error!(
                    "volet InfluxDB de la sauvegarde manquant ({e}) — \
                     l'archive porte les comptes et le parc, pas les mesures"
                );
                let _ = std::fs::remove_dir_all(&influx_dir);
                Some(e.to_string())
            }
        },
    };
    let influx_included = influx_error.is_none();

    // 3. Les fichiers du volume qui ne sont pas en base.
    let mut planned: Vec<(String, PathBuf)> = vec![(DB_ENTRY.to_string(), db_copy)];
    for name in SECRET_FILES {
        let source = req.config_dir.join(name);
        if source.is_file() {
            planned.push((format!("{SECRETS_PREFIX}{name}"), source));
        }
    }
    if influx_included {
        let mut found = Vec::new();
        collect_files(&influx_dir, &influx_dir, &mut found)?;
        found.sort();
        for relative in found {
            planned.push((format!("{INFLUX_PREFIX}{relative}"), influx_dir.join(&relative)));
        }
    }

    let mut entries = Vec::with_capacity(planned.len());
    for (name, source) in &planned {
        let bytes = std::fs::metadata(source)?.len();
        entries.push(ManifestEntry {
            path: name.clone(),
            bytes,
            sha256: sha256_file(source)?,
        });
    }

    let manifest = Manifest {
        marker: MARKER.to_string(),
        format: FORMAT_VERSION,
        hub_version: hub_version.to_string(),
        schema_version,
        created_at: format_rfc3339(req.now),
        created_at_unix: req.now,
        influx_included,
        influx_error,
        entries,
    };

    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    // Le manifeste en premier : c'est la première chose que la restauration
    // lit, et un outil qui liste l'archive doit le voir en tête.
    let rendered = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| BackupError::Internal(e.to_string()))?;
    zip.start_file(MANIFEST_NAME, options)
        .map_err(|e| BackupError::Internal(e.to_string()))?;
    zip.write_all(&rendered)?;

    for (name, source) in &planned {
        zip.start_file(name.as_str(), options)
            .map_err(|e| BackupError::Internal(e.to_string()))?;
        let mut reader = std::fs::File::open(source)?;
        std::io::copy(&mut reader, &mut zip)?;
    }
    zip.finish().map_err(|e| BackupError::Internal(e.to_string()))?;

    let bytes = std::fs::metadata(path)?.len();
    Ok((manifest, bytes))
}

fn run_influx_backup(target: &InfluxTarget, dest: &Path) -> BackupResult<()> {
    std::fs::create_dir_all(dest)?;
    let mut cmd = std::process::Command::new(&target.cli);
    cmd.arg("backup")
        .arg("--host")
        .arg(&target.host)
        .arg("--org")
        .arg(&target.org)
        .arg("--token")
        .arg(&target.token);
    if target.skip_verify {
        cmd.arg("--skip-verify");
    }
    cmd.arg(dest);
    run_influx(cmd, "influx backup")
}

fn run_influx(mut cmd: std::process::Command, what: &str) -> BackupResult<()> {
    let output = cmd
        .output()
        .map_err(|e| BackupError::Io(format!("{what} n'a pas pu être lancée : {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    // Le jeton est passé en argument : il n'apparaît pas dans stderr, mais on
    // ne rend que stderr, jamais la ligne de commande.
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(BackupError::Io(format!(
        "{what} a échoué ({}) : {}",
        output.status,
        stderr.trim()
    )))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> BackupResult<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| BackupError::Internal(e.to_string()))?;
            let relative = relative
                .to_str()
                .ok_or_else(|| BackupError::Internal("nom de fichier non UTF-8".into()))?;
            out.push(relative.replace('\\', "/"));
        }
    }
    Ok(())
}

// ── Inventaire et rétention ────────────────────────────────────────────────

/// Les archives présentes, de la plus récente à la plus ancienne.
///
/// Le tri se fait sur l'instant lu dans le nom, **pas** sur le nom : la
/// version précède la date, donc un tri alphabétique mélangerait les
/// générations dès la première mise à jour du hub.
pub fn list(backup_dir: &Path) -> BackupResult<Vec<ArchiveInfo>> {
    let mut found = Vec::new();
    if !backup_dir.is_dir() {
        return Ok(found);
    }
    for entry in std::fs::read_dir(backup_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((hub_version, created_at_unix)) = parse_archive_name(file) else {
            continue;
        };
        found.push(ArchiveInfo {
            file: file.to_string(),
            bytes: entry.metadata().map(|m| m.len()).unwrap_or(0),
            path,
            hub_version,
            created_at_unix,
            created_at: format_rfc3339(created_at_unix),
        });
    }
    found.sort_by(|a, b| {
        b.created_at_unix
            .cmp(&a.created_at_unix)
            .then_with(|| b.file.cmp(&a.file))
    });
    Ok(found)
}

/// Ne garde que les `keep_last` archives les plus récentes.
///
/// ⚠️ **Cette fonction supprime des fichiers.** C'est la seule du hub à le
/// faire, et elle ne touche que ce qui porte exactement le nom d'une archive
/// LanProbe dans le répertoire de sauvegarde — jamais ce que l'utilisateur y
/// aura déposé. Le garde-fou est ailleurs : baisser `backup_keep_last` exige
/// une confirmation explicite côté serveur, comme la rétention Influx.
///
/// `keep_last <= 0` ne supprime rien : c'est « garder tout », pas « tout
/// jeter ». Confondre les deux ferait de la valeur la plus prudente la plus
/// destructrice.
pub fn prune(backup_dir: &Path, keep_last: i64) -> BackupResult<Vec<String>> {
    if keep_last <= 0 {
        return Ok(Vec::new());
    }
    let archives = list(backup_dir)?;
    let mut removed = Vec::new();
    for archive in archives.into_iter().skip(keep_last as usize) {
        match std::fs::remove_file(&archive.path) {
            Ok(()) => removed.push(archive.file),
            Err(e) => tracing::warn!("archive {} non retirée : {e}", archive.file),
        }
    }
    Ok(removed)
}

/// `lanprobe_backup_v1.0.0_2026.08.28_14.30.00.zip`
fn archive_name(hub_version: &str, now: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_utc(now);
    format!("{ARCHIVE_PREFIX}{hub_version}_{y:04}.{mo:02}.{d:02}_{h:02}.{mi:02}.{s:02}{ARCHIVE_EXT}")
}

fn parse_archive_name(file: &str) -> Option<(String, i64)> {
    let rest = file.strip_prefix(ARCHIVE_PREFIX)?.strip_suffix(ARCHIVE_EXT)?;
    // `1.0.0_2026.08.28_14.30.00` — la version ne contient pas de `_`.
    let (hub_version, stamp) = rest.split_once('_')?;
    let (date, time) = stamp.split_once('_')?;
    let date: Vec<&str> = date.split('.').collect();
    let time: Vec<&str> = time.split('.').collect();
    if date.len() != 3 || time.len() != 3 || hub_version.is_empty() {
        return None;
    }
    let n = |v: &str| v.parse::<i64>().ok();
    let at = unix_from_civil(
        n(date[0])?,
        n(date[1])?,
        n(date[2])?,
        n(time[0])?,
        n(time[1])?,
        n(time[2])?,
    );
    Some((hub_version.to_string(), at))
}

// ── Déclenchement automatique ──────────────────────────────────────────────

/// Instant de l'archive la plus récente, ou `None` s'il n'y en a aucune.
///
/// L'échéance se lit **dans les archives**, jamais depuis le démarrage du
/// processus : un hub redémarré plus souvent que sa cadence ne sauvegarderait
/// jamais, le compte à rebours repartant de zéro à chaque fois, et personne
/// ne s'en apercevrait avant d'en avoir besoin.
pub fn last_backup_at(backup_dir: &Path) -> Option<i64> {
    list(backup_dir).ok()?.first().map(|a| a.created_at_unix)
}

/// Vrai s'il est temps de sauvegarder. Sans archive du tout, c'est tout de
/// suite : attendre un jour entier laisserait la journée de configuration —
/// celle où l'on crée les comptes et enrôle le parc — sans filet.
pub fn is_due(last_backup: Option<i64>, now: i64, interval_hours: i64) -> bool {
    if interval_hours <= 0 {
        return false;
    }
    match last_backup {
        None => true,
        Some(last) => now - last >= interval_hours * 3600,
    }
}

/// Boucle de sauvegarde planifiée.
///
/// C'est le mode de fonctionnement normal : personne ne doit avoir à penser à
/// déclencher une sauvegarde. Le pas est court devant la cadence — c'est la
/// cadence qui décide, pas le rythme de la boucle — pour qu'un changement de
/// réglage prenne effet sans redémarrage.
pub async fn run(scheduler: Scheduler, step: std::time::Duration) {
    loop {
        tokio::time::sleep(step).await;
        scheduler.run_once().await;
    }
}

/// Ce qu'il faut pour sauvegarder sans requête HTTP.
#[derive(Clone)]
pub struct Scheduler {
    pub db: std::sync::Arc<Db>,
    pub settings: crate::settings::Settings,
    pub config_dir: PathBuf,
    pub backup_dir: PathBuf,
    pub influx: Option<InfluxTarget>,
}

impl Scheduler {
    /// Sauvegarde si l'échéance est passée, puis applique la rétention.
    /// Rend le nom de l'archive produite, le cas échéant.
    pub async fn run_once(&self) -> Option<String> {
        if !self.settings.backup_enabled() {
            return None;
        }
        let interval = self.settings.backup_interval_hours();
        let backup_dir = self.backup_dir.clone();
        if !is_due(last_backup_at(&backup_dir), now(), interval) {
            return None;
        }

        let (db, config_dir, influx, keep) = (
            self.db.clone(),
            self.config_dir.clone(),
            self.influx.clone(),
            self.settings.backup_keep_last(),
        );
        // La sauvegarde est du calcul synchrone qui dure : hors du fil
        // asynchrone, sinon elle gèle le hub le temps de son VACUUM.
        let outcome = tokio::task::spawn_blocking(move || {
            let report = create(BackupRequest {
                db: &db,
                config_dir: &config_dir,
                backup_dir: &backup_dir,
                influx,
                now: now(),
            })?;
            let removed = prune(&backup_dir, keep)?;
            Ok::<_, BackupError>((report, removed))
        })
        .await;

        match outcome {
            Ok(Ok((report, removed))) => {
                let removed = if removed.is_empty() {
                    String::new()
                } else {
                    format!(", {} archive(s) retirée(s)", removed.len())
                };
                tracing::info!(
                    "sauvegarde automatique : {} ({} octets){removed}",
                    report.file,
                    report.bytes
                );
                // Journalisée sans acteur : personne ne l'a demandée, c'est
                // le hub. Une ligne d'audit reste due — c'est ce qui permet
                // de constater après coup que les sauvegardes tournent.
                let _ = self.db.record_audit(
                    None,
                    "backup.create",
                    Some(&report.file),
                    crate::db::Outcome::Success,
                    Some("sauvegarde automatique"),
                );
                Some(report.file)
            }
            Ok(Err(e)) => {
                tracing::error!("sauvegarde automatique en échec : {e}");
                let _ = self.db.record_audit(
                    None,
                    "backup.create",
                    None,
                    crate::db::Outcome::Failure,
                    Some(&e.to_string()),
                );
                None
            }
            Err(e) => {
                tracing::error!("sauvegarde automatique interrompue : {e}");
                None
            }
        }
    }
}

// ── Lecture d'une archive ──────────────────────────────────────────────────

/// Lit le manifeste sans rien extraire. C'est ce qui permet de refuser
/// proprement un fichier quelconque au lieu de dérouler une extraction sur
/// des entrées arbitraires.
pub fn inspect(archive: &Path) -> BackupResult<Manifest> {
    let file = std::fs::File::open(archive)
        .map_err(|e| BackupError::NotAnArchive(format!("{} : {e}", archive.display())))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| BackupError::NotAnArchive(format!("ZIP illisible ({e})")))?;
    read_manifest(&mut zip)
}

fn read_manifest<R: Read + Seek>(zip: &mut zip::ZipArchive<R>) -> BackupResult<Manifest> {
    let mut entry = zip.by_name(MANIFEST_NAME).map_err(|_| {
        BackupError::NotAnArchive(format!("pas de {MANIFEST_NAME} à la racine de l'archive"))
    })?;
    let mut raw = String::new();
    entry
        .read_to_string(&mut raw)
        .map_err(|e| BackupError::NotAnArchive(format!("{MANIFEST_NAME} illisible ({e})")))?;
    drop(entry);

    let manifest: Manifest = serde_json::from_str(&raw)
        .map_err(|e| BackupError::NotAnArchive(format!("{MANIFEST_NAME} mal formé ({e})")))?;
    if manifest.marker != MARKER {
        return Err(BackupError::NotAnArchive(format!(
            "marqueur « {} » inattendu",
            manifest.marker
        )));
    }
    if manifest.format > FORMAT_VERSION {
        return Err(BackupError::NotAnArchive(format!(
            "format d'archive {} — ce hub ne lit que le format {FORMAT_VERSION}",
            manifest.format
        )));
    }
    Ok(manifest)
}

/// Chemin d'entrée acceptable. Tout le reste est refusé : c'est ce qui
/// empêche une archive forgée d'écrire ailleurs que dans le volume
/// (`../`, chemin absolu, lettre de lecteur Windows).
fn entry_is_safe(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return false;
    }
    if path.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return false;
    }
    path == DB_ENTRY || path.starts_with(SECRETS_PREFIX) || path.starts_with(INFLUX_PREFIX)
}

// ── Restauration ───────────────────────────────────────────────────────────

/// Restaure une archive dans `config_dir`.
///
/// Une sauvegarde jamais restaurée n'est pas une sauvegarde, c'est un fichier
/// qu'on espère. Les garde-fous, dans l'ordre où ils s'appliquent :
///
/// 1. le fichier est une archive LanProbe, marqueur et manifeste vérifiés ;
/// 2. son schéma n'est pas plus récent que ce binaire — sinon on refuse en le
///    disant, et rien n'est touché ;
/// 3. rien n'écrase des données en place sans `confirm_overwrite` ;
/// 4. tout est extrait et vérifié par sa somme **avant** que le premier
///    fichier en place ne bouge ;
/// 5. la base restaurée est ouverte et son intégrité contrôlée avant d'être
///    mise en service ;
/// 6. ce qui était là est **déplacé**, pas supprimé.
pub fn restore(req: RestoreRequest<'_>) -> BackupResult<RestoreReport> {
    let file = std::fs::File::open(req.archive)
        .map_err(|e| BackupError::NotAnArchive(format!("{} : {e}", req.archive.display())))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| BackupError::NotAnArchive(format!("ZIP illisible ({e})")))?;
    let manifest = read_manifest(&mut zip)?;

    if manifest.schema_version > SCHEMA_VERSION {
        return Err(BackupError::SchemaTooNew {
            archive: manifest.schema_version,
            binary: SCHEMA_VERSION,
        });
    }
    if !manifest.entries.iter().any(|e| e.path == DB_ENTRY) {
        return Err(BackupError::Corrupt(format!(
            "aucune entrée {DB_ENTRY} au manifeste"
        )));
    }

    let occupied = occupied_paths(req.config_dir);
    if !occupied.is_empty() && !req.confirm_overwrite {
        return Err(BackupError::ConfirmationRequired(format!(
            "{} porte déjà des données du hub ({}) — la restauration les remplacerait. \
             Renvoyer la demande avec confirm_overwrite: true (ou --force en ligne de \
             commande). Rien n'a été touché.",
            req.config_dir.display(),
            occupied.join(", ")
        )));
    }

    // Tout est extrait à côté d'abord. Un manifeste qui ment ou une archive
    // tronquée doit être découvert **avant** que le volume ne bouge.
    std::fs::create_dir_all(req.config_dir)?;
    let staging = req.config_dir.join(format!(".restauration-{}", req.now));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;

    if let Err(e) = extract_and_verify(&mut zip, &manifest, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    // La base extraite s'ouvre-t-elle vraiment ? C'est la question à laquelle
    // une sauvegarde doit répondre, et c'est ici qu'on la pose — pas après
    // avoir écrasé ce qui marchait.
    if let Err(e) = check_sqlite(&staging.join(DB_ENTRY), manifest.schema_version) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    // Ce qui est en place part de côté. Rien n'est supprimé : une
    // restauration sur la mauvaise archive doit rester réparable.
    let aside = if occupied.is_empty() {
        None
    } else {
        let aside = req
            .config_dir
            .join(format!("avant-restauration-{}", stamp(req.now)));
        std::fs::create_dir_all(&aside)?;
        for name in &occupied {
            let _ = std::fs::rename(req.config_dir.join(name), aside.join(name));
        }
        Some(aside)
    };

    let mut restored = Vec::new();
    for entry in &manifest.entries {
        if entry.path.starts_with(INFLUX_PREFIX) {
            // Les séries ne se posent pas dans le volume du hub : elles
            // repassent par `influx restore`.
            continue;
        }
        let from = staging.join(&entry.path);
        let to = req.config_dir.join(target_name(&entry.path));
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        move_file(&from, &to)?;
        restore_permissions(&to);
        restored.push(target_name(&entry.path));
    }

    let (influx_restored, influx_error) = match (&req.influx, manifest.influx_included) {
        (Some(target), true) => match run_influx_restore(target, &staging.join("influx")) {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e.to_string())),
        },
        (None, true) => (
            false,
            Some(
                "les séries sont dans l'archive mais aucune instance InfluxDB n'a été \
                 fournie : relancez la restauration depuis le conteneur du hub"
                    .to_string(),
            ),
        ),
        (_, false) => (
            false,
            Some("l'archive ne contient pas de sauvegarde InfluxDB".to_string()),
        ),
    };

    let _ = std::fs::remove_dir_all(&staging);
    restored.sort();

    Ok(RestoreReport {
        hub_version: manifest.hub_version,
        schema_version: manifest.schema_version,
        created_at: manifest.created_at,
        restored,
        moved_aside: aside,
        influx_restored,
        influx_error,
        // Le processus qui tourne tient encore l'ancienne base ouverte : il
        // continuera de servir ses données jusqu'au redémarrage. Le taire
        // ferait croire à une restauration sans effet.
        restart_required: true,
    })
}

/// `secrets/secret.key` retombe à plat dans le volume : c'est le rangement de
/// l'archive, pas celui du hub.
fn target_name(entry: &str) -> String {
    entry.strip_prefix(SECRETS_PREFIX).unwrap_or(entry).to_string()
}

fn occupied_paths(config_dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for name in std::iter::once(DB_ENTRY).chain(SECRET_FILES.iter().copied()) {
        if config_dir.join(name).is_file() {
            found.push(name.to_string());
        }
    }
    found
}

fn extract_and_verify<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    manifest: &Manifest,
    staging: &Path,
) -> BackupResult<()> {
    // **On n'itère pas sur les entrées de l'archive, on itère sur le
    // manifeste.** Une entrée en trop dans le ZIP n'est jamais extraite : le
    // manifeste décide, pas le contenu.
    for entry in &manifest.entries {
        if !entry_is_safe(&entry.path) {
            return Err(BackupError::NotAnArchive(format!(
                "entrée refusée : « {} »",
                entry.path
            )));
        }
        let mut source = zip.by_name(&entry.path).map_err(|_| {
            BackupError::Corrupt(format!("« {} » annoncée au manifeste et absente", entry.path))
        })?;
        let destination = staging.join(&entry.path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&destination)?;
        let written = std::io::copy(&mut source, &mut out)?;
        out.flush()?;
        drop(out);

        if written != entry.bytes {
            return Err(BackupError::Corrupt(format!(
                "« {} » : {written} octets extraits, {} annoncés",
                entry.path, entry.bytes
            )));
        }
        let sum = sha256_file(&destination)?;
        if sum != entry.sha256 {
            return Err(BackupError::Corrupt(format!(
                "« {} » : somme SHA-256 différente de celle du manifeste — \
                 l'archive est abîmée ou a été retouchée",
                entry.path
            )));
        }
    }
    Ok(())
}

/// Ouvre la base extraite et vérifie qu'elle est réellement exploitable.
/// `PRAGMA integrity_check` est ce que la copie de fichier ne garantit jamais.
fn check_sqlite(path: &Path, expected_schema: i64) -> BackupResult<()> {
    let conn = rusqlite::Connection::open(path)
        .map_err(|e| BackupError::Corrupt(format!("{DB_ENTRY} ne s'ouvre pas : {e}")))?;
    let verdict: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| BackupError::Corrupt(format!("{DB_ENTRY} : integrity_check a échoué ({e})")))?;
    if verdict != "ok" {
        return Err(BackupError::Corrupt(format!(
            "{DB_ENTRY} : integrity_check répond « {verdict} »"
        )));
    }
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| BackupError::Corrupt(e.to_string()))?;
    if version != expected_schema {
        return Err(BackupError::Corrupt(format!(
            "{DB_ENTRY} au schéma {version}, le manifeste annonce {expected_schema}"
        )));
    }
    Ok(())
}

/// Restaure le seul volet InfluxDB d'une archive.
///
/// Séparé de `restore` parce que le jeton opérateur qu'il faut pour appeler
/// `influx` est **lui-même dans l'archive** : sur un volume vierge — le cas
/// d'une machine neuve, celui qui compte — on ne l'a qu'une fois les fichiers
/// remis en place. La ligne de commande enchaîne donc les deux dans cet
/// ordre ; l'API, qui tourne sur un hub déjà configuré, a déjà son jeton.
pub fn restore_influx(archive: &Path, target: &InfluxTarget) -> BackupResult<()> {
    let file = std::fs::File::open(archive)
        .map_err(|e| BackupError::NotAnArchive(format!("{} : {e}", archive.display())))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| BackupError::NotAnArchive(format!("ZIP illisible ({e})")))?;
    let manifest = read_manifest(&mut zip)?;
    if !manifest.influx_included {
        return Err(BackupError::Corrupt(
            "l'archive ne contient pas de sauvegarde InfluxDB".into(),
        ));
    }

    let staging = std::env::temp_dir().join(format!("lanprobe-influx-{}", now()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let series = Manifest {
        entries: manifest
            .entries
            .iter()
            .filter(|e| e.path.starts_with(INFLUX_PREFIX))
            .cloned()
            .collect(),
        ..manifest
    };
    let outcome = extract_and_verify(&mut zip, &series, &staging)
        .and_then(|()| run_influx_restore(target, &staging.join("influx")));
    let _ = std::fs::remove_dir_all(&staging);
    outcome
}

fn run_influx_restore(target: &InfluxTarget, dir: &Path) -> BackupResult<()> {
    let mut cmd = std::process::Command::new(&target.cli);
    // `--full` : l'archive porte la sauvegarde complète de l'instance, et le
    // jeton opérateur restauré avec elle correspond à cet état-là. Restaurer
    // les deux séparément donnerait un hub qui ne sait plus parler à son
    // propre Influx.
    cmd.arg("restore")
        .arg("--full")
        .arg("--host")
        .arg(&target.host)
        .arg("--token")
        .arg(&target.token);
    if target.skip_verify {
        cmd.arg("--skip-verify");
    }
    cmd.arg(dir);
    run_influx(cmd, "influx restore")
}

fn move_file(from: &Path, to: &Path) -> BackupResult<()> {
    // `rename` échoue entre systèmes de fichiers ; la zone de travail est
    // dans le volume, donc ça passe presque toujours — mais un `/backup`
    // monté ailleurs rend le repli nécessaire.
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            let _ = std::fs::remove_file(from);
            Ok(())
        }
    }
}

/// Le volume est relu par un hub qui tourne en utilisateur non privilégié :
/// une clé restaurée en 0644 serait une régression silencieuse par rapport à
/// ce que `tls.rs` et `secrets.rs` posent.
fn restore_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

// ── Sommes et dates ────────────────────────────────────────────────────────

fn sha256_file(path: &Path) -> BackupResult<String> {
    let mut file = std::fs::File::open(path)?;
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        context.update(&buffer[..read]);
    }
    Ok(context
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Calendrier civil UTC depuis un instant Unix. Le hub n'embarque pas de
/// bibliothèque de dates — `web.rs` fait déjà la conversion inverse à la
/// main pour la même raison : une dépendance de plus à auditer pour formater
/// six nombres ne se justifie pas.
fn civil_utc(unix: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // Algorithme de Howard Hinnant, `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (secs / 3600) as u32,
        ((secs % 3600) / 60) as u32,
        (secs % 60) as u32,
    )
}

fn unix_from_civil(y: i64, m: i64, d: i64, h: i64, mi: i64, s: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + h * 3600 + mi * 60 + s
}

fn format_rfc3339(unix: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_utc(unix);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn stamp(unix: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_utc(unix);
    format!("{y:04}.{mo:02}.{d:02}_{h:02}.{mi:02}.{s:02}")
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{AuditFilter, Outcome, Role};

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lanprobe-backup-{}-{}-{}",
            tag,
            std::process::id(),
            now()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 2026-08-28T14:30:00Z — la date de l'exemple du contrat.
    const T0: i64 = 1_787_927_400;

    /// Un hub peuplé : parc, comptes, audit, réglages, secrets scellés, et un
    /// `secret.key` dans le volume. C'est ce qu'une restauration doit rendre
    /// à l'identique.
    fn populated(config_dir: &Path) -> Db {
        let db = Db::open(&config_dir.join(DB_ENTRY)).unwrap();
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_user("lecteur", "password123", Role::Viewer).unwrap();
        db.create_user("operateur", "password123", Role::Operator).unwrap();

        let durand = db.create_site("Durand").unwrap();
        let martin = db.create_site("Martin").unwrap();
        db.enroll_probe(&durand.site_id, "Paris").unwrap();
        db.enroll_probe(&durand.site_id, "Lyon").unwrap();
        db.enroll_probe(&martin.site_id, "Paris").unwrap();

        db.set_setting("retention_days", "90").unwrap();
        db.set_setting("heartbeat_interval_secs", "30").unwrap();
        db.set_sealed("notify_webhook", "enc:v1:contenu-scelle").unwrap();

        db.record_audit(Some("admin"), "auth.login", None, Outcome::Success, None)
            .unwrap();
        db.record_audit(
            Some("operateur"),
            "probe.enroll",
            Some("Paris"),
            Outcome::Success,
            Some("via code"),
        )
        .unwrap();
        db.record_audit(None, "auth.login", None, Outcome::Failure, Some("inconnu"))
            .unwrap();

        // Les fichiers du volume qui ne sont pas en base. `secret.key` est le
        // plus important : sans lui les canaux de notification deviennent
        // illisibles après restauration.
        std::fs::write(config_dir.join("secret.key"), b"cle-de-scellement-32-octets-xxxx").unwrap();
        std::fs::write(config_dir.join("influx-operator-token"), b"jeton-operateur").unwrap();
        std::fs::write(config_dir.join("hub-cert.pem"), b"-----BEGIN CERTIFICATE-----\n").unwrap();
        std::fs::write(config_dir.join("hub-key.pem"), b"-----BEGIN PRIVATE KEY-----\n").unwrap();
        db
    }

    fn backup_into(db: &Db, config_dir: &Path, backup_dir: &Path, at: i64) -> BackupReport {
        create(BackupRequest {
            db,
            config_dir,
            backup_dir,
            influx: None,
            now: at,
        })
        .unwrap()
    }

    /// Réécrit une archive en laissant le test modifier ses entrées. Sert à
    /// fabriquer les archives abîmées ou forgées qu'on doit savoir refuser.
    fn rewrite(archive: &Path, mutate: impl FnOnce(&mut Vec<(String, Vec<u8>)>)) {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(archive).unwrap()).unwrap();
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).unwrap();
            let name = file.name().to_string();
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).unwrap();
            entries.push((name, buffer));
        }
        mutate(&mut entries);

        let mut writer = zip::ZipWriter::new(std::fs::File::create(archive).unwrap());
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            writer.start_file(name, options).unwrap();
            writer.write_all(&content).unwrap();
        }
        writer.finish().unwrap();
    }

    fn manifest_of(entries: &[(String, Vec<u8>)]) -> Manifest {
        let raw = entries.iter().find(|(n, _)| n == MANIFEST_NAME).unwrap();
        serde_json::from_slice(&raw.1).unwrap()
    }

    fn set_manifest(entries: &mut [(String, Vec<u8>)], manifest: &Manifest) {
        for (name, content) in entries.iter_mut() {
            if name == MANIFEST_NAME {
                *content = serde_json::to_vec_pretty(manifest).unwrap();
            }
        }
    }

    // ── Le nom et le manifeste ─────────────────────────────────────────────

    #[test]
    fn an_archive_is_named_with_the_hub_version_and_the_date() {
        let dir = tmp_dir("nom");
        let db = populated(&dir);
        let backups = dir.join("backup");
        let report = backup_into(&db, &dir, &backups, T0);

        assert_eq!(
            report.file,
            format!("lanprobe_backup_v{}_2026.08.28_14.30.00.zip", env!("CARGO_PKG_VERSION")),
            "le nom doit dire la version et la date sans qu'on ouvre l'archive"
        );
        assert!(report.path.is_file());
        assert!(report.bytes > 0);
    }

    #[test]
    fn the_manifest_carries_the_hub_version_and_the_schema_version() {
        let dir = tmp_dir("manifeste");
        let db = populated(&dir);
        let report = backup_into(&db, &dir, &dir.join("backup"), T0);

        let manifest = inspect(&report.path).unwrap();
        assert_eq!(manifest.marker, MARKER);
        assert_eq!(manifest.format, FORMAT_VERSION);
        assert_eq!(manifest.hub_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            manifest.schema_version, SCHEMA_VERSION,
            "sans le schéma on ne sait pas si l'archive est restaurable"
        );
        assert_eq!(manifest.created_at, "2026-08-28T14:30:00Z");
        assert_eq!(manifest.created_at_unix, T0);

        let db_entry = manifest.entries.iter().find(|e| e.path == DB_ENTRY).unwrap();
        assert!(db_entry.bytes > 0);
        assert_eq!(db_entry.sha256.len(), 64, "SHA-256 en hexadécimal");
        assert!(manifest.total_bytes() > 0);
    }

    #[test]
    fn the_archive_carries_the_secrets_of_the_volume() {
        // Sans `secret.key`, une base restaurée rend les canaux de
        // notification illisibles. L'archive doit donc le porter — et la
        // documentation doit dire qu'elle contient un secret.
        let dir = tmp_dir("secrets");
        let db = populated(&dir);
        let report = backup_into(&db, &dir, &dir.join("backup"), T0);

        let manifest = inspect(&report.path).unwrap();
        let paths: Vec<&str> = manifest.entries.iter().map(|e| e.path.as_str()).collect();
        for expected in [
            "secrets/secret.key",
            "secrets/influx-operator-token",
            "secrets/hub-cert.pem",
            "secrets/hub-key.pem",
        ] {
            assert!(paths.contains(&expected), "{expected} manque : {paths:?}");
        }
    }

    #[test]
    fn a_backup_without_influx_says_why_instead_of_pretending() {
        // Une archive sans séries qui ne dit pas pourquoi est un piège : on
        // découvrirait le trou en tentant de restaurer.
        let dir = tmp_dir("sans-influx");
        let db = populated(&dir);
        let report = backup_into(&db, &dir, &dir.join("backup"), T0);

        assert!(!report.influx_included);
        assert!(report.influx_error.is_some(), "l'absence doit être motivée");
        let manifest = inspect(&report.path).unwrap();
        assert!(!manifest.influx_included);
        assert!(manifest.influx_error.is_some());
    }

    // ── Le tour complet : le livrable qui décide de tout ───────────────────

    #[test]
    fn a_populated_hub_survives_a_full_round_trip() {
        // Une sauvegarde jamais restaurée n'est pas une sauvegarde, c'est un
        // fichier qu'on espère. On sauvegarde une base peuplée, on restaure
        // AILLEURS, et on compare le parc, les comptes et l'audit.
        let source = tmp_dir("aller");
        let db = populated(&source);
        let backups = tmp_dir("aller-archives");
        let report = backup_into(&db, &source, &backups, T0);

        let sites_before = db.list_sites().unwrap();
        let probes_before = db.list_probes(None).unwrap();
        let users_before = db.list_users().unwrap();
        let audit_before = db.list_audit(&AuditFilter::default()).unwrap();
        let hash_before = db.password_hash_of("admin").unwrap();
        // Le hub continue de tourner et d'écrire pendant que l'archive
        // existe : c'est exactement le cas qu'une copie de fichier rate.
        db.create_site("Après-la-sauvegarde").unwrap();

        // Restauration dans un volume vierge.
        let target = tmp_dir("retour");
        let restored_report = restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: false,
            influx: None,
            now: T0 + 3600,
        })
        .unwrap();

        assert_eq!(restored_report.schema_version, SCHEMA_VERSION);
        assert_eq!(restored_report.hub_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(restored_report.created_at, "2026-08-28T14:30:00Z");
        assert!(
            restored_report.restart_required,
            "le processus tient encore l'ancienne base : le taire ferait croire \
             à une restauration sans effet"
        );
        assert!(restored_report.moved_aside.is_none(), "le volume cible était vide");

        let back = Db::open(&target.join(DB_ENTRY)).unwrap();
        assert_eq!(back.user_version().unwrap(), SCHEMA_VERSION);

        // Le parc.
        let sites_after = back.list_sites().unwrap();
        assert_eq!(
            sites_before.iter().map(|s| (&s.site_id, &s.name)).collect::<Vec<_>>(),
            sites_after.iter().map(|s| (&s.site_id, &s.name)).collect::<Vec<_>>(),
        );
        assert!(
            !sites_after.iter().any(|s| s.name == "Après-la-sauvegarde"),
            "l'archive fige l'instant du VACUUM, pas l'état courant"
        );
        let probes_after = back.list_probes(None).unwrap();
        assert_eq!(probes_before.len(), 3);
        assert_eq!(
            probes_before
                .iter()
                .map(|p| (&p.probe_id, &p.site_id, &p.name))
                .collect::<Vec<_>>(),
            probes_after
                .iter()
                .map(|p| (&p.probe_id, &p.site_id, &p.name))
                .collect::<Vec<_>>(),
        );

        // Les comptes, jusqu'au hash — un mot de passe qui ne marche plus
        // après restauration est une restauration ratée.
        let users_after = back.list_users().unwrap();
        assert_eq!(users_before.len(), 3);
        assert_eq!(
            users_before.iter().map(|u| (&u.username, u.role)).collect::<Vec<_>>(),
            users_after.iter().map(|u| (&u.username, u.role)).collect::<Vec<_>>(),
        );
        assert_eq!(back.password_hash_of("admin").unwrap(), hash_before);
        assert_eq!(back.verify_credentials("lecteur", "password123").unwrap(), "lecteur");

        // L'audit, lignes et ordre.
        let audit_after = back.list_audit(&AuditFilter::default()).unwrap();
        assert_eq!(audit_before.len(), audit_after.len());
        assert_eq!(
            audit_before
                .iter()
                .map(|e| (e.id, &e.actor, &e.action, &e.target, e.outcome, &e.detail))
                .collect::<Vec<_>>(),
            audit_after
                .iter()
                .map(|e| (e.id, &e.actor, &e.action, &e.target, e.outcome, &e.detail))
                .collect::<Vec<_>>(),
        );

        // Les réglages et les secrets scellés.
        assert_eq!(back.get_setting("retention_days").unwrap().as_deref(), Some("90"));
        assert_eq!(
            back.get_sealed("notify_webhook").unwrap().as_deref(),
            Some("enc:v1:contenu-scelle")
        );

        // Et la clé qui les ouvre : sans elle le canal serait `unreadable`.
        assert_eq!(
            std::fs::read(target.join("secret.key")).unwrap(),
            std::fs::read(source.join("secret.key")).unwrap(),
            "secret.key doit être restauré à l'octet près"
        );
        assert_eq!(
            std::fs::read(target.join("influx-operator-token")).unwrap(),
            b"jeton-operateur"
        );
        assert!(target.join("hub-cert.pem").is_file());
        // Le rangement de l'archive ne doit pas déborder dans le volume.
        assert!(!target.join("secrets").exists());
    }

    #[test]
    fn a_backup_taken_during_an_unfinished_write_is_still_restorable() {
        // LE test du lot. Copier `hub.sqlite` pendant qu'il est écrit produit
        // une archive qui a l'air valide et qu'on découvre inutilisable le
        // jour de la restauration. On reproduit exactement cet instant : un
        // second écrivain a une transaction ouverte, non validée, et un
        // `cache_size` minuscule l'a forcé à déverser ses pages sales dans le
        // fichier. Un `cp` ramasserait ici une base porteuse de lignes qui
        // n'ont jamais été validées. `VACUUM INTO` ne les voit pas.
        let dir = tmp_dir("transaction");
        let db = populated(&dir);
        let path = dir.join(DB_ENTRY);
        let filter = AuditFilter { limit: 500, ..Default::default() };
        let audit_before = db.list_audit(&filter).unwrap().len();

        // L'écrivain vit dans un autre fil, comme il vivrait dans un autre
        // processus : c'est le cas réel, un cron de l'hôte qui sauvegarde
        // pendant que le conteneur du hub tourne.
        let (spilled_tx, spilled_rx) = std::sync::mpsc::channel();
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            let conn = rusqlite::Connection::open(&writer_path).unwrap();
            conn.execute_batch("PRAGMA cache_size = 10; BEGIN IMMEDIATE;").unwrap();
            {
                let mut insert = conn
                    .prepare(
                        "INSERT INTO audit_log (at, actor, action, outcome)
                         VALUES (?1, 'jamais-valide', 'bruit', 'success')",
                    )
                    .unwrap();
                for i in 0..5_000i64 {
                    insert.execute([i]).unwrap();
                }
            }
            // Les pages sales sont dans le fichier, la transaction n'est pas
            // validée : c'est l'instant que `cp` ne doit jamais capturer.
            spilled_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1_500));
            // La transaction n'aboutit pas — panne, conflit, redémarrage.
            conn.execute_batch("ROLLBACK;").unwrap();
        });
        spilled_rx.recv().unwrap();

        let report = backup_into(&db, &dir, &dir.join("backup"), T0);
        writer.join().unwrap();

        let target = tmp_dir("transaction-cible");
        restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: false,
            influx: None,
            now: T0,
        })
        .expect("l'archive doit se restaurer : c'est toute la question");

        let back = Db::open(&target.join(DB_ENTRY)).unwrap();
        let entries = back.list_audit(&filter).unwrap();
        assert!(
            !entries.iter().any(|e| e.actor.as_deref() == Some("jamais-valide")),
            "l'archive porte des lignes d'une transaction jamais validée — \
             elle a été prise au niveau du fichier, pas par VACUUM INTO"
        );
        assert_eq!(
            entries.len(),
            audit_before,
            "l'archive doit figer l'état validé, ni plus ni moins"
        );
        assert!(back.list_sites().unwrap().iter().any(|s| s.name == "Durand"));
    }

    #[test]
    fn a_backup_is_correct_whatever_the_journal_mode() {
        // `journal_mode` est persistant DANS le fichier de base : une seule
        // commande suffit à passer un hub en WAL, et rien dans le produit ne
        // l'interdit. En WAL, copier `hub.sqlite` seul donne une base qui
        // passe `integrity_check` sans broncher et ne contient plus AUCUNE
        // table — l'archive a l'air parfaitement valide et n'a rien dedans.
        // C'est le mode de panne que ce lot existe pour empêcher.
        let dir = tmp_dir("wal");
        {
            let raw = rusqlite::Connection::open(dir.join(DB_ENTRY)).unwrap();
            let mode: String = raw
                .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
                .unwrap();
            assert_eq!(mode, "wal");
        }

        let db = populated(&dir);
        // La base reste ouverte : le WAL n'est pas replié dans le fichier
        // principal, exactement comme sur un hub qui tourne.
        assert!(dir.join("hub.sqlite-wal").is_file(), "le WAL doit être là");

        let report = backup_into(&db, &dir, &dir.join("backup"), T0);
        let sites_before = db.list_sites().unwrap().len();
        let users_before = db.list_users().unwrap().len();

        let target = tmp_dir("wal-cible");
        restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: false,
            influx: None,
            now: T0,
        })
        .expect("l'archive doit se restaurer");

        let back = Db::open(&target.join(DB_ENTRY)).unwrap();
        assert_eq!(
            back.list_sites().unwrap().len(),
            sites_before,
            "archive vide : elle a été prise au niveau du fichier, pas par VACUUM INTO"
        );
        assert_eq!(back.list_users().unwrap().len(), users_before);
        assert!(back.list_probes(None).unwrap().len() == 3);
    }

    #[test]
    fn the_restored_secret_key_stays_unreadable_to_others() {
        let source = tmp_dir("droits-source");
        let db = populated(&source);
        let report = backup_into(&db, &source, &source.join("backup"), T0);
        let target = tmp_dir("droits-cible");
        restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: false,
            influx: None,
            now: T0,
        })
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(target.join("secret.key")).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "la clé restaurée ne doit être lisible que par le hub");
        }
    }

    // ── Les refus ──────────────────────────────────────────────────────────

    #[test]
    fn an_archive_from_a_newer_schema_is_refused_and_says_so() {
        let source = tmp_dir("schema-source");
        let db = populated(&source);
        let report = backup_into(&db, &source, &source.join("backup"), T0);
        rewrite(&report.path, |entries| {
            let mut manifest = manifest_of(entries);
            manifest.schema_version = SCHEMA_VERSION + 1;
            set_manifest(entries, &manifest);
        });

        let target = tmp_dir("schema-cible");
        let err = restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: true,
            influx: None,
            now: T0,
        })
        .unwrap_err();

        assert!(
            matches!(err, BackupError::SchemaTooNew { .. }),
            "une archive plus récente que le binaire doit être refusée : {err:?}"
        );
        let message = err.to_string();
        assert!(message.contains(&(SCHEMA_VERSION + 1).to_string()));
        assert!(message.contains(&SCHEMA_VERSION.to_string()));
        assert!(
            std::fs::read_dir(&target).unwrap().next().is_none(),
            "rien ne doit avoir été écrit dans le volume cible"
        );
    }

    #[test]
    fn an_older_archive_is_accepted_because_migrations_replay() {
        // Le refus porte sur le schéma PLUS RÉCENT, pas sur l'ancien : une
        // archive d'une version antérieure se restaure et les migrations la
        // rattrapent au démarrage suivant.
        let source = tmp_dir("ancien-source");
        let db = populated(&source);
        let report = backup_into(&db, &source, &source.join("backup"), T0);
        let target = tmp_dir("ancien-cible");
        let done = restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: false,
            influx: None,
            now: T0,
        });
        assert!(done.is_ok(), "{done:?}");
    }

    #[test]
    fn restoring_over_live_data_needs_an_explicit_confirmation() {
        let source = tmp_dir("ecrase-source");
        let db = populated(&source);
        let report = backup_into(&db, &source, &source.join("backup"), T0);

        // Le volume cible porte déjà un hub — un autre.
        let target = tmp_dir("ecrase-cible");
        let live = populated(&target);
        live.create_site("Ne-doit-pas-disparaitre").unwrap();
        let before = std::fs::read(target.join(DB_ENTRY)).unwrap();
        drop(live);

        let err = restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: false,
            influx: None,
            now: T0,
        })
        .unwrap_err();
        assert!(matches!(err, BackupError::ConfirmationRequired(_)), "{err:?}");
        assert!(err.to_string().contains("confirm_overwrite"), "{err}");
        assert_eq!(
            std::fs::read(target.join(DB_ENTRY)).unwrap(),
            before,
            "rien ne doit avoir bougé"
        );
    }

    #[test]
    fn what_was_in_place_is_moved_aside_never_deleted() {
        let source = tmp_dir("cote-source");
        let db = populated(&source);
        let report = backup_into(&db, &source, &source.join("backup"), T0);

        let target = tmp_dir("cote-cible");
        let live = populated(&target);
        live.create_site("Unique-a-la-cible").unwrap();
        drop(live);
        std::fs::write(target.join("secret.key"), b"ancienne-cle").unwrap();

        let done = restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: true,
            influx: None,
            now: T0,
        })
        .unwrap();

        let aside = done.moved_aside.expect("l'ancien état doit être quelque part");
        assert!(aside.is_dir());
        assert_eq!(std::fs::read(aside.join("secret.key")).unwrap(), b"ancienne-cle");

        // L'ancienne base est intacte à côté, et porte encore son site.
        let old = Db::open(&aside.join(DB_ENTRY)).unwrap();
        assert!(old.list_sites().unwrap().iter().any(|s| s.name == "Unique-a-la-cible"));

        // Et la nouvelle est bien en place.
        let new = Db::open(&target.join(DB_ENTRY)).unwrap();
        assert!(!new.list_sites().unwrap().iter().any(|s| s.name == "Unique-a-la-cible"));
        assert!(new.list_sites().unwrap().iter().any(|s| s.name == "Durand"));
    }

    #[test]
    fn a_file_that_is_not_a_lanprobe_archive_is_refused() {
        let dir = tmp_dir("pas-une-archive");

        // Ni un ZIP.
        let junk = dir.join("photo.zip");
        std::fs::write(&junk, b"\x89PNG\r\n\x1a\n pas du tout un zip").unwrap();
        assert!(matches!(inspect(&junk), Err(BackupError::NotAnArchive(_))));

        // Un ZIP, mais sans manifeste.
        let stranger = dir.join("etranger.zip");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&stranger).unwrap());
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("notes.txt", options).unwrap();
            zip.write_all(b"bonjour").unwrap();
            zip.finish().unwrap();
        }
        let err = inspect(&stranger).unwrap_err();
        assert!(matches!(err, BackupError::NotAnArchive(_)), "{err:?}");
        assert!(err.to_string().contains(MANIFEST_NAME));

        // Un manifeste, mais pas le nôtre.
        let foreign = dir.join("autre-produit.zip");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&foreign).unwrap());
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file(MANIFEST_NAME, options).unwrap();
            zip.write_all(br#"{"marker":"autre-chose","format":1,"hub_version":"9","schema_version":1,"created_at":"","created_at_unix":0,"influx_included":false,"entries":[]}"#).unwrap();
            zip.finish().unwrap();
        }
        assert!(matches!(inspect(&foreign), Err(BackupError::NotAnArchive(_))));

        // Et la restauration refuse au même endroit, avant toute extraction.
        let target = tmp_dir("pas-une-archive-cible");
        assert!(matches!(
            restore(RestoreRequest {
                archive: &junk,
                config_dir: &target,
                confirm_overwrite: true,
                influx: None,
                now: T0,
            }),
            Err(BackupError::NotAnArchive(_))
        ));
        assert!(std::fs::read_dir(&target).unwrap().next().is_none());
    }

    #[test]
    fn a_forged_entry_path_is_refused_before_it_escapes_the_volume() {
        assert!(entry_is_safe("hub.sqlite"));
        assert!(entry_is_safe("secrets/secret.key"));
        assert!(entry_is_safe("influx/20260828T143000Z.bolt.gz"));
        for forged in [
            "../../etc/passwd",
            "/etc/passwd",
            "secrets/../../.ssh/authorized_keys",
            "secrets/",
            "C:\\Windows\\System32\\config",
            "quelconque.sh",
            "influx/../../root/.bashrc",
            "",
        ] {
            assert!(!entry_is_safe(forged), "« {forged} » doit être refusé");
        }

        // Et le refus tient de bout en bout, sur une vraie archive forgée.
        let source = tmp_dir("forge-source");
        let db = populated(&source);
        let report = backup_into(&db, &source, &source.join("backup"), T0);
        rewrite(&report.path, |entries| {
            let mut manifest = manifest_of(entries);
            manifest.entries.push(ManifestEntry {
                path: "../evade.txt".into(),
                bytes: 3,
                sha256: "0".repeat(64),
            });
            set_manifest(entries, &manifest);
            entries.push(("../evade.txt".into(), b"pwn".to_vec()));
        });

        let target = tmp_dir("forge-cible");
        let err = restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: true,
            influx: None,
            now: T0,
        })
        .unwrap_err();
        assert!(matches!(err, BackupError::NotAnArchive(_)), "{err:?}");
        assert!(!target.parent().unwrap().join("evade.txt").exists());
        assert!(!target.join(DB_ENTRY).exists(), "rien ne doit avoir été posé");
    }

    #[test]
    fn a_tampered_entry_is_caught_by_its_checksum() {
        let source = tmp_dir("somme-source");
        let db = populated(&source);
        let report = backup_into(&db, &source, &source.join("backup"), T0);
        rewrite(&report.path, |entries| {
            for (name, content) in entries.iter_mut() {
                if name == "secrets/secret.key" {
                    // Même longueur : seule la somme peut trahir la retouche.
                    *content = b"CLE-DE-SCELLEMENT-32-OCTETS-YYYY".to_vec();
                }
            }
        });

        let target = tmp_dir("somme-cible");
        let err = restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: true,
            influx: None,
            now: T0,
        })
        .unwrap_err();
        assert!(matches!(err, BackupError::Corrupt(_)), "{err:?}");
        assert!(err.to_string().contains("SHA-256"), "{err}");
        assert!(!target.join(DB_ENTRY).exists(), "rien ne doit avoir été posé");
    }

    #[test]
    fn an_entry_absent_from_the_archive_is_caught_before_anything_moves() {
        let source = tmp_dir("manquant-source");
        let db = populated(&source);
        let report = backup_into(&db, &source, &source.join("backup"), T0);
        rewrite(&report.path, |entries| {
            entries.retain(|(name, _)| name != "secrets/secret.key");
        });

        let target = tmp_dir("manquant-cible");
        let live = populated(&target);
        drop(live);
        let before = std::fs::read(target.join("secret.key")).unwrap();

        let err = restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: true,
            influx: None,
            now: T0,
        })
        .unwrap_err();
        assert!(matches!(err, BackupError::Corrupt(_)), "{err:?}");
        assert_eq!(
            std::fs::read(target.join("secret.key")).unwrap(),
            before,
            "l'échec doit survenir avant que le volume ne bouge"
        );
    }

    #[test]
    fn an_entry_absent_from_the_manifest_is_never_extracted() {
        // Le manifeste décide, pas le contenu du ZIP : quelqu'un qui glisse
        // un fichier dans une archive valide ne doit pas le voir atterrir
        // dans le volume.
        let source = tmp_dir("intrus-source");
        let db = populated(&source);
        let report = backup_into(&db, &source, &source.join("backup"), T0);
        rewrite(&report.path, |entries| {
            entries.push(("secrets/intrus.sh".into(), b"#!/bin/sh\nrm -rf /\n".to_vec()));
        });

        let target = tmp_dir("intrus-cible");
        restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: true,
            influx: None,
            now: T0,
        })
        .unwrap();
        assert!(target.join(DB_ENTRY).is_file(), "la restauration doit avoir eu lieu");
        assert!(!target.join("intrus.sh").exists());
        assert!(!target.join("secrets").exists());
    }

    #[test]
    fn an_archive_without_a_database_is_refused() {
        let source = tmp_dir("sans-base-source");
        let db = populated(&source);
        let report = backup_into(&db, &source, &source.join("backup"), T0);
        rewrite(&report.path, |entries| {
            let mut manifest = manifest_of(entries);
            manifest.entries.retain(|e| e.path != DB_ENTRY);
            set_manifest(entries, &manifest);
        });

        let target = tmp_dir("sans-base-cible");
        let err = restore(RestoreRequest {
            archive: &report.path,
            config_dir: &target,
            confirm_overwrite: true,
            influx: None,
            now: T0,
        })
        .unwrap_err();
        assert!(matches!(err, BackupError::Corrupt(_)), "{err:?}");
    }

    // ── Rétention ──────────────────────────────────────────────────────────

    #[test]
    fn the_list_is_chronological_across_hub_versions() {
        // La version précède la date dans le nom : un tri alphabétique
        // mélangerait les générations dès la première mise à jour du hub.
        let dir = tmp_dir("liste");
        for (name, _) in [
            ("lanprobe_backup_v1.0.0_2026.08.28_14.30.00.zip", 0),
            ("lanprobe_backup_v0.9.0_2026.08.29_02.00.00.zip", 0),
            ("lanprobe_backup_v1.0.0_2026.08.27_02.00.00.zip", 0),
            ("pas-une-archive.zip", 0),
            ("notes.txt", 0),
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }

        let archives = list(&dir).unwrap();
        assert_eq!(archives.len(), 3, "les fichiers étrangers sont ignorés : {archives:?}");
        assert_eq!(archives[0].file, "lanprobe_backup_v0.9.0_2026.08.29_02.00.00.zip");
        assert_eq!(archives[0].hub_version, "0.9.0");
        assert_eq!(archives[0].created_at, "2026-08-29T02:00:00Z");
        assert_eq!(archives[2].file, "lanprobe_backup_v1.0.0_2026.08.27_02.00.00.zip");
    }

    #[test]
    fn retention_keeps_the_most_recent_and_touches_nothing_else() {
        let dir = tmp_dir("retention");
        for day in 1..=5 {
            std::fs::write(
                dir.join(format!("lanprobe_backup_v1.0.0_2026.08.{day:02}_02.00.00.zip")),
                b"x",
            )
            .unwrap();
        }
        // Ce que l'utilisateur a déposé là ne regarde pas la rétention.
        std::fs::write(dir.join("a-garder.zip"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();

        let removed = prune(&dir, 2).unwrap();
        assert_eq!(removed.len(), 3);
        let left: Vec<String> = list(&dir).unwrap().into_iter().map(|a| a.file).collect();
        assert_eq!(
            left,
            vec![
                "lanprobe_backup_v1.0.0_2026.08.05_02.00.00.zip".to_string(),
                "lanprobe_backup_v1.0.0_2026.08.04_02.00.00.zip".to_string(),
            ]
        );
        assert!(dir.join("a-garder.zip").is_file());
        assert!(dir.join("notes.txt").is_file());
    }

    #[test]
    fn keeping_zero_means_keeping_everything_not_throwing_everything() {
        // Confondre les deux ferait de la valeur la plus prudente la plus
        // destructrice.
        let dir = tmp_dir("retention-zero");
        std::fs::write(dir.join("lanprobe_backup_v1.0.0_2026.08.01_02.00.00.zip"), b"x").unwrap();
        assert!(prune(&dir, 0).unwrap().is_empty());
        assert!(prune(&dir, -1).unwrap().is_empty());
        assert_eq!(list(&dir).unwrap().len(), 1);
    }

    #[test]
    fn two_backups_in_the_same_second_do_not_overwrite_each_other() {
        let dir = tmp_dir("collision");
        let db = populated(&dir);
        let backups = dir.join("backup");
        backup_into(&db, &dir, &backups, T0);
        let second = create(BackupRequest {
            db: &db,
            config_dir: &dir,
            backup_dir: &backups,
            influx: None,
            now: T0,
        });
        assert!(second.is_err(), "la seconde ne doit pas écraser la première");
        assert_eq!(list(&backups).unwrap().len(), 1);
    }

    // ── Déclenchement automatique ──────────────────────────────────────────

    #[test]
    fn a_hub_that_never_backed_up_is_due_immediately() {
        // Le cas qui compte : un hub installé et jamais sauvegardé. Attendre
        // vingt-quatre heures avant la première archive laisserait la
        // première journée — celle de la configuration — sans filet.
        assert!(is_due(None, T0, 24));
    }

    #[test]
    fn the_next_backup_is_due_one_interval_after_the_last_one() {
        let last = Some(T0);
        assert!(!is_due(last, T0, 24));
        assert!(!is_due(last, T0 + 23 * 3600, 24));
        assert!(is_due(last, T0 + 24 * 3600, 24));
        assert!(is_due(last, T0 + 72 * 3600, 24));
        // Une cadence courte se respecte aussi.
        assert!(is_due(last, T0 + 6 * 3600, 6));
        assert!(!is_due(last, T0 + 5 * 3600, 6));
    }

    #[test]
    fn the_due_date_is_read_from_the_archives_not_from_the_uptime() {
        // Sans cela, un hub redémarré toutes les six heures ne sauvegarderait
        // jamais : le compte à rebours repartirait de zéro à chaque
        // démarrage, et personne ne le verrait.
        let dir = tmp_dir("echeance");
        assert_eq!(last_backup_at(&dir), None);
        std::fs::write(dir.join("lanprobe_backup_v1.0.0_2026.08.27_02.00.00.zip"), b"x").unwrap();
        std::fs::write(dir.join("lanprobe_backup_v1.0.0_2026.08.28_02.00.00.zip"), b"x").unwrap();
        let last = last_backup_at(&dir).unwrap();
        assert_eq!(format_rfc3339(last), "2026-08-28T02:00:00Z");
        assert!(!is_due(Some(last), last + 3600, 24));
        assert!(is_due(Some(last), last + 25 * 3600, 24));
    }

    #[test]
    fn a_non_positive_interval_never_triggers() {
        // La cadence ne sert pas de bouton d'arrêt — `backup_enabled` est là
        // pour ça — mais une valeur aberrante ne doit pas déclencher en
        // boucle.
        assert!(!is_due(None, T0, 0));
        assert!(!is_due(Some(T0 - 10_000_000), T0, -1));
    }

    fn scheduler(config_dir: &Path, backup_dir: &Path) -> (Scheduler, std::sync::Arc<Db>) {
        let db = std::sync::Arc::new(Db::open(&config_dir.join(DB_ENTRY)).unwrap());
        db.create_initial_admin("admin", "password123").unwrap();
        db.create_site("Durand").unwrap();
        let settings = crate::settings::Settings::new(db.clone());
        (
            Scheduler {
                db: db.clone(),
                settings,
                config_dir: config_dir.to_path_buf(),
                backup_dir: backup_dir.to_path_buf(),
                influx: None,
            },
            db,
        )
    }

    #[tokio::test]
    async fn the_scheduler_backs_up_by_itself_and_stops_once_it_has() {
        let dir = tmp_dir("planif");
        let backups = dir.join("backup");
        let (scheduler, db) = scheduler(&dir, &backups);

        let file = scheduler
            .run_once()
            .await
            .expect("un hub sans archive doit sauvegarder tout de suite");
        assert!(file.starts_with("lanprobe_backup_v"));
        assert_eq!(list(&backups).unwrap().len(), 1);

        // Le tour suivant, l'échéance n'est pas passée : rien ne bouge.
        assert!(scheduler.run_once().await.is_none());
        assert_eq!(list(&backups).unwrap().len(), 1);

        // Et la sauvegarde automatique laisse sa trace : c'est ce qui permet
        // de constater après coup qu'elle tourne, sans acteur puisque
        // personne ne l'a demandée.
        let lines = db.list_audit(&AuditFilter::default()).unwrap();
        let line = lines.iter().find(|e| e.action == "backup.create").unwrap();
        assert!(line.actor.is_none());
        assert_eq!(line.target.as_deref(), Some(file.as_str()));
    }

    #[tokio::test]
    async fn the_scheduler_obeys_the_off_switch() {
        let dir = tmp_dir("planif-coupee");
        let backups = dir.join("backup");
        let (scheduler, _db) = scheduler(&dir, &backups);
        scheduler
            .settings
            .put(crate::settings::keys::BACKUP_ENABLED, "false", false)
            .unwrap();

        assert!(scheduler.run_once().await.is_none());
        assert!(list(&backups).unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_scheduler_applies_the_retention_it_is_given() {
        let dir = tmp_dir("planif-retention");
        let backups = dir.join("backup");
        std::fs::create_dir_all(&backups).unwrap();
        for day in 1..=4 {
            std::fs::write(
                backups.join(format!("lanprobe_backup_v1.0.0_2026.08.{day:02}_02.00.00.zip")),
                b"x",
            )
            .unwrap();
        }
        let (scheduler, _db) = scheduler(&dir, &backups);
        scheduler
            .settings
            .put(crate::settings::keys::BACKUP_KEEP_LAST, "2", true)
            .unwrap();

        scheduler.run_once().await.expect("l'échéance est largement passée");
        let left = list(&backups).unwrap();
        assert_eq!(left.len(), 2, "{left:?}");
        // La plus récente est celle qu'on vient de produire.
        assert!(left[0].created_at_unix > left[1].created_at_unix);
    }

    // ── Dates ──────────────────────────────────────────────────────────────

    #[test]
    fn the_utc_clock_round_trips() {
        for at in [0i64, 951_868_800, 1_709_208_000, T0, 4_102_444_800] {
            let (y, mo, d, h, mi, s) = civil_utc(at);
            assert_eq!(unix_from_civil(y, mo as i64, d as i64, h as i64, mi as i64, s as i64), at);
        }
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        // 2024 est bissextile : le 29 février doit exister.
        assert_eq!(format_rfc3339(1_709_208_000), "2024-02-29T12:00:00Z");
        assert_eq!(stamp(T0), "2026.08.28_14.30.00");
    }

    #[test]
    fn a_malformed_archive_name_is_not_an_archive() {
        for bad in [
            "lanprobe_backup_2026.08.28_14.30.00.zip",
            "lanprobe_backup_v1.0.0_2026.08.28.zip",
            "lanprobe_backup_v_2026.08.28_14.30.00.zip",
            "lanprobe_backup_v1.0.0_2026.08.28_14.30.00.tar",
            "sauvegarde.zip",
        ] {
            assert!(parse_archive_name(bad).is_none(), "« {bad} » ne doit pas être lu");
        }
        assert_eq!(
            parse_archive_name("lanprobe_backup_v1.2.3_2026.08.28_14.30.00.zip"),
            Some(("1.2.3".to_string(), T0)),
        );
    }
}
