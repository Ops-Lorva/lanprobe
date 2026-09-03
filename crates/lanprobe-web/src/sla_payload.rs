//! Le corps du rapport SLA d'une sonde, **hors de toute requête HTTP**.
//!
//! 🔴 **Pourquoi ce module existe.** Le classeur remis au client était fabriqué
//! dans le navigateur (`web-ui/src/lib/sla-report.ts`) à partir de la réponse
//! de `GET /api/probes/{id}/sla`. Le hub doit désormais le produire lui-même
//! (contrat § 23) : il lui faut donc le **même** corps, sans passer par une
//! requête HTTP à lui-même.
//!
//! ⚠️ **Un seul producteur, jamais deux.** Recopier la construction dans le
//! générateur de rapport aurait donné deux corps qui divergent — d'abord d'un
//! champ, puis d'un chiffre — et c'est celui du document remis au client qu'on
//! ne saurait plus expliquer. `probe_sla_samples` appelle donc [`build`] et se
//! contente de traduire l'erreur en code HTTP.
//!
//! ⚠️ **Aucun octet de la réponse de `/sla` ne change**, et un test le tient :
//! il compare le corps rendu par la route et celui rendu par [`build`], champ
//! par champ.

use serde_json::json;

use crate::db::DbError;
use crate::web::{internet_samples_from_flux_csv, samples_from_flux_csv, AppState, Window};

/// Ce qui peut empêcher le rapport d'exister.
///
/// ⚠️ Deux cas et non un : une sonde inconnue est un **404**, un Influx
/// injoignable un **502**. Les confondre ferait réessayer là où il n'y a rien à
/// retrouver, et abandonner là où l'amont sera revenu dans dix secondes.
#[derive(Debug)]
pub(crate) enum SlaPayloadError {
    /// La sonde, son inventaire, son historique d'adresses — le stockage.
    Db(DbError),
    /// La requête Flux des relevés de ping. Le volet internet, lui, dégrade.
    Influx(String),
}

impl std::fmt::Display for SlaPayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SlaPayloadError::Db(e) => write!(f, "{e}"),
            SlaPayloadError::Influx(e) => write!(f, "{e}"),
        }
    }
}

/// Construit le rapport brut d'une sonde sur une fenêtre.
///
/// `asked_start` / `asked_stop` sont les bornes **telles qu'elles ont été
/// demandées** — `None` sur une fenêtre glissante. Elles sont rendues telles
/// quelles (contrat § 3) et ne se déduisent pas de la fenêtre : `from`/`to`
/// valent toujours quelque chose, y compris quand personne n'a demandé de
/// bornes.
///
/// ⚠️ Rendus **bruts, jamais agrégés**. Le rapport ne se contente pas de
/// moyennes : il liste les coupures avec leur durée et rend la série complète.
/// Un résumé calculé ici obligerait le hub à refaire ce que le générateur sait
/// déjà faire, et les deux finiraient par diverger sur la définition d'une
/// coupure.
pub(crate) async fn build(
    state: &AppState,
    probe_id: &str,
    window: &Window,
    asked_start: Option<i64>,
    asked_stop: Option<i64>,
) -> Result<serde_json::Value, SlaPayloadError> {
    let probe = state.db.get_probe(probe_id).map_err(SlaPayloadError::Db)?;

    let bucket = state.settings.influx_bucket();
    let flux = format!(
        "from(bucket: {})\n  |> range({})\n  \
         |> filter(fn: (r) => r[\"probe_id\"] == {})\n  \
         |> filter(fn: (r) => r[\"_measurement\"] == \"ping_latency\")",
        crate::web::flux_string(&bucket),
        window.clause,
        crate::web::flux_string(probe_id),
    );
    let csv = state
        .influx
        .query_flux(&flux)
        .await
        .map_err(SlaPayloadError::Influx)?;

    // L'accès internet est une ligne du rapport au même titre qu'une cible :
    // un client qui lit « 99,9 % sur la passerelle » veut aussi savoir si le
    // lien vers l'extérieur tenait pendant ce temps-là.
    let internet_flux = format!(
        "from(bucket: {})\n  |> range({})\n  \
         |> filter(fn: (r) => r[\"probe_id\"] == {})\n  \
         |> filter(fn: (r) => r[\"_measurement\"] == \"internet_status\")\n  \
         |> filter(fn: (r) => r[\"_field\"] == \"state\" or r[\"_field\"] == \"icmp_ms\")",
        crate::web::flux_string(&bucket),
        window.clause,
        crate::web::flux_string(probe_id),
    );
    let internet = match state.influx.query_flux(&internet_flux).await {
        Ok(csv) => internet_samples_from_flux_csv(&csv),
        // Un rapport sans le volet internet vaut mieux que pas de rapport :
        // l'absence se verra à l'écran, elle ne fera pas échouer l'export.
        Err(e) => {
            tracing::warn!("volet internet du rapport SLA indisponible : {e}");
            Vec::new()
        }
    };

    let speedtests = state.db.speedtest_history(probe_id, 500).unwrap_or_default();
    // Les inventaires du rapport : ce que la sonde a vu sur le réseau, et les
    // ports ouverts qu'elle y a relevés. Un client qui reçoit un SLA veut
    // aussi savoir ce qui tournait chez lui.
    let discovery = state.db.latest_scan(probe_id, "discovery").ok().flatten();
    let ports = state.db.latest_scan(probe_id, "ports").ok().flatten();

    // L'historique des adresses voyage AVEC le rapport : le tableau de
    // l'interface et l'onglet Excel découpent les mêmes relevés avec les mêmes
    // intervalles. Deux sources séparées finiraient par diverger d'une minute
    // et donneraient deux disponibilités différentes pour la même période.
    //
    // ⚠️ Une fenêtre antérieure à la migration ne contient aucun intervalle :
    // le volet par adresse ressort alors entièrement en « indéterminé ». C'est
    // la vérité, pas une panne — à l'interface de le dire.
    let public_ip_history = state
        .db
        .public_ip_history(probe_id, window.from, window.to)
        .unwrap_or_default();

    Ok(json!({
        "probe": probe.name,
        "site": probe.site_name,
        "range": window.label,
        "start": asked_start,
        "stop": asked_stop,
        "generated_at": crate::db::now(),
        "targets": samples_from_flux_csv(&csv, (window.from, window.to)),
        "internet": internet,
        "speedtests": speedtests,
        "discovery": discovery,
        "ports": ports,
        "public_ip_history": public_ip_history,
    }))
}
