//! lanprobe-core — logique réseau partagée entre le client Tauri (desktop)
//! et le serveur headless. Aucune dépendance à Tauri.

pub mod proc;
pub mod interfaces;
pub mod configure;
pub mod permissions;
pub mod ping;
pub mod discovery;
pub mod oui;
pub mod ports;
pub mod sla;
pub mod speedtest;
pub mod iperf;
pub mod internet;
pub mod public_ip;
pub mod updater;
