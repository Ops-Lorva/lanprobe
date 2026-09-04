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
    http::{header, StatusCode},
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?10, ?9)",
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
                ReportState::Preparing.as_str(),
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
            "UPDATE reports SET step = ?2 WHERE report_id = ?1 AND state = ?3",
            rusqlite::params![report_id, step, ReportState::Preparing.as_str()],
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
                SET state = ?7, step = NULL, error = NULL,
                    file_name = ?2, file_size = ?3, file_sha256 = ?4,
                    ready_at = ?5, expires_at = ?6
              WHERE report_id = ?1 AND state = ?8",
            rusqlite::params![
                report_id,
                file_name,
                file_size,
                file_sha256,
                ready_at,
                expires_at,
                ReportState::Ready.as_str(),
                ReportState::Preparing.as_str(),
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
                SET state = ?3, step = NULL, error = ?2
              WHERE report_id = ?1 AND state = ?4",
            rusqlite::params![
                report_id,
                cause,
                ReportState::Failed.as_str(),
                ReportState::Preparing.as_str(),
            ],
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
                  WHERE state = ?2 AND expires_at IS NOT NULL AND expires_at <= ?1",
            )?;
            let rows = stmt.query_map(
                rusqlite::params![now, ReportState::Ready.as_str()],
                |r| r.get(0),
            )?;
            rows.collect::<Result<_, _>>()?
        };
        // 🔴 `UPDATE`, jamais `DELETE`. La ligne — qui, quand, quelles sondes,
        // quelle fenêtre — survit à son fichier.
        tx.execute(
            "UPDATE reports
                SET state = ?2, purged_at = ?1,
                    file_name = NULL, file_size = NULL, file_sha256 = NULL
              WHERE state = ?3 AND expires_at IS NOT NULL AND expires_at <= ?1",
            rusqlite::params![
                now,
                ReportState::Purged.as_str(),
                ReportState::Ready.as_str(),
            ],
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
    pub fn fail_reports_interrupted_by_restart(&self) -> DbResult<usize> {
        let conn = self.conn().lock().map_err(|_| poisoned())?;
        let closes = conn.execute(
            "UPDATE reports
                SET state = ?2, step = NULL, error = ?1
              WHERE state = ?3",
            rusqlite::params![
                CAUSE_REDEMARRAGE,
                ReportState::Failed.as_str(),
                ReportState::Preparing.as_str(),
            ],
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
            post(request_report).get(list_site_reports),
        ),
    );

    let par_rapport = guarded(
        state,
        Role::Viewer,
        Router::new()
            .route("/api/reports/{id}", get(track_report))
            .route("/api/reports/{id}/file", get(download_report)),
    );

    par_site.merge(par_rapport)
}

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

// ── Demander un rapport ────────────────────────────────────────────────────

/// Ce que l'app envoie.
#[derive(serde::Deserialize)]
struct NewReport {
    /// ⚠️ **Absent = toutes les sondes du site.** Une liste VIDE, elle, n'est
    /// pas « toutes » : c'est une demande sans sonde, et elle est refusée. Les
    /// confondre produirait le classeur du site entier là où l'écran affichait
    /// zéro case cochée.
    #[serde(default)]
    probe_ids: Option<Vec<String>>,
    #[serde(default)]
    range: Option<String>,
    #[serde(default)]
    start: Option<i64>,
    #[serde(default)]
    stop: Option<i64>,
    // 🔴 **Pas de `locale` ici, et c'est le correctif.** La langue du classeur
    // est celle du SITE (`sites.report_locale`), pas celle du demandeur : le
    // document part chez un client, et deux opérateurs ne doivent pas lui
    // envoyer deux langues.
    //
    // ⚠️ L'app iOS envoie encore `locale: "fr"` en dur. Le champ reste
    // **accepté sur le fil** — serde ignore ce qu'il ne connaît pas — pour
    // qu'une app déjà installée continue d'obtenir ses rapports plutôt qu'un
    // `400`. Il n'est simplement plus écouté.
}

