//! Les sous-commandes, exercées sur le vrai binaire.
//!
//! Elles existent pour l'hôte : un cron sauvegarde sans passer par l'API, et
//! une restauration se fait hub arrêté — c'est le seul moment où elle prend
//! effet sans redémarrage. Ce chemin-là ne se teste pas en appelant la
//! bibliothèque : la première version de `lanprobe-web restore` ouvrait la
//! base avant de dispatcher, **créait** ainsi `hub.sqlite` dans le volume
//! cible, puis refusait de restaurer au motif que le volume portait déjà des
//! données. Vu seulement en lançant la commande.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use lanprobe_web::db::{Db, Role};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_lanprobe-web")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lanprobe-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(config_dir: &Path, backup_dir: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .arg("--config-dir")
        .arg(config_dir)
        .arg("--backup-dir")
        .arg(backup_dir)
        .args(args)
        .env("RUST_LOG", "warn")
        // Le jeton opérateur n'existe pas ici : sans cette mise à zéro, une
        // variable d'environnement de la machine de test ferait appeler une
        // vraie instance InfluxDB.
        .env_remove("INFLUX_TOKEN")
        .output()
        .expect("le binaire doit se lancer")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Un volume peuplé, monté par la bibliothèque : la commande n'a pas à
/// dépendre d'un serveur qui tourne.
fn populate(volume: &Path) {
    let db = Db::open(&volume.join("hub.sqlite")).unwrap();
    db.create_initial_admin("admin", "password123").unwrap();
    db.create_user("lecteur", "password123", Role::Viewer).unwrap();
    let site = db.create_site("Durand").unwrap();
    db.enroll_probe(&site.site_id, "Paris").unwrap();
    std::fs::write(volume.join("secret.key"), b"cle-de-scellement-du-volume").unwrap();
}

#[test]
fn a_backup_then_a_restore_into_a_fresh_volume() {
    let source = scratch("source");
    let archives = scratch("archives");
    populate(&source);

    let out = run(&source, &archives, &["backup"]);
    assert!(out.status.success(), "backup : {}", stderr(&out));
    let produced = stdout(&out);
    assert!(produced.contains("lanprobe_backup_v"), "{produced}");
    // Une archive sans les mesures reste utile, mais le taire serait un piège.
    assert!(
        stderr(&out).contains("InfluxDB"),
        "l'absence des mesures doit être annoncée : {}",
        stderr(&out)
    );

    let listing = run(&source, &archives, &["backups"]);
    assert!(listing.status.success());
    let file = stdout(&listing)
        .split_whitespace()
        .find(|w| w.starts_with("lanprobe_backup_v"))
        .expect("la liste doit nommer l'archive")
        .to_string();

    // ── Le cas qui compte : un volume neuf, une machine neuve. ──────────────
    let fresh = scratch("neuf");
    let out = run(&fresh, &archives, &["restore", &file]);
    assert!(
        out.status.success(),
        "restaurer dans un volume vierge doit marcher : {}",
        stderr(&out)
    );
    assert!(stdout(&out).contains("hub.sqlite"), "{}", stdout(&out));

    let back = Db::open(&fresh.join("hub.sqlite")).unwrap();
    assert!(back.get_user("admin").is_ok());
    assert_eq!(back.list_users().unwrap().len(), 2);
    assert!(back.list_sites().unwrap().iter().any(|s| s.name == "Durand"));
    assert_eq!(back.list_probes(None).unwrap().len(), 1);
    assert_eq!(
        std::fs::read(fresh.join("secret.key")).unwrap(),
        b"cle-de-scellement-du-volume",
        "sans secret.key les canaux de notification deviennent illisibles"
    );
}

#[test]
fn restoring_over_a_populated_volume_needs_force() {
    let source = scratch("ecrase-source");
    let archives = scratch("ecrase-archives");
    populate(&source);
    let out = run(&source, &archives, &["backup"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let file = stdout(&run(&source, &archives, &["backups"]))
        .split_whitespace()
        .find(|w| w.starts_with("lanprobe_backup_v"))
        .unwrap()
        .to_string();

    let target = scratch("ecrase-cible");
    populate(&target);
    std::fs::write(target.join("secret.key"), b"cle-de-la-cible").unwrap();

    let refused = run(&target, &archives, &["restore", &file]);
    assert!(!refused.status.success(), "le refus doit se voir au code de sortie");
    assert!(
        stderr(&refused).contains("--force"),
        "le refus doit nommer ce qu'il faut ajouter : {}",
        stderr(&refused)
    );
    assert_eq!(
        std::fs::read(target.join("secret.key")).unwrap(),
        b"cle-de-la-cible",
        "rien ne doit avoir bougé"
    );

    let forced = run(&target, &archives, &["restore", &file, "--force"]);
    assert!(forced.status.success(), "{}", stderr(&forced));
    assert_eq!(
        std::fs::read(target.join("secret.key")).unwrap(),
        b"cle-de-scellement-du-volume"
    );
    // Rien n'est supprimé : l'état précédent doit rester récupérable.
    assert!(
        stdout(&forced).contains("état précédent"),
        "la commande doit dire où est parti l'ancien volume : {}",
        stdout(&forced)
    );
    let aside: Vec<_> = std::fs::read_dir(&target)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("avant-restauration-"))
        .collect();
    assert_eq!(aside.len(), 1, "l'ancien volume doit être à côté");
    assert_eq!(
        std::fs::read(aside[0].path().join("secret.key")).unwrap(),
        b"cle-de-la-cible"
    );
}

#[test]
fn a_file_that_is_not_an_archive_is_refused_by_the_command() {
    let volume = scratch("cli-junk");
    let archives = scratch("cli-junk-archives");
    std::fs::write(archives.join("photo.zip"), b"\x89PNG\r\n\x1a\n pas une archive").unwrap();

    let out = run(&volume, &archives, &["restore", "photo.zip", "--force"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("LanProbe"),
        "le refus doit dire ce qu'on attendait : {}",
        stderr(&out)
    );
    assert!(
        !volume.join("hub.sqlite").exists(),
        "une restauration refusée ne doit rien laisser derrière elle"
    );
}

#[test]
fn listing_an_empty_backup_directory_is_not_an_error() {
    let volume = scratch("cli-vide");
    let archives = scratch("cli-vide-archives");
    let out = run(&volume, &archives, &["backups"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("aucune archive"), "{}", stdout(&out));
}
