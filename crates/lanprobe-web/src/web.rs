//! Routes HTTP du hub.
//!
//! Trois formes d'authentification cohabitent, parce que trois interlocuteurs
//! différents frappent à la porte :
//!
//! - **session** (cookie) pour le navigateur ;
//! - **identifiants du compte** (Basic, ou dans le corps de l'enrôlement) pour
//!   l'usage scripté ;
//! - **jeton de sonde** (`Bearer`) pour le battement de cœur.
//!
//! Le jeton opérateur Influx n'apparaît dans aucune de ces routes : le
//! navigateur ne parle jamais directement à Influx, le hub proxifie.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use cookie::{Cookie, SameSite};
use serde::Deserialize;
use serde_json::json;

use crate::auth::Auth;
use crate::db::{Db, DbError, DbResult};
use crate::influx::Influx;
use crate::probe::ProbeStatus;
use crate::settings::Settings;

const SESSION_COOKIE: &str = "lanprobe_hub_session";

/// Durée de vie d'un code d'enrôlement. Assez pour aller brancher la sonde,
/// trop court pour qu'un code oublié sur un tableau blanc serve encore.
const ENROLL_CODE_TTL_SECS: i64 = 15 * 60;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub auth: Arc<Auth>,
    pub settings: Settings,
    pub influx: Arc<Influx>,
}

pub fn build_router(state: AppState) -> Router {
    // Routes ouvertes : elles portent leur propre authentification (token de
    // setup, code d'enrôlement, identifiants, jeton de sonde).
    let public = Router::new()
        .route("/api/status", get(status))
        .route("/api/setup", post(setup))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/probes/enroll", post(enroll))
        .route("/api/probes/{id}/heartbeat", post(heartbeat));

    // `GET /api/sites` accepte aussi les identifiants du compte : la sonde
    // l'appelle avant l'enrôlement, quand elle n'a pas encore de session.
    let sites_read = Router::new()
        .route("/api/sites", get(list_sites))
        .layer(middleware::from_fn_with_state(state.clone(), session_or_credentials));

    let protected = Router::new()
        .route("/api/sites", post(create_site))
        .route("/api/sites/{id}", patch(rename_site).delete(delete_site))
        .route("/api/enroll-codes", post(create_enroll_code))
        .route("/api/probes", get(list_probes))
        .route("/api/probes/{id}", patch(update_probe).delete(revoke_probe))
        .route("/api/probes/{id}/metrics", get(probe_metrics))
        .route("/api/probes/{id}/rotate", post(rotate_token))
        .route("/api/probes/{id}/revoke-token", post(revoke_token_now))
        .route("/api/probes/{id}/reenroll-code", post(create_reenroll_code))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route(
            "/api/settings/influx-advertise",
            get(get_advertise).put(put_advertise),
        )
        .route("/api/settings/influx-advertise/test", post(test_advertise))
        .route("/api/influx", get(influx_overview))
        .route("/api/influx/read-token", post(create_read_token))
        .route("/api/influx/read-tokens", get(list_read_tokens))
        .route("/api/influx/read-tokens/{id}", delete(revoke_read_token))
        .layer(middleware::from_fn_with_state(state.clone(), session_guard));

    Router::new()
        .merge(public)
        .merge(sites_read)
        .merge(protected)
        .with_state(state)
}

// ── Erreurs ────────────────────────────────────────────────────────────────

/// Traduit une erreur du stockage en code HTTP. Les handlers ne devinent pas
/// le code d'après le texte du message — ils se tromperaient.
fn error_response(e: DbError) -> Response {
    let status = match e {
        DbError::Conflict(_) => StatusCode::CONFLICT,
        DbError::NotFound(_) => StatusCode::NOT_FOUND,
        DbError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
        DbError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    fail(status, &e.to_string())
}

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

fn ok_json(value: serde_json::Value) -> Response {
    (StatusCode::OK, Json(value)).into_response()
}

// ── Statut, setup, session ─────────────────────────────────────────────────

async fn status(State(state): State<AppState>) -> Response {
    ok_json(json!({
        "needs_setup": state.auth.needs_setup(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Deserialize)]
struct SetupBody {
    #[serde(default)]
    setup_token: String,
    username: String,
    password: String,
}

async fn setup(State(state): State<AppState>, Json(body): Json<SetupBody>) -> Response {
    if body.username.trim().is_empty() || body.password.len() < 8 {
        return fail(
            StatusCode::BAD_REQUEST,
            "nom d'utilisateur requis, mot de passe d'au moins 8 caractères",
        );
    }
    match state
        .auth
        .setup(&body.setup_token, body.username.trim(), &body.password)
    {
        Ok(()) => ok_json(json!({ "ok": true })),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

async fn login(State(state): State<AppState>, Json(body): Json<Credentials>) -> Response {
    match state.auth.login(&body.username, &body.password) {
        Ok(token) => (
            [(header::SET_COOKIE, session_cookie(&token, false))],
            Json(json!({ "ok": true })),
        )
            .into_response(),
        Err(e) => error_response(e),
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = session_of(&headers) {
        state.auth.logout(&token);
    }
    (
        [(header::SET_COOKIE, session_cookie("", true))],
        Json(json!({ "ok": true })),
    )
        .into_response()
}

fn session_cookie(token: &str, expired: bool) -> String {
    let mut builder = Cookie::build((SESSION_COOKIE, token.to_string()))
        .http_only(true)
        .same_site(SameSite::Strict)
        .path("/")
        .secure(true);
    if expired {
        builder = builder.max_age(cookie::time::Duration::seconds(0));
    }
    builder.build().to_string()
}

fn session_of(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|pair| Cookie::parse(pair.trim()).ok())
        .find(|c| c.name() == SESSION_COOKIE)
        .map(|c| c.value().to_string())
}

async fn session_guard(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    if state.auth.needs_setup() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "hub non configuré", "needs_setup": true })),
        )
            .into_response();
    }
    match session_of(&headers).and_then(|t| state.auth.validate(&t)) {
        Some(_) => next.run(req).await,
        None => fail(StatusCode::UNAUTHORIZED, "session absente ou expirée"),
    }
}

/// Session **ou** identifiants du compte en Basic. La sonde n'a pas de session
/// avant son enrôlement : lui imposer d'en ouvrir une pour lire la liste des
/// sites l'obligerait à manipuler un cookie pour rien.
async fn session_or_credentials(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    if session_of(&headers)
        .and_then(|t| state.auth.validate(&t))
        .is_some()
    {
        return next.run(req).await;
    }
    if let Some((username, password)) = basic_credentials(&headers)
        && state.db.verify_credentials(&username, &password).is_ok()
    {
        return next.run(req).await;
    }
    fail(StatusCode::UNAUTHORIZED, "session ou identifiants requis")
}

fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    use base64::Engine;
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = raw.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (username, password) = decoded.split_once(':')?;
    Some((username.to_string(), password.to_string()))
}

