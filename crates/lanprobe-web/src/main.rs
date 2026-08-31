//! `lanprobe-web` — le hub auto-hébergé.
//!
//! Le démarrage suit une règle : **rien de ce qui peut attendre ne bloque le
//! service**. Influx pas encore prêt, jeton opérateur pas encore écrit par
//! l'entrypoint — le hub écoute quand même et réessaie en tâche de fond. Un
//! conteneur qui refuse de démarrer parce qu'un autre n'est pas là est
//! ingérable en compose, puisqu'on ne contrôle pas l'ordre de disponibilité.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use lanprobe_web::{
    auth::Auth, backup, db::Db, influx, notify, secrets::Secrets, settings::Settings, tls, web,
};

#[derive(Parser)]
#[command(name = "lanprobe-web", about = "LanProbe Web — hub auto-hébergé")]
struct Args {
    /// Adresse d'écoute.
    #[arg(long, env = "LANPROBE_WEB_HOST", default_value = "0.0.0.0")]
    host: String,

    #[arg(long, env = "LANPROBE_WEB_PORT", default_value_t = 8080)]
    port: u16,

    /// Termine le TLS dans le hub, avec un certificat auto-signé.
    ///
    /// Par défaut le hub sert en clair : en auto-hébergement il vit presque
    /// toujours derrière un reverse proxy qui porte déjà un vrai certificat, et
    /// un certificat auto-signé n'y ajouterait qu'un avertissement de
    /// navigateur. L'option existe pour qui n'a pas de proxy.
    #[arg(long, env = "LANPROBE_WEB_TLS", default_value_t = false)]
    tls: bool,

    /// Volume du hub : base SQLite, certificat, jeton opérateur, token de setup.
    #[arg(long, env = "LANPROBE_WEB_CONFIG_DIR", default_value = "/data/lanprobe")]
    config_dir: PathBuf,

    /// Volume des archives de sauvegarde. Monté sur `/backup` dans le
    /// conteneur ; l'utilisateur le pointe où il veut depuis le compose.
    #[arg(long, env = "LANPROBE_WEB_BACKUP_DIR", default_value = lanprobe_web::backup::DEFAULT_BACKUP_DIR)]
    backup_dir: PathBuf,

