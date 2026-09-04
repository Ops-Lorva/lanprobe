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
    extract::{ConnectInfo, Path, Query, State},
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
    /// Empreinte SHA-256 du certificat que le hub présente, `AB:CD:…`.
    ///
    /// 🔴 C'est ce que le QR d'appairage transporte (contrat § 22) : le
    /// téléphone épingle le certificat auto-signé du hub, et l'empreinte lui
    /// arrive par un canal que l'utilisateur contrôle physiquement — l'écran
    /// qu'il a sous les yeux — plutôt que par le réseau qu'on cherche
    /// justement à authentifier.
    ///
    /// ⚠️ `None` quand le hub sert **en clair** derrière un reverse proxy : il
    /// n'y a alors aucun certificat du hub à épingler, celui que le téléphone
    /// verra est celui du proxy. Annoncer une empreinte dans ce cas la ferait
    /// comparer à un certificat qui n'est pas le sien, et l'appairage
    /// échouerait sans que rien ne dise pourquoi.
    ///
    /// Calculée **une fois au démarrage**, sur le certificat servi : le
    /// certificat est conservé entre deux démarrages, l'empreinte ne bouge
    /// donc pas — sinon chaque redémarrage invaliderait celle qu'ont épinglée
    /// tous les téléphones appairés.
    pub cert_fingerprint: Option<String>,
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
        // ⚠️ **Ces quatre routes portent un `{id}` de sonde et ne passent PAS
        // par la garde de portée** : elles s'authentifient au jeton de la sonde
        // elle-même. `authenticate_probe` fait le `get_probe`, mais transforme
        // l'absence en **401**, volontairement indiscernable d'un mauvais
        // jeton.
        //
        // 🔴 **Ne pas les « aligner » sur le 404 des routes de session par
        // cohérence apparente.** Un 404 ici apprendrait à qui présente un jeton
        // quelconque quelles sondes existent — c'est-à-dire exactement ce que
        // le 404 protège ailleurs, retourné contre lui-même. La règle commune
        // n'est pas « le même code partout », c'est « ne rien révéler à qui n'a
        // pas déjà le droit de le savoir ».
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
            .route("/api/me/lang", post(set_own_lang))
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
            .route("/api/enroll-codes", post(create_enroll_code))
            .route("/api/enroll-codes/pending", get(list_pending_codes))
            // Nommer un réseau est un acte de conduite du parc, pas une
            // consultation : le libellé se retrouve dans tous les rapports.
            .route("/api/networks/label", put(set_network_label))
            .route("/api/settings/influx-advertise/test", post(test_advertise))
            .route(
                "/api/notifications/subscriptions",
                put(put_subscription),
            ),
    );

    // Les routes dont le `{id}` du chemin désigne UN SITE, derrière leur
    // propre garde de portée.
    //
    // 🔴 **Elles appelaient `site_in_scope` à la main dans chaque handler.**
    // Une garde qu'il faut penser à écrire finit par être oubliée — c'est le
    // motif que le contrat § 17 décrit comme revenu trois fois côté sondes, et
    // il était déjà là, en production, sur ces trois routes-ci. Un handler
    // ajouté demain hérite désormais du garde sans que personne y pense.
    let site_write = guarded_site_scope(
        &state,
        Role::Operator,
        Router::new()
            .route("/api/sites/{id}", patch(rename_site).delete(delete_site))
            .route("/api/sites/{id}/archive", post(archive_site)),
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
            .route("/api/probes/{id}/uptime", get(probe_uptime))
            .route("/api/probes/{id}/inventory", get(probe_inventory))
            .route("/api/probes/{id}/sla.csv", get(probe_sla_csv))
            .route("/api/probes/{id}/public-ips", get(probe_public_ips))
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
            .route("/api/probes/{id}/realtime", post(set_realtime))
            .route("/api/probes/{id}/monitors/revive", post(revive_monitor))
            .route("/api/probes/{id}/monitors/remove", post(remove_monitor_entry))
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
        .merge(site_write)
        .merge(probe_write)
        .merge(administration)
        // Les deux domaines de l'app mobile portent leurs routes eux-mêmes.
        // ⚠️ Ils ne sont PAS déclarés ici : `web.rs` fait déjà 11 600 lignes,
        // et deux tâches parallèles ne peuvent pas s'y partager le routeur.
        // Chacune tient son module, celui-ci ne tient que la couture.
        .merge(crate::pairing::routes(&state))
        .merge(crate::reports::routes(&state))
        .merge(crate::packages::routes(&state))
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
pub(crate) fn guarded(state: &AppState, minimum: Role, router: Router<AppState>) -> Router<AppState> {
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

/// Comme [`guarded`], plus la portée par site — pour les routes dont le `{id}`
/// du chemin est un **`site_id`**.
///
/// ⚠️ **C'est le jumeau de [`guarded_probe_scope`], pas un doublon.** Les deux
/// paramètres de chemin s'appellent `id` et ne désignent pas le même objet :
/// un garde unique aurait cherché une sonde là où passe un site. La symétrie
/// est dans la règle appliquée, pas dans le code.
///
/// 🔴 **Pourquoi une couche plutôt qu'un appel dans chaque handler.** La
/// vérification de site était une fonction ordinaire, appelée à la main par
/// `rename_site`, `delete_site` et `archive_site`. Elle marchait — jusqu'au
/// handler où quelqu'un oublierait de l'écrire. C'est très exactement le motif
/// que le contrat § 17 documente comme **revenu trois fois** côté sondes, et
/// que `9f95a12` a fini par reprendre au niveau de la garde plutôt que route
/// par route. Ici, un handler ajouté demain en hérite sans que personne y
/// pense.
///
/// ⚠️ Ce que la couche NE couvre PAS, et qui garde donc son appel explicite :
/// un site nommé dans le **corps** de la requête — code d'enrôlement, libellé
/// de réseau, abonnement d'alerte, destination d'un déplacement de sonde. Un
/// intergiciel ne lit que l'URL.
pub(crate) fn guarded_site_scope(
    state: &AppState,
    minimum: Role,
    router: Router<AppState>,
) -> Router<AppState> {
    router
        .layer(middleware::from_fn_with_state(state.clone(), site_scope_guard))
        .layer(middleware::from_fn_with_state((state.clone(), minimum), role_guard))
        .layer(middleware::from_fn_with_state(state.clone(), session_guard))
}

/// Rend 404 quand la sonde visée n'existe pas **ou** sort de la portée de
/// l'appelant.
///
/// ⚠️ **404 et non 403.** « Ça existe mais pas pour vous » révèle déjà
/// l'existence du site d'un autre client — précisément ce que la portée
/// protège. Une sonde hors portée doit être indistinguable d'une sonde qui
/// n'existe pas.
///
/// 🔴 **Elle vérifie l'existence pour TOUS les comptes, y compris `Scope::All`.**
/// Elle court-circuitait pour eux : l'existence n'était alors vérifiée que par
/// EFFET DE BORD, dans le `get_probe` que fait `probe_in_scope` pour les comptes
/// restreints. Une route qui ne contrôlait rien elle-même paraissait donc
/// gardée, l'était pour un opérateur, et ne l'était pas pour l'administrateur —
/// c'est-à-dire pour l'utilisateur le plus susceptible de taper un identifiant
/// à la main dans une URL.
///
/// ⚠️ **Et c'est un angle mort de la SUITE DE TESTS, pas seulement du code** :
/// un test écrit avec un compte restreint reste vert quoi qu'on casse sur ce
/// chemin. C'est ce qui a laissé le même défaut revenir trois fois — routes de
/// moniteurs, `/public-ips`, `GET /commands` — chacune rendant une réponse
/// plausible et fausse (« aucun relevé », « aucune commande ») là où la vérité
/// était « cette sonde n'existe pas ». Tout test de cette garde doit être écrit
/// avec un compte `Scope::All`.
///
/// Le coût est d'un `get_probe` par requête, que quatorze des quinze routes
/// gardées faisaient déjà pour leur propre compte.
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

/// Rend 404 quand le site visé n'existe pas **ou** sort de la portée de
/// l'appelant. Même règle, même code, même message que pour une sonde.
///
/// 🔴 **Elle vérifie l'existence pour TOUS les comptes, `Scope::All` compris**
/// (contrat § 17.3). L'existence d'un site tenait par **effet de bord** — un
/// `UPDATE` qui ne touche aucune ligne finit par se voir — et elle a lâché
/// exactement là où l'effet de bord n'avait pas lieu.
///
/// ⚠️ **La garde parle AVANT que le corps de la requête soit lu.** Sur un site
/// inconnu, un corps invalide rend donc `404` et non `400`. C'est le bon
/// ordre : « ce site n'existe pas » est la plus vraie des deux réponses, et
/// corriger son JSON n'aurait rien changé.
async fn site_scope_guard(
    State(state): State<AppState>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    use axum::extract::RawPathParams;
    use axum::RequestExt;

    // L'identité est posée par `session_guard`, qui tourne avant celui-ci.
    let Some(actor) = req.extensions().get::<Identity>().cloned() else {
        return next.run(req).await;
    };

    let site_id = match req.extract_parts::<RawPathParams>().await {
        Ok(params) => params
            .iter()
            .find(|(name, _)| *name == "id")
            .map(|(_, value)| value.to_string()),
        Err(_) => None,
    };
    if let Some(id) = site_id {
        if let Err(refusal) = site_in_scope(&state, &actor, &id) {
            return refusal;
        }
    }
    next.run(req).await
}

// ── Erreurs ────────────────────────────────────────────────────────────────

/// Traduit une erreur du stockage en code HTTP. Les handlers ne devinent pas
/// le code d'après le texte du message — ils se tromperaient.
pub(crate) fn error_response(e: DbError) -> Response {
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

pub(crate) fn ok_json(value: serde_json::Value) -> Response {
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
/// ⚠️ `lang` peut être `null` : « rien de réglé », et c'est alors au
/// navigateur de choisir — le comportement d'avant le réglage. Un défaut
/// inventé côté hub imposerait une langue à tous les comptes existants.
async fn me(State(state): State<AppState>, Extension(identity): Extension<Identity>) -> Response {
    let lang = state.db.user_ui_locale(&identity.username).unwrap_or(None);
    ok_json(json!({
        "username": identity.username,
        "role": identity.role.as_str(),
        "lang": lang,
    }))
}

#[derive(serde::Deserialize)]
struct OwnLangBody {
    /// `null` retire le réglage et rend le compte au choix du navigateur.
    lang: Option<String>,
}

/// La langue de l'INTERFACE, par compte.
///
/// 🔴 **Elle suit le compte, pas le navigateur.** Le sélecteur n'écrivait que
/// dans `localStorage` : changer de poste ramenait l'anglais. `localStorage`
/// reste un cache local — il évite un éclair au chargement — mais la base fait
/// foi.
///
/// ⚠️ Ce n'est **pas** la langue des rapports : celle-là appartient au site
/// (§ 23). Les confondre laisserait le même client recevoir deux langues.
async fn set_own_lang(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(body): Json<OwnLangBody>,
) -> Response {
    if let Some(refus) = refus_si_langue_inconnue(body.lang.as_deref()) {
        return refus;
    }
    match state
        .db
        .set_user_ui_locale(&identity.username, body.lang.as_deref())
    {
        Ok(()) => ok_json(json!({ "ok": true, "lang": body.lang })),
        Err(e) => error_response(e),
    }
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
    /// L'appareil appairé d'où vient la requête (contrat § 22). `None` pour
    /// une session de navigateur.
    ///
    /// ⚠️ **Il ne porte aucun droit** : le rôle et la portée restent ceux du
    /// compte, relus à chaque requête. Il sert à dire depuis QUEL téléphone un
    /// rapport a été demandé (§ 23) — l'audit dirait « benjamin », ce qui est
    /// vrai et trompeur le jour où le téléphone est entre d'autres mains.
    pub device_id: Option<String>,
}

/// Rend l'identité d'un compte actif. `None` si le compte a été désactivé
/// pendant que sa session courait — la session ne vaut alors plus rien.
///
/// ⚠️ **C'est le seul endroit qui construit une `Identity`.** Session,
/// identifiants du compte, jeton d'appareil : trois chemins d'authentification,
/// une seule fabrique. Deux fabriques, c'est une qui oubliera un jour de relire
/// le rôle ou la portée — et une portée figée survivrait à sa révocation.
///
/// ⚠️ `pub(crate)` : le jeton d'accès d'un appareil (§ 22) passe par elle, et
/// c'est précisément ce qu'on veut — une seconde fabrique dans `pairing.rs`
/// serait celle qui oublierait un jour de relire le rôle ou la portée.
pub(crate) fn identity_of(
    state: &AppState,
    username: &str,
    device_id: Option<String>,
) -> Option<Identity> {
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
        device_id,
    })
}

/// Refuse un site inexistant **ou** hors portée, en **404**.
///
/// ⚠️ Pas 403 : « ça existe mais pas pour vous » révèle déjà l'existence du
/// site d'un autre client, ce qui est exactement ce que la portée protège.
/// Un site hors portée doit être indistinguable d'un site inexistant.
///
/// 🔴 **L'existence est vérifiée ICI, pour tous les comptes.** La portée d'un
/// compte `Scope::All` autorise tout : sans ce `get_site`, la garde ne
/// contrôlait rien pour un administrateur, et l'existence n'était rattrapée
/// qu'en aval, par accident — un `UPDATE` qui ne touche aucune ligne. Là où
/// personne ne rattrapait (`notify_subscriptions` ne porte aucune clé
/// étrangère), un réglage d'alerte partait en base pour un parc imaginaire.
/// Une garde qui tient par effet de bord finit par lâcher là où l'effet de
/// bord n'existe pas.
fn site_in_scope(state: &AppState, actor: &Identity, site_id: &str) -> Result<(), Response> {
    match state.db.get_site(site_id) {
        Ok(_) if actor.scope.allows(site_id) => Ok(()),
        _ => Err(fail(StatusCode::NOT_FOUND, "site inconnu")),
    }
}

/// Même règle pour une sonde : inexistante et hors portée se répondent à
/// l'identique, et l'existence est vérifiée pour tous les comptes.
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
    // Le cookie d'abord — c'est le chemin du navigateur, le seul en
    // production aujourd'hui, et il ne doit pas pouvoir régresser.
    let identity = session_of(&headers)
        .and_then(|t| state.auth.validate(&t))
        .and_then(|username| identity_of(&state, &username, None))
        // Puis le porteur : un appareil appairé présente un jeton d'accès de
        // quinze minutes, jamais un cookie (contrat § 22).
        //
        // ⚠️ Le refus ne nomme PAS le motif ici, et c'est assumé : le motif
        // d'un refus d'appareil — révoqué, inconnu, compte désactivé — se rend
        // sur `POST /api/devices/token`, la route que l'app appelle pour
        // renouveler. Le dire sur chaque route protégée apprendrait à qui
        // présente un jeton quelconque quels appareils existent.
        //
        // 🔴 **L'asymétrie avec les `401` de `pairing.rs` est VOULUE, ne
        // l'uniformisez pas.** Là-bas le motif est explicite parce que l'app
        // doit distinguer trois conduites — appeler celui qui a révoqué,
        // réappairer, ou s'arrêter. Ici il n'y a rien à distinguer : l'app
        // renouvelle son jeton, et c'est le renouvellement qui lui dira quoi
        // faire. Un motif ici ne servirait que celui qui n'a pas d'appareil.
        .or_else(|| {
            bearer_token(&headers)
                .and_then(|token| crate::pairing::identity_from_access_token(&state, &token))
        });
    match identity {
        Some(identity) => {
            req.extensions_mut().insert(identity);
            next.run(req).await
        }
        // 🔴 **Pas « session expirée ».** Le mot serait faux sur le chemin du
        // porteur : dans ce modèle, l'identité d'un appareil appairé ne meurt
        // que d'une révocation, rien n'y expire (contrat § 22). Il ferait
        // attendre au technicien une reconnexion qui n'arrivera pas.
        // « Authentification requise » est vrai des deux chemins — cookie
        // absent comme porteur invalide — et n'en dit pas un mot de plus.
        None => fail(StatusCode::UNAUTHORIZED, "authentification requise"),
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
        // Même formulation qu'en amont, et pour la même raison : ce garde
        // s'applique aux deux chemins d'authentification.
        return fail(StatusCode::UNAUTHORIZED, "authentification requise");
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
pub(crate) fn audit(
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
pub(crate) fn audited<T>(
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
        .and_then(|username| identity_of(&state, &username, None));
    if identity.is_none()
        && let Some((username, password)) = basic_credentials(&headers)
        && state.db.verify_credentials(&username, &password).is_ok()
    {
        identity = identity_of(&state, &username, None);
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
        "site" => site_in_scope(&state, &actor, &body.scope_id).is_ok(),
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
    /// 🔴 **La langue des rapports de ce client** (contrat § 23), réglée là où
    /// on nomme le site parce que c'est une propriété du client.
    ///
    /// ⚠️ **Trois états, pas deux.** `Option<Option<_>>` distingue « absent »
    /// (un renommage qui ne touche pas au réglage) de `null` (« remets le
    /// défaut du hub »). Une simple `Option` aurait fait effacer la langue
    /// d'un client à chaque renommage, en silence.
    #[serde(default, deserialize_with = "champ_present_meme_a_null")]
    report_locale: Option<Option<String>>,
}

/// Rend `Some(_)` dès que la clé est **présente**, `Some(None)` sur `null`.
/// Absente, `#[serde(default)]` donne `None` sans passer par ici.
fn champ_present_meme_a_null<'de, D>(d: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    Ok(Some(Option::<String>::deserialize(d)?))
}

/// Refus commun aux deux réglages de langue.
///
/// ⚠️ Strict — exactement `fr`, `en` ou `es`. La lecture, elle, pardonne
/// (`report_i18n::resolve_locale`) : un rapport ne doit jamais échouer sur un
/// réglage illisible, mais un réglage accepté puis jamais appliqué se
/// découvrirait sur le document remis au client.
fn refus_si_langue_inconnue(lang: Option<&str>) -> Option<Response> {
    match lang {
        Some(l) if !crate::report_i18n::is_supported(l) => Some(fail(
            StatusCode::BAD_REQUEST,
            &format!("langue inconnue : « {l} »"),
        )),
        _ => None,
    }
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
///
/// ⚠️ Le site du chemin est garanti **existant et dans la portée** par
/// `site_scope_guard` : ce handler n'a plus à le vérifier pour son propre
/// compte (contrat § 17).
async fn archive_site(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<ArchiveBody>,
) -> Response {
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

/// Les réglages du site : son nom, et la langue de ses rapports.
///
/// ⚠️ Site du chemin garanti existant et dans la portée par
/// `site_scope_guard`.
async fn rename_site(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<SiteBody>,
) -> Response {
    // ⚠️ Le refus vient AVANT le renommage : un `400` sur la langue ne doit
    // pas laisser derrière lui un site à moitié renommé.
    if let Some(refus) = refus_si_langue_inconnue(
        body.report_locale
            .as_ref()
            .and_then(|l| l.as_deref()),
    ) {
        return refus;
    }

    let site = match audited(
        &state,
        Some(&actor.username),
        "site.rename",
        Some(&id),
        state.db.rename_site(&id, &body.name),
    ) {
        Ok(site) => site,
        Err(e) => return error_response(e),
    };

    // Absent = « on n'y touche pas ». C'est ce qui permet à un renommage seul
    // de laisser le réglage du client intact.
    let Some(demandée) = body.report_locale else {
        return ok_json(serde_json::to_value(site).unwrap_or(serde_json::Value::Null));
    };

    // Ce réglage change ce qu'un CLIENT reçoit : il laisse une trace, au même
    // titre que le renommage.
    match audited(
        &state,
        Some(&actor.username),
        "site.report_locale",
        Some(&id),
        state.db.set_site_report_locale(&id, demandée.as_deref()),
    ) {
        Ok(site) => ok_json(serde_json::to_value(site).unwrap_or(serde_json::Value::Null)),
        Err(e) => error_response(e),
    }
}

/// ⚠️ Site du chemin garanti existant et dans la portée par
/// `site_scope_guard`.
async fn delete_site(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
) -> Response {
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
    if let Err(refusal) = site_in_scope(&state, &actor, body.site_id.trim()) {
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

/// Un ajout ou un retrait annoncé par la sonde, avec son **ancienneté**.
///
/// ⚠️ `age_secs`, jamais une date. La sonde la mesure sur son horloge monotone
/// et le hub la soustrait de son propre `now()` : deux horloges ne sont jamais
/// comparées. Accepter une date à la place réintroduirait la dérive — et un
/// portable qui sort de veille saute de plusieurs minutes d'un coup, ce qui
/// lui ferait gagner des arbitrages qu'il doit perdre, sans que rien ne le
/// signale.
#[derive(Deserialize)]
struct MonitorChange {
    target: String,
    /// `add` | `remove`.
    action: String,
    #[serde(default)]
    age_secs: i64,
}

/// Ancienneté maximale acceptée dans un changement de surveillance : 30 jours.
///
/// ⚠️ Cette borne n'est pas une coquetterie. Une valeur aberrante — champ
/// corrompu, bug de la sonde, sonde malveillante — daterait le changement en
/// 1970 et lui ferait gagner **tous** les arbitrages à l'envers : plus aucun
/// ajout fait depuis le hub ne pourrait défaire ce retrait. Au-delà de la
/// borne on ignore la valeur et on date à la réception : on perd la précision
/// d'un cas invraisemblable, on ne perd jamais la main.
const MAX_MONITOR_AGE_SECS: i64 = 30 * 24 * 3600;

/// Convertit l'ancienneté annoncée en date dans la référence du hub.
fn monitor_change_instant(age_secs: i64) -> i64 {
    let now = crate::db::now();
    // Négatif = un changement dans le futur, qui gagnerait tout lui aussi.
    if !(0..=MAX_MONITOR_AGE_SECS).contains(&age_secs) {
        return now;
    }
    now - age_secs
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
    /// Changements de surveillance non encore accusés (ajouts et retraits).
    ///
    /// ⚠️ **Ce n'est pas la liste de la sonde, ce sont ses actes.** Une cible
    /// qui n'y figure pas n'a rien subi : c'est toute la conception. En
    /// déduire un retrait est le défaut qu'on corrige — l'app qui redémarre
    /// effaçait en silence ce qu'on avait ajouté depuis le hub.
    #[serde(default)]
    monitor_changes: Vec<MonitorChange>,
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

/// Ne retient une valeur d'identité réseau du battement que si c'est vraiment
/// une adresse IP. Sinon : traitée comme **absente**.
///
/// ⚠️ Le risque n'est pas d'afficher une bêtise, c'est de faire enfler la base.
/// `record_heartbeat` ouvre un intervalle d'historique dès que `public_ip`
/// diffère de la précédente, et la purge par rétention ne supprime rien quand
/// la rétention Influx est illimitée — le réglage par DÉFAUT. Une sonde
/// authentifiée qui bat avec une valeur différente à chaque fois saturerait
/// donc le disque du hub, pour tous les sites qu'il porte. Exiger une adresse
/// ferme d'un coup la porte au nombre ET à la longueur.
///
/// ⚠️ On ne fait **pas** échouer le battement : une sonde qui bat mal doit
/// rester visible, c'est tout l'intérêt du battement. `debug` et pas `warn` —
/// une sonde antérieure ou fantaisiste ne doit pas remplir le journal.
///
/// La valeur est ramenée à sa forme canonique : `::1` et `0:0:0:0:0:0:0:1`
/// sont la même adresse, et les laisser diverger ouvrirait deux intervalles
/// pour un réseau qui n'a pas changé.
fn address_or_none(value: Option<String>, field: &str, probe_id: &str) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<std::net::IpAddr>() {
        Ok(addr) => Some(addr.to_string()),
        Err(_) => {
            // Jamais la valeur elle-même : elle peut peser un mégaoctet.
            tracing::debug!(
                "battement de {probe_id} : {field} ignoré, ce n'est pas une adresse ({} octets)",
                value.len()
            );
            None
        }
    }
}

/// Même règle pour une adresse locale, qui s'écrit en CIDR (`10.6.8.42/24`).
/// Une entrée illisible est retirée de la liste sans en invalider le reste :
/// ces valeurs finissent en base et dans les rapports remis aux clients.
fn local_address_or_none(entry: String, probe_id: &str) -> Option<String> {
    let trimmed = entry.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (addr, prefix) = match trimmed.split_once('/') {
        Some((a, bits)) => (a, Some(bits)),
        None => (trimmed, None),
    };
    let parsed = addr.parse::<std::net::IpAddr>().ok().filter(|addr| {
        prefix.is_none_or(|bits| {
            let max = if addr.is_ipv4() { 32 } else { 128 };
            bits.parse::<u8>().is_ok_and(|b| b <= max)
        })
    });
    match (parsed, prefix) {
        (Some(addr), Some(bits)) => Some(format!("{addr}/{bits}")),
        (Some(addr), None) => Some(addr.to_string()),
        (None, _) => {
            tracing::debug!(
                "battement de {probe_id} : adresse locale ignorée, forme illisible ({} octets)",
                entry.len()
            );
            None
        }
    }
}

async fn heartbeat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
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
        public_ip: address_or_none(body.public_ip, "public_ip", &id),
        interface: body.interface.filter(|v| !v.trim().is_empty()),
        local_ips: body
            .local_ips
            .into_iter()
            .filter_map(|entry| local_address_or_none(entry, &id))
            .collect(),
        gateway: address_or_none(body.gateway, "gateway", &id),
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

    // ⚠️ Les changements de surveillance sont des FAITS DATÉS, pas une liste.
    // Une cible qui n'y figure pas n'a rien subi : c'est ce qui empêche l'app
    // qui redémarre d'effacer en silence ce qui a été ajouté depuis le hub.
    //
    // ⚠️ Un battement sans ce champ n'efface donc rien et ne change rien : une
    // sonde antérieure à ce mécanisme se comporte exactement comme avant.
    let mut monitor_acks: Vec<String> = Vec::new();
    for change in &body.monitor_changes {
        let target = change.target.trim();
        if target.is_empty() {
            continue;
        }
        let removed = match change.action.trim() {
            "remove" => true,
            "add" => false,
            // Vocabulaire fermé, comme pour le verdict internet. On accuse
            // quand même : réémettre à chaque battement un changement
            // illisible ne le rendrait pas lisible, et la file de la sonde ne
            // se viderait jamais.
            other => {
                tracing::debug!("battement de {id} : action de surveillance inconnue ({other})");
                monitor_acks.push(target.to_string());
                continue;
            }
        };
        match state
            .db
            .apply_monitor_change(&id, target, removed, monitor_change_instant(change.age_secs))
        {
            // Accusé dans les deux cas : `false` veut dire « arbitré, et c'est
            // un autre changement qui a gagné » — la sonde n'a plus rien à
            // faire de celui-ci, elle s'alignera sur la liste ci-dessous.
            Ok(_) => monitor_acks.push(target.to_string()),
            // Pas d'accusé : la sonde réémettra au battement suivant. Le
            // battement, lui, aboutit — une écriture manquée ne doit pas faire
            // passer une sonde saine pour hors ligne.
            Err(e) => tracing::warn!("changement de surveillance refusé pour {id} : {e}"),
        }
    }

    // La liste effective, retirées comprises : c'est elle que la sonde suit.
    // Les retirées EN FONT PARTIE — sans elles, la sonde ne distinguerait pas
    // « le hub ne connaît pas cette cible » de « le hub l'a retirée », et la
    // ressusciterait au redémarrage.
    let monitors = state.db.monitors(&id).unwrap_or_else(|e| {
        tracing::warn!("surveillances illisibles pour {id} : {e}");
        Vec::new()
    });

    // ⚠️ Le hub ne joint jamais la sonde : c'est ce battement qui porte les
    // commandes. Les remettre échoue en silence plutôt que de faire échouer le
    // battement — une file cassée ne doit pas faire passer une sonde saine
    // pour hors ligne.
    let commands = state.db.take_pending_commands(&id).unwrap_or_else(|e| {
        tracing::warn!("file de commandes illisible pour {id} : {e}");
        Vec::new()
    });

    // ⚠️ L'adresse vue ici DÉTECTE un changement, elle n'est jamais
    // enregistrée : le battement peut sortir par la route par défaut alors que
    // les mesures sont liées à une autre interface. C'est la sonde qui mesure,
    // par la bonne interface, et qui renverra sa valeur au battement suivant.
    //
    // ⚠️ Le déclencheur est muet quand le hub est dans le LAN des sondes —
    // l'adresse source est alors privée et ne dit rien de l'adresse publique.
    // C'est le cas courant en auto-hébergement ; les déclencheurs locaux de la
    // sonde le couvrent.
    //
    // Un échec de mémorisation ne demande rien plutôt que de faire échouer le
    // battement : un relevé manqué se rattrape au TTL, un battement refusé
    // fait passer une sonde saine pour hors ligne.
    let observed = crate::client_ip::resolve(
        peer.ip(),
        headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()),
        &state.settings.trusted_proxies(),
    )
    .to_string();
    let recheck_public_ip = state
        .db
        .note_observed_source(&id, &observed)
        .unwrap_or(false);

    // ⚠️ Le mode temps réel ne voyage pas dans un champ à lui : il se traduit
    // simplement par une cadence plus courte dans le champ que la sonde lit
    // déjà. Aucune modification du contrat, et une sonde ancienne l'honore.
    //
    // La comparaison se fait ICI, à chaque battement : c'est ce qui rend
    // l'extinction automatique et increvable — pas de tâche de fond à rater.
    let heartbeat_interval_secs = match state.db.realtime_until(&id) {
        // La cadence du mode temps réel est désormais réglable ; la
        // constante ne sert plus que de défaut, dans `Settings`.
        Ok(Some(until)) if until > crate::db::now() => state.settings.realtime_heartbeat_secs(),
        _ => state.settings.heartbeat_interval_secs(),
    };

    let mut response = json!({
        "ok": true,
        "name": probe.name,
        "site_id": probe.site_id,
        "site": probe.site_name,
        "heartbeat_interval_secs": heartbeat_interval_secs,
        "commands": commands,
        "recheck_public_ip": recheck_public_ip,
        "monitors": monitors,
        // Accusé de réception, même mécanisme que les commandes (§ 14) : la
        // sonde ne retire un changement de sa file que lorsqu'il lui revient
        // accusé. Sans lui, un battement perdu perdrait le retrait avec.
        "monitor_acks": monitor_acks,
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
                        // ⚠️ DEUX champs, deux faits — ce n'est pas une
                        // redondance. `monitors` ci-dessus dit COMMENT chaque
                        // cible se porte : c'est la sonde qui mesure, elle
                        // seule connaît `alive`, la latence, la disponibilité.
                        // `probe_monitors` dit QUI est surveillé, et depuis
                        // quand : c'est le hub qui l'arbitre, avec les
                        // retraits datés que la sonde ignore encore.
                        // Fusionner les deux ferait perdre l'un ou l'autre.
                        "probe_monitors": state.db.monitors(&p.probe_id).unwrap_or_default(),
                        // ⚠️ La cadence EFFECTIVE de cette sonde, pas le réglage
                        // global. Sans elle, l'interface juge le silence à
                        // l'aune de 60 s alors qu'une sonde en temps réel bat
                        // toutes les 5 s : elle resterait verte douze fois trop
                        // longtemps, et afficherait « en ligne » une sonde
                        // muette depuis cinquante secondes.
                        "heartbeat_interval_secs": if p.realtime_until.unwrap_or(0) > crate::db::now() {
                            state.settings.realtime_heartbeat_secs()
                        } else {
                            state.settings.heartbeat_interval_secs()
                        },
                        // Échéance du mode temps réel, pour que l'interface
                        // affiche le compte à rebours sans un appel de plus
                        // par sonde — et que ce qui tourne se voie.
                        "realtime_until": p.realtime_until,
                    })
                })
                .collect(),
        )),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct RealtimeBody {
    on: bool,
}

/// Arme le mode temps réel sur une sonde, pour la durée réglée.
///
/// ⚠️ La minuterie n'est pas une précaution optionnelle : on redescend de
/// l'échelle, la caméra marche, on passe à la suivante — on ne revient pas
/// éteindre le mode. Sans elle, au bout de quelques mois tout le parc bat
/// toutes les 5 secondes et personne ne sait pourquoi. Ce n'est pas une panne,
/// c'est pire : ça marche, et la charge a été multipliée sans raison.
async fn set_realtime(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RealtimeBody>,
) -> Response {
    let until = body
        .on
        .then(|| crate::db::now() + state.settings.realtime_duration_min() * 60);
    match state.db.set_realtime(&id, until) {
        Ok(()) => ok_json(json!({ "ok": true, "realtime_until": until })),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct MonitorTarget {
    target: String,
}

/// Réactive une surveillance retirée. **Réactiver, c'est effacer la
/// suppression** — il n'y a pas d'autre mécanisme, et c'est ce qui fait que le
/// « revive des deux côtés » tombe seul : la sonde reprend la cible au
/// battement suivant parce qu'elle ne porte plus de date de retrait.
///
/// ⚠️ Derrière le garde de portée, comme toute route qui désigne UNE sonde :
/// 404 hors portée, et rien d'écrit. Réactiver une surveillance relance du
/// trafic sur le réseau d'un client — ce n'est pas une consultation.
/// Retire une cible **depuis le hub**, sans passer par la sonde.
///
/// ⚠️ Il existait déjà un chemin de retrait, mais il passait par une commande
/// envoyée à la sonde : retirer une cible pendant que la sonde est **hors
/// ligne** n'écrivait donc aucune suppression, et le sous-onglet « Retirées »
/// restait vide jusqu'à son retour. La décision de l'utilisateur disparaissait
/// dans l'intervalle — exactement le défaut que ce chantier corrige.
///
/// Ici la suppression est écrite **tout de suite**, datée de l'horloge du hub.
/// La sonde s'y alignera à son prochain battement, qu'elle soit là ou non.
async fn remove_monitor_entry(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<MonitorTarget>,
) -> Response {
    // ⚠️ **404, pas 500.** Sans cette garde, la clé étrangère
    // `probe_monitors.probe_id` refusait l'écriture, l'erreur rusqlite
    // devenait `DbError::Internal`, et le client recevait « FOREIGN KEY
    // constraint failed » sous un 500 — c'est-à-dire « le hub est en panne »
    // là où la vérité est « cette sonde n'existe pas ». Même formulation que
    // `probe_metrics`, et surtout la MÊME que le refus de portée : une sonde
    // hors portée doit rester indiscernable d'une sonde inexistante.
    if state.db.get_probe(&id).is_err() {
        return fail(StatusCode::NOT_FOUND, "sonde inconnue");
    }
    let target = body.target.trim();
    if target.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "cible requise");
    }
    match audited(
        &state,
        Some(&actor.username),
        "probe.monitor_remove",
        Some(&id),
        state
            .db
            .apply_monitor_change(&id, target, true, crate::db::now()),
    ) {
        Ok(applied) => ok_json(json!({ "ok": true, "applied": applied })),
        Err(e) => error_response(e),
    }
}

async fn revive_monitor(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<MonitorTarget>,
) -> Response {
    // ⚠️ **404, pas 500.** Sans cette garde, la clé étrangère
    // `probe_monitors.probe_id` refusait l'écriture, l'erreur rusqlite
    // devenait `DbError::Internal`, et le client recevait « FOREIGN KEY
    // constraint failed » sous un 500 — c'est-à-dire « le hub est en panne »
    // là où la vérité est « cette sonde n'existe pas ». Même formulation que
    // `probe_metrics`, et surtout la MÊME que le refus de portée : une sonde
    // hors portée doit rester indiscernable d'une sonde inexistante.
    if state.db.get_probe(&id).is_err() {
        return fail(StatusCode::NOT_FOUND, "sonde inconnue");
    }
    let target = body.target.trim();
    if target.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "cible requise");
    }
    // `now()` du hub : la réactivation vient du hub, elle est donc déjà dans
    // sa référence. Rien à convertir, et surtout aucune horloge à comparer.
    match audited(
        &state,
        Some(&actor.username),
        "probe.monitor_revive",
        Some(&id),
        state
            .db
            .apply_monitor_change(&id, target, false, crate::db::now()),
    ) {
        Ok(applied) => ok_json(json!({ "ok": true, "applied": applied })),
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
    // La sonde d'origine est validée par l'intergiciel de portée — qui vérifie
    // son EXISTENCE et sa portée, pour tous les comptes. ⚠️ Ce commentaire
    // affirmait cette garantie à une époque où l'intergiciel court-circuitait
    // pour un compte `Scope::All` : la sonde n'était alors validée que par le
    // `get_probe` de `db.update_probe`, plus bas. Un commentaire faux sur une
    // garde est ce qui fait sauter la vérification au refactor suivant.
    //
    // Ce qui n'est pas couvert par l'intergiciel, c'est la DESTINATION : sans
    // le contrôle ci-dessous, un opérateur restreint déplacerait une sonde chez
    // un autre client — et la perdrait de vue au passage, puisqu'elle sortirait
    // de sa propre portée.
    if let Some(target) = body.site_id.as_deref() {
        if let Err(refusal) = site_in_scope(&state, &actor, target) {
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
///
/// 🔴 **Le corps est construit par `sla_payload::build`, pas ici.** Le hub doit
/// produire le même rapport hors de toute requête HTTP pour fabriquer le
/// classeur (contrat § 23) : deux constructions auraient donné deux corps qui
/// divergent, et c'est celui du document remis au client qu'on ne saurait plus
/// expliquer. Cette route ne fait plus que lire la fenêtre et traduire l'erreur
/// en code.
async fn probe_sla_samples(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> Response {
    let window = match flux_window(&query) {
        Ok(w) => w,
        Err(refus) => return refus,
    };
    match crate::sla_payload::build(&state, &id, &window, query.start, query.stop).await {
        Ok(payload) => ok_json(payload),
        // ⚠️ Deux codes et non un : une sonde inconnue est un 404, un Influx
        // injoignable un 502. Les confondre ferait réessayer là où il n'y a
        // rien à retrouver.
        Err(crate::sla_payload::SlaPayloadError::Db(e)) => error_response(e),
        Err(crate::sla_payload::SlaPayloadError::Influx(e)) => {
            fail(StatusCode::BAD_GATEWAY, &e)
        }
    }
}

/// Historique des adresses publiques d'une sonde, sur la même fenêtre que le
/// rapport SLA. Le tableau sous le graphe n'a pas besoin du rapport complet.
async fn probe_public_ips(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> Response {
    // ⚠️ 404 et non un tableau vide. `{"intervals": []}` AFFIRME que la sonde
    // existe et n'a jamais changé d'adresse : sur un identifiant mal saisi,
    // c'est une réponse plausible et fausse, et elle se lit exactement à
    // l'envers de la vérité. La garde de portée ne couvre pas ce cas — elle
    // court-circuite pour un compte `Scope::All`, donc un admin arrivait ici.
    //
    // ⚠️ Même code et même message que la garde de portée : une sonde qui
    // existe hors portée doit rester indiscernable d'une sonde inexistante.
    if state.db.get_probe(&id).is_err() {
        return fail(StatusCode::NOT_FOUND, "sonde inconnue");
    }
    // ⚠️ La fenêtre est VALIDÉE ici comme sur `/metrics`, `/sla`, `/sla.csv` et
    // `/uptime`. Cette route ne lisait que les bornes en secondes, et
    // `range=nawak` y retombait EN SILENCE sur 24 h : elle rendait un tableau
    // bien formé, mais celui d'une période que personne n'avait demandée, sous
    // une légende qui en annonçait une autre. C'est le repli muet corrigé
    // partout ailleurs en cfff27d, resté sur cette route.
    let window = match flux_window(&query) {
        Ok(w) => w,
        Err(refus) => return refus,
    };
    match state.db.public_ip_history(&id, window.from, window.to) {
        Ok(intervals) => ok_json(json!({ "intervals": intervals })),
        Err(e) => error_response(e),
    }
}

/// Longueur maximale d'un libellé de réseau.
///
/// Rien ne la bornait. Un libellé part dans tous les rapports remis aux
/// clients et dans la colonne du tableau sous le graphe : un roman y serait
/// illisible, et il n'y a aucune raison de laisser une saisie non bornée
/// entrer en base.
const MAX_NETWORK_LABEL_LEN: usize = 128;

#[derive(Deserialize)]
struct LabelBody {
    /// Le site auquel appartient le réseau nommé. Obligatoire : c'est lui qui
    /// rend le libellé vérifiable par la portée de l'appelant.
    site_id: String,
    public_ip: String,
    /// Facultative : une sonde antérieure aux relevés de passerelle n'en a
    /// pas, et la clé la traite alors comme une chaîne vide.
    #[serde(default)]
    gateway: String,
    label: String,
}

/// Nomme un réseau (« Maison », « Bureau »).
///
/// La clé est `(site_id, public_ip, gateway)`. Ni l'adresse seule ni le couple
/// `(adresse, passerelle)` ne suffisent : deux réseaux derrière un même
/// opérateur partagent l'adresse publique, et sous CGNAT deux CLIENTS la
/// partagent aussi — avec, une fois sur deux, la même `192.168.1.1` de box.
async fn set_network_label(
    State(state): State<AppState>,
    Extension(actor): Extension<Identity>,
    Json(body): Json<LabelBody>,
) -> Response {
    // ⚠️ Le rôle ne suffisait pas : un opérateur restreint à un site posait
    // le libellé d'un autre client, et ce texte remonte dans le rapport SLA
    // qu'on lui remet. Refus en 404 comme partout ailleurs — nommer un site
    // qu'on n'a pas le droit de voir ne doit pas apprendre qu'il existe.
    if let Err(refusal) = site_in_scope(&state, &actor, body.site_id.trim()) {
        return refusal;
    }
    // Un site inexistant se solderait sinon par une violation de clé
    // étrangère, donc un 500 : c'est un 404 que l'appelant doit lire.
    if let Err(e) = state.db.get_site(body.site_id.trim()) {
        return error_response(e);
    }
    if body.label.trim().chars().count() > MAX_NETWORK_LABEL_LEN {
        return fail(
            StatusCode::BAD_REQUEST,
            &format!("libellé limité à {MAX_NETWORK_LABEL_LEN} caractères"),
        );
    }
    match state.db.set_network_label(
        body.site_id.trim(),
        body.public_ip.trim(),
        body.gateway.trim(),
        &body.label,
    ) {
        Ok(()) => ok_json(json!({ "ok": true })),
        Err(e) => error_response(e),
    }
}

/// Relevés d'accès internet, ramenés à la même forme que les cibles ICMP.
///
/// ⚠️ `state` est un texte (`online` / `limited` / `offline`), pas un booléen.
/// Le convertir en « vivant / mort » perdrait `limited`, qui est justement
/// l'état qu'un client conteste — « ça marchait » alors que rien ne passait.
pub(crate) fn internet_samples_from_flux_csv(csv: &str) -> Vec<serde_json::Value> {
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
pub(crate) fn samples_from_flux_csv(csv: &str, window: (i64, i64)) -> Vec<serde_json::Value> {
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
            let coverage = lanprobe_core::sla::compute_coverage(
                &points.keys().copied().collect::<Vec<i64>>(),
                window.0,
                window.1,
            );
            let samples: Vec<serde_json::Value> = points
                .into_iter()
                .map(|(ts, (alive, latency_ms))| {
                    json!({
                        "timestamp": ts,
                        // ⚠️ `null` quand la sonde n'a pas écrit `alive` :
                        // INDÉTERMINÉ, ni disponible ni en panne. On le
                        // déduisait de la présence d'une latence, ce qui
                        // faisait d'un silence un fait.
                        "alive": alive,
                        "latency_ms": latency_ms,
                    })
                })
                .collect();
            // ⚠️ Calculée ICI et transportée, pas recalculée dans le
            // navigateur : le classeur remis au client et l'export CSV du hub
            // doivent annoncer la MÊME complétude. Deux calculs finiraient par
            // diverger d'un point de pourcentage, et c'est celui du document
            // qu'on ne pourrait plus expliquer.
            json!({ "ip": ip, "samples": samples, "coverage": coverage })
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
    // ⚠️ `flux_window`, comme `/metrics`, `/sla` et `/public-ips`. Cette route
    // lisait `query.range` SEUL : demander « du 1er au 31 » rendait les
    // dernières 24 h, dans un classeur dont la colonne `fenetre` annonçait
    // pourtant la période demandée. Un rapport remis à un client avec le bon
    // en-tête et les mauvaises données est le pire des deux mondes — c'est le
    // défaut corrigé en b05b049, resté sur l'export.
    let window = match flux_window(&query) {
        Ok(w) => w,
        Err(refus) => return refus,
    };
    let bucket = state.settings.influx_bucket();
    let flux = format!(
        "from(bucket: {})\n  |> range({})\n           |> filter(fn: (r) => r[\"probe_id\"] == {})\n           |> filter(fn: (r) => r[\"_measurement\"] == \"ping_latency\")",
        flux_string(&bucket),
        window.clause,
        flux_string(&id),
    );
    let csv = match state.influx.query_flux(&flux).await {
        Ok(csv) => csv,
        Err(e) => return fail(StatusCode::BAD_GATEWAY, &e),
    };

    let report = sla_from_flux_csv(&csv, (window.from, window.to));
    let mut out = String::from(
        "sonde,site,fenetre,cible,disponibilite_pct,latence_moy_ms,\
         latence_min_ms,latence_max_ms,latence_p95_ms,releves,echecs,indetermines,\
         couverture\n",
    );
    for r in &report {
        let row = &r.stats;
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            csv_cell(&probe.name),
            csv_cell(&probe.site_name),
            csv_cell(&window.label),
            csv_cell(&row.ip),
            // ⚠️ Ni vide ni `0.00`. Une cellule vide se lit comme un oubli de
            // l'outil, `0.00` comme une panne totale. Le mot se lit comme ce
            // qu'il est. ⚠️ Et il n'est PAS numérique : un tableur ne
            // l'agrégera pas comme un zéro dans une moyenne de colonne.
            row.uptime_pct
                .map(|p| format!("{p:.2}"))
                .unwrap_or_else(|| "indetermine".into()),
            row.avg_latency_ms.map(|v| format!("{v:.1}")).unwrap_or_default(),
            row.min_latency_ms.map(|v| v.to_string()).unwrap_or_default(),
            row.max_latency_ms.map(|v| v.to_string()).unwrap_or_default(),
            row.p95_latency_ms.map(|v| v.to_string()).unwrap_or_default(),
            row.total_samples,
            row.failed_samples,
            row.undetermined_samples,
            // ⚠️ Même règle que `indetermine` : jamais un nombre. « complete »
            // et « 87.4 % mesures » ne s'agrègent pas dans une moyenne de
            // colonne — c'est par là que le faux chiffre reviendrait.
            csv_cell(&coverage_cell(&r.coverage)),
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
                    window.label.trim_start_matches('-')
                ),
            ),
        ],
        out,
    )
        .into_response()
}

/// Cellule de couverture, en toutes lettres.
///
/// ⚠️ **Jamais un nombre.** Un tableur agrégerait « 87,4 » dans une moyenne de
/// colonne et referait naître un chiffre que personne n'a calculé — la même
/// porte de derrière que la cellule « indetermine ». Et rien à dire quand
/// c'est complet : la mention n'existe que lorsqu'elle apprend quelque chose.
fn coverage_cell(c: &lanprobe_core::sla::Coverage) -> String {
    match (c.is_complete(), c.pct()) {
        (true, _) => "complete".into(),
        (false, Some(p)) => format!("{p:.1} % mesures"),
        (false, None) => "indetermine".into(),
    }
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
fn sla_from_flux_csv(csv: &str, window: (i64, i64)) -> Vec<SlaRow> {
    use std::collections::BTreeMap;

    // Par cible, puis par instant : `alive` et `latency_ms` sont deux lignes
    // distinctes du CSV et doivent être recollées avant d'être comptées.
    // ⚠️ Clé en SECONDES et non en chaîne RFC3339, comme `samples_from_flux_csv`
    // juste au-dessus : c'est la même donnée découpée pareil, et c'est ce qui
    // donne les horodatages numériques dont la couverture a besoin.
    let mut by_ip: BTreeMap<String, BTreeMap<i64, (Option<bool>, Option<u64>)>> = BTreeMap::new();
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
        let slot = by_ip.entry(ip.to_string()).or_default().entry(ts / 1000).or_default();
        match field {
            "alive" => slot.0 = Some(value == "true" || value == "1"),
            "latency_ms" => slot.1 = value.parse::<f64>().ok().map(|v| v.round() as u64),
            _ => {}
        }
    }

    by_ip
        .into_iter()
        .map(|(ip, points)| {
            let times: Vec<i64> = points.keys().copied().collect();
            let samples: Vec<lanprobe_core::sla::PingSample> = points
                .into_values()
                .map(|(alive, latency_ms)| lanprobe_core::sla::PingSample {
                    // ⚠️ Voir `samples_from_flux_csv` : pas de repli. Le
                    // commentaire en tête de cette fonction affirmait déjà que
                    // la disponibilité se calcule « sur `alive`, jamais sur la
                    // présence d'une latence » — le code disait le contraire.
                    alive,
                    latency_ms,
                })
                .collect();
            SlaRow {
                stats: lanprobe_core::sla::compute_sla(&ip, &samples),
                // ⚠️ Une SEULE implémentation de la couverture, dans
                // `lanprobe-core` : l'export CSV, le rapport `/sla` et le
                // classeur lisent tous ce calcul-ci. Deux calculs qui
                // divergeraient seraient pires qu'un seul imparfait.
                coverage: lanprobe_core::sla::compute_coverage(&times, window.0, window.1),
            }
        })
        .collect()
}

/// Le SLA d'une cible, et ce qui a réellement été mesuré pour l'obtenir.
struct SlaRow {
    stats: lanprobe_core::sla::SlaStats,
    coverage: lanprobe_core::sla::Coverage,
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

    // 🔴 Le hub note SA PROPRE décision, au moment où il l'émet.
    //
    // Lu dans la base de production le 02/09 : commande `add_monitor` à
    // 02:09:16, cible réellement pinguée à 02:10:34, ligne dans
    // `probe_monitors` seulement à 02:12:31 — le hub attendait que la sonde
    // RÉANNONCE une cible qu'il venait lui-même de demander. Trois minutes
    // pendant lesquelles une cible mesurée était inconnue de la liste
    // partagée, et l'écran la marquait « plus annoncée ».
    //
    // ⚠️ Les DEUX gestes, pas seulement l'ajout. Le bouton « Retirer » de
    // l'écran passe par `POST /monitors/remove` depuis 8a946d5, mais le type
    // de commande `remove_monitor` reste servi : un appel scripté rouvrait la
    // même fenêtre, en miroir. Le défaut n'est pas dans l'appelant, il est
    // ici — un ordre qui part sans que le hub note sa décision.
    //
    // ⚠️ APRÈS l'empilement, jamais avant. Si l'écriture échoue, on retombe
    // sur le comportement d'avant — le battement suivant portera le
    // changement. Dans l'autre ordre, un empilement raté laisserait une
    // décision que rien ne viendrait jamais confirmer : une cible
    // « surveillée » que personne ne pingue, ou une cible « retirée » qui
    // continue de l'être.
    //
    // ⚠️ L'arbitrage reste délégué à `apply_monitor_change`, et rien n'est
    // écrit en propre : un ajout ne ressuscite pas une cible retirée plus
    // récemment, un retrait ne piétine pas un ajout plus récent (§20).
    //
    // ⚠️ Pas de second `audit` : `probe.command` ci-dessus nomme déjà le geste
    // et son auteur. Deux entrées pour un seul clic feraient lire deux gestes.
    if let Some(removed) = match kind {
        "add_monitor" => Some(false),
        "remove_monitor" => Some(true),
        _ => None,
    } {
        if let Some(target) = args
            .get("ip")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            if let Err(e) = state
                .db
                .apply_monitor_change(&id, target, removed, crate::db::now())
            {
                tracing::warn!(
                    "décision de {kind} sur « {target} » non notée ({e}) — le battement de la sonde la portera"
                );
            }
        }
    }

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
    /// Nombre de points que le demandeur peut afficher — en pratique la
    /// largeur du graphe, en pixels. Facultatif : voir `clamp_max_points`.
    #[serde(default)]
    max_points: Option<i64>,
}

/// Bornes explicites, si elles tiennent debout. Des bornes inversées rendraient
/// une fenêtre vide, donc un rapport à 0 % qu'on prendrait pour une panne
/// totale : on retombe alors sur la fenêtre glissante.
fn explicit_bounds(query: &MetricsQuery) -> Option<(i64, i64)> {
    match (query.start, query.stop) {
        (Some(start), Some(stop)) if stop > start => Some((start, stop)),
        _ => None,
    }
}

/// Une fenêtre glissante (`-24h`, `7d`, `30m`…), lue **une seule fois**.
///
/// 🔴 **Deux fonctions lisaient cette chaîne, et pas de la même façon** :
/// l'une refusait ce qu'elle ne comprenait pas, l'autre retombait en silence
/// sur 24 h. C'est cette divergence qui a produit le `24h` visant le futur,
/// puis le repli muet resté sur `/public-ips` : corriger route par route
/// laissait la porte ouverte. Une seule lecture, une seule décision — le refus
/// et la durée ne peuvent plus se contredire.
///
/// ⚠️ **Une saisie illisible ne donne aucune fenêtre**, pas une fenêtre par
/// défaut. C'est aussi ce qui garde l'alphabet clos : seuls un entier et une
/// unité connue sortent d'ici, jamais un fragment de Flux.
#[derive(Clone, Copy)]
struct SlidingWindow {
    count: i64,
    /// L'unité telle qu'elle se réécrit — `s`, `m`, `h`, `d`, `w`, et rien
    /// d'autre.
    unit: &'static str,
    unit_secs: i64,
}

impl SlidingWindow {
    /// `None` quand la saisie ne décrit aucune fenêtre.
    fn parse(range: &str) -> Option<Self> {
        let raw = range.trim().trim_start_matches('-');
        let split = raw.find(|c: char| !c.is_ascii_digit())?;
        let (count, unit) = raw.split_at(split);
        let (unit, unit_secs) = match unit {
            "s" => ("s", 1),
            "m" => ("m", 60),
            "h" => ("h", 3600),
            "d" => ("d", 86_400),
            "w" => ("w", 7 * 86_400),
            _ => return None,
        };
        // ⚠️ Un compte vide ou qui déborde échoue ICI, donc pour les deux
        // formes à la fois. Il passait la lecture Flux et retombait sur 24 h
        // dans la lecture en secondes : la requête partait sur une fenêtre que
        // le tableau par adresse ne couvrait pas.
        let count = count.parse::<i64>().ok()?;
        Some(Self { count, unit, unit_secs })
    }

    /// La durée telle qu'elle s'écrit en Flux.
    ///
    /// 🔴 **Le signe est normalisé.** En Flux une durée POSITIVE part de
    /// maintenant vers l'AVANT : `range(start: 24h)` visait les 24 h à VENIR,
    /// Influx refusait en 400, et le hub rendait 502 — « InfluxDB est en
    /// panne » pour une saisie parfaitement anodine.
    fn flux(&self) -> String {
        format!("-{}{}", self.count, self.unit)
    }

    /// La même durée en secondes — l'unité de l'historique des adresses en
    /// SQLite et du calcul du pas d'agrégation.
    fn secs(&self) -> i64 {
        self.count.saturating_mul(self.unit_secs)
    }
}

/// La fenêtre demandée, sous les trois formes dont les routes ont besoin.
///
/// ⚠️ Un seul objet, construit une fois par requête : la clause Flux, le
/// libellé rendu à l'appelant et les bornes en secondes décrivent forcément la
/// même période. Deux calculs séparés finiraient par couvrir deux fenêtres
/// légèrement différentes, et le tableau par adresse contredirait la courbe
/// qu'il légende.
pub(crate) struct Window {
    /// Le contenu de la clause `range(...)`.
    pub(crate) clause: String,
    /// Ce que la réponse annonce : `-24h`, ou `<start>..<stop>` quand les
    /// bornes sont explicites.
    pub(crate) label: String,
    pub(crate) from: i64,
    pub(crate) to: i64,
}

/// Lit la fenêtre d'une requête.
///
/// `Err` = la fenêtre demandée est illisible : c'est un **400**, pas un 502.
/// La saisie du client est en cause, pas l'amont.
fn flux_window(query: &MetricsQuery) -> Result<Window, Response> {
    if let Some((from, to)) = explicit_bounds(query) {
        return Ok(Window {
            clause: format!("start: {from}, stop: {to}"),
            label: format!("{from}..{to}"),
            from,
            to,
        });
    }
    let asked = query.range.as_deref().unwrap_or("-24h");
    let Some(sliding) = SlidingWindow::parse(asked) else {
        return Err(fail(StatusCode::BAD_REQUEST, "fenêtre illisible"));
    };
    let label = sliding.flux();
    let to = crate::db::now();
    Ok(Window {
        clause: format!("start: {label}"),
        from: to - sliding.secs(),
        to,
        label,
    })
}

/// La même lecture de fenêtre, pour un appelant qui n'a **pas** de requête
/// HTTP — la demande de rapport, dont les bornes arrivent dans un corps JSON
/// (contrat § 23).
///
/// ⚠️ Elle passe par [`flux_window`] et ne recalcule rien : un rapport dont la
/// fenêtre serait construite à part finirait par couvrir une période
/// légèrement différente de celle que l'écran annonce, et c'est le document
/// remis au client qui porterait l'écart.
pub(crate) fn window_of(
    range: Option<String>,
    start: Option<i64>,
    stop: Option<i64>,
) -> Result<Window, Response> {
    flux_window(&MetricsQuery {
        measurement: None,
        range,
        start,
        stop,
        max_points: None,
    })
}

/// Instant avant lequel les intervalles d'adresse ne servent plus à rien : les
/// mesures qu'ils découpaient sont sorties de la rétention Influx.
///
/// ⚠️ `0` (et toute valeur négative) vaut **rétention illimitée**, et c'est le
/// défaut. Calculer `now - 0 × 86400` donnerait `now` et effacerait tout
/// l'historique dès le premier passage de la purge, sur l'installation la plus
/// courante qui soit. On ne purge donc rien du tout dans ce cas.
pub fn public_ip_history_cutoff(retention_days: i64, now: i64) -> Option<i64> {
    (retention_days > 0).then(|| now - retention_days * 86_400)
}

// ── Agrégation des mesures ─────────────────────────────────────────────────
//
// ⚠️ **L'agrégation ne dépend ni de l'ancienneté de la plage, ni de sa durée —
// seulement du nombre de points rapporté à la largeur du graphe.** Dix minutes
// prises il y a trois mois se rendent brutes : les points existent, ils
// tiennent dans la carte, il n'y a aucune raison de les moyenner. Quatre-vingts
// jours à un ping par seconde font sept millions et demi de points par cible,
// que le navigateur n'affichera jamais.

/// Points affichables quand le demandeur ne dit rien. C'est une largeur de
/// graphe usuelle, en pixels : au-delà, on moyenne des mesures dans un pixel
/// qu'on ne saurait de toute façon pas dessiner.
const DEFAULT_MAX_POINTS: i64 = 800;
/// Bornes du plafond. La valeur vient du NAVIGATEUR : `0` diviserait par zéro
/// et `10_000_000` rendrait la protection inopérante.
const MIN_MAX_POINTS: i64 = 100;
const MAX_MAX_POINTS: i64 = 5_000;

pub fn clamp_max_points(asked: Option<i64>) -> i64 {
    asked.unwrap_or(DEFAULT_MAX_POINTS).clamp(MIN_MAX_POINTS, MAX_MAX_POINTS)
}

/// Échelle des pas d'agrégation, **lisibles par un humain**.
///
/// ⚠️ Un axe qui annonce « moyenné par 30 min » se lit ; « moyenné par 1 847 s »
/// ne se lit pas. On arrondit donc au pas supérieur de cette échelle plutôt que
/// de rendre le quotient exact — arrondir vers le haut garantit en prime de ne
/// jamais dépasser le nombre de points demandé.
///
/// Le premier échelon est la seconde : c'est la cadence du ping, un pas plus
/// fin ne ferait que fabriquer des intervalles vides.
const AGGREGATE_STEPS: &[(i64, &str)] = &[
    (1, "1s"),
    (2, "2s"),
    (5, "5s"),
    (10, "10s"),
    (15, "15s"),
    (30, "30s"),
    (60, "1m"),
    (120, "2m"),
    (300, "5m"),
    (600, "10m"),
    (900, "15m"),
    (1_800, "30m"),
    (3_600, "1h"),
    (7_200, "2h"),
    (10_800, "3h"),
    (21_600, "6h"),
    (43_200, "12h"),
    (86_400, "24h"),
    (604_800, "7d"),
];

/// Pas d'agrégation d'une fenêtre, ou `None` quand elle se rend brute.
///
/// `None` dès que la plage ne PEUT PAS dépasser `max_points` : à une mesure par
/// seconde, une fenêtre de `n` secondes porte au plus `n` points. C'est un
/// majorant, pas un compte — le hub ne sait pas combien de points existent
/// vraiment sans les lire, et sur-agréger un peu vaut mieux que de découvrir le
/// débordement une fois la réponse partie.
pub fn aggregate_every(span_secs: i64, max_points: i64) -> Option<&'static str> {
    if max_points <= 0 || span_secs <= max_points {
        return None;
    }
    // `div_ceil` reste instable sur les entiers signés : arrondi à la main.
    let needed = (span_secs + max_points - 1) / max_points;
    let step = AGGREGATE_STEPS
        .iter()
        .find(|(secs, _)| *secs >= needed)
        // Une fenêtre plus large que le dernier échelon retombe sur lui, jamais
        // sur `None` : rendre les points bruts serait précisément l'inverse de
        // ce que le plafond protège.
        .unwrap_or_else(|| AGGREGATE_STEPS.last().expect("échelle non vide"));
    Some(step.1)
}

/// Requête du taux de disponibilité par cible, **agrégée côté Influx**.
///
/// ⚠️ `/sla` rend des relevés BRUTS et la fiche se rafraîchit toutes les
/// 3 secondes : le rejouer pour un pourcentage ferait relire des millions de
/// points par tour d'horloge. Ici Influx ne renvoie qu'UNE ligne par cible.
///
/// 🔴 **Un relevé indéterminé sort du dénominateur par CONSTRUCTION**, pas par
/// précaution : le filtre `_field == "alive"` ne sélectionne que les points
/// qui portent un verdict. Une mesure antérieure au champ `alive` n'en a pas,
/// elle n'existe donc pas pour `count()`. Vérifié sur un influxd 2.9.1 réel —
/// 90 relevés sans verdict à côté de 10 avec : `count()` rend 10.
///
/// ⚠️ `mean()` d'un booléen converti en 0/1 EST le taux, et c'est
/// arithmétiquement la même chose que `compute_sla` — un test tient cette
/// égalité, pour qu'une divergence future casse la suite au lieu de s'installer.
///
/// 🔴 `first()` et `last()` sont là pour que l'écran puisse dire, à côté du
/// taux, quelle PART de la fenêtre n'a aucun relevé. Un taux sans sa couverture
/// se lit « tout va bien depuis six heures » quand il dit « tout allait bien
/// pendant les vingt-huit minutes mesurées ». Ce sont des SÉLECTEURS : une
/// ligne par cible, comme `mean` et `count` — le volume de cette route ne
/// change pas, et c'est la raison pour laquelle elle n'embarque pas les
/// horodatages bruts que `/sla` seul peut se permettre de lire.
///
/// 🔴 `elapsed()` puis `max()` rendent le PLUS GRAND trou intérieur, et c'est
/// lui qui manquait : une sonde dont les relevés touchent les deux bords mais
/// dort au milieu ne déclenchait rien, alors que 97 % de sa période n'a aucun
/// relevé. `max` est un agrégat — une ligne par cible, comme les quatre autres.
///
/// 🔴 **`sort` n'est pas décoratif.** Vérifié sur un influxd 2.9.1 réel, sur une
/// cible portant DEUX séries (un `host` qui change) : sans lui, `group` les
/// concatène au lieu de les fusionner dans l'ordre du temps, et les trois
/// rendus mentent ensemble — `first` rendait le premier point de la seconde
/// série, `last` le dernier de la première, et `elapsed` un écart de 2 s là où
/// les relevés sont à la seconde. Un `first` trop tardif gonfle le trou de bord
/// annoncé : c'est exactement le sens qui casse le minorant.
///
/// ⚠️ Le chiffre reste un MINORANT de la couverture du rapport : les bords et
/// le plus grand trou, jamais la somme de tous les trous que `compute_coverage`
/// additionne. La démonstration vit avec le calcul, dans
/// `web-ui/src/lib/uptime.ts`.
fn uptime_flux(bucket: &str, probe_id: &str, window: &str) -> String {
    format!(
        "base = from(bucket: {})\n  |> range({})\n  \
         |> filter(fn: (r) => r[\"probe_id\"] == {})\n  \
         |> filter(fn: (r) => r[\"_measurement\"] == \"ping_latency\")\n  \
         |> filter(fn: (r) => r[\"_field\"] == \"alive\")\n  \
         |> toFloat()\n  \
         |> group(columns: [\"ip\"])\n  \
         |> sort(columns: [\"_time\"])\n\n\
         base |> mean() |> yield(name: \"uptime\")\n\
         base |> count() |> yield(name: \"samples\")\n\
         base |> first() |> yield(name: \"first\")\n\
         base |> last() |> yield(name: \"last\")\n\
         base |> elapsed(unit: 1s) |> max(column: \"elapsed\") |> yield(name: \"gap\")\n",
        flux_string(bucket),
        window,
        flux_string(probe_id),
    )
}

/// Les cinq rendus (`uptime`, `samples`, `first`, `last`, `gap`) recollés par cible.
///
/// ⚠️ C'est la colonne `result` qui distingue les deux blocs — la seule route
/// du hub où elle porte de l'information. `series_from_flux_csv` l'ignore
/// justement parce qu'elle n'en porte pas ailleurs.
fn uptime_from_flux_csv(csv: &str) -> Vec<serde_json::Value> {
    use std::collections::BTreeMap;

    /// Ce que les quatre rendus disent d'une même cible.
    #[derive(Default)]
    struct Cible {
        /// Taux 0..1.
        ratio: Option<f64>,
        /// Nombre de relevés déterminés.
        samples: i64,
        /// Bornes en SECONDES epoch — `None` tant que le rendu n'est pas lu.
        first_at: Option<i64>,
        last_at: Option<i64>,
        /// Plus grand écart entre deux relevés consécutifs, en secondes.
        ///
        /// ⚠️ `None` sous DEUX relevés : `elapsed` ne rend alors aucune ligne.
        /// Constaté sur un influxd réel, pas supposé.
        max_gap_secs: Option<i64>,
    }

    let mut by_ip: BTreeMap<String, Cible> = BTreeMap::new();
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
        let (Some(result), Some(ip), Some(value)) = (at("result"), at("ip"), at("_value")) else {
            continue;
        };
        let slot = by_ip.entry(ip.to_string()).or_default();
        match result {
            "uptime" => slot.ratio = value.parse::<f64>().ok(),
            "samples" => slot.samples = value.parse::<i64>().unwrap_or(0),
            // ⚠️ Secondes et non millisecondes : c'est l'unité de `Coverage`,
            // et le calcul de bord qui les consomme est le même des deux côtés.
            "first" => slot.first_at = at("_time").and_then(rfc3339_to_millis).map(|ms| ms / 1_000),
            "last" => slot.last_at = at("_time").and_then(rfc3339_to_millis).map(|ms| ms / 1_000),
            // ⚠️ La valeur est dans la colonne `elapsed`, PAS dans `_value` :
            // `elapsed` ajoute sa colonne et laisse `_value` intact — celui du
            // relevé, donc le booléen `alive`. Le lire ici rendrait 0 ou 1.
            "gap" => slot.max_gap_secs = at("elapsed").and_then(|v| v.parse::<i64>().ok()),
            _ => {}
        }
    }

    by_ip
        .into_iter()
        .map(|(ip, c)| {
            // ⚠️ `null` et jamais `0` quand aucun relevé déterminé n'existe :
            // « 0 % » se lirait comme une panne totale. Même règle que partout
            // depuis ce matin.
            let uptime = (c.samples > 0)
                .then_some(c.ratio)
                .flatten()
                .map(|r| serde_json::Value::from(r * 100.0))
                .unwrap_or(serde_json::Value::Null);
            // ⚠️ `null` aussi pour les bornes, et pour la même raison : une
            // borne absente ne veut pas dire « fenêtre couverte ».
            let borne = |v: Option<i64>| v.map(serde_json::Value::from).unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "ip": ip,
                "uptime_pct": uptime,
                "samples": c.samples,
                "first_at": borne(c.first_at),
                "last_at": borne(c.last_at),
                "max_gap_secs": borne(c.max_gap_secs),
            })
        })
        .collect()
}

