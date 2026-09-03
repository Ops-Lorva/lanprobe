//! Rapports produits par le hub : demande, suivi, fichier, purge (contrat § 23).
//!
//! 🔴 **Un rapport gardé est une trace, pas un fichier.** Deux durées de vie
//! dans une seule ligne : le fichier s'en va à la purge, l'entrée reste, avec
//! son demandeur et sa date. Purger « les rapports » du dimanche ne doit pas
//! emporter la réponse à « qui a extrait les données de Durand le 12 août ».
//!
//! ⚠️ **Aucune route ne supprime un rapport**, et l'app n'a pas de bouton
//! « supprimer ». Qui peut effacer un rapport peut effacer la trace de son
//! extraction — la raison même qui interdit de nettoyer le journal d'audit.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};

use rusqlite::OptionalExtension;

use crate::db::{Db, DbError, DbResult, Outcome, Role};
use crate::web::{guarded, guarded_site_scope, AppState, Identity};

// ⚠️ La rétention des **fichiers** se lit dans les réglages
// (`crate::settings::keys::REPORT_DAYS`, défaut 7, plancher 1) et n'est PAS
// redéclarée ici : deux constantes pour le même délai finiraient par annoncer à
// l'écran une échéance que la purge ne tient pas.

/// Où vivent les fichiers, sous le volume du hub.
///
/// ⚠️ **Jamais servi par la racine web.** L'identifiant est un UUID et la route
/// est gardée : un répertoire statique donnerait le classeur d'un autre client
/// à qui devine une URL.
pub(crate) const REPORTS_DIR: &str = "reports";

/// Le classeur sur le disque, nommé d'après le **report_id** et non d'après le
/// nom remis au client.
///
/// ⚠️ Deux rapports du même site sur la même période portent le même
/// `file_name` — c'est voulu, il part chez le client — et s'écraseraient l'un
/// l'autre sur le volume. Le second aurait alors une empreinte qui ne
/// correspond plus à ses octets : le contrôle censé attraper une corruption
/// l'aurait inventée.
pub(crate) fn report_path(config_dir: &std::path::Path, report_id: &str) -> std::path::PathBuf {
    config_dir.join(REPORTS_DIR).join(format!("{report_id}.xlsx"))
}

/// Les colonnes de la table, dans l'ordre que lit [`report_from_row`].
const REPORT_COLUMNS: &str = "r.report_id, r.site_id, s.name, r.requested_by, r.device_id, \
     r.probe_ids, r.locale, r.range_start, r.range_stop, r.state, r.step, r.error, \
     r.file_name, r.file_size, r.file_sha256, r.requested_at, r.ready_at, r.expires_at, \
     r.purged_at, r.downloads";

fn report_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReportRecord> {
    Ok(ReportRecord {
        report_id: r.get(0)?,
        site_id: r.get(1)?,
        site_name: r.get(2)?,
        requested_by: r.get(3)?,
        device_id: r.get(4)?,
        // ⚠️ Une liste illisible rend un rapport **vide**, jamais une erreur de
        // lecture : l'entrée d'historique doit rester consultable même si sa
        // colonne JSON a été abîmée. C'est la trace qu'on protège.
        probe_ids: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
        locale: r.get(6)?,
        range_start: r.get(7)?,
        range_stop: r.get(8)?,
        state: r.get(9)?,
        step: r.get(10)?,
        error: r.get(11)?,
        file_name: r.get(12)?,
        file_size: r.get(13)?,
        file_sha256: r.get(14)?,
        requested_at: r.get(15)?,
        ready_at: r.get(16)?,
        expires_at: r.get(17)?,
        purged_at: r.get(18)?,
        downloads: r.get(19)?,
    })
}

fn poisoned() -> DbError {
    DbError::Internal("verrou SQLite empoisonné".into())
}

