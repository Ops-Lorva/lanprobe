//! lanprobe-web — le hub auto-hébergé.
//!
//! Il ne relaie pas les mesures : les sondes écrivent directement dans
//! InfluxDB. Le hub tient les comptes, les sites, l'enrôlement, le battement
//! de cœur, et proxifie les lectures pour que le jeton Influx de lecture ne
//! quitte jamais le serveur.

pub mod assets;
pub mod auth;
pub mod backup;
pub mod client_ip;
pub mod db;
pub mod influx;
pub mod notify;
pub mod pairing;
pub mod probe;
pub mod report_i18n;
pub mod report_xlsx;
pub mod reports;
pub mod secrets;
pub mod settings;
pub mod sla_payload;
pub mod smtp;
pub mod tls;
pub mod passkeys;
pub mod totp;
pub mod web;