// ── Sites ──────────────────────────────────────────────────────────────────

async fn list_sites(State(state): State<AppState>) -> Response {
    match state.db.list_sites() {
        Ok(sites) => ok_json(serde_json::to_value(sites).unwrap_or(serde_json::Value::Null)),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct SiteBody {
    name: String,
}

async fn create_site(State(state): State<AppState>, Json(body): Json<SiteBody>) -> Response {
    match state.db.create_site(&body.name) {
        Ok(site) => (
            StatusCode::CREATED,
            Json(serde_json::to_value(site).unwrap_or(serde_json::Value::Null)),
        )
            .into_response(),
        Err(e) => error_response(e),
    }
}

async fn rename_site(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SiteBody>,
) -> Response {
    match state.db.rename_site(&id, &body.name) {
        Ok(site) => ok_json(serde_json::to_value(site).unwrap_or(serde_json::Value::Null)),
        Err(e) => error_response(e),
    }
}

async fn delete_site(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.db.delete_site(&id) {
        Ok(()) => ok_json(json!({ "ok": true })),
        Err(e) => error_response(e),
    }
}

// ── Codes d'enrôlement ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct EnrollCodeBody {
    site_id: String,
}

async fn create_enroll_code(
    State(state): State<AppState>,
    Json(body): Json<EnrollCodeBody>,
) -> Response {
    let site = match state.db.get_site(&body.site_id) {
        Ok(site) => site,
        Err(e) => return error_response(e),
    };
    match state
        .db
        .create_enroll_code(&site.site_id, None, ENROLL_CODE_TTL_SECS)
    {
        Ok(code) => ok_json(json!({
            "code": code.code,
            "site": site.name,
            "site_id": site.site_id,
            "expires_at": code.expires_at,
        })),
        Err(e) => error_response(e),
    }
}

async fn create_reenroll_code(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let probe = match state.db.get_probe(&id) {
        Ok(probe) => probe,
        Err(e) => return error_response(e),
    };
    match state
        .db
        .create_enroll_code(&probe.site_id, Some(&probe.probe_id), ENROLL_CODE_TTL_SECS)
    {
        Ok(code) => ok_json(json!({
            "code": code.code,
            "probe_id": probe.probe_id,
            "name": probe.name,
            "site": probe.site_name,
            "expires_at": code.expires_at,
        })),
        Err(e) => error_response(e),
    }
}

// ── Enrôlement ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct EnrollBody {
    name: String,
    /// Chemin recommandé : un code court, à usage unique.
    #[serde(default)]
    code: Option<String>,
    /// Chemin scripté : identifiants du compte + nom de site.
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    site: Option<String>,
}

async fn enroll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<EnrollBody>,
) -> Response {
    // 1. Qui a le droit d'enrôler, et dans quel site.
    let (site, site_created, repairing) = match resolve_enrollment_target(&state, &body) {
        Ok(target) => target,
        Err(e) => return error_response(e),
    };

    // 2. La sonde : réparation d'une existante, ou création.
    let (probe, probe_token) = match repairing {
        Some(probe_id) => {
            // La machine compromise a gardé une copie de sa clé d'écriture :
            // sans destruction, elle continuerait d'écrire dans le bucket
            // après la réparation.
            if let Ok(previous) = state.db.get_probe(&probe_id)
                && let Some(auth_id) = previous.influx_auth_id
                && let Err(e) = state.influx.delete_authorization(&auth_id).await
            {
                tracing::warn!("autorisation Influx {auth_id} non retirée : {e}");
            }
            match state.db.reenroll_probe(&probe_id) {
                Ok(pair) => pair,
                Err(e) => return error_response(e),
            }
        }
        None => match state.db.enroll_probe(&site.site_id, &body.name) {
            Ok(pair) => pair,
            Err(e) => return error_response(e),
        },
    };

    // 3. Son jeton d'écriture Influx, frappé pour elle seule.
    let minted = match mint_probe_token(&state, &probe.probe_id, &probe.name).await {
        Ok(token) => token,
        Err(e) => return fail(StatusCode::SERVICE_UNAVAILABLE, &e),
    };

    let host = host_header(&headers);
    let influx = state.influx.probe_settings(&minted, host.as_deref());
    (
        StatusCode::CREATED,
        Json(json!({
            "probe_id": probe.probe_id,
            "name": probe.name,
            "site_id": site.site_id,
            "site": site.name,
            "site_created": site_created,
            "token": probe_token,
            "influx": influx,
            "heartbeat_interval_secs": state.settings.heartbeat_interval_secs(),
        })),
    )
        .into_response()
}

/// Rend `(site, site_créé, sonde_à_réparer)`.
fn resolve_enrollment_target(
    state: &AppState,
    body: &EnrollBody,
) -> DbResult<(crate::db::Site, bool, Option<String>)> {
    if let Some(code) = body.code.as_deref().filter(|c| !c.trim().is_empty()) {
        let claim = state.db.consume_enroll_code(code.trim())?;
        let site = state.db.get_site(&claim.site_id)?;
        return Ok((site, false, claim.probe_id));
    }
    let (Some(username), Some(password), Some(site_name)) = (
        body.username.as_deref(),
        body.password.as_deref(),
        body.site.as_deref(),
    ) else {
        return Err(DbError::Unauthorized(
            "code d'enrôlement ou identifiants du compte requis".into(),
        ));
    };
    state.db.verify_credentials(username, password)?;
    let (site, created) = state.db.resolve_or_create_site(site_name)?;
    Ok((site, created, None))
}