/// Taux de disponibilité par cible sur la fenêtre — indicateur vivant.
async fn probe_uptime(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MetricsQuery>,
) -> Response {
    if state.db.get_probe(&id).is_err() {
        return fail(StatusCode::NOT_FOUND, "sonde inconnue");
    }
    let window = match flux_window(&query) {
        Ok(w) => w,
        Err(refus) => return refus,
    };
    let flux = uptime_flux(&state.settings.influx_bucket(), &id, &window.clause);
    match state.influx.query_flux(&flux).await {
        Ok(csv) => ok_json(serde_json::json!({
            "range": window.label,
            "targets": uptime_from_flux_csv(&csv),
        })),
        Err(e) => fail(StatusCode::BAD_GATEWAY, &e),
    }
}

/// Requête Flux d'une lecture de mesures. `every` à `None` = points bruts.
///
/// ⚠️ **Deux champs par série quand on agrège : la moyenne ET l'extrême qui
/// préserve l'incident.** Une moyenne sur deux heures noie un pic à 800 ms dans
/// une valeur à 12 ms : la courbe reste crédible et l'incident disparaît. Une
/// valeur plausible et fausse est pire qu'une valeur absente. Le champ de
/// l'extrême prend un suffixe, ce qui laisse le nom nu à la moyenne :
/// l'interface qui cherchait `latency_ms` continue de le trouver.
///
/// 🔴 **Et cet extrême n'est PAS le même selon ce qu'on mesure.** L'asymétrie
/// est voulue ; un relecteur pressé la « corrigera » par cohérence apparente.
///
/// | Mesure | Agrégation | Pourquoi |
/// |---|---|---|
/// | latence, débit… | `mean` + `max` → `_max` | un pic à 800 ms noyé dans une moyenne à 12 ms disparaît |
/// | disponibilité (booléens) | `mean` SEUL | la moyenne d'un booléen EST le taux |
///
/// 🔴 **Une disponibilité ne reçoit ni minimum ni maximum, et c'est délibéré.**
/// `max(alive)` vaut 1 dès qu'un seul ping passe — cinquante minutes de coupure
/// s'afficheraient « tout va bien ». `min(alive)` vaut 0 dès qu'un seul ping
/// échoue, et un plancher entre 0 et 1 ne se lit tout simplement pas. La bonne
/// réponse n'est aucun des deux : c'est un TAUX, et `mean` sur un booléen
/// converti en 0/1 est exactement ce taux. « 95 % sur la période » se lit d'un
/// coup ; c'est la bande verte et rouge qui reste le support visuel.
///
/// Le tri se fait sur le TYPE Influx, donc avant `toFloat()` : après
/// conversion, un booléen ne se distingue plus d'un compteur à 0 ou 1.
///
/// ⚠️ **`createEmpty: false`, sans exception.** Un intervalle où rien n'a été
/// mesuré ne produit AUCUN point — ni zéro, ni valeur interpolée. Un trou reste
/// un trou : on ne déduit pas un fait d'un silence.
///
/// ⚠️ Les champs TEXTE (`state` d'`internet_status`, `server` d'un speedtest) ne
/// se moyennent pas : `mean` sur une chaîne fait échouer la requête entière,
/// donc les trois graphes de la fiche d'un coup. Ils sont isolés par leur type
/// et rendus bruts, dans un `yield` séparé — une union les rejetterait de toute
/// façon, leur `_value` n'ayant pas le type des autres.
fn metrics_flux(
    bucket: &str,
    probe_id: &str,
    measurement: Option<&str>,
    window: &str,
    every: Option<&str>,
) -> String {
    let mut source = format!(
        "from(bucket: {})\n  |> range({})\n  |> filter(fn: (r) => r[\"probe_id\"] == {})",
        flux_string(bucket),
        window,
        flux_string(probe_id)
    );
    if let Some(measurement) = measurement {
        source.push_str(&format!(
            "\n  |> filter(fn: (r) => r[\"_measurement\"] == {})",
            flux_string(measurement)
        ));
    }
    let Some(every) = every else {
        return source;
    };
    format!(
        r#"import "types"

source = {source}

// Le texte ne se moyenne pas : il passe brut, dans son propre rendu.
texte = source |> filter(fn: (r) => types.isType(v: r._value, type: "string"))

// Les booleens sont des DISPONIBILITES (alive, icmp_ok, dns_ok...). Leur
// moyenne EST le taux de disponibilite de l'intervalle : ni minimum ni
// maximum, un taux. « 95 % sur la periode » se lit d'un coup d'oeil ; un
// minimum entre 0 et 1 ne se lit pas.
dispo = source
  |> filter(fn: (r) => types.isType(v: r._value, type: "bool"))
  |> toFloat()

// Les valeurs continues (latence, debits) vont dans l'autre sens : c'est le
// MAXIMUM qui preserve le pic que la moyenne gomme.
mesures = source
  |> filter(fn: (r) => not types.isType(v: r._value, type: "string") and not types.isType(v: r._value, type: "bool"))
  |> toFloat()

texte |> yield(name: "raw")
mesures |> aggregateWindow(every: {every}, fn: mean, createEmpty: false) |> yield(name: "mean")
mesures
  |> aggregateWindow(every: {every}, fn: max, createEmpty: false)
  |> map(fn: (r) => ({{r with _field: r._field + "_max"}}))
  |> yield(name: "max")
dispo |> aggregateWindow(every: {every}, fn: mean, createEmpty: false) |> yield(name: "uptime")
"#
    )
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
    // ⚠️ `flux_window`, comme `/sla` et `/public-ips` — et non plus le seul
    // `range`. Cette route déclarait `start`/`stop` dans sa requête et les
    // IGNORAIT en silence : demander « du 20 au 22 » renvoyait la dernière
    // heure, sous une légende annonçant trois jours. L'interface devait
    // sur-tirer puis découper elle-même, ce qui faisait lire quatre-vingts
    // jours de points pour en afficher un.
    let window = match flux_window(&query) {
        Ok(w) => w,
        Err(refus) => return refus,
    };
    // ⚠️ Le pas se calcule sur LA fenêtre, celle-là même qui part dans la
    // requête. La recalculer à côté ferait diverger le pas annoncé de la période
    // réellement lue — deux affirmations vraies séparément, fausses ensemble.
    let every = aggregate_every(window.to - window.from, clamp_max_points(query.max_points));
    let bucket = state.settings.influx_bucket();
    let flux = metrics_flux(
        &bucket,
        &id,
        query.measurement.as_deref().filter(|m| !m.is_empty()),
        &window.clause,
        every,
    );
    match state.influx.query_flux(&flux).await {
        Ok(csv) => {
            let mut body = serde_json::json!({
                "range": window.label,
                "series": series_from_flux_csv(&csv),
            });
            // ⚠️ Le champ est ABSENT quand c'est brut, jamais `null` ni `"1s"` :
            // l'interface n'annonce une moyenne que s'il y en a une, sans quoi
            // elle tracerait la mesure à la seconde sous une légende qui promet
            // une moyenne — ou l'inverse.
            if let Some(every) = every {
                body["aggregated_every"] = serde_json::Value::from(every);
            }
            ok_json(body)
        }
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
pub(crate) fn flux_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

// `flux_duration` a été remplacée par `SlidingWindow`. Elle laissait passer
// `24h` tel quel — donc une fenêtre dans le FUTUR — et repliait toute saisie
// illisible sur `-1h` en silence. La garder à côté de son remplaçant serait
// laisser un piège armé : les deux ont la même tête, une seule est sûre.

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

    /// Réponse RÉELLE d'un InfluxDB 2.9.1 — la version que le conteneur
    /// embarque — à la requête agrégée de `metrics_flux`. Trois rendus
    /// (`raw`, `mean`, `max`), donc trois blocs d'en-tête aux colonnes
    /// ordonnées différemment, ce qui casse un parseur qui suppose des
    /// positions fixes. Capturée, pas écrite à la main.
    const AGGREGATED_CSV: &str = "\
,result,table,_start,_stop,_time,_value,_field,_measurement,host,probe_id,site_id
,raw,0,2026-09-02T16:40:00Z,2026-09-02T17:40:00Z,2026-09-02T16:41:19Z,online,state,internet_status,a,p1,s1
,raw,0,2026-09-02T16:40:00Z,2026-09-02T17:40:00Z,2026-09-02T17:00:19Z,offline,state,internet_status,a,p1,s1

,result,table,_time,_start,_stop,_field,_measurement,host,ip,probe_id,site_id,_value
,mean,4,2026-09-02T16:45:00Z,2026-09-02T16:40:00Z,2026-09-02T17:40:00Z,latency_ms,ping_latency,a,1.1.1.1,p1,s1,12
,mean,4,2026-09-02T17:00:00Z,2026-09-02T16:40:00Z,2026-09-02T17:40:00Z,latency_ms,ping_latency,a,1.1.1.1,p1,s1,12.4
,mean,4,2026-09-02T17:20:00Z,2026-09-02T16:40:00Z,2026-09-02T17:40:00Z,latency_ms,ping_latency,a,1.1.1.1,p1,s1,12

,result,table,_start,_stop,_time,_value,_field,_measurement,host,ip,probe_id,site_id
,max,6,2026-09-02T16:40:00Z,2026-09-02T17:40:00Z,2026-09-02T16:45:00Z,12,latency_ms_max,ping_latency,a,1.1.1.1,p1,s1
,max,6,2026-09-02T16:40:00Z,2026-09-02T17:40:00Z,2026-09-02T17:00:00Z,800,latency_ms_max,ping_latency,a,1.1.1.1,p1,s1
,max,6,2026-09-02T16:40:00Z,2026-09-02T17:40:00Z,2026-09-02T17:20:00Z,12,latency_ms_max,ping_latency,a,1.1.1.1,p1,s1
";

    #[test]
    fn an_aggregated_response_carries_the_peak_beside_the_average() {
        // ⚠️ C'est tout l'enjeu : la moyenne de l'intervalle de 17 h reste à
        // 12,4 ms — une courbe parfaitement crédible — alors qu'un pic à
        // 800 ms s'y est produit. Sans la série `_max`, l'incident disparaît
        // sous une valeur plausible et fausse.
        let series = series_from_flux_csv(AGGREGATED_CSV);

        let avg = series.iter().find(|s| s["measure"] == "latency_ms").unwrap();
        let peak = series.iter().find(|s| s["measure"] == "latency_ms_max").unwrap();
        assert_eq!(avg["points"][1]["v"], 12.4);
        assert_eq!(peak["points"][1]["v"], 800.0);

        // Le maximum garde l'étiquette de sa cible : deux IP surveillées en
        // parallèle ont chacune leur pic, les confondre en dessinerait un
        // troisième qui n'a jamais eu lieu.
        assert_eq!(peak["tags"]["ip"], "1.1.1.1");

        // ⚠️ Vingt minutes sans mesure entre 17 h et 17 h 20 : `createEmpty:
        // false` ne rend aucun point, et le parseur n'en invente pas. Un trou
        // reste un trou.
        assert_eq!(avg["points"].as_array().unwrap().len(), 3);

        // Le rendu `raw` des champs texte survit au découpage en blocs, malgré
        // un ordre de colonnes différent de celui des deux autres.
        let state = series.iter().find(|s| s["measure"] == "state").unwrap();
        assert_eq!(state["points"][1]["v"], "offline");
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
                // Hub en clair : aucun certificat du hub à épingler.
                cert_fingerprint: None,
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

        async fn call(&self, mut req: Request<Body>) -> (StatusCode, serde_json::Value, Vec<String>) {
            // En production c'est `into_make_service_with_connect_info` qui
            // pose l'adresse de la socket ; `oneshot` court-circuite la couche
            // qui le fait. On la pose donc ici, sauf si le test en a déjà
            // choisi une (voir `with_peer`).
            if req
                .extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .is_none()
            {
                req.extensions_mut().insert(axum::extract::ConnectInfo(
                    "127.0.0.1:41000".parse::<std::net::SocketAddr>().unwrap(),
                ));
            }
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

    /// Pose l'adresse de la socket, comme le fait `ConnectInfo` en production.
    /// Sans elle, `Harness::call` en met une par défaut : c'est le battement
    /// d'une sonde qui ne bouge pas.
    fn with_peer(mut req: Request<Body>, peer: &str) -> Request<Body> {
        req.extensions_mut().insert(axum::extract::ConnectInfo(
            format!("{peer}:41000").parse::<std::net::SocketAddr>().unwrap(),
        ));
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

    /// Un battement émis depuis `peer`. Rend le corps de la réponse.
    async fn heartbeat_from(
        h: &Harness,
        probe_id: &str,
        token: &str,
        peer: &str,
        body: serde_json::Value,
    ) -> serde_json::Value {
        let req = with_peer(
            with_bearer(
                json_request("POST", &format!("/api/probes/{probe_id}/heartbeat"), body),
                token,
            ),
            peer,
        );
        h.call(req).await.1
    }

    #[tokio::test]
    async fn arming_realtime_needs_the_site_in_scope() {
        // ⚠️ Le mode change le comportement d'une sonde et la charge du hub : un
        // opérateur restreint ne doit pas pouvoir l'armer chez un autre client.
        // 404 et non 403 — « ça existe mais pas pour vous » révèle déjà
        // l'existence de la sonde d'un tiers.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;

        let autre = h.state.db.create_site("Martin").unwrap();
        h.user("olivier", Role::Operator).await;
        h.state
            .db
            .set_user_sites("olivier", &[autre.site_id.clone()])
            .unwrap();
        let restreint = h.login_as("olivier").await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/realtime"),
                    json!({ "on": true }),
                ),
                &restreint,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(
            h.state.db.realtime_until(&probe_id).unwrap(),
            None,
            "rien ne doit avoir été armé"
        );

        // ⚠️ Sans cette moitié, le test ne prouve RIEN : le `fallback` du
        // routeur rend déjà 404 sur toute route inconnue, donc l'assertion
        // ci-dessus passerait à l'identique si la route avait été supprimée
        // par mégarde. On vérifie donc aussi que la MÊME requête aboutit quand
        // le site est dans la portée — c'est ce qui distingue « refusé » de
        // « inexistant ».
        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/realtime"),
                    json!({ "on": true }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(
            h.state.db.realtime_until(&probe_id).unwrap().is_some(),
            "la route existe bel et bien et arme quand la portée le permet"
        );
    }

    #[tokio::test]
    async fn arming_then_disarming_realtime_round_trips() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;

        h.call(with_cookie(
            json_request(
                "POST",
                &format!("/api/probes/{probe_id}/realtime"),
                json!({ "on": true }),
            ),
            &session,
        ))
        .await;
        let armed = h.state.db.realtime_until(&probe_id).unwrap().unwrap();
        assert!(armed > crate::db::now(), "l'échéance doit être dans le futur");

        // L'interface affiche le compte à rebours depuis la liste des sondes :
        // sans ce champ, il lui faudrait un appel de plus par sonde.
        let (_, fleet, _) = h
            .call(with_cookie(
                json_request("GET", "/api/probes", json!({})),
                &session,
            ))
            .await;
        assert_eq!(fleet[0]["realtime_until"], armed, "{fleet}");

        // Réappuyer relance la minuterie : un chantier qui dure ne doit pas
        // obliger à ruser.
        h.call(with_cookie(
            json_request(
                "POST",
                &format!("/api/probes/{probe_id}/realtime"),
                json!({ "on": true }),
            ),
            &session,
        ))
        .await;
        assert!(h.state.db.realtime_until(&probe_id).unwrap().unwrap() >= armed);

        h.call(with_cookie(
            json_request(
                "POST",
                &format!("/api/probes/{probe_id}/realtime"),
                json!({ "on": false }),
            ),
            &session,
        ))
        .await;
        assert_eq!(h.state.db.realtime_until(&probe_id).unwrap(), None);
    }

    #[tokio::test]
    async fn a_probe_in_realtime_mode_is_told_to_beat_faster() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let normal = heartbeat_from(&h, &probe_id, &token, "203.0.113.10", json!({})).await;
        assert_eq!(normal["heartbeat_interval_secs"], 60);

        h.state
            .db
            .set_realtime(&probe_id, Some(crate::db::now() + 1800))
            .unwrap();
        let fast = heartbeat_from(&h, &probe_id, &token, "203.0.113.10", json!({})).await;
        assert_eq!(fast["heartbeat_interval_secs"], 5);
    }

    #[tokio::test]
    async fn an_expired_realtime_mode_falls_back_on_its_own() {
        // ⚠️ Le cœur du dispositif : AUCUNE tâche de fond ne nettoie
        // l'expiration. La décision se prend à la lecture, donc elle est
        // toujours juste — même après un redémarrage du hub, même si personne
        // ne repasse derrière.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        h.state
            .db
            .set_realtime(&probe_id, Some(crate::db::now() - 1))
            .unwrap();
        let back = heartbeat_from(&h, &probe_id, &token, "203.0.113.10", json!({})).await;
        assert_eq!(
            back["heartbeat_interval_secs"], 60,
            "l'échéance passée ne doit plus rien accélérer"
        );
    }

    #[tokio::test]
    async fn a_changed_source_address_asks_the_probe_to_recheck_its_public_ip() {
        // ⚠️ L'adresse vue ici DÉTECTE, elle n'est jamais enregistrée : le
        // battement peut sortir par la route par défaut alors que les mesures
        // sont liées à une autre interface. Le hub déclenche, la sonde mesure.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let first =
            heartbeat_from(&h, &probe_id, &token, "203.0.113.10", serde_json::json!({})).await;
        assert_eq!(
            first["recheck_public_ip"], false,
            "premier battement : rien à comparer, un « changement » serait une invention"
        );

        let second =
            heartbeat_from(&h, &probe_id, &token, "203.0.113.10", serde_json::json!({})).await;
        assert_eq!(second["recheck_public_ip"], false);

        let third =
            heartbeat_from(&h, &probe_id, &token, "198.51.100.4", serde_json::json!({})).await;
        assert_eq!(third["recheck_public_ip"], true, "source différente = relevé demandé");

        // Ce que le hub a vu ne devient jamais l'adresse publique de la sonde.
        let seen = h.state.db.get_probe(&probe_id).unwrap();
        assert_eq!(seen.public_ip, None, "l'adresse source ne doit jamais être enregistrée");
    }

    #[tokio::test]
    async fn without_trusted_proxies_a_forged_header_changes_nothing() {
        // ⚠️ `X-Forwarded-For` est écrit par le client. Sans proxy de
        // confiance déclaré, le lire laisserait une sonde faire croire au hub
        // qu'elle se déplace en boucle et marteler le relevé d'IP publique.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        heartbeat_from(&h, &probe_id, &token, "203.0.113.10", serde_json::json!({})).await;
        let mut req = with_peer(
            with_bearer(
                json_request("POST", &format!("/api/probes/{probe_id}/heartbeat"), serde_json::json!({})),
                &token,
            ),
            "203.0.113.10",
        );
        req.headers_mut()
            .insert("x-forwarded-for", "9.9.9.9".parse().unwrap());
        let (_, body, _) = h.call(req).await;
        assert_eq!(body["recheck_public_ip"], false, "l'en-tête doit être ignoré");
    }

    #[tokio::test]
    async fn an_identity_that_is_not_made_of_addresses_opens_no_interval() {
        // ⚠️ Chaque valeur d'adresse DIFFÉRENTE ouvre un intervalle. Une sonde
        // authentifiée qui bat avec n'importe quoi ferait donc grossir
        // l'historique sans fin — et la purge ne rattrape rien quand la
        // rétention est illimitée. Ce qui n'est pas une adresse est traité
        // comme absent.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/heartbeat"),
                    serde_json::json!({
                        "public_ip": "pas-une-ip",
                        "gateway": "la-box",
                        "local_ips": ["salon", "10.6.8.42/24", "10.6.8.43"]
                    }),
                ),
                &token,
            ))
            .await;
        // ⚠️ Le battement RÉUSSIT quand même : une sonde qui bat mal doit
        // rester visible, c'est tout l'intérêt du battement.
        assert_eq!(status, StatusCode::OK, "{body}");

        let seen = h.state.db.get_probe(&probe_id).unwrap();
        assert_eq!(seen.public_ip, None, "« pas-une-ip » n'est pas une adresse");
        assert_eq!(seen.gateway, None, "« la-box » non plus");
        assert_eq!(
            seen.local_ips,
            vec!["10.6.8.42/24".to_string(), "10.6.8.43".to_string()],
            "seules les entrées qui parsent sont gardées"
        );

        let (_, ips, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/public-ips?range=24h")),
                &session,
            ))
            .await;
        assert!(
            ips["intervals"].as_array().unwrap().is_empty(),
            "aucun intervalle ne doit s'ouvrir : {ips}"
        );
    }

    #[tokio::test]
    async fn a_megabyte_of_junk_as_a_public_ip_opens_no_interval() {
        // Le cas qui fait enfler le disque du hub pour tous les sites : une
        // sonde compromise bat avec une valeur énorme et différente à chaque
        // fois. Rien ne bornait la longueur.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_bearer(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/heartbeat"),
                    serde_json::json!({ "public_ip": "8".repeat(1024 * 1024) }),
                ),
                &token,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let (_, ips, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/public-ips?range=24h")),
                &session,
            ))
            .await;
        assert!(
            ips["intervals"].as_array().unwrap().is_empty(),
            "aucun intervalle ne doit s'ouvrir : {ips}"
        );
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

    // ── Surveillances partagées ────────────────────────────────────────────

    #[tokio::test]
    async fn an_age_is_converted_into_a_date_by_the_hub() {
        // ⚠️ La sonde n'envoie JAMAIS de date : elle dit « il y a N secondes ».
        // Aucune horloge n'est comparée à une autre — c'est ce qui rend le
        // dispositif insensible à la dérive et aux réveils de veille.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        heartbeat_from(&h, &probe_id, &token, "203.0.113.10", json!({
            "monitor_changes": [{ "target": "8.8.8.8", "action": "add", "age_secs": 60 }]
        })).await;

        let m = h.state.db.monitors(&probe_id).unwrap();
        assert_eq!(m.len(), 1);
        let now = crate::db::now();
        assert!(m[0].added_at <= now - 59 && m[0].added_at >= now - 62,
                "daté il y a ~60 s, pas à la réception");
    }

    #[tokio::test]
    async fn an_absurd_age_is_ignored_and_the_change_is_dated_on_arrival() {
        // ⚠️ Une ancienneté aberrante — champ corrompu, bug de la sonde, sonde
        // malveillante — placerait le changement en 1970 et lui ferait gagner
        // TOUS les arbitrages à l'envers : un retrait vieux d'un demi-siècle
        // ne peut plus jamais être défait par un ajout fait depuis le hub.
        // Au-delà de la borne, on ignore la valeur et on date à la réception.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        heartbeat_from(&h, &probe_id, &token, "203.0.113.10", json!({
            "monitor_changes": [
                { "target": "8.8.8.8", "action": "add", "age_secs": 4_000_000_000i64 },
                // Une ancienneté négative n'a pas de sens non plus : elle
                // daterait le changement dans le futur, ce qui gagne tout.
                { "target": "1.1.1.1", "action": "add", "age_secs": -86_400 },
            ]
        })).await;

        let now = crate::db::now();
        let entries = h.state.db.monitors(&probe_id).unwrap();
        // Sans ce compte, la boucle qui suit ne parcourrait rien et le test
        // passerait à vide — y compris si le champ était ignoré en entier.
        assert_eq!(entries.len(), 2, "les deux changements sont bien enregistrés");
        for entry in entries {
            assert!(
                entry.added_at >= now - 5 && entry.added_at <= now,
                "{} daté à la réception, pas en 1970 ni dans le futur : {}",
                entry.target,
                entry.added_at
            );
        }
    }

    #[tokio::test]
    async fn an_old_probe_without_changes_keeps_working() {
        // Compatibilité : sans le champ, rien ne casse et rien n'est effacé.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;
        h.state.db.apply_monitor_change(&probe_id, "1.1.1.1", false, crate::db::now()).unwrap();

        let r = heartbeat_from(&h, &probe_id, &token, "203.0.113.10", json!({})).await;
        assert_eq!(r["ok"], true);
        assert_eq!(h.state.db.monitors(&probe_id).unwrap().len(), 1,
                   "un battement muet n'efface rien");
    }

    #[tokio::test]
    async fn the_heartbeat_answers_with_the_effective_list_removals_included() {
        // La sonde s'aligne sur cette liste. Les retirées EN FONT PARTIE : sans
        // elles, la sonde ne saurait pas distinguer « le hub ne connaît pas
        // cette cible » de « le hub l'a retirée », et la ressusciterait au
        // redémarrage — le défaut d'aujourd'hui, inversé.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;

        let r = heartbeat_from(&h, &probe_id, &token, "203.0.113.10", json!({
            "monitor_changes": [
                { "target": "8.8.8.8", "action": "add",    "age_secs": 300 },
                { "target": "9.9.9.9", "action": "add",    "age_secs": 200 },
                { "target": "9.9.9.9", "action": "remove", "age_secs": 100 },
            ]
        })).await;

        let list = r["monitors"].as_array().expect("la liste effective : {r}");
        assert_eq!(list.len(), 2, "{r}");
        assert_eq!(list[0]["target"], "8.8.8.8");
        assert!(list[0]["removed_at"].is_null(), "{r}");
        assert_eq!(list[1]["target"], "9.9.9.9");
        assert!(list[1]["removed_at"].is_i64(), "la retirée garde sa date : {r}");

        // Accusé de réception : la sonde ne réémet ses changements que
        // lorsqu'ils lui reviennent accusés, comme pour les commandes (§14).
        let acks = r["monitor_acks"].as_array().expect("{r}");
        assert_eq!(acks.len(), 3, "chaque changement reçu est accusé : {r}");
    }

    #[tokio::test]
    async fn removing_from_the_hub_works_even_when_the_probe_is_offline() {
        // ⚠️ Le retrait passait uniquement par une commande envoyée à la
        // sonde : hors ligne, la décision de l'utilisateur disparaissait dans
        // l'intervalle et « Retirées » restait vide. Ici elle est écrite tout
        // de suite ; la sonde s'alignera à son retour.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;
        h.state
            .db
            .apply_monitor_change(&probe_id, "8.8.8.8", false, crate::db::now() - 60)
            .unwrap();

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/monitors/remove"),
                    json!({ "target": "8.8.8.8" }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let entries = h.state.db.monitors(&probe_id).unwrap();
        assert_eq!(entries.len(), 1, "la ligne reste, c'est un fait daté");
        assert!(
            entries[0].removed_at.is_some(),
            "la suppression est écrite sans attendre la sonde"
        );
    }

    #[tokio::test]
    async fn monitor_routes_answer_404_on_an_unknown_probe_not_500() {
        // ⚠️ Un 500 AFFIRME que le hub est en panne. Ici la vérité est « cette
        // sonde n'existe pas » : la clé étrangère `probe_monitors.probe_id`
        // refusait l'écriture, l'erreur rusqlite devenait `DbError::Internal`,
        // et le client partait chercher une panne qui n'existe pas.
        //
        // ⚠️ La garde de portée ne couvre PAS ce cas : elle court-circuite pour
        // un compte `Scope::All`, donc un admin atteignait le handler.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        for route in ["remove", "revive"] {
            let (status, body, _) = h
                .call(with_cookie(
                    json_request(
                        "POST",
                        &format!("/api/probes/sonde-qui-nexiste-pas/monitors/{route}"),
                        json!({ "target": "8.8.8.8" }),
                    ),
                    &session,
                ))
                .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "/{route} : {body}");
            assert_eq!(body["error"], "sonde inconnue", "/{route} : {body}");
        }
    }

    #[tokio::test]
    async fn an_unknown_probe_is_indistinguishable_from_one_out_of_scope() {
        // 🔴 Le piège de la correction précédente. La portée rend déjà 404 pour
        // une sonde qui EXISTE mais qu'on n'a pas le droit de voir, et c'est
        // volontaire : « ça existe mais pas pour vous » révèle l'existence du
        // site d'un autre client. Distinguer les deux cas côté client — par le
        // code OU par le message — annulerait cette protection.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;

        let autre = h.state.db.create_site("Martin").unwrap();
        h.user("olivier", Role::Operator).await;
        h.state.db.set_user_sites("olivier", &[autre.site_id.clone()]).unwrap();
        let restreint = h.login_as("olivier").await;

        // Sonde réelle, hors portée.
        let (hors_portee, corps_hors_portee, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/monitors/remove"),
                    json!({ "target": "8.8.8.8" }),
                ),
                &restreint,
            ))
            .await;

        // Sonde inexistante, même compte.
        let (inexistante, corps_inexistante, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    "/api/probes/sonde-qui-nexiste-pas/monitors/remove",
                    json!({ "target": "8.8.8.8" }),
                ),
                &restreint,
            ))
            .await;

        assert_eq!(hors_portee, inexistante, "les deux refus doivent être le même");
        assert_eq!(corps_hors_portee, corps_inexistante, "et dire exactement la même chose");
        assert_eq!(hors_portee, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reviving_needs_the_site_in_scope() {
        // Même garde que partout : 404 hors portée, et rien d'écrit.
        // ⚠️ Ce test doit aussi vérifier le cas EN portée, sinon il passerait
        // à l'identique si la route n'existait pas — le routeur rend 404 sur
        // toute URL inconnue.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;
        h.state.db.apply_monitor_change(&probe_id, "8.8.8.8", false, 100).unwrap();
        h.state.db.apply_monitor_change(&probe_id, "8.8.8.8", true, 200).unwrap();

        let autre = h.state.db.create_site("Martin").unwrap();
        h.user("olivier", Role::Operator).await;
        h.state.db.set_user_sites("olivier", &[autre.site_id.clone()]).unwrap();
        let restreint = h.login_as("olivier").await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/monitors/revive"),
                    json!({ "target": "8.8.8.8" }),
                ),
                &restreint,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert!(
            h.state.db.monitors(&probe_id).unwrap()[0].removed_at.is_some(),
            "rien ne doit avoir été réactivé"
        );

        // ⚠️ La moitié qui prouve que la route existe : la MÊME requête, en
        // portée, doit aboutir. Sans elle le refus ci-dessus serait
        // indiscernable d'une route absente.
        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/monitors/revive"),
                    json!({ "target": "8.8.8.8" }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            h.state.db.monitors(&probe_id).unwrap()[0].removed_at,
            None,
            "réactiver, c'est effacer la suppression"
        );
    }

    #[tokio::test]
    async fn adding_from_the_hub_is_recorded_at_once_like_removing_is() {
        // 🔴 Lu dans la base de production le 02/09 :
        //
        //     02:09:16  commande add_monitor {"ip":"8.8.8.8"}  ← émise par le hub
        //     02:10:34  la cible est PINGUÉE, mais absente de probe_monitors
        //     02:12:31  probe_monitors reçoit enfin sa ligne
        //
        // Trois minutes pendant lesquelles le hub ne connaît pas une cible
        // qu'il vient lui-même d'ordonner de surveiller. C'est ce trou qui
        // produisait le drapeau « plus annoncée » sur l'écran de la sonde : il
        // disait vrai, sur une cible que le hub avait demandée.
        //
        // Retirer écrit tout de suite depuis `POST /monitors/remove`. Ajouter
        // attendait que la sonde RÉANNONCE la cible. Les deux gestes doivent
        // être symétriques : le hub note sa décision au moment où il l'émet.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/commands"),
                    json!({ "kind": "add_monitor", "args": { "ip": "8.8.8.8" } }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        // ⚠️ AUCUN battement entre les deux : c'est tout l'objet du test.
        let entries = h.state.db.monitors(&probe_id).unwrap();
        assert_eq!(entries.len(), 1, "la décision du hub est écrite sans attendre la sonde : {body}");
        assert_eq!(entries[0].target, "8.8.8.8");
        assert_eq!(entries[0].removed_at, None, "elle est ajoutée, pas retirée");
    }

    #[tokio::test]
    async fn adding_from_the_hub_still_sends_the_order_to_the_probe() {
        // ⚠️ On AJOUTE un chemin, on n'en retire pas. La route ne fait
        // qu'enregistrer la décision ; c'est la commande qui fait AGIR la
        // sonde. Noter la décision sans empiler la commande donnerait une
        // cible « surveillée » côté hub que personne ne pingue — l'écran
        // afficherait « en attente du premier relevé » pour toujours.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/commands"),
                    json!({ "kind": "add_monitor", "args": { "ip": "8.8.8.8" } }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "pending", "la commande part toujours : {body}");

        let commands = h.state.db.list_commands(&probe_id, 50).unwrap();
        assert_eq!(commands.len(), 1, "la file garde son ordre");
    }

    #[tokio::test]
    async fn adding_never_resurrects_a_target_removed_more_recently() {
        // ⚠️ Règle du §20, et c'est exactement ce qu'un ajout écrit « en
        // direct » peut casser : l'arbitrage au plus récent reste délégué à
        // `apply_monitor_change`, l'ajout ne force jamais la ligne. Un retrait
        // postérieur — la sonde rapporte au battement un geste local daté
        // d'après le nôtre — doit continuer de gagner.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;
        h.state
            .db
            .apply_monitor_change(&probe_id, "8.8.8.8", true, crate::db::now() + 60)
            .unwrap();

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/commands"),
                    json!({ "kind": "add_monitor", "args": { "ip": "8.8.8.8" } }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let entries = h.state.db.monitors(&probe_id).unwrap();
        assert!(
            entries[0].removed_at.is_some(),
            "le retrait plus récent tient : l'ajout ne le piétine pas"
        );
    }

    #[tokio::test]
    async fn adding_on_an_unknown_probe_writes_nothing_and_still_says_404() {
        // ⚠️ Acquis à ne pas casser : « sonde inconnue » et « sonde hors
        // portée » restent indiscernables — même code, même message. Et une
        // décision notée pour une sonde qui n'existe pas serait une ligne
        // orpheline qu'aucun battement ne viendrait jamais confirmer.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    "/api/probes/sonde-qui-nexiste-pas/commands",
                    json!({ "kind": "add_monitor", "args": { "ip": "8.8.8.8" } }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"], "sonde inconnue", "{body}");
        assert!(
            h.state.db.monitors("sonde-qui-nexiste-pas").unwrap().is_empty(),
            "rien d'écrit pour une sonde qui n'existe pas"
        );
    }

    #[tokio::test]
    async fn removing_from_the_command_queue_is_recorded_at_once_too() {
        // 🔴 Le miroir du défaut de l'ajout, et le dernier de la série. Le
        // bouton « Retirer » de l'écran passe par `POST /monitors/remove`
        // depuis 8a946d5 et écrit tout de suite ; mais le type de commande
        // `remove_monitor` est toujours servi, et il rouvrait exactement la
        // même fenêtre pour un appel scripté : l'ordre partait sans que le hub
        // note sa propre décision.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;
        h.state
            .db
            .apply_monitor_change(&probe_id, "8.8.8.8", false, crate::db::now() - 60)
            .unwrap();

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/commands"),
                    json!({ "kind": "remove_monitor", "args": { "ip": "8.8.8.8" } }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "pending", "la commande part toujours : {body}");

        // ⚠️ AUCUN battement entre les deux.
        let entries = h.state.db.monitors(&probe_id).unwrap();
        assert_eq!(entries.len(), 1, "la ligne reste, c'est un fait daté");
        assert!(
            entries[0].removed_at.is_some(),
            "la suppression est écrite sans attendre la sonde"
        );
        assert_eq!(
            h.state.db.list_commands(&probe_id, 50).unwrap().len(),
            1,
            "la file garde son ordre : c'est elle qui fait AGIR la sonde"
        );
    }

    #[tokio::test]
    async fn removing_never_beats_an_add_made_more_recently() {
        // ⚠️ L'arbitrage du §20 dans l'autre sens, et c'est la propriété que ce
        // cinquième correctif pouvait casser : un retrait doit PERDRE contre un
        // ajout postérieur, exactement comme l'ajout perd contre un retrait
        // postérieur. Rien n'est écrit en propre ici, tout passe par
        // `apply_monitor_change` — c'est lui, et lui seul, qui tranche.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;
        h.state
            .db
            .apply_monitor_change(&probe_id, "8.8.8.8", false, crate::db::now() + 60)
            .unwrap();

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/probes/{probe_id}/commands"),
                    json!({ "kind": "remove_monitor", "args": { "ip": "8.8.8.8" } }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        assert_eq!(
            h.state.db.monitors(&probe_id).unwrap()[0].removed_at,
            None,
            "l'ajout plus récent tient : le retrait ne le piétine pas"
        );
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

    /// Une requête de fenêtre, sans le reste.
    fn window_query(range: &str, bounds: Option<(i64, i64)>) -> MetricsQuery {
        MetricsQuery {
            measurement: None,
            range: Some(range.into()),
            start: bounds.map(|b| b.0),
            stop: bounds.map(|b| b.1),
            max_points: None,
        }
    }

    #[test]
    fn explicit_bounds_win_over_a_sliding_window() {
        // ⚠️ Un rapport de SLA porte sur une période convenue — « du 1er au
        // 31 » — pas sur « les sept derniers jours ». Une fenêtre glissante
        // donnerait un chiffre différent à chaque ouverture du document.
        let window = flux_window(&window_query("-7d", Some((1_788_000_000, 1_788_600_000))))
            .expect("des bornes explicites sont toujours lisibles");
        assert_eq!(window.clause, "start: 1788000000, stop: 1788600000");
        assert_eq!(window.label, "1788000000..1788600000");
        assert_eq!((window.from, window.to), (1_788_000_000, 1_788_600_000));
    }

    #[test]
    fn reversed_bounds_fall_back_instead_of_asking_influx_for_nothing() {
        // Des bornes inversées rendraient une fenêtre vide, donc un rapport à
        // 0 % qu'on prendrait pour une panne totale.
        let window = flux_window(&window_query("-24h", Some((1_788_600_000, 1_788_000_000))))
            .expect("bornes inversées : on retombe sur la fenêtre glissante");
        assert_eq!(window.clause, "start: -24h");
        assert_eq!(window.to - window.from, 24 * 3600);
    }

    #[test]
    fn one_reading_of_the_string_decides_the_clause_and_the_seconds() {
        // 🔴 La clause Flux et la fenêtre en secondes viennent de LA MÊME
        // lecture. Deux lectures séparées ne disaient déjà pas la même chose :
        // la lecture Flux refusait ce qu'elle ne comprenait pas quand la
        // lecture en secondes rendait 24 h en silence, et c'est cette
        // divergence qui a produit le `24h` visant le futur puis le repli muet
        // de `/public-ips`.
        for (range, clause, span) in [
            ("24h", "start: -24h", 86_400),
            ("-7d", "start: -7d", 7 * 86_400),
            ("-30m", "start: -30m", 1_800),
        ] {
            let window = flux_window(&window_query(range, None))
                .unwrap_or_else(|_| panic!("« {range} » est une fenêtre lisible"));
            assert_eq!(window.clause, clause, "fenêtre {range}");
            assert_eq!(window.label, &clause["start: ".len()..], "fenêtre {range}");
            assert_eq!(window.to - window.from, span, "fenêtre {range}");
        }
    }

    #[test]
    fn an_unreadable_range_yields_no_window_at_all() {
        // ⚠️ Ni clause, ni secondes : une saisie illisible ne produit AUCUNE
        // fenêtre. C'est ce qui ferme la porte par laquelle le repli muet
        // revenait route par route — il n'existe plus d'endroit où retomber
        // sur 24 h sans le dire.
        for illisible in ["abc", "", "12x", "-", "1h) |> drop(", "99999999999999999999h"] {
            assert!(
                flux_window(&window_query(illisible, None)).is_err(),
                "« {illisible} » ne décrit aucune fenêtre"
            );
        }
    }

    // ── Agrégation : nombre de points, jamais ancienneté ───────────────────

    #[test]
    fn a_bare_duration_means_the_LAST_24h_not_the_next_ones() {
        // 🔴 `range=24h` produisait `range(start: 24h)`. En Flux une durée
        // POSITIVE part de maintenant vers l'AVANT : la fenêtre visait les
        // 24 h à venir, Influx refusait en 400, et le hub rendait 502 — donc
        // « InfluxDB est en panne » pour une saisie parfaitement anodine.
        //
        // ⚠️ La lecture en secondes coupait déjà le signe : les deux fonctions
        // lisaient la même chaîne différemment. Une seule la lit désormais.
        let flux = |r: &str| SlidingWindow::parse(r).map(|w| w.flux());
        assert_eq!(flux("24h").as_deref(), Some("-24h"));
        assert_eq!(flux("-24h").as_deref(), Some("-24h"));
        assert_eq!(flux("  7d ").as_deref(), Some("-7d"));
        assert_eq!(flux("-30m").as_deref(), Some("-30m"));
        // Et la même lecture donne la durée : `24h` vaut 24 h de PASSÉ, pas
        // une fenêtre vide comme le signe positif le faisait croire à Influx.
        assert_eq!(SlidingWindow::parse("24h").unwrap().secs(), 86_400);
    }

    #[test]
    fn an_unreadable_range_is_refused_instead_of_silently_becoming_an_hour() {
        // ⚠️ Le repli muet sur `-1h` est la même faute que le 502 : l'écran
        // affichait une heure sous une légende annonçant autre chose, et
        // personne ne pouvait s'en apercevoir. Un refus se comprend.
        assert!(SlidingWindow::parse("abc").is_none());
        assert!(SlidingWindow::parse("").is_none());
        assert!(SlidingWindow::parse("12x").is_none());
        assert!(SlidingWindow::parse("-").is_none());
        // Une injection Flux ne doit évidemment pas passer non plus.
        assert!(SlidingWindow::parse("1h) |> drop(").is_none());
        // ⚠️ Un compte qui déborde non plus : il passait la première lecture
        // (`-99999999999999999999h` partait dans la requête, qu'Influx
        // refusait en 502) et retombait sur 24 h dans la seconde.
        assert!(SlidingWindow::parse("99999999999999999999h").is_none());
    }

    #[test]
    fn a_window_that_fits_the_graph_stays_raw_however_old_it_is() {
        // ⚠️ La règle est le NOMBRE de points rapporté à la largeur du graphe,
        // pas l'âge de la plage ni sa durée. Dix minutes prises il y a trois
        // mois tiennent dans huit cents pixels : on les rend brutes. Agréger
        // sur l'ancienneté effacerait le détail qu'on est justement venu voir.
        assert_eq!(aggregate_every(600, 800), None);
        assert_eq!(aggregate_every(800, 800), None);
        // Un pas ne descend jamais sous la seconde : la cadence de mesure est
        // d'une seconde, un pas plus fin fabriquerait des intervalles vides.
        assert_eq!(aggregate_every(801, 800), Some("2s"));
    }

    #[test]
    fn a_long_window_gets_a_step_a_human_can_read() {
        // « moyenné par 1 847 s » ne se lit pas ; « moyenné par 30 min » si.
        assert_eq!(aggregate_every(86_400, 800), Some("2m"));
        assert_eq!(aggregate_every(7 * 86_400, 800), Some("15m"));
        // Les 87 jours du rapport : ~7,5 millions de points bruts par cible.
        assert_eq!(aggregate_every(87 * 86_400, 800), Some("3h"));

        // Chaque pas rendu appartient à l'échelle lisible, et tient la
        // promesse : jamais plus de points que la largeur demandée.
        for span in [900i64, 3_600, 86_400, 30 * 86_400, 400 * 86_400] {
            for max in [100i64, 800, 5_000] {
                let Some(label) = aggregate_every(span, max) else {
                    continue;
                };
                let (secs, _) = AGGREGATE_STEPS
                    .iter()
                    .find(|(_, l)| *l == label)
                    .unwrap_or_else(|| panic!("pas illisible : {label}"));
                assert!(*secs >= 1, "plancher d'une seconde : {label}");
                assert!(span / secs <= max, "{span} s par {label} déborde de {max} points");
            }
        }
    }

    #[test]
    fn an_absurd_window_falls_back_on_the_coarsest_step_not_on_raw_points() {
        // Rendre `None` ici enverrait des millions de points au navigateur —
        // l'inverse de ce que le plafond protège.
        assert_eq!(aggregate_every(4_000 * 86_400, 100), Some("7d"));
    }

    #[test]
    fn max_points_is_bounded_because_it_comes_from_the_browser() {
        // C'est une largeur en pixels envoyée par l'interface : `0` ferait une
        // division par zéro, et `10_000_000` rendrait le plafond inopérant.
        assert_eq!(clamp_max_points(None), 800);
        assert_eq!(clamp_max_points(Some(0)), 100);
        assert_eq!(clamp_max_points(Some(-5)), 100);
        assert_eq!(clamp_max_points(Some(1_200)), 1_200);
        assert_eq!(clamp_max_points(Some(9_999_999)), 5_000);
    }

    #[test]
    fn a_raw_query_carries_no_aggregate_window_at_all() {
        // Brut veut dire brut : pas d'`aggregateWindow`, pas d'import, pas de
        // conversion. La requête d'hier doit rester mot pour mot celle-ci.
        let flux = metrics_flux("lanprobe", "sonde-1", Some("ping_latency"), "start: -10m", None);
        assert!(!flux.contains("aggregateWindow"), "{flux}");
        assert!(!flux.contains("import"), "{flux}");
        assert!(flux.starts_with("from(bucket: \"lanprobe\")"), "{flux}");
        assert!(flux.contains("r[\"_measurement\"] == \"ping_latency\""), "{flux}");
    }

    #[test]
    fn an_aggregated_query_keeps_the_peak_and_never_fills_a_hole() {
        let flux = metrics_flux("lanprobe", "sonde-1", Some("ping_latency"), "start: -87d", Some("3h"));

        // ⚠️ Deux champs par série. Une moyenne sur trois heures noie un pic à
        // 800 ms dans une valeur à 12 ms : la courbe reste crédible et
        // l'incident disparaît. Le maximum de l'intervalle est ce qui le garde.
        assert!(flux.contains("fn: mean"), "{flux}");
        assert!(flux.contains("fn: max"), "{flux}");
        assert!(flux.contains("_field + \"_max\""), "{flux}");
        // Trois fenêtres : moyenne et maximum des valeurs continues, TAUX des
        // disponibilités (voir le test de l'agrégation en taux).
        assert_eq!(flux.matches("every: 3h").count(), 3, "{flux}");

        // ⚠️ Un intervalle sans mesure ne produit AUCUN point — ni zéro, ni
        // valeur interpolée. Un trou reste un trou : on ne déduit pas un fait
        // d'un silence.
        assert_eq!(flux.matches("createEmpty: false").count(), 3, "{flux}");
        assert!(!flux.contains("createEmpty: true"), "{flux}");

        // Les champs texte (`state` d'`internet_status`) ne se moyennent pas :
        // `mean` sur une chaîne fait échouer la requête ENTIÈRE, donc les trois
        // graphes de la fiche. Ils passent par un rendu séparé, bruts.
        assert!(flux.contains("types.isType"), "{flux}");
        // ⚠️ `types`, PAS `experimental/types` : ce dernier n'existe pas, et
        // InfluxDB 2.9.1 refuse la requête ENTIÈRE sur un import inconnu —
        // « invalid import path ». Vérifié contre un vrai influxd 2.9.1.
        assert!(flux.contains("import \"types\""), "{flux}");
    }

    #[test]
    fn availability_aggregates_into_a_rate_not_into_an_extreme() {
        // 🔴 L'asymétrie est VOULUE, et un relecteur pressé la « corrigera »
        // par cohérence apparente. Elle tient en deux phrases :
        //
        //   • LATENCE — c'est le MAXIMUM qui préserve l'incident. Un pic à
        //     800 ms noyé dans une moyenne à 12 ms disparaît.
        //   • DISPONIBILITÉ — c'est le MINIMUM. `max(alive)` vaut 1 dès qu'UN
        //     SEUL ping passe : cinquante minutes de coupure sur une heure
        //     s'afficheraient « tout va bien ».
        //
        // Un `alive_max` toujours à 1 est le cas d'école de la valeur plausible
        // et fausse : elle ne fait échouer aucune requête, elle rassure à tort.
        let flux = metrics_flux("lanprobe", "sonde-1", Some("ping_latency"), "start: -87d", Some("3h"));

        // 🔴 Et la disponibilité n'a NI l'un NI l'autre. Décision du
        // propriétaire : la bonne agrégation d'un booléen n'est ni un minimum
        // ni un maximum, c'est un TAUX — et `mean` sur un booléen converti en
        // 0/1 est exactement ce taux. « 95 % sur la période » se lit d'un
        // coup ; un minimum de disponibilité entre 0 et 1 ne se lit pas.
        assert!(!flux.contains("fn: min"), "{flux}");
        assert!(!flux.contains("_min"), "{flux}");
        assert!(flux.contains("types.isType(v: r._value, type: \"bool\")"), "{flux}");

        // Trois familles disjointes : texte brut, booléens en TAUX, valeurs
        // continues en moyenne + maximum.
        assert_eq!(flux.matches("fn: mean").count(), 2, "{flux}");
        assert_eq!(flux.matches("every: 3h").count(), 3, "{flux}");
        assert_eq!(flux.matches("createEmpty: false").count(), 3, "{flux}");
    }

    /// Réponse RÉELLE d'un InfluxDB 2.9.1 sur une fenêtre où la cible ne
    /// répond presque plus : un ping sur dix passe pendant une heure.
    /// Capturée, pas écrite à la main.
    const OUTAGE_CSV: &str = "\
,result,table,_start,_stop,_time,_value,_field,_measurement,host,ip,probe_id,site_id
,max,0,2026-09-02T16:55:37Z,2026-09-02T17:55:37Z,2026-09-02T17:00:00Z,14,latency_ms_max,ping_latency,a,1.1.1.1,p9,s1
,max,0,2026-09-02T16:55:37Z,2026-09-02T17:55:37Z,2026-09-02T17:15:00Z,14,latency_ms_max,ping_latency,a,1.1.1.1,p9,s1

,result,table,_time,_start,_stop,_field,_measurement,host,ip,probe_id,site_id,_value
,uptime,0,2026-09-02T17:00:00Z,2026-09-02T16:55:37Z,2026-09-02T17:55:37Z,alive,ping_latency,a,1.1.1.1,p9,s1,0.07692307692307693
,uptime,0,2026-09-02T17:15:00Z,2026-09-02T16:55:37Z,2026-09-02T17:55:37Z,alive,ping_latency,a,1.1.1.1,p9,s1,0.1
";

    #[test]
    fn a_mostly_failed_window_reports_its_rate_and_no_extreme() {
        // Une heure à un ping sur dix : le taux le dit — 7,7 % puis 10 %.
        // `max(alive)` aurait rendu 1 (« la cible a répondu »), ce qui est
        // vrai et n'est pas la question ; `min(alive)` aurait rendu 0, ce qui
        // est vrai aussi et ne se lit pas davantage.
        let series = series_from_flux_csv(OUTAGE_CSV);
        // ⚠️ Ni minimum ni maximum : une disponibilité s'agrège en TAUX.
        assert!(
            series.iter().all(|s| s["measure"] != "alive_max" && s["measure"] != "alive_min"),
            "une disponibilité ne s'accompagne d'aucun extrême : {series:?}"
        );

        // La moyenne d'un booléen est un TAUX, et elle dit la vérité : 7,7 %
        // de pings aboutis sur le premier intervalle. C'est lui qu'on affiche.
        let taux = series.iter().find(|s| s["measure"] == "alive").unwrap();
        assert_eq!(taux["points"][0]["v"], 0.076_923_076_923_076_93);

        // ⚠️ L'asymétrie n'est pas un abandon du maximum : sur la LATENCE, qui
        // est une valeur continue, il reste émis dans la même réponse.
        assert!(series.iter().any(|s| s["measure"] == "latency_ms_max"), "{series:?}");
    }

    #[test]
    fn an_unlimited_retention_purges_nothing() {
        // ⚠️ `0` = rétention illimitée, et c'est le DÉFAUT. Calculer
        // `now - 0 × 86400` donnerait `now` et effacerait tout l'historique
        // dès le premier passage de la tâche de purge, sur l'installation la
        // plus courante qui soit.
        assert_eq!(public_ip_history_cutoff(0, 1_788_600_000), None);
        assert_eq!(public_ip_history_cutoff(-1, 1_788_600_000), None);
        assert_eq!(
            public_ip_history_cutoff(30, 1_788_600_000),
            Some(1_788_600_000 - 30 * 86_400)
        );
    }

    #[tokio::test]
    async fn the_sla_payload_carries_the_address_history() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;
        heartbeat_from(
            &h,
            &probe_id,
            &token,
            "203.0.113.10",
            serde_json::json!({ "public_ip": "88.120.0.1", "gateway": "10.6.8.1" }),
        )
        .await;

        let (status, sla, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/sla?range=24h")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{sla}");
        let history = sla["public_ip_history"]
            .as_array()
            .expect("public_ip_history absent");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["public_ip"], "88.120.0.1");
        assert_eq!(history[0]["label"], serde_json::Value::Null);

        // La même chose se lit seule, pour le tableau qui n'a pas besoin du
        // rapport complet.
        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/public-ips?range=24h")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["intervals"][0]["public_ip"], "88.120.0.1");
    }

    #[tokio::test]
    async fn the_sla_payload_is_the_same_whether_it_comes_through_http_or_not() {
        // 🔴 Le rapport du hub (contrat § 23) doit produire EXACTEMENT le
        // corps que `/sla` rend déjà : c'est le seul endroit de cette tâche où
        // une régression silencieuse est possible — un champ perdu ne fait
        // échouer aucune requête, il vide une colonne du classeur remis au
        // client.
        //
        // Ce test compare les deux chemins champ par champ. Il redeviendrait
        // rouge le jour où quelqu'un « améliore » l'un des deux.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;
        heartbeat_from(
            &h,
            &probe_id,
            &token,
            "203.0.113.10",
            serde_json::json!({ "public_ip": "88.120.0.1", "gateway": "10.6.8.1" }),
        )
        .await;

        let (status, par_http, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/sla?range=24h")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{par_http}");

        let query = MetricsQuery {
            measurement: None,
            range: Some("24h".into()),
            start: None,
            stop: None,
            max_points: None,
        };
        let window = flux_window(&query).expect("la fenêtre doit être lisible");
        let mut hors_http = crate::sla_payload::build(&h.state, &probe_id, &window, None, None)
            .await
            .expect("le rapport doit se construire hors HTTP");

        // ⚠️ Seul `generated_at` est écarté, et pour une raison qui n'est pas
        // du confort : c'est l'instant de la génération, les deux appels ne
        // tombent pas dans la même seconde. Il est vérifié présent des deux
        // côtés plutôt qu'ignoré.
        let mut par_http = par_http;
        assert!(par_http["generated_at"].is_i64(), "{par_http}");
        assert!(hors_http["generated_at"].is_i64(), "{hors_http}");
        par_http["generated_at"] = serde_json::Value::Null;
        hors_http["generated_at"] = serde_json::Value::Null;

        assert_eq!(
            par_http, hors_http,
            "le rapport doit être le même octet pour octet, `generated_at` mis à part"
        );
        assert_eq!(par_http["probe"], "Paris");
        assert_eq!(par_http["site"], "Durand");
        assert_eq!(par_http["public_ip_history"][0]["public_ip"], "88.120.0.1");
        // Les bornes sont rendues TELLES QU'ELLES ONT ÉTÉ DEMANDÉES : `null`
        // sur une fenêtre glissante, et non recalculées d'après la fenêtre.
        assert!(par_http["start"].is_null(), "{par_http}");
        assert!(par_http["stop"].is_null(), "{par_http}");
    }

    #[tokio::test]
    async fn an_unreadable_window_is_refused_by_the_address_history_too() {
        // ⚠️ `/public-ips` était la dernière route de fenêtre à retomber EN
        // SILENCE sur 24 h : `range=nawak` rendait un tableau bien formé, mais
        // celui d'une période que personne n'avait demandée, sous une légende
        // qui en annonçait une autre. `/metrics`, `/sla`, `/sla.csv` et
        // `/uptime` refusent depuis cfff27d ; un refus se comprend, un repli
        // muet non.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;
        heartbeat_from(
            &h,
            &probe_id,
            &token,
            "203.0.113.10",
            serde_json::json!({ "public_ip": "88.120.0.1", "gateway": "10.6.8.1" }),
        )
        .await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/public-ips?range=nawak")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], "fenêtre illisible", "{body}");
        // 🔴 Le code seul ne suffit pas : c'est l'ABSENCE d'effet qu'on vérifie.
        // Un 400 qui rendrait quand même les intervalles des dernières 24 h
        // laisserait l'interface les afficher.
        assert!(
            body.get("intervals").is_none(),
            "une fenêtre refusée ne rend aucun intervalle : {body}"
        );

        // Et la preuve que le refus ne vient pas d'un historique vide : la même
        // requête avec une fenêtre lisible rend bien l'intervalle que le repli
        // muet aurait servi.
        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/public-ips?range=24h")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["intervals"][0]["public_ip"], "88.120.0.1", "{body}");
    }

    #[tokio::test]
    async fn an_unknown_probe_gets_a_404_not_an_empty_address_history() {
        // 🔴 `{"intervals": []}` AFFIRME que la sonde existe et n'a jamais
        // changé d'adresse. Sur un identifiant mal saisi, c'est une réponse
        // plausible et fausse — la faute que ce dépôt refuse partout ailleurs,
        // et la conclusion exactement inverse de la vérité.
        //
        // ⚠️ La garde de portée ne couvre PAS ce cas : elle court-circuite pour
        // un compte `Scope::All`, donc un admin atteint le handler.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", "/api/probes/sonde-qui-nexiste-pas/public-ips?range=24h"),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"], "sonde inconnue", "{body}");
        assert!(
            body.get("intervals").is_none(),
            "une sonde inconnue ne rend aucun tableau : {body}"
        );
    }

    #[tokio::test]
    async fn a_probe_that_never_changed_address_still_gets_an_empty_table() {
        // ⚠️ L'autre moitié du correctif, sans quoi il se perdrait : une sonde
        // qui EXISTE et n'a pas d'historique rend 200 avec un tableau vide.
        // L'absence d'intervalle n'est pas une erreur — une fenêtre antérieure
        // à la mise en place de l'historique en est le cas courant.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/public-ips?range=24h")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["intervals"].as_array().map(|a| a.len()),
            Some(0),
            "une sonde sans historique rend un tableau vide : {body}"
        );
    }

    #[tokio::test]
    async fn the_address_history_hides_an_out_of_scope_probe_behind_the_same_404() {
        // 🔴 Le piège du correctif ci-dessus. Une sonde qui existe mais qu'on
        // n'a pas le droit de voir doit rester INDISCERNABLE d'une sonde
        // inexistante — même code ET même message. Les distinguer révélerait
        // l'existence du parc d'un autre client, ce que la portée protège.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;

        let autre = h.state.db.create_site("Martin").unwrap();
        h.user("olivier", Role::Viewer).await;
        h.state.db.set_user_sites("olivier", &[autre.site_id.clone()]).unwrap();
        let restreint = h.login_as("olivier").await;

        let (hors_portee, corps_hors_portee, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/public-ips?range=24h")),
                &restreint,
            ))
            .await;
        let (inexistante, corps_inexistante, _) = h
            .call(with_cookie(
                empty_request("GET", "/api/probes/sonde-qui-nexiste-pas/public-ips?range=24h"),
                &restreint,
            ))
            .await;

        assert_eq!(hors_portee, StatusCode::NOT_FOUND, "{corps_hors_portee}");
        assert_eq!(hors_portee, inexistante, "les deux refus doivent être le même");
        assert_eq!(
            corps_hors_portee, corps_inexistante,
            "et dire exactement la même chose"
        );
    }

    #[tokio::test]
    async fn the_scope_guard_checks_existence_for_an_admin_too() {
        // 🔴 **Ce test est écrit avec un compte `Scope::All`, et c'est tout son
        // intérêt.** La garde de portée court-circuitait pour ces comptes-là :
        // elle vérifiait l'existence de la sonde par effet de bord, pour les
        // seuls comptes restreints. Un test écrit avec un compte restreint
        // reste donc vert quoi qu'on casse sur ce chemin — c'est l'angle mort
        // qui a laissé le même défaut revenir trois fois (moniteurs,
        // `/public-ips`, puis `/commands`).
        //
        // ⚠️ Il redeviendrait rouge si quelqu'un remettait le court-circuit.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", "/api/probes/sonde-qui-nexiste-pas/commands"),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"], "sonde inconnue", "{body}");
        assert!(
            body.get("commands").is_none(),
            "une sonde inconnue ne rend aucune file : {body}"
        );
    }

    #[tokio::test]
    async fn both_verbs_of_the_command_route_answer_the_same_on_an_unknown_probe() {
        // ⚠️ `POST` vérifiait l'existence, `GET` non : la MÊME URL affirmait
        // « cette sonde n'existe pas » d'un côté et « elle n'a jamais reçu de
        // commande » de l'autre. Deux verbes qui ne racontent pas la même
        // histoire sur le même objet, c'est déjà la preuve qu'un des deux ment.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let (lecture, corps_lecture, _) = h
            .call(with_cookie(
                empty_request("GET", "/api/probes/sonde-qui-nexiste-pas/commands"),
                &session,
            ))
            .await;
        let (ecriture, corps_ecriture, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    "/api/probes/sonde-qui-nexiste-pas/commands",
                    json!({ "kind": "discovery", "args": {} }),
                ),
                &session,
            ))
            .await;

        assert_eq!(lecture, StatusCode::NOT_FOUND, "{corps_lecture}");
        assert_eq!(lecture, ecriture, "les deux verbes doivent répondre pareil");
        assert_eq!(corps_lecture, corps_ecriture, "et dire la même chose");
    }

    #[tokio::test]
    async fn a_probe_that_exists_still_lists_its_empty_command_queue() {
        // ⚠️ L'autre moitié : une sonde qui EXISTE et n'a jamais reçu de
        // commande rend 200 avec une file vide. Sans ce test, un correctif
        // trop large transformerait « rien à faire » en « sonde inconnue ».
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _token) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/commands")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["commands"].as_array().map(|c| c.len()), Some(0), "{body}");
    }

    #[tokio::test]
    async fn an_identity_carries_the_device_it_came_from() {
        // ⚠️ Un seul site de construction pour `Identity` : `identity_of`.
        // Deux endroits qui la fabriquent, c'est un endroit qui oubliera un
        // jour de relire le rôle ou la portée — et une portée figée survivrait
        // à sa révocation (contrat § 22).
        //
        // `device_id` est `None` pour une session de navigateur, et nommé pour
        // un appareil appairé : c'est ce qui permet à la ligne d'un rapport de
        // dire depuis QUEL téléphone il a été demandé (§ 23). L'audit dirait
        // « benjamin », ce qui est vrai et trompeur quand le téléphone est
        // entre d'autres mains.
        let h = Harness::with_admin().await;

        let par_session = identity_of(&h.state, "admin", None).expect("le compte existe");
        assert!(par_session.device_id.is_none(), "une session n'a pas d'appareil");
        assert_eq!(par_session.role, Role::Admin);

        let par_appareil = identity_of(&h.state, "admin", Some("appareil-1".into()))
            .expect("le compte existe");
        assert_eq!(par_appareil.device_id.as_deref(), Some("appareil-1"));
        // ⚠️ Le rôle et la portée restent ceux du COMPTE, relus : un appareil
        // n'a pas de droits propres.
        assert_eq!(par_appareil.role, par_session.role);
        assert_eq!(par_appareil.scope, par_session.scope);
    }

    #[tokio::test]
    async fn a_bearer_on_a_session_route_is_judged_and_never_panics() {
        // Le crochet d'authentification par porteur est posé dans
        // `session_guard`. Ce que ce test fige, c'est qu'un porteur que le hub
        // ne reconnaît pas est refusé proprement — pas qu'il soit accepté.
        //
        // ⚠️ Il fige aussi l'ordre : le cookie d'abord, le porteur ensuite. Un
        // navigateur n'envoie jamais d'en-tête `Authorization` et l'app
        // n'aura jamais de cookie ; l'ordre inverse ne changerait rien en
        // pratique, mais celui-ci ne peut pas régresser sur le chemin du
        // navigateur, qui est le seul en production aujourd'hui.
        let h = Harness::with_admin().await;

        assert!(
            crate::pairing::identity_from_access_token(&h.state, "jeton-inconnu").is_none(),
            "un jeton qui n'a pas été scellé par ce hub n'ouvre rien"
        );

        let mut req = empty_request("GET", "/api/probes");
        req.headers_mut().insert(
            header::AUTHORIZATION,
            "Bearer jeton-inconnu".parse().unwrap(),
        );
        let (status, body, _) = h.call(req).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        // 🔴 **Le refus ne parle pas d'expiration.** Il n'en dit pas plus
        // qu'avant — il ne nomme toujours pas le motif, et n'apprend donc
        // toujours pas quels appareils existent — mais il cesse d'affirmer une
        // expiration qui n'existe pas : l'identité d'un appareil appairé ne
        // meurt que d'une révocation (contrat § 22). Le mot « expirée » était
        // faux sur ce chemin, et c'est le mot que l'app aurait affiché.
        assert_eq!(body["error"], "authentification requise", "{body}");
        let rendu = body.to_string().to_lowercase();
        assert!(!rendu.contains("session"), "{rendu}");
        assert!(!rendu.contains("expir"), "{rendu}");

        // Le cookie continue de passer, porteur ou non.
        let session = h.login().await;
        let mut req = with_cookie(empty_request("GET", "/api/probes"), &session);
        req.headers_mut().insert(
            header::AUTHORIZATION,
            "Bearer jeton-inconnu".parse().unwrap(),
        );
        let (status, body, _) = h.call(req).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    #[tokio::test]
    async fn the_site_guard_checks_existence_for_an_admin_too() {
        // 🔴 **Écrit avec un compte `Scope::All`, et c'est tout son intérêt**
        // (contrat § 17.3). Un test écrit avec un compte restreint reste vert
        // quoi qu'on casse sur ce chemin : la portée d'un compte restreint
        // vérifie l'existence par effet de bord, celle d'un admin ne vérifie
        // rien. C'est l'angle mort qui a laissé le même défaut revenir trois
        // fois côté sondes.
        //
        // Les trois routes de site appelaient `site_in_scope` **à la main**.
        // Une garde qu'il faut penser à écrire finit par être oubliée : elle
        // est devenue une couche.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let appels: Vec<(&str, Request<Body>)> = vec![
            (
                "PATCH /api/sites/{id}",
                json_request("PATCH", "/api/sites/site-inconnu", json!({ "name": "Autre" })),
            ),
            (
                "DELETE /api/sites/{id}",
                empty_request("DELETE", "/api/sites/site-inconnu"),
            ),
            (
                "POST /api/sites/{id}/archive",
                json_request(
                    "POST",
                    "/api/sites/site-inconnu/archive",
                    json!({ "archived": true }),
                ),
            ),
        ];

        for (nom, req) in appels {
            let (status, body, _) = h.call(with_cookie(req, &session)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{nom} : {body}");
            // ⚠️ Le CORPS autant que le code : un 404 qui nommerait la route,
            // la table ou l'identifiant apprendrait ce que le 404 protège.
            assert_eq!(body["error"], "site inconnu", "{nom} : {body}");
        }
    }

    #[tokio::test]
    async fn an_unknown_site_answers_before_the_body_is_read() {
        // ⚠️ Effet de bord assumé du passage en couche (contrat § 17) : sur un
        // site inconnu, un corps invalide rend **404 et non 400**. La garde
        // parle avant que le corps soit lu, et c'est le bon ordre — « ce site
        // n'existe pas » est la plus vraie des deux réponses, corriger son
        // JSON n'aurait rien changé.
        //
        // C'est aussi ce qui prouve que la vérification a bien QUITTÉ le
        // handler : tant qu'elle y était, axum refusait le corps le premier.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request("PATCH", "/api/sites/site-inconnu", json!({ "pas_le_bon_champ": 1 })),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"], "site inconnu", "{body}");
    }

    #[tokio::test]
    async fn a_site_that_exists_is_still_renamed_archived_and_deleted() {
        // ⚠️ La moitié qui prouve que le refus ci-dessus vise l'inexistence et
        // non la route. Sans elle, une garde trop large rendrait « site
        // inconnu » sur tout le parc sans qu'aucun test ne bronche.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let site = h.state.db.create_site("Durand").unwrap();
        let id = site.site_id;

        let (status, body, _) = h
            .call(with_cookie(
                json_request("PATCH", &format!("/api/sites/{id}"), json!({ "name": "Durand SA" })),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "renommage : {body}");
        assert_eq!(body["name"], "Durand SA", "{body}");

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "POST",
                    &format!("/api/sites/{id}/archive"),
                    json!({ "archived": true }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "archivage : {body}");

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("DELETE", &format!("/api/sites/{id}")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "retrait : {body}");
    }

    #[tokio::test]
    async fn subscribing_to_a_site_that_does_not_exist_writes_nothing() {
        // 🔴 Le même angle mort, côté SITE, et lui aussi visible d'un compte
        // `Scope::All` seulement : la portée d'un admin autorise tout, aucune
        // existence n'était vérifiée, et l'abonnement partait en base pour un
        // site qui n'existe pas. La ligne s'affiche ensuite comme un réglage
        // d'alerte actif sur un parc imaginaire.
        //
        // ⚠️ `notify_subscriptions.scope_id` ne porte AUCUNE clé étrangère :
        // rien en aval ne rattrape la faute, contrairement aux routes de site
        // où le `UPDATE` ne touchant aucune ligne finissait par la révéler.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/subscriptions",
                    json!({ "scope": "site", "scope_id": "site-qui-nexiste-pas", "enabled": true }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"], "cible inconnue", "{body}");
        assert!(
            h.state.db.list_notify_subscriptions().unwrap().is_empty(),
            "rien ne doit avoir été écrit"
        );
    }

    #[tokio::test]
    async fn subscribing_to_a_site_that_exists_still_works() {
        // ⚠️ La moitié qui prouve que le refus ci-dessus vise l'inexistence et
        // non la route : le même appel sur un site réel doit écrire.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let site = h.state.db.create_site("Durand").unwrap();

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/notifications/subscriptions",
                    json!({ "scope": "site", "scope_id": site.site_id, "enabled": true }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            h.state.db.list_notify_subscriptions().unwrap().len(),
            1,
            "l'abonnement doit être écrit"
        );
    }

    #[tokio::test]
    async fn a_network_label_comes_back_with_the_history() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;
        let site_id = h.state.db.get_probe(&probe_id).unwrap().site_id;
        heartbeat_from(
            &h,
            &probe_id,
            &token,
            "203.0.113.10",
            serde_json::json!({ "public_ip": "88.120.0.1", "gateway": "10.6.8.1" }),
        )
        .await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/networks/label",
                    serde_json::json!({
                        "site_id": site_id,
                        "public_ip": "88.120.0.1",
                        "gateway": "10.6.8.1",
                        "label": "Maison"
                    }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let (_, sla, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/sla?range=24h")),
                &session,
            ))
            .await;
        assert_eq!(sla["public_ip_history"][0]["label"], "Maison");
    }

    #[tokio::test]
    async fn an_operator_out_of_scope_cannot_name_a_network() {
        // ⚠️ Le libellé remonte dans le RAPPORT SLA du site. Un opérateur
        // restreint qui pourrait le poser écrirait un texte arbitraire dans un
        // document remis au client d'un autre.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, token) = h.enroll(&session, "Durand", "Paris").await;
        let site_id = h.state.db.get_probe(&probe_id).unwrap().site_id;
        heartbeat_from(
            &h,
            &probe_id,
            &token,
            "203.0.113.10",
            serde_json::json!({ "public_ip": "88.120.0.1", "gateway": "10.6.8.1" }),
        )
        .await;

        // Un autre site, seul dans la portée de l'opérateur.
        let autre = h.state.db.create_site("Martin").unwrap();
        h.user("olivier", Role::Operator).await;
        h.state
            .db
            .set_user_sites("olivier", &[autre.site_id.clone()])
            .unwrap();
        let restreint = h.login_as("olivier").await;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/networks/label",
                    serde_json::json!({
                        "site_id": site_id,
                        "public_ip": "88.120.0.1",
                        "gateway": "10.6.8.1",
                        "label": "Chez le concurrent"
                    }),
                ),
                &restreint,
            ))
            .await;
        // ⚠️ 404 et non 403 : « ça existe mais pas pour vous » révèle déjà
        // l'existence du site d'un autre client.
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

        // Et RIEN n'a été écrit : un refus qui laisse une trace en base n'est
        // pas un refus.
        let (_, sla, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/sla?range=24h")),
                &session,
            ))
            .await;
        assert!(
            sla["public_ip_history"][0]["label"].is_null(),
            "aucun libellé ne doit avoir été posé : {sla}"
        );
    }

    #[tokio::test]
    async fn two_sites_behind_the_same_public_ip_keep_their_own_label() {
        // ⚠️ Sous CGNAT deux clients partagent l'adresse publique, et
        // `192.168.1.1` est la passerelle d'une box sur deux. Sans dimension de
        // site, « Maison de Durand » s'affichait dans le rapport de Martin.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (durand, t_durand) = h.enroll(&session, "Durand", "Paris").await;
        let (martin, t_martin) = h.enroll(&session, "Martin", "Lyon").await;
        let identite =
            serde_json::json!({ "public_ip": "88.120.0.1", "gateway": "192.168.1.1" });
        heartbeat_from(&h, &durand, &t_durand, "203.0.113.10", identite.clone()).await;
        heartbeat_from(&h, &martin, &t_martin, "203.0.113.10", identite).await;

        for (probe_id, label) in [(&durand, "Maison de Durand"), (&martin, "Bureau de Martin")] {
            let site_id = h.state.db.get_probe(probe_id).unwrap().site_id;
            let (status, body, _) = h
                .call(with_cookie(
                    json_request(
                        "PUT",
                        "/api/networks/label",
                        serde_json::json!({
                            "site_id": site_id,
                            "public_ip": "88.120.0.1",
                            "gateway": "192.168.1.1",
                            "label": label
                        }),
                    ),
                    &session,
                ))
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }

        for (probe_id, attendu) in [(&durand, "Maison de Durand"), (&martin, "Bureau de Martin")] {
            let (_, sla, _) = h
                .call(with_cookie(
                    empty_request("GET", &format!("/api/probes/{probe_id}/sla?range=24h")),
                    &session,
                ))
                .await;
            assert_eq!(
                sla["public_ip_history"][0]["label"], attendu,
                "chaque site garde le sien : {sla}"
            );
        }
    }

    #[tokio::test]
    async fn a_label_longer_than_the_bound_is_refused() {
        // Rien ne bornait la longueur : un libellé se retrouve dans tous les
        // rapports, un roman n'y a pas sa place.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;
        let site_id = h.state.db.get_probe(&probe_id).unwrap().site_id;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PUT",
                    "/api/networks/label",
                    serde_json::json!({
                        "site_id": site_id,
                        "public_ip": "88.120.0.1",
                        "gateway": "10.6.8.1",
                        "label": "M".repeat(129)
                    }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    #[test]
    fn alive_and_latency_are_recollected_into_one_sample() {
        // ⚠️ Ils arrivent sur DEUX lignes distinctes du CSV d'Influx. Les
        // traiter séparément donnerait deux fois plus de relevés que la sonde
        // n'en a pris, et fausserait toute disponibilité calculée dessus.
        let csv = ",result,table,_time,_value,_field,ip\n\
                   ,,0,2026-08-29T09:00:00Z,true,alive,10.0.0.1\n\
                   ,,0,2026-08-29T09:00:00Z,12.5,latency_ms,10.0.0.1\n";
        let targets = samples_from_flux_csv(csv, (0, 3_600));
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
        let report = sla_from_flux_csv(csv, (0, 3_600));
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].stats.uptime_pct, Some(0.0));
        assert_eq!(report[0].stats.failed_samples, 2);
    }

    /// Réponse RÉELLE d'influxd 2.9.1 à la requête de `uptime_flux`, sur une
    /// cible ayant 10 relevés déterminés (8 vivants) **et 90 relevés
    /// indéterminés** — des mesures antérieures au champ `alive`, qui ne
    /// portent qu'une latence. Capturée, pas écrite à la main.
    const UPTIME_CSV: &str = "\
,result,table,ip,_value
,samples,0,1.1.1.1,10