    /// CLI `influx`, présente dans l'image du hub. `influx backup` est le
    /// seul moyen correct de sauvegarder InfluxDB — jamais une copie du
    /// répertoire `engine`.
    #[arg(long, env = "LANPROBE_INFLUX_CLI", default_value = "influx")]
    influx_cli: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

/// Sans sous-commande, le binaire sert le hub — c'est ce que fait le
/// conteneur, et le changer casserait toutes les images déjà déployées.
///
/// Les sous-commandes existent pour l'hôte : un cron peut sauvegarder sans
/// passer par l'API, et une restauration se fait hub arrêté — c'est le seul
/// moment où elle prend effet sans redémarrage.
#[derive(Subcommand)]
enum Command {
    /// Produit une archive dans le répertoire de sauvegarde, puis applique
    /// la rétention.
    Backup,
    /// Liste les archives présentes.
    Backups,
    /// Restaure une archive. ⚠️ Remplace la base et les secrets du volume.
    Restore {
        /// Nom d'une archive du répertoire de sauvegarde, ou chemin complet.
        archive: PathBuf,
        /// Sans ceci, une restauration qui écraserait des données en place
        /// est refusée.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Réinitialise le mot de passe d'un compte, et le réactive s'il était
    /// désactivé. ⚠️ Nécessite l'accès au conteneur ou au volume.
    ///
    /// C'est le **seul** chemin de récupération d'un hub auto-hébergé : pas
    /// d'e-mail, pas de support. Sans lui, un administrateur qui perd son mot
    /// de passe perd son parc, et la seule issue serait d'éditer SQLite à la
    /// main.
    /// Retire le second facteur d'un compte. ⚠️ Nécessite l'accès au
    /// conteneur ou au volume.
    ///
    /// C'est la **seule** sortie de secours d'un téléphone perdu sur un hub
    /// auto-hébergé : pas d'e-mail de récupération, pas de support. Sans
    /// elle, un second facteur activé et un appareil cassé enfermeraient le
    /// compte dehors définitivement.
    DisableTotp {
        /// Compte dont on retire le second facteur.
        username: String,
    },
    ResetPassword {
        /// Compte à réinitialiser.
        username: String,
        /// Nouveau mot de passe.
        password: String,
        /// Remet aussi le compte en rôle administrateur.
        #[arg(long, default_value_t = false)]
        promote: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    std::fs::create_dir_all(&args.config_dir).map_err(|e| e.to_string())?;

    if let Some(command) = &args.command {
        return run_command(&args, command);
    }

    let db = Arc::new(Db::open(&args.config_dir.join("hub.sqlite")).map_err(|e| e.to_string())?);
    let settings = Settings::new(db.clone());
    // L'environnement n'amorce que ce qui n'a jamais été réglé : ensuite,
    // c'est l'interface qui décide.
    if let Err(e) = settings.seed_from_env() {
        tracing::warn!("amorçage des réglages depuis l'environnement : {e}");
    }

    let auth = Arc::new(Auth::new(db.clone()));
    let token_path = lanprobe_web::auth::default_setup_token_path(&args.config_dir);
    match auth.init_setup_token(&token_path) {
        Ok(Some(token)) => tracing::warn!(
            "aucun compte configuré — POST /api/setup exige ce token de setup.\n\
             Token : {token}\n\
             (aussi lisible dans {} — permissions 0600)",
            token_path.display()
        ),
        Ok(None) => {}
        Err(e) => tracing::error!("token de setup non généré : {e}"),
    }

    // Jeton opérateur absent : on démarre quand même. L'interface est alors
    // joignable, ce qui est exactement ce qu'il faut pour diagnostiquer.
    let operator_token = match influx::load_operator_token(&args.config_dir) {
        Ok(token) => token,
        Err(e) => {
            tracing::error!("{e} — le provisionnement InfluxDB échouera jusqu'à correction");
            String::new()
        }
    };
    let influx = Arc::new(influx::Influx::new(settings.clone(), operator_token));

    {
        let influx = influx.clone();
        let retention_days = settings.retention_days();
        tokio::spawn(async move {
            influx::run_provisioning(
                influx.clone(),
                std::time::Duration::from_secs(2),
                std::time::Duration::from_secs(60),
            )
            .await;
            // Recalculée à chaque démarrage : un certificat Influx renouvelé
            // se propage ainsi aux sondes au battement de cœur suivant, sans
            // que personne n'intervienne.
            match influx.ensure_tls_fingerprint().await {
                Ok(f) if !f.is_empty() => tracing::info!("empreinte TLS d'InfluxDB : {f}"),
                Ok(_) => tracing::warn!("InfluxDB est servi en HTTP clair : rien à épingler"),
                Err(e) => tracing::warn!("empreinte TLS d'InfluxDB non calculée : {e}"),
            }
            if let Err(e) = influx.apply_retention(retention_days).await {
                tracing::warn!("rétention non appliquée : {e}");
            }
        });
    }

    // Clé de scellement des secrets de notification. Si le volume refuse
    // l'écriture, on continue avec une clé de session : le hub sert, et
    // l'opérateur reconfigure ses canaux — refuser de démarrer pour ça serait
    // une panne totale au service d'une fonctionnalité accessoire.
    let secrets = match Secrets::load(db.clone(), &args.config_dir) {
        Ok(secrets) => secrets,
        Err(e) => {
            tracing::error!(
                "clé de scellement indisponible ({e}) — les identifiants de \
                 notification ne survivront pas à ce démarrage"
            );
            Secrets::ephemeral(db.clone()).map_err(|e| e.to_string())?
        }
    };
    let notifier = notify::Notifier::new(db.clone(), secrets.clone(), settings.clone());

    // Boucle d'alerte. Le pas est court devant le délai : c'est le délai qui
    // décide de la réactivité, pas la cadence de la boucle.
    tokio::spawn(notify::run(
        notifier.clone(),
        std::time::Duration::from_secs(30),
    ));

    // Sauvegarde planifiée. Elle tourne toute seule : une sauvegarde qu'il
    // faut déclencher est une sauvegarde qu'on oublie. L'échéance se lit dans
    // les archives, pas depuis ce démarrage — un hub redémarré souvent
    // sauvegarde quand même.
    tokio::spawn(backup::run(
        backup::Scheduler {
            db: db.clone(),
            settings: settings.clone(),
            config_dir: args.config_dir.clone(),
            backup_dir: args.backup_dir.clone(),
            influx: influx_backup_target(&influx, &args.influx_cli),
        },
        std::time::Duration::from_secs(15 * 60),
    ));

    // ── Rétention de l'inventaire ─────────────────────────────────────────
    //
    // Séparée de la rétention Influx : l'inventaire vit en SQLite, et le
    // bucket ne sait rien de ces tables. Le pas est large parce que la
    // question ne se pose qu'en jours — une purge de plus ou de moins dans
    // la journée ne change rien à ce qu'on lit.
    {
        let db = db.clone();
        let settings = settings.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
            loop {
                tick.tick().await;

                // Historique des adresses publiques : il suit la rétention
                // Influx, pas celle de l'inventaire. Les mesures que ces
                // intervalles découpaient n'existent plus, l'intervalle ne
                // sert donc plus à rien. Les libellés, eux, ne sont jamais
                // purgés — ils se réappliquent quand le réseau revient.
                if let Some(cutoff) =
                    web::public_ip_history_cutoff(settings.retention_days(), lanprobe_web::db::now())
                {
                    match db.purge_public_ip_history(cutoff) {
                        Ok(0) => {}
                        Ok(n) => tracing::info!("historique des adresses : {n} intervalle(s) retirés"),
                        Err(e) => tracing::warn!("purge de l'historique des adresses : {e}"),
                    }
                }

                let days = settings.inventory_days();
                if days <= 0 {
                    continue;
                }
                match db.prune_inventory(days) {
                    Ok(0) => {}
                    Ok(n) => tracing::info!("inventaire : {n} scan(s) au-delà de {days} jours retirés"),
                    Err(e) => tracing::warn!("purge de l'inventaire : {e}"),
                }
            }
        });
    }

