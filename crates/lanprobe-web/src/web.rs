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
//!
//! ## Les rôles s'appliquent ici, route par route
//!
//! Une interface qui masque les boutons ne protège rien : c'est le routeur
//! qui décide. Trois groupes, du moins au plus privilégié :
//!
//! - **viewer** — consulter le parc et les réglages ;
//! - **operator** — enrôler, renommer, déplacer, faire tourner les clés ;
//! - **admin** — les comptes, les réglages, la rétention, le journal d'audit
//!   et les jetons qui quittent le hub.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
    Extension, Json, Router,
};
use cookie::{Cookie, SameSite};
use serde::Deserialize;
use serde_json::json;

use crate::auth::Auth;
use webauthn_rs::prelude::*;
use crate::db::{AuditFilter, Db, DbError, DbResult, Outcome, ProbeIdentity, Role};
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
    /// Alertes : évaluation des transitions et envoi sur les canaux. Ses
    /// secrets sont scellés et ne sortent par aucune route.
    pub notifier: crate::notify::Notifier,
    /// Magasin scellé (`enc:v1:`). Porte les secrets des canaux de
    /// notification et, depuis le second facteur, celui de chaque compte.
    pub secrets: crate::secrets::Secrets,
    /// Défis WebAuthn entamés, en mémoire et à durée de vie courte.
    ///
    /// En mémoire et non en base : un défi ne survit pas à un redémarrage de
    /// toute façon — le navigateur, lui, aura perdu le sien — et le poser en
    /// base ne ferait qu'y laisser des lignes mortes.
    pub ceremonies: Arc<crate::passkeys::Ceremonies>,
    /// Vrai quand le hub termine lui-même le TLS. Faux quand il sert en clair
    /// derrière un reverse proxy — le cas courant en auto-hébergement.
    pub tls: bool,
    /// Vrai dès qu'une restauration a eu lieu dans ce processus.
    ///
    /// Le hub sert encore l'ancienne base : SQLite tient le fichier remplacé
    /// sous le pied, et toute écriture échoue en « readonly database ». Sans
    /// ce drapeau exposé par `/api/status`, l'avertissement de redémarrage ne
    /// vivait que sur la carte de restauration — il disparaissait au premier
    /// rechargement, laissant un hub qui refuse d'écrire sans dire pourquoi.
    pub restart_required: Arc<std::sync::atomic::AtomicBool>,
    /// Le volume du hub : base SQLite, `secret.key`, jeton opérateur,
    /// certificats. C'est ce que la sauvegarde emporte et ce que la
    /// restauration remplace.
    pub config_dir: std::path::PathBuf,
    /// Le volume des archives, monté sur `/backup` dans le conteneur.
    pub backup_dir: std::path::PathBuf,
    /// Chemin de la CLI `influx`, présente dans l'image du hub.
    pub influx_cli: std::path::PathBuf,
}

pub fn build_router(state: AppState) -> Router {
    // Routes ouvertes : elles portent leur propre authentification (token de
    // setup, code d'enrôlement, identifiants, jeton de sonde).
    let public = Router::new()
        .route("/api/status", get(status))
        .route("/api/setup", post(setup))
        .route("/api/login", post(login))
        .route("/api/login/passkey/start", post(passkey_login_start))
        .route("/api/login/passkey/finish", post(passkey_login_finish))
        .route("/api/logout", post(logout))
        .route("/api/probes/enroll", post(enroll))
        .route("/api/probes/{id}/write", post(write_metrics))
        .route("/api/probes/{id}/heartbeat", post(heartbeat))
        .route("/api/probes/{id}/scans", post(publish_scan))
        .route("/api/probes/{id}/config", post(store_probe_config));

    // `GET /api/sites` accepte aussi les identifiants du compte : la sonde
    // l'appelle avant l'enrôlement, quand elle n'a pas encore de session.
    let sites_read = Router::new()
        .route("/api/sites", get(list_sites))
        .layer(middleware::from_fn_with_state(
            (state.clone(), Role::Viewer),
            role_guard,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), session_or_credentials));

    // Consultation. Aucune de ces réponses ne porte de secret — c'est ce qui
    // permet de les ouvrir au rôle le plus bas.
    let read_only = guarded(
        &state,
        Role::Viewer,
        Router::new()
            .route("/api/me", get(me))
            .route("/api/me/password", post(change_own_password))
            .route("/api/me/totp", get(totp_status).delete(disable_own_totp))
            .route("/api/me/totp/start", post(start_own_totp))
            .route("/api/me/totp/confirm", post(confirm_own_totp))
            .route("/api/me/passkeys", get(list_own_passkeys))
            .route("/api/me/passkeys/start", post(start_passkey_registration))
            .route("/api/me/passkeys/finish", post(finish_passkey_registration))
            .route("/api/me/passkeys/{id}", axum::routing::delete(delete_own_passkey))
            .route("/api/probes", get(list_probes))
            .route("/api/settings", get(get_settings))
            .route("/api/settings/influx-advertise", get(get_advertise))
            .route("/api/influx", get(influx_overview))
            .route(
                "/api/notifications/subscriptions",
                get(list_subscriptions),
            ),
    );

    // Conduite du parc. `GET /api/enroll-codes/pending` en fait partie et non
    // de la consultation : le code y figure **en clair** pendant sa validité,
    // c'est de quoi enrôler une sonde.
    let parc = guarded(
        &state,
        Role::Operator,
        Router::new()
            .route("/api/sites", post(create_site))
            .route("/api/sites/{id}", patch(rename_site).delete(delete_site))
            .route("/api/sites/{id}/archive", post(archive_site))
            .route("/api/enroll-codes", post(create_enroll_code))
            .route("/api/enroll-codes/pending", get(list_pending_codes))
            .route("/api/settings/influx-advertise/test", post(test_advertise))
            .route(
                "/api/notifications/subscriptions",
                put(put_subscription),
            ),
    );

    // Les routes qui désignent UNE sonde vivent à part, derrière le garde de
    // portée. Elles gardent leur rôle minimum d'origine — la portée s'ajoute
    // au rôle, elle ne le remplace pas : un lecteur restreint à un site reste
    // un lecteur sur ce site.
    let probe_read = guarded_probe_scope(
        &state,
        Role::Viewer,
        Router::new()
            .route("/api/probes/{id}/metrics", get(probe_metrics))
            .route("/api/probes/{id}/inventory", get(probe_inventory))
            .route("/api/probes/{id}/sla.csv", get(probe_sla_csv))
            .route("/api/probes/{id}/sla", get(probe_sla_samples)),
    );

    let probe_write = guarded_probe_scope(
        &state,
        Role::Operator,
        Router::new()
            .route("/api/probes/{id}", patch(update_probe).delete(revoke_probe))
            .route("/api/probes/{id}/archive", post(archive_probe))
            .route("/api/probes/{id}/rotate", post(rotate_token))
            .route("/api/probes/{id}/revoke-token", post(revoke_token_now))
            .route("/api/probes/{id}/reenroll-code", post(create_reenroll_code))
            .route(
                "/api/probes/{id}/commands",
                get(list_commands).post(create_command),
            ),
    );

    // Administration : les comptes, les réglages, la rétention, le journal, et
    // les jetons de lecture — qui, eux, quittent le hub pour aller dans un
    // Grafana. **Aucune route de suppression du journal d'audit n'existe.**
    let administration = guarded(
        &state,
        Role::Admin,
        Router::new()
            .route("/api/settings", put(put_settings))
            .route("/api/settings/influx-advertise", put(put_advertise))
            .route("/api/influx/read-token", post(create_read_token))
            .route("/api/influx/read-tokens", get(list_read_tokens))
            .route("/api/influx/read-tokens/{id}", delete(revoke_read_token))
            .route("/api/users", get(list_users).post(create_user))
            .route("/api/users/{username}", patch(update_user))
            .route("/api/users/{username}/password", post(reset_password))
            .route("/api/audit", get(read_audit))
            .route("/api/notifications", get(notifications))
            .route(
                "/api/notifications/webhook",
                put(put_webhook).delete(clear_webhook),
            )
            .route(
                "/api/notifications/smtp",
                put(put_smtp).delete(clear_smtp),
            )
            .route("/api/notifications/test", post(test_notification))
            .route("/api/backup", post(create_backup))
            .route("/api/backups", get(list_backups))
            .route("/api/backup/restore/{file}", post(restore_named_backup))
            .route(
                "/api/backup/restore",
                post(restore_uploaded_backup)
                    // Une archive fait plusieurs centaines de Mo dès qu'elle
                    // porte les séries : le plafond de 2 Mio d'axum la
                    // rejetterait sans que personne comprenne pourquoi. Le
                    // corps est écrit au fil de l'eau sur disque, jamais
                    // gardé en mémoire.
                    .layer(axum::extract::DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
            ),
    );

    Router::new()
        .merge(public)
        .merge(sites_read)
        .merge(read_only)
        .merge(probe_read)
        .merge(parc)
        .merge(probe_write)
        .merge(administration)
        .with_state(state)
        // L'interface web est servie en dernier : toute requête qui n'a
        // trouvé aucune route d'API tombe ici.
        .fallback(crate::assets::serve)
        // Journal d'accès. Sans lui, un « je n'arrive pas à me connecter »
        // est indiagnosticable : le serveur répond correctement à tous les
        // tests et personne ne voit ce que le client a réellement envoyé.
        // On journalise la présence du cookie, jamais sa valeur.
        .layer(middleware::from_fn(access_log))
}

/// Authentifie puis exige un rôle minimum. L'ordre compte : `session_guard`
/// est posée en dernier, donc s'exécute en premier et pose l'identité que
/// `role_guard` relit.
fn guarded(state: &AppState, minimum: Role, router: Router<AppState>) -> Router<AppState> {
    router
        .layer(middleware::from_fn_with_state((state.clone(), minimum), role_guard))
        .layer(middleware::from_fn_with_state(state.clone(), session_guard))
}

/// Comme [`guarded`], plus la portée par site (contrat § 17).
///
/// ⚠️ **À réserver aux routes dont le paramètre `{id}` est un `probe_id`.**
/// C'est la raison d'être de ce routeur séparé : `/api/sites/{id}` porte le
/// même nom de paramètre et désigne autre chose. Un garde posé sur tout le
/// routeur aurait cherché une sonde là où passe un site.
///
/// Le contrôle est un intergiciel et non un appel dans chaque handler parce
/// qu'un handler ajouté demain hériterait alors du garde sans que personne y
/// pense — et une route oubliée, ici, rend le parc d'un autre client.
fn guarded_probe_scope(
    state: &AppState,
    minimum: Role,
    router: Router<AppState>,
) -> Router<AppState> {
    router
        .layer(middleware::from_fn_with_state(state.clone(), probe_scope_guard))
        .layer(middleware::from_fn_with_state((state.clone(), minimum), role_guard))
        .layer(middleware::from_fn_with_state(state.clone(), session_guard))
}

/// Rend 404 quand la sonde visée sort de la portée de l'appelant.
///
/// ⚠️ **404 et non 403.** « Ça existe mais pas pour vous » révèle déjà
/// l'existence du site d'un autre client — précisément ce que la portée
/// protège. Une sonde hors portée doit être indistinguable d'une sonde qui
/// n'existe pas.
async fn probe_scope_guard(
    State(state): State<AppState>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    use axum::extract::RawPathParams;
    use axum::RequestExt;

    // L'identité est posée par `session_guard`, qui tourne avant celui-ci.
    // Son absence n'est pas un cas à traiter ici : sans elle la requête ne
    // serait jamais arrivée jusqu'à cette couche.
    let Some(actor) = req.extensions().get::<Identity>().cloned() else {
        return next.run(req).await;
    };
    if matches!(actor.scope, crate::db::Scope::All) {
        return next.run(req).await;
    }

    let probe_id = match req.extract_parts::<RawPathParams>().await {
        Ok(params) => params
            .iter()
            .find(|(name, _)| *name == "id")
            .map(|(_, value)| value.to_string()),
        Err(_) => None,
    };
    if let Some(id) = probe_id {
        if let Err(refusal) = probe_in_scope(&state, &actor, &id) {
            return refusal;
        }
    }
    next.run(req).await
}

// ── Erreurs ────────────────────────────────────────────────────────────────

/// Traduit une erreur du stockage en code HTTP. Les handlers ne devinent pas
/// le code d'après le texte du message — ils se tromperaient.
fn error_response(e: DbError) -> Response {
    let status = match e {
        DbError::Conflict(_) => StatusCode::CONFLICT,
        DbError::NotFound(_) => StatusCode::NOT_FOUND,
        DbError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
        DbError::Forbidden(_) => StatusCode::FORBIDDEN,
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
        "restart_required": state
            .restart_required
            .load(std::sync::atomic::Ordering::Relaxed),
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
    let username = body.username.trim();
    match state.auth.setup(&body.setup_token, username, &body.password) {
        Ok(()) => {
            audit(&state, Some(username), "auth.setup", None, Outcome::Success, None);
            ok_json(json!({ "ok": true }))
        }
        Err(e) => {
            // Une tentative de setup ratée est une tentative de prise de
            // contrôle : c'est la ligne qu'on veut voir, pas celle du succès.
            audit(&state, None, "auth.setup", None, Outcome::Failure, Some(&e.to_string()));
            error_response(e)
        }
    }
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
    /// Code du second facteur, quand le compte en exige un (contrat § 19).
    ///
    /// Absent au premier envoi : le hub répond alors `totp_required` et le
    /// formulaire réclame le code. Renvoyer le mot de passe avec le code
    /// évite d'entretenir un état « connexion à moitié faite » côté hub —
    /// un état qui expire, qu'il faut nettoyer, et qui devient une file de
    /// sessions à demi ouvertes qu'un attaquant peut remplir.
    #[serde(default)]
    code: Option<String>,
}

/// Clé du secret TOTP d'un compte dans le magasin scellé.
fn totp_key(username: &str) -> String {
    format!("totp:{username}")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredTotp {
    secret: String,
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Credentials>,
) -> Response {
    let secure = is_https(&state, &headers);

    // Le mot de passe d'abord, le second facteur ensuite : sans cet ordre, un
    // code valide renseignerait sur l'existence du compte avant même qu'un
    // mot de passe soit exigé.
    let verified = state.db.verify_credentials(&body.username, &body.password);
    if let Ok(username) = &verified {
        if state.db.totp_enabled(username).unwrap_or(false) {
            let Some(code) = body.code.as_deref().filter(|c| !c.trim().is_empty()) else {
                // Pas une ligne d'audit : c'est la moitié normale d'une
                // connexion à deux facteurs, pas un échec. La journaliser
                // remplirait le journal d'échecs qui n'en sont pas, et
                // noierait ceux qui comptent.
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "code à usage unique requis", "totp_required": true })),
                )
                    .into_response();
            };
            let stored: Option<StoredTotp> = state.secrets.get_json(&totp_key(username));
            let Some(stored) = stored else {
                // 🔴 Fail **closed**. Le drapeau dit que ce compte exige un
                // second facteur ; le secret ne s'ouvre pas — volume restauré
                // sans son `secret.key`, typiquement. Laisser passer sur le
                // seul mot de passe désactiverait le second facteur en
                // silence, exactement le jour où l'on a des raisons de s'en
                // méfier. Le message dit la sortie de secours.
                audit(
                    &state,
                    Some(username.as_str()),
                    "auth.login",
                    None,
                    Outcome::Failure,
                    Some("secret TOTP illisible"),
                );
                return fail(
                    StatusCode::FORBIDDEN,
                    "le second facteur de ce compte est illisible (clé du volume perdue ?). \
                     Depuis le conteneur : `lanprobe-web disable-totp <compte>`.",
                );
            };
            let last = state.db.totp_last_step(username).unwrap_or(None);
            match crate::totp::verify(&stored.secret, code, crate::db::now(), last) {
                Some(step) => {
                    // 🔴 Retenu AVANT d'ouvrir la session : un code vaut
                    // trente secondes, et sans cette mémoire il se rejoue.
                    let _ = state.db.set_totp_last_step(username, step);
                }
                None => {
                    audit(
                        &state,
                        Some(username.as_str()),
                        "auth.login",
                        None,
                        Outcome::Failure,
                        Some("code à usage unique invalide"),
                    );
                    return fail(StatusCode::UNAUTHORIZED, "code à usage unique invalide");
                }
            }
        }
    }

    match verified.and_then(|username| state.auth.start_session(username)) {
        Ok(token) => {
            audit(
                &state,
                Some(body.username.trim()),
                "auth.login",
                None,
                Outcome::Success,
                None,
            );
            (
                [(header::SET_COOKIE, session_cookie(&token, false, secure))],
                Json(json!({ "ok": true })),
            )
                .into_response()
        }
        Err(e) => {
            // Le champ « identifiant » reçoit ce que quelqu'un a tapé : un
            // jour, ce sera un mot de passe entré dans la mauvaise case. On ne
            // le recopie donc que si le compte existe — sinon la tentative est
            // journalisée sans acteur, ce qui suffit à la voir.
            let known = state.db.get_user(body.username.trim()).ok();
            let (actor, detail) = match &known {
                Some(user) if user.disabled_at.is_some() => {
                    (Some(user.username.as_str()), "compte désactivé")
                }
                Some(user) => (Some(user.username.as_str()), "mot de passe invalide"),
                None => (None, "compte inconnu"),
            };
            audit(&state, actor, "auth.login", None, Outcome::Failure, Some(detail));
            error_response(e)
        }
    }
}

// ── Second facteur (contrat § 19) ──────────────────────────────────────────