,result,table,ip,_value
,uptime,0,1.1.1.1,0.8
";

    #[test]
    fn the_aggregated_path_excludes_undetermined_samples_by_construction() {
        // 🔴 Le point que tu m'as demandé de ne pas croire sur parole. Les 90
        // relevés sans `alive` ne sont PAS dans le dénominateur : le filtre
        // `_field == "alive"` ne les sélectionne pas, ils n'existent tout
        // simplement pas pour `count()`. Vérifié sur un vrai influxd — 10, pas
        // 100 — et pas déduit du fait que le calcul « a l'air » juste.
        let targets = uptime_from_flux_csv(UPTIME_CSV);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0]["ip"], "1.1.1.1");
        assert_eq!(targets[0]["samples"], 10);
        assert_eq!(targets[0]["uptime_pct"], 80.0);
    }

    #[test]
    fn the_aggregated_rate_agrees_with_compute_sla_on_the_same_data() {
        // ⚠️ **Une seule définition de la disponibilité**, tenue SOUS TEST :
        // deux pourcentages différents sur le même écran seraient pires que
        // pas de pourcentage. Si l'un des deux chemins dérive un jour, c'est
        // ici que ça casse.
        //
        // Mêmes données des deux côtés : 8 vivants, 2 morts, 90 indéterminés.
        let mut samples: Vec<lanprobe_core::sla::PingSample> = Vec::new();
        for i in 0..10 {
            samples.push(lanprobe_core::sla::PingSample {
                alive: Some(i >= 2),
                latency_ms: None,
            });
        }
        for _ in 0..90 {
            samples.push(lanprobe_core::sla::PingSample { alive: None, latency_ms: Some(12) });
        }
        let brut = lanprobe_core::sla::compute_sla("1.1.1.1", &samples);
        let agrege = uptime_from_flux_csv(UPTIME_CSV);

        assert_eq!(brut.uptime_pct, Some(80.0));
        assert_eq!(agrege[0]["uptime_pct"], 80.0);
        assert_eq!(
            brut.total_samples as i64,
            agrege[0]["samples"].as_i64().unwrap(),
            "le dénominateur doit être le même des deux côtés"
        );
    }

    #[test]
    fn the_window_edges_come_back_with_the_rate() {
        // 🔴 Ce que l'écran ne pouvait pas dire : « 99,5 % » sans jamais
        // annoncer que la période n'a été mesurée qu'en partie. Les deux
        // bornes suffisent à en énoncer un MINORANT sans supposer de cadence,
        // et elles ne coûtent qu'une ligne de plus par cible — ce chemin
        // existe justement pour ne pas relire des millions de points.
        let csv = "\
,result,table,ip,_value
,samples,0,1.1.1.1,600

,result,table,ip,_value
,uptime,0,1.1.1.1,0.995

,result,table,_time,_value,_field,ip
,first,0,2026-08-29T09:00:00Z,1,alive,1.1.1.1

,result,table,_time,_value,_field,ip
,last,0,2026-08-29T09:29:59Z,1,alive,1.1.1.1
";
        let targets = uptime_from_flux_csv(csv);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0]["first_at"], 1_787_994_000);
        assert_eq!(targets[0]["last_at"], 1_787_995_799);
    }

    /// Réponse RÉELLE d'un influxd 2.9.1 jetable à la requête de `uptime_flux`,
    /// sur quatre cibles construites pour les quatre cas qui comptent :
    ///
    /// - `10.0.8.1` : 1 600 relevés touchant les DEUX bords, avec un trou de
    ///   20 001 s au milieu — la capture de Benjamin, celle que les seules
    ///   bornes ne voyaient pas ;
    /// - `10.0.8.2` : cadence régulière de 20 s, sans trou ;
    /// - `10.0.8.3` : DEUX `host` pour une même IP, écrits dans le désordre —
    ///   c'est elle qui a fait apparaître le besoin de `sort` ;
    /// - `10.0.8.4` : UN seul relevé, donc aucun rendu `gap`.
    ///
    /// Capturée, pas écrite à la main.
    const UPTIME_REEL_CSV: &str = "\
,result,table,ip,_value
,samples,0,10.0.8.1,1600
,samples,1,10.0.8.2,1080
,samples,2,10.0.8.3,600
,samples,3,10.0.8.4,1