/// `202` : le hub a pris la demande, il n'a pas encore le fichier.
///
/// ⚠️ La garde de portée du chemin a déjà refusé un site inconnu ou hors
/// portée. `probe_ids` n'est donc plus une garde ici mais un contrôle
/// d'**appartenance** : une sonde d'un autre site dans la liste rendrait un
/// classeur à deux clients, ce que le générateur ne sait de toute façon pas
/// produire — il lit le site du premier relevé et suppose qu'il vaut pour tous.
async fn request_report(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(site_id): Path<String>,
    Json(body): Json<NewReport>,
) -> Response {
    let window = match crate::web::window_of(body.range, body.start, body.stop) {
        Ok(w) => w,
        Err(refus) => return refus,
    };

    let probe_ids = match body.probe_ids {
        Some(demandées) => {
            for probe_id in &demandées {
                // ⚠️ Même code et même message que la garde de portée : une
                // sonde d'un autre client doit rester indiscernable d'une sonde
                // qui n'existe pas.
                match state.db.get_probe(probe_id) {
                    Ok(p) if p.site_id == site_id => {}
                    _ => return fail(StatusCode::NOT_FOUND, "sonde inconnue"),
                }
            }
            demandées
        }
        None => match state.db.list_probes_scoped(Some(&site_id), &actor.scope, false) {
            Ok(sondes) => sondes.into_iter().map(|p| p.probe_id).collect(),
            Err(e) => return crate::web::error_response(e),
        },
    };
    if probe_ids.is_empty() {
        // Un classeur d'une feuille vide est « techniquement produit » et sans
        // contenu : sur un document qui part chez un client, c'est pire qu'un
        // refus.
        return fail(
            StatusCode::BAD_REQUEST,
            "aucune sonde à mettre dans ce rapport",
        );
    }

    let demande = ReportRequest {
        site_id: site_id.clone(),
        probe_ids,
        // ⚠️ Les bornes sont **résolues** ici, jamais gardées sous forme
        // glissante : « les 7 derniers jours » relus à la génération, puis à
        // une régénération, donneraient trois chiffres différents pour le même
        // document.
        range_start: window.from,
        range_stop: window.to,
        // 🔴 La langue du CLIENT, résolue ici et figée dans la ligne. Le site
        // du chemin est garanti existant par la garde de portée ; une lecture
        // en échec ou une valeur illisible retombent sur le défaut plutôt que
        // de refuser un rapport pour un réglage.
        locale: crate::report_i18n::resolve_locale(
            state
                .db
                .get_site(&site_id)
                .ok()
                .and_then(|s| s.report_locale)
                .as_deref(),
        )
        .to_string(),
        requested_by: actor.username.clone(),
        device_id: actor.device_id.clone(),
    };
    let report_id = match state.db.create_report(&demande) {
        Ok(id) => id,
        Err(e) => return crate::web::error_response(e),
    };

    // Première ligne : la demande, avec son PÉRIMÈTRE. La seconde viendra au
    // verdict — elles se rejoignent par le `report_id`.
    crate::web::audit(
        &state,
        Some(&actor.username),
        "report.request",
        Some(&report_id),
        Outcome::Success,
        Some(&format!(
            "site {} · {} · {} sonde(s){}",
            demande.site_id,
            window.label,
            demande.probe_ids.len(),
            match &actor.device_id {
                Some(d) => format!(" · depuis l'appareil {d}"),
                None => String::new(),
            }
        )),
    );

    lancer_preparation(state.clone(), report_id.clone());

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "report_id": report_id,
            "state": ReportState::Preparing.as_str(),
        })),
    )
        .into_response()
}

/// L'ouvrier : **une génération à la fois pour tout le hub**.
///
/// 🔴 Trente sondes cochées, c'est trente requêtes Flux et trente séries en
/// mémoire ; deux techniciens qui demandent en même temps en font soixante. Le
/// navigateur sérialisait déjà pour cette raison, et un hub auto-hébergé tourne
/// sur la machine dont on dispose, pas sur celle qu'on aurait choisie.
///
/// ⚠️ Le jeton est **global au processus** et non porté par `AppState` : deux
/// hubs dans un même processus n'existent qu'en test, et deux files séparées ne
/// protégeraient plus la mémoire, qui est commune.
fn ouvrier() -> &'static tokio::sync::Semaphore {
    static OUVRIER: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    OUVRIER.get_or_init(|| tokio::sync::Semaphore::new(1))
}

fn lancer_preparation(state: AppState, report_id: String) {
    tokio::spawn(async move {
        // Le sémaphore n'est jamais fermé : l'attente ne peut pas échouer.
        let _place = ouvrier().acquire().await;
        let issue = prepare(&state, &report_id).await;
        let (outcome, detail) = match &issue {
            Ok(()) => (Outcome::Success, None),
            Err(cause) => {
                // La cause **nommée** part dans la ligne avant le journal :
                // c'est elle que l'app affiche.
                if let Err(e) = state.db.set_report_failed(&report_id, cause) {
                    tracing::error!("échec du rapport {report_id} non consigné : {e}");
                }
                (Outcome::Failure, Some(cause.clone()))
            }
        };
        // La seconde ligne d'audit : le verdict.
        crate::web::audit(
            &state,
            None,
            "report.request",
            Some(&report_id),
            outcome,
            detail.as_deref(),
        );
    });
}