/// Ce que le hub a le droit d'annoncer : actif ou non. Jamais le secret.
async fn totp_status(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Response {
    let enabled = state.db.totp_enabled(&identity.username).unwrap_or(false);
    // Un secret posé mais jamais confirmé n'est pas « actif » : tant que
    // personne n'a prouvé que son application génère les bons codes, activer
    // le second facteur reviendrait à s'enfermer dehors.
    let pending = !enabled && state.secrets.is_set(&totp_key(&identity.username));
    ok_json(json!({ "enabled": enabled, "pending": pending }))
}

/// Tire un secret et rend de quoi l'enrôler : QR, URI et secret en clair.
///
/// ⚠️ Le secret ne sort qu'ICI, une seule fois, et le second facteur n'est PAS
/// encore actif. Il le devient à la confirmation — c'est-à-dire quand le
/// compte a prouvé que son application produit les bons codes. Activer dès la
/// génération enfermerait dehors quiconque scanne mal son QR.
async fn start_own_totp(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Response {
    if state.db.totp_enabled(&identity.username).unwrap_or(false) {
        return fail(
            StatusCode::CONFLICT,
            "le second facteur est déjà actif sur ce compte — désactivez-le d'abord",
        );
    }
    let secret = match crate::totp::generate_secret() {
        Ok(s) => s,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
    };
    if let Err(e) = state
        .secrets
        .put_json(&totp_key(&identity.username), &StoredTotp { secret: secret.clone() })
    {
        return error_response(e);
    }
    let uri = crate::totp::provisioning_uri("LanProbe Hub", &identity.username, &secret);
    let qr = crate::totp::qr_svg(&uri).unwrap_or_default();
    // Pas de ligne d'audit ici : rien n'a changé pour la sécurité du compte
    // tant que la confirmation n'a pas eu lieu. C'est celle-là qu'on veut
    // voir dans le journal, pas les essais.
    ok_json(json!({ "secret": secret, "uri": uri, "qr_svg": qr }))
}

#[derive(Deserialize)]
struct TotpCode {
    code: String,
}

/// Active le second facteur après avoir vérifié un premier code.
async fn confirm_own_totp(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(body): Json<TotpCode>,
) -> Response {
    let Some(stored) = state
        .secrets
        .get_json::<StoredTotp>(&totp_key(&identity.username))
    else {
        return fail(
            StatusCode::CONFLICT,
            "aucun secret en attente — relancez la configuration",
        );
    };
    let Some(step) = crate::totp::verify(&stored.secret, &body.code, crate::db::now(), None) else {
        audit(
            &state,
            Some(&identity.username),
            "auth.totp.enable",
            None,
            Outcome::Failure,
            Some("code invalide"),
        );
        return fail(
            StatusCode::UNAUTHORIZED,
            "code invalide — vérifiez l'heure de l'appareil qui le génère",
        );
    };
    if let Err(e) = state.db.set_totp_enabled(&identity.username, true) {
        return error_response(e);
    }
    // Le code qui vient de servir à confirmer ne doit pas servir à se
    // connecter dans la foulée : c'est le même rejeu, à une seconde près.
    let _ = state.db.set_totp_last_step(&identity.username, step);
    audit(
        &state,
        Some(&identity.username),
        "auth.totp.enable",
        None,
        Outcome::Success,
        None,
    );
    ok_json(json!({ "enabled": true }))
}

#[derive(Deserialize)]
struct DisableTotpBody {
    /// Le mot de passe courant. ⚠️ Exigé : sans lui, une session volée
    /// suffirait à retirer le second facteur, c'est-à-dire à annuler
    /// précisément ce contre quoi il protège.
    password: String,
}

async fn disable_own_totp(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(body): Json<DisableTotpBody>,
) -> Response {
    if state
        .db
        .verify_credentials(&identity.username, &body.password)
        .is_err()
    {
        audit(
            &state,
            Some(&identity.username),
            "auth.totp.disable",
            None,
            Outcome::Failure,
            Some("mot de passe invalide"),
        );
        return fail(StatusCode::UNAUTHORIZED, "mot de passe invalide");
    }
    if let Err(e) = state.db.set_totp_enabled(&identity.username, false) {
        return error_response(e);
    }
    // Le secret part avec : le garder scellé sans qu'il serve à rien ne
    // protège personne et ferait revenir un ancien code au prochain
    // « activer », sur un QR que l'utilisateur croit neuf.
    let _ = state.secrets.clear(&totp_key(&identity.username));
    audit(
        &state,
        Some(&identity.username),
        "auth.totp.disable",
        None,
        Outcome::Success,
        None,
    );
    ok_json(json!({ "enabled": false }))
}

// ── Clés d'accès (contrat § 19) ────────────────────────────────────────────

/// Décrit une clé sans rien révéler de sensible — une clé publique ne l'est
/// pas, mais l'identifiant complet n'a aucune raison de s'afficher.
fn passkey_json(p: &crate::db::PasskeyRecord) -> serde_json::Value {
    json!({
        "id": p.credential_id,
        "label": p.label,
        "created_at": p.created_at,
        "last_used_at": p.last_used_at,
    })
}

async fn list_own_passkeys(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
) -> Response {
    let host = host_header(&headers).unwrap_or_default();
    let https = is_https(&state, &headers);
    match state.db.list_passkeys(&identity.username) {
        Ok(rows) => ok_json(json!({
            "passkeys": rows.iter().map(passkey_json).collect::<Vec<_>>(),
            // ⚠️ Renvoyé pour que l'écran DISE pourquoi le bouton ne peut pas
            // marcher, au lieu de proposer une action que le navigateur
            // refusera sans message.
            "secure_context": crate::passkeys::secure_context(&host, https),
            "rp_id": crate::passkeys::rp_id_of(&host),
        })),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct PasskeyLabel {
    #[serde(default)]
    label: Option<String>,
}

async fn start_passkey_registration(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
) -> Response {
    let host = host_header(&headers).unwrap_or_default();
    let https = is_https(&state, &headers);
    if !crate::passkeys::secure_context(&host, https) {
        // Le navigateur refuserait de toute façon, mais sans rien dire
        // d'exploitable. Le hub, lui, peut nommer la cause et la solution.
        return fail(
            StatusCode::CONFLICT,
            "les clés d'accès exigent une connexion HTTPS (ou localhost). \
             Activez le TLS dans Réglages → Général, ou placez le hub derrière \
             un reverse proxy.",
        );
    }
    let webauthn = match crate::passkeys::context(&host, https) {
        Ok(w) => w,
        Err(e) => return fail(StatusCode::CONFLICT, &e),
    };

    // Les clés déjà posées sont exclues : sans cela, le navigateur propose
    // gaiement d'en réenregistrer une, et l'insertion échoue à la fin sur une
    // contrainte d'unicité — après que l'utilisateur a touché son capteur.
    let existing: Vec<CredentialID> = state
        .db
        .list_passkeys(&identity.username)
        .unwrap_or_default()
        .iter()
        .filter_map(|p| serde_json::from_str::<Passkey>(&p.credential).ok())
        .map(|k| k.cred_id().clone())
        .collect();

    // ⚠️ Un identifiant STABLE et propre au compte. Le tirer au hasard à
    // chaque cérémonie ferait apparaître un nouveau compte dans le
    // gestionnaire de clés à chaque enregistrement.
    let user_unique_id = stable_user_uuid(&identity.username);

    match webauthn.start_passkey_registration(
        user_unique_id,
        &identity.username,
        &identity.username,
        Some(existing),
    ) {
        Ok((challenge, registration)) => {
            state
                .ceremonies
                .put_registration(&identity.username, registration);
            ok_json(serde_json::to_value(challenge).unwrap_or(serde_json::Value::Null))
        }
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// Un UUID déterministe, dérivé du nom de compte.
///
/// WebAuthn veut un identifiant opaque et stable. Le nom seul ne convient pas
/// (il doit faire 64 octets au plus et rester binaire), et un tirage aléatoire
/// non plus : il changerait à chaque cérémonie, et le gestionnaire de clés du
/// système afficherait un compte de plus à chaque enregistrement.
fn stable_user_uuid(username: &str) -> Uuid {
    let digest = ring::digest::digest(&ring::digest::SHA256, username.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.as_ref()[..16]);
    Uuid::from_bytes(bytes)
}

#[derive(Deserialize)]
struct FinishRegistration {
    credential: RegisterPublicKeyCredential,
    #[serde(flatten)]
    naming: PasskeyLabel,
}

async fn finish_passkey_registration(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Json(body): Json<FinishRegistration>,
) -> Response {
    let host = host_header(&headers).unwrap_or_default();
    let https = is_https(&state, &headers);
    let webauthn = match crate::passkeys::context(&host, https) {
        Ok(w) => w,
        Err(e) => return fail(StatusCode::CONFLICT, &e),
    };
    let Some(pending) = state.ceremonies.take_registration(&identity.username) else {
        return fail(
            StatusCode::CONFLICT,
            "aucune inscription en cours, ou elle a expiré — recommencez",
        );
    };

    let passkey = match webauthn.finish_passkey_registration(&body.credential, &pending) {
        Ok(k) => k,
        Err(e) => {
            audit(
                &state,
                Some(&identity.username),
                "auth.passkey.add",
                None,
                Outcome::Failure,
                Some(&e.to_string()),
            );
            return fail(StatusCode::BAD_REQUEST, &e.to_string());
        }
    };

    let id = base64_url(passkey.cred_id().as_ref());
    let label = body
        .naming
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or("Clé d'accès")
        .to_string();
    let serialized = match serde_json::to_string(&passkey) {
        Ok(s) => s,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    match state
        .db
        .add_passkey(&identity.username, &id, &label, &serialized)
    {
        Ok(()) => {
            audit(
                &state,
                Some(&identity.username),
                "auth.passkey.add",
                Some(&label),
                Outcome::Success,
                None,
            );
            ok_json(json!({ "ok": true }))
        }
        Err(e) => error_response(e),
    }
}

async fn delete_own_passkey(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    match state.db.remove_passkey(&identity.username, &id) {
        Ok(()) => {
            audit(
                &state,
                Some(&identity.username),
                "auth.passkey.remove",
                Some(&id),
                Outcome::Success,
                None,
            );
            ok_json(json!({ "ok": true }))
        }
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct PasskeyLoginStart {
    username: String,
}

/// Ouvre une cérémonie d'authentification pour un compte.
///
/// ⚠️ Répond la même chose pour un compte sans clé et pour un compte
/// inexistant : un défi qui n'aboutira jamais plutôt qu'un refus. Distinguer
/// les deux transformerait cette route en oracle qui dit quels comptes
/// existent, sans qu'aucun mot de passe soit demandé.
async fn passkey_login_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PasskeyLoginStart>,
) -> Response {
    let host = host_header(&headers).unwrap_or_default();
    let https = is_https(&state, &headers);
    let webauthn = match crate::passkeys::context(&host, https) {
        Ok(w) => w,
        Err(e) => return fail(StatusCode::CONFLICT, &e),
    };
    let username = body.username.trim().to_string();
    let keys: Vec<Passkey> = state
        .db
        .list_passkeys(&username)
        .unwrap_or_default()
        .iter()
        .filter_map(|p| serde_json::from_str(&p.credential).ok())
        .collect();

    if keys.is_empty() {
        return fail(
            StatusCode::NOT_FOUND,
            "aucune clé d'accès pour ce compte sur ce hub",
        );
    }

    match webauthn.start_passkey_authentication(&keys) {
        Ok((challenge, pending)) => {
            state.ceremonies.put_authentication(&username, pending);
            ok_json(serde_json::to_value(challenge).unwrap_or(serde_json::Value::Null))
        }
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct PasskeyLoginFinish {
    username: String,
    credential: PublicKeyCredential,
}

async fn passkey_login_finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PasskeyLoginFinish>,
) -> Response {
    let host = host_header(&headers).unwrap_or_default();
    let https = is_https(&state, &headers);
    let secure = is_https(&state, &headers);
    let webauthn = match crate::passkeys::context(&host, https) {
        Ok(w) => w,
        Err(e) => return fail(StatusCode::CONFLICT, &e),
    };
    let username = body.username.trim().to_string();
    let Some(pending) = state.ceremonies.take_authentication(&username) else {
        return fail(
            StatusCode::UNAUTHORIZED,
            "aucune authentification en cours, ou elle a expiré",
        );
    };

    let result = match webauthn.finish_passkey_authentication(&body.credential, &pending) {
        Ok(r) => r,
        Err(e) => {
            audit(
                &state,
                Some(&username),
                "auth.login",
                None,
                Outcome::Failure,
                Some(&format!("clé d'accès refusée : {e}")),
            );
            return fail(StatusCode::UNAUTHORIZED, "clé d'accès refusée");
        }
    };

    // Un compte désactivé ne rentre pas, quelle que soit la clé : la
    // désactivation doit valoir pour tous les chemins d'entrée, sinon elle ne
    // vaut rien.
    match state.db.get_user(&username) {
        Ok(user) if user.disabled_at.is_none() => {}
        _ => {
            audit(
                &state,
                Some(&username),
                "auth.login",
                None,
                Outcome::Failure,
                Some("compte désactivé"),
            );
            return fail(StatusCode::FORBIDDEN, "compte désactivé");
        }
    }

    // 🔴 Le compteur anti-clonage a pu bouger : le réécrire est la seule
    // chose qui rende ce compteur utile. Sans cette mise à jour, une clé
    // dupliquée ne serait jamais détectée.
    if result.needs_update() {
        let id = base64_url(result.cred_id().as_ref());
        if let Some(record) = state
            .db
            .list_passkeys(&username)
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.credential_id == id)
        {
            if let Ok(mut key) = serde_json::from_str::<Passkey>(&record.credential) {
                key.update_credential(&result);
                if let Ok(updated) = serde_json::to_string(&key) {
                    let _ = state.db.touch_passkey(&username, &id, &updated);
                }
            }
        }
    }

    match state.auth.start_session(username.clone()) {
        Ok(token) => {
            audit(
                &state,
                Some(&username),
                "auth.login",
                None,
                Outcome::Success,
                Some("clé d'accès"),
            );
            (
                [(header::SET_COOKIE, session_cookie(&token, false, secure))],
                Json(json!({ "ok": true })),
            )
                .into_response()
        }
        Err(e) => error_response(e),
    }
}

/// Base64 URL sans remplissage — la forme qu'emploie WebAuthn partout.
fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Qui est connecté, et avec quel rôle.
///
/// Sans cette route, l'interface ne peut ni savoir quels écrans montrer, ni
/// marquer votre propre ligne dans la liste des comptes : elle apprenait son
/// rôle en se prenant un refus, ce qui écrit une ligne d'audit à chaque
/// ouverture de session — précisément le bruit qui empêche de repérer, dans
/// ce même journal, la série de refus qui compte.
async fn me(Extension(identity): Extension<Identity>) -> Response {
    ok_json(json!({ "username": identity.username, "role": identity.role.as_str() }))
}

#[derive(Deserialize)]
struct OwnPasswordBody {
    current: String,
    next: String,
}

/// Changement de son PROPRE mot de passe (contrat § 19).
///
/// ⚠️ Distinct de `POST /api/users/{…}/password`, réservé aux
/// administrateurs. Sans cette route, un lecteur ou un opérateur ne pourrait
/// jamais changer son mot de passe sans demander à un administrateur — et un
/// administrateur qui change le mot de passe de quelqu'un le connaît.
///
/// ⚠️ L'ancien mot de passe est **exigé**. Une session volée suffirait sinon à
/// s'approprier le compte définitivement : le voleur changerait le mot de
/// passe et le propriétaire n'aurait plus aucun chemin de retour.
async fn change_own_password(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(body): Json<OwnPasswordBody>,
) -> Response {
    if state
        .db
        .verify_credentials(&identity.username, &body.current)
        .is_err()
    {
        audit(
            &state,
            Some(&identity.username),
            "user.password",
            Some(&identity.username),
            Outcome::Failure,
            Some("mot de passe actuel incorrect"),
        );
        return fail(StatusCode::FORBIDDEN, "mot de passe actuel incorrect");
    }
    if body.next == body.current {
        return fail(StatusCode::BAD_REQUEST, "le nouveau mot de passe est identique à l'ancien");
    }
    if let Err(e) = state.db.reset_user_password(&identity.username, &body.next) {
        return error_response(e);
    }
    audit(
        &state,
        Some(&identity.username),
        "user.password",
        Some(&identity.username),
        Outcome::Success,
        Some("changement par le titulaire du compte"),
    );
    ok_json(json!({ "ok": true }))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = session_of(&headers) {
        if let Some(username) = state.auth.validate(&token) {
            audit(&state, Some(&username), "auth.logout", None, Outcome::Success, None);
        }
        state.auth.logout(&token);
    }
    (
        [(header::SET_COOKIE, session_cookie("", true, is_https(&state, &headers)))],
        Json(json!({ "ok": true })),
    )
        .into_response()
}

/// Vrai si le client parle en HTTPS — soit directement, soit via un reverse
/// proxy qui a terminé le TLS et l'annonce par `X-Forwarded-Proto`.
///
/// C'est ce qui décide de l'attribut `Secure` du cookie. Le poser en dur
/// rendrait la connexion impossible derrière un proxy en clair : le navigateur
/// refuse silencieusement le cookie, l'utilisateur reçoit un 200 puis retombe
/// sur l'écran de connexion, sans le moindre message d'erreur.
fn is_https(state: &AppState, headers: &HeaderMap) -> bool {
    if state.tls {
        return true;
    }
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        // Un proxy peut chaîner les valeurs ; la première est celle du client.
        .map(|v| v.split(',').next().unwrap_or("").trim().eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

fn session_cookie(token: &str, expired: bool, secure: bool) -> String {
    let mut builder = Cookie::build((SESSION_COOKIE, token.to_string()))
        .http_only(true)
        .same_site(SameSite::Strict)
        .path("/")
        .secure(secure);
    if expired {
        builder = builder.max_age(cookie::time::Duration::seconds(0));
    }
    builder.build().to_string()
}

/// Journal d'accès : méthode, chemin, code, et **présence** du cookie de
/// session — jamais sa valeur. Savoir si le navigateur a renvoyé son cookie
/// est ce qui distingue « mauvais mot de passe » de « cookie refusé par le
/// navigateur », deux pannes qui se ressemblent et se corrigent autrement.
async fn access_log(request: axum::extract::Request, next: middleware::Next) -> Response {
    let method = request.method().clone();
    let path = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_default();
    let has_cookie = session_of(request.headers()).is_some();
    let forwarded = request
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
        .to_string();
    let response = next.run(request).await;
    tracing::info!(
        "{method} {path} → {} (cookie: {}, x-forwarded-proto: {forwarded})",
        response.status().as_u16(),
        if has_cookie { "présent" } else { "absent" },
    );
    response
}

fn session_of(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|pair| Cookie::parse(pair.trim()).ok())
        .find(|c| c.name() == SESSION_COOKIE)
        .map(|c| c.value().to_string())
}

/// Qui appelle, et avec quel rôle.
///
/// **Le rôle est relu à chaque requête**, jamais figé dans la session : une
/// rétrogradation ou une désactivation prendrait sinon effet à la prochaine
/// connexion, c'est-à-dire jamais pour qui garde son onglet ouvert.
#[derive(Clone, Debug)]
pub struct Identity {
    pub username: String,
    pub role: Role,
    /// Les sites que ce compte a le droit de voir (contrat § 17).
    ///
    /// ⚠️ Calculée à CHAQUE requête, comme le rôle : une portée posée dans le
    /// cookie de session survivrait à sa révocation, et un compte qu'on vient
    /// de restreindre continuerait de tout voir tant qu'il garde son onglet
    /// ouvert.
    pub scope: crate::db::Scope,
}

/// Rend l'identité d'un compte actif. `None` si le compte a été désactivé
/// pendant que sa session courait — la session ne vaut alors plus rien.
fn identity_of(state: &AppState, username: &str) -> Option<Identity> {
    let user = state.db.get_user(username).ok()?;
    if user.disabled_at.is_some() {
        return None;
    }
    let scope = state
        .db
        .user_scope(&user.username, user.role)
        .unwrap_or(crate::db::Scope::All);
    Some(Identity {
        username: user.username,
        role: user.role,
        scope,
    })
}

/// Refuse une sonde hors portée, en **404**.
///
/// ⚠️ Pas 403 : « ça existe mais pas pour vous » révèle déjà l'existence du
/// site d'un autre client, ce qui est exactement ce que la portée protège.
/// Une sonde hors portée doit être indistinguable d'une sonde inexistante.
fn site_in_scope(actor: &Identity, site_id: &str) -> Result<(), Response> {
    if actor.scope.allows(site_id) {
        return Ok(());
    }
    Err(fail(StatusCode::NOT_FOUND, "site inconnu"))
}

fn probe_in_scope(state: &AppState, actor: &Identity, probe_id: &str) -> Result<(), Response> {
    match state.db.get_probe(probe_id) {
        Ok(p) if actor.scope.allows(&p.site_id) => Ok(()),
        _ => Err(fail(StatusCode::NOT_FOUND, "sonde inconnue")),
    }
}

async fn session_guard(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    if state.auth.needs_setup() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "hub non configuré", "needs_setup": true })),
        )
            .into_response();
    }
    let identity = session_of(&headers)
        .and_then(|t| state.auth.validate(&t))
        .and_then(|username| identity_of(&state, &username));
    match identity {
        Some(identity) => {
            req.extensions_mut().insert(identity);
            next.run(req).await
        }
        None => fail(StatusCode::UNAUTHORIZED, "session absente ou expirée"),
    }
}

/// Exige un rôle minimum, **après** authentification. Un refus est une ligne
/// d'audit : une tentative sur une route interdite est exactement ce qu'un
/// journal doit montrer.
async fn role_guard(
    State((state, minimum)): State<(AppState, Role)>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(identity) = req.extensions().get::<Identity>().cloned() else {
        return fail(StatusCode::UNAUTHORIZED, "session absente ou expirée");
    };
    if identity.role >= minimum {
        return next.run(req).await;
    }
    let target = format!("{} {}", req.method(), req.uri().path());
    audit(
        &state,
        Some(&identity.username),
        "access.denied",
        Some(&target),
        Outcome::Failure,
        Some(&format!(
            "rôle {} — {} requis au minimum",
            identity.role.as_str(),
            minimum.as_str()
        )),
    );
    fail(
        StatusCode::FORBIDDEN,
        "rôle insuffisant pour cette action",
    )
}

/// Ajoute une ligne au journal d'audit. L'échec d'écriture ne fait pas
/// échouer la requête — mais il est bruyant : un journal muet qui laisse
/// croire qu'il enregistre est pire que pas de journal du tout.
///
/// **Aucun secret ne doit passer par `detail`.**
fn audit(
    state: &AppState,
    actor: Option<&str>,
    action: &str,
    target: Option<&str>,
    outcome: Outcome,
    detail: Option<&str>,
) {
    if let Err(e) = state.db.record_audit(actor, action, target, outcome, detail) {
        tracing::error!("ligne d'audit « {action} » non écrite : {e}");
    }
}

/// Journalise le résultat d'une opération, quel qu'il soit, et rend la
/// réponse. Les échecs comptent autant que les réussites.
fn audited<T>(
    state: &AppState,
    actor: Option<&str>,
    action: &str,
    target: Option<&str>,
    result: DbResult<T>,
) -> DbResult<T> {
    match &result {
        Ok(_) => audit(state, actor, action, target, Outcome::Success, None),
        Err(e) => audit(
            state,
            actor,
            action,
            target,
            Outcome::Failure,
            Some(&e.to_string()),
        ),
    }
    result
}

/// Session **ou** identifiants du compte en Basic. La sonde n'a pas de session
/// avant son enrôlement : lui imposer d'en ouvrir une pour lire la liste des
/// sites l'obligerait à manipuler un cookie pour rien.
async fn session_or_credentials(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let mut identity = session_of(&headers)
        .and_then(|t| state.auth.validate(&t))
        .and_then(|username| identity_of(&state, &username));
    if identity.is_none()
        && let Some((username, password)) = basic_credentials(&headers)
        && state.db.verify_credentials(&username, &password).is_ok()
    {
        identity = identity_of(&state, &username);
    }
    match identity {
        Some(identity) => {
            req.extensions_mut().insert(identity);
            next.run(req).await
        }
        None => fail(StatusCode::UNAUTHORIZED, "session ou identifiants requis"),
    }
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

// ── Comptes ────────────────────────────────────────────────────────────────
//
// Il n'y a pas de route de suppression, et il n'y en aura pas : on désactive.
// Un compte effacé emporterait la traçabilité de tout ce qu'il a fait au
// journal d'audit — c'est précisément ce qu'on veut garder.

async fn list_users(State(state): State<AppState>) -> Response {
    let users = match state.db.list_users() {
        Ok(users) => users,
        Err(e) => return error_response(e),
    };
    // La portée est jointe à chaque compte : sans elle, l'écran des comptes
    // devrait interroger une route par ligne pour savoir ce que chacun voit,
    // et afficherait « tous les sites » en attendant — c'est-à-dire l'inverse
    // de la vérité pour un compte restreint.
    let rows: Vec<serde_json::Value> = users
        .into_iter()
        .map(|u| {
            let sites = state.db.list_user_sites(&u.username).unwrap_or_default();
            let mut value = serde_json::to_value(&u).unwrap_or(serde_json::Value::Null);
            if let Some(map) = value.as_object_mut() {
                map.insert("sites".into(), serde_json::json!(sites));
            }
            value
        })
        .collect();
    ok_json(serde_json::Value::Array(rows))
}

#[derive(Deserialize)]
struct NewUserBody {
    username: String,
    password: String,
    role: String,
}

async fn create_user(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Json(body): Json<NewUserBody>,
) -> Response {
    // Un rôle mal orthographié est refusé, jamais interprété : le
    // réinterpréter donnerait un compte dont personne ne sait ce qu'il peut.
    let Some(role) = Role::parse(&body.role) else {
        audit(
            &state,
            Some(&actor.username),
            "user.create",
            Some(body.username.trim()),
            Outcome::Failure,
            Some("rôle inconnu"),
        );
        return fail(
            StatusCode::BAD_REQUEST,
            "rôle attendu : admin, operator ou viewer",
        );
    };
    let created = audited(
        &state,
        Some(&actor.username),
        "user.create",
        Some(body.username.trim()),
        state.db.create_user(body.username.trim(), &body.password, role),
    );
    match created {
        Ok(user) => (
            StatusCode::CREATED,
            Json(serde_json::to_value(user).unwrap_or(serde_json::Value::Null)),
        )
            .into_response(),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct UserPatch {
    #[serde(default)]
    role: Option<String>,
    /// `true` désactive, `false` réactive. Jamais de suppression.
    #[serde(default)]
    disabled: Option<bool>,
    /// Portée par site (contrat § 17). Liste **vide** = accès à tous les
    /// sites ; absente = on ne touche pas à la portée existante. La
    /// distinction compte : `[]` est un ordre, `null` est un silence.
    #[serde(default)]
    sites: Option<Vec<String>>,
}

async fn update_user(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(username): Path<String>,
    Json(body): Json<UserPatch>,
) -> Response {
    if let Some(raw) = body.role.as_deref() {
        let Some(role) = Role::parse(raw) else {
            return fail(
                StatusCode::BAD_REQUEST,
                "rôle attendu : admin, operator ou viewer",
            );
        };
        let changed = audited(
            &state,
            Some(&actor.username),
            "user.role",
            Some(&username),
            state.db.set_user_role(&actor.username, &username, role),
        );
        if let Err(e) = changed {
            return error_response(e);
        }
    }
    if let Some(disabled) = body.disabled {
        let action = if disabled { "user.disable" } else { "user.enable" };
        let changed = audited(
            &state,
            Some(&actor.username),
            action,
            Some(&username),
            state.db.set_user_disabled(&actor.username, &username, disabled),
        );
        if let Err(e) = changed {
            return error_response(e);
        }
    }
    if let Some(sites) = body.sites.as_deref() {
        // ⚠️ Restreindre un `admin` ne veut rien dire : il gère les comptes,
        // donc il lèverait sa propre restriction en trois clics. Le refus est
        // ici, en plus du champ masqué dans l'interface — une limite qu'on
        // peut retirer soi-même n'est pas une limite, et la promettre serait
        // pire que ne rien promettre.
        let target_role = match state.db.get_user(&username) {
            Ok(u) => u.role,
            Err(e) => return error_response(e),
        };
        if target_role == Role::Admin && !sites.is_empty() {
            return fail(
                StatusCode::CONFLICT,
                "un administrateur voit tous les sites : il pourrait lever sa propre \
                 restriction. Changez son rôle d'abord.",
            );
        }
        let detail = if sites.is_empty() {
            "tous les sites".to_string()
        } else {
            format!("{} site(s)", sites.len())
        };
        let written = state.db.set_user_sites(&username, sites);
        match &written {
            Ok(()) => audit(
                &state,
                Some(&actor.username),
                "user.scope",
                Some(&username),
                Outcome::Success,
                Some(&detail),
            ),
            Err(e) => audit(
                &state,
                Some(&actor.username),
                "user.scope",
                Some(&username),
                Outcome::Failure,
                Some(&e.to_string()),
            ),
        }
        if let Err(e) = written {
            return error_response(e);
        }
    }
    match state.db.get_user(&username) {
        Ok(user) => {
            let sites = state.db.list_user_sites(&username).unwrap_or_default();
            let mut value = serde_json::to_value(&user).unwrap_or(serde_json::Value::Null);
            if let Some(map) = value.as_object_mut() {
                map.insert("sites".into(), serde_json::json!(sites));
            }
            ok_json(value)
        }
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct PasswordBody {
    password: String,
}

async fn reset_password(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(username): Path<String>,
    Json(body): Json<PasswordBody>,
) -> Response {
    let done = audited(
        &state,
        Some(&actor.username),
        "user.password",
        Some(&username),
        state.db.reset_user_password(&username, &body.password),
    );
    match done {
        Ok(()) => ok_json(json!({ "ok": true })),
        Err(e) => error_response(e),
    }
}

// ── Journal d'audit ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AuditQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    before_id: Option<i64>,
    #[serde(default)]
    actor: Option<String>,
    #[serde(default)]
    action: Option<String>,
}

/// Lecture seule, et c'est la seule route du journal. Aucune ne supprime une
/// ligne, pas même pour un administrateur : un journal qu'on peut nettoyer ne
/// prouve rien.
async fn read_audit(State(state): State<AppState>, Query(query): Query<AuditQuery>) -> Response {
    let filter = AuditFilter {
        limit: query.limit.unwrap_or(crate::db::AUDIT_DEFAULT_LIMIT),
        before_id: query.before_id,
        actor: query.actor.filter(|a| !a.trim().is_empty()),
        action: query.action.filter(|a| !a.trim().is_empty()),
    };
    match state.db.list_audit(&filter) {
        Ok(entries) => {
            // Rendu seulement si la page est pleine : sinon l'interface
            // afficherait un « suivant » qui ne mène à rien.
            let next = (entries.len() as i64 >= filter.limit.clamp(1, crate::db::AUDIT_MAX_LIMIT))
                .then(|| entries.last().map(|e| e.id))
                .flatten();
            ok_json(json!({
                "entries": serde_json::to_value(&entries).unwrap_or(serde_json::Value::Null),
                "next_before_id": next,
            }))
        }
        Err(e) => error_response(e),
    }
}

// ── Notifications ──────────────────────────────────────────────────────────
//
// **Les secrets ne ressortent jamais.** L'URL du webhook porte à elle seule le
// droit d'écrire dans le salon, le mot de passe SMTP se passe de commentaire :
// `GET /api/notifications` dit « configuré » ou « non configuré », et le
// bouton de test envoie un vrai message — la seule vérification qui prouve
// quelque chose.

async fn notifications(State(state): State<AppState>) -> Response {
    ok_json(state.notifier.status())
}

/// `Option<Option<…>>` distingue **absent** de `null` : l'interface n'a jamais
/// vu l'URL ni le mot de passe, les lui faire renvoyer pour corriger un port
/// reviendrait à les effacer un jour sur deux. Absent = « ne touche pas »,
/// `null` = « retire-le ».
#[derive(Deserialize)]
struct WebhookBody {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    template: Option<String>,
}

async fn put_webhook(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Json(body): Json<WebhookBody>,
) -> Response {
    let template = match body.template.as_deref().unwrap_or("generic") {
        "slack" => crate::notify::WebhookTemplate::Slack,
        "discord" => crate::notify::WebhookTemplate::Discord,
        "generic" => crate::notify::WebhookTemplate::Generic,
        other => {
            return fail(
                StatusCode::BAD_REQUEST,
                &format!("modèle inconnu : {other} (attendu : generic, slack ou discord)"),
            )
        }
    };
    let existing = state.notifier.webhook_config();
    let url = match body.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        Some(url) => {
            if let Err(message) = validate_url(url) {
                return fail(StatusCode::BAD_REQUEST, &message);
            }
            url.to_string()
        }
        None => match existing.as_ref() {
            Some(cfg) => cfg.url.clone(),
            None => {
                return fail(
                    StatusCode::BAD_REQUEST,
                    "l'URL du webhook est requise à la première configuration",
                )
            }
        },
    };
    let cfg = crate::notify::WebhookConfig { url, template };
    // Le journal dit qu'on a configuré le canal et avec quel modèle. **Pas
    // l'URL** : elle est le secret.
    match audited(
        &state,
        Some(&actor.username),
        "notify.webhook",
        Some(template.as_str()),
        state.notifier.set_webhook(&cfg),
    ) {
        Ok(()) => ok_json(state.notifier.status()),
        Err(e) => error_response(e),
    }
}

async fn clear_webhook(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
) -> Response {
    unconfigure(&state, &actor, "webhook").await
}

#[derive(Deserialize)]
struct SmtpBody {
    host: String,
    port: u16,
    #[serde(default)]
    security: Option<String>,
    #[serde(default)]
    username: Option<String>,
    /// Absent = on garde celui qui est déjà scellé ; `null` = on le retire.
    #[serde(default, deserialize_with = "present_even_if_null")]
    password: Option<Option<String>>,
    from: String,
    to: Vec<String>,
}

/// Distingue « champ absent » de « champ à `null` ». Sans ça, serde rend
/// `None` dans les deux cas, et l'interface — qui n'a jamais vu le mot de
/// passe — l'effacerait à chaque correction de port.
fn present_even_if_null<'de, D>(de: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(de).map(Some)
}

async fn put_smtp(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Json(body): Json<SmtpBody>,
) -> Response {
    let security = match body.security.as_deref().unwrap_or("starttls") {
        "none" => crate::notify::SmtpSecurity::None,
        "starttls" => crate::notify::SmtpSecurity::Starttls,
        "tls" => crate::notify::SmtpSecurity::Tls,
        other => {
            return fail(
                StatusCode::BAD_REQUEST,
                &format!("chiffrement inconnu : {other} (attendu : none, starttls ou tls)"),
            )
        }
    };
    let recipients: Vec<String> = body
        .to
        .iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect();
    // Refusé tout de suite plutôt qu'au premier incident : une configuration
    // incomplète se découvrirait le jour où on compte sur elle.
    if body.host.trim().is_empty() {
        return fail(StatusCode::BAD_REQUEST, "le serveur SMTP est requis");
    }
    if body.from.trim().is_empty() {
        return fail(StatusCode::BAD_REQUEST, "l'adresse d'expéditeur est requise");
    }
    if recipients.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "au moins un destinataire est requis");
    }
    if body.port == 0 {
        return fail(StatusCode::BAD_REQUEST, "le port SMTP est requis");
    }

    let cfg = crate::notify::SmtpConfig {
        host: body.host.trim().to_string(),
        port: body.port,
        security,
        username: body.username.filter(|u| !u.trim().is_empty()),
        password: match body.password {
            // Fourni : on prend ce qui est fourni, une chaîne vide valant
            // « pas de mot de passe ».
            Some(given) => given.filter(|p| !p.is_empty()),
            // Absent : on garde celui qui est déjà scellé.
            None => state.notifier.smtp_config().and_then(|c| c.password),
        },
        from: body.from.trim().to_string(),
        to: recipients,
    };
    // Le serveur et le port ne sont pas des secrets ; le mot de passe n'entre
    // pas au journal.
    let detail = format!("{}:{} ({})", cfg.host, cfg.port, security.as_str());
    match audited(
        &state,
        Some(&actor.username),
        "notify.smtp",
        Some(&detail),
        state.notifier.set_smtp(&cfg),
    ) {
        Ok(()) => ok_json(state.notifier.status()),
        Err(e) => error_response(e),
    }
}

async fn clear_smtp(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
) -> Response {
    unconfigure(&state, &actor, "smtp").await
}

/// Dé-configure un canal. **Rien n'est supprimé en base** : la valeur scellée
/// est remplacée par du vide, la ligne et sa date restent.
async fn unconfigure(state: &AppState, actor: &Identity, channel: &'static str) -> Response {
    match audited(
        state,
        Some(&actor.username),
        "notify.unconfigure",
        Some(channel),
        state.notifier.clear(channel),
    ) {
        Ok(()) => ok_json(state.notifier.status()),
        Err(e) => error_response(e),
    }
}

/// Envoie un vrai message sur chaque canal configuré et rend le verdict de
/// chacun. Un canal en panne n'empêche pas l'autre : c'est justement le jour
/// où l'un tombe qu'on a besoin de l'autre.
async fn test_notification(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
) -> Response {
    let outcomes = state.notifier.send_test().await;
    let attempted = outcomes.iter().filter(|o| o.attempted).count();
    let succeeded = outcomes.iter().filter(|o| o.attempted && o.ok).count();
    audit(
        &state,
        Some(&actor.username),
        "notify.test",
        None,
        if attempted > 0 && succeeded == attempted {
            Outcome::Success
        } else {
            Outcome::Failure
        },
        Some(&format!("{succeeded}/{attempted} canal(aux)")),
    );
    ok_json(json!({
        "channels": serde_json::to_value(&outcomes).unwrap_or(serde_json::Value::Null),
    }))
}

/// Les abonnements, **héritage déjà résolu**. Le refaire dans l'interface est
/// exactement l'endroit où les deux se mettraient à diverger.
async fn list_subscriptions(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
) -> Response {
    let sites = match state.db.list_sites_scoped(&actor.scope, false) {
        Ok(sites) => sites,
        Err(e) => return error_response(e),
    };
    let probes = match state.db.list_probes_scoped(None, &actor.scope, false) {
        Ok(probes) => probes,
        Err(e) => return error_response(e),
    };
    let subscriptions = match state.db.list_notify_subscriptions() {
        Ok(rows) => rows,
        Err(e) => return error_response(e),
    };
    let stored = |scope: &str, id: &str| -> Option<bool> {
        subscriptions
            .iter()
            .find(|s| s.scope == scope && s.scope_id == id)
            .and_then(|s| s.enabled)
    };

    let sites: Vec<serde_json::Value> = sites
        .iter()
        .map(|site| {
            json!({
                "site_id": site.site_id,
                "name": site.name,
                "enabled": stored("site", &site.site_id).unwrap_or(false),
            })
        })
        .collect();
    let probes: Vec<serde_json::Value> = probes
        .iter()
        .map(|probe| {
            let exception = stored("probe", &probe.probe_id);
            json!({
                "probe_id": probe.probe_id,
                "name": probe.name,
                "site_id": probe.site_id,
                "site": probe.site_name,
                // `exception` dit ce qui est réglé sur la sonde — `null` =
                // elle suit son site. `enabled` dit ce qui s'applique.
                "exception": exception,
                "enabled": exception.unwrap_or_else(|| {
                    stored("site", &probe.site_id).unwrap_or(false)
                }),
            })
        })
        .collect();

    ok_json(json!({ "sites": sites, "probes": probes }))
}

#[derive(Deserialize)]
struct SubscriptionBody {
    scope: String,
    scope_id: String,
    /// `null` sur une sonde = elle revient à l'héritage de son site. Ce n'est
    /// pas une suppression : la ligne reste, avec `enabled` à NULL.
    #[serde(default)]
    enabled: Option<bool>,
}

async fn put_subscription(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Json(body): Json<SubscriptionBody>,
) -> Response {
    let detail = match body.enabled {
        Some(true) => "activé",
        Some(false) => "désactivé",
        None => "hérite",
    };
    let target = format!("{}:{}", body.scope, body.scope_id);
    // ⚠️ Écrire un abonnement hors portée reviendrait à révéler — et à
    // modifier — le réglage d'alerte du site d'un autre client. Le refus
    // prend la forme d'un 404 comme partout ailleurs : « inconnu », pas
    // « interdit ».
    let in_scope = match body.scope.as_str() {
        "site" => actor.scope.allows(&body.scope_id),
        "probe" => probe_in_scope(&state, &actor, &body.scope_id).is_ok(),
        _ => true,
    };
    if !in_scope {
        return fail(StatusCode::NOT_FOUND, "cible inconnue");
    }
    let written = state
        .db
        .set_notify_subscription(&body.scope, &body.scope_id, body.enabled);
    match &written {
        Ok(()) => audit(
            &state,
            Some(&actor.username),
            "notify.subscription",
            Some(&target),
            Outcome::Success,
            Some(detail),
        ),
        Err(e) => audit(
            &state,
            Some(&actor.username),
            "notify.subscription",
            Some(&target),
            Outcome::Failure,
            Some(&e.to_string()),
        ),
    }
    match written {
        Ok(()) => ok_json(json!({ "ok": true })),
        Err(e) => error_response(e),
    }
}

/// URL absolue avec schéma et hôte. Une URL tronquée donnerait un canal qui
/// paraît configuré et n'atteint rien.
fn validate_url(value: &str) -> Result<(), String> {
    let value = value.trim();
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err("URL absolue attendue, avec son schéma (https://…)".into());
    };
    if !matches!(scheme, "http" | "https") {
        return Err("schéma attendu : http ou https".into());
    }
    if rest.split(['/', '?']).next().unwrap_or("").trim().is_empty() {
        return Err("l'URL doit comporter un hôte".into());
    }
    Ok(())
}

// ── Sites ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ArchivedFilter {
    /// Inclure les sites/sondes archivés. Absent = non.
    ///
    /// Un paramètre plutôt qu'une route à part : c'est la même liste, avec
    /// une ligne de plus. Deux routes auraient fini par diverger sur le tri,
    /// la portée ou les champs rendus.
    #[serde(default)]
    archived: bool,
}

async fn list_sites(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Query(filter): Query<ArchivedFilter>,
) -> Response {
    match state.db.list_sites_scoped(&actor.scope, filter.archived) {
        Ok(sites) => ok_json(serde_json::to_value(sites).unwrap_or(serde_json::Value::Null)),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct SiteBody {
    name: String,
}

async fn create_site(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Json(body): Json<SiteBody>,
) -> Response {
    match audited(
        &state,
        Some(&actor.username),
        "site.create",
        Some(body.name.trim()),
        state.db.create_site(&body.name),
    ) {
        Ok(site) => (
            StatusCode::CREATED,
            Json(serde_json::to_value(site).unwrap_or(serde_json::Value::Null)),
        )
            .into_response(),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct ArchiveBody {
    /// `true` archive, `false` remet dans le parc. Explicite dans les deux
    /// sens : une bascule implicite se trompe dès que deux onglets sont
    /// ouverts sur le même site.
    archived: bool,
}

/// Retire un site du parc — ou l'y remet — **sans rien supprimer**.
///
/// 🔴 C'est la réponse à « comment je fais partir un client ». Le supprimer
/// vraiment rendrait ses mesures orphelines : un rapport SLA sur l'année
/// écoulée n'aurait plus de site auquel se rattacher, et les séries d'Influx
/// ne désigneraient plus rien. Ici tout reste, et l'opération se défait.
async fn archive_site(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<ArchiveBody>,
) -> Response {
    if let Err(refusal) = site_in_scope(&actor, &id) {
        return refusal;
    }
    let action = if body.archived { "site.archive" } else { "site.unarchive" };
    match audited(
        &state,
        Some(&actor.username),
        action,
        Some(&id),
        state.db.set_site_archived(&id, body.archived),
    ) {
        Ok(touched) => ok_json(json!({ "ok": true, "probes": touched })),
        Err(e) => error_response(e),
    }
}

async fn archive_probe(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<ArchiveBody>,
) -> Response {
    let action = if body.archived { "probe.archive" } else { "probe.unarchive" };
    match audited(
        &state,
        Some(&actor.username),
        action,
        Some(&id),
        state.db.set_probe_archived(&id, body.archived),
    ) {
        Ok(()) => ok_json(json!({ "ok": true })),
        Err(e) => error_response(e),
    }
}

async fn rename_site(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<SiteBody>,
) -> Response {
    if let Err(refusal) = site_in_scope(&actor, &id) {
        return refusal;
    }
    match audited(
        &state,
        Some(&actor.username),
        "site.rename",
        Some(&id),
        state.db.rename_site(&id, &body.name),
    ) {
        Ok(site) => ok_json(serde_json::to_value(site).unwrap_or(serde_json::Value::Null)),
        Err(e) => error_response(e),
    }
}

async fn delete_site(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    if let Err(refusal) = site_in_scope(&actor, &id) {
        return refusal;
    }
    match audited(
        &state,
        Some(&actor.username),
        "site.delete",
        Some(&id),
        state.db.delete_site(&id),
    ) {
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
    Extension(actor): Extension<Identity>,
    Json(body): Json<EnrollCodeBody>,
) -> Response {
    // ⚠️ Un opérateur restreint ne doit pas pouvoir poser une sonde chez un
    // autre client. Le refus est un 404 : nommer un site qu'on n'a pas le
    // droit de voir ne doit pas apprendre qu'il existe.
    if let Err(refusal) = site_in_scope(&actor, &body.site_id) {
        return refusal;
    }
    let site = match state.db.get_site(&body.site_id) {
        Ok(site) => site,
        Err(e) => return error_response(e),
    };
    // Le code lui-même n'entre jamais au journal : c'est de quoi enrôler.
    match audited(
        &state,
        Some(&actor.username),
        "enroll.code",
        Some(&site.site_id),
        state
            .db
            .create_enroll_code(&site.site_id, None, ENROLL_CODE_TTL_SECS),
    ) {
        Ok(code) => ok_json(json!({
            "code": code.code,
            "site": site.name,
            "site_id": site.site_id,
            "expires_at": code.expires_at,
        })),
        Err(e) => error_response(e),
    }
}

async fn create_reenroll_code(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    let probe = match state.db.get_probe(&id) {
        Ok(probe) => probe,
        Err(e) => return error_response(e),
    };
    match audited(
        &state,
        Some(&actor.username),
        "probe.reenroll_code",
        Some(&probe.probe_id),
        state
            .db
            .create_enroll_code(&probe.site_id, Some(&probe.probe_id), ENROLL_CODE_TTL_SECS),
    ) {
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
    /// Facultatif. Un **ré-enrôlement l'ignore toujours** : il répare une sonde
    /// existante, son nom appartient au hub. L'accepter renommait la sonde dès
    /// qu'on saisissait autre chose sur la machine — un geste de réparation ne
    /// doit pas changer l'identité de ce qu'il répare.
    ///
    /// Absent lors d'un premier enrôlement, le hub en choisit un.
    #[serde(default)]
    name: Option<String>,
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
    // L'acteur n'est connu que sur le chemin scripté ; par code d'enrôlement,
    // la sonde ne présente aucun compte. La ligne d'audit le dit plutôt que de
    // nommer un acteur qu'on aurait deviné.
    let actor = body.username.clone();

    // 1. Qui a le droit d'enrôler, et dans quel site.
    let (site, site_created, repairing) = match resolve_enrollment_target(&state, &body) {
        Ok(target) => target,
        Err(e) => {
            audit(
                &state,
                actor
                    .as_deref()
                    .map(str::to_string)
                    .or_else(|| body.name.as_deref().map(|n| format!("sonde {}", n.trim())))
                    .as_deref(),
                "probe.enroll",
                body.name.as_deref().map(str::trim),
                Outcome::Failure,
                Some(&e.to_string()),
            );
            return error_response(e);
        }
    };

    // 2. La sonde : réparation d'une existante, ou création.
    // Le nom n'est demandé qu'au premier enrôlement, et il reste facultatif :
    // c'est dans le hub qu'on nomme ses sondes, pas sur chaque machine.
    let chosen_name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_probe_name(&state, &site.site_id));

    let (probe, probe_token) = match repairing.clone() {
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
        None => match state.db.enroll_probe(&site.site_id, &chosen_name) {
            Ok(pair) => pair,
            Err(e) => return error_response(e),
        },
    };

    // 3. Son jeton d'écriture Influx, frappé pour elle seule.
    let minted = match mint_probe_token(&state, &probe.probe_id, &probe.name).await {
        Ok(token) => token,
        Err(e) => return fail(StatusCode::SERVICE_UNAVAILABLE, &e),
    };

    audit(
        &state,
        actor
            .as_deref()
            .map(str::to_string)
            .or_else(|| Some(format!("sonde {}", probe.name)))
            .as_deref(),
        if repairing.is_some() { "probe.reenroll" } else { "probe.enroll" },
        Some(&probe.probe_id),
        Outcome::Success,
        Some(&format!("site {}", site.name)),
    );

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
            // ⚠️ Rendue à l'enrôlement, pas au battement : c'est le seul
            // moment où une machine réinstallée peut retrouver ses profils.
            // Au battement, elle en a déjà, et les réécrire écraserait ce que
            // quelqu'un vient de régler devant l'écran.
            "config": probe
                .config
                .as_deref()
                .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok()),
        })),
    )
        .into_response()
}

/// Rend `(site, site_créé, sonde_à_réparer)`.
/// Un nom libre dans le site, quand la machine n'en propose pas.
///
/// « Sonde 1 », « Sonde 2 »… Sans ça, un enrôlement sans nom échouerait sur la
/// contrainte d'unicité dès la deuxième sonde du site.
fn default_probe_name(state: &AppState, site_id: &str) -> String {
    let existing = state.db.list_probes(Some(site_id)).unwrap_or_default();
    for n in 1..=999 {
        let candidate = format!("Sonde {n}");
        if !existing.iter().any(|p| p.name.eq_ignore_ascii_case(&candidate)) {
            return candidate;
        }
    }
    format!("Sonde {}", crate::db::now())
}

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
    // 🔴 Ce chemin n'a aucun moyen de présenter un second facteur : il n'y a
    // pas d'écran, et une sonde scriptée ne peut pas lire un code à usage
    // unique. Le laisser passer ferait du mot de passe seul une clé valide
    // pour un compte qui a précisément déclaré que le mot de passe seul ne
    // suffit plus — c'est-à-dire une porte dérobée autour du second facteur.
    //
    // Le code d'enrôlement est le chemin prévu pour les machines : il est à
    // usage unique, daté, et lié à un site.
    if state.db.totp_enabled(username).unwrap_or(false) {
        return Err(DbError::Forbidden(
            "ce compte exige un second facteur, que l'enrôlement par identifiants ne peut pas \
             présenter — utilisez un code d'enrôlement"
                .into(),
        ));
    }
    // Le chemin scripté passe par les identifiants du compte et non par une
    // session : sans ce contrôle, il contournerait tous les rôles.
    match state.db.role_of(username)? {
        Some(role) if role >= Role::Operator => {}
        _ => {
            return Err(DbError::Forbidden(
                "enrôler demande le rôle operator au minimum".into(),
            ))
        }
    }
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

/// Verdict d'une commande, rendu par la sonde.
#[derive(Deserialize)]
struct CommandAck {
    id: i64,
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

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
    /// Accusés des commandes remises au battement précédent (contrat § 14).
    /// Absent tant que la sonde n'a rien à accuser — une sonde plus ancienne
    /// que la file n'envoie simplement jamais ce champ.
    #[serde(default)]
    command_acks: Vec<CommandAck>,
    /// Verdict internet vu par la sonde (`online` / `limited` / `offline`).
    /// Absent d'une sonde antérieure à ce champ : le hub garde alors le
    /// dernier connu plutôt que de l'effacer.
    #[serde(default)]
    internet_state: Option<String>,
    /// Surveillances ICMP actives. Un tableau vide veut dire « je ne
    /// surveille plus rien » — ce n'est pas la même chose qu'un champ absent,
    /// qui vient d'une sonde antérieure à ce champ.
    #[serde(default)]
    monitors: Option<Vec<serde_json::Value>>,
    /// Identité réseau du site (contrat, section 15). Chaque champ est
    /// facultatif : une sonde sans accès internet bat **sans** IP publique
    /// plutôt que d'échouer, et un champ absent n'efface jamais ce que le hub
    /// savait déjà.
    #[serde(default)]
    public_ip: Option<String>,
    #[serde(default)]
    interface: Option<String>,
    #[serde(default)]
    local_ips: Vec<String>,
    #[serde(default)]
    gateway: Option<String>,
}

/// Écriture des mesures, relayée vers Influx.
///
/// Les sondes n'atteignent plus Influx directement : une seule porte, une
/// seule authentification, un seul certificat. Le corps est du line protocol
/// brut, transmis tel quel — le hub relaie, il n'accumule pas.
async fn write_metrics(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return fail(StatusCode::UNAUTHORIZED, "jeton de sonde requis");
    };
    if let Err(e) = state.db.authenticate_probe(&id, &token) {
        return error_response(e);
    }
    if body.trim().is_empty() {
        // Un lot vide n'est pas une erreur : une sonde qui n'a rien mesuré
        // pendant son intervalle a le droit de le dire sans être rejetée.
        return ok_json(json!({ "ok": true, "written": 0 }));
    }
    // ⚠️ **Le hub étiquette les points lui-même.**
    //
    // La sonde n'écrit que `host=<nom machine>` : sans `probe_id`, le hub ne
    // retrouvait plus ses propres mesures — elles arrivaient bien dans Influx,
    // mais toutes les requêtes filtrent sur `probe_id` et ne trouvaient rien.
    // Une sonde en ligne, des écritures en 200, et des graphes vides.
    //
    // Le faire ici plutôt que dans la sonde a un second mérite : l'étiquette
    // vient du jeton présenté, pas de ce que la sonde déclare. Une sonde ne
    // peut donc pas écrire sous l'identité d'une autre.
    let probe = match state.db.get_probe(&id) {
        Ok(p) => p,
        Err(e) => return error_response(e),
    };
    let tagged = tag_lines(&body, &probe.probe_id, &probe.site_id);
    let lines = tagged.lines().filter(|l| !l.trim().is_empty()).count();
    match state.influx.write_line_protocol(tagged).await {
        Ok(()) => ok_json(json!({ "ok": true, "written": lines })),
        // 502 et non 500 : la panne est en aval du hub. La sonde doit
        // conserver son lot et réessayer, pas le jeter comme s'il était
        // malformé — un 4xx lui dirait au contraire d'abandonner.
        Err(e) => fail(StatusCode::BAD_GATEWAY, &e),
    }
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

    let identity = ProbeIdentity {
        public_ip: body.public_ip.filter(|v| !v.trim().is_empty()),
        interface: body.interface.filter(|v| !v.trim().is_empty()),
        local_ips: body.local_ips,
        gateway: body.gateway.filter(|v| !v.trim().is_empty()),
        // ⚠️ Liste fermée : la sonde décide de son verdict, pas de son
        // vocabulaire. Une valeur inattendue serait affichée telle quelle dans
        // le parc, sans couleur ni sens.
        internet_state: body
            .internet_state
            .map(|v| v.trim().to_lowercase())
            .filter(|v| matches!(v.as_str(), "online" | "limited" | "offline")),
        // ⚠️ `Option`, pas `Vec`. Absent et vide ne veulent pas dire la même
        // chose : une sonde antérieure à ce champ n'envoie rien, et
        // l'enregistrer comme « je ne surveille rien » ferait passer TOUTES
        // ses cibles réelles pour des surveillances mortes à l'écran.
        // Un tableau explicitement vide, lui, efface : une sonde qui a cessé
        // de surveiller doit pouvoir le dire.
        monitors: body.monitors.as_ref().and_then(|m| serde_json::to_string(m).ok()),
    };
    let change = match state.db.record_heartbeat(
        &id,
        body.version.as_deref(),
        body.platform.as_deref(),
        body.buffered_points,
        &identity,
    ) {
        Ok(change) => change,
        Err(e) => return error_response(e),
    };

    // Une bascule d'IP publique se voit dans le journal : c'est ainsi qu'on
    // repère un passage sur un lien de secours ou un changement d'opérateur.
    // **Au changement seulement** — une ligne par battement noierait tout le
    // reste. L'acteur est la sonde elle-même, pas un opérateur.
    if let Some(change) = change {
        let previous = change.previous.as_deref().unwrap_or("(inconnue)");
        // L'acteur est la sonde, pas « anonyme ». Un journal qui dit
        // « anonyme » là où il connaît le nom oblige à recouper la cible pour
        // savoir qui a agi — et rend illisible la seule colonne qu'on lit en
        // diagonale quand on cherche ce qui s'est passé.
        audit(
            &state,
            Some(&format!("sonde {}", probe.name)),
            "probe.public_ip_changed",
            Some(&id),
            Outcome::Success,
            Some(&format!("{previous} → {}", change.current)),
        );
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

    // Les accusés d'abord : une commande réglée à ce battement ne doit pas
    // repartir dans la file du même battement.
    for ack in &body.command_acks {
        if let Err(e) = state.db.settle_command(
            &id,
            ack.id,
            ack.ok,
            ack.error.as_deref().filter(|m| !m.trim().is_empty()),
        ) {
            tracing::warn!("accusé de commande {} non enregistré : {e}", ack.id);
        }
    }

    // ⚠️ Le hub ne joint jamais la sonde : c'est ce battement qui porte les
    // commandes. Les remettre échoue en silence plutôt que de faire échouer le
    // battement — une file cassée ne doit pas faire passer une sonde saine
    // pour hors ligne.
    let commands = state.db.take_pending_commands(&id).unwrap_or_else(|e| {
        tracing::warn!("file de commandes illisible pour {id} : {e}");
        Vec::new()
    });

    let mut response = json!({
        "ok": true,
        "name": probe.name,
        "site_id": probe.site_id,
        "site": probe.site_name,
        "heartbeat_interval_secs": state.settings.heartbeat_interval_secs(),
        "commands": commands,
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
    #[serde(default)]
    archived: bool,
}

async fn list_probes(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Query(filter): Query<ProbeFilter>,
) -> Response {
    let interval = state.settings.heartbeat_interval_secs();
    let now = crate::db::now();
    match state
        .db
        .list_probes_scoped(filter.site.as_deref(), &actor.scope, filter.archived)
    {
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
                        "archived_at": p.archived_at,
                        // Dernière identité réseau connue. Jamais remise à
                        // vide quand la sonde ne répond plus : c'est
                        // précisément là qu'elle sert.
                        "public_ip": p.public_ip,
                        "public_ip_at": p.public_ip_at,
                        "interface": p.interface,
                        "local_ips": p.local_ips,
                        "gateway": p.gateway,
                        // Le dernier verdict internet de la sonde. C'est ce
                        // qui distingue une sonde saine d'une sonde qui bat
                        // très bien mais ne mesure plus rien.
                        "internet_state": p.internet_state,
                        "internet_state_at": p.internet_state_at,
                        // Ce que la sonde surveille MAINTENANT, par opposition
                        // aux mesures présentes dans la fenêtre affichée.
                        "monitors": p.monitors
                            .as_deref()
                            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok()),
                        "monitors_at": p.monitors_at,
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
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<ProbePatch>,
) -> Response {
    // La sonde d'origine est déjà validée par l'intergiciel de portée. Ce qui
    // ne l'est pas, c'est la DESTINATION : sans ce contrôle, un opérateur
    // restreint pourrait déplacer une sonde chez un autre client — et la
    // perdre de vue au passage, puisqu'elle sortirait de sa propre portée.
    if let Some(target) = body.site_id.as_deref() {
        if let Err(refusal) = site_in_scope(&actor, target) {
            return refusal;
        }
    }
    match audited(
        &state,
        Some(&actor.username),
        "probe.update",
        Some(&id),
        state
            .db
            .update_probe(&id, body.name.as_deref(), body.site_id.as_deref()),
    ) {
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
async fn revoke_probe(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    let probe = match state.db.get_probe(&id) {
        Ok(probe) => probe,
        Err(e) => return error_response(e),
    };
    if let Err(e) = audited(
        &state,
        Some(&actor.username),
        "probe.revoke",
        Some(&id),
        state.db.revoke_probe(&id),
    ) {
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
/// La sonde dépose sa configuration pour sauvegarde (contrat § 16).
///
/// ⚠️ Le hub ne la lit pas et ne l'affiche pas. Les profils réseau servent en
/// local, devant la machine ; les montrer ici n'apprendrait rien. Le hub est
/// un concentrateur : il les garde, ils partent dans ses sauvegardes, et une
/// sonde ré-enrôlée les retrouve — c'est tout ce qu'on lui demande.
async fn store_probe_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return fail(StatusCode::UNAUTHORIZED, "jeton de sonde requis");
    };
    if let Err(e) = state.db.authenticate_probe(&id, &token) {
        return error_response(e);
    }
    // Plafond : le blob est opaque, donc rien ne borne sa taille côté hub.
    // Un mégaoctet couvre très large pour des profils réseau.
    let serialised = body.to_string();
    if serialised.len() > 1024 * 1024 {
        return fail(StatusCode::PAYLOAD_TOO_LARGE, "configuration trop volumineuse");
    }
    match state.db.store_probe_config(&id, &serialised) {
        Ok(()) => ok_json(json!({ "ok": true })),
        Err(e) => error_response(e),
    }
}

/// La sonde publie le résultat complet d'un scan (contrat § 12).
///
/// ⚠️ Le hub ne dérive **pas** le compteur Influx de ce message : la sonde
/// continue de l'écrire elle-même. Deux chemins pour la même valeur finiraient
/// par diverger, et on ne saurait plus lequel croire.
async fn publish_scan(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(report): Json<crate::db::ScanReport>,
) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return fail(StatusCode::UNAUTHORIZED, "jeton de sonde requis");
    };
    if let Err(e) = state.db.authenticate_probe(&id, &token) {
        return error_response(e);
    }
    if !matches!(report.kind.as_str(), "ports" | "discovery" | "speedtest") {
        return fail(StatusCode::BAD_REQUEST, "genre de scan inconnu");
    }
    match state.db.record_scan(&id, &report) {
        Ok(scan_id) => ok_json(json!({ "scan_id": scan_id })),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct InventoryQuery {
    #[serde(default)]
    kind: Option<String>,
}

/// Dernier inventaire connu d'une sonde, et l'historique des tests de débit.
async fn probe_inventory(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<InventoryQuery>,
) -> Response {
    if state.db.get_probe(&id).is_err() {
        return fail(StatusCode::NOT_FOUND, "sonde inconnue");
    }
    match query.kind.as_deref() {
        Some("speedtest") => match state.db.speedtest_history(&id, 50) {
            Ok(rows) => ok_json(json!({ "speedtests": rows })),
            Err(e) => error_response(e),
        },
        Some(kind @ ("ports" | "discovery")) => match state.db.latest_scan(&id, kind) {
            // `null` et non un scan vide : « jamais lancé » et « lancé, rien
            // trouvé » appellent deux phrases différentes à l'écran.
            Ok(scan) => ok_json(json!({ "scan": scan })),
            Err(e) => error_response(e),
        },
        _ => fail(StatusCode::BAD_REQUEST, "genre attendu : ports, discovery ou speedtest"),
    }
}

/// Relevés bruts par cible, pour un export au format de l'application.
///
/// ⚠️ Rendus **bruts**, pas agrégés. Le rapport de l'application ne se
/// contente pas de moyennes : il liste les coupures avec leur durée et rend la
/// série complète. Envoyer un résumé obligerait le hub à recalculer ce que
/// l'application sait déjà faire, et les deux finiraient par diverger sur la
/// définition d'une coupure.
async fn probe_sla_samples(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> Response {
    let probe = match state.db.get_probe(&id) {
        Ok(probe) => probe,
        Err(e) => return error_response(e),
    };
    let (window, range) = flux_range(&query);
    let bucket = state.settings.influx_bucket();
    let flux = format!(
        "from(bucket: {})\n  |> range({})\n  \
         |> filter(fn: (r) => r[\"probe_id\"] == {})\n  \
         |> filter(fn: (r) => r[\"_measurement\"] == \"ping_latency\")",
        flux_string(&bucket),
        window,
        flux_string(&id),
    );
    let csv = match state.influx.query_flux(&flux).await {
        Ok(csv) => csv,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, &e),
    };

    // L'accès internet est une ligne du rapport au même titre qu'une cible :
    // un client qui lit « 99,9 % sur la passerelle » veut aussi savoir si le
    // lien vers l'extérieur tenait pendant ce temps-là.
    let internet_flux = format!(
        "from(bucket: {})\n  |> range({})\n  \
         |> filter(fn: (r) => r[\"probe_id\"] == {})\n  \
         |> filter(fn: (r) => r[\"_measurement\"] == \"internet_status\")\n  \
         |> filter(fn: (r) => r[\"_field\"] == \"state\" or r[\"_field\"] == \"icmp_ms\")",
        flux_string(&bucket),
        window,
        flux_string(&id),
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

    let speedtests = state.db.speedtest_history(&id, 500).unwrap_or_default();
    // Les inventaires du rapport : ce que la sonde a vu sur le réseau, et les
    // ports ouverts qu'elle y a relevés. Un client qui reçoit un SLA veut
    // aussi savoir ce qui tournait chez lui.
    let discovery = state.db.latest_scan(&id, "discovery").ok().flatten();
    let ports = state.db.latest_scan(&id, "ports").ok().flatten();

    ok_json(json!({
        "probe": probe.name,
        "site": probe.site_name,
        "range": range,
        "start": query.start,
        "stop": query.stop,
        "generated_at": crate::db::now(),
        "targets": samples_from_flux_csv(&csv),
        "internet": internet,
        "speedtests": speedtests,
        "discovery": discovery,
        "ports": ports,
    }))
}

/// Relevés d'accès internet, ramenés à la même forme que les cibles ICMP.
///
/// ⚠️ `state` est un texte (`online` / `limited` / `offline`), pas un booléen.
/// Le convertir en « vivant / mort » perdrait `limited`, qui est justement
/// l'état qu'un client conteste — « ça marchait » alors que rien ne passait.
fn internet_samples_from_flux_csv(csv: &str) -> Vec<serde_json::Value> {
    use std::collections::BTreeMap;

    let mut points: BTreeMap<i64, (Option<String>, Option<f64>)> = BTreeMap::new();
    let mut cols: Option<Vec<String>> = None;

    for line in csv.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') {
            continue;
        }
        if line.is_empty() {
            cols = None;
            continue;
        }
        let cells: Vec<&str> = line.split(',').collect();
        let Some(header) = cols.as_ref() else {
            cols = Some(cells.iter().map(|c| c.trim().to_string()).collect());
            continue;
        };
        let at = |name: &str| {
            header
                .iter()
                .position(|c| c == name)
                .and_then(|i| cells.get(i))
                .map(|s| s.trim())
        };
        let (Some(time), Some(field), Some(value)) = (at("_time"), at("_field"), at("_value"))
        else {
            continue;
        };
        let Some(ts) = rfc3339_to_millis(time) else {
            continue;
        };
        let slot = points.entry(ts / 1000).or_default();
        match field {
            "state" => slot.0 = Some(value.to_string()),
            "icmp_ms" => slot.1 = value.parse().ok(),
            _ => {}
        }
    }

    points
        .into_iter()
        .filter_map(|(ts, (state, latency_ms))| {
            state.map(|state| {
                json!({
                    "timestamp": ts,
                    "state": state,
                    // `alive` pour que le rapport puisse traiter internet
                    // comme une cible de plus ; `state` reste à côté pour
                    // distinguer « limité » de « hors ligne ».
                    "alive": state == "online",
                    "latency_ms": latency_ms,
                })
            })
        })
        .collect()
}

/// Relevés ICMP recollés par cible et par instant.
///
/// ⚠️ `alive` et `latency_ms` arrivent sur DEUX lignes distinctes du CSV
/// d'Influx. Les traiter séparément donnerait deux fois plus de relevés que
/// la sonde n'en a pris, et une disponibilité calculée sur cette base serait
/// fausse.
fn samples_from_flux_csv(csv: &str) -> Vec<serde_json::Value> {
    use std::collections::BTreeMap;

    let mut by_ip: BTreeMap<String, BTreeMap<i64, (Option<bool>, Option<f64>)>> = BTreeMap::new();
    let mut cols: Option<Vec<String>> = None;

    for line in csv.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') {
            continue;
        }
        if line.is_empty() {
            cols = None;
            continue;
        }
        let cells: Vec<&str> = line.split(',').collect();
        let Some(header) = cols.as_ref() else {
            cols = Some(cells.iter().map(|c| c.trim().to_string()).collect());
            continue;
        };
        let at = |name: &str| {
            header
                .iter()
                .position(|c| c == name)
                .and_then(|i| cells.get(i))
                .map(|s| s.trim())
        };
        let (Some(ip), Some(time), Some(field), Some(value)) =
            (at("ip"), at("_time"), at("_field"), at("_value"))
        else {
            continue;
        };
        let Some(ts) = rfc3339_to_millis(time) else {
            continue;
        };
        // En secondes : c'est l'unité des relevés de l'application, et le
        // rapport doit pouvoir mélanger les deux origines.
        let slot = by_ip.entry(ip.to_string()).or_default().entry(ts / 1000).or_default();
        match field {
            "alive" => slot.0 = Some(value == "true" || value == "1"),
            "latency_ms" => slot.1 = value.parse().ok(),
            _ => {}
        }
    }

    by_ip
        .into_iter()
        .map(|(ip, points)| {
            let samples: Vec<serde_json::Value> = points
                .into_iter()
                .map(|(ts, (alive, latency_ms))| {
                    json!({
                        "timestamp": ts,
                        // Pas de relevé `alive` : on retombe sur la présence
                        // d'une latence, faute de mieux, plutôt que d'écarter
                        // le point.
                        "alive": alive.unwrap_or(latency_ms.is_some()),
                        "latency_ms": latency_ms,
                    })
                })
                .collect();
            json!({ "ip": ip, "samples": samples })
        })
        .collect()
}

/// Rapport SLA d'une sonde, en CSV.
///
/// ⚠️ Calculé **depuis Influx**, pas depuis un cache : le SLA est une lecture
/// de l'historique, et un chiffre calculé une fois puis conservé se met à
/// mentir dès que la fenêtre change. C'est aussi pourquoi la fenêtre voyage
/// dans l'URL : le rapport doit dire sur quoi il porte.
async fn probe_sla_csv(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> Response {
    let probe = match state.db.get_probe(&id) {
        Ok(probe) => probe,
        Err(e) => return error_response(e),
    };
    let range = query.range.unwrap_or_else(|| "-24h".into());
    let bucket = state.settings.influx_bucket();
    let flux = format!(
        "from(bucket: {})\n  |> range(start: {})\n           |> filter(fn: (r) => r[\"probe_id\"] == {})\n           |> filter(fn: (r) => r[\"_measurement\"] == \"ping_latency\")",
        flux_string(&bucket),
        flux_duration(&range),
        flux_string(&id),
    );
    let csv = match state.influx.query_flux(&flux).await {
        Ok(csv) => csv,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, &e),
    };

    let report = sla_from_flux_csv(&csv);
    let mut out = String::from(
        "sonde,site,fenetre,cible,disponibilite_pct,latence_moy_ms,\
         latence_min_ms,latence_max_ms,latence_p95_ms,releves,echecs\n",
    );
    for row in &report {
        out.push_str(&format!(
            "{},{},{},{},{:.2},{},{},{},{},{},{}\n",
            csv_cell(&probe.name),
            csv_cell(&probe.site_name),
            csv_cell(&range),
            csv_cell(&row.ip),
            row.uptime_pct,
            row.avg_latency_ms.map(|v| format!("{v:.1}")).unwrap_or_default(),
            row.min_latency_ms.map(|v| v.to_string()).unwrap_or_default(),
            row.max_latency_ms.map(|v| v.to_string()).unwrap_or_default(),
            row.p95_latency_ms.map(|v| v.to_string()).unwrap_or_default(),
            row.total_samples,
            row.failed_samples,
        ));
    }

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"sla-{}-{}.csv\"",
                    slug(&probe.name),
                    range.trim_start_matches('-')
                ),
            ),
        ],
        out,
    )
        .into_response()
}