/// Frappe un jeton d'écriture pour une sonde et le rattache à sa ligne.
async fn mint_probe_token(
    state: &AppState,
    probe_id: &str,
    probe_name: &str,
) -> Result<String, String> {
    let minted = state
        .influx
        .mint_write_token(&format!("sonde {probe_name} ({probe_id})"))
        .await?;
    state
        .db
        .set_probe_influx_auth(probe_id, &minted.auth_id)
        .map_err(|e| e.to_string())?;
    Ok(minted.token)
}

fn host_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

// ── Battement de cœur ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct HeartbeatBody {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    buffered_points: i64,
    #[serde(default)]
    last_write_ok: Option<bool>,
    /// Version du jeton d'écriture que la sonde utilise réellement. C'est
    /// l'accusé de réception qui autorise le hub à retirer l'ancien.
    #[serde(default)]
    influx_token_version: Option<i64>,
}

async fn heartbeat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<HeartbeatBody>,
) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return fail(StatusCode::UNAUTHORIZED, "jeton de sonde requis");
    };
    let probe = match state.db.authenticate_probe(&id, &token) {
        Ok(probe) => probe,
        Err(e) => return error_response(e),
    };
    let _ = body.last_write_ok; // remonté par la sonde, exploité par l'interface

    if let Err(e) = state.db.record_heartbeat(
        &id,
        body.version.as_deref(),
        body.platform.as_deref(),
        body.buffered_points,
    ) {
        return error_response(e);
    }

    // Accusé de réception d'une rotation : c'est le seul moment où l'ancienne
    // autorisation peut être détruite sans risquer de couper la sonde.
    if let Some(acknowledged) = body.influx_token_version {
        match state.db.acknowledge_token_version(&id, acknowledged) {
            Ok(Some(retired)) => {
                if let Err(e) = state.influx.delete_authorization(&retired).await {
                    tracing::warn!("autorisation Influx {retired} non retirée : {e}");
                }
            }
            Ok(None) => {}
            Err(e) => return error_response(e),
        }
    }

    let mut response = json!({
        "ok": true,
        "name": probe.name,
        "site_id": probe.site_id,
        "site": probe.site_name,
        "heartbeat_interval_secs": state.settings.heartbeat_interval_secs(),
    });

    // Relecture : la rotation vient peut-être d'être accusée.
    let probe = match state.db.get_probe(&id) {
        Ok(probe) => probe,
        Err(e) => return error_response(e),
    };
    if let (Some(pending), Some(version)) = (
        probe.pending_influx_token.clone(),
        probe.pending_influx_token_version,
    ) {
        let host = host_header(&headers);
        let mut influx = serde_json::to_value(state.influx.probe_settings(&pending, host.as_deref()))
            .unwrap_or(serde_json::Value::Null);
        influx["token_version"] = json!(version);
        response["influx"] = influx;
    }
    ok_json(response)
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

// ── Sondes ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ProbeFilter {
    #[serde(default)]
    site: Option<String>,
}