/// Produit le classeur d'un rapport ouvert. `Err` porte la cause **nommée**.
async fn prepare(state: &AppState, report_id: &str) -> Result<(), String> {
    let record = state.db.get_report(report_id).map_err(|e| e.to_string())?;
    let window = crate::web::window_of(None, Some(record.range_start), Some(record.range_stop))
        .map_err(|_| "la fenêtre du rapport est illisible".to_string())?;

    let total = record.probe_ids.len();
    let mut payloads = Vec::with_capacity(total);
    for (rang, probe_id) in record.probe_ids.iter().enumerate() {
        // ⚠️ Le nom de la sonde, pas son identifiant : l'étape s'affiche à
        // l'écran d'un technicien, pas dans un journal.
        let nom = state
            .db
            .get_probe(probe_id)
            .map(|p| p.name)
            .map_err(|_| format!("la sonde {probe_id} n'existe plus"))?;
        if let Err(e) = state
            .db
            .set_report_step(report_id, &format!("sonde {} sur {total} — {nom}", rang + 1))
        {
            tracing::warn!("étape du rapport {report_id} non écrite : {e}");
        }
        let payload = crate::sla_payload::build(
            state,
            probe_id,
            &window,
            Some(record.range_start),
            Some(record.range_stop),
        )
        .await
        .map_err(|e| format!("relevés de {nom} indisponibles : {e}"))?;
        payloads.push(payload);
    }

    let workbook = crate::report_xlsx::Workbook {
        site_name: record.site_name.clone(),
        range_start: record.range_start,
        range_stop: record.range_stop,
        payloads,
    };
    let locale = record.locale.clone();
    // ⚠️ Le classeur se construit **hors du réacteur** : quelques centaines de
    // milliers de cellules et un PNG, c'est du calcul pur, et il tiendrait un
    // fil d'exécution qui doit répondre aux autres requêtes.
    let built = tokio::task::spawn_blocking(move || {
        let catalog = crate::report_i18n::Catalog::load(&locale);
        crate::report_xlsx::build(&workbook, &catalog)
    })
    .await
    .map_err(|e| format!("la préparation s'est arrêtée : {e}"))??;

    let chemin = report_path(&state.config_dir, report_id);
    if let Some(parent) = chemin.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("le dossier des rapports n'a pas pu être créé : {e}"))?;
    }
    tokio::fs::write(&chemin, &built.bytes)
        .await
        .map_err(|e| format!("le classeur n'a pas pu être écrit : {e}"))?;

    let ready_at = crate::db::now();
    // ⚠️ L'échéance est fixée ICI et ne bougera plus, même si le réglage
    // change : l'app va afficher « disponible jusqu'à… », et une date annoncée
    // est tenue.
    let expires_at = ready_at + state.settings.report_days() * 86_400;
    state
        .db
        .set_report_ready(
            report_id,
            &built.file_name,
            built.bytes.len() as i64,
            &built.sha256,
            ready_at,
            expires_at,
        )
        .map_err(|e| e.to_string())
}

// ── Lire ───────────────────────────────────────────────────────────────────

async fn list_site_reports(State(state): State<AppState>, Path(site_id): Path<String>) -> Response {
    match state.db.list_reports(&site_id) {
        Ok(reports) => crate::web::ok_json(serde_json::json!({ "reports": reports })),
        Err(e) => crate::web::error_response(e),
    }
}

/// Le rapport, si l'appelant a le droit de le voir.
///
/// 🔴 **Le contrôle est écrit à la main parce que le chemin porte un
/// `report_id`, pas un `site_id`** — la garde de portée ne saurait pas quoi
/// chercher. C'est le seul motif légitime, et il vaut pour TOUS les comptes :
/// un rapport inexistant et un rapport hors portée rendent le même 404.
fn report_in_scope(state: &AppState, actor: &Identity, report_id: &str) -> Result<ReportRecord, Response> {
    match state.db.get_report(report_id) {
        Ok(r) if actor.scope.allows(&r.site_id) => Ok(r),
        _ => Err(fail(StatusCode::NOT_FOUND, "rapport inconnu")),
    }
}

async fn track_report(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(report_id): Path<String>,
) -> Response {
    match report_in_scope(&state, &actor, &report_id) {
        Ok(record) => crate::web::ok_json(serde_json::json!(record)),
        Err(refus) => refus,
    }
}

// ── Le fichier ─────────────────────────────────────────────────────────────