/// Échappe une cellule CSV. Un nom de sonde peut contenir une virgule ; sans
/// ça, une seule sonde mal nommée décale toutes les colonnes du fichier.
fn csv_cell(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Nom de fichier sûr, sans espace ni accent problématique.
fn slug(value: &str) -> String {
    let s: String = value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_lowercase();
    if s.is_empty() { "sonde".into() } else { s }
}

/// Agrège les relevés ICMP d'Influx en un SLA par cible.
///
/// ⚠️ La disponibilité se calcule sur le champ `alive`, jamais sur la présence
/// d'une latence : un hôte qui ne répond pas n'écrit pas de latence, et
/// compter les latences ferait un uptime de 100 % sur un hôte mort.
fn sla_from_flux_csv(csv: &str) -> Vec<lanprobe_core::sla::SlaStats> {
    use std::collections::BTreeMap;

    // Par cible, puis par instant : `alive` et `latency_ms` sont deux lignes
    // distinctes du CSV et doivent être recollées avant d'être comptées.
    let mut by_ip: BTreeMap<String, BTreeMap<String, (Option<bool>, Option<u64>)>> =
        BTreeMap::new();
    let mut cols: Option<Vec<String>> = None;

    for line in csv.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') {
            continue;
        }
        if line.is_empty() {
            cols = None;
            continue;
        }
        let cells: Vec<&str> = line.split(',').collect();
        let Some(header) = cols.as_ref() else {
            cols = Some(cells.iter().map(|c| c.trim().to_string()).collect());
            continue;
        };
        let at = |name: &str| {
            header
                .iter()
                .position(|c| c == name)
                .and_then(|i| cells.get(i))
                .map(|s| s.trim())
        };
        let (Some(ip), Some(time), Some(field), Some(value)) =
            (at("ip"), at("_time"), at("_field"), at("_value"))
        else {
            continue;
        };
        let slot = by_ip
            .entry(ip.to_string())
            .or_default()
            .entry(time.to_string())
            .or_default();
        match field {
            "alive" => slot.0 = Some(value == "true" || value == "1"),
            "latency_ms" => slot.1 = value.parse::<f64>().ok().map(|v| v.round() as u64),
            _ => {}
        }
    }

    by_ip
        .into_iter()
        .map(|(ip, points)| {
            let samples: Vec<lanprobe_core::sla::PingSample> = points
                .into_values()
                .map(|(alive, latency_ms)| lanprobe_core::sla::PingSample {
                    alive: alive.unwrap_or(latency_ms.is_some()),
                    latency_ms,
                })
                .collect();
            lanprobe_core::sla::compute_sla(&ip, &samples)
        })
        .collect()
}

