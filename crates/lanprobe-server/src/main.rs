//! `lanprobe-server` — la sonde LanProbe sans interface graphique.
//!
//! ⚠️ **Ce binaire n'écoute plus sur aucun port.** Il servait autrefois sa
//! propre interface web en HTTPS sur 8443, avec ses comptes et son certificat
//! auto-signé. Cette interface vit maintenant dans le hub : une sonde qui
//! exposerait la sienne en plus serait une seconde surface à sécuriser pour
//! afficher les mêmes mesures. On la pilote donc par la ligne de commande, et
//! on la consulte depuis le hub.
//!
//! ```text
//! lanprobe-server enroll --hub https://hub.exemple.fr --code A1B2-C3D4
//! lanprobe-server run            # mesure et envoie, jusqu'à Ctrl-C
//! lanprobe-server status
//! lanprobe-server forget
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use lanprobe_server::{config, default_config_dir, hub, influxdb, scheduler, secrets, state};
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(name = "lanprobe-server", version, about = "Sonde LanProbe (headless)")]
struct Cli {
    /// Dossier de configuration. Défaut : ~/.config/lanprobe
    #[arg(long, global = true)]
    config_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Mesure et envoie au hub, jusqu'à Ctrl-C. C'est l'action par défaut.
    Run,

    /// Rattache cette sonde à un hub.
    Enroll {
        /// Adresse du hub, par exemple https://hub.exemple.fr
        #[arg(long)]
        hub: String,
        /// Code d'enrôlement affiché par le hub. Valable 15 minutes, une fois.
        #[arg(long, conflicts_with_all = ["username", "password"])]
        code: Option<String>,
        /// Nom de la sonde. Vide, le hub en choisit un ; un ré-enrôlement
        /// garde de toute façon le nom défini côté hub.
        #[arg(long, default_value = "")]
        name: String,
        /// Enrôlement par identifiants, pour un déploiement scripté.
        #[arg(long, requires_all = ["password", "site"])]
        username: Option<String>,
        #[arg(long)]
        password: Option<String>,
        /// Site d'accueil, obligatoire avec `--username`.
        #[arg(long)]
        site: Option<String>,
        /// Accepte un certificat auto-signé. ⚠️ N'utilisez ceci que sur un
        /// réseau que vous maîtrisez : la sonde ne pourra plus distinguer le
        /// hub d'une machine qui se fait passer pour lui.
        #[arg(long)]
        allow_self_signed: bool,
    },

    /// Affiche l'état du rattachement.
    Status,

    /// Détache cette sonde. Le hub garde ses mesures ; il faudra un nouveau
    /// code pour la rattacher.
    Forget,
}

/// État minimal, sans ordonnanceur : suffisant pour lire et écrire la config.
fn bare_state(config_dir: &PathBuf) -> Result<state::AppState, String> {
    std::fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let store = Arc::new(config::ConfigStore::load(config::default_config_path(config_dir)));
    Ok(state::AppState::new_headless(store))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lanprobe_server=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let config_dir = cli.config_dir.unwrap_or_else(default_config_dir);

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run(config_dir).await?,
        Command::Enroll { hub: hub_url, code, name, username, password, site, allow_self_signed } => {
            let state = bare_state(&config_dir)?;
            let key = secrets::load_or_create_key(&config_dir)?;
            let credentials = match (&username, &password, &site) {
                (Some(u), Some(p), Some(s)) => Some((u.as_str(), p.as_str(), s.as_str())),
                _ => None,
            };
            if code.is_none() && credentials.is_none() {
                return Err("il faut --code, ou --username/--password/--site".into());
            }
            let cfg = hub::enroll(
                &state,
                &key,
                &hub_url,
                &name,
                code.as_deref(),
                credentials,
                allow_self_signed,
            )
            .await?;
            println!("Rattachée au hub {}", cfg.url);
            println!("  sonde  : {} ({})", cfg.name, cfg.probe_id);
            println!("  site   : {}", cfg.site);
            println!("Lancez `lanprobe-server run` pour commencer à mesurer.");
        }
        Command::Status => {
            let state = bare_state(&config_dir)?;
            let cfg = hub::load(&state);
            if cfg.url.is_empty() {
                println!("Non rattachée. `lanprobe-server enroll --hub … --code …` pour le faire.");
            } else {
                println!("Rattachée au hub {}", cfg.url);
                println!("  sonde : {} ({})", cfg.name, cfg.probe_id);
                println!("  site  : {}", cfg.site);
            }
        }
        Command::Forget => {
            let state = bare_state(&config_dir)?;
            let cfg = hub::load(&state);
            if cfg.url.is_empty() {
                println!("Déjà détachée, rien à faire.");
            } else {
                hub::forget(&state)?;
                // Dire ce qui N'A PAS été fait vaut mieux qu'un « ok » : on
                // détache souvent une sonde en croyant effacer son historique.
                println!("Détachée de {}. Ses mesures restent dans le hub.", cfg.url);
            }
        }
    }
    Ok(())
}

/// Boucle de mesure : ordonnanceur, moniteur internet, battement de cœur et
/// envoi au hub. Rend la main sur Ctrl-C.
async fn run(config_dir: PathBuf) -> Result<(), String> {
    let state = bare_state(&config_dir)?;
    info!("dossier de configuration : {}", config_dir.display());

    let cfg = hub::load(&state);
    if cfg.url.is_empty() {
        // Ce n'est pas une erreur : la sonde mesure quand même, et le tampon
        // gardera les points jusqu'au rattachement. Mais le dire évite de
        // chercher pendant une heure pourquoi le hub ne montre rien.
        warn!("aucun hub configuré — les mesures resteront dans le tampon local");
    } else {
        info!("hub {} — sonde « {} » sur le site « {} »", cfg.url, cfg.name, cfg.site);
    }

    let history = state.internet.clone();
    let iface = state.selected_interface.clone();
    let events = state.events.clone();
    tokio::spawn(lanprobe_core::internet::run_internet_monitor(
        history,
        iface,
        move |tick| {
            let _ = events.send(state::BroadcastEvent {
                event: "internet:tick".into(),
                payload: serde_json::to_value(tick).unwrap_or(serde_json::Value::Null),
            });
        },
    ));

    tokio::spawn(scheduler::run(state.clone()));

    match secrets::load_or_create_key(&config_dir) {
        Ok(key) => {
            let status = Arc::new(hub::WriteStatus::default());
            tokio::spawn(hub::run(state.clone(), key.clone(), status.clone()));
            tokio::spawn(influxdb::run(state.clone(), Some(key), Some(status)));
        }
        Err(e) => error!("clé de scellement indisponible, rattachement impossible : {e}"),
    }

    tokio::signal::ctrl_c().await.map_err(|e| e.to_string())?;
    info!("arrêt demandé");
    Ok(())
}
