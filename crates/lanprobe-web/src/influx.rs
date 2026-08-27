//! Provisionnement InfluxDB 2 et frappe des jetons d'écriture par sonde.
//!
//! Le jeton **opérateur** (`INFLUX_TOKEN`) reste dans ce module : il ne part
//! ni vers une sonde, ni vers le navigateur. Une sonde reçoit un jeton frappé
//! pour elle, en **écriture seule sur le seul bucket** — compromise, elle ne
//! peut ni lire les mesures des autres, ni les effacer.
//!
//! Un seul bucket pour tous les sites : le site est un *tag*. Un bucket porte
//! une politique de rétention, pas une frontière de sécurité — le contrôle
//! d'accès est fait par le hub, en SQL.

use std::sync::Mutex;

use serde::Serialize;

/// Coordonnées Influx renvoyées à une sonde à l'enrôlement. `token` est le
/// jeton frappé pour elle, jamais celui de l'opérateur.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeInfluxSettings {
    pub url: String,
    pub org: String,
    pub bucket: String,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct InfluxConfig {
    /// URL vue par le hub (nom de service Docker, typiquement).
    pub url: String,
    /// URL que les sondes doivent joindre depuis l'extérieur du réseau
    /// Docker. Distincte de `url` : `http://influxdb:8086` ne veut rien dire
    /// pour une sonde sur le LAN d'un client.
    pub public_url: String,
    pub org: String,
    pub bucket: String,
    pub operator_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfluxIds {
    pub org_id: String,
    pub bucket_id: String,
}

pub struct Influx {
    cfg: InfluxConfig,
    http: reqwest::Client,
    /// `None` tant que le provisionnement n'a pas abouti. Le hub démarre sans
    /// Influx et réessaie : refuser de démarrer parce que la base de séries
    /// n'est pas encore prête rendrait le compose ingérable.
    ids: Mutex<Option<InfluxIds>>,
}

impl Influx {
    pub fn new(cfg: InfluxConfig) -> Self {
        Self {
            cfg,
            http: reqwest::Client::new(),
            ids: Mutex::new(None),
        }
    }

    pub fn ids(&self) -> Option<InfluxIds> {
        self.ids.lock().ok().and_then(|g| g.clone())
    }

    pub fn is_ready(&self) -> bool {
        self.ids().is_some()
    }

    /// S'assure que l'org et le bucket existent, et mémorise leurs
    /// identifiants. Idempotent : ne recrée rien de ce qui est déjà là.
    pub async fn ensure_provisioned(&self) -> Result<InfluxIds, String> {
        let org_id = match self.find_org().await? {
            Some(id) => id,
            None => self.create_org().await?,
        };
        let bucket_id = match self.find_bucket(&org_id).await? {
            Some(id) => id,
            None => self.create_bucket(&org_id).await?,
        };
        let ids = InfluxIds { org_id, bucket_id };
        if let Ok(mut guard) = self.ids.lock() {
            *guard = Some(ids.clone());
        }
        Ok(ids)
    }

    /// Frappe un jeton d'écriture restreint au seul bucket.
    pub async fn mint_write_token(&self, description: &str) -> Result<String, String> {
        let ids = self
            .ids()
            .ok_or_else(|| "InfluxDB pas encore provisionné".to_string())?;
        let body = serde_json::json!({
            "orgID": ids.org_id,
            "description": description,
            "permissions": [{
                "action": "write",
                "resource": {
                    "type": "buckets",
                    "id": ids.bucket_id,
                    "orgID": ids.org_id,
                }
            }],
        });
        let value = self
            .send_json(reqwest::Method::POST, "/api/v2/authorizations", None, Some(body))
            .await?;
        value["token"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "réponse d'autorisation Influx sans jeton".to_string())
    }

    /// Proxifie une requête Flux. Le navigateur ne parle jamais directement à
    /// Influx : le jeton de lecture reste côté serveur.
    pub async fn query_flux(&self, flux: &str) -> Result<String, String> {
        let url = format!(
            "{}/api/v2/query?org={}",
            self.cfg.url.trim_end_matches('/'),
            urlencode(&self.cfg.org)
        );
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Token {}", self.cfg.operator_token))
            .header("Content-Type", "application/vnd.flux")
            .header("Accept", "application/csv")
            .body(flux.to_string())
            .send()
            .await
            .map_err(|e| format!("InfluxDB injoignable : {e}"))?;
        let status = response.status();
        let text = response.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("InfluxDB a répondu {status}"));
        }
        Ok(text)
    }

    pub fn probe_settings(&self, token: &str) -> ProbeInfluxSettings {
        ProbeInfluxSettings {
            url: self.cfg.public_url.clone(),
            org: self.cfg.org.clone(),
            bucket: self.cfg.bucket.clone(),
            token: token.to_string(),
        }
    }

    async fn find_org(&self) -> Result<Option<String>, String> {
        let value = self
            .send_json(
                reqwest::Method::GET,
                &format!("/api/v2/orgs?org={}", urlencode(&self.cfg.org)),
                Some(&[404]),
                None,
            )
            .await?;
        Ok(first_id(&value["orgs"]))
    }

    async fn create_org(&self) -> Result<String, String> {
        let body = serde_json::json!({ "name": self.cfg.org });
        let value = self
            .send_json(reqwest::Method::POST, "/api/v2/orgs", None, Some(body))
            .await?;
        value["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "création d'org Influx sans identifiant".to_string())
    }

    async fn find_bucket(&self, org_id: &str) -> Result<Option<String>, String> {
        let value = self
            .send_json(
                reqwest::Method::GET,
                &format!(
                    "/api/v2/buckets?orgID={}&name={}",
                    urlencode(org_id),
                    urlencode(&self.cfg.bucket)
                ),
                // Influx répond 404 sur un bucket absent selon les versions —
                // c'est une absence, pas une panne.
                Some(&[404]),
                None,
            )
            .await?;
        Ok(first_id(&value["buckets"]))
    }

    async fn create_bucket(&self, org_id: &str) -> Result<String, String> {
        let body = serde_json::json!({
            "orgID": org_id,
            "name": self.cfg.bucket,
            // Rétention infinie par défaut : c'est à l'opérateur de décider
            // combien de temps il garde ses mesures, pas au hub de trancher
            // à sa place en effaçant.
            "retentionRules": [],
        });
        let value = self
            .send_json(reqwest::Method::POST, "/api/v2/buckets", None, Some(body))
            .await?;
        value["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "création de bucket Influx sans identifiant".to_string())
    }

    /// `tolerated` liste les codes non-2xx à traiter comme une réponse vide
    /// plutôt que comme une erreur.
    async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        tolerated: Option<&[u16]>,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}{}", self.cfg.url.trim_end_matches('/'), path);
        let mut req = self
            .http
            .request(method, &url)
            .header("Authorization", format!("Token {}", self.cfg.operator_token));
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req
            .send()
            .await
            .map_err(|e| format!("InfluxDB injoignable : {e}"))?;
        let status = response.status();
        if !status.is_success() {
            if tolerated.is_some_and(|codes| codes.contains(&status.as_u16())) {
                return Ok(serde_json::Value::Null);
            }
            // Le corps d'erreur d'Influx peut contenir des détails de config ;
            // on ne remonte que le code, les logs n'ont pas à porter ça.
            return Err(format!("InfluxDB a répondu {status} sur {path}"));
        }
        response
            .json::<serde_json::Value>()
            .await
            .map_err(|e| format!("réponse Influx illisible : {e}"))
    }
}

