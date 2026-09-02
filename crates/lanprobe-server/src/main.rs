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
use lanprobe_server::{
    config, default_config_dir, hub, influxdb, scheduler, secrets, state, tls_pin,
};
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
    Run {
        /// Interface réseau à mesurer, par exemple `en0` ou `eth0`.
        ///
        /// ⚠️ Sans elle, la sonde suit la route par défaut du système et ne
        /// rapporte NI passerelle NI adresses locales : la colonne
        /// « Passerelle » du rapport SLA et les libellés réseau restent vides
        /// à jamais. En mode graphique le choix se fait à l'écran ; sans
        /// écran, c'est ici.
        ///
        /// Le nom est retenu : on ne la repasse pas à chaque lancement.
        /// `lanprobe-server interfaces` liste celles que la machine expose.
        #[arg(long)]
        interface: Option<String>,
    },

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
        /// Accepte un certificat auto-signé. ⚠️ **Déconseillé** : la sonde
        /// ne peut alors plus distinguer le hub d'une machine qui se fait
        /// passer pour lui, et c'est son jeton qui part dans la requête.
        /// Préférez `--pin`, ou laissez la commande relever l'empreinte et
        /// vous la montrer.
        #[arg(long)]
        allow_self_signed: bool,
        /// Interface réseau à mesurer, par exemple `en0` ou `eth0`.
        ///
        /// ⚠️ Sans elle, la sonde suit la route par défaut du système et ne
        /// rapporte NI passerelle NI adresses locales : la colonne
        /// « Passerelle » du rapport SLA et les libellés réseau restent vides
        /// à jamais. En mode graphique le choix se fait à l'écran ; sans
        /// écran, c'est ici.
        ///
        /// Le nom est retenu : on ne la repasse pas à chaque lancement.
        /// `lanprobe-server interfaces` liste celles que la machine expose.
        #[arg(long)]
        interface: Option<String>,
        /// Empreinte SHA-256 attendue du certificat du hub. Seul ce
        /// certificat-là sera accepté, maintenant et ensuite.
        ///
        /// Sans cette option et sans `--allow-self-signed`, un hub dont le
        /// certificat ne se vérifie pas fait échouer la commande **en
        /// affichant l'empreinte à confirmer** : c'est le chemin normal.
        #[arg(long)]
        pin: Option<String>,
    },

    /// Liste les interfaces réseau que cette machine expose.
    ///
    /// ⚠️ Sans écran, choisir à l'aveugle n'est pas un choix : c'est cette
    /// commande qui rend `--interface` utilisable en pratique.
    Interfaces,

    /// Affiche l'état du rattachement.
    Status,

    /// Détache cette sonde. Le hub garde ses mesures ; il faudra un nouveau
    /// code pour la rattacher.
    Forget,
}

/// L'échec vient-il de la vérification du certificat ?
///
/// Sur le texte de l'erreur, faute de mieux : `reqwest` enveloppe l'erreur TLS
/// sans type distinct exploitable. Se tromper ici ne coûte qu'un message trop
/// serviable — on ne relâche aucune vérification, on propose seulement de
/// relever l'empreinte.
fn looks_like_tls_failure(message: &str) -> bool {
    let m = message.to_lowercase();
    ["certificate", "certificat", "tls", "self-signed", "unknownissuer", "invalidcertificate"]
        .iter()
        .any(|needle| m.contains(needle))
}