/// Type MIME d'un classeur. Écrit en toutes lettres : `mime_guess` rendrait le
/// même, mais sur le fichier remis à un client on ne devine pas.
const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// Sert le classeur, **d'un bloc**.
///
/// 🔴 **Le corps est lu en entier avant d'être servi**, et sa longueur est
/// annoncée. Une réponse construite en flux part volontiers en
/// `Transfer-Encoding: chunked` : il n'y a alors aucune taille à comparer, et
/// la garantie contre un fichier tronqué tombe en silence. Un classeur pèse
/// quelques centaines de kilo-octets.
async fn download_report(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(report_id): Path<String>,
) -> Response {
    let record = match report_in_scope(&state, &actor, &report_id) {
        Ok(r) => r,
        Err(refus) => return refus,
    };

    // ⚠️ **410 et non 404.** L'entrée existe — l'app vient d'en afficher la
    // ligne — et c'est le fichier qui n'est plus là. La date de purge dit
    // pourquoi, et l'écran propose alors « Redemander ».
    if record.state == ReportState::Purged.as_str() {
        return (
            StatusCode::GONE,
            Json(serde_json::json!({
                "error": "le fichier de ce rapport a été purgé",
                "state": record.state,
                "purged_at": record.purged_at,
            })),
        )
            .into_response();
    }
    if record.state != ReportState::Ready.as_str() {
        // Ni un fichier vide, ni un classeur partiel : l'état, tel quel, avec
        // sa cause nommée quand il y en a une.
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "ce rapport n'a pas de fichier",
                "state": record.state,
                "step": record.step,
                "detail": record.error,
            })),
        )
            .into_response();
    }

    let chemin = report_path(&state.config_dir, &record.report_id);
    let octets = match tokio::fs::read(&chemin).await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("classeur {report_id} illisible sur le volume : {e}");
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "le fichier de ce rapport est introuvable sur le volume du hub",
            );
        }
    };

    // 🔴 L'empreinte est recalculée **sur les octets qu'on s'apprête à
    // servir**, et comparée à celle de la production. C'est le seul contrôle
    // qui attrape une corruption silencieuse du volume — un classeur tronqué
    // s'ouvre très bien, avec moins de lignes.
    let empreinte: String = ring::digest::digest(&ring::digest::SHA256, &octets)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if record.file_sha256.as_deref() != Some(empreinte.as_str())
        || record.file_size != Some(octets.len() as i64)
    {
        tracing::error!("classeur {report_id} : les octets ne correspondent plus à son empreinte");
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "le fichier de ce rapport ne correspond plus à son empreinte — il n'a pas été servi",
        );
    }

    if let Err(e) = state.db.record_report_download(&record.report_id) {
        tracing::warn!("envoi du rapport {report_id} non compté : {e}");
    }
    // ⚠️ « le hub a servi le fichier à X », pas « X a téléchargé » : une
    // connexion qui meurt à 80 % laisse une réponse commencée côté serveur et
    // rien d'exploitable côté client.
    crate::web::audit(
        &state,
        Some(&actor.username),
        "report.download",
        Some(&record.report_id),
        Outcome::Success,
        Some(&format!(
            "le hub a servi le fichier à {} ({} octets)",
            actor.username,
            octets.len()
        )),
    );

    let nom = record.file_name.unwrap_or_else(|| format!("{report_id}.xlsx"));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, XLSX.to_string()),
            // ⚠️ Annoncée explicitement, jamais laissée à la couche de
            // transport : c'est le nombre que l'app compare aux octets reçus.
            (header::CONTENT_LENGTH, octets.len().to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{nom}\""),
            ),
            (EMPREINTE, empreinte),
        ],
        octets,
    )
        .into_response()
}

/// L'empreinte du corps, à comparer côté app. Le message d'échec y dit **les
/// deux nombres** : « attendu 248 013 o · reçu 91 220 o ».
const EMPREINTE: axum::http::HeaderName =
    axum::http::HeaderName::from_static("x-lanprobe-sha256");

// ── La purge ───────────────────────────────────────────────────────────────