/// La cause **nommée** d'un rapport que le redémarrage du hub a interrompu.
///
/// 🔴 Elle est écrite en toutes lettres dans la ligne, pas déduite à
/// l'affichage : l'app la montre telle quelle, et « échec » ne dit pas s'il
/// faut réessayer ou appeler.
pub(crate) const CAUSE_REDEMARRAGE: &str = "le hub a redémarré pendant la préparation";

/// L'état d'un rapport, tel que l'écran le lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportState {
    Preparing,
    Ready,
    Failed,
    /// Le fichier n'est plus là ; l'entrée, si.
    Purged,
}

impl ReportState {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            ReportState::Preparing => "preparing",
            ReportState::Ready => "ready",
            ReportState::Failed => "failed",
            ReportState::Purged => "purged",
        }
    }
}

/// Une ligne de la table `reports`, telle que la liste et le suivi la rendent.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ReportRecord {
    pub report_id: String,
    pub site_id: String,
    pub site_name: String,
    pub requested_by: String,
    /// ⚠️ `None` = demandé depuis le hub web, pas « inconnu ». « Qui a
    /// demandé » désigne une personne ; l'appareil restreint la réponse le jour
    /// où un téléphone est entre d'autres mains.
    pub device_id: Option<String>,
    pub probe_ids: Vec<String>,
    /// La langue figée à la demande — celle de l'opérateur.
    pub locale: String,
    pub range_start: i64,
    pub range_stop: i64,
    pub state: String,
    /// « sonde 2 sur 3 — Lyon ». Ce que l'app affiche pendant la préparation.
    pub step: Option<String>,
    /// La cause **nommée**, jamais « erreur » : c'est elle qui dit s'il faut
    /// réessayer ou appeler.
    pub error: Option<String>,
    pub file_name: Option<String>,
    pub file_size: Option<i64>,
    pub file_sha256: Option<String>,
    pub requested_at: i64,
    pub ready_at: Option<i64>,
    /// ⚠️ Fixée à la préparation et **ne bouge plus**, même si `report_days`
    /// change ensuite : l'app a affiché « disponible jusqu'à demain 9:44 », et
    /// une date annoncée est tenue.
    pub expires_at: Option<i64>,
    pub purged_at: Option<i64>,
    pub downloads: i64,
}

/// Ce qu'une demande porte.
///
/// ⚠️ **Un site, une fenêtre, un sous-ensemble de ses sondes.** `probe_ids`
/// absent = **toutes les sondes du site dans la portée de l'appelant**, jamais
/// toutes celles du site.
pub(crate) struct ReportRequest {
    pub site_id: String,
    pub probe_ids: Vec<String>,
    pub range_start: i64,
    pub range_stop: i64,
    pub locale: String,
    pub requested_by: String,
    pub device_id: Option<String>,
}

/// Le stockage des rapports, hors de `db.rs`.
impl Db {
    /// Ouvre une demande en `preparing` et rend son identifiant.
    pub(crate) fn create_report(&self, request: &ReportRequest) -> DbResult<String> {
        // UUIDv4 : l'identifiant sert d'URL de téléchargement. Un compteur
        // donnerait le rapport du voisin à qui incrémente.
        let report_id = uuid::Uuid::new_v4().to_string();
        let probe_ids = serde_json::to_string(&request.probe_ids)
            .map_err(|e| DbError::Internal(e.to_string()))?;
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        conn.execute(
            "INSERT INTO reports
               (report_id, site_id, requested_by, device_id, probe_ids, locale,
                range_start, range_stop, state, requested_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'preparing', ?9)",
            rusqlite::params![
                report_id,
                request.site_id,
                request.requested_by,
                request.device_id,
                probe_ids,
                request.locale,
                request.range_start,
                request.range_stop,
                crate::db::now(),
            ],
        )?;
        Ok(report_id)
    }