/// Commandes que le hub accepte de faire exécuter à une sonde.
///
/// ⚠️ Liste **fermée**, et pas par prudence excessive : une commande part de
/// l'intérieur du LAN surveillé. Accepter un `kind` arbitraire reviendrait à
/// offrir l'exécution de n'importe quoi sur le réseau d'un client à qui
/// obtiendrait un compte opérateur.
const COMMAND_KINDS: &[&str] = &[
    "speedtest",
    "port_scan",
    "discovery",
    "add_monitor",
    "remove_monitor",
];

#[derive(Deserialize)]
struct CommandBody {
    kind: String,
    #[serde(default)]
    args: serde_json::Value,
}

/// Empile une commande pour une sonde (contrat § 14).
///
/// ⚠️ Journalisée avec son auteur : un scan de ports lancé d'ici part de
/// l'intérieur du réseau d'un client. On doit pouvoir dire qui l'a demandé
/// des mois après, même si le compte a été désactivé depuis.
async fn create_command(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<CommandBody>,
) -> Response {
    let kind = body.kind.trim();
    if !COMMAND_KINDS.contains(&kind) {
        return fail(StatusCode::BAD_REQUEST, "commande inconnue");
    }
    let args = if body.args.is_null() {
        json!({})
    } else if body.args.is_object() {
        body.args.clone()
    } else {
        return fail(StatusCode::BAD_REQUEST, "les arguments doivent être un objet");
    };
    // Refusé ici plutôt qu'à l'arrivée : une commande vouée à l'échec
    // occuperait la file, serait remise trois fois, et l'opérateur lirait
    // « échouée » une minute plus tard pour une faute de saisie.
    if kind == "speedtest"
        && args.get("engine").and_then(|v| v.as_str()) == Some("iperf3")
        && args
            .get("server")
            .and_then(|v| v.as_str())
            .is_none_or(|v| v.trim().is_empty())
    {
        return fail(StatusCode::BAD_REQUEST, "iperf3 demande un serveur");
    }

    let probe = match state.db.get_probe(&id) {
        Ok(probe) => probe,
        Err(e) => return error_response(e),
    };
    let command_id = match state.db.enqueue_command(&id, kind, &args, &actor.username) {
        Ok(command_id) => command_id,
        Err(e) => return error_response(e),
    };
    audit(
        &state,
        Some(&actor.username),
        "probe.command",
        Some(&id),
        Outcome::Success,
        Some(&format!("{kind} sur « {} »", probe.name)),
    );
    // La sonde ne l'exécutera qu'à son prochain battement : le dire évite
    // qu'on croie la commande perdue pendant la minute qui suit.
    ok_json(json!({
        "id": command_id,
        "kind": kind,
        "state": "pending",
        "heartbeat_interval_secs": state.settings.heartbeat_interval_secs(),
    }))
}

/// Historique récent des commandes d'une sonde.
async fn list_commands(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.db.list_commands(&id, 50) {
        Ok(rows) => ok_json(json!({ "commands": rows })),
        Err(e) => error_response(e),
    }
}

