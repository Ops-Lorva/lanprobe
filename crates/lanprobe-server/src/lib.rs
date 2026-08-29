//! `lanprobe-server` : le cœur de la sonde, sans interface.
//!
//! ⚠️ **Ce module n'expose plus de serveur HTTP.** L'application embarquait
//! autrefois son propre serveur web — routes, comptes, certificat auto-signé,
//! interface servie sur `https://…:8443` — pour qu'on puisse la consulter à
//! distance. Ce rôle appartient désormais au hub : c'est lui qui affiche le
//! parc, garde les comptes et porte l'authentification, et une sonde qui
//! exposerait en plus sa propre interface serait une seconde surface à
//! sécuriser pour la même information.
//!
//! Il reste donc : l'état partagé, l'ordonnanceur des mesures, le tampon
//! d'export, et le rattachement au hub. Le client desktop et le binaire
//! headless partagent tout ça ; ils ne diffèrent que par la façon dont on
//! les pilote — fenêtre Tauri d'un côté, ligne de commande de l'autre.

pub mod commands;
pub mod config;
pub mod export_buffer;
pub mod influxdb;
pub mod inventory;
pub mod monitor;
pub mod hub;
pub mod scheduler;
pub mod secrets;
pub mod state;

use std::path::PathBuf;

pub use state::AppState;

pub fn default_config_dir() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lanprobe")
}