async fn list_probes(State(state): State<AppState>, Query(filter): Query<ProbeFilter>) -> Response {
    let interval = state.settings.heartbeat_interval_secs();
    let now = crate::db::now();
    match state.db.list_probes(filter.site.as_deref()) {
        Ok(probes) => ok_json(serde_json::Value::Array(
            probes
                .into_iter()
                .map(|p| {
                    json!({
                        "probe_id": p.probe_id,
                        "name": p.name,
                        "site_id": p.site_id,
                        "site": p.site_name,
                        "platform": p.platform,
                        "version": p.version,
                        "last_seen": p.last_seen,
                        "status": ProbeStatus::derive(p.last_seen, now, interval),
                        "buffered_points": p.buffered_points,
                        "created_at": p.created_at,
                        "revoked_at": p.revoked_at,
                    })
                })
                .collect(),
        )),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct ProbePatch {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    site_id: Option<String>,
}

async fn update_probe(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ProbePatch>,
) -> Response {
    match state
        .db
        .update_probe(&id, body.name.as_deref(), body.site_id.as_deref())
    {
        Ok(probe) => ok_json(json!({
            "probe_id": probe.probe_id,
            "name": probe.name,
            "site_id": probe.site_id,
            "site": probe.site_name,
        })),
        Err(e) => error_response(e),
    }
}

/// Révoque le jeton d'une sonde. **Ne supprime aucune mesure** : la ligne
/// reste, les points Influx restent, seul l'accès est fermé.
async fn revoke_probe(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let probe = match state.db.get_probe(&id) {
        Ok(probe) => probe,
        Err(e) => return error_response(e),
    };
    if let Err(e) = state.db.revoke_probe(&id) {
        return error_response(e);
    }
    if let Some(auth_id) = probe.influx_auth_id
        && let Err(e) = state.influx.delete_authorization(&auth_id).await
    {
        tracing::warn!("autorisation Influx {auth_id} non retirée : {e}");
    }
    ok_json(json!({ "ok": true, "measurements_kept": true }))
}

/// Rotation d'hygiène : le nouveau jeton attend d'être remis au prochain
/// battement, l'ancien reste actif jusqu'à l'accusé.
async fn rotate_token(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let probe = match state.db.get_probe(&id) {
        Ok(probe) => probe,
        Err(e) => return error_response(e),
    };
    let token = match state
        .influx
        .mint_write_token(&format!("sonde {} ({}) — rotation", probe.name, probe.probe_id))
        .await
    {
        Ok(minted) => minted,
        Err(e) => return fail(StatusCode::SERVICE_UNAVAILABLE, &e),
    };
    match state.db.stage_token_rotation(&id, &token.auth_id, &token.token) {
        Ok(version) => ok_json(json!({
            "ok": true,
            "token_version": version,
            "pending_delivery": true,
        })),
        Err(e) => error_response(e),
    }
}

/// Compromission : l'ancienne autorisation est détruite tout de suite. Le
/// jeton **de sonde** n'est pas touché — c'est lui qui permet à la machine de
/// récupérer sa nouvelle clé sans que personne ne se déplace.
async fn revoke_token_now(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let probe = match state.db.get_probe(&id) {
        Ok(probe) => probe,
        Err(e) => return error_response(e),
    };
    let token = match state
        .influx
        .mint_write_token(&format!("sonde {} ({}) — après révocation", probe.name, probe.probe_id))
        .await
    {
        Ok(minted) => minted,
        Err(e) => return fail(StatusCode::SERVICE_UNAVAILABLE, &e),
    };
    let (retired, version) =
        match state.db.replace_token_immediately(&id, &token.auth_id, &token.token) {
        Ok(pair) => pair,
        Err(e) => return error_response(e),
    };
    if let Some(retired) = retired
        && let Err(e) = state.influx.delete_authorization(&retired).await
    {
        tracing::warn!("autorisation Influx {retired} non retirée : {e}");
    }
    ok_json(json!({
        "ok": true,
        "token_version": version,
        "revoked_immediately": true,
    }))
}

#[derive(Deserialize)]
struct MetricsQuery {
    #[serde(default)]
    measurement: Option<String>,
    #[serde(default)]
    range: Option<String>,
}

/// Requête Flux proxifiée. Le navigateur ne parle jamais directement à
/// Influx : le jeton de lecture reste côté serveur.
async fn probe_metrics(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> Response {
    if state.db.get_probe(&id).is_err() {
        return fail(StatusCode::NOT_FOUND, "sonde inconnue");
    }
    let range = query.range.unwrap_or_else(|| "-1h".into());
    let bucket = state.settings.influx_bucket();
    let mut flux = format!(
        "from(bucket: {})\n  |> range(start: {})\n  |> filter(fn: (r) => r[\"probe_id\"] == {})",
        flux_string(&bucket),
        flux_duration(&range),
        flux_string(&id)
    );
    if let Some(measurement) = query.measurement.filter(|m| !m.is_empty()) {
        flux.push_str(&format!(
            "\n  |> filter(fn: (r) => r[\"_measurement\"] == {})",
            flux_string(&measurement)
        ));
    }
    match state.influx.query_flux(&flux).await {
        Ok(csv) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/csv; charset=utf-8")],
            csv,
        )
            .into_response(),
        Err(e) => fail(StatusCode::BAD_GATEWAY, &e),
    }
}

/// Littéral de chaîne Flux échappé — les valeurs viennent de l'URL, elles ne
/// sont pas de confiance.
fn flux_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Une durée Flux n'est pas une chaîne : elle ne peut pas être mise entre
/// guillemets, donc on la restreint à un alphabet sûr au lieu de l'échapper.
fn flux_duration(value: &str) -> String {
    let safe: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '.')
        .collect();
    if safe.is_empty() { "-1h".into() } else { safe }
}

// ── Réglages ───────────────────────────────────────────────────────────────

async fn get_settings(State(state): State<AppState>) -> Response {
    ok_json(serde_json::to_value(state.settings.all()).unwrap_or(serde_json::Value::Null))
}

async fn put_settings(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Map<String, serde_json::Value>>,
) -> Response {
    // Drapeau de confirmation, pas un réglage : il ne doit pas finir en base.
    let confirm = body
        .get("confirm_data_loss")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut influx_touched = false;

    for (key, value) in body.iter() {
        if key == "confirm_data_loss" {
            continue;
        }
        let value = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if let Err(e) = state.settings.put(key, &value, confirm) {
            return error_response(e);
        }
        influx_touched |= key.starts_with("influx_") || key == crate::settings::keys::RETENTION_DAYS;
    }

    if influx_touched {
        // Le bucket ou l'URL ont pu changer : les identifiants déduits des
        // anciens réglages ne valent plus rien.
        state.influx.invalidate();
        let influx = state.influx.clone();
        let days = state.settings.retention_days();
        tokio::spawn(async move {
            if influx.ensure_provisioned().await.is_ok() {
                let _ = influx.ensure_tls_fingerprint().await;
                if let Err(e) = influx.apply_retention(days).await {
                    tracing::warn!("rétention non appliquée : {e}");
                }
            }
        });
    }
    ok_json(serde_json::to_value(state.settings.all()).unwrap_or(serde_json::Value::Null))
}

async fn get_advertise(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let (url, source) = state
        .influx
        .resolve_advertise_url(host_header(&headers).as_deref());
    ok_json(json!({
        "url": url,
        "source": source.as_str(),
        "configured": state.settings.stored_advertise_url(),
    }))
}

#[derive(Deserialize)]
struct AdvertiseBody {
    url: String,
}

async fn put_advertise(State(state): State<AppState>, Json(body): Json<AdvertiseBody>) -> Response {
    match state
        .settings
        .put(crate::settings::keys::INFLUX_ADVERTISE_URL, &body.url, false)
    {
        Ok(()) => ok_json(json!({ "ok": true, "url": body.url })),
        Err(e) => error_response(e),
    }
}

/// Vérifie que l'URL annoncée répond. Un échec est un **résultat de test**,
/// pas une panne du hub : on répond `200` avec `reachable: false`, sinon
/// l'interface ne saurait pas distinguer « URL fausse » de « hub cassé ».
async fn test_advertise(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let (url, source) = state
        .influx
        .resolve_advertise_url(host_header(&headers).as_deref());
    let reachable = state.influx.check_reachable(&url).await;
    ok_json(json!({ "url": url, "reachable": reachable, "source": source.as_str() }))
}

// ── Influx vu depuis les réglages ──────────────────────────────────────────

async fn influx_overview(State(state): State<AppState>) -> Response {
    // L'utilisateur ne doit jamais avoir à administrer Influx, mais il doit
    // pouvoir aller voir ses données. Aucun jeton dans cette réponse.
    let health = state.influx.health().await;
    ok_json(json!({
        "url": state.settings.influx_url(),
        "org": state.settings.influx_org(),
        "bucket": state.settings.influx_bucket(),
        "provisioned": state.influx.is_ready(),
        "tls_fingerprint_sha256": state.influx.tls_fingerprint(),
        "retention_days": state.settings.retention_days(),
        "health": health.status,
        "version": health.version,
        "bucket_bytes": health.bucket_bytes,
        "series_count": health.series_count,
    }))
}

#[derive(Deserialize)]
struct ReadTokenBody {
    #[serde(default)]
    description: String,
}

/// Jeton **en lecture seule** sur le bucket, à coller dans Grafana. Affiché
/// une seule fois : le hub n'en garde que l'identifiant, pour pouvoir le
/// révoquer.
async fn create_read_token(
    State(state): State<AppState>,
    Json(body): Json<ReadTokenBody>,
) -> Response {
    let description = if body.description.trim().is_empty() {
        "jeton de lecture".to_string()
    } else {
        body.description.trim().to_string()
    };
    let minted = match state.influx.mint_read_token(&description).await {
        Ok(minted) => minted,
        Err(e) => return fail(StatusCode::SERVICE_UNAVAILABLE, &e),
    };
    if let Err(e) = state.db.record_read_token(&minted.auth_id, &description) {
        return error_response(e);
    }
    (
        StatusCode::CREATED,
        Json(json!({
            "auth_id": minted.auth_id,
            "description": description,
            "token": minted.token,
        })),
    )
        .into_response()
}

async fn list_read_tokens(State(state): State<AppState>) -> Response {
    match state.db.list_read_tokens() {
        Ok(tokens) => ok_json(serde_json::to_value(tokens).unwrap_or(serde_json::Value::Null)),
        Err(e) => error_response(e),
    }
}

async fn revoke_read_token(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if let Err(e) = state.db.revoke_read_token(&id) {
        return error_response(e);
    }
    if let Err(e) = state.influx.delete_authorization(&id).await {
        tracing::warn!("autorisation Influx {id} non retirée : {e}");
    }
    ok_json(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::influx::testing::{FakeInflux, ENV_LOCK, OPERATOR_TOKEN};
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const PASSWORD: &str = "password123";

    struct Harness {
        state: AppState,
        router: axum::Router,
        _fake: FakeInflux,
    }

    impl Harness {
        /// Hub complet devant un faux Influx local, admin déjà créé.
        async fn start() -> Self {
            let fake = FakeInflux::start(true, true).await;
            let db = std::sync::Arc::new(crate::db::Db::open_in_memory().unwrap());
            let settings = crate::settings::Settings::new(db.clone());
            settings
                .put(crate::settings::keys::INFLUX_URL, &fake.base_url, false)
                .unwrap();
            let influx = std::sync::Arc::new(crate::influx::Influx::new(
                settings.clone(),
                OPERATOR_TOKEN.to_string(),
            ));
            influx.ensure_provisioned().await.unwrap();
            let auth = std::sync::Arc::new(crate::auth::Auth::new(db.clone()));
            let state = AppState {
                db,
                auth,
                settings,
                influx,
            };
            let router = build_router(state.clone());
            Self {
                state,
                router,
                _fake: fake,
            }
        }

        async fn with_admin() -> Self {
            let h = Self::start().await;
            h.state.db.create_initial_admin("admin", PASSWORD).unwrap();
            h
        }

        async fn call(&self, req: Request<Body>) -> (StatusCode, serde_json::Value, Vec<String>) {
            let response = self.router.clone().oneshot(req).await.unwrap();
            let status = response.status();
            let cookies = response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok().map(str::to_string))
                .collect();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
            (status, json, cookies)
        }

        async fn login(&self) -> String {
            let (status, _, cookies) = self
                .call(json_request(
                    "POST",
                    "/api/login",
                    serde_json::json!({ "username": "admin", "password": PASSWORD }),
                ))
                .await;
            assert_eq!(status, StatusCode::OK, "le login doit réussir");
            cookies
                .first()
                .and_then(|c| c.split(';').next())
                .expect("un cookie de session est attendu")
                .to_string()
        }

        /// Enrôle une sonde par code et rend `(probe_id, jeton de sonde)`.
        async fn enroll(&self, session: &str, site: &str, name: &str) -> (String, String) {
            let (_, site_json, _) = self
                .call(with_cookie(
                    json_request("POST", "/api/sites", serde_json::json!({ "name": site })),
                    session,
                ))
                .await;
            let site_id = site_json["site_id"].as_str().unwrap().to_string();

            let (_, code, _) = self
                .call(with_cookie(
                    json_request(
                        "POST",
                        "/api/enroll-codes",
                        serde_json::json!({ "site_id": site_id }),
                    ),
                    session,
                ))
                .await;
            let code = code["code"].as_str().unwrap().to_string();

            let (status, body, _) = self
                .call(json_request(
                    "POST",
                    "/api/probes/enroll",
                    serde_json::json!({ "code": code, "name": name }),
                ))
                .await;
            assert_eq!(status, StatusCode::CREATED, "enrôlement : {body}");
            (
                body["probe_id"].as_str().unwrap().to_string(),
                body["token"].as_str().unwrap().to_string(),
            )
        }
    }

    fn json_request(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::HOST, "hub.example.org:8443")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn empty_request(method: &str, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "hub.example.org:8443")
            .body(Body::empty())
            .unwrap()
    }

    fn with_cookie(mut req: Request<Body>, cookie: &str) -> Request<Body> {
        req.headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        req
    }

    fn with_bearer(mut req: Request<Body>, token: &str) -> Request<Body> {
        req.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        req
    }

    // ── Statut et premier lancement ────────────────────────────────────────

    #[tokio::test]
    async fn status_announces_the_setup_then_stops() {
        let h = Harness::start().await;
        let (status, body, _) = h.call(empty_request("GET", "/api/status")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["needs_setup"], true);
        assert!(body["version"].is_string(), "la version doit être annoncée");

        h.state.db.create_initial_admin("admin", PASSWORD).unwrap();
        let (_, body, _) = h.call(empty_request("GET", "/api/status")).await;
        assert_eq!(body["needs_setup"], false);
    }

    #[tokio::test]
    async fn setup_needs_the_token_and_does_not_replay() {
        let h = Harness::start().await;
        let dir = std::env::temp_dir().join(format!("lanprobe-web-setup-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let token = h
            .state
            .auth
            .init_setup_token(&dir.join("setup-token"))
            .unwrap()
            .unwrap();

        let (status, body, _) = h
            .call(json_request(
                "POST",
                "/api/setup",
                serde_json::json!({ "setup_token": "faux", "username": "admin", "password": PASSWORD }),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body["error"].is_string(), "forme d'erreur du contrat : {body}");

        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/setup",
                serde_json::json!({ "setup_token": token, "username": "admin", "password": PASSWORD }),
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        // Non rejouable, même avec le bon token.
        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/setup",
                serde_json::json!({ "setup_token": token, "username": "pirate", "password": PASSWORD }),
            ))
            .await;
        assert_ne!(status, StatusCode::OK, "le setup ne doit pas rejouer");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Session ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn login_sets_a_hardened_cookie_that_logout_clears() {
        let h = Harness::with_admin().await;
        let (status, _, cookies) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({ "username": "admin", "password": PASSWORD }),
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        let cookie = cookies.first().expect("un cookie est attendu");
        assert!(cookie.contains("HttpOnly"), "cookie non HttpOnly : {cookie}");
        assert!(cookie.contains("SameSite=Strict"), "{cookie}");
        assert!(cookie.contains("Secure"), "{cookie}");

        let session = cookie.split(';').next().unwrap().to_string();
        let (status, _, _) = h
            .call(with_cookie(empty_request("GET", "/api/probes"), &session))
            .await;
        assert_eq!(status, StatusCode::OK);

        h.call(with_cookie(empty_request("POST", "/api/logout"), &session))
            .await;
        let (status, _, _) = h
            .call(with_cookie(empty_request("GET", "/api/probes"), &session))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "la session doit être fermée");
    }

    #[tokio::test]
    async fn login_with_a_wrong_password_is_rejected() {
        let h = Harness::with_admin().await;
        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({ "username": "admin", "password": "mauvais" }),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn listing_probes_without_a_session_is_rejected() {
        let h = Harness::with_admin().await;
        let (status, _, _) = h.call(empty_request("GET", "/api/probes")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // ── Sites ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn sites_are_readable_with_account_credentials() {
        // La sonde appelle cette route avant l'enrôlement, pour proposer une
        // liste plutôt qu'un champ libre — c'est ce qui évite qu'un « Durnad »
        // se glisse dans le parc.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        h.call(with_cookie(
            json_request("POST", "/api/sites", serde_json::json!({ "name": "Durand" })),
            &session,
        ))
        .await;

        let basic = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            format!("admin:{PASSWORD}"),
        );
        let mut req = empty_request("GET", "/api/sites");
        req.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Basic {basic}").parse().unwrap(),
        );
        let (status, body, _) = h.call(req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body[0]["name"], "Durand");
    }

    #[tokio::test]
    async fn deleting_a_populated_site_is_refused() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        h.enroll(&session, "Durand", "Paris").await;
        let (_, sites, _) = h
            .call(with_cookie(empty_request("GET", "/api/sites"), &session))
            .await;
        let site_id = sites[0]["site_id"].as_str().unwrap().to_string();

        let (status, _, _) = h
            .call(with_cookie(
                empty_request("DELETE", &format!("/api/sites/{site_id}")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "aucune suppression en cascade");
    }

    // ── Enrôlement ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn enrolling_with_a_code_returns_influx_settings_and_a_token() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (_, site, _) = h
            .call(with_cookie(
                json_request("POST", "/api/sites", serde_json::json!({ "name": "Durand" })),
                &session,
            ))
            .await;
        let site_id = site["site_id"].as_str().unwrap().to_string();
        let (_, code, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    "/api/enroll-codes",
                    serde_json::json!({ "site_id": site_id }),
                ),
                &session,
            ))
            .await;

        let (status, body, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({ "code": code["code"], "name": "Paris" }),
            ))
            .await;

        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["site"], "Durand", "le site vient du code");
        assert_eq!(body["name"], "Paris");
        assert!(body["token"].is_string());
        assert_eq!(body["influx"]["bucket"], "lanprobe");
        assert!(body["influx"]["token"].is_string());
        assert_eq!(body["heartbeat_interval_secs"], 60);

        let rendered = body.to_string();
        assert!(
            !rendered.contains(OPERATOR_TOKEN),
            "le jeton opérateur ne doit jamais sortir : {rendered}"
        );
    }

    #[tokio::test]
    async fn a_consumed_enrollment_code_no_longer_opens_anything() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (_, site, _) = h
            .call(with_cookie(
                json_request("POST", "/api/sites", serde_json::json!({ "name": "Durand" })),
                &session,
            ))
            .await;
        let (_, code, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    "/api/enroll-codes",
                    serde_json::json!({ "site_id": site["site_id"] }),
                ),
                &session,
            ))
            .await;
        h.call(json_request(
            "POST",
            "/api/probes/enroll",
            serde_json::json!({ "code": code["code"], "name": "Paris" }),
        ))
        .await;

        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({ "code": code["code"], "name": "Lyon" }),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn enrolling_with_credentials_creates_the_site_and_says_so() {
        let h = Harness::with_admin().await;
        let (status, body, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({
                    "username": "admin", "password": PASSWORD,
                    "name": "Paris", "site": "Durand"
                }),
            ))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["site_created"], true);

        // Le même site, écrit autrement : résolu, pas recréé.
        let (_, body, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({
                    "username": "admin", "password": PASSWORD,
                    "name": "Lyon", "site": "DURAND"
                }),
            ))
            .await;
        assert_eq!(body["site_created"], false);
    }

    #[tokio::test]
    async fn enrolling_with_bad_credentials_is_rejected() {
        let h = Harness::with_admin().await;
        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({
                    "username": "admin", "password": "mauvais",
                    "name": "Paris", "site": "Durand"
                }),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_duplicate_probe_name_in_the_same_site_is_a_conflict() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        h.enroll(&session, "Durand", "Paris").await;

        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({
                    "username": "admin", "password": PASSWORD,
                    "name": "paris", "site": "Durand"
                }),
            ))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn reenrolling_repairs_the_probe_without_changing_its_id() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, old_token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, code, _) = h
            .call(with_cookie(
                empty_request("POST", &format!("/api/probes/{probe_id}/reenroll-code")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{code}");

        let (status, body, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({ "code": code["code"], "name": "Paris" }),
            ))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(
            body["probe_id"], probe_id,
            "le probe_id doit survivre, sinon les courbes se coupent en deux"
        );
        assert_ne!(body["token"].as_str().unwrap(), old_token);

        // L'ancien jeton de sonde ne vaut plus rien.
        let (status, _, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/heartbeat"),
                    serde_json::json!({ "buffered_points": 0 }),
                ),
                &old_token,
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn reenrolling_also_retires_the_old_influx_write_token() {
        // La machine compromise a gardé une copie de sa clé d'écriture : si on
        // ne la détruit pas, elle continue d'écrire dans le bucket après le
        // ré-enrôlement.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;
        let old_auth = h
            .state
            .db
            .get_probe(&probe_id)
            .unwrap()
            .influx_auth_id
            .expect("l'enrôlement doit rattacher une autorisation");

        let (_, code, _) = h
            .call(with_cookie(
                empty_request("POST", &format!("/api/probes/{probe_id}/reenroll-code")),
                &session,
            ))
            .await;
        h.call(json_request(
            "POST",
            "/api/probes/enroll",
            serde_json::json!({ "code": code["code"], "name": "Paris" }),
        ))
        .await;

        let deleted = h._fake.calls().into_iter().any(|(m, p, _)| {
            m == "DELETE" && p == format!("/api/v2/authorizations/{old_auth}")
        });
        assert!(deleted, "l'ancienne autorisation Influx doit être détruite");
    }

    #[tokio::test]
    async fn a_reenrollment_code_cannot_create_a_different_probe() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let (_, code, _) = h
            .call(with_cookie(
                empty_request("POST", &format!("/api/probes/{probe_id}/reenroll-code")),
                &session,
            ))
            .await;

        // Un autre nom dans le corps ne doit pas ouvrir la création d'une
        // seconde sonde : le code désigne une machine, pas un droit d'entrée.
        let (status, body, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({ "code": code["code"], "name": "Machine-du-pirate" }),
            ))
            .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["probe_id"], probe_id);
        assert_eq!(body["name"], "Paris", "le nom du corps est ignoré");
        assert_eq!(
            h.state.db.list_probes(None).unwrap().len(),
            1,
            "aucune seconde sonde ne doit apparaître"
        );
    }

    // ── Battement de cœur ──────────────────────────────────────────────────

    #[tokio::test]
    async fn a_heartbeat_returns_the_current_name_and_site() {
        // Sans ce retour, une sonde renommée continuerait d'écrire l'ancien
        // nom indéfiniment.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Pariss").await;

        h.call(with_cookie(
            json_request(
                "PATCH",
                &format!("/api/probes/{probe_id}"),
                serde_json::json!({ "name": "Paris" }),
            ),
            &session,
        ))
        .await;

        let (status, body, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/heartbeat"),
                    serde_json::json!({ "version": "1.2.0", "platform": "linux", "buffered_points": 7 }),
                ),
                &token,
            ))
            .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        assert_eq!(body["name"], "Paris");
        assert_eq!(body["site"], "Durand");
        assert_eq!(body["heartbeat_interval_secs"], 60);
    }

    #[tokio::test]
    async fn a_heartbeat_with_a_wrong_token_is_rejected() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let (status, _, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/heartbeat"),
                    serde_json::json!({ "buffered_points": 0 }),
                ),
                "faux-jeton",
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_revoked_probe_can_no_longer_beat() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, _, _) = h
            .call(with_cookie(
                empty_request("DELETE", &format!("/api/probes/{probe_id}")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/heartbeat"),
                    serde_json::json!({ "buffered_points": 0 }),
                ),
                &token,
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // La ligne — donc l'historique lisible — est intacte.
        let kept = h.state.db.get_probe(&probe_id).unwrap();
        assert!(kept.revoked_at.is_some());
        assert_eq!(kept.name, "Paris");
    }

    // ── Rotation ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_rotation_delivers_the_new_token_at_the_next_heartbeat() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;
        let before = h.state.db.get_probe(&probe_id).unwrap();

        let (status, _, _) = h
            .call(with_cookie(
                empty_request("POST", &format!("/api/probes/{probe_id}/rotate")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            h.state.db.get_probe(&probe_id).unwrap().influx_auth_id,
            before.influx_auth_id,
            "l'ancien jeton reste actif tant que la sonde n'a pas accusé réception"
        );

        let (_, body, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/heartbeat"),
                    serde_json::json!({ "buffered_points": 0 }),
                ),
                &token,
            ))
            .await;
        assert_eq!(body["influx"]["token_version"], 2);
        assert!(body["influx"]["token"].is_string(), "{body}");

        // La sonde accuse la nouvelle version : l'ancienne autorisation est
        // alors — et seulement alors — retirée.
        h.call(with_bearer(
            json_request(
                "POST",
                &format!("/api/probes/{probe_id}/heartbeat"),
                serde_json::json!({ "buffered_points": 0, "influx_token_version": 2 }),
            ),
            &token,
        ))
        .await;
        let after = h.state.db.get_probe(&probe_id).unwrap();
        assert_eq!(after.influx_token_version, 2);
        assert!(after.pending_influx_token.is_none());
    }

    #[tokio::test]
    async fn an_immediate_revocation_cuts_the_write_token_but_not_the_probe_token() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, _, _) = h
            .call(with_cookie(
                empty_request("POST", &format!("/api/probes/{probe_id}/revoke-token")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            h.state.db.get_probe(&probe_id).unwrap().influx_auth_id.is_none(),
            "le jeton d'écriture est coupé tout de suite"
        );

        // Le jeton de sonde survit : c'est lui qui rend la réparation à
        // distance possible.
        let (status, body, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/heartbeat"),
                    serde_json::json!({ "buffered_points": 0 }),
                ),
                &token,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["influx"]["token"].is_string(), "la sonde repart avec une clé neuve");
    }

    // ── Réglages ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn settings_never_expose_a_secret() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (status, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/settings"), &session))
            .await;

        assert_eq!(status, StatusCode::OK);
        let rendered = body.to_string();
        assert!(!rendered.contains(OPERATOR_TOKEN), "{rendered}");
        assert!(rendered.contains("influx_url"));
    }

    #[tokio::test]
    async fn shortening_the_retention_requires_an_explicit_confirmation() {
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request("PUT", "/api/settings", serde_json::json!({ "retention_days": "30" })),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/settings",
                    serde_json::json!({ "retention_days": "30", "confirm_data_loss": true }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(h.state.settings.retention_days(), 30);
    }

    #[tokio::test]
    async fn a_changed_heartbeat_interval_reaches_enrollment_and_heartbeat() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        h.call(with_cookie(
            json_request(
                "PUT",
                "/api/settings",
                serde_json::json!({ "heartbeat_interval_secs": "30" }),
            ),
            &session,
        ))
        .await;

        let (_, site, _) = h
            .call(with_cookie(
                json_request("POST", "/api/sites", serde_json::json!({ "name": "Durand" })),
                &session,
            ))
            .await;
        let (_, code, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    "/api/enroll-codes",
                    serde_json::json!({ "site_id": site["site_id"] }),
                ),
                &session,
            ))
            .await;
        let (_, enrolled, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({ "code": code["code"], "name": "Paris" }),
            ))
            .await;
        assert_eq!(enrolled["heartbeat_interval_secs"], 30);

        let (_, beat, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{}/heartbeat", enrolled["probe_id"].as_str().unwrap()),
                    serde_json::json!({ "buffered_points": 0 }),
                ),
                enrolled["token"].as_str().unwrap(),
            ))
            .await;
        assert_eq!(
            beat["heartbeat_interval_secs"], 30,
            "les sondes déjà enrôlées doivent suivre le changement"
        );
    }

    #[tokio::test]
    async fn an_invalid_advertise_url_is_refused_with_a_readable_message() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/settings/influx-advertise",
                    serde_json::json!({ "url": "influx.example:8086" }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["error"].as_str().unwrap().contains("URL"), "{body}");
    }

    #[tokio::test]
    async fn the_advertise_test_reports_unreachable_as_a_result_not_an_error() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        h.call(with_cookie(
            json_request(
                "PUT",
                "/api/settings/influx-advertise",
                serde_json::json!({ "url": "http://127.0.0.1:1" }),
            ),
            &session,
        ))
        .await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("POST", "/api/settings/influx-advertise/test"),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "un échec de joignabilité n'est pas une panne du hub");
        assert_eq!(body["reachable"], false);
        assert_eq!(body["source"], "settings");
        assert_eq!(body["url"], "http://127.0.0.1:1");
    }

    #[tokio::test]
    // Verrou tenu pendant les `await` : c'est voulu. Il protège une variable
    // d'environnement, globale au processus — la relâcher plus tôt rouvrirait
    // la course qu'il existe précisément pour fermer.
    #[allow(clippy::await_holding_lock)]
    async fn the_tested_url_is_the_one_the_probe_receives() {
        // Deux chemins de résolution finiraient par diverger, et le test
        // validerait alors une URL que la sonde ne reçoit pas.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY : sérialisé par ENV_LOCK.
        unsafe { std::env::remove_var(crate::influx::ADVERTISE_URL_ENV) };
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let (_, tested, _) = h
            .call(with_cookie(
                empty_request("POST", "/api/settings/influx-advertise/test"),
                &session,
            ))
            .await;
        let (_, enrolled, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({
                    "username": "admin", "password": PASSWORD,
                    "name": "Paris", "site": "Durand"
                }),
            ))
            .await;

        assert_eq!(tested["url"], enrolled["influx"]["url"]);
        assert_eq!(tested["source"], "host_header");
    }

    // ── Jetons de lecture ──────────────────────────────────────────────────

    #[tokio::test]
    async fn a_read_token_is_shown_once_and_only_tracked_by_id() {
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    "/api/influx/read-token",
                    serde_json::json!({ "description": "Grafana du bureau" }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert!(body["token"].is_string(), "affiché une seule fois : {body}");
        let auth_id = body["auth_id"].as_str().unwrap().to_string();

        let (_, listed, _) = h
            .call(with_cookie(empty_request("GET", "/api/influx/read-tokens"), &session))
            .await;
        assert_eq!(listed[0]["auth_id"], auth_id);
        assert!(
            listed[0].get("token").is_none(),
            "la valeur n'est jamais conservée : {listed}"
        );

        let (status, _, _) = h
            .call(with_cookie(
                empty_request("DELETE", &format!("/api/influx/read-tokens/{auth_id}")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        let (_, listed, _) = h
            .call(with_cookie(empty_request("GET", "/api/influx/read-tokens"), &session))
            .await;
        assert_eq!(listed.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn a_read_token_is_scoped_to_reading_the_bucket_only() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        h.call(with_cookie(
            json_request(
                "POST",
                "/api/influx/read-token",
                serde_json::json!({ "description": "Grafana" }),
            ),
            &session,
        ))
        .await;

        let (_, _, body) = h
            ._fake
            .calls()
            .into_iter()
            .rev()
            .find(|(m, p, _)| m == "POST" && p.starts_with("/api/v2/authorizations"))
            .expect("une autorisation doit être demandée");
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        let permissions = body["permissions"].as_array().unwrap();
        assert_eq!(permissions.len(), 1, "une seule permission : {body}");
        assert_eq!(
            permissions[0]["action"], "read",
            "un jeton d'écriture distribué pour Grafana serait un joli trou : {body}"
        );
        assert_eq!(permissions[0]["resource"]["type"], "buckets");
        assert_eq!(permissions[0]["resource"]["id"], "bucket-1");
    }

    // ── Lecture des mesures ────────────────────────────────────────────────

    #[tokio::test]
    async fn metrics_are_proxied_and_need_a_session() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let (status, _, _) = h
            .call(empty_request(
                "GET",
                &format!("/api/probes/{probe_id}/metrics?measurement=ping&range=-1h"),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _, _) = h
            .call(with_cookie(
                empty_request(
                    "GET",
                    &format!("/api/probes/{probe_id}/metrics?measurement=ping&range=-1h"),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let queried = h
            ._fake
            .calls()
            .into_iter()
            .any(|(m, p, _)| m == "POST" && p.starts_with("/api/v2/query"));
        assert!(queried, "la requête doit partir vers Influx, pas vers le navigateur");
    }
}