    /// Avance le libellé d'étape pendant la préparation.
    ///
    /// ⚠️ `WHERE state = 'preparing'` : un rapport déjà clos ne rouvre pas. Une
    /// étape écrite après coup afficherait « sonde 2 sur 3 » sur un rapport
    /// prêt.
    pub(crate) fn set_report_step(&self, report_id: &str, step: &str) -> DbResult<()> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        conn.execute(
            "UPDATE reports SET step = ?2 WHERE report_id = ?1 AND state = 'preparing'",
            rusqlite::params![report_id, step],
        )?;
        Ok(())
    }

    /// Clôt une préparation réussie : fichier, taille, empreinte, échéance.
    pub(crate) fn set_report_ready(
        &self,
        report_id: &str,
        file_name: &str,
        file_size: i64,
        file_sha256: &str,
        ready_at: i64,
        expires_at: i64,
    ) -> DbResult<()> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        let touchées = conn.execute(
            "UPDATE reports
                SET state = 'ready', step = NULL, error = NULL,
                    file_name = ?2, file_size = ?3, file_sha256 = ?4,
                    ready_at = ?5, expires_at = ?6
              WHERE report_id = ?1 AND state = 'preparing'",
            rusqlite::params![
                report_id,
                file_name,
                file_size,
                file_sha256,
                ready_at,
                expires_at
            ],
        )?;
        if touchées == 0 {
            return Err(DbError::NotFound("rapport inconnu".into()));
        }
        Ok(())
    }

    /// Clôt une préparation en échec, avec sa cause **nommée**.
    pub(crate) fn set_report_failed(&self, report_id: &str, cause: &str) -> DbResult<()> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        conn.execute(
            "UPDATE reports
                SET state = 'failed', step = NULL, error = ?2
              WHERE report_id = ?1 AND state = 'preparing'",
            rusqlite::params![report_id, cause],
        )?;
        Ok(())
    }

    /// L'historique d'un site, du plus récent au plus ancien.
    pub(crate) fn list_reports(&self, site_id: &str) -> DbResult<Vec<ReportRecord>> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {REPORT_COLUMNS}
               FROM reports r JOIN sites s ON s.site_id = r.site_id
              WHERE r.site_id = ?1
              ORDER BY r.requested_at DESC, r.rowid DESC"
        ))?;
        let rows = stmt.query_map(rusqlite::params![site_id], report_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub(crate) fn get_report(&self, report_id: &str) -> DbResult<ReportRecord> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        conn.query_row(
            &format!(
                "SELECT {REPORT_COLUMNS}
                   FROM reports r JOIN sites s ON s.site_id = r.site_id
                  WHERE r.report_id = ?1"
            ),
            rusqlite::params![report_id],
            report_from_row,
        )
        .optional()?
        .ok_or_else(|| DbError::NotFound("rapport inconnu".into()))
    }

    /// Incrémente le compteur d'envois.
    ///
    /// ⚠️ Le hub sait qu'il a **servi** le fichier ; il ne sait pas que le
    /// téléphone l'a reçu. Le libellé d'audit est « le hub a servi le fichier à
    /// X », pas « X a téléchargé ».
    pub(crate) fn record_report_download(&self, report_id: &str) -> DbResult<()> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        conn.execute(
            "UPDATE reports SET downloads = downloads + 1 WHERE report_id = ?1",
            rusqlite::params![report_id],
        )?;
        Ok(())
    }

    /// Purge les **fichiers** échus et rend les entrées touchées.
    ///
    /// 🔴 Ne supprime **aucune ligne** : `purged_at` daté, `file_name`,
    /// `file_size` et `file_sha256` vidés, `state = 'purged'`. Un rapport purgé
    /// reste visible dans la liste, à sa date, avec son demandeur.
    ///
    /// ⚠️ Un rapport **en préparation** n'est jamais purgé : il n'a pas encore
    /// de date de fin.
    pub(crate) fn purge_expired_reports(&self, now: i64) -> DbResult<Vec<ReportRecord>> {
        let mut conn = self.conn().lock().map_err(|_| poisoned())?;
        let tx = conn.transaction()?;
        // ⚠️ `state = 'ready'` et non « échue » : un rapport déjà purgé ne se
        // repurge pas — il reviendrait à chaque passage de la boucle, et
        // l'appelant redemanderait l'effacement d'un fichier absent toutes les
        // six heures. Un rapport en préparation n'a, lui, pas de date de fin.
        let ids: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT report_id FROM reports
                  WHERE state = 'ready' AND expires_at IS NOT NULL AND expires_at <= ?1",
            )?;
            let rows = stmt.query_map(rusqlite::params![now], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        // 🔴 `UPDATE`, jamais `DELETE`. La ligne — qui, quand, quelles sondes,
        // quelle fenêtre — survit à son fichier.
        tx.execute(
            "UPDATE reports
                SET state = 'purged', purged_at = ?1,
                    file_name = NULL, file_size = NULL, file_sha256 = NULL
              WHERE state = 'ready' AND expires_at IS NOT NULL AND expires_at <= ?1",
            rusqlite::params![now],
        )?;
        let mut purgés = Vec::with_capacity(ids.len());
        {
            let mut stmt = tx.prepare(&format!(
                "SELECT {REPORT_COLUMNS}
                   FROM reports r JOIN sites s ON s.site_id = r.site_id
                  WHERE r.report_id = ?1"
            ))?;
            for id in &ids {
                purgés.push(stmt.query_row(rusqlite::params![id], report_from_row)?);
            }
        }
        tx.commit()?;
        Ok(purgés)
    }

    /// Clôt en échec ce qui était « en préparation » au démarrage du hub.
    ///
    /// 🔴 Sans elle, un redémarrage laisse une demande éternellement en cours
    /// et l'app affiche une progression qui n'avance plus — une valeur
    /// plausible et fausse. Cause nommée : « le hub a redémarré pendant la
    /// préparation ». **Rien ne reprend tout seul** : une préparation reprise à
    /// moitié produirait un classeur partiel.
    ///
    /// ⚠️ Sans horodatage : la table n'a pas de colonne pour la date d'échec, et
    /// écrire l'instant du redémarrage dans `ready_at` ferait passer un échec
    /// pour un classeur produit.
    pub(crate) fn fail_reports_interrupted_by_restart(&self) -> DbResult<usize> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        let closes = conn.execute(
            "UPDATE reports
                SET state = 'failed', step = NULL, error = ?1
              WHERE state = 'preparing'",
            rusqlite::params![CAUSE_REDEMARRAGE],
        )?;
        Ok(closes)
    }
}

