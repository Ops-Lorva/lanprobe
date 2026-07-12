//! Routes HTTP (setup, login, logout) + WebSocket bridge pour les events.
//!
//! L'API suit une règle simple : toutes les routes `/api/*` (sauf `/api/auth/*`
//! et `/api/setup`) exigent un cookie `lanprobe_session` valide. Côté
//! navigateur, le shim Tauri fait `credentials: 'same-origin'` donc le cookie
//! part automatiquement avec chaque invoke.

use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use cookie::{Cookie, SameSite};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

use crate::{assets, auth::AuthStore, routes, state::AppState};

const SESSION_COOKIE: &str = "lanprobe_session";

pub fn build_router(state: AppState) -> Router {
    let public = Router::new()
        .route("/api/status", get(status))
        .route("/api/setup", post(setup))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout));

    let protected = Router::new()
        .route("/api/invoke/{cmd}", post(routes::invoke))
        .route("/ws", any(ws_upgrade))
        .layer(middleware::from_fn_with_state(state.clone(), auth_guard));

    Router::new()
        .merge(public)
        .merge(protected)
        .fallback(get(|uri| async move { assets::serve(uri).await }))
        .with_state(state)
}

#[derive(Serialize)]
struct Status {
    needs_setup: bool,
    authenticated: bool,
}

async fn status(State(state): State<AppState>, headers: HeaderMap) -> Json<Status> {
    let token = extract_cookie(&headers);
    let authenticated = token.map(|t| state.auth.validate(&t).is_some()).unwrap_or(false);
    Json(Status {
        needs_setup: state.auth.needs_setup(),
        authenticated,
    })
}

#[derive(Deserialize)]
struct SetupBody {
    username: String,
    password: String,
    /// Token de setup généré au démarrage (fichier `setup-token` du config
    /// dir, visible aussi via `journalctl`). Peut aussi arriver via l'en-tête
    /// `X-Setup-Token` — le champ body a la priorité s'il est renseigné.
    #[serde(default)]
    token: String,
}

async fn setup(State(state): State<AppState>, headers: HeaderMap, Json(body): Json<SetupBody>) -> Response {
    if body.username.trim().is_empty() || body.password.len() < 8 {
        return (StatusCode::BAD_REQUEST, "username required, password ≥ 8 chars").into_response();
    }
    let token = if !body.token.is_empty() {
        body.token.clone()
    } else {
        headers
            .get("x-setup-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    match state.auth.initial_setup_with_token(&token, body.username.trim(), &body.password) {
        Ok(()) => (StatusCode::OK, json!({ "ok": true }).to_string()).into_response(),
        Err(e) => (StatusCode::CONFLICT, e).into_response(),
    }
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

async fn login(State(state): State<AppState>, Json(body): Json<LoginBody>) -> Response {
    match state.auth.login(&body.username, &body.password) {
        Ok(token) => {
            let cookie = Cookie::build((SESSION_COOKIE, token))
                .http_only(true)
                .same_site(SameSite::Strict)
                .path("/")
                .secure(true)
                .build()
                .to_string();
            ([(header::SET_COOKIE, cookie)], json!({ "ok": true }).to_string()).into_response()
        }
        Err(e) => (StatusCode::UNAUTHORIZED, e).into_response(),
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = extract_cookie(&headers) {
        state.auth.logout(&token);
    }
    let cookie = Cookie::build((SESSION_COOKIE, ""))
        .http_only(true)
        .same_site(SameSite::Strict)
        .path("/")
        .secure(true)
        .max_age(cookie::time::Duration::seconds(0))
        .build()
        .to_string();
    ([(header::SET_COOKIE, cookie)], json!({ "ok": true }).to_string()).into_response()
}

async fn auth_guard(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    if state.auth.needs_setup() {
        return (StatusCode::SERVICE_UNAVAILABLE, json!({ "needs_setup": true }).to_string()).into_response();
    }
    let Some(token) = extract_cookie(&headers) else {
        return (StatusCode::UNAUTHORIZED, "missing session").into_response();
    };
    if state.auth.validate(&token).is_none() {
        return (StatusCode::UNAUTHORIZED, "invalid session").into_response();
    }
    next.run(req).await
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| ws_handle(socket, state))
}

async fn ws_handle(mut socket: WebSocket, state: AppState) {
    let rx = state.events.subscribe();
    let mut stream = BroadcastStream::new(rx);

    while let Some(ev) = stream.next().await {
        let Ok(ev) = ev else { continue };
        let Ok(txt) = serde_json::to_string(&ev) else { continue };
        if socket.send(Message::Text(txt.into())).await.is_err() {
            break;
        }
    }
}

fn extract_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        if let Ok(c) = Cookie::parse(pair.trim()) {
            if c.name() == SESSION_COOKIE {
                return Some(c.value().to_string());
            }
        }
    }
    None
}

pub fn auth_store_path(config_dir: &std::path::Path) -> std::path::PathBuf {
    crate::auth::default_users_path(config_dir)
}

pub fn load_auth(config_dir: &std::path::Path) -> std::io::Result<Arc<AuthStore>> {
    AuthStore::load(auth_store_path(config_dir)).map(Arc::new)
}