async fn rotate_token(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
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
    // Le jeton frappé ne va pas au journal : seul le fait de la rotation.
    match audited(
        &state,
        Some(&actor.username),
        "probe.rotate",
        Some(&id),
        state.db.stage_token_rotation(&id, &token.auth_id, &token.token),
    ) {
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
async fn revoke_token_now(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
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
    let (retired, version) = match audited(
        &state,
        Some(&actor.username),
        "probe.revoke_token",
        Some(&id),
        state.db.replace_token_immediately(&id, &token.auth_id, &token.token),
    ) {
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
    /// Bornes explicites, en secondes epoch. Prennent le pas sur `range`.
    ///
    /// ⚠️ Un rapport de SLA porte sur une période convenue — « du 1er au 31 »
    /// — pas sur « les sept derniers jours ». Une fenêtre glissante donnerait
    /// un chiffre différent à chaque ouverture du document.
    #[serde(default)]
    start: Option<i64>,
    #[serde(default)]
    stop: Option<i64>,
}

/// Construit la clause `range(...)` de Flux et le libellé de la période.
fn flux_range(query: &MetricsQuery) -> (String, String) {
    match (query.start, query.stop) {
        (Some(start), Some(stop)) if stop > start => (
            format!("start: {start}, stop: {stop}"),
            format!("{start}..{stop}"),
        ),
        _ => {
            let range = query.range.clone().unwrap_or_else(|| "-24h".into());
            (format!("start: {}", flux_duration(&range)), range)
        }
    }
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
        Ok(csv) => ok_json(serde_json::json!({
            "range": range,
            "series": series_from_flux_csv(&csv),
        })),
        Err(e) => fail(StatusCode::BAD_GATEWAY, &e),
    }
}

/// Insère `probe_id` et `site_id` dans le jeu d'étiquettes de chaque ligne.
///
/// Le line protocol place les étiquettes entre le nom de mesure et le premier
/// espace non échappé : `mesure,tag=v champ=1 horodatage`. On les ajoute donc
/// juste avant cet espace. Une ligne malformée est laissée telle quelle —
/// Influx la refusera et le dira, ce qui vaut mieux qu'un silence.
fn tag_lines(body: &str, probe_id: &str, site_id: &str) -> String {
    let extra = format!(",probe_id={probe_id},site_id={site_id}");
    let mut out = String::with_capacity(body.len() + body.lines().count() * extra.len());
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Premier espace non précédé d'une barre oblique inverse : la fin du
        // bloc « mesure + étiquettes ».
        let split = line
            .char_indices()
            .find(|(i, c)| *c == ' ' && !line[..*i].ends_with('\\'))
            .map(|(i, _)| i);
        match split {
            Some(i) => {
                out.push_str(&line[..i]);
                out.push_str(&extra);
                out.push_str(&line[i..]);
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Convertit le CSV annoté d'InfluxDB en séries JSON.
///
/// Le navigateur ne doit pas apprendre à lire le CSV annoté : c'est un format
/// versionné, propre à Influx, dont les colonnes d'annotation changent d'une
/// version à l'autre. Le hub proxifie déjà la requête pour garder le jeton de
/// lecture côté serveur — il isole aussi l'interface du format de sortie, sans
/// quoi une montée de version d'Influx casserait les graphes.
///
/// Sortie : `[{ "field": "latency_ms", "points": [{ "t": <ms>, "v": <f64> }] }]`
fn series_from_flux_csv(csv: &str) -> Vec<serde_json::Value> {
    use std::collections::BTreeMap;

    // Colonnes de service d'Influx : tout le reste est une étiquette.
    const INTERNAL: &[&str] = &[
        "", "result", "table", "_start", "_stop", "_time", "_value", "_field", "_measurement",
    ];

    // Clé = (champ, étiquettes triées). Sans les étiquettes, deux IP surveillées
    // en parallèle s'écrasaient dans une seule courbe : les points
    // s'entrelaçaient et le graphe montrait une latence qui n'existe nulle part.
    let mut series: BTreeMap<(String, Vec<(String, String)>), Vec<serde_json::Value>> =
        BTreeMap::new();
    let mut cols: Option<Vec<String>> = None;

    for line in csv.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') {
            continue;
        }
        if line.is_empty() {
            cols = None;
            continue;
        }
        let cells: Vec<&str> = line.split(',').collect();
        let Some(header) = cols.as_ref() else {
            cols = Some(cells.iter().map(|c| c.trim().to_string()).collect());
            continue;
        };
        let at = |name: &str| {
            header
                .iter()
                .position(|c| c == name)
                .and_then(|i| cells.get(i))
                .map(|s| s.trim())
        };
        let (Some(time), Some(value)) = (at("_time"), at("_value")) else {
            continue;
        };
        // Une valeur vide est une absence de mesure, pas un zéro : l'écrire
        // comme 0 dessinerait une chute qui n'a jamais eu lieu.
        //
        // Les booléens (`alive`, `icmp_ok`…) deviennent 1/0 : ce sont des
        // séries qu'on veut tracer, et savoir si l'hôte répondait est
        // l'information la plus utile d'un ping.
        //
        // Les champs texte (`state` d'`internet_status`) sont rendus tels
        // quels, pas écartés : ils n'ont pas de place sur un axe, mais la
        // bande d'état de l'onglet sonde ne trace justement pas — elle
        // colorie des intervalles nommés. Les écarter laissait cette bande
        // définitivement vide.
        let v = match value {
            "" => continue,
            "true" => serde_json::Value::from(1.0),
            "false" => serde_json::Value::from(0.0),
            other => match other.parse::<f64>() {
                Ok(n) => serde_json::Value::from(n),
                Err(_) => serde_json::Value::from(other),
            },
        };
        let Some(t) = rfc3339_to_millis(time) else {
            continue;
        };
        let field = at("_field").unwrap_or("value").to_string();

        // `probe_id` et `site_id` ne distinguent rien ici — la requête porte
        // déjà sur une seule sonde. Les garder créerait une étiquette identique
        // sur toutes les courbes, illisible dans une légende.
        let mut tags: Vec<(String, String)> = header
            .iter()
            .enumerate()
            .filter(|(_, name)| {
                !INTERNAL.contains(&name.as_str())
                    && !matches!(name.as_str(), "probe_id" | "site_id" | "probe" | "site" | "host")
            })
            .filter_map(|(i, name)| {
                cells
                    .get(i)
                    .map(|v| v.trim())
                    .filter(|v| !v.is_empty())
                    .map(|v| (name.clone(), v.to_string()))
            })
            .collect();
        tags.sort();

        series
            .entry((field, tags))
            .or_default()
            .push(serde_json::json!({ "t": t, "v": v }));
    }

    series
        .into_iter()
        .map(|((field, tags), points)| {
            // Le libellé porte ce qui distingue la courbe : « latency_ms »
            // seule quand il n'y a qu'une cible, « latency_ms · 1.1.1.1 »
            // quand plusieurs sont surveillées en parallèle.
            let suffix = tags
                .iter()
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
                .join(" · ");
            let label = if suffix.is_empty() {
                field.clone()
            } else {
                format!("{field} · {suffix}")
            };
            let tag_map: serde_json::Map<String, serde_json::Value> = tags
                .into_iter()
                .map(|(k, v)| (k, serde_json::Value::String(v)))
                .collect();
            serde_json::json!({
                "field": label,
                "measure": field,
                "tags": tag_map,
                "points": points,
            })
        })
        .collect()
}

/// RFC3339 → millisecondes epoch, sans dépendance de date : Influx rend
/// toujours de l'UTC (`…Z`) sur ces requêtes.
fn rfc3339_to_millis(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let frac = s
        .split_once('.')
        .and_then(|(_, rest)| {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let ms: String = digits.chars().chain("000".chars()).take(3).collect();
            ms.parse::<i64>().ok()
        })
        .unwrap_or(0);

    // Jours écoulés depuis l'époque — algorithme des jours civils de Howard
    // Hinnant, valable pour toute date grégorienne.
    let y_adj = if mo <= 2 { y - 1 } else { y };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = y_adj - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    Some(((days * 86_400 + h * 3600 + mi * 60 + sec) * 1000) + frac)
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

// ── Sauvegarde et restauration ─────────────────────────────────────────────

/// Plafond du téléversement d'une archive. Généreux — une archive avec ses
/// séries pèse lourd — mais fini : la route est réservée aux administrateurs,
/// pas ouverte, et remplir le volume du hub reste un déni de service.
const MAX_UPLOAD_BYTES: usize = 16 * 1024 * 1024 * 1024;

/// Traduit une erreur de sauvegarde en code HTTP.
///
/// Tout écraser en 500 rendrait l'interface incapable de distinguer « ce
/// fichier n'est pas une archive » (l'utilisateur s'est trompé de fichier) de
/// « le disque est plein » (le hub a un problème). Ce sont deux écrans
/// différents.
fn backup_status(e: &crate::backup::BackupError) -> StatusCode {
    use crate::backup::BackupError::*;
    match e {
        NotAnArchive(_) | Corrupt(_) => StatusCode::BAD_REQUEST,
        SchemaTooNew { .. } | ConfirmationRequired(_) => StatusCode::CONFLICT,
        Io(_) | Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn backup_error(e: crate::backup::BackupError) -> Response {
    let status = backup_status(&e);
    (status, Json(json!({ "error": e.to_string() }))).into_response()
}

/// La cible InfluxDB, ou `None` s'il n'y a pas de quoi l'appeler. Sans jeton
/// opérateur `influx backup` échouerait de toute façon ; le dire à l'avance
/// donne un message utile plutôt qu'un code de sortie.
fn influx_target(state: &AppState) -> Option<crate::backup::InfluxTarget> {
    state
        .influx
        .has_operator_token()
        .then(|| state.influx.backup_target(state.influx_cli.clone()))
}

/// Sauvegarde immédiate, puis application de la rétention.
///
/// Elle existe pour le geste « je sauvegarde avant de faire quelque chose de
/// risqué ». Le fonctionnement normal est la sauvegarde planifiée : personne
/// ne doit avoir à penser à cliquer ici.
async fn create_backup(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
) -> Response {
    // `spawn_blocking` et non `block_in_place` : la sauvegarde fait du
    // VACUUM, du SHA-256 et de la compression — plusieurs secondes de calcul
    // synchrone qui bloqueraient le fil d'exécution asynchrone, donc toutes
    // les autres requêtes du hub.
    let (db, config_dir, backup_dir, influx) = (
        state.db.clone(),
        state.config_dir.clone(),
        state.backup_dir.clone(),
        influx_target(&state),
    );
    let report = tokio::task::spawn_blocking(move || {
        crate::backup::create(crate::backup::BackupRequest {
            db: &db,
            config_dir: &config_dir,
            backup_dir: &backup_dir,
            influx,
            now: crate::backup::now(),
        })
    })
    .await;
    let report = match report {
        Ok(report) => report,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let report = match report {
        Ok(report) => report,
        Err(e) => {
            audit(
                &state,
                Some(&actor.username),
                "backup.create",
                None,
                Outcome::Failure,
                Some(&e.to_string()),
            );
            return backup_error(e);
        }
    };
    audit(
        &state,
        Some(&actor.username),
        "backup.create",
        Some(&report.file),
        Outcome::Success,
        Some(&format!(
            "{} octets, InfluxDB {}",
            report.bytes,
            if report.influx_included { "inclus" } else { "absent" }
        )),
    );

    let pruned = apply_retention(&state, Some(&actor.username));

    ok_json(json!({
        "file": report.file,
        "bytes": report.bytes,
        "created_at": report.created_at,
        "hub_version": report.hub_version,
        "schema_version": report.schema_version,
        "influx_included": report.influx_included,
        "influx_error": report.influx_error,
        "pruned": pruned,
    }))
}

/// Retire les archives au-delà de `backup_keep_last`. Chaque retrait est
/// journalisé : c'est le seul endroit du hub qui supprime un fichier, il n'a
/// pas le droit de le faire en silence.
fn apply_retention(state: &AppState, actor: Option<&str>) -> Vec<String> {
    let keep = state.settings.backup_keep_last();
    match crate::backup::prune(&state.backup_dir, keep) {
        Ok(removed) if removed.is_empty() => removed,
        Ok(removed) => {
            audit(
                state,
                actor,
                "backup.retention",
                None,
                Outcome::Success,
                Some(&format!(
                    "{} archive(s) retirée(s) au-delà de {keep} : {}",
                    removed.len(),
                    removed.join(", ")
                )),
            );
            removed
        }
        Err(e) => {
            audit(
                state,
                actor,
                "backup.retention",
                None,
                Outcome::Failure,
                Some(&e.to_string()),
            );
            Vec::new()
        }
    }
}

async fn list_backups(State(state): State<AppState>) -> Response {
    let archives = match crate::backup::list(&state.backup_dir) {
        Ok(archives) => archives,
        Err(e) => return backup_error(e),
    };
    ok_json(json!({
        "dir": state.backup_dir.to_string_lossy(),
        "enabled": state.settings.backup_enabled(),
        "interval_hours": state.settings.backup_interval_hours(),
        "keep_last": state.settings.backup_keep_last(),
        "backups": archives,
    }))
}

#[derive(Deserialize, Default)]
struct RestoreBody {
    #[serde(default)]
    confirm_overwrite: bool,
}

/// Restaure une archive **déjà présente** dans le répertoire de sauvegarde.
/// Le chemin de ligne de commande, pour qui a déposé son fichier dans le
/// volume plutôt que de le téléverser.
async fn restore_named_backup(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(file): Path<String>,
    body: Option<Json<RestoreBody>>,
) -> Response {
    // `{file}` nomme une archive DANS le répertoire de sauvegarde. Un nom qui
    // en sort n'a pas à être ouvert, quel que soit ce qu'il désigne.
    if !is_plain_file_name(&file) {
        audit(
            &state,
            Some(&actor.username),
            "backup.restore",
            Some(&file),
            Outcome::Failure,
            Some("nom d'archive refusé"),
        );
        return fail(
            StatusCode::BAD_REQUEST,
            "nom d'archive invalide : attendu le nom d'un fichier du répertoire de sauvegarde",
        );
    }
    let confirm = body.map(|Json(b)| b.confirm_overwrite).unwrap_or(false);
    let archive = state.backup_dir.join(&file);
    run_restore(&state, &actor, archive, &file, confirm).await
}

/// Restaure une archive **téléversée**. C'est le chemin principal : on remet
/// le fichier compressé, et le hub se débrouille.
///
/// ⚠️ Une archive est un fichier de confiance : elle porte `secret.key` et
/// remplace la base. Le manifeste est vérifié avant tout, et le corps est
/// écrit au fil de l'eau dans le répertoire de sauvegarde plutôt que gardé en
/// mémoire — une archive avec ses séries n'y tiendrait pas.
async fn restore_uploaded_backup(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    let mut confirm = false;
    let mut uploaded: Option<std::path::PathBuf> = None;

    if let Err(e) = std::fs::create_dir_all(&state.backup_dir) {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let temporary = state
        .backup_dir
        .join(format!(".televerse-{}.zip", crate::backup::now()));

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(e) => {
                let _ = std::fs::remove_file(&temporary);
                return fail(StatusCode::BAD_REQUEST, &format!("envoi illisible : {e}"));
            }
        };
        match field.name().unwrap_or_default() {
            "confirm_overwrite" => {
                confirm = field.text().await.map(|v| v.trim() == "true").unwrap_or(false);
            }
            "archive" => {
                let mut field = field;
                let file = match std::fs::File::create(&temporary) {
                    Ok(file) => file,
                    Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
                };
                let mut file = std::io::BufWriter::new(file);
                loop {
                    match field.chunk().await {
                        Ok(Some(chunk)) => {
                            if let Err(e) = std::io::Write::write_all(&mut file, &chunk) {
                                let _ = std::fs::remove_file(&temporary);
                                return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            let _ = std::fs::remove_file(&temporary);
                            return fail(
                                StatusCode::BAD_REQUEST,
                                &format!("envoi interrompu : {e}"),
                            );
                        }
                    }
                }
                if let Err(e) = std::io::Write::flush(&mut file) {
                    return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
                }
                uploaded = Some(temporary.clone());
            }
            _ => {}
        }
    }

    let Some(archive) = uploaded else {
        return fail(
            StatusCode::BAD_REQUEST,
            "aucune archive dans l'envoi : champ « archive » attendu",
        );
    };
    let response = run_restore(&state, &actor, archive.clone(), "archive téléversée", confirm).await;

    // L'archive téléversée reste dans le répertoire de sauvegarde, sous son
    // nom canonique quand il est libre. Rien ne se supprime sur ce projet, et
    // celle qui vient de servir est précisément celle qu'on veut garder.
    keep_uploaded_archive(&state, &archive);
    response
}

fn keep_uploaded_archive(state: &AppState, archive: &std::path::Path) {
    let manifest = match crate::backup::inspect(archive) {
        Ok(manifest) => manifest,
        Err(_) => {
            // Ce n'est pas une archive LanProbe : ce fichier n'a rien à faire
            // dans le répertoire des sauvegardes. Le laisser le remplirait
            // sans que personne ne s'en aperçoive — `list` ne le montre pas,
            // son nom ne correspondant à aucune archive. On ne retire ici que
            // ce que cette requête vient elle-même d'écrire.
            let _ = std::fs::remove_file(archive);
            return;
        }
    };
    let name = format!(
        "lanprobe_backup_v{}_{}.zip",
        manifest.hub_version,
        manifest.created_at.replace(['-', ':'], ".").replace('T', "_").replace('Z', "")
    );
    let destination = state.backup_dir.join(&name);
    if !destination.exists() {
        let _ = std::fs::rename(archive, destination);
    }
}

async fn run_restore(
    state: &AppState,
    actor: &Identity,
    archive: std::path::PathBuf,
    label: &str,
    confirm_overwrite: bool,
) -> Response {
    let (config_dir, influx) = (state.config_dir.clone(), influx_target(state));
    let outcome = tokio::task::spawn_blocking(move || {
        crate::backup::restore(crate::backup::RestoreRequest {
            archive: &archive,
            config_dir: &config_dir,
            confirm_overwrite,
            influx,
            now: crate::backup::now(),
        })
    })
    .await;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    match outcome {
        Ok(report) => {
            audit(
                state,
                Some(&actor.username),
                "backup.restore",
                Some(label),
                Outcome::Success,
                Some(&format!(
                    "archive du {} (hub {}, schéma {})",
                    report.created_at, report.hub_version, report.schema_version
                )),
            );
            tracing::warn!(
                "base restaurée depuis {label} — le hub sert encore l'ancienne \
                 base tant qu'il n'a pas redémarré"
            );
            // Rendu par `/api/status` : l'avertissement doit survivre au
            // rechargement de la page. Sans ça, le hub refuse d'écrire
            // (« readonly database ») sans que rien à l'écran ne l'explique.
            state
                .restart_required
                .store(true, std::sync::atomic::Ordering::Relaxed);
            ok_json(json!({
                "ok": true,
                "hub_version": report.hub_version,
                "schema_version": report.schema_version,
                "created_at": report.created_at,
                "restored": report.restored,
                "moved_aside": report.moved_aside.map(|p| p.to_string_lossy().into_owned()),
                "influx_restored": report.influx_restored,
                "influx_error": report.influx_error,
                "restart_required": report.restart_required,
            }))
        }
        Err(e) => {
            // Un refus de restauration est exactement la ligne qu'un journal
            // doit montrer : quelqu'un a tenté de remplacer la base.
            audit(
                state,
                Some(&actor.username),
                "backup.restore",
                Some(label),
                Outcome::Failure,
                Some(&e.to_string()),
            );
            backup_error(e)
        }
    }
}

/// Un nom de fichier nu : ni séparateur, ni remontée, ni chemin absolu.
fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

// ── Réglages ───────────────────────────────────────────────────────────────

async fn get_settings(State(state): State<AppState>) -> Response {
    ok_json(state.settings.all())
}

async fn put_settings(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
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
        // La rétention a son action propre : c'est le seul réglage dont le
        // changement efface des mesures, et on doit pouvoir le retrouver sans
        // relire tout le journal.
        let action = if key == crate::settings::keys::RETENTION_DAYS {
            "settings.retention"
        } else {
            "settings.update"
        };
        // `settings` ne contient aucun secret, par construction : la valeur
        // peut donc figurer au journal, et c'est ce qui rend la ligne utile.
        let written = state.settings.put(key, &value, confirm);
        match &written {
            Ok(()) => audit(
                &state,
                Some(&actor.username),
                action,
                Some(key),
                Outcome::Success,
                Some(&value),
            ),
            Err(e) => audit(
                &state,
                Some(&actor.username),
                action,
                Some(key),
                Outcome::Failure,
                Some(&e.to_string()),
            ),
        }
        if let Err(e) = written {
            return error_response(e);
        }
        influx_touched |= key.starts_with("influx_") || key == crate::settings::keys::RETENTION_DAYS;
        // Le mode de service (clair ou TLS) est lu **au démarrage**. Le
        // signaler ici est la seule façon pour l'interface de dire « c'est
        // enregistré, mais ça ne s'appliquera qu'au redémarrage » — sans
        // quoi l'utilisateur enregistre, recharge en `https://`, tombe sur
        // rien, et croit avoir cassé son hub.
        if key == crate::settings::keys::TLS_ENABLED {
            state
                .restart_required
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
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
    ok_json(state.settings.all())
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

async fn put_advertise(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Json(body): Json<AdvertiseBody>,
) -> Response {
    match audited(
        &state,
        Some(&actor.username),
        "settings.advertise",
        Some(&body.url),
        state
            .settings
            .put(crate::settings::keys::INFLUX_ADVERTISE_URL, &body.url, false),
    ) {
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

async fn influx_overview(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // L'utilisateur ne doit jamais avoir à administrer Influx, mais il doit
    // pouvoir aller voir ses données. Aucun jeton dans cette réponse.
    let health = state.influx.health().await;
    // Deux adresses, et les confondre coûte cher : `url` est celle que le hub
    // emprunte depuis l'intérieur du conteneur (`127.0.0.1`), inutilisable
    // depuis Grafana. `grafana_url` est celle à coller dans Grafana — même
    // hôte public que le hub, port d'Influx.
    let (grafana_url, source) = state.influx.resolve_advertise_url(host_header(&headers).as_deref());
    ok_json(json!({
        "url": state.settings.influx_url(),
        "grafana_url": grafana_url,
        "grafana_url_source": source.as_str(),
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
    Extension(actor): Extension<Identity>,
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
    // On journalise l'identifiant d'autorisation et la description, jamais
    // le jeton : il part dans un Grafana, il ne doit pas rester ici.
    if let Err(e) = audited(
        &state,
        Some(&actor.username),
        "influx.read_token.create",
        Some(&minted.auth_id),
        state.db.record_read_token(&minted.auth_id, &description),
    ) {
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

/// Les enrôlements encore ouverts, pour que l'interface les affiche comme des
/// lignes en attente dans la liste des sondes du site, avec leur compte à
/// rebours. Le code lui-même n'y figure pas : il est haché en base.
async fn list_pending_codes(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
) -> Response {
    match state.db.pending_enroll_codes(crate::db::now()) {
        // ⚠️ Ces lignes portent le code d'enrôlement EN CLAIR. Un code de la
        // baie d'un autre client suffirait à y poser une sonde : le filtre de
        // portée n'est pas cosmétique ici, il est le contrôle.
        Ok(rows) => {
            let visible: Vec<_> = rows
                .into_iter()
                .filter(|r| actor.scope.allows(&r.site_id))
                .collect();
            ok_json(serde_json::to_value(visible).unwrap_or(serde_json::Value::Null))
        }
        Err(e) => error_response(e),
    }
}

async fn revoke_read_token(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
    if let Err(e) = audited(
        &state,
        Some(&actor.username),
        "influx.read_token.revoke",
        Some(&id),
        state.db.revoke_read_token(&id),
    ) {
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
    use crate::db::Role;
    use crate::influx::testing::{FakeInflux, ENV_LOCK, OPERATOR_TOKEN};
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const PASSWORD: &str = "password123";

    /// CSV annoté tel qu'InfluxDB 2 le rend : trois lignes d'annotation, un
    /// en-tête, puis les données. Deux tables séparées par une ligne vide,
    /// parce que c'est le cas qui casse un parseur naïf.
    const FLUX_CSV: &str = "\
#datatype,string,long,dateTime:RFC3339,dateTime:RFC3339,dateTime:RFC3339,double,string,string
#group,false,false,true,true,false,false,true,true
#default,_result,,,,,,,
,result,table,_start,_stop,_time,_value,_field,_measurement
,,0,2026-08-27T00:00:00Z,2026-08-27T01:00:00Z,2026-08-27T00:00:00Z,12.5,latency_ms,ping
,,0,2026-08-27T00:00:00Z,2026-08-27T01:00:00Z,2026-08-27T00:01:00.250Z,13,latency_ms,ping

#datatype,string,long,dateTime:RFC3339,double,string
,result,table,_time,_value,_field
,,1,2026-08-27T00:00:00Z,1,alive
";

    #[test]
    fn flux_csv_becomes_json_series() {
        let series = series_from_flux_csv(FLUX_CSV);
        assert_eq!(series.len(), 2, "un champ = une série : {series:?}");

        let latency = series.iter().find(|s| s["field"] == "latency_ms").unwrap();
        let points = latency["points"].as_array().unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0]["t"], 1_787_788_800_000i64);
        assert_eq!(points[0]["v"], 12.5);
        // Les millisecondes de l'horodatage ne doivent pas être perdues.
        assert_eq!(points[1]["t"], 1_787_788_860_250i64);

        assert!(series.iter().any(|s| s["field"] == "alive"));
    }

    #[test]
    fn flux_csv_skips_empty_values_instead_of_writing_zero() {
        // Une mesure absente n'est pas une mesure à zéro : l'écrire comme 0
        // dessinerait une chute qui n'a jamais eu lieu.
        let csv = ",result,table,_time,_value,_field\n\
                   ,,0,2026-08-27T00:00:00Z,,latency_ms\n\
                   ,,0,2026-08-27T00:00:01Z,7,latency_ms\n";
        let series = series_from_flux_csv(csv);
        let points = series[0]["points"].as_array().unwrap();
        assert_eq!(points.len(), 1, "la ligne vide doit être ignorée");
        assert_eq!(points[0]["v"], 7.0);
    }

    #[test]
    fn flux_csv_turns_booleans_into_one_and_zero() {
        // `alive` est la série la plus utile d'un ping : la perdre parce
        // qu'elle n'est pas numérique viderait le graphe de son intérêt.
        let csv = ",result,table,_time,_value,_field\n\
                   ,,0,2026-08-27T00:00:00Z,true,alive\n\
                   ,,0,2026-08-27T00:00:01Z,false,alive\n\
                   ,,0,2026-08-27T00:00:02Z,dégradé,state\n";
        let series = series_from_flux_csv(csv);
        let alive = series.iter().find(|s| s["field"] == "alive").unwrap();
        let points = alive["points"].as_array().unwrap();
        assert_eq!(points[0]["v"], 1.0);
        assert_eq!(points[1]["v"], 0.0);
    }

    #[test]
    fn flux_csv_keeps_text_values_instead_of_dropping_them() {
        // `state` d'`internet_status` est du texte : il n'a pas de place sur
        // un axe, mais la bande d'état de l'onglet sonde ne trace pas — elle
        // colorie des intervalles nommés. L'écarter la laissait vide en
        // permanence, alors qu'Influx contenait des milliers de points.
        let csv = ",result,table,_time,_value,_field\n\
                   ,,0,2026-08-27T00:00:00Z,online,state\n\
                   ,,0,2026-08-27T00:00:01Z,offline,state\n";
        let series = series_from_flux_csv(csv);
        let points = series[0]["points"].as_array().unwrap();
        assert_eq!(points[0]["v"], "online");
        assert_eq!(points[1]["v"], "offline");
    }

    #[test]
    fn a_tagged_series_still_names_its_bare_field() {
        // Le libellé se décore de l'étiquette (`latency_ms · 10.0.30.12`)
        // pour distinguer deux cibles pinguées en parallèle. L'interface, elle,
        // cherche ses points par le nom NU du champ : si `measure` disparaît,
        // le graphique reste vide sans qu'aucune erreur ne soit levée — c'est
        // exactement ce qui est arrivé.
        let csv = ",result,table,_time,_value,_field,ip\n\
                   ,,0,2026-08-27T00:00:00Z,12.5,latency_ms,10.0.30.12\n";
        let series = series_from_flux_csv(csv);
        assert_eq!(series[0]["measure"], "latency_ms");
        assert_eq!(series[0]["field"], "latency_ms · 10.0.30.12");
    }

    #[test]
    fn flux_csv_without_data_is_an_empty_list_not_an_error() {
        assert!(series_from_flux_csv("").is_empty());
        assert!(series_from_flux_csv("#datatype,string\n").is_empty());
    }

    #[test]
    fn rfc3339_matches_known_epochs() {
        assert_eq!(rfc3339_to_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_to_millis("2000-03-01T00:00:00Z"), Some(951_868_800_000));
        // 2024 est bissextile : le 29 février doit exister.
        assert_eq!(rfc3339_to_millis("2024-02-29T12:00:00Z"), Some(1_709_208_000_000));
        assert_eq!(rfc3339_to_millis("pas une date"), None);
    }

    struct Harness {
        state: AppState,
        router: axum::Router,
        _fake: FakeInflux,
    }

    /// Volume jetable, propre à chaque test : la sauvegarde et la
    /// restauration écrivent réellement sur disque.
    fn scratch(tag: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "lanprobe-web-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    impl Harness {
        /// Hub complet devant un faux Influx local, **sans compte**.
        /// Pour un hub déjà installé, voir [`Harness::with_admin`].
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
            // Clé de scellement tirée en mémoire : les tests n'ont pas de
            // volume où poser un `secret.key`.
            let secrets = crate::secrets::Secrets::ephemeral(db.clone()).unwrap();
            let notifier =
                crate::notify::Notifier::new(db.clone(), secrets.clone(), settings.clone());
            let state = AppState {
                db,
                auth,
                settings,
                influx,
                notifier,
                secrets,
                ceremonies: std::sync::Arc::new(crate::passkeys::Ceremonies::new()),
                // Les tests jouent le cas courant : hub en clair derrière un
                // proxy. Le cookie `Secure` est vérifié via `X-Forwarded-Proto`.
                tls: false,
                config_dir: scratch("volume"),
                backup_dir: scratch("archives"),
                // Aucune CLI `influx` en test : les sauvegardes doivent donc
                // annoncer leur volet Influx manquant plutôt que de mentir.
                influx_cli: "influx-absent-des-tests".into(),
                            restart_required: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            self.login_as("admin").await
        }

        async fn login_as(&self, username: &str) -> String {
            let (status, _, cookies) = self
                .call(json_request(
                    "POST",
                    "/api/login",
                    serde_json::json!({ "username": username, "password": PASSWORD }),
                ))
                .await;
            assert_eq!(status, StatusCode::OK, "le login de {username} doit réussir");
            cookies
                .first()
                .and_then(|c| c.split(';').next())
                .expect("un cookie de session est attendu")
                .to_string()
        }

        /// Crée un compte directement en base : les tests de rôle veulent un
        /// acteur, pas un parcours de création.
        async fn user(&self, username: &str, role: Role) {
            self.state
                .db
                .create_user(username, PASSWORD, role)
                .expect("la création du compte doit réussir");
        }

        /// Les lignes d'audit d'une action donnée, lues par l'API — c'est la
        /// seule vue dont dispose un administrateur.
        async fn audit(&self, session: &str, action: &str) -> Vec<serde_json::Value> {
            let (status, body, _) = self
                .call(with_cookie(
                    empty_request("GET", &format!("/api/audit?action={action}")),
                    session,
                ))
                .await;
            assert_eq!(status, StatusCode::OK, "lecture du journal : {body}");
            body["entries"].as_array().cloned().unwrap_or_default()
        }

        /// Enrôle une sonde par code et rend `(probe_id, jeton de sonde)`.
        async fn enroll(&self, session: &str, site: &str, name: &str) -> (String, String) {
            let (_, site_json, _) = self
                .call(with_cookie(
                    json_request("POST", "/api/sites", serde_json::json!({ "name": site })),
                    session,
                ))
                .await;
            // Le site existe déjà quand on enrôle une seconde sonde chez le
            // même client : la création répond 409, on retombe sur l'existant.
            let site_id = match site_json["site_id"].as_str() {
                Some(id) => id.to_string(),
                None => self
                    .state
                    .db
                    .find_site_by_name(site)
                    .unwrap()
                    .expect("le site doit exister")
                    .site_id,
            };

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

    // ── Sauvegarde et restauration ─────────────────────────────────────────

    /// Corps multipart minimal : un champ fichier, et le drapeau de
    /// confirmation. Écrit à la main parce qu'aucune dépendance de test ne le
    /// fabrique et que la forme est justement ce qu'on veut figer.
    fn multipart_request(uri: &str, archive: &[u8], confirm: Option<bool>) -> Request<Body> {
        const BOUNDARY: &str = "----lanprobe-test-boundary";
        let mut body: Vec<u8> = Vec::new();
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"archive\"; \
                 filename=\"televerse.zip\"\r\nContent-Type: application/zip\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(archive);
        body.extend_from_slice(b"\r\n");
        if let Some(confirm) = confirm {
            body.extend_from_slice(
                format!(
                    "--{BOUNDARY}\r\nContent-Disposition: form-data; \
                     name=\"confirm_overwrite\"\r\n\r\n{confirm}\r\n"
                )
                .as_bytes(),
            );
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={BOUNDARY}"))
            .header(header::HOST, "hub.example.org:8443")
            .body(Body::from(body))
            .unwrap()
    }

    #[test]
    fn the_backup_error_codes_do_not_all_collapse_into_five_hundred() {
        // Un schéma trop récent n'est pas une panne du hub, et un fichier
        // quelconque non plus : l'interface doit pouvoir les distinguer.
        use crate::backup::BackupError;
        assert_eq!(backup_status(&BackupError::NotAnArchive("x".into())), StatusCode::BAD_REQUEST);
        assert_eq!(backup_status(&BackupError::Corrupt("x".into())), StatusCode::BAD_REQUEST);
        assert_eq!(
            backup_status(&BackupError::SchemaTooNew { archive: 9, binary: 8 }),
            StatusCode::CONFLICT
        );
        assert_eq!(
            backup_status(&BackupError::ConfirmationRequired("x".into())),
            StatusCode::CONFLICT
        );
        assert_eq!(
            backup_status(&BackupError::Io("x".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn a_backup_is_produced_on_demand_listed_and_recorded() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        h.state.db.create_site("Durand").unwrap();

        let (status, body, _) = h
            .call(with_cookie(empty_request("POST", "/api/backup"), &session))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let file = body["file"].as_str().expect("un nom de fichier");
        assert!(file.starts_with("lanprobe_backup_v"), "{file}");
        assert!(file.ends_with(".zip"), "{file}");
        assert_eq!(body["hub_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(body["schema_version"], crate::db::SCHEMA_VERSION);
        assert!(body["bytes"].as_u64().unwrap() > 0);
        // Pas d'InfluxDB joignable dans les tests : l'archive doit le dire
        // plutôt que de laisser croire qu'elle porte les mesures.
        assert_eq!(body["influx_included"], false);
        assert!(body["influx_error"].is_string());

        let (status, listing, _) = h
            .call(with_cookie(empty_request("GET", "/api/backups"), &session))
            .await;
        assert_eq!(status, StatusCode::OK, "{listing}");
        let archives = listing["backups"].as_array().unwrap();
        assert_eq!(archives.len(), 1);
        assert_eq!(archives[0]["file"], file);
        assert_eq!(listing["keep_last"], 7);
        assert_eq!(listing["enabled"], true);

        let lines = h.audit(&session, "backup.create").await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["actor"], "admin");
        assert_eq!(lines[0]["outcome"], "success");
        assert_eq!(lines[0]["target"], file);
    }

    #[tokio::test]
    async fn backing_up_and_restoring_are_reserved_to_admins() {
        let h = Harness::with_admin().await;
        h.user("operateur", Role::Operator).await;
        h.user("lecteur", Role::Viewer).await;

        for username in ["operateur", "lecteur"] {
            let session = h.login_as(username).await;
            for (method, uri) in [
                ("POST", "/api/backup"),
                ("GET", "/api/backups"),
                ("POST", "/api/backup/restore/quelconque.zip"),
            ] {
                let (status, body, _) = h
                    .call(with_cookie(empty_request(method, uri), &session))
                    .await;
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "{username} ne doit pas pouvoir {method} {uri} : {body}"
                );
            }
        }
    }

    #[tokio::test]
    async fn an_uploaded_file_that_is_not_an_archive_is_refused_without_touching_the_volume() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let witness = h.state.config_dir.join("secret.key");
        std::fs::write(&witness, b"cle-en-place").unwrap();

        let (status, body, _) = h
            .call(with_cookie(
                multipart_request(
                    "/api/backup/restore",
                    b"\x89PNG\r\n\x1a\n absolument pas une archive",
                    Some(true),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("LanProbe"),
            "le refus doit dire ce qu'on attendait : {body}"
        );
        assert_eq!(
            std::fs::read(&witness).unwrap(),
            b"cle-en-place",
            "rien du volume ne doit avoir bougé"
        );

        let lines = h.audit(&session, "backup.restore").await;
        assert_eq!(lines.len(), 1, "un refus est une ligne d'audit : {lines:?}");
        assert_eq!(lines[0]["outcome"], "failure");
    }

    #[tokio::test]
    async fn a_rejected_upload_leaves_nothing_behind_in_the_backup_directory() {
        // Le fichier reçu est écrit sur disque avant d'être examiné — une
        // archive avec ses séries ne tiendrait pas en mémoire. Un envoi
        // refusé ne doit pas pour autant laisser un déchet dans /backup :
        // `list` ne le montrerait pas (son nom ne correspond à rien) et le
        // disque se remplirait sans que personne ne voie quoi que ce soit.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        for _ in 0..3 {
            let (status, _, _) = h
                .call(with_cookie(
                    multipart_request("/api/backup/restore", b"pas une archive", Some(true)),
                    &session,
                ))
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }

        let left: Vec<String> = std::fs::read_dir(&h.state.backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(left.is_empty(), "des envois refusés traînent dans /backup : {left:?}");
    }

    #[tokio::test]
    async fn an_uploaded_archive_is_restored_and_the_restart_is_announced() {
        let h = Harness::with_admin().await;
        let session = h.login().await;

        // Une archive fabriquée depuis un hub voisin, avec du contenu à
        // reconnaître après restauration.
        let source = std::env::temp_dir().join(format!("lanprobe-web-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&source);
        std::fs::create_dir_all(&source).unwrap();
        let other = crate::db::Db::open(&source.join("hub.sqlite")).unwrap();
        other.create_initial_admin("venu-de-l-archive", PASSWORD).unwrap();
        other.create_site("Site-de-l-archive").unwrap();
        std::fs::write(source.join("secret.key"), b"cle-de-l-archive").unwrap();
        let report = crate::backup::create(crate::backup::BackupRequest {
            db: &other,
            config_dir: &source,
            backup_dir: &source.join("archives"),
            influx: None,
            now: 1_787_927_400,
        })
        .unwrap();
        let bytes = std::fs::read(&report.path).unwrap();

        let (status, body, _) = h
            .call(with_cookie(
                multipart_request("/api/backup/restore", &bytes, Some(true)),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["schema_version"], crate::db::SCHEMA_VERSION);
        assert_eq!(body["created_at"], "2026-08-28T14:30:00Z");
        assert_eq!(
            body["restart_required"], true,
            "le processus tient encore l'ancienne base : le taire ferait croire \
             à une restauration sans effet"
        );
        let restored = body["restored"].as_array().unwrap();
        assert!(restored.iter().any(|v| v == "hub.sqlite"), "{restored:?}");
        assert!(restored.iter().any(|v| v == "secret.key"), "{restored:?}");

        // Et le volume porte bien le contenu de l'archive.
        assert_eq!(
            std::fs::read(h.state.config_dir.join("secret.key")).unwrap(),
            b"cle-de-l-archive"
        );
        let landed = crate::db::Db::open(&h.state.config_dir.join("hub.sqlite")).unwrap();
        assert!(landed.get_user("venu-de-l-archive").is_ok());
        assert!(landed.list_sites().unwrap().iter().any(|s| s.name == "Site-de-l-archive"));

        let lines = h.audit(&session, "backup.restore").await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["outcome"], "success");
        assert_eq!(lines[0]["actor"], "admin");
    }

    #[tokio::test]
    async fn restoring_over_a_populated_volume_needs_the_confirmation() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        std::fs::write(h.state.config_dir.join("secret.key"), b"cle-en-place").unwrap();

        // Une archive valide, produite par ce hub même.
        let (status, created, _) = h
            .call(with_cookie(empty_request("POST", "/api/backup"), &session))
            .await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let file = created["file"].as_str().unwrap().to_string();

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/backup/restore/{file}"),
                    serde_json::json!({}),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(body["error"].as_str().unwrap().contains("confirm_overwrite"), "{body}");
        assert_eq!(
            std::fs::read(h.state.config_dir.join("secret.key")).unwrap(),
            b"cle-en-place"
        );

        // Avec la confirmation, la même demande passe.
        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/backup/restore/{file}"),
                    serde_json::json!({ "confirm_overwrite": true }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["moved_aside"].is_string(), "l'ancien état doit être quelque part : {body}");
    }

    #[tokio::test]
    async fn an_archive_named_by_a_traversal_is_never_opened() {
        // `/api/backup/restore/{file}` désigne une archive DANS le répertoire
        // de sauvegarde. Un nom qui en sort n'a pas à être ouvert.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        for name in ["..%2F..%2Fetc%2Fpasswd", "..", "%2Fetc%2Fpasswd"] {
            let (status, body, _) = h
                .call(with_cookie(
                    json_request(
                        "POST",
                        &format!("/api/backup/restore/{name}"),
                        serde_json::json!({ "confirm_overwrite": true }),
                    ),
                    &session,
                ))
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "« {name} » : {body}");
        }
    }

    #[tokio::test]
    async fn an_archive_from_a_newer_schema_is_refused_over_http_with_a_readable_reason() {
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let source = std::env::temp_dir().join(format!("lanprobe-web-futur-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&source);
        std::fs::create_dir_all(&source).unwrap();
        let other = crate::db::Db::open(&source.join("hub.sqlite")).unwrap();
        let report = crate::backup::create(crate::backup::BackupRequest {
            db: &other,
            config_dir: &source,
            backup_dir: &source.join("archives"),
            influx: None,
            now: 1_787_927_400,
        })
        .unwrap();
        // On vieillit le binaire plutôt que d'inventer un schéma : le
        // manifeste annonce une version que ce hub ne connaît pas.
        let bytes = forge_schema(&report.path, crate::db::SCHEMA_VERSION + 3);

        let (status, body, _) = h
            .call(with_cookie(
                multipart_request("/api/backup/restore", &bytes, Some(true)),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let message = body["error"].as_str().unwrap();
        assert!(message.contains(&(crate::db::SCHEMA_VERSION + 3).to_string()), "{message}");
        assert!(
            message.contains("jour") || message.contains("récente"),
            "le message doit dire quoi faire : {message}"
        );
    }

    /// Réécrit le manifeste d'une archive avec un autre numéro de schéma.
    fn forge_schema(archive: &std::path::Path, schema: i64) -> Vec<u8> {
        use std::io::{Read, Write};
        let mut zip = zip::ZipArchive::new(std::fs::File::open(archive).unwrap()).unwrap();
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).unwrap();
            let name = file.name().to_string();
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).unwrap();
            entries.push((name, buffer));
        }
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut out);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            for (name, content) in entries {
                let content = if name == crate::backup::MANIFEST_NAME {
                    let mut manifest: serde_json::Value = serde_json::from_slice(&content).unwrap();
                    manifest["schema_version"] = serde_json::json!(schema);
                    serde_json::to_vec(&manifest).unwrap()
                } else {
                    content
                };
                writer.start_file(name, options).unwrap();
                writer.write_all(&content).unwrap();
            }
            writer.finish().unwrap();
        }
        out.into_inner()
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
    async fn the_fleet_reports_the_internet_verdict_the_probe_gave() {
        // ⚠️ Le cas que ça rend visible : une sonde qui bat parfaitement alors
        // que son lien mesuré est mort. Battement et mesures ne passent pas
        // par la même requête — sans ce champ, le parc affiche un voyant vert
        // pour une sonde qui n'écrit plus rien.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        h.call(with_bearer(
            json_request(
                "POST",
                &format!("/api/probes/{probe_id}/heartbeat"),
                serde_json::json!({ "internet_state": "offline" }),
            ),
            &token,
        ))
        .await;

        let (_, fleet, _) = h
            .call(with_cookie(
                json_request("GET", "/api/probes", serde_json::json!({})),
                &session,
            ))
            .await;
        assert_eq!(fleet[0]["internet_state"], "offline");
        assert!(fleet[0]["internet_state_at"].is_i64());
    }

    #[tokio::test]
    async fn a_heartbeat_without_an_internet_verdict_erases_nothing() {
        // Une sonde plus ancienne que ce champ ne l'envoie pas. Prendre son
        // silence pour « je ne sais plus » effacerait un état connu.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        for body in [
            serde_json::json!({ "internet_state": "limited" }),
            serde_json::json!({}),
        ] {
            h.call(with_bearer(
                json_request("POST", &format!("/api/probes/{probe_id}/heartbeat"), body),
                &token,
            ))
            .await;
        }

        let (_, fleet, _) = h
            .call(with_cookie(
                json_request("GET", "/api/probes", serde_json::json!({})),
                &session,
            ))
            .await;
        assert_eq!(fleet[0]["internet_state"], "limited");
    }

    #[tokio::test]
    async fn an_unexpected_internet_verdict_is_dropped() {
        // La sonde décide de son verdict, pas de son vocabulaire : une valeur
        // inattendue s'afficherait telle quelle dans le parc, sans couleur ni
        // sens.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        h.call(with_bearer(
            json_request(
                "POST",
                &format!("/api/probes/{probe_id}/heartbeat"),
                serde_json::json!({ "internet_state": "peut-être" }),
            ),
            &token,
        ))
        .await;

        let (_, fleet, _) = h
            .call(with_cookie(
                json_request("GET", "/api/probes", serde_json::json!({})),
                &session,
            ))
            .await;
        assert!(fleet[0]["internet_state"].is_null(), "{fleet}");
    }

    // ── Inventaire réseau (contrat § 12) ───────────────────────────────────

    #[tokio::test]
    async fn a_probe_publishes_a_scan_and_the_interface_reads_it_back() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/scans"),
                    serde_json::json!({
                        "kind": "discovery",
                        "started_at": 1_788_000_000,
                        "cidr": "10.0.8.0/24",
                        "hosts": [
                            { "ip": "10.0.8.1", "hostname": "gw", "mac": "aa:bb", "vendor": "Ubiquiti" },
                            { "ip": "10.0.8.50", "latency_ms": 2 }
                        ]
                    }),
                ),
                &token,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let (_, read, _) = h
            .call(with_cookie(
                json_request(
                    "GET",
                    &format!("/api/probes/{probe_id}/inventory?kind=discovery"),
                    serde_json::json!({}),
                ),
                &session,
            ))
            .await;
        assert_eq!(read["scan"]["cidr"], "10.0.8.0/24");
        assert_eq!(read["scan"]["hosts"].as_array().unwrap().len(), 2);
        assert_eq!(read["scan"]["hosts"][0]["vendor"], "Ubiquiti");
    }

    #[tokio::test]
    async fn a_scan_never_launched_is_null_not_an_empty_scan() {
        // « Jamais lancé » et « lancé, rien trouvé » appellent deux phrases
        // différentes à l'écran. Rendre un scan vide dans les deux cas ferait
        // écrire « aucune machine sur ce réseau » pour un scan qui n'a jamais
        // eu lieu.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let (_, read, _) = h
            .call(with_cookie(
                json_request(
                    "GET",
                    &format!("/api/probes/{probe_id}/inventory?kind=ports"),
                    serde_json::json!({}),
                ),
                &session,
            ))
            .await;
        assert!(read["scan"].is_null(), "{read}");
    }

    #[tokio::test]
    async fn each_run_is_kept_so_history_can_be_read() {
        // ⚠️ Un scan par exécution, jamais un état courant écrasé : sans ça on
        // ne peut plus répondre à « quels ports étaient ouverts le 12 mars ».
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        for (at, dl) in [(1_788_000_000, 100.0), (1_788_000_600, 900.0)] {
            h.call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/scans"),
                    serde_json::json!({
                        "kind": "speedtest",
                        "started_at": at,
                        "speedtest": { "engine": "ookla", "download_mbps": dl, "upload_mbps": 50.0 }
                    }),
                ),
                &token,
            ))
            .await;
        }

        let (_, read, _) = h
            .call(with_cookie(
                json_request(
                    "GET",
                    &format!("/api/probes/{probe_id}/inventory?kind=speedtest"),
                    serde_json::json!({}),
                ),
                &session,
            ))
            .await;
        let rows = read["speedtests"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "les deux exécutions sont conservées : {read}");
        // Du plus récent au plus ancien.
        assert_eq!(rows[0]["download_mbps"], 900.0);
    }

    #[tokio::test]
    async fn a_scan_without_a_probe_token_is_refused() {
        // La route est ouverte au réseau des sondes : sans authentification,
        // n'importe qui pourrait remplir l'inventaire d'un client.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let (status, _, _) = h
            .call(json_request(
                "POST",
                &format!("/api/probes/{probe_id}/scans"),
                serde_json::json!({ "kind": "discovery", "started_at": 1 }),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn an_unknown_scan_kind_is_refused() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, _, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/scans"),
                    serde_json::json!({ "kind": "tout", "started_at": 1 }),
                ),
                &token,
            ))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // ── File de commandes (contrat § 14) ───────────────────────────────────

    #[tokio::test]
    async fn a_command_reaches_the_probe_through_its_heartbeat() {
        // Le hub n'a aucune route vers la sonde : le battement est le seul
        // canal dans ce sens. Si la commande ne voyage pas là, elle ne voyage
        // nulle part.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/commands"),
                    serde_json::json!({ "kind": "port_scan", "args": { "ip": "10.0.0.1" } }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let id = body["id"].as_i64().unwrap();

        let (status, beat, _) = h
            .call(with_bearer(
                json_request("POST", &format!("/api/probes/{probe_id}/heartbeat"), serde_json::json!({})),
                &token,
                ))
            .await;
        assert_eq!(status, StatusCode::OK, "{beat}");
        let commands = beat["commands"].as_array().unwrap();
        assert_eq!(commands.len(), 1, "{beat}");
        assert_eq!(commands[0]["id"], id);
        assert_eq!(commands[0]["kind"], "port_scan");
        assert_eq!(commands[0]["args"]["ip"], "10.0.0.1");
    }

    #[tokio::test]
    async fn an_acknowledged_command_is_not_sent_again() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;
        let (_, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/commands"),
                    serde_json::json!({ "kind": "speedtest" }),
                ),
                &session,
            ))
            .await;
        let id = body["id"].as_i64().unwrap();

        h.call(with_bearer(
            json_request("POST", &format!("/api/probes/{probe_id}/heartbeat"), serde_json::json!({})),
            &token,
            ))
        .await;

        let (_, beat, _) = h
            .call(with_bearer(
                json_request("POST", &format!("/api/probes/{probe_id}/heartbeat"), serde_json::json!({ "command_acks": [{ "id": id, "ok": true }] })),
                &token,
                ))
            .await;
        // ⚠️ L'accusé est traité AVANT la file : sans cet ordre, la commande
        // repartirait dans le battement qui vient de l'accuser.
        assert!(beat["commands"].as_array().unwrap().is_empty(), "{beat}");

        let (_, listed, _) = h
            .call(with_cookie(
                json_request("GET", &format!("/api/probes/{probe_id}/commands"), serde_json::json!({})),
                &session,
            ))
            .await;
        assert_eq!(listed["commands"][0]["state"], "done");
    }

    #[tokio::test]
    async fn a_failed_command_keeps_the_reason_the_probe_gave() {
        // C'est ce texte que l'opérateur lira. « Échec » sans raison
        // l'obligerait à aller lire les journaux de la machine distante.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;
        let (_, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/commands"),
                    serde_json::json!({ "kind": "discovery" }),
                ),
                &session,
            ))
            .await;
        let id = body["id"].as_i64().unwrap();
        h.call(with_bearer(
            json_request("POST", &format!("/api/probes/{probe_id}/heartbeat"), serde_json::json!({})),
            &token,
            ))
        .await;
        h.call(with_bearer(
            json_request("POST", &format!("/api/probes/{probe_id}/heartbeat"), serde_json::json!({
                "command_acks": [{ "id": id, "ok": false, "error": "interface sans adresse" }]
            })),
            &token,
            ))
        .await;

        let (_, listed, _) = h
            .call(with_cookie(
                json_request("GET", &format!("/api/probes/{probe_id}/commands"), serde_json::json!({})),
                &session,
            ))
            .await;
        assert_eq!(listed["commands"][0]["state"], "failed");
        assert_eq!(listed["commands"][0]["error"], "interface sans adresse");
    }

    #[tokio::test]
    async fn an_arbitrary_command_kind_is_refused() {
        // ⚠️ Une commande part de l'intérieur du LAN d'un client. Accepter un
        // `kind` libre offrirait l'exécution de n'importe quoi à qui
        // obtiendrait un compte opérateur.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;
        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/commands"),
                    serde_json::json!({ "kind": "shell", "args": { "cmd": "rm -rf /" } }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    #[test]
    fn explicit_bounds_win_over_a_sliding_window() {
        // ⚠️ Un rapport de SLA porte sur une période convenue — « du 1er au
        // 31 » — pas sur « les sept derniers jours ». Une fenêtre glissante
        // donnerait un chiffre différent à chaque ouverture du document.
        let (window, label) = flux_range(&MetricsQuery {
            measurement: None,
            range: Some("-7d".into()),
            start: Some(1_788_000_000),
            stop: Some(1_788_600_000),
        });
        assert_eq!(window, "start: 1788000000, stop: 1788600000");
        assert_eq!(label, "1788000000..1788600000");
    }

    #[test]
    fn reversed_bounds_fall_back_instead_of_asking_influx_for_nothing() {
        // Des bornes inversées rendraient une fenêtre vide, donc un rapport à
        // 0 % qu'on prendrait pour une panne totale.
        let (window, _) = flux_range(&MetricsQuery {
            measurement: None,
            range: Some("-24h".into()),
            start: Some(1_788_600_000),
            stop: Some(1_788_000_000),
        });
        assert_eq!(window, "start: -24h");
    }

    #[test]
    fn alive_and_latency_are_recollected_into_one_sample() {
        // ⚠️ Ils arrivent sur DEUX lignes distinctes du CSV d'Influx. Les
        // traiter séparément donnerait deux fois plus de relevés que la sonde
        // n'en a pris, et fausserait toute disponibilité calculée dessus.
        let csv = ",result,table,_time,_value,_field,ip\n\
                   ,,0,2026-08-29T09:00:00Z,true,alive,10.0.0.1\n\
                   ,,0,2026-08-29T09:00:00Z,12.5,latency_ms,10.0.0.1\n";
        let targets = samples_from_flux_csv(csv);
        assert_eq!(targets.len(), 1);
        let samples = targets[0]["samples"].as_array().unwrap();
        assert_eq!(samples.len(), 1, "un instant = un relevé : {samples:?}");
        assert_eq!(samples[0]["alive"], true);
        assert_eq!(samples[0]["latency_ms"], 12.5);
    }

    #[test]
    fn a_host_that_never_answered_does_not_get_a_hundred_percent_uptime() {
        // ⚠️ Un hôte qui ne répond pas n'écrit PAS de latence. Calculer la
        // disponibilité sur la présence d'une latence donnerait 100 % sur un
        // hôte mort — le chiffre exactement inverse de la vérité.
        let csv = ",result,table,_time,_value,_field,ip\n\
                   ,,0,2026-08-29T09:00:00Z,false,alive,10.0.0.9\n\
                   ,,0,2026-08-29T09:00:01Z,false,alive,10.0.0.9\n";
        let report = sla_from_flux_csv(csv);
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].uptime_pct, 0.0);
        assert_eq!(report[0].failed_samples, 2);
    }

    #[test]
    fn sla_is_computed_per_target_not_merged() {
        // Deux cibles fusionnées donneraient une disponibilité moyenne
        // qu'aucune des deux n'a jamais eue.
        let csv = ",result,table,_time,_value,_field,ip\n\
                   ,,0,2026-08-29T09:00:00Z,true,alive,10.0.0.1\n\
                   ,,0,2026-08-29T09:00:00Z,12,latency_ms,10.0.0.1\n\
                   ,,0,2026-08-29T09:00:00Z,false,alive,8.8.8.8\n";
        let report = sla_from_flux_csv(csv);
        assert_eq!(report.len(), 2);
        assert_eq!(report[0].ip, "10.0.0.1");
        assert_eq!(report[0].uptime_pct, 100.0);
        assert_eq!(report[1].ip, "8.8.8.8");
        assert_eq!(report[1].uptime_pct, 0.0);
    }

    #[test]
    fn a_probe_name_with_a_comma_does_not_shift_the_columns() {
        // Une seule sonde mal nommée décalerait tout le fichier.
        assert_eq!(csv_cell("Paris, bureau"), "\"Paris, bureau\"");
        assert_eq!(csv_cell("Paris"), "Paris");
        assert_eq!(csv_cell("dit \"Paris\""), "\"dit \"\"Paris\"\"\"");
    }

    #[tokio::test]
    async fn the_sla_export_is_a_csv_named_after_the_probe() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let response = h
            .router
            .clone()
            .oneshot(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/sla.csv?range=-24h")),
                &session,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let disposition = response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(disposition.contains("sla-paris-24h.csv"), "{disposition}");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        // L'en-tête doit être là même sans mesure : un fichier vide se lit
        // comme un export raté.
        assert!(text.starts_with("sonde,site,fenetre,cible,"), "{text}");
    }

    #[tokio::test]
    async fn an_iperf_command_without_a_server_is_refused_by_the_hub() {
        // ⚠️ Refusé ici, pas à l'arrivée : une commande vouée à l'échec
        // occuperait la file, serait remise trois fois, et l'opérateur lirait
        // « échouée » une minute plus tard pour une faute de saisie.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        for args in [
            serde_json::json!({ "engine": "iperf3" }),
            serde_json::json!({ "engine": "iperf3", "server": "   " }),
        ] {
            let (status, _, _) = h
                .call(with_cookie(
                    json_request(
                        "POST",
                        &format!("/api/probes/{probe_id}/commands"),
                        serde_json::json!({ "kind": "speedtest", "args": args }),
                    ),
                    &session,
                ))
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }

        // Ookla n'a pas de serveur à donner : il ne doit pas être bloqué.
        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/commands"),
                    serde_json::json!({ "kind": "speedtest", "args": { "engine": "ookla" } }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_fleet_reports_the_active_watches_the_probe_announced() {
        // ⚠️ Le hub déduisait les cibles des mesures de la fenêtre affichée :
        // une cible retirée continuait d'apparaître tant que ses points y
        // restaient. Seule la sonde sait ce qui tourne à l'instant.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        h.call(with_bearer(
            json_request(
                "POST",
                &format!("/api/probes/{probe_id}/heartbeat"),
                serde_json::json!({
                    "monitors": [
                        { "ip": "10.0.0.1", "alive": true, "latency_ms": 1.2,
                          "uptime_pct": 99.5, "samples": 200 }
                    ]
                }),
            ),
            &token,
        ))
        .await;

        let (_, fleet, _) = h
            .call(with_cookie(
                json_request("GET", "/api/probes", serde_json::json!({})),
                &session,
            ))
            .await;
        assert_eq!(fleet[0]["monitors"][0]["ip"], "10.0.0.1");
        assert_eq!(fleet[0]["monitors"][0]["uptime_pct"], 99.5);

        // Une liste VIDE doit effacer : une sonde qui ne surveille plus rien
        // doit pouvoir le dire, sinon l'écran affirme une surveillance morte.
        h.call(with_bearer(
            json_request(
                "POST",
                &format!("/api/probes/{probe_id}/heartbeat"),
                serde_json::json!({ "monitors": [] }),
            ),
            &token,
        ))
        .await;
        let (_, fleet, _) = h
            .call(with_cookie(
                json_request("GET", "/api/probes", serde_json::json!({})),
                &session,
            ))
            .await;
        assert_eq!(fleet[0]["monitors"].as_array().unwrap().len(), 0, "{fleet}");
    }

    #[tokio::test]
    async fn a_viewer_cannot_launch_a_command() {
        // Lancer un scan est une action sur le réseau d'un client, pas une
        // consultation.
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let (probe_id, _) = h.enroll(&admin, "Durand", "Paris").await;
        h.user("claire", Role::Viewer).await;
        let viewer = h.login_as("claire").await;

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/commands"),
                    serde_json::json!({ "kind": "speedtest" }),
                ),
                &viewer,
            ))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn launching_a_command_writes_an_audit_line_naming_its_author() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;
        h.call(with_cookie(
            json_request(
                "POST",
                &format!("/api/probes/{probe_id}/commands"),
                serde_json::json!({ "kind": "speedtest" }),
            ),
            &session,
        ))
        .await;
        let lines = h.audit(&session, "probe.command").await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["actor"], "admin");
        assert_eq!(lines[0]["target"], probe_id);
    }

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
        // En clair, `Secure` doit être absent : le navigateur refuserait le
        // cookie et l'utilisateur retomberait sur l'écran de connexion sans
        // le moindre message d'erreur.
        assert!(!cookie.contains("Secure"), "{cookie}");
    }

    #[tokio::test]
    async fn session_cookie_is_secure_behind_an_https_proxy() {
        // Le proxy a terminé le TLS : le cookie doit reprendre `Secure`,
        // sinon il voyagerait en clair sur le dernier tronçon si quelqu'un
        // atteignait le hub sans passer par le proxy.
        let h = Harness::with_admin().await;
        let request = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-forwarded-proto", "https")
            .body(Body::from(
                serde_json::json!({ "username": "admin", "password": PASSWORD }).to_string(),
            ))
            .unwrap();
        let response = h.router.clone().oneshot(request).await.unwrap();
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
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
    async fn a_heartbeat_carries_the_network_identity_to_the_probe_list() {
        // L'IP publique identifie le site d'un coup d'œil sur la ligne du
        // parc ; l'interface et les adresses locales vont sur la fiche.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, _, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/heartbeat"),
                    serde_json::json!({
                        "version": "1.2.0",
                        "platform": "linux",
                        "public_ip": "88.120.0.1",
                        "interface": "en0",
                        "local_ips": ["10.6.8.42/24"],
                        "gateway": "10.6.8.1"
                    }),
                ),
                &token,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/probes"), &session))
            .await;
        assert_eq!(status, StatusCode::OK);
        let probe = &body[0];
        assert_eq!(probe["public_ip"], "88.120.0.1");
        assert_eq!(probe["interface"], "en0");
        assert_eq!(probe["local_ips"][0], "10.6.8.42/24");
        assert_eq!(probe["gateway"], "10.6.8.1");
        assert!(
            probe["public_ip_at"].is_i64(),
            "la date de l'adresse doit sortir : {probe}"
        );
    }

    #[tokio::test]
    async fn a_heartbeat_without_a_public_ip_is_accepted_and_erases_nothing() {
        // Une sonde qui perd son accès internet bat sans IP publique — c'est
        // justement à ce moment-là qu'on veut savoir quelle adresse elle
        // avait. Le hub ne remplace jamais une valeur connue par du vide.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        h.call(with_bearer(
            json_request(
                "POST",
                &format!("/api/probes/{probe_id}/heartbeat"),
                serde_json::json!({
                    "public_ip": "88.120.0.1",
                    "interface": "en0",
                    "local_ips": ["10.6.8.42/24"],
                    "gateway": "10.6.8.1"
                }),
            ),
            &token,
        ))
        .await;

        let (status, body, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/heartbeat"),
                    serde_json::json!({ "buffered_points": 12 }),
                ),
                &token,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "un battement sans IP publique reste valide : {body}");

        let (_, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/probes"), &session))
            .await;
        let probe = &body[0];
        assert_eq!(probe["public_ip"], "88.120.0.1", "{probe}");
        assert_eq!(probe["interface"], "en0");
        assert_eq!(probe["local_ips"][0], "10.6.8.42/24");
        assert_eq!(probe["gateway"], "10.6.8.1");
        assert_eq!(probe["buffered_points"], 12);
    }

    #[tokio::test]
    async fn a_probe_that_goes_offline_keeps_its_last_known_address() {
        // Afficher un tiret parce qu'elle ne répond plus perdrait
        // l'information au moment exact où elle sert.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        h.call(with_bearer(
            json_request(
                "POST",
                &format!("/api/probes/{probe_id}/heartbeat"),
                serde_json::json!({ "public_ip": "88.120.0.1", "interface": "en0" }),
            ),
            &token,
        ))
        .await;

        // On la fait taire depuis longtemps : le statut dérivé bascule, les
        // valeurs restent.
        h.state
            .db
            .backdate_last_seen(&probe_id, crate::db::now() - 3 * 24 * 3600)
            .unwrap();

        let (_, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/probes"), &session))
            .await;
        let probe = &body[0];
        assert_eq!(probe["status"], "offline", "{probe}");
        assert_eq!(probe["public_ip"], "88.120.0.1");
        assert_eq!(probe["interface"], "en0");
    }

    /// Un battement qui ne porte que son IP publique.
    async fn beat_public_ip(h: &Harness, probe_id: &str, token: &str, ip: &str) {
        let (status, body, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/heartbeat"),
                    serde_json::json!({ "public_ip": ip }),
                ),
                token,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    #[tokio::test]
    async fn only_a_change_of_public_ip_writes_an_audit_line() {
        // Une bascule sur un lien de secours doit se voir ; cent battements
        // identiques ne doivent rien écrire, sinon plus personne ne lit le
        // journal.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        beat_public_ip(&h, &probe_id, &token, "88.120.0.1").await;
        beat_public_ip(&h, &probe_id, &token, "88.120.0.1").await;
        beat_public_ip(&h, &probe_id, &token, "88.120.0.1").await;
        let lines = h.audit(&session, "probe.public_ip_changed").await;
        assert_eq!(
            lines.len(),
            1,
            "la première adresse compte, les répétitions non : {lines:?}"
        );

        beat_public_ip(&h, &probe_id, &token, "88.120.0.2").await;
        let lines = h.audit(&session, "probe.public_ip_changed").await;
        assert_eq!(lines.len(), 2, "{lines:?}");
        let last = &lines[0];
        assert_eq!(last["target"], probe_id);
        let detail = last["detail"].as_str().unwrap_or_default();
        assert!(detail.contains("88.120.0.1"), "l'ancienne adresse : {detail}");
        assert!(detail.contains("88.120.0.2"), "la nouvelle adresse : {detail}");
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

    #[tokio::test]
    async fn me_says_who_is_connected_and_with_which_role() {
        // Route posée d'abord dans le groupe public : l'extension d'identité
        // n'y est pas injectée, et la route répondait par une erreur interne.
        // Elle doit vivre derrière le garde de session.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/me"), &session))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["username"], "admin");
        assert_eq!(body["role"], "admin");

        // Sans session : refusé, pas d'erreur interne.
        let (status, _, _) = h.call(empty_request("GET", "/api/me")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // ── Écriture des mesures ───────────────────────────────────────────────

    #[tokio::test]
    async fn writing_metrics_needs_the_probe_token() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let line = "ping,probe_id=x latency_ms=12 1787788800000000000";

        // Sans jeton : refusé.
        let request = Request::builder()
            .method("POST")
            .uri(format!("/api/probes/{probe_id}/write"))
            .body(Body::from(line))
            .unwrap();
        let (status, _, _) = h.call(request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Avec un mauvais jeton : refusé aussi. Le hub est la seule porte,
        // il ne peut pas se permettre d'être permissif.
        let request = Request::builder()
            .method("POST")
            .uri(format!("/api/probes/{probe_id}/write"))
            .header(header::AUTHORIZATION, "Bearer faux")
            .body(Body::from(line))
            .unwrap();
        let (status, _, _) = h.call(request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Avec le bon jeton : relayé.
        let request = Request::builder()
            .method("POST")
            .uri(format!("/api/probes/{probe_id}/write"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(line))
            .unwrap();
        let (status, body, _) = h.call(request).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["written"], 1);
    }

    #[test]
    fn relayed_points_carry_the_probe_identity() {
        // Sans ces étiquettes, le hub écrit bien dans Influx mais ne retrouve
        // plus ses propres mesures : toutes ses requêtes filtrent sur
        // `probe_id`. Sonde en ligne, écritures en 200, graphes vides.
        let out = tag_lines(
            "ping_latency,host=Windows,ip=1.1.1.1 latency_ms=12i 1787788800000000000",
            "p-1",
            "s-1",
        );
        assert_eq!(
            out.trim(),
            "ping_latency,host=Windows,ip=1.1.1.1,probe_id=p-1,site_id=s-1 latency_ms=12i 1787788800000000000"
        );
    }

    #[test]
    fn a_measurement_without_tags_still_gets_the_identity() {
        let out = tag_lines("speedtest download_mbps=100 1787788800000000000", "p-1", "s-1");
        assert!(out.starts_with("speedtest,probe_id=p-1,site_id=s-1 "), "{out}");
    }

    #[tokio::test]
    async fn an_empty_batch_is_accepted_not_rejected() {
        // Une sonde qui n'a rien mesuré pendant son intervalle a le droit de
        // le dire : la rejeter la ferait réessayer en boucle pour rien.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let request = Request::builder()
            .method("POST")
            .uri(format!("/api/probes/{probe_id}/write"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(""))
            .unwrap();
        let (status, body, _) = h.call(request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["written"], 0);
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

    // ── Rôles : l'application se fait côté serveur, route par route ────────
    //
    // Une interface qui masque les boutons ne protège rien : chaque test
    // ci-dessous appelle la route à la main, comme le ferait un `curl`.

    #[tokio::test]
    async fn a_viewer_can_read_the_parc() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.enroll(&admin, "Durand", "Paris").await;
        h.user("claire", Role::Viewer).await;
        let viewer = h.login_as("claire").await;

        for path in ["/api/sites", "/api/probes", "/api/settings", "/api/influx"] {
            let (status, _, _) = h.call(with_cookie(empty_request("GET", path), &viewer)).await;
            assert_eq!(status, StatusCode::OK, "un viewer doit pouvoir lire {path}");
        }
    }

    #[tokio::test]
    async fn a_viewer_cannot_revoke_a_probe() {
        // Le scénario même qui justifie le rôle : montrer le parc à un client
        // sans qu'un clic — ou un `curl` — révoque une sonde.
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let (probe_id, _) = h.enroll(&admin, "Durand", "Paris").await;
        h.user("claire", Role::Viewer).await;
        let viewer = h.login_as("claire").await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("DELETE", &format!("/api/probes/{probe_id}")),
                &viewer,
            ))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(
            h.state.db.get_probe(&probe_id).unwrap().revoked_at.is_none(),
            "la sonde ne doit pas avoir été révoquée"
        );
    }

    #[tokio::test]
    async fn a_viewer_cannot_touch_the_parc() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let (probe_id, _) = h.enroll(&admin, "Durand", "Paris").await;
        let site_id = h.state.db.find_site_by_name("Durand").unwrap().unwrap().site_id;
        h.user("claire", Role::Viewer).await;
        let viewer = h.login_as("claire").await;

        let refused: Vec<(StatusCode, String)> = vec![
            (
                h.call(with_cookie(
                    json_request("POST", "/api/sites", serde_json::json!({ "name": "Martin" })),
                    &viewer,
                ))
                .await
                .0,
                "POST /api/sites".into(),
            ),
            (
                h.call(with_cookie(
                    json_request(
                        "POST",
                        "/api/enroll-codes",
                        serde_json::json!({ "site_id": site_id }),
                    ),
                    &viewer,
                ))
                .await
                .0,
                "POST /api/enroll-codes".into(),
            ),
            (
                h.call(with_cookie(
                    json_request(
                        "PATCH",
                        &format!("/api/probes/{probe_id}"),
                        serde_json::json!({ "name": "Renommée" }),
                    ),
                    &viewer,
                ))
                .await
                .0,
                "PATCH /api/probes/{id}".into(),
            ),
            (
                h.call(with_cookie(
                    json_request(
                        "POST",
                        &format!("/api/probes/{probe_id}/rotate"),
                        serde_json::json!({}),
                    ),
                    &viewer,
                ))
                .await
                .0,
                "POST /api/probes/{id}/rotate".into(),
            ),
        ];
        for (status, what) in refused {
            assert_eq!(status, StatusCode::FORBIDDEN, "{what} doit être refusé au viewer");
        }
    }

    #[tokio::test]
    async fn a_viewer_cannot_read_a_pending_enrollment_code() {
        // Le code en attente est en clair pendant sa validité : c'est de quoi
        // enrôler une sonde, donc ce n'est pas une lecture de consultation.
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.enroll(&admin, "Durand", "Paris").await;
        h.user("claire", Role::Viewer).await;
        let viewer = h.login_as("claire").await;

        let (status, _, _) = h
            .call(with_cookie(empty_request("GET", "/api/enroll-codes/pending"), &viewer))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn an_operator_runs_the_parc_but_not_the_accounts() {
        let h = Harness::with_admin().await;
        h.login().await;
        h.user("olivier", Role::Operator).await;
        let operator = h.login_as("olivier").await;

        let (status, _, _) = h
            .call(with_cookie(
                json_request("POST", "/api/sites", serde_json::json!({ "name": "Martin" })),
                &operator,
            ))
            .await;
        assert_eq!(status, StatusCode::CREATED, "l'opérateur enrôle");

        for (method, path) in [("GET", "/api/users"), ("GET", "/api/audit")] {
            let (status, _, _) = h
                .call(with_cookie(empty_request(method, path), &operator))
                .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path} est réservé à l'admin");
        }
    }

    #[tokio::test]
    async fn an_operator_cannot_change_the_retention() {
        // « admin : tout, y compris les comptes et la rétention. »
        let h = Harness::with_admin().await;
        h.login().await;
        h.user("olivier", Role::Operator).await;
        let operator = h.login_as("olivier").await;

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/settings",
                    serde_json::json!({ "retention_days": "30", "confirm_data_loss": true }),
                ),
                &operator,
            ))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(h.state.settings.retention_days(), 0, "rien ne doit avoir changé");
    }

    #[tokio::test]
    async fn enrolling_with_a_viewer_account_is_refused() {
        // Le chemin scripté passe par les identifiants du compte, pas par une
        // session : sans contrôle de rôle, il contournerait tout le reste.
        let h = Harness::with_admin().await;
        h.login().await;
        h.user("claire", Role::Viewer).await;

        let (status, body, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({
                    "name": "Paris",
                    "username": "claire",
                    "password": PASSWORD,
                    "site": "Durand",
                }),
            ))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(h.state.db.list_probes(None).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_role_change_takes_effect_without_reconnecting() {
        // Le rôle est relu à chaque requête, pas figé dans la session : sinon
        // une rétrogradation n'aurait d'effet qu'à la prochaine connexion.
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.user("olivier", Role::Operator).await;
        let operator = h.login_as("olivier").await;

        let (status, _, _) = h
            .call(with_cookie(empty_request("GET", "/api/users"), &operator))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PATCH",
                    "/api/users/olivier",
                    serde_json::json!({ "role": "admin" }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _, _) = h
            .call(with_cookie(empty_request("GET", "/api/users"), &operator))
            .await;
        assert_eq!(status, StatusCode::OK, "la même session doit voir son nouveau rôle");
    }

    #[tokio::test]
    async fn a_disabled_account_can_no_longer_open_a_session() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.user("claire", Role::Viewer).await;

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PATCH",
                    "/api/users/claire",
                    serde_json::json!({ "disabled": true }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({ "username": "claire", "password": PASSWORD }),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // ── Comptes ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn an_admin_creates_and_lists_accounts_without_any_hash() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    "/api/users",
                    serde_json::json!({ "username": "claire", "password": PASSWORD, "role": "viewer" }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["username"], "claire");
        assert_eq!(body["role"], "viewer");

        let (status, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/users"), &admin))
            .await;
        assert_eq!(status, StatusCode::OK);
        let rendered = body.to_string();
        assert!(rendered.contains("claire"));
        assert!(!rendered.contains("$argon2"), "aucun hash ne sort : {rendered}");
        assert!(!rendered.contains(PASSWORD), "aucun mot de passe ne sort : {rendered}");
    }

    #[tokio::test]
    async fn an_unknown_role_is_refused_rather_than_interpreted() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    "/api/users",
                    serde_json::json!({ "username": "claire", "password": PASSWORD, "role": "sorcier" }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn there_is_no_route_to_delete_an_account() {
        // On désactive, on ne supprime pas : un compte effacé emporterait la
        // traçabilité de ce qu'il a fait.
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.user("claire", Role::Viewer).await;

        let (status, _, _) = h
            .call(with_cookie(empty_request("DELETE", "/api/users/claire"), &admin))
            .await;
        assert_eq!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "aucune route ne doit supprimer un compte"
        );
        assert!(h.state.db.get_user("claire").is_ok());
    }

    #[tokio::test]
    async fn the_last_admin_cannot_be_stripped_over_http() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PATCH",
                    "/api/users/admin",
                    serde_json::json!({ "role": "viewer" }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PATCH",
                    "/api/users/admin",
                    serde_json::json!({ "disabled": true }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(h.state.db.role_of("admin").unwrap(), Some(Role::Admin));
        assert!(h.state.db.get_user("admin").unwrap().disabled_at.is_none());
    }

    #[tokio::test]
    async fn an_admin_resets_a_password_and_the_account_logs_in_again() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.user("claire", Role::Viewer).await;

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    "/api/users/claire/password",
                    serde_json::json!({ "password": "un-autre-mot-de-passe" }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({ "username": "claire", "password": PASSWORD }),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "l'ancien mot de passe ne vaut plus");

        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({ "username": "claire", "password": "un-autre-mot-de-passe" }),
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    // ── Journal d'audit ────────────────────────────────────────────────────

    #[tokio::test]
    async fn only_an_admin_reads_the_audit_log() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.user("claire", Role::Viewer).await;
        let viewer = h.login_as("claire").await;

        let (status, _, _) = h
            .call(with_cookie(empty_request("GET", "/api/audit"), &viewer))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/audit"), &admin))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["entries"].is_array(), "{body}");
    }

    #[tokio::test]
    async fn there_is_no_route_to_delete_an_audit_line() {
        // Un journal qu'on peut nettoyer ne prouve rien — pas même pour un
        // administrateur.
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        for uri in ["/api/audit", "/api/audit/1"] {
            let (status, _, _) = h
                .call(with_cookie(empty_request("DELETE", uri), &admin))
                .await;
            assert!(
                status == StatusCode::METHOD_NOT_ALLOWED || status == StatusCode::NOT_FOUND,
                "DELETE {uri} a répondu {status}"
            );
        }
    }

    #[tokio::test]
    async fn a_successful_login_is_recorded() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        let entries = h.audit(&admin, "auth.login").await;
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0]["actor"], "admin");
        assert_eq!(entries[0]["outcome"], "success");
    }

    #[tokio::test]
    async fn a_failed_login_is_recorded_as_a_failure() {
        // Un journal qui n'enregistre que les succès ne montre jamais une
        // tentative d'intrusion.
        let h = Harness::with_admin().await;
        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({ "username": "admin", "password": "mauvais" }),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let admin = h.login().await;
        let entries = h.audit(&admin, "auth.login").await;
        let failed: Vec<_> = entries.iter().filter(|e| e["outcome"] == "failure").collect();
        assert_eq!(failed.len(), 1, "{entries:?}");
        assert_eq!(failed[0]["actor"], "admin");
    }

    #[tokio::test]
    async fn a_login_on_an_unknown_account_never_writes_what_was_typed() {
        // Ce champ reçoit ce que quelqu'un a tapé : un jour, ce sera un mot de
        // passe entré dans la mauvaise case. La tentative est journalisée,
        // sans acteur et sans recopier la saisie.
        let h = Harness::with_admin().await;
        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({ "username": "MonMotDePasseSecret", "password": "x" }),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let admin = h.login().await;
        let (_, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/audit"), &admin))
            .await;
        let rendered = body.to_string();
        assert!(
            !rendered.contains("MonMotDePasseSecret"),
            "la saisie ne doit pas être recopiée : {rendered}"
        );
        let anonymous = body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "auth.login" && e["outcome"] == "failure" && e["actor"].is_null());
        assert!(anonymous, "la tentative doit être journalisée : {rendered}");
    }

    #[tokio::test]
    async fn a_refused_request_is_recorded_as_a_failure() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let (probe_id, _) = h.enroll(&admin, "Durand", "Paris").await;
        h.user("claire", Role::Viewer).await;
        let viewer = h.login_as("claire").await;

        h.call(with_cookie(
            empty_request("DELETE", &format!("/api/probes/{probe_id}")),
            &viewer,
        ))
        .await;

        let denied = h.audit(&admin, "access.denied").await;
        assert_eq!(denied.len(), 1, "{denied:?}");
        assert_eq!(denied[0]["actor"], "claire");
        assert_eq!(denied[0]["outcome"], "failure");
        assert!(
            denied[0]["target"].as_str().unwrap().contains("DELETE"),
            "la cible doit nommer la route tentée : {denied:?}"
        );
    }

    #[tokio::test]
    async fn the_parc_gestures_are_all_recorded() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let (probe_id, _) = h.enroll(&admin, "Durand", "Paris").await;

        for path in ["rotate", "revoke-token", "reenroll-code"] {
            let (status, _, _) = h
                .call(with_cookie(
                    json_request(
                        "POST",
                        &format!("/api/probes/{probe_id}/{path}"),
                        serde_json::json!({}),
                    ),
                    &admin,
                ))
                .await;
            assert_eq!(status, StatusCode::OK, "{path}");
        }
        h.call(with_cookie(
            empty_request("DELETE", &format!("/api/probes/{probe_id}")),
            &admin,
        ))
        .await;

        for action in [
            "probe.enroll",
            "probe.rotate",
            "probe.revoke_token",
            "probe.reenroll_code",
            "probe.revoke",
        ] {
            assert!(
                !h.audit(&admin, action).await.is_empty(),
                "« {action} » doit figurer au journal"
            );
        }
    }

    #[tokio::test]
    async fn changing_the_retention_is_recorded() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/settings",
                    serde_json::json!({ "retention_days": "90", "confirm_data_loss": true }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let entries = h.audit(&admin, "settings.retention").await;
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0]["outcome"], "success");
    }

    #[tokio::test]
    async fn read_tokens_are_recorded_without_ever_writing_the_token() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    "/api/influx/read-token",
                    serde_json::json!({ "description": "Grafana" }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let token = body["token"].as_str().unwrap().to_string();
        let auth_id = body["auth_id"].as_str().unwrap().to_string();

        let (status, _, _) = h
            .call(with_cookie(
                empty_request("DELETE", &format!("/api/influx/read-tokens/{auth_id}")),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (_, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/audit"), &admin))
            .await;
        let rendered = body.to_string();
        assert!(!rendered.contains(&token), "le jeton ne doit jamais être journalisé");
        assert!(rendered.contains("influx.read_token.create"), "{rendered}");
        assert!(rendered.contains("influx.read_token.revoke"), "{rendered}");
    }

    #[tokio::test]
    async fn the_audit_log_never_carries_a_probe_token() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let (_, probe_token) = h.enroll(&admin, "Durand", "Paris").await;

        let (_, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/audit"), &admin))
            .await;
        assert!(
            !body.to_string().contains(&probe_token),
            "le jeton de sonde ne doit jamais entrer au journal"
        );
    }

    #[tokio::test]
    async fn account_changes_are_recorded() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        // Création par la route, pas en base : c'est elle qu'on journalise.
        h.call(with_cookie(
            json_request(
                "POST",
                "/api/users",
                serde_json::json!({ "username": "claire", "password": PASSWORD, "role": "viewer" }),
            ),
            &admin,
        ))
        .await;
        h.call(with_cookie(
            json_request("PATCH", "/api/users/claire", serde_json::json!({ "role": "operator" })),
            &admin,
        ))
        .await;
        h.call(with_cookie(
            json_request(
                "POST",
                "/api/users/claire/password",
                serde_json::json!({ "password": "un-autre-mot-de-passe" }),
            ),
            &admin,
        ))
        .await;
        h.call(with_cookie(
            json_request("PATCH", "/api/users/claire", serde_json::json!({ "disabled": true })),
            &admin,
        ))
        .await;

        for action in ["user.create", "user.role", "user.password", "user.disable"] {
            assert!(
                !h.audit(&admin, action).await.is_empty(),
                "« {action} » doit figurer au journal"
            );
        }
        let (_, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/audit"), &admin))
            .await;
        assert!(
            !body.to_string().contains("un-autre-mot-de-passe"),
            "un mot de passe ne doit jamais entrer au journal"
        );
    }

    // ── Notifications ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn only_an_admin_configures_the_channels() {
        let h = Harness::with_admin().await;
        h.login().await;
        h.user("olivier", Role::Operator).await;
        let operator = h.login_as("olivier").await;

        for (method, uri) in [
            ("GET", "/api/notifications"),
            ("DELETE", "/api/notifications/webhook"),
        ] {
            let (status, _, _) = h
                .call(with_cookie(empty_request(method, uri), &operator))
                .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        }
        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/webhook",
                    serde_json::json!({ "url": "https://hooks.example/x", "template": "slack" }),
                ),
                &operator,
            ))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, _) = h
            .call(with_cookie(
                json_request("POST", "/api/notifications/test", serde_json::json!({})),
                &operator,
            ))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_channels_are_configured_and_never_read_back() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/webhook",
                    serde_json::json!({
                        "url": "https://hooks.example/T000/xoxb-tres-secret",
                        "template": "discord",
                    }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/smtp",
                    serde_json::json!({
                        "host": "smtp.example.org",
                        "port": 587,
                        "security": "starttls",
                        "username": "hub",
                        "password": "mot-de-passe-tres-secret",
                        "from": "hub@example.org",
                        "to": ["ops@example.org"],
                    }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/notifications"), &admin))
            .await;
        assert_eq!(status, StatusCode::OK);
        let rendered = body.to_string();
        assert!(!rendered.contains("xoxb-tres-secret"), "{rendered}");
        assert!(!rendered.contains("hooks.example"), "{rendered}");
        assert!(!rendered.contains("mot-de-passe-tres-secret"), "{rendered}");
        assert_eq!(body["webhook"]["configured"], true);
        assert_eq!(body["webhook"]["template"], "discord");
        assert_eq!(body["smtp"]["configured"], true);
        assert_eq!(body["smtp"]["password_set"], true);
        assert_eq!(body["delay_secs"], 300);

        // Le journal non plus ne doit pas les porter.
        let (_, audit, _) = h
            .call(with_cookie(empty_request("GET", "/api/audit"), &admin))
            .await;
        let audit = audit.to_string();
        assert!(!audit.contains("xoxb-tres-secret"), "{audit}");
        assert!(!audit.contains("mot-de-passe-tres-secret"), "{audit}");
        assert!(audit.contains("notify.webhook"), "{audit}");
        assert!(audit.contains("notify.smtp"), "{audit}");
    }

    #[tokio::test]
    async fn unconfiguring_a_channel_is_a_route_of_its_own() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.call(with_cookie(
            json_request(
                "PUT",
                "/api/notifications/webhook",
                serde_json::json!({ "url": "https://hooks.example/x", "template": "slack" }),
            ),
            &admin,
        ))
        .await;

        let (status, _, _) = h
            .call(with_cookie(
                empty_request("DELETE", "/api/notifications/webhook"),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (_, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/notifications"), &admin))
            .await;
        assert_eq!(body["webhook"]["configured"], false);
    }

    #[tokio::test]
    async fn an_incomplete_channel_configuration_is_refused() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        for (uri, body) in [
            (
                "/api/notifications/webhook",
                serde_json::json!({ "url": "pas-une-url" }),
            ),
            (
                "/api/notifications/smtp",
                serde_json::json!({ "host": "", "port": 587, "from": "x@y", "to": ["a@b"] }),
            ),
            (
                "/api/notifications/smtp",
                serde_json::json!({ "host": "smtp.example.org", "port": 587, "from": "x@y", "to": [] }),
            ),
        ] {
            let (status, _, _) = h
                .call(with_cookie(json_request("PUT", uri, body.clone()), &admin))
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} {body}");
        }
    }

    #[tokio::test]
    async fn the_test_button_reports_each_channel_separately() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let hook = crate::notify::testing::FakeWebhook::start().await;
        h.call(with_cookie(
            json_request(
                "PUT",
                "/api/notifications/webhook",
                serde_json::json!({ "url": hook.url, "template": "slack" }),
            ),
            &admin,
        ))
        .await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request("POST", "/api/notifications/test", serde_json::json!({})),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let channels = body["channels"].as_array().unwrap();
        let webhook = channels.iter().find(|c| c["channel"] == "webhook").unwrap();
        assert_eq!(webhook["attempted"], true);
        assert_eq!(webhook["ok"], true);
        let smtp = channels.iter().find(|c| c["channel"] == "smtp").unwrap();
        assert_eq!(smtp["attempted"], false, "on ne prétend pas avoir essayé");

        assert_eq!(hook.received().len(), 1, "un vrai message doit partir");
    }

    #[tokio::test]
    async fn the_subscription_view_resolves_the_inheritance_for_the_interface() {
        // L'héritage est calculé côté serveur : le refaire en JavaScript est
        // exactement l'endroit où les deux se mettraient à diverger.
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let (paris, _) = h.enroll(&admin, "Durand", "Paris").await;
        let (banc, _) = h.enroll(&admin, "Durand", "Banc de test").await;
        let site_id = h.state.db.find_site_by_name("Durand").unwrap().unwrap().site_id;

        h.call(with_cookie(
            json_request(
                "PUT",
                "/api/notifications/subscriptions",
                serde_json::json!({ "scope": "site", "scope_id": site_id, "enabled": true }),
            ),
            &admin,
        ))
        .await;
        h.call(with_cookie(
            json_request(
                "PUT",
                "/api/notifications/subscriptions",
                serde_json::json!({ "scope": "probe", "scope_id": banc, "enabled": false }),
            ),
            &admin,
        ))
        .await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", "/api/notifications/subscriptions"),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let sites = body["sites"].as_array().unwrap();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0]["enabled"], true);

        let probes = body["probes"].as_array().unwrap();
        let paris_row = probes.iter().find(|p| p["probe_id"] == paris).unwrap();
        assert_eq!(paris_row["enabled"], true, "Paris hérite de son site");
        assert!(paris_row["exception"].is_null(), "sans exception");

        let banc_row = probes.iter().find(|p| p["probe_id"] == banc).unwrap();
        assert_eq!(banc_row["enabled"], false);
        assert_eq!(banc_row["exception"], false, "l'exception est visible");
    }

    #[tokio::test]
    async fn an_exception_can_be_given_back_to_its_site() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let (probe_id, _) = h.enroll(&admin, "Durand", "Paris").await;
        let site_id = h.state.db.find_site_by_name("Durand").unwrap().unwrap().site_id;
        h.call(with_cookie(
            json_request(
                "PUT",
                "/api/notifications/subscriptions",
                serde_json::json!({ "scope": "site", "scope_id": site_id, "enabled": true }),
            ),
            &admin,
        ))
        .await;
        h.call(with_cookie(
            json_request(
                "PUT",
                "/api/notifications/subscriptions",
                serde_json::json!({ "scope": "probe", "scope_id": probe_id, "enabled": false }),
            ),
            &admin,
        ))
        .await;

        // `enabled: null` veut dire « hérite ». Ce n'est pas une suppression.
        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/subscriptions",
                    serde_json::json!({ "scope": "probe", "scope_id": probe_id, "enabled": null }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(h.state.db.notify_enabled_for_probe(&probe_id).unwrap());
    }

    // ── Second facteur (contrat § 19) ─────────────────────────────────────

    /// Active le TOTP sur un compte et rend son secret.
    async fn enable_totp(h: &Harness, session: &str) -> String {
        let (status, body, _) = h
            .call(with_cookie(empty_request("POST", "/api/me/totp/start"), session))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let secret = body["secret"].as_str().unwrap().to_string();
        assert!(body["qr_svg"].as_str().unwrap().contains("<svg"));

        let code = current_code(&secret);
        let (status, body, _) = h
            .call(with_cookie(
                json_request("POST", "/api/me/totp/confirm", serde_json::json!({ "code": code })),
                session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        secret
    }

    fn current_code(secret: &str) -> String {
        code_for(secret, 0)
    }

    /// Le code du pas SUIVANT, accepté par la tolérance d'un pas.
    ///
    /// ⚠️ Nécessaire après une confirmation : celle-ci retient le pas qui
    /// vient de servir, et rejouer le même code serait — à raison — refusé.
    /// C'est aussi ce que vivra l'utilisateur qui se déconnecte et se
    /// reconnecte dans les trente secondes.
    fn next_code(secret: &str) -> String {
        code_for(secret, 1)
    }

    fn code_for(secret: &str, ahead: u64) -> String {
        // On rejoue le calcul du hub plutôt que de figer une valeur : un code
        // en dur expirerait trente secondes après avoir été écrit.
        let bytes = crate::totp::base32_decode(secret).unwrap();
        let step = crate::totp::step_of(crate::db::now()) + ahead;
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, &bytes);
        let tag = ring::hmac::sign(&key, &step.to_be_bytes());
        let d = tag.as_ref();
        let o = (d[d.len() - 1] & 0x0f) as usize;
        let v = ((u32::from(d[o]) & 0x7f) << 24)
            | (u32::from(d[o + 1]) << 16)
            | (u32::from(d[o + 2]) << 8)
            | u32::from(d[o + 3]);
        format!("{:06}", v % 1_000_000)
    }

    #[tokio::test]
    async fn the_secret_alone_does_not_arm_the_second_factor() {
        // 🔴 Activer dès la génération enfermerait dehors quiconque scanne mal
        // son QR : le compte exigerait un code que rien ne sait produire.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let (_, body, _) = h
            .call(with_cookie(empty_request("POST", "/api/me/totp/start"), &session))
            .await;
        assert!(body["secret"].is_string());

        let (_, status_body, _) = h
            .call(with_cookie(empty_request("GET", "/api/me/totp"), &session))
            .await;
        assert_eq!(status_body["enabled"], false, "pas encore actif");
        assert_eq!(status_body["pending"], true, "mais en attente de preuve");

        // Et la connexion par mot de passe seul marche encore.
        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({ "username": "admin", "password": PASSWORD }),
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn once_armed_the_password_alone_no_longer_opens_a_session() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let secret = enable_totp(&h, &session).await;

        let (status, body, cookies) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({ "username": "admin", "password": PASSWORD }),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["totp_required"], true, "le formulaire doit savoir quoi demander");
        assert!(
            cookies.is_empty(),
            "aucun cookie de session ne doit partir avant le second facteur"
        );

        // Un code faux ne passe pas davantage.
        let (status, _, _) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({
                    "username": "admin", "password": PASSWORD, "code": "000000"
                }),
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Le bon code, lui, ouvre la session.
        let (status, _, cookies) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({
                    "username": "admin",
                    "password": PASSWORD,
                    "code": next_code(&secret),
                }),
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!cookies.is_empty(), "la session s'ouvre avec le bon code");
    }

    #[tokio::test]
    async fn a_code_that_worked_cannot_be_replayed() {
        // 🔴 Un code vaut trente secondes. Sans mémoire du pas accepté, un
        // code intercepté rouvre une session autant de fois qu'on veut.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let secret = enable_totp(&h, &session).await;
        // Le pas suivant : celui de la confirmation est déjà consommé.
        let code = next_code(&secret);

        let (first, _, _) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({
                    "username": "admin", "password": PASSWORD, "code": code
                }),
            ))
            .await;
        assert_eq!(first, StatusCode::OK);

        let (second, _, cookies) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({
                    "username": "admin", "password": PASSWORD, "code": code
                }),
            ))
            .await;
        assert_eq!(second, StatusCode::UNAUTHORIZED, "le rejeu doit être refusé");
        assert!(cookies.is_empty());
    }

    #[tokio::test]
    async fn an_unreadable_secret_closes_the_door_instead_of_opening_it() {
        // 🔴 Volume restauré sans son `secret.key` : le drapeau dit « second
        // facteur exigé », le secret ne s'ouvre plus. Laisser passer sur le
        // mot de passe seul désactiverait la protection en silence, le jour
        // précis où l'on a des raisons de s'en méfier.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        enable_totp(&h, &session).await;
        h.state.secrets.clear("totp:admin").unwrap();

        let (status, body, _) = h
            .call(json_request(
                "POST",
                "/api/login",
                serde_json::json!({
                    "username": "admin", "password": PASSWORD, "code": "123456"
                }),
            ))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            body["error"].as_str().unwrap().contains("disable-totp"),
            "le refus doit nommer la sortie de secours : {body}"
        );
    }

    #[tokio::test]
    async fn disabling_the_second_factor_requires_the_password() {
        // Sans le mot de passe, une session volée suffirait à retirer le
        // second facteur — c'est-à-dire à annuler ce contre quoi il protège.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        enable_totp(&h, &session).await;

        let (status, _, _) = h
            .call(with_cookie(
                json_request("DELETE", "/api/me/totp", serde_json::json!({ "password": "faux" })),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(h.state.db.totp_enabled("admin").unwrap(), "toujours actif");

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "DELETE",
                    "/api/me/totp",
                    serde_json::json!({ "password": PASSWORD }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!h.state.db.totp_enabled("admin").unwrap());
        assert!(
            !h.state.secrets.is_set("totp:admin"),
            "le secret part avec : un ancien code ne doit pas revivre sur un QR qu'on croit neuf"
        );
    }

    #[tokio::test]
    async fn a_second_factor_closes_the_scripted_enrolment_path() {
        // 🔴 Ce chemin n'a pas d'écran : il ne peut pas présenter de code.
        // Le laisser passer ferait du mot de passe seul une clé valide pour un
        // compte qui vient de déclarer le contraire.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        enable_totp(&h, &session).await;

        let (status, body, _) = h
            .call(json_request(
                "POST",
                "/api/probes/enroll",
                serde_json::json!({
                    "username": "admin",
                    "password": PASSWORD,
                    "site": "Durand",
                    "name": "Sonde scriptée",
                }),
            ))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("code d'enrôlement"),
            "le refus doit nommer le chemin qui marche : {body}"
        );
    }

    // ── Portée par site (contrat § 17) ────────────────────────────────────

    /// Deux clients, deux sites, et un compte qui n'a le droit d'en voir
    /// qu'un. C'est le décor de tous les tests de portée qui suivent.
    async fn two_clients_and_a_restricted_viewer(
    ) -> (Harness, String, String, String, String, String) {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let (probe_a, _) = h.enroll(&admin, "Durand", "Paris").await;
        let (probe_b, _) = h.enroll(&admin, "Martin", "Lyon").await;
        let site_a = h.state.db.find_site_by_name("Durand").unwrap().unwrap().site_id;
        let site_b = h.state.db.find_site_by_name("Martin").unwrap().unwrap().site_id;

        h.user("claire", Role::Viewer).await;
        h.state
            .db
            .set_user_sites("claire", &[site_a.clone()])
            .unwrap();
        let viewer = h.login_as("claire").await;
        (h, viewer, site_a, site_b, probe_a, probe_b)
    }

    #[tokio::test]
    async fn no_row_means_every_site_so_an_update_locks_nobody_out() {
        // 🔴 Le point : le codage inverse — une ligne par site autorisé,
        // obligatoire — aurait enfermé tous les comptes existants hors de
        // leur parc à la migration. L'absence de ligne DOIT valoir « tout ».
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.enroll(&admin, "Durand", "Paris").await;
        h.user("claire", Role::Viewer).await;
        let viewer = h.login_as("claire").await;

        let (status, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/probes"), &viewer))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_array().unwrap().len(), 1, "aucune portée = tout voir");
    }

    #[tokio::test]
    async fn a_restricted_account_only_lists_its_own_sites_and_probes() {
        let (h, viewer, site_a, _site_b, _pa, _pb) = two_clients_and_a_restricted_viewer().await;

        let (_, sites, _) = h
            .call(with_cookie(empty_request("GET", "/api/sites"), &viewer))
            .await;
        let sites = sites.as_array().unwrap();
        assert_eq!(sites.len(), 1, "un seul site visible");
        assert_eq!(sites[0]["site_id"].as_str().unwrap(), site_a);

        let (_, probes, _) = h
            .call(with_cookie(empty_request("GET", "/api/probes"), &viewer))
            .await;
        let probes = probes.as_array().unwrap();
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0]["site"].as_str().unwrap(), "Durand");
    }

    #[tokio::test]
    async fn asking_for_a_site_outside_the_scope_returns_nothing_not_everything() {
        // Le paramètre d'URL ne doit pas devenir une clé : demander le site
        // d'un autre ne rend ni son parc, ni le sien par repli.
        let (h, viewer, _a, site_b, _pa, _pb) = two_clients_and_a_restricted_viewer().await;
        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes?site={site_b}")),
                &viewer,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn every_per_probe_route_answers_404_outside_the_scope() {
        // 🔴 404 et non 403 : « ça existe mais pas pour vous » révèle déjà
        // l'existence du site d'un autre client. Ce test balaie TOUTES les
        // routes qui désignent une sonde — c'est le filet qui rattrape une
        // route ajoutée plus tard sans son garde.
        let (h, viewer, _a, _b, probe_a, probe_b) = two_clients_and_a_restricted_viewer().await;

        for path in [
            format!("/api/probes/{probe_b}/metrics"),
            format!("/api/probes/{probe_b}/inventory"),
            format!("/api/probes/{probe_b}/sla.csv"),
            format!("/api/probes/{probe_b}/sla"),
        ] {
            let (status, _, _) = h
                .call(with_cookie(empty_request("GET", &path), &viewer))
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path} doit répondre 404");
        }

        // Et la sonde de son propre site répond, elle : la portée restreint,
        // elle ne casse pas.
        let (status, _, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_a}/inventory?kind=ports")),
                &viewer,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_restricted_operator_cannot_touch_another_clients_site() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let (_, _) = h.enroll(&admin, "Durand", "Paris").await;
        let (probe_b, _) = h.enroll(&admin, "Martin", "Lyon").await;
        let site_a = h.state.db.find_site_by_name("Durand").unwrap().unwrap().site_id;
        let site_b = h.state.db.find_site_by_name("Martin").unwrap().unwrap().site_id;
        h.user("olivier", Role::Operator).await;
        h.state.db.set_user_sites("olivier", &[site_a.clone()]).unwrap();
        let op = h.login_as("olivier").await;

        // Renommer le site d'un autre.
        let (status, _, _) = h
            .call(with_cookie(
                json_request("PATCH", &format!("/api/sites/{site_b}"), serde_json::json!({ "name": "Volé" })),
                &op,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            h.state.db.get_site(&site_b).unwrap().name,
            "Martin",
            "le site ne doit pas avoir bougé"
        );

        // Poser une sonde chez un autre.
        let (status, _, _) = h
            .call(with_cookie(
                json_request("POST", "/api/enroll-codes", serde_json::json!({ "site_id": site_b })),
                &op,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Piloter la sonde d'un autre.
        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PATCH",
                    &format!("/api/probes/{probe_b}"),
                    serde_json::json!({ "name": "Volée" }),
                ),
                &op,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_probe_cannot_be_moved_out_of_the_operators_scope() {
        // L'intergiciel valide la sonde d'ORIGINE ; sans ce contrôle-ci, la
        // destination passait — et la sonde partait chez un autre client.
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        let (probe_a, _) = h.enroll(&admin, "Durand", "Paris").await;
        h.enroll(&admin, "Martin", "Lyon").await;
        let site_a = h.state.db.find_site_by_name("Durand").unwrap().unwrap().site_id;
        let site_b = h.state.db.find_site_by_name("Martin").unwrap().unwrap().site_id;
        h.user("olivier", Role::Operator).await;
        h.state.db.set_user_sites("olivier", &[site_a.clone()]).unwrap();
        let op = h.login_as("olivier").await;

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PATCH",
                    &format!("/api/probes/{probe_a}"),
                    serde_json::json!({ "site_id": site_b }),
                ),
                &op,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(h.state.db.get_probe(&probe_a).unwrap().site_id, site_a);
    }

    #[tokio::test]
    async fn pending_enrollment_codes_of_another_client_are_never_listed() {
        // Ces lignes portent le code EN CLAIR : le filtre n'est pas cosmétique,
        // il est le contrôle. Un code suffit à poser une sonde chez l'autre.
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.enroll(&admin, "Durand", "Paris").await;
        let site_a = h.state.db.find_site_by_name("Durand").unwrap().unwrap().site_id;
        let (_, site_b_json, _) = h
            .call(with_cookie(
                json_request("POST", "/api/sites", serde_json::json!({ "name": "Martin" })),
                &admin,
            ))
            .await;
        let site_b = site_b_json["site_id"].as_str().unwrap().to_string();
        h.call(with_cookie(
            json_request("POST", "/api/enroll-codes", serde_json::json!({ "site_id": site_b })),
            &admin,
        ))
        .await;

        h.user("olivier", Role::Operator).await;
        h.state.db.set_user_sites("olivier", &[site_a]).unwrap();
        let op = h.login_as("olivier").await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", "/api/enroll-codes/pending"),
                &op,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.as_array().unwrap().is_empty(),
            "aucun code d'un autre site ne doit sortir : {body}"
        );
    }

    #[tokio::test]
    async fn an_admin_can_never_be_restricted() {
        // 🔴 Il gère les comptes : il lèverait sa propre restriction en trois
        // clics. Une limite qu'on peut retirer soi-même n'est pas une limite,
        // et la promettre serait pire que ne rien promettre.
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.enroll(&admin, "Durand", "Paris").await;
        let site_a = h.state.db.find_site_by_name("Durand").unwrap().unwrap().site_id;
        h.user("alice", Role::Admin).await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PATCH",
                    "/api/users/alice",
                    serde_json::json!({ "sites": [site_a] }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(h.state.db.list_user_sites("alice").unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_scope_set_then_cleared_gives_everything_back() {
        let (h, viewer, site_a, _b, _pa, _pb) = two_clients_and_a_restricted_viewer().await;
        let admin = h.login().await;
        let _ = site_a;

        // `[]` est un ordre — « plus de restriction » — et non un silence.
        let (status, _, _) = h
            .call(with_cookie(
                json_request("PATCH", "/api/users/claire", serde_json::json!({ "sites": [] })),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (_, probes, _) = h
            .call(with_cookie(empty_request("GET", "/api/probes"), &viewer))
            .await;
        assert_eq!(probes.as_array().unwrap().len(), 2, "la portée est levée");
    }

    #[tokio::test]
    async fn a_viewer_reads_the_subscriptions_but_does_not_change_them() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.enroll(&admin, "Durand", "Paris").await;
        let site_id = h.state.db.find_site_by_name("Durand").unwrap().unwrap().site_id;
        h.user("claire", Role::Viewer).await;
        let viewer = h.login_as("claire").await;

        let (status, _, _) = h
            .call(with_cookie(
                empty_request("GET", "/api/notifications/subscriptions"),
                &viewer,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/subscriptions",
                    serde_json::json!({ "scope": "site", "scope_id": site_id, "enabled": true }),
                ),
                &viewer,
            ))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(h.state.db.list_notify_subscriptions().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_operator_toggles_a_site_and_it_is_recorded() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.enroll(&admin, "Durand", "Paris").await;
        let site_id = h.state.db.find_site_by_name("Durand").unwrap().unwrap().site_id;
        h.user("olivier", Role::Operator).await;
        let operator = h.login_as("olivier").await;

        let (status, _, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/subscriptions",
                    serde_json::json!({ "scope": "site", "scope_id": site_id, "enabled": true }),
                ),
                &operator,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let entries = h.audit(&admin, "notify.subscription").await;
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0]["actor"], "olivier");
    }

    #[tokio::test]
    async fn the_alert_delay_is_a_plain_setting() {
        // Le seul réglage de notification qui n'est pas un secret.
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/settings",
                    serde_json::json!({ "notify_delay_secs": "600" }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["notify_delay_secs"], 600);
        assert_eq!(h.state.settings.notify_delay_secs(), 600);
    }

    #[tokio::test]
    async fn editing_a_channel_does_not_wipe_the_secret_it_cannot_show() {
        // L'interface n'a jamais vu le mot de passe ni l'URL : les lui faire
        // renvoyer pour corriger un port reviendrait à les effacer un jour
        // sur deux.
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.call(with_cookie(
            json_request(
                "PUT",
                "/api/notifications/smtp",
                serde_json::json!({
                    "host": "smtp.example.org", "port": 587,
                    "password": "mot-de-passe-smtp",
                    "from": "hub@example.org", "to": ["ops@example.org"],
                }),
            ),
            &admin,
        ))
        .await;
        h.call(with_cookie(
            json_request(
                "PUT",
                "/api/notifications/webhook",
                serde_json::json!({ "url": "https://hooks.example/x", "template": "slack" }),
            ),
            &admin,
        ))
        .await;

        // Second envoi sans le mot de passe, sans l'URL : on ne corrige que
        // le port et le modèle.
        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/smtp",
                    serde_json::json!({
                        "host": "smtp.example.org", "port": 2525,
                        "from": "hub@example.org", "to": ["ops@example.org"],
                    }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["smtp"]["port"], 2525);
        assert_eq!(body["smtp"]["password_set"], true, "le mot de passe a été effacé");

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/webhook",
                    serde_json::json!({ "template": "discord" }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["webhook"]["template"], "discord");
        assert_eq!(
            h.state.notifier.webhook_config().unwrap().url,
            "https://hooks.example/x",
            "l'URL a été perdue"
        );
    }

    #[tokio::test]
    async fn an_explicit_null_does_remove_the_smtp_password() {
        // Absent veut dire « ne touche pas » ; `null` veut dire « retire-le ».
        let h = Harness::with_admin().await;
        let admin = h.login().await;
        h.call(with_cookie(
            json_request(
                "PUT",
                "/api/notifications/smtp",
                serde_json::json!({
                    "host": "smtp.example.org", "port": 587,
                    "password": "mot-de-passe-smtp",
                    "from": "hub@example.org", "to": ["ops@example.org"],
                }),
            ),
            &admin,
        ))
        .await;

        let (_, body, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/smtp",
                    serde_json::json!({
                        "host": "smtp.example.org", "port": 587, "password": null,
                        "from": "hub@example.org", "to": ["ops@example.org"],
                    }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(body["smtp"]["password_set"], false);
    }

    #[tokio::test]
    async fn a_webhook_without_url_and_without_history_is_refused() {
        let h = Harness::with_admin().await;
        let admin = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/webhook",
                    serde_json::json!({ "template": "slack" }),
                ),
                &admin,
            ))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
}
