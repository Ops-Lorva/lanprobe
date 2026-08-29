//! Publication de l'inventaire réseau vers le hub (contrat § 12).
//!
//! ⚠️ **Un scan n'est pas une métrique.** « Le port 22 est ouvert sur
//! 192.168.1.5 » est un fait daté, pas une valeur par instant. Rangé dans
//! Influx, il fabriquerait une série par couple (adresse, port) — un /24 dont
//! chaque machine expose vingt ports, c'est ~5 000 séries pour un seul scan.
//! C'est le mode de panne classique d'Influx, et il ne se voit pas avant qu'il
//! soit trop tard. Le détail va donc en SQLite, côté hub.
//!
//! Le compteur, lui, reste dans Influx et continue d'être écrit par la sonde :
//! deux chemins pour la même valeur finiraient par diverger, et on ne saurait
//! plus lequel croire.

use std::time::Duration;

use serde::Serialize;

use crate::state::AppState;
use crate::{hub, secrets::SecretKey};

#[derive(Debug, Serialize)]
pub struct ScanHost {
    pub ip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ScanPort {
    pub ip: String,
    pub port: u16,
    pub proto: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SpeedtestReport {
    pub engine: String,
    pub server_name: String,
    pub download_mbps: f64,
    pub upload_mbps: f64,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jitter_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ScanReport {
    /// `ports` | `discovery` | `speedtest`.
    pub kind: String,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cidr: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<ScanHost>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<ScanPort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speedtest: Option<SpeedtestReport>,
}

/// Envoie un rapport au hub. Sans effet si la sonde n'est pas rattachée.
///
/// ⚠️ Un échec ne fait rien échouer d'autre : le scan a eu lieu, ses compteurs
/// sont déjà dans Influx, et l'inventaire est un confort de lecture. Le perdre
/// vaut mieux que faire échouer une mesure qui, elle, a réussi.
pub async fn publish(state: &AppState, key: &SecretKey, report: ScanReport) {
    let cfg = hub::load(state);
    if !cfg.is_enrolled() {
        return;
    }
    let Ok(token) = crate::secrets::open(key, &cfg.token) else {
        tracing::warn!("inventaire non publié : jeton local illisible");
        return;
    };
    // Même contrainte d'interface que le reste du trafic de la sonde.
    let source = match crate::scheduler::resolve_src(state) {
        Ok(source) => source,
        Err(e) => {
            tracing::warn!("inventaire non publié : {e}");
            return;
        }
    };

    let url = format!("{}/api/probes/{}/scans", cfg.url, cfg.probe_id);
    let result = hub::http_client_for(cfg.allow_self_signed, source)
        .post(&url)
        .bearer_auth(token)
        .json(&report)
        .timeout(Duration::from_secs(20))
        .send()
        .await;

    match result {
        Ok(response) if response.status().is_success() => {
            tracing::info!("inventaire « {} » publié au hub", report.kind);
        }
        Ok(response) => {
            tracing::warn!("inventaire refusé par le hub ({})", response.status());
        }
        Err(e) => tracing::warn!("inventaire non publié : {e}"),
    }
}

/// Horodatage UNIX en secondes.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}