    // ⚠️ `--tls` (ou `LANPROBE_WEB_TLS`) l'emporte sur la base : une option
    // passée explicitement au démarrage ne doit pas pouvoir être annulée
    // depuis l'interface. Sans cette règle, un opérateur qui n'a pas de proxy
    // pourrait se retrouver à servir en clair d'un clic, sans rien remarquer.
    let tls_on = args.tls || settings.tls_enabled();

    let state = web::AppState {
        db,
        auth,
        settings,
        influx,
        notifier,
        secrets,
        ceremonies: Arc::new(lanprobe_web::passkeys::Ceremonies::new()),
        tls: tls_on,
        config_dir: args.config_dir.clone(),
        backup_dir: args.backup_dir.clone(),
        influx_cli: args.influx_cli.clone(),
            restart_required: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let router = web::build_router(state);

    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .map_err(|e| format!("adresse d'écoute invalide : {e}"))?;
    if tls_on {
        let tls_paths = tls::tls_paths(&args.config_dir);
        let tls_config = tls::server_config(&tls_paths)?;
        tracing::info!("hub à l'écoute sur https://{addr}");
        return axum_server::bind_rustls(
            addr,
            axum_server::tls_rustls::RustlsConfig::from_config(tls_config),
        )
        // ⚠️ `with_connect_info` et non `into_make_service` : sans lui,
        // l'adresse de la socket n'atteint jamais les gestionnaires et le
        // battement ne peut plus détecter qu'une sonde a changé de réseau.
        .serve(router.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .map_err(|e| e.to_string());
    }

    tracing::info!("hub à l'écoute sur http://{addr}");
    tracing::warn!(
        "le hub sert en clair : placez-le derrière un reverse proxy en HTTPS. \
         Sans proxy, le mot de passe administrateur circule en clair sur le \
         réseau — utilisez --tls si vous n'en avez pas."
    );
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| e.to_string())?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|e| e.to_string())
}

/// La cible InfluxDB pour `influx backup`, ou `None` s'il n'y a pas de jeton
/// opérateur : la commande échouerait de toute façon, et le dire à l'avance
/// donne un message utile plutôt qu'un code de sortie.
fn influx_backup_target(
    influx: &influx::Influx,
    cli: &std::path::Path,
) -> Option<backup::InfluxTarget> {
    influx
        .has_operator_token()
        .then(|| influx.backup_target(cli.to_path_buf()))
}

/// Les sous-commandes. Elles ouvrent le volume, agissent, et rendent la main
/// — pas de serveur, pas de boucle.
fn run_command(args: &Args, command: &Command) -> Result<(), String> {
    match command {
        Command::Backups => {
            let archives = backup::list(&args.backup_dir).map_err(|e| e.to_string())?;
            if archives.is_empty() {
                println!("aucune archive dans {}", args.backup_dir.display());
                return Ok(());
            }
            for archive in archives {
                println!("{}  {:>12} octets  {}", archive.created_at, archive.bytes, archive.file);
            }
            Ok(())
        }
        Command::Backup => {
            let db = open_volume(args)?;
            let settings = Settings::new(db.clone());
            let target = influx_from_volume(args, settings.clone());
            let report = backup::create(backup::BackupRequest {
                db: &db,
                config_dir: &args.config_dir,
                backup_dir: &args.backup_dir,
                influx: target,
                now: backup::now(),
            })
            .map_err(|e| e.to_string())?;
            println!("{} ({} octets)", report.path.display(), report.bytes);
            // Une archive sans les séries reste utile — elle porte les
            // comptes, le parc et les secrets — mais le taire serait un piège.
            if let Some(reason) = &report.influx_error {
                eprintln!("⚠️  sans les mesures InfluxDB : {reason}");
            }
            let _ = db.record_audit(
                None,
                "backup.create",
                Some(&report.file),
                lanprobe_web::db::Outcome::Success,
                Some("ligne de commande"),
            );
            let removed = backup::prune(&args.backup_dir, settings.backup_keep_last())
                .map_err(|e| e.to_string())?;
            for file in &removed {
                println!("retirée (rétention) : {file}");
            }
            if !removed.is_empty() {
                let _ = db.record_audit(
                    None,
                    "backup.retention",
                    None,
                    lanprobe_web::db::Outcome::Success,
                    Some(&format!("{} archive(s) retirée(s)", removed.len())),
                );
            }
            Ok(())
        }
        Command::ResetPassword { username, password, promote } => {
            let db = open_volume(args)?;
            db.reset_user_password(username, password)
                .map_err(|e| e.to_string())?;
            // Un compte désactivé qu'on réinitialise sans le réactiver reste
            // inutilisable : la commande de secours doit rendre l'accès, pas
            // un mot de passe qui ne sert à rien.
            let _ = db.set_user_disabled("système", username, false);
            if *promote {
                db.set_user_role("système", username, lanprobe_web::db::Role::Admin)
                    .map_err(|e| e.to_string())?;
            }
            // ⚠️ Journalisé comme le reste : cette commande contourne la
            // connexion, elle doit laisser une trace. Quelqu'un qui la lance
            // légitimement n'a rien à cacher ; quelqu'un d'autre, si.
            // Le `Result` est examiné : une trace d'audit qu'on croit écrite
            // alors qu'elle ne l'est pas vaut moins que pas de trace du tout.
            // On ne fait pas échouer la commande pour autant — le mot de passe
            // EST changé, le taire serait pire.
            if let Err(e) = db.record_audit(
                Some("système"),
                "user.password_reset_cli",
                Some(username),
                lanprobe_web::db::Outcome::Success,
                Some("réinitialisation depuis la ligne de commande"),
            ) {
                eprintln!("⚠️  mot de passe changé, mais le journal d'audit n'a pas pu l'enregistrer : {e}");
            }
            println!("mot de passe de « {username} » réinitialisé, compte actif");
            if *promote {
                println!("  rôle porté à administrateur");
            }
            return Ok(());
        }
        Command::DisableTotp { username } => {
            let db = open_volume(args)?;
            db.set_totp_enabled(username, false)
                .map_err(|e| e.to_string())?;
            // Le secret scellé part avec le drapeau : le garder ne protège
            // personne et ferait revenir un ancien code sur un QR que
            // l'utilisateur croira neuf.
            let secrets = lanprobe_web::secrets::Secrets::load(db.clone(), &args.config_dir)?;
            let _ = secrets.clear(&format!("totp:{username}"));
            let _ = db.record_audit(
                None,
                "auth.totp.disable",
                Some(username.as_str()),
                lanprobe_web::db::Outcome::Success,
                Some("ligne de commande"),
            );
            println!("second facteur retiré du compte « {username} »");
            Ok(())
        }
        Command::Restore { archive, force } => {
            // ⚠️ **Aucune ouverture de base avant la restauration.** Ouvrir
            // `hub.sqlite` le CRÉE dans le volume cible, et la garde
            // « ce volume porte déjà des données » se déclenche alors sur le
            // fichier que la commande vient elle-même de poser — rendant la
            // restauration sur une machine neuve impossible.
            //
            // Un nom nu désigne une archive du répertoire de sauvegarde :
            // c'est ainsi qu'on la lit dans `lanprobe-web backups`.
            let path = if archive.is_file() {
                archive.clone()
            } else {
                args.backup_dir.join(archive)
            };
            // ⚠️ Le jeton opérateur **de l'instance vivante**, capturé AVANT
            // que la restauration n'écrase le fichier.
            //
            // `influx restore` s'authentifie auprès de l'InfluxDB en place, qui
            // porte encore ses propres jetons — ceux de l'archive n'y existent
            // pas. Utiliser le jeton de l'archive donnait un `401` à tous les
            // coups : la restauration des mesures n'a donc jamais pu
            // fonctionner. Après un `--full` réussi, l'instance adopte les
            // jetons de l'archive, et le fichier déjà remis en place devient
            // le bon.
            let live_token = influx::load_operator_token(&args.config_dir).ok();

            let report = backup::restore(backup::RestoreRequest {
                archive: &path,
                config_dir: &args.config_dir,
                confirm_overwrite: *force,
                // Le volet InfluxDB se joue après, avec le jeton capturé
                // ci-dessus — pas avec celui que l'archive vient de poser.
                influx: None,
                now: backup::now(),
            })
            .map_err(|e| e.to_string())?;

            println!(
                "restauré : archive du {} (hub {}, schéma {})",
                report.created_at, report.hub_version, report.schema_version
            );
            for name in &report.restored {
                println!("  {name}");
            }
            if let Some(aside) = &report.moved_aside {
                println!("l'état précédent est conservé dans {}", aside.display());
            }

            // Maintenant seulement : la base et le jeton opérateur de
            // l'archive sont en place, on peut parler à InfluxDB.
            let db = open_volume(args)?;
            let settings = Settings::new(db.clone());
            match influx_target_with_token(args, settings, live_token) {
                Some(target) => match backup::restore_influx(&path, &target) {
                    Ok(()) => println!("  mesures InfluxDB restaurées"),
                    Err(e) => eprintln!("⚠️  mesures InfluxDB non restaurées : {e}"),
                },
                None => eprintln!(
                    "⚠️  mesures InfluxDB non restaurées : pas de jeton opérateur dans {}",
                    args.config_dir.display()
                ),
            }

            let _ = db.record_audit(
                None,
                "backup.restore",
                path.file_name().and_then(|n| n.to_str()),
                lanprobe_web::db::Outcome::Success,
                Some("ligne de commande"),
            );
            warn_if_restored_files_changed_owner(args);
            println!(
                "redémarrez le hub : un processus déjà lancé sert encore l'ancienne base"
            );
            Ok(())
        }
    }
}

/// Prévient quand la restauration a reposé la base sous un autre propriétaire
/// que celui du volume.
///
/// 🔴 C'est le cas normal du chemin documenté : `docker exec` s'exécute en
/// root, alors que le hub tourne sans privilège. La restauration réussit, dit
/// qu'elle a réussi, et le hub refuse ensuite d'ouvrir sa propre base avec
/// « unable to open database file » — au redémarrage, donc loin du message
/// qui aurait pu l'expliquer.
///
/// L'image répare cela toute seule à chaque démarrage (voir `entrypoint.sh`),
/// mais une restauration faite à la main sur l'hôte n'a pas ce filet : on le
/// dit ici, avec la commande qui corrige.
fn warn_if_restored_files_changed_owner(args: &Args) {
    use std::os::unix::fs::MetadataExt;

    let db_path = args.config_dir.join("hub.sqlite");
    let (Ok(dir), Ok(db)) = (
        std::fs::metadata(&args.config_dir),
        std::fs::metadata(&db_path),
    ) else {
        return;
    };
    if dir.uid() == db.uid() {
        return;
    }
    eprintln!(
        "⚠️  {} appartient maintenant à l'uid {} alors que {} appartient à l'uid {} — \n\
         \x20   le hub, qui tourne sans privilège, ne pourra pas l'ouvrir. Corrigez avec :\n\
         \x20     chown -R {}:{} {}",
        db_path.display(),
        db.uid(),
        args.config_dir.display(),
        dir.uid(),
        dir.uid(),
        dir.gid(),
        args.config_dir.display()
    );
}

/// Ouvre la base du volume. À n'appeler que lorsque l'existence du fichier
/// est voulue — jamais avant une restauration.
fn open_volume(args: &Args) -> Result<Arc<Db>, String> {
    Ok(Arc::new(
        Db::open(&args.config_dir.join("hub.sqlite")).map_err(|e| e.to_string())?,
    ))
}

fn influx_from_volume(args: &Args, settings: Settings) -> Option<backup::InfluxTarget> {
    let token = influx::load_operator_token(&args.config_dir).ok();
    influx_target_with_token(args, settings, token)
}

/// Construit la cible InfluxDB avec un jeton **fourni**, et non relu du volume.
///
/// La restauration en a besoin : elle doit s'authentifier avec le jeton de
/// l'instance vivante, capturé avant que l'archive n'écrase le fichier.
fn influx_target_with_token(
    args: &Args,
    settings: Settings,
    token: Option<String>,
) -> Option<backup::InfluxTarget> {
    let influx = influx::Influx::new(settings, token.unwrap_or_default());
    influx_backup_target(&influx, &args.influx_cli)
}