/// Boucle de provisionnement, lancée en tâche de fond au démarrage. Le hub
/// sert déjà ses routes pendant ce temps : refuser de démarrer parce que la
/// base de séries n'est pas encore prête rendrait le compose ingérable — on
/// ne contrôle pas l'ordre de disponibilité des conteneurs.
///
/// Repli exponentiel plafonné, pour ne pas marteler un Influx qui redémarre.
pub async fn run_provisioning(
    influx: std::sync::Arc<Influx>,
    base_delay: std::time::Duration,
    max_delay: std::time::Duration,
) {
    let mut delay = base_delay;
    loop {
        match influx.ensure_provisioned().await {
            Ok(ids) => {
                tracing::info!(
                    "InfluxDB provisionné — org {} / bucket {}",
                    ids.org_id,
                    ids.bucket_id
                );
                return;
            }
            Err(e) => {
                // Pas de secret dans ce message : `send_json` ne remonte que
                // le code HTTP et l'endpoint.
                tracing::warn!("InfluxDB indisponible ({e}) — nouvelle tentative dans {delay:?}");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(max_delay);
            }
        }
    }
}

fn first_id(list: &serde_json::Value) -> Option<String> {
    list.as_array()?
        .first()?
        .get("id")?
        .as_str()
        .map(str::to_string)
}