,result,table,ip,_value
,uptime,0,10.0.8.1,0.995
,uptime,1,10.0.8.2,1
,uptime,2,10.0.8.3,1
,uptime,3,10.0.8.4,1

,result,table,_start,_stop,_time,_value,_field,_measurement,host,ip,probe_id,site_id
,first,0,2026-08-29T10:40:00Z,2026-08-29T16:40:00Z,2026-08-29T10:40:00Z,0,alive,ping_latency,mac,10.0.8.1,sonde-1,s1
,first,2,2026-08-29T10:40:00Z,2026-08-29T16:40:00Z,2026-08-29T10:40:00Z,1,alive,ping_latency,b,10.0.8.3,sonde-1,s1
,first,3,2026-08-29T10:40:00Z,2026-08-29T16:40:00Z,2026-08-29T10:40:05Z,1,alive,ping_latency,a,10.0.8.4,sonde-1,s1

,result,table,_start,_stop,_time,_value,_field,_measurement,host,ip,probe_id,site_id
,last,0,2026-08-29T10:40:00Z,2026-08-29T16:40:00Z,2026-08-29T16:39:59Z,1,alive,ping_latency,mac,10.0.8.1,sonde-1,s1
,last,2,2026-08-29T10:40:00Z,2026-08-29T16:40:00Z,2026-08-29T10:49:59Z,1,alive,ping_latency,a,10.0.8.3,sonde-1,s1
,last,3,2026-08-29T10:40:00Z,2026-08-29T16:40:00Z,2026-08-29T10:40:05Z,1,alive,ping_latency,a,10.0.8.4,sonde-1,s1