/// Les quatre routes du § 23.
///
/// 🔴 **L'historique est une route DE SITE**, pas un `GET /api/reports?site=`.
/// Un paramètre de requête aurait obligé le handler à vérifier lui-même sa
/// portée — la forme dont le § 17 dit qu'elle finit par être oubliée. Ici le
/// site est dans le chemin, donc `guarded_site_scope` s'applique **avant** le
/// handler, sans que personne ait à y penser.
///
/// ⚠️ `GET /api/reports/{id}` et `/file` portent un `report_id`, pas un
/// `site_id` : leur portée se vérifie sur le site **du rapport**, dans le
/// handler. C'est le seul motif légitime d'un contrôle écrit à la main.
pub(crate) fn routes(state: &AppState) -> Router<AppState> {
    let par_site = guarded_site_scope(
        state,
        Role::Viewer,
        Router::new().route(
            "/api/sites/{id}/reports",
            post(not_implemented).get(not_implemented),
        ),
    );

    let par_rapport = guarded(
        state,
        Role::Viewer,
        Router::new()
            .route("/api/reports/{id}", get(not_implemented))
            .route("/api/reports/{id}/file", get(not_implemented)),
    );

    par_site.merge(par_rapport)
}

/// ⚠️ `501` et non `404` : une route annoncée mais pas encore écrite ne doit
/// pas laisser croire à un client mal versionné qu'il s'est trompé de chemin.
async fn not_implemented() -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::NOT_IMPLEMENTED,
        axum::Json(serde_json::json!({ "error": "rapports non encore implémentés" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un hub installé, un compte admin, un site. Le stockage des rapports ne
    /// touche ni Influx ni le routeur.
    fn db_with_site() -> (Db, String) {
        let db = Db::open_in_memory().unwrap();
        db.create_initial_admin("admin", "mot-de-passe-de-test").unwrap();
        let site = db.create_site("Durand").unwrap();
        (db, site.site_id)
    }

    fn demande(site_id: &str) -> ReportRequest {
        ReportRequest {
            site_id: site_id.to_string(),
            probe_ids: vec!["sonde-lyon".into(), "sonde-paris".into()],
            range_start: 1_000,
            range_stop: 2_000,
            locale: "fr".into(),
            requested_by: "admin".into(),
            device_id: None,
        }
    }

    #[test]
    fn purging_keeps_the_row_and_who_asked_for_it() {
        // 🔴 L'exigence du § 23 : purger « les rapports » du dimanche ne doit
        // pas emporter la réponse à « qui a extrait les données de Durand ».
        let (db, site_id) = db_with_site();
        let id = db.create_report(&demande(&site_id)).unwrap();
        db.set_report_ready(&id, "lanprobe-sla-durand.xlsx", 248_013, "abc", 5_000, 6_000)
            .unwrap();

        let purgés = db.purge_expired_reports(6_001).unwrap();
        assert_eq!(purgés.len(), 1);

        let gardé = db.get_report(&id).unwrap();
        assert_eq!(gardé.state, "purged");
        assert_eq!(gardé.requested_by, "admin");
        assert_eq!(gardé.site_name, "Durand");
        assert_eq!(gardé.probe_ids, vec!["sonde-lyon", "sonde-paris"]);
        assert_eq!((gardé.range_start, gardé.range_stop), (1_000, 2_000));
        assert_eq!(gardé.purged_at, Some(6_001));
        // Le fichier, lui, s'en va.
        assert_eq!(gardé.file_name, None);
        assert_eq!(gardé.file_size, None);
        assert_eq!(gardé.file_sha256, None);
    }

    #[test]
    fn an_already_purged_report_is_not_purged_twice() {
        // Sinon la boucle de six heures redemanderait l'effacement d'un fichier
        // absent à chaque passage, et le journal annoncerait une purge sans fin.
        let (db, site_id) = db_with_site();
        let id = db.create_report(&demande(&site_id)).unwrap();
        db.set_report_ready(&id, "f.xlsx", 10, "abc", 5_000, 6_000).unwrap();

        assert_eq!(db.purge_expired_reports(6_001).unwrap().len(), 1);
        assert!(db.purge_expired_reports(9_999).unwrap().is_empty());
        assert_eq!(db.get_report(&id).unwrap().purged_at, Some(6_001));
    }

    #[test]
    fn a_report_still_being_prepared_is_never_purged() {
        // Il n'a pas encore de date de fin : rien à comparer.
        let (db, site_id) = db_with_site();
        let id = db.create_report(&demande(&site_id)).unwrap();

        assert!(db.purge_expired_reports(i64::MAX).unwrap().is_empty());
        assert_eq!(db.get_report(&id).unwrap().state, "preparing");
    }

    #[test]
    fn a_file_still_within_its_announced_deadline_stays() {
        // « Disponible jusqu'à demain 9:44 » : une date annoncée est tenue.
        let (db, site_id) = db_with_site();
        let id = db.create_report(&demande(&site_id)).unwrap();
        db.set_report_ready(&id, "f.xlsx", 10, "abc", 5_000, 6_000).unwrap();

        assert!(db.purge_expired_reports(5_999).unwrap().is_empty());
        assert_eq!(db.get_report(&id).unwrap().state, "ready");
    }

    #[test]
    fn a_restart_leaves_no_report_waiting_forever() {
        // 🔴 Sans ça, l'app affiche une progression qui n'avance plus.
        let (db, site_id) = db_with_site();
        let interrompu = db.create_report(&demande(&site_id)).unwrap();
        db.set_report_step(&interrompu, "sonde 2 sur 3 — Lyon").unwrap();
        let prêt = db.create_report(&demande(&site_id)).unwrap();
        db.set_report_ready(&prêt, "f.xlsx", 10, "abc", 5_000, 6_000).unwrap();

        assert_eq!(db.fail_reports_interrupted_by_restart().unwrap(), 1);

        let mort = db.get_report(&interrompu).unwrap();
        assert_eq!(mort.state, "failed");
        // La cause est NOMMÉE : elle dit s'il faut réessayer ou appeler.
        assert_eq!(mort.error.as_deref(), Some(CAUSE_REDEMARRAGE));
        // L'étape figée ne survit pas : « sonde 2 sur 3 » sur un rapport mort
        // est exactement la progression qui n'avance plus.
        assert_eq!(mort.step, None);
        // Un rapport prêt n'est pas touché.
        assert_eq!(db.get_report(&prêt).unwrap().state, "ready");
    }

    #[test]
    fn nothing_resumes_a_report_on_its_own() {
        // Une préparation reprise à moitié produirait un classeur partiel : un
        // rapport clos en échec ne redevient jamais « en préparation ».
        let (db, site_id) = db_with_site();
        let id = db.create_report(&demande(&site_id)).unwrap();
        db.fail_reports_interrupted_by_restart().unwrap();

        db.set_report_step(&id, "sonde 1 sur 2").unwrap();
        assert!(db
            .set_report_ready(&id, "f.xlsx", 10, "abc", 5_000, 6_000)
            .is_err());

        let mort = db.get_report(&id).unwrap();
        assert_eq!(mort.state, "failed");
        assert_eq!(mort.step, None);
    }

    #[test]
    fn the_history_of_a_site_holds_only_its_own_reports_newest_first() {
        let (db, durand) = db_with_site();
        let martin = db.create_site("Martin").unwrap().site_id;
        let vieux = db.create_report(&demande(&durand)).unwrap();
        let neuf = db.create_report(&demande(&durand)).unwrap();
        db.create_report(&demande(&martin)).unwrap();

        let liste = db.list_reports(&durand).unwrap();
        let ids: Vec<&str> = liste.iter().map(|r| r.report_id.as_str()).collect();
        assert_eq!(ids, vec![neuf.as_str(), vieux.as_str()]);
    }

    #[test]
    fn serving_the_file_is_counted() {
        // Le hub sait qu'il a SERVI le fichier, pas que le téléphone l'a reçu.
        let (db, site_id) = db_with_site();
        let id = db.create_report(&demande(&site_id)).unwrap();
        db.set_report_ready(&id, "f.xlsx", 10, "abc", 5_000, 6_000).unwrap();

        assert_eq!(db.get_report(&id).unwrap().downloads, 0);
        db.record_report_download(&id).unwrap();
        db.record_report_download(&id).unwrap();
        assert_eq!(db.get_report(&id).unwrap().downloads, 2);
    }

    #[test]
    fn an_unknown_report_is_not_found() {
        let (db, _) = db_with_site();
        assert!(db.get_report("rapport-qui-nexiste-pas").is_err());
    }

    #[test]
    fn the_file_on_disk_is_named_after_the_report_not_after_the_client() {
        // Deux rapports du même site sur la même période portent le même nom
        // remis au client : sur le volume, ils ne doivent pas s'écraser.
        let a = report_path(std::path::Path::new("/vol"), "aaa");
        let b = report_path(std::path::Path::new("/vol"), "bbb");
        assert_ne!(a, b);
        assert_eq!(a, std::path::Path::new("/vol/reports/aaa.xlsx"));
    }
}