/// `https://hub.exemple.fr:8443/…` → `("hub.exemple.fr", 8443)`. `None` si
/// l'URL n'est pas en HTTPS : il n'y a alors aucun certificat à épingler.
fn split_https_authority(url: &str) -> Option<(String, u16)> {
    let rest = url.trim().strip_prefix("https://")?;
    let authority = rest.split(['/', '?']).next()?;
    if let Some(close) = authority.rfind(']') {
        let host = &authority[..=close];
        let port = authority[close + 1..]
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(443);
        return Some((host.trim_matches(|c| c == '[' || c == ']').to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            Some((host.to_string(), port.parse().unwrap_or(443)))
        }
        _ => Some((authority.to_string(), 443)),
    }
}

/// Ce que la sonde retient de la désignation d'interface.
#[derive(Debug, PartialEq, Eq)]
enum InterfaceChoice {
    /// Aucune interface désignée : on suit la route par défaut du système.
    /// C'est le comportement d'avant — assumé et journalisé, plus subi.
    SystemRoute,
    /// Interface désignée et présente sur la machine.
    Selected(String),
    /// Désignée, mais absente en ce moment.
    ///
    /// ⚠️ On la GARDE. Un adaptateur USB débranché ou un VPN arrêté revient ;
    /// l'oublier en silence rendrait la sonde muette sur son réseau sans que
    /// rien ne le dise, et refuser de démarrer l'empêcherait de mesurer pour
    /// un câble. On garde le choix et on le signale.
    Missing(String),
}

/// Résout l'interface à utiliser : ligne de commande, puis config, puis rien.
///
/// ⚠️ **Un nom demandé et inconnu est une ERREUR, pas un repli.** Sans écran,
/// une faute de frappe ne se voit pas : retomber en silence sur « aucune »
/// donnerait une sonde qui a l'air configurée et ne l'est pas — passerelle et
/// adresses locales vides à jamais, et rien pour l'expliquer. Le message nomme
/// donc la saisie ET les interfaces qui existent.
///
/// Un nom *enregistré* devenu introuvable ne suit pas la même règle : voir
/// `InterfaceChoice::Missing`.
fn resolve_interface(
    demandee: Option<&str>,
    enregistree: Option<&str>,
    disponibles: &[String],
) -> Result<InterfaceChoice, String> {
    if let Some(name) = demandee {
        if disponibles.iter().any(|d| d == name) {
            return Ok(InterfaceChoice::Selected(name.to_string()));
        }
        let liste = if disponibles.is_empty() {
            "aucune interface détectée sur cette machine".to_string()
        } else {
            format!("interfaces disponibles : {}", disponibles.join(", "))
        };
        return Err(format!("interface « {name} » inconnue — {liste}"));
    }
    match enregistree {
        Some(name) if disponibles.iter().any(|d| d == name) => {
            Ok(InterfaceChoice::Selected(name.to_string()))
        }
        Some(name) => Ok(InterfaceChoice::Missing(name.to_string())),
        None => Ok(InterfaceChoice::SystemRoute),
    }
}

/// Applique la désignation d'interface à l'état, et la retient si elle vient
/// d'être demandée.
///
/// ⚠️ **Avant tout démarrage de tâche.** Le battement, les mesures et le relevé
/// d'IP publique se lient à cette interface : les laisser partir d'abord ferait
/// sortir les premiers par la route par défaut, et une mesure prise par la
/// mauvaise interface est pire qu'une mesure absente — elle a l'air normale.
/// Même raison que côté graphique (f05b025), et le même emplacement en config.
fn select_interface(state: &state::AppState, demandee: Option<&str>) -> Result<(), String> {
    let enregistree = state
        .config
        .get()
        .get("selected_interface")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let disponibles = lanprobe_core::interfaces::list_interfaces();

    let name = match resolve_interface(demandee, enregistree.as_deref(), &disponibles)? {
        InterfaceChoice::SystemRoute => {
            info!(
                "aucune interface désignée — route par défaut du système. Ni passerelle ni adresses locales ne seront rapportées ; `lanprobe-server interfaces` liste les choix possibles."
            );
            return Ok(());
        }
        InterfaceChoice::Selected(name) => {
            info!("interface mesurée : {name}");
            name
        }
        InterfaceChoice::Missing(name) => {
            warn!(
                "interface « {name} » retenue mais absente pour l'instant                  (disponibles : {}). On la garde : un lien débranché revient.",
                if disponibles.is_empty() { "aucune".into() } else { disponibles.join(", ") }
            );
            name
        }
    };

    *state
        .selected_interface
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(name.clone());

    // Retenue seulement quand elle vient d'être DEMANDÉE : un simple
    // lancement ne doit pas réécrire la config, et surtout pas y figer une
    // interface absente que l'on vient seulement de signaler.
    if demandee.is_some() {
        let mut data = state.config.get();
        if let Some(obj) = data.as_object_mut() {
            obj.insert("selected_interface".into(), serde_json::Value::String(name));
            let _ = state.config.put(data);
        }
    }
    Ok(())
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

    match cli.command.unwrap_or(Command::Run { interface: None }) {
        Command::Run { interface } => run(config_dir, interface.as_deref()).await?,

        // ⚠️ Sans écran, choisir à l'aveugle n'est pas un choix. On imprime la
        // passerelle et les adresses avec chaque nom : c'est ce qui permet de
        // reconnaître le bon lien sur une machine qui en expose six.
        Command::Interfaces => {
            let noms = lanprobe_core::interfaces::list_interfaces();
            if noms.is_empty() {
                println!("Aucune interface détectée sur cette machine.");
            }
            for nom in &noms {
                let d = lanprobe_core::interfaces::get_interface_details(nom);
                let ip = lanprobe_core::interfaces::local_cidr(&d).unwrap_or_else(|| "—".into());
                let gw = d.gateway.clone().filter(|g| !g.is_empty()).unwrap_or_else(|| "—".into());
                // ⚠️ L'état du lien est la première chose à lire ici : choisir
                // une interface débranchée donne une sonde muette, et le
                // diagnostic part alors dans toutes les directions sauf la
                // bonne. « activée, sans porteuse » dit quoi faire — vérifier
                // le câble — là où « inactive » enverrait l'activer.
                let etat = match (d.admin_up, d.is_up) {
                    (_, true) => "actif",
                    (true, false) => "activé, sans porteuse",
                    (false, false) => "inactif",
                };
                println!("{nom}\tadresse {ip}\tpasserelle {gw}\t{etat}");
            }
        }
        Command::Enroll {
            hub: hub_url,
            code,
            name,
            username,
            password,
            site,
            allow_self_signed,
            pin,
            interface,
        } => {
            let state = bare_state(&config_dir)?;
            // ⚠️ AVANT l'enrôlement : la requête sort par cette interface, et
            // c'est aussi le moment où l'opérateur est devant sa machine. Une
            // faute de frappe doit se voir là, pas six heures plus tard dans
            // une colonne vide du rapport.
            select_interface(&state, interface.as_deref())?;
            let key = secrets::load_or_create_key(&config_dir)?;
            let credentials = match (&username, &password, &site) {
                (Some(u), Some(p), Some(s)) => Some((u.as_str(), p.as_str(), s.as_str())),
                _ => None,
            };
            if code.is_none() && credentials.is_none() {
                return Err("il faut --code, ou --username/--password/--site".into());
            }
            let pin = pin.unwrap_or_default();
            let cfg = match hub::enroll(
                &state,
                &key,
                &hub_url,
                &name,
                code.as_deref(),
                credentials,
                allow_self_signed,
                &pin,
            )
            .await
            {
                Ok(cfg) => cfg,
                // 🔴 Un échec de certificat n'est pas une panne à signaler
                // sèchement : c'est le moment exact où l'opérateur doit
                // décider. On relève l'empreinte et on la lui montre, plutôt
                // que de le laisser chercher `--allow-self-signed` et tout
                // désactiver pour de bon.
                Err(e) if pin.is_empty() && !allow_self_signed && looks_like_tls_failure(&e) => {
                    let (host, port) = split_https_authority(&hub_url)
                        .ok_or_else(|| format!("{e}"))?;
                    let seen = tls_pin::capture_fingerprint(&host, port, None)
                        .await
                        .map_err(|inner| format!("{e}\n(empreinte non relevable : {inner})"))?;
                    eprintln!("Le certificat de {host} ne se vérifie pas.");
                    eprintln!();
                    eprintln!("  empreinte SHA-256 : {seen}");
                    eprintln!();
                    eprintln!("Comparez-la à celle affichée par le hub, puis relancez avec :");
                    eprintln!("  --pin {seen}");
                    return Err("certificat à confirmer".into());
                }
                Err(e) => return Err(e.into()),
            };
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
async fn run(config_dir: PathBuf, interface: Option<&str>) -> Result<(), String> {
    let state = bare_state(&config_dir)?;
    info!("dossier de configuration : {}", config_dir.display());
    // ⚠️ Ici, et pas plus bas : les tâches lancées ensuite se lient à cette
    // interface. C'est le pendant headless de f05b025 — sans ça le premier
    // battement sort par la route par défaut, et le hub note « sonde vue à
    // l'instant » sur la foi d'une mesure venue du mauvais réseau.
    select_interface(&state, interface)?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn dispo() -> Vec<String> {
        vec!["en0".to_string(), "en1".to_string(), "utun3".to_string()]
    }

    #[test]
    fn an_unknown_interface_fails_loudly_and_names_those_that_exist() {
        // ⚠️ Sans écran, une faute de frappe ne se voit pas. Retomber en
        // silence sur « aucune » donnerait une sonde qui A L'AIR configurée et
        // ne l'est pas — exactement le défaut qu'on corrige : passerelle et
        // adresses locales vides à jamais, colonne « Passerelle » du rapport
        // SLA définitivement absente, et rien pour l'expliquer.
        let err = resolve_interface(Some("en5"), None, &dispo()).unwrap_err();
        assert!(err.contains("en5"), "l'erreur doit citer la saisie : {err}");
        assert!(err.contains("en0") && err.contains("utun3"), "et lister les vraies : {err}");
    }

    #[test]
    fn the_command_line_wins_over_what_was_stored() {
        let choice = resolve_interface(Some("en1"), Some("en0"), &dispo()).unwrap();
        assert_eq!(choice, InterfaceChoice::Selected("en1".into()));
    }

    #[test]
    fn a_stored_interface_is_reused_without_repeating_the_option() {
        // Le pendant headless de f05b025 : l'interface est relue au démarrage,
        // avant qu'une seule tâche parte. Sans ça le premier battement sort par
        // la route par défaut, et le hub note « sonde vue à l'instant » sur la
        // foi d'une mesure venue du mauvais réseau.
        let choice = resolve_interface(None, Some("utun3"), &dispo()).unwrap();
        assert_eq!(choice, InterfaceChoice::Selected("utun3".into()));
    }

    #[test]
    fn a_stored_interface_that_vanished_is_kept_and_signalled_never_dropped() {
        // 🔴 Un adaptateur USB débranché, un VPN arrêté : l'interface revient.
        // L'oublier en silence rendrait la sonde muette sur son réseau sans
        // que rien ne le dise ; refuser de démarrer l'empêcherait de mesurer
        // pour un câble. On garde le choix ET on le signale.
        let choice = resolve_interface(None, Some("en9"), &dispo()).unwrap();
        assert_eq!(choice, InterfaceChoice::Missing("en9".into()));
    }

    #[test]
    fn asking_for_nothing_follows_the_system_route() {
        // Le défaut d'avant, mais ASSUMÉ et journalisé, pas subi.
        assert_eq!(resolve_interface(None, None, &dispo()).unwrap(), InterfaceChoice::SystemRoute);
    }

    #[test]
    fn a_machine_without_any_interface_still_names_the_problem() {
        // Liste vide : le message ne doit pas se réduire à « choisissez parmi : ».
        let err = resolve_interface(Some("en0"), None, &[]).unwrap_err();
        assert!(err.contains("aucune"), "{err}");
    }
}