/// Échappement minimal pour les quelques valeurs qu'on met en query string
/// (noms d'org, de bucket, identifiants). Éviter une dépendance de plus pour
/// trois caractères.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Faux InfluxDB local, partagé par les tests de ce crate. Local et
/// jetable : aucun test ne touche un Influx réel.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Ce qu'un faux Influx a vu passer. Les tests assertent sur les requêtes
    /// réellement émises — pas sur un mock qui se contenterait de dire oui.
    #[derive(Default)]
    pub(crate) struct Seen {
        calls: Vec<(String, String, String)>, // méthode, chemin+query, corps
        org_exists: bool,
        bucket_exists: bool,
        /// Nombre de requêtes restantes à faire échouer — simule un Influx
        /// encore en train de démarrer.
        fail_next: u32,
    }

    pub(crate) struct FakeInflux {
        pub(crate) base_url: String,
        seen: Arc<Mutex<Seen>>,
    }

    impl FakeInflux {
        pub(crate) async fn start(org_exists: bool, bucket_exists: bool) -> Self {
            Self::start_flaky(org_exists, bucket_exists, 0).await
        }

        pub(crate) async fn start_flaky(org_exists: bool, bucket_exists: bool, fail_next: u32) -> Self {
            use axum::{
                body::Bytes,
                extract::State,
                http::{Method, Uri},
                routing::any,
                Router,
            };

            let seen = Arc::new(Mutex::new(Seen {
                org_exists,
                bucket_exists,
                fail_next,
                ..Default::default()
            }));

            async fn handle(
                State(seen): State<Arc<Mutex<Seen>>>,
                method: Method,
                uri: Uri,
                headers: axum::http::HeaderMap,
                body: Bytes,
            ) -> axum::response::Response {
                use axum::response::IntoResponse;
                let path_and_query = uri.path_and_query().map(|p| p.to_string()).unwrap_or_default();
                let body = String::from_utf8_lossy(&body).to_string();
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                let mut guard = seen.lock().unwrap();
                guard.calls.push((method.to_string(), path_and_query.clone(), body));

                if guard.fail_next > 0 {
                    guard.fail_next -= 1;
                    return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "").into_response();
                }

                // Le jeton opérateur doit voyager en `Token <…>` — c'est le
                // schéma d'Influx 2, `Bearer` n'est pas accepté partout.
                if !auth.starts_with("Token ") {
                    return (axum::http::StatusCode::UNAUTHORIZED, "").into_response();
                }

                let json = |s: &str| {
                    (
                        axum::http::StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        s.to_string(),
                    )
                        .into_response()
                };

                match (method.as_str(), uri.path()) {
                    ("GET", "/api/v2/orgs") if guard.org_exists => {
                        json(r#"{"orgs":[{"id":"org-1","name":"lanprobe"}]}"#)
                    }
                    ("GET", "/api/v2/orgs") => json(r#"{"orgs":[]}"#),
                    ("POST", "/api/v2/orgs") => {
                        guard.org_exists = true;
                        json(r#"{"id":"org-1","name":"lanprobe"}"#)
                    }
                    ("GET", "/api/v2/buckets") if guard.bucket_exists => {
                        json(r#"{"buckets":[{"id":"bucket-1","name":"lanprobe","orgID":"org-1"}]}"#)
                    }
                    ("GET", "/api/v2/buckets") => json(r#"{"buckets":[]}"#),
                    ("POST", "/api/v2/buckets") => {
                        guard.bucket_exists = true;
                        json(r#"{"id":"bucket-1","name":"lanprobe","orgID":"org-1"}"#)
                    }
                    ("POST", "/api/v2/authorizations") => {
                        json(r#"{"id":"auth-1","token":"jeton-de-sonde-frappe"}"#)
                    }
                    ("POST", "/api/v2/query") => (
                        axum::http::StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "text/csv")],
                        "result,table,_time,_value\n,0,2026-01-01T00:00:00Z,42\n".to_string(),
                    )
                        .into_response(),
                    _ => (axum::http::StatusCode::NOT_FOUND, "").into_response(),
                }
            }

            let app = Router::new().fallback(any(handle)).with_state(seen.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });

            Self {
                base_url: format!("http://{addr}"),
                seen,
            }
        }

        pub(crate) fn calls(&self) -> Vec<(String, String, String)> {
            self.seen.lock().unwrap().calls.clone()
        }
    }

    pub(crate) fn config(url: &str) -> InfluxConfig {
        InfluxConfig {
            url: url.to_string(),
            public_url: "https://hub.example.org:8086".into(),
            org: "lanprobe".into(),
            bucket: "lanprobe".into(),
            operator_token: "jeton-operateur-secret".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[tokio::test]
    async fn provisioning_creates_the_org_and_the_bucket_when_missing() {
        let fake = FakeInflux::start(false, false).await;
        let influx = Influx::new(config(&fake.base_url));

        let ids = influx.ensure_provisioned().await.unwrap();
        assert_eq!(ids.org_id, "org-1");
        assert_eq!(ids.bucket_id, "bucket-1");

        let posted: Vec<_> = fake
            .calls()
            .into_iter()
            .filter(|(m, _, _)| m == "POST")
            .map(|(_, p, _)| p)
            .collect();
        assert!(posted.iter().any(|p| p.starts_with("/api/v2/orgs")), "{posted:?}");
        assert!(posted.iter().any(|p| p.starts_with("/api/v2/buckets")), "{posted:?}");
    }

    #[tokio::test]
    async fn provisioning_creates_nothing_when_everything_exists() {
        let fake = FakeInflux::start(true, true).await;
        let influx = Influx::new(config(&fake.base_url));

        influx.ensure_provisioned().await.unwrap();

        let posted: Vec<_> = fake.calls().into_iter().filter(|(m, _, _)| m == "POST").collect();
        assert!(posted.is_empty(), "rien ne doit être recréé : {posted:?}");
    }

    #[tokio::test]
    async fn minting_yields_a_token_restricted_to_writing_the_bucket() {
        let fake = FakeInflux::start(true, true).await;
        let influx = Influx::new(config(&fake.base_url));
        influx.ensure_provisioned().await.unwrap();

        let token = influx.mint_write_token("sonde Paris").await.unwrap();
        assert_eq!(token, "jeton-de-sonde-frappe");
        assert_ne!(
            token, "jeton-operateur-secret",
            "le jeton opérateur ne doit jamais être renvoyé à une sonde"
        );

        let (_, _, body) = fake
            .calls()
            .into_iter()
            .find(|(m, p, _)| m == "POST" && p.starts_with("/api/v2/authorizations"))
            .expect("une autorisation doit être demandée");
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();

        let permissions = body["permissions"].as_array().unwrap();
        assert_eq!(permissions.len(), 1, "une seule permission : {body}");
        assert_eq!(permissions[0]["action"], "write", "écriture seule : {body}");
        assert_eq!(permissions[0]["resource"]["type"], "buckets");
        assert_eq!(
            permissions[0]["resource"]["id"], "bucket-1",
            "la permission doit viser le seul bucket, pas l'org entière : {body}"
        );
    }

    #[tokio::test]
    async fn minting_before_provisioning_fails_instead_of_guessing() {
        let fake = FakeInflux::start(true, true).await;
        let influx = Influx::new(config(&fake.base_url));

        let err = influx.mint_write_token("sonde Paris").await.unwrap_err();
        assert!(err.contains("provision"), "message attendu explicite : {err}");
    }

    #[tokio::test]
    async fn an_unreachable_influx_reports_an_error_without_panicking() {
        // Port fermé : c'est l'état normal des premières secondes d'un
        // `docker compose up`, le hub ne doit pas s'écrouler dessus.
        let influx = Influx::new(config("http://127.0.0.1:1"));
        assert!(influx.ensure_provisioned().await.is_err());
        assert!(influx.ids().is_none(), "aucun identifiant ne doit être mémorisé");
    }

    #[tokio::test]
    async fn probe_settings_expose_the_public_url_never_the_operator_token() {
        let fake = FakeInflux::start(true, true).await;
        let influx = Influx::new(config(&fake.base_url));

        let settings = influx.probe_settings("jeton-de-sonde");
        assert_eq!(settings.url, "https://hub.example.org:8086");
        assert_eq!(settings.org, "lanprobe");
        assert_eq!(settings.bucket, "lanprobe");
        assert_eq!(settings.token, "jeton-de-sonde");

        let rendered = serde_json::to_string(&settings).unwrap();
        assert!(
            !rendered.contains("jeton-operateur-secret"),
            "le jeton opérateur ne doit jamais être sérialisé : {rendered}"
        );
    }

    #[tokio::test]
    async fn provisioning_retries_until_influx_answers() {
        // Les premières secondes d'un `docker compose up` : Influx répond 503.
        // Le hub doit démarrer quand même et réessayer.
        let fake = FakeInflux::start_flaky(true, true, 2).await;
        let influx = std::sync::Arc::new(Influx::new(config(&fake.base_url)));
        assert!(!influx.is_ready());

        run_provisioning(
            influx.clone(),
            std::time::Duration::from_millis(10),
            std::time::Duration::from_millis(50),
        )
        .await;

        assert!(influx.is_ready(), "le provisionnement doit finir par aboutir");
        assert!(
            fake.calls().len() >= 3,
            "au moins deux échecs puis un succès : {:?}",
            fake.calls().len()
        );
    }

    #[tokio::test]
    async fn flux_queries_are_proxied_and_their_result_returned() {
        let fake = FakeInflux::start(true, true).await;
        let influx = Influx::new(config(&fake.base_url));
        influx.ensure_provisioned().await.unwrap();

        let csv = influx.query_flux("from(bucket:\"lanprobe\")").await.unwrap();
        assert!(csv.contains("_value"), "le CSV Influx doit être relayé tel quel : {csv}");

        let (_, path, body) = fake
            .calls()
            .into_iter()
            .find(|(m, p, _)| m == "POST" && p.starts_with("/api/v2/query"))
            .expect("la requête Flux doit être relayée");
        assert!(path.contains("org=lanprobe"), "{path}");
        assert!(body.contains("from(bucket:"), "{body}");
    }
}