,result,table,_start,_stop,_time,_value,_field,_measurement,host,ip,probe_id,site_id,elapsed
,gap,0,2026-08-29T10:40:00Z,2026-08-29T16:40:00Z,2026-08-29T16:18:20Z,1,alive,ping_latency,mac,10.0.8.1,sonde-1,s1,20001
,gap,1,2026-08-29T10:40:00Z,2026-08-29T16:40:00Z,2026-08-29T10:40:20Z,1,alive,ping_latency,mac,10.0.8.2,sonde-1,s1,20
,gap,2,2026-08-29T10:40:00Z,2026-08-29T16:40:00Z,2026-08-29T10:40:01Z,1,alive,ping_latency,a,10.0.8.3,sonde-1,s1,1
";

    #[test]
    fn the_largest_interior_hole_comes_back_with_the_rate() {
        // 🔴 Le cas que les seules bornes laissaient passer : les relevés
        // touchent les deux bords, la carte se taisait, et 93 % de la période
        // n'avait aucun relevé. C'est `elapsed` qui le dit.
        let cibles = uptime_from_flux_csv(UPTIME_REEL_CSV);
        let de = |ip: &str| {
            cibles.iter().find(|c| c["ip"] == ip).unwrap_or_else(|| panic!("{ip} absente")).clone()
        };
        assert_eq!(de("10.0.8.1")["max_gap_secs"], 20_001);
        assert_eq!(de("10.0.8.2")["max_gap_secs"], 20);
    }

    #[test]
    fn the_hole_is_read_in_the_elapsed_column_and_not_in_the_value() {
        // ⚠️ `elapsed` AJOUTE sa colonne et laisse `_value` intact — celui du
        // relevé, donc `alive`. Le lire là rendrait 0 ou 1, c'est-à-dire une
        // cadence d'une seconde annoncée sur n'importe quelles données.
        let cibles = uptime_from_flux_csv(UPTIME_REEL_CSV);
        let une = cibles.iter().find(|c| c["ip"] == "10.0.8.3").unwrap();
        assert_eq!(une["max_gap_secs"], 1, "la colonne `elapsed`, pas `_value` qui vaut 1");
        assert_eq!(une["samples"], 600, "et le reste du recollage tient");
    }

    #[test]
    fn a_single_sample_has_no_hole_at_all() {
        // ⚠️ Constaté sur un influxd réel : `elapsed` ne rend AUCUNE ligne sous
        // deux relevés. `null` donc, et l'écran ne dira rien — un point isolé ne
        // dit pas quelle durée il représente.
        let cibles = uptime_from_flux_csv(UPTIME_REEL_CSV);
        let seule = cibles.iter().find(|c| c["ip"] == "10.0.8.4").unwrap();
        assert!(seule["max_gap_secs"].is_null(), "{seule:?}");
        assert_eq!(seule["samples"], 1);
    }

    #[test]
    fn the_query_merges_the_series_of_one_target_in_time_order() {
        // 🔴 Trouvé sur un influxd 2.9.1 réel, pas déduit : sur une cible
        // portant deux `host`, `group` CONCATÈNE les séries. Sans `sort`,
        // `first` rendait le premier point de la seconde série, `last` le
        // dernier de la première, et `elapsed` 2 s là où les relevés sont à la
        // seconde. Un `first` trop tardif gonfle le trou annoncé — le seul sens
        // qui casse le minorant.
        let flux = uptime_flux("lanprobe", "sonde-1", "start: -24h");
        assert!(flux.contains("|> sort(columns: [\"_time\"])"), "{flux}");
        // Et l'ordre compte : le tri doit précéder les rendus qui en dépendent.
        let tri = flux.find("sort(columns:").expect("tri absent");
        for rendu in ["|> first()", "|> last()", "|> elapsed("] {
            assert!(flux.find(rendu).expect(rendu) > tri, "{rendu} avant le tri — {flux}");
        }
    }

    #[test]
    fn a_hub_answer_without_edges_carries_null_and_not_a_covered_window() {
        // ⚠️ Absence d'information n'est pas absence de trou. `null`, et
        // l'écran ne dira rien plutôt que d'affirmer une période couverte.
        let targets = uptime_from_flux_csv(UPTIME_CSV);
        assert!(targets[0]["first_at"].is_null(), "{targets:?}");
        assert!(targets[0]["last_at"].is_null(), "{targets:?}");
        assert!(targets[0]["max_gap_secs"].is_null(), "{targets:?}");
    }

    #[test]
    fn a_target_without_a_single_determinate_sample_has_no_rate() {
        // ⚠️ Ni `0 %` — qui se lirait comme une panne totale — ni cellule
        // vide. `null`, et l'écran dira « indéterminé ».
        let csv = ",result,table,ip,_value\n,samples,0,10.0.0.9,0\n";
        let targets = uptime_from_flux_csv(csv);
        assert_eq!(targets[0]["samples"], 0);
        assert!(targets[0]["uptime_pct"].is_null(), "{targets:?}");
    }

    #[test]
    fn an_empty_influx_answer_is_an_empty_list_not_an_error() {
        assert!(uptime_from_flux_csv("").is_empty());
    }

    #[test]
    fn the_uptime_query_is_accepted_by_the_hardened_double() {
        // La doublure durcie sait désormais refuser un Flux invalide : cette
        // requête-ci doit passer, et c'est enfin une vérification qui vaut
        // quelque chose.
        let flux = uptime_flux("lanprobe", "sonde-1", "start: -24h");
        assert_eq!(crate::influx::testing::flux_refusal(&flux), None, "{flux}");
        assert!(flux.contains("r[\"_field\"] == \"alive\""), "{flux}");
        assert!(flux.contains("|> mean()"), "{flux}");
        assert!(flux.contains("|> count()"), "{flux}");
        // ⚠️ Deux SÉLECTEURS, donc une ligne chacun par cible : la borne de
        // couverture ne fait pas retomber cette route dans le volume que
        // `/sla` assume et qu'elle refuse.
        assert!(flux.contains("|> first()"), "{flux}");
        assert!(flux.contains("|> last()"), "{flux}");
        // ⚠️ `max(column: \"elapsed\")` et non `max()` : la valeur cherchée
        // n'est pas dans `_value`. Vérifié sur un influxd 2.9.1 réel.
        assert!(flux.contains("|> elapsed(unit: 1s) |> max(column: \"elapsed\")"), "{flux}");
    }

    #[test]
    fn the_coverage_cell_is_never_a_number_a_spreadsheet_could_average() {
        // ⚠️ Même règle que la cellule « indetermine » et pour la même raison :
        // « 87.4 » dans une colonne se retrouverait dans une moyenne, et ferait
        // naître un chiffre que personne n'a calculé. Le mot, jamais le nombre.
        use lanprobe_core::sla::compute_coverage;

        let complete = compute_coverage(&(0..600).collect::<Vec<i64>>(), 0, 599);
        assert_eq!(coverage_cell(&complete), "complete");

        // Sonde enrôlée en cours de fenêtre : le bord compte.
        let partielle = compute_coverage(&(500..600).collect::<Vec<i64>>(), 0, 599);
        let cellule = coverage_cell(&partielle);
        assert!(cellule.contains('%'), "{cellule}");
        assert!(
            cellule.parse::<f64>().is_err(),
            "la cellule ne doit PAS être un nombre : {cellule}"
        );

        assert_eq!(coverage_cell(&compute_coverage(&[], 200, 100)), "indetermine");
    }

    #[test]
    fn a_latency_without_a_verdict_is_undetermined_not_available() {
        // 🔴 Le repli `alive.unwrap_or(latency_ms.is_some())` faisait d'une
        // latence écrite un verdict de disponibilité. Une mesure ancienne,
        // antérieure au champ `alive`, ressortait donc à 100 % — un chiffre
        // qu'on n'avait pas, dans un document remis au client.
        //
        // ⚠️ Et l'inverse est tout aussi faux : rendre 0 % affirmerait une
        // panne totale. Il n'y a pas de pourcentage à donner, et le type le
        // dit.
        let csv = ",result,table,_time,_value,_field,ip\n\
                   ,,0,2026-08-29T09:00:00Z,12,latency_ms,10.0.0.7\n\
                   ,,0,2026-08-29T09:00:01Z,14,latency_ms,10.0.0.7\n";
        let report = sla_from_flux_csv(csv, (0, 3_600));
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].stats.uptime_pct, None, "aucun verdict : aucun pourcentage");
        assert_eq!(report[0].stats.total_samples, 0);
        assert_eq!(report[0].stats.undetermined_samples, 2);
        // Les latences, elles, sont de vraies mesures : elles restent.
        assert_eq!(report[0].stats.min_latency_ms, Some(12));
    }

    #[test]
    fn sla_is_computed_per_target_not_merged() {
        // Deux cibles fusionnées donneraient une disponibilité moyenne
        // qu'aucune des deux n'a jamais eue.
        let csv = ",result,table,_time,_value,_field,ip\n\
                   ,,0,2026-08-29T09:00:00Z,true,alive,10.0.0.1\n\
                   ,,0,2026-08-29T09:00:00Z,12,latency_ms,10.0.0.1\n\
                   ,,0,2026-08-29T09:00:00Z,false,alive,8.8.8.8\n";
        let report = sla_from_flux_csv(csv, (0, 3_600));
        assert_eq!(report.len(), 2);
        assert_eq!(report[0].stats.ip, "10.0.0.1");
        assert_eq!(report[0].stats.uptime_pct, Some(100.0));
        assert_eq!(report[1].stats.ip, "8.8.8.8");
        assert_eq!(report[1].stats.uptime_pct, Some(0.0));
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
    async fn the_sla_export_honours_explicit_dates_like_every_other_window() {
        // 🔴 Le défaut corrigé en b05b049 sur `/metrics`, resté sur l'export :
        // `probe_sla_csv` lisait `query.range` SEUL. Demander « du 1er au 31 »
        // rendait les dernières 24 h, sous un fichier qui annonce la période
        // demandée dans sa colonne `fenetre` — un classeur remis à un client
        // avec la mauvaise période et le bon en-tête.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let (status, _, _) = h
            .call(with_cookie(
                empty_request(
                    "GET",
                    &format!(
                        "/api/probes/{probe_id}/sla.csv?range=-24h\
                         &start=1788000000&stop=1788600000"
                    ),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);

        let (_, _, flux) = h
            ._fake
            .calls()
            .into_iter()
            .find(|(m, p, _)| m == "POST" && p.starts_with("/api/v2/query"))
            .expect("la requête Flux doit partir");
        // ⚠️ Les bornes explicites priment sur `range`, comme partout ailleurs.
        assert!(flux.contains("start: 1788000000, stop: 1788600000"), "{flux}");
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

    #[tokio::test]
    async fn la_langue_de_linterface_suit_le_compte_pas_le_navigateur() {
        // 🔴 Le défaut : le sélecteur n'écrivait que dans `localStorage`.
        // Changer de navigateur ramenait l'anglais, et deux personnes sur le
        // même hub le voyaient chacune dans sa langue. La base fait foi.
        let h = Harness::with_admin().await;
        let session = h.login().await;

        // Rien de réglé : `null`, et le navigateur décide — comme avant.
        let (status, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/me"), &session))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["lang"].is_null(), "{body}");

        let (status, body, _) = h
            .call(with_cookie(
                json_request("POST", "/api/me/lang", json!({ "lang": "es" })),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");

        // Une session TOUTE NEUVE — donc un autre navigateur — la retrouve.
        let autre = h.login().await;
        let (_, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/me"), &autre))
            .await;
        assert_eq!(body["lang"], "es", "{body}");

        // Et on peut revenir au défaut : c'est un réglage, pas un aller simple.
        let (status, _, _) = h
            .call(with_cookie(
                json_request("POST", "/api/me/lang", json!({ "lang": null })),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        let (_, body, _) = h
            .call(with_cookie(empty_request("GET", "/api/me"), &session))
            .await;
        assert!(body["lang"].is_null(), "{body}");
    }

    #[tokio::test]
    async fn une_langue_que_le_produit_ne_parle_pas_est_refusee_a_lecriture() {
        // ⚠️ Strict à l'ÉCRITURE, tolérant à la lecture : un réglage accepté
        // puis jamais appliqué est pire qu'un refus — il se découvre sur le
        // document remis au client.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let id = h.state.db.create_site("Durand").unwrap().site_id;

        for (uri, corps) in [
            ("/api/me/lang", json!({ "lang": "de" })),
            (
                "/api/me/lang",
                json!({ "lang": "es-419" }),
            ),
        ] {
            let (status, body, _) = h
                .call(with_cookie(json_request("POST", uri, corps), &session))
                .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert!(
                body["error"].as_str().unwrap_or_default().contains("langue"),
                "{body}"
            );
        }

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PATCH",
                    &format!("/api/sites/{id}"),
                    json!({ "name": "Durand", "report_locale": "de" }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        // Et rien n'a été écrit : un refus partiel serait pire que tout.
        assert_eq!(h.state.db.get_site(&id).unwrap().report_locale, None);
    }

    #[tokio::test]
    async fn la_langue_des_rapports_se_regle_la_ou_on_nomme_le_site() {
        // 🔴 Le classeur part chez UN client : sa langue est une propriété du
        // client. Elle se règle donc dans les réglages du site, avec son nom —
        // pas dans « Mon compte ».
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let id = h.state.db.create_site("Durand").unwrap().site_id;

        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PATCH",
                    &format!("/api/sites/{id}"),
                    json!({ "name": "Durand SA", "report_locale": "es" }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["name"], "Durand SA", "{body}");
        assert_eq!(body["report_locale"], "es", "{body}");

        // ⚠️ Un renommage seul ne doit PAS effacer la langue : le champ absent
        // veut dire « on n'y touche pas », pas « remets le défaut ».
        let (status, body, _) = h
            .call(with_cookie(
                json_request("PATCH", &format!("/api/sites/{id}"), json!({ "name": "Durand" })),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["report_locale"], "es", "{body}");

        // `null` explicite, lui, rend le site au défaut du hub.
        let (status, body, _) = h
            .call(with_cookie(
                json_request(
                    "PATCH",
                    &format!("/api/sites/{id}"),
                    json!({ "name": "Durand", "report_locale": null }),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["report_locale"].is_null(), "{body}");
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

    #[tokio::test]
    async fn the_uptime_route_needs_a_session_and_knows_its_probes() {
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let (status, _, _) = h
            .call(empty_request("GET", &format!("/api/probes/{probe_id}/uptime?range=-24h")))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // ⚠️ 404 et non 500 sur une sonde inconnue, comme les autres routes.
        let (status, _, _) = h
            .call(with_cookie(
                empty_request("GET", "/api/probes/pas-une-sonde/uptime?range=-24h"),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // ⚠️ Et une fenêtre illisible reste un 400, pas un 502.
        let (status, _, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/uptime?range=nawak")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/uptime?range=-24h")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.get("targets").is_some(), "{body}");
    }

    #[tokio::test]
    async fn a_window_wider_than_the_graph_is_aggregated_and_says_so() {
        // ⚠️ Sans `aggregated_every` dans la réponse, l'interface ne peut pas
        // être honnête : elle tracerait une moyenne de trois heures sous une
        // légende qui promet la mesure à la seconde.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request(
                    "GET",
                    &format!("/api/probes/{probe_id}/metrics?measurement=ping_latency&range=-7d"),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["aggregated_every"], "15m", "{body}");

        let (_, _, flux) = h
            ._fake
            .calls()
            .into_iter()
            .find(|(m, p, _)| m == "POST" && p.starts_with("/api/v2/query"))
            .expect("la requête Flux doit partir");
        assert!(flux.contains("aggregateWindow(every: 15m, fn: mean"), "{flux}");
        assert!(flux.contains("createEmpty: false"), "{flux}");
    }

    #[tokio::test]
    async fn a_malformed_range_is_400_not_502() {
        // ⚠️ 502 AFFIRME qu'InfluxDB est en panne. La vérité est « votre
        // fenêtre est illisible » : c'est la saisie du client qui est en
        // cause, pas l'amont. Même faute d'honnêteté que le 500 sur une sonde
        // inconnue — elle envoie chercher une panne qui n'existe pas.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/metrics?range=nawak")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

        // Et la saisie anodine, elle, passe — en visant bien le PASSÉ.
        let (status, _, _) = h
            .call(with_cookie(
                empty_request("GET", &format!("/api/probes/{probe_id}/metrics?range=24h")),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        let (_, _, flux) = h
            ._fake
            .calls()
            .into_iter()
            .find(|(m, p, _)| m == "POST" && p.starts_with("/api/v2/query"))
            .expect("la requête Flux doit partir");
        assert!(flux.contains("start: -24h"), "{flux}");
    }

    #[tokio::test]
    async fn a_short_window_stays_raw_and_announces_no_step() {
        // Dix minutes prises il y a trois mois : les points existent, ils
        // tiennent dans le graphe, on les rend tels quels.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request(
                    "GET",
                    &format!(
                        "/api/probes/{probe_id}/metrics?measurement=ping_latency\
                         &start=1780000000&stop=1780000600"
                    ),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.get("aggregated_every").is_none(), "{body}");

        let (_, _, flux) = h
            ._fake
            .calls()
            .into_iter()
            .find(|(m, p, _)| m == "POST" && p.starts_with("/api/v2/query"))
            .expect("la requête Flux doit partir");
        assert!(!flux.contains("aggregateWindow"), "{flux}");
        assert!(flux.contains("start: 1780000000, stop: 1780000600"), "{flux}");
    }

    #[tokio::test]
    async fn the_browser_decides_the_width_of_its_own_graph() {
        // `max_points` = largeur du graphe en pixels. Un écran étroit demande
        // moins de points, un mur d'écrans davantage — et le hub borne, parce
        // que la valeur vient du navigateur.
        let h = Harness::with_admin().await;
        let session = h.login().await;
        let (probe_id, _) = h.enroll(&session, "Durand", "Paris").await;

        let (status, body, _) = h
            .call(with_cookie(
                empty_request(
                    "GET",
                    &format!(
                        "/api/probes/{probe_id}/metrics?measurement=ping_latency\
                         &range=-1h&max_points=120"
                    ),
                ),
                &session,
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        // 3 600 s pour 120 points : 30 s par point.
        assert_eq!(body["aggregated_every"], "30s", "{body}");
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