/// Date les entrées échues, puis retire leurs classeurs du volume. Rend le
/// nombre d'entrées touchées.
///
/// 🔴 **L'ordre compte.** L'entrée est datée d'abord, dans une transaction ; le
/// fichier part ensuite. Un hub arrêté entre les deux laisse une entrée purgée
/// et un fichier orphelin — un octet de trop sur le volume. L'ordre inverse
/// aurait laissé une entrée qui annonce « disponible » et un fichier absent,
/// c'est-à-dire un rapport qu'on propose de télécharger et qui rend une erreur.
///
/// ⚠️ Un fichier déjà absent n'arrête pas le balayage : un volume restauré
/// depuis une archive antérieure aux classeurs n'en a aucun, et les entrées
/// doivent quand même être datées.
pub async fn purge_report_files(db: &Db, config_dir: &std::path::Path, now: i64) -> usize {
    let purgés = match db.purge_expired_reports(now) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("purge des rapports : {e}");
            return 0;
        }
    };
    for record in &purgés {
        let chemin = report_path(config_dir, &record.report_id);
        match tokio::fs::remove_file(&chemin).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("classeur {} non retiré du volume : {e}", record.report_id),
        }
    }
    purgés.len()
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

    #[tokio::test]
    async fn the_sweep_takes_the_file_off_the_volume_and_leaves_the_line() {
        // Les deux durées de vie, vues depuis la boucle des six heures.
        let (db, site_id) = db_with_site();
        let volume = std::env::temp_dir().join(format!(
            "lanprobe-purge-{}-{}",
            std::process::id(),
            crate::db::now()
        ));
        let id = db.create_report(&demande(&site_id)).unwrap();
        let chemin = report_path(&volume, &id);
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, b"un classeur").unwrap();
        db.set_report_ready(&id, "f.xlsx", 11, "abc", 5_000, 6_000).unwrap();

        assert_eq!(purge_report_files(&db, &volume, 6_001).await, 1);

        assert!(!chemin.exists(), "le fichier est resté sur le volume");
        let gardé = db.get_report(&id).unwrap();
        assert_eq!(gardé.state, "purged");
        assert_eq!(gardé.requested_by, "admin");

        // Un second passage ne trouve plus rien à faire — et surtout ne
        // redemande pas l'effacement d'un fichier déjà parti.
        assert_eq!(purge_report_files(&db, &volume, 9_999).await, 0);
        let _ = std::fs::remove_dir_all(&volume);
    }

    #[tokio::test]
    async fn a_file_already_gone_does_not_stop_the_sweep() {
        // Un volume restauré depuis une archive antérieure n'a pas les
        // classeurs : la purge doit quand même dater les entrées.
        let (db, site_id) = db_with_site();
        let volume = std::env::temp_dir().join("lanprobe-purge-volume-absent");
        let id = db.create_report(&demande(&site_id)).unwrap();
        db.set_report_ready(&id, "f.xlsx", 11, "abc", 5_000, 6_000).unwrap();

        assert_eq!(purge_report_files(&db, &volume, 6_001).await, 1);
        assert_eq!(db.get_report(&id).unwrap().purged_at, Some(6_001));
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

#[cfg(test)]
mod routes_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Request};
    use tower::ServiceExt;

    /// Un Influx qui répond « aucune mesure ».
    ///
    /// ⚠️ Un rapport sans relevé **est** un rapport : le classeur se produit,
    /// avec ses feuilles vides. Ce qu'on veut tenir ici n'est pas la richesse
    /// du contenu mais la chaîne complète — demande, préparation, fichier,
    /// empreinte.
    async fn faux_influx() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::<()>::new().fallback(|| async { "" });
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    /// Volume jetable, propre à chaque test : les rapports s'écrivent
    /// réellement sur disque.
    fn scratch() -> std::path::PathBuf {
        static COMPTEUR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COMPTEUR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("lanprobe-reports-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct Harness {
        state: AppState,
        router: axum::Router,
    }

    impl Harness {
        async fn new() -> Self {
            use std::sync::Arc;
            let db = Arc::new(Db::open_in_memory().unwrap());
            db.create_initial_admin("admin", "mot-de-passe-de-test").unwrap();
            let settings = crate::settings::Settings::new(db.clone());
            settings
                .put(crate::settings::keys::INFLUX_URL, &faux_influx().await, false)
                .unwrap();
            let secrets = crate::secrets::Secrets::ephemeral(db.clone()).unwrap();
            let state = AppState {
                auth: Arc::new(crate::auth::Auth::new(db.clone())),
                influx: Arc::new(crate::influx::Influx::new(
                    settings.clone(),
                    "jeton-operateur-de-test".into(),
                )),
                notifier: crate::notify::Notifier::new(db.clone(), secrets.clone(), settings.clone()),
                ceremonies: Arc::new(crate::passkeys::Ceremonies::new()),
                db,
                settings,
                secrets,
                tls: false,
                cert_fingerprint: None,
                restart_required: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                config_dir: scratch(),
                backup_dir: scratch(),
                influx_cli: "influx-absent-des-tests".into(),
            };
            let router = crate::web::build_router(state.clone());
            Self { state, router }
        }

        fn session(&self, username: &str) -> String {
            let token = self.state.auth.start_session(username.to_string()).unwrap();
            format!("lanprobe_hub_session={token}")
        }

        async fn brut(&self, req: Request<Body>) -> axum::response::Response {
            self.router.clone().oneshot(req).await.unwrap()
        }

        async fn get(&self, path: &str, cookie: &str) -> (StatusCode, serde_json::Value) {
            let r = self
                .brut(
                    Request::builder()
                        .uri(path)
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
            let status = r.status();
            let bytes = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
            (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
        }

        async fn post(
            &self,
            path: &str,
            cookie: &str,
            body: serde_json::Value,
        ) -> (StatusCode, serde_json::Value) {
            let r = self
                .brut(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header(header::COOKIE, cookie)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await;
            let status = r.status();
            let bytes = axum::body::to_bytes(r.into_body(), 1 << 22).await.unwrap();
            (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
        }

        /// Un site, une sonde enrôlée.
        fn site_avec_sonde(&self, site: &str, sonde: &str) -> (String, String) {
            let site_id = self.state.db.create_site(site).unwrap().site_id;
            let (probe, _) = self.state.db.enroll_probe(&site_id, sonde).unwrap();
            (site_id, probe.probe_id)
        }

        /// Attend la fin de la préparation et rend l'entrée de suivi.
        async fn attendre(&self, cookie: &str, report_id: &str) -> serde_json::Value {
            for _ in 0..200 {
                let (status, body) = self.get(&format!("/api/reports/{report_id}"), cookie).await;
                assert_eq!(status, StatusCode::OK, "{body}");
                if body["state"] != "preparing" {
                    return body;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            panic!("le rapport {report_id} est resté en préparation");
        }
    }

    fn fenetre() -> (i64, i64) {
        let stop = crate::db::now();
        (stop - 3_600, stop)
    }

    #[tokio::test]
    async fn a_report_is_served_with_its_length_and_the_fingerprint_of_the_bytes_sent() {
        // 🔴 Un fichier tronqué s'ouvre très bien. La longueur annoncée et
        // l'empreinte sont les deux seuls contrôles qui l'attrapent.
        let h = Harness::new().await;
        let cookie = h.session("admin");
        let (site_id, _) = h.site_avec_sonde("Durand", "Lyon");
        let (start, stop) = fenetre();

        let (status, ouvert) = h
            .post(
                &format!("/api/sites/{site_id}/reports"),
                &cookie,
                serde_json::json!({ "start": start, "stop": stop, "locale": "fr" }),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{ouvert}");
        assert_eq!(ouvert["state"], "preparing");
        let report_id = ouvert["report_id"].as_str().unwrap().to_string();

        let suivi = h.attendre(&cookie, &report_id).await;
        assert_eq!(suivi["state"], "ready", "{suivi}");

        let réponse = h
            .brut(
                Request::builder()
                    .uri(format!("/api/reports/{report_id}/file"))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(réponse.status(), StatusCode::OK);
        let entêtes = réponse.headers().clone();
        let octets = axum::body::to_bytes(réponse.into_body(), 1 << 24).await.unwrap();

        // La longueur est ANNONCÉE, sur une réponse non chunkée : sans elle, il
        // n'y a rien à comparer côté app.
        assert_eq!(
            entêtes.get(header::CONTENT_LENGTH).unwrap().to_str().unwrap(),
            octets.len().to_string(),
        );
        assert!(entêtes.get(header::TRANSFER_ENCODING).is_none());
        // Un classeur, pas un flux d'octets anonyme : c'est ce que le
        // téléphone regarde pour l'ouvrir dans le bon lecteur.
        assert_eq!(
            entêtes.get(header::CONTENT_TYPE).unwrap().to_str().unwrap(),
            XLSX
        );
        // L'empreinte porte sur les octets SERVIS.
        let attendu: String = ring::digest::digest(&ring::digest::SHA256, &octets)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            entêtes.get("x-lanprobe-sha256").unwrap().to_str().unwrap(),
            attendu
        );
        // Et elle est la même que celle gardée en base.
        assert_eq!(suivi["file_sha256"].as_str().unwrap(), attendu);
        assert_eq!(suivi["file_size"].as_i64().unwrap(), octets.len() as i64);
        // Le nom remis au client, pas l'identifiant technique.
        let disposition = entêtes
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(disposition.contains("durand"), "{disposition}");
        assert!(!disposition.contains(&report_id), "{disposition}");
        // Le hub sait qu'il a SERVI le fichier.
        assert_eq!(
            h.state.db.get_report(&report_id).unwrap().downloads,
            1
        );
    }

    #[tokio::test]
    async fn a_purged_report_answers_410_with_its_purge_date_and_keeps_its_line() {
        // ⚠️ 404 dirait « ça n'a jamais existé », sur un rapport dont l'app
        // vient d'afficher la ligne.
        let h = Harness::new().await;
        let cookie = h.session("admin");
        let (site_id, _) = h.site_avec_sonde("Durand", "Lyon");
        let (start, stop) = fenetre();

        let (_, ouvert) = h
            .post(
                &format!("/api/sites/{site_id}/reports"),
                &cookie,
                serde_json::json!({ "start": start, "stop": stop, "locale": "fr" }),
            )
            .await;
        let report_id = ouvert["report_id"].as_str().unwrap().to_string();
        h.attendre(&cookie, &report_id).await;

        let purgés = h.state.db.purge_expired_reports(i64::MAX).unwrap();
        assert_eq!(purgés.len(), 1);
        let _ = std::fs::remove_file(report_path(&h.state.config_dir, &report_id));

        let (status, body) = h.get(&format!("/api/reports/{report_id}/file"), &cookie).await;
        assert_eq!(status, StatusCode::GONE, "{body}");
        assert_eq!(body["state"], "purged");
        assert!(body["purged_at"].is_i64(), "{body}");

        // 🔴 L'entrée survit, avec son demandeur : c'est la réponse à « qui a
        // extrait les données de Durand ».
        let (status, liste) = h.get(&format!("/api/sites/{site_id}/reports"), &cookie).await;
        assert_eq!(status, StatusCode::OK);
        let entrée = &liste["reports"][0];
        assert_eq!(entrée["report_id"], report_id.as_str());
        assert_eq!(entrée["requested_by"], "admin");
        assert_eq!(entrée["state"], "purged");
        assert_eq!(entrée["range_start"], start);
        assert_eq!(entrée["range_stop"], stop);
        assert!(entrée["probe_ids"].as_array().unwrap().len() == 1);
    }

    #[tokio::test]
    async fn a_file_that_no_longer_matches_its_fingerprint_is_never_served() {
        // Une corruption silencieuse sur le volume du hub ne doit pas partir
        // chez le client : c'est la valeur plausible et fausse à l'endroit où
        // elle coûte le plus cher.
        let h = Harness::new().await;
        let cookie = h.session("admin");
        let (site_id, _) = h.site_avec_sonde("Durand", "Lyon");
        let (start, stop) = fenetre();
        let (_, ouvert) = h
            .post(
                &format!("/api/sites/{site_id}/reports"),
                &cookie,
                serde_json::json!({ "start": start, "stop": stop, "locale": "fr" }),
            )
            .await;
        let report_id = ouvert["report_id"].as_str().unwrap().to_string();
        h.attendre(&cookie, &report_id).await;

        // Le fichier est tronqué sur le volume, comme le ferait un disque plein.
        let chemin = report_path(&h.state.config_dir, &report_id);
        let octets = std::fs::read(&chemin).unwrap();
        std::fs::write(&chemin, &octets[..octets.len() / 2]).unwrap();

        let (status, body) = h.get(&format!("/api/reports/{report_id}/file"), &cookie).await;
        assert_ne!(status, StatusCode::OK, "un classeur tronqué a été servi");
        assert!(
            body["error"].as_str().unwrap_or_default().contains("empreinte"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn an_account_that_sees_every_site_cannot_mix_two_clients_in_one_report() {
        // 🔴 Le test qui manquait trois fois : écrit avec un compte
        // `Scope::All`, pour qui les gardes de portée ne refusent rien. Ici la
        // seule chose qui protège est le contrôle d'APPARTENANCE de la sonde.
        let h = Harness::new().await;
        let cookie = h.session("admin");
        assert_eq!(
            h.state.db.user_scope("admin", crate::db::Role::Admin).unwrap(),
            crate::db::Scope::All
        );
        let (durand, _) = h.site_avec_sonde("Durand", "Lyon");
        let (_, sonde_martin) = h.site_avec_sonde("Martin", "Paris");
        let (start, stop) = fenetre();

        let (status, body) = h
            .post(
                &format!("/api/sites/{durand}/reports"),
                &cookie,
                serde_json::json!({
                    "probe_ids": [sonde_martin],
                    "start": start, "stop": stop, "locale": "fr",
                }),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert!(h.state.db.list_reports(&durand).unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unknown_report_is_not_found_even_for_an_account_that_sees_everything() {
        // Même motif : l'existence se vérifie pour TOUS les comptes.
        let h = Harness::new().await;
        let cookie = h.session("admin");

        let (status, _) = h.get("/api/reports/rapport-qui-nexiste-pas", &cookie).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = h.get("/api/reports/rapport-qui-nexiste-pas/file", &cookie).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = h
            .post(
                "/api/sites/site-qui-nexiste-pas/reports",
                &cookie,
                serde_json::json!({ "locale": "fr" }),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_restricted_account_never_learns_that_another_clients_report_exists() {
        // Le nom d'un client est déjà une information (contrat § 17).
        let h = Harness::new().await;
        let admin = h.session("admin");
        let (durand, _) = h.site_avec_sonde("Durand", "Lyon");
        let (martin, _) = h.site_avec_sonde("Martin", "Paris");
        h.state
            .db
            .create_user("lecteur", "mot-de-passe-de-test", crate::db::Role::Viewer)
            .unwrap();
        h.state.db.set_user_sites("lecteur", &[durand.clone()]).unwrap();
        let lecteur = h.session("lecteur");

        let (start, stop) = fenetre();
        let (_, ouvert) = h
            .post(
                &format!("/api/sites/{martin}/reports"),
                &admin,
                serde_json::json!({ "start": start, "stop": stop, "locale": "fr" }),
            )
            .await;
        let report_id = ouvert["report_id"].as_str().unwrap().to_string();

        // Même code, même message que pour un rapport inexistant.
        let (status, body) = h.get(&format!("/api/reports/{report_id}"), &lecteur).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        let (status, _) = h.get(&format!("/api/reports/{report_id}/file"), &lecteur).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = h.get(&format!("/api/sites/{martin}/reports"), &lecteur).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn omitting_the_probes_takes_every_probe_of_the_site_and_only_those() {
        let h = Harness::new().await;
        let cookie = h.session("admin");
        let (durand, lyon) = h.site_avec_sonde("Durand", "Lyon");
        let (_, marseille) = {
            let (probe, _) = h.state.db.enroll_probe(&durand, "Marseille").unwrap();
            (durand.clone(), probe.probe_id)
        };
        h.site_avec_sonde("Martin", "Paris");
        let (start, stop) = fenetre();

        let (status, ouvert) = h
            .post(
                &format!("/api/sites/{durand}/reports"),
                &cookie,
                serde_json::json!({ "start": start, "stop": stop, "locale": "fr" }),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{ouvert}");
        let record = h
            .state
            .db
            .get_report(ouvert["report_id"].as_str().unwrap())
            .unwrap();
        let mut retenues = record.probe_ids.clone();
        retenues.sort();
        let mut attendues = vec![lyon, marseille];
        attendues.sort();
        assert_eq!(retenues, attendues);
    }

    #[tokio::test]
    async fn a_site_without_probes_says_so_rather_than_producing_an_empty_workbook() {
        // Un document remis au client ne doit jamais être « techniquement
        // produit » et sans contenu.
        let h = Harness::new().await;
        let cookie = h.session("admin");
        let site_id = h.state.db.create_site("Vide").unwrap().site_id;
        let (start, stop) = fenetre();

        let (status, body) = h
            .post(
                &format!("/api/sites/{site_id}/reports"),
                &cookie,
                serde_json::json!({ "start": start, "stop": stop, "locale": "fr" }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body["error"].as_str().unwrap_or_default().contains("sonde"), "{body}");
    }

    #[tokio::test]
    async fn la_langue_du_rapport_est_celle_du_site_pas_celle_du_demandeur() {
        // 🔴 Le classeur part chez UN client. Sa langue est une propriété du
        // client, pas de l'employé qui appuie sur le bouton : c'est ce qui
        // garantit qu'un même client reçoive TOUJOURS ses rapports dans la
        // même langue, quel que soit l'opérateur.
        //
        // ⚠️ Le corps porte encore `locale` — l'app iOS l'envoie en dur. Il
        // est reçu et **ignoré** : c'est le site qui décide.
        let h = Harness::new().await;
        let cookie = h.session("admin");
        let (site_id, _) = h.site_avec_sonde("Durand", "Lyon");
        h.state
            .db
            .set_site_report_locale(&site_id, Some("es"))
            .unwrap();
        let (start, stop) = fenetre();

        let (status, ouvert) = h
            .post(
                &format!("/api/sites/{site_id}/reports"),
                &cookie,
                serde_json::json!({ "start": start, "stop": stop, "locale": "fr" }),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{ouvert}");
        let report_id = ouvert["report_id"].as_str().unwrap().to_string();
        assert_eq!(h.state.db.get_report(&report_id).unwrap().locale, "es");
    }

    #[tokio::test]
    async fn un_site_sans_langue_choisie_garde_le_defaut_du_hub() {
        // ⚠️ Ne pas casser les hubs existants : aucun site n'a de langue le
        // jour de la migration, et tous doivent continuer à produire ce
        // qu'ils produisaient.
        let h = Harness::new().await;
        let cookie = h.session("admin");
        let (site_id, _) = h.site_avec_sonde("Durand", "Lyon");
        let (start, stop) = fenetre();

        let (_, ouvert) = h
            .post(
                &format!("/api/sites/{site_id}/reports"),
                &cookie,
                serde_json::json!({ "start": start, "stop": stop, "locale": "es" }),
            )
            .await;
        let report_id = ouvert["report_id"].as_str().unwrap().to_string();
        assert_eq!(
            h.state.db.get_report(&report_id).unwrap().locale,
            crate::report_i18n::FALLBACK_LOCALE
        );
    }

    #[tokio::test]
    async fn une_langue_illisible_en_base_ne_fait_pas_echouer_le_rapport() {
        // 🔴 La colonne est libre : une valeur écrite à la main, ou une langue
        // retirée du produit, retombe sur le défaut. Un rapport qui échoue
        // parce qu'un réglage est illisible est le pire des deux mondes.
        let h = Harness::new().await;
        let cookie = h.session("admin");
        let (site_id, _) = h.site_avec_sonde("Durand", "Lyon");
        h.state
            .db
            .set_site_report_locale(&site_id, Some("klingon"))
            .unwrap();
        let (start, stop) = fenetre();

        let (status, ouvert) = h
            .post(
                &format!("/api/sites/{site_id}/reports"),
                &cookie,
                serde_json::json!({ "start": start, "stop": stop }),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{ouvert}");
        let report_id = ouvert["report_id"].as_str().unwrap().to_string();
        assert_eq!(
            h.state.db.get_report(&report_id).unwrap().locale,
            crate::report_i18n::FALLBACK_LOCALE
        );

        // Et il va au bout : le repli n'est pas seulement écrit en base.
        let suivi = h.attendre(&cookie, &report_id).await;
        assert_eq!(suivi["state"], "ready", "{suivi}");
    }

    #[tokio::test]
    async fn an_unreadable_window_is_a_bad_request_not_a_report_that_fails_later() {
        let h = Harness::new().await;
        let cookie = h.session("admin");
        let (site_id, _) = h.site_avec_sonde("Durand", "Lyon");

        let (status, _) = h
            .post(
                &format!("/api/sites/{site_id}/reports"),
                &cookie,
                serde_json::json!({ "range": "la semaine dernière", "locale": "fr" }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(h.state.db.list_reports(&site_id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn asking_and_serving_are_two_audit_lines() {
        // Demander et télécharger ne sont pas le même acte, ni forcément la
        // même personne.
        let h = Harness::new().await;
        let cookie = h.session("admin");
        let (site_id, _) = h.site_avec_sonde("Durand", "Lyon");
        let (start, stop) = fenetre();

        let (_, ouvert) = h
            .post(
                &format!("/api/sites/{site_id}/reports"),
                &cookie,
                serde_json::json!({ "start": start, "stop": stop, "locale": "fr" }),
            )
            .await;
        let report_id = ouvert["report_id"].as_str().unwrap().to_string();
        h.attendre(&cookie, &report_id).await;
        let (status, _) = h.get(&format!("/api/reports/{report_id}/file"), &cookie).await;
        assert_eq!(status, StatusCode::OK);

        let (status, journal) = h.get("/api/audit?action=report.request", &cookie).await;
        assert_eq!(status, StatusCode::OK, "{journal}");
        let demandes = journal["entries"].as_array().unwrap();
        // Une ligne à la demande, une au verdict.
        assert_eq!(demandes.len(), 2, "{journal}");
        assert!(demandes.iter().all(|l| l["target"] == report_id.as_str()));

        let (_, journal) = h.get("/api/audit?action=report.download", &cookie).await;
        assert_eq!(journal["entries"].as_array().unwrap().len(), 1, "{journal}");
    }
}
