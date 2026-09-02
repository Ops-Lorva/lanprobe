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
    /// Empreinte du certificat auto-signé d'Influx, à épingler côté sonde.
    /// Absente si Influx est servi en HTTP clair : il n'y a alors rien à
    /// épingler, et un champ vide laisserait croire le contraire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_fingerprint_sha256: Option<String>,
}

/// Nom du fichier du volume qui porte le jeton opérateur. Il est généré au
/// premier démarrage par l'entrypoint : ce n'est pas un réglage utilisateur,
/// il n'a donc rien à faire ni dans la base ni dans l'interface.
pub const OPERATOR_TOKEN_FILE: &str = "influx-operator-token";

/// Variable d'environnement acceptée en repli du fichier, pour un lancement
/// à la main hors conteneur.
pub const OPERATOR_TOKEN_ENV: &str = "INFLUX_TOKEN";

/// Variable acceptée comme niveau de repli de l'URL annoncée. Elle n'est pas
/// amorcée en base : la garder distincte permet de dire, au diagnostic, si la
/// valeur vient de l'interface ou de l'environnement.
pub const ADVERTISE_URL_ENV: &str = "INFLUX_ADVERTISE_URL";

/// D'où vient l'URL annoncée à une sonde. Journalisé à chaque enrôlement :
/// une sonde qui s'enrôle puis n'écrit jamais rien est la panne la plus
/// pénible à diagnostiquer, et la première question est « quelle URL a-t-elle
/// reçue, et pourquoi celle-là ».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvertiseSource {
    /// Réglée dans l'interface — c'est elle qui gagne.
    Settings,
    /// `INFLUX_ADVERTISE_URL`, pour qui préfère préconfigurer.
    Env,
    /// Déduite de l'adresse publique du hub : même hôte, port d'Influx.
    /// C'est le chemin nominal une fois le hub configuré.
    HubPublicUrl,
    /// L'hôte par lequel la sonde vient de joindre le hub.
    HostHeader,
    /// Dernier recours : l'URL interne. Elle ne marchera probablement pas pour
    /// la sonde, mais c'est journalisé — mieux vaut une URL fausse et tracée
    /// qu'un champ vide qui fait échouer l'app à l'autre bout.
    InternalUrl,
}

impl AdvertiseSource {
    /// Valeur telle qu'elle part sur le fil, pour que le diagnostic dise
    /// **d'où vient** l'URL, pas seulement qu'elle est mauvaise.
    pub fn as_str(&self) -> &'static str {
        match self {
            AdvertiseSource::Settings => "settings",
            AdvertiseSource::Env => "env",
            AdvertiseSource::HubPublicUrl => "hub_public_url",
            AdvertiseSource::HostHeader => "host_header",
            AdvertiseSource::InternalUrl => "internal_url",
        }
    }
}

/// Jeton frappé : sa valeur, rendue **une seule fois**, et l'identifiant
/// d'autorisation, seul élément que le hub conserve.
#[derive(Debug, Clone)]
pub struct MintedToken {
    pub auth_id: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InfluxHealth {
    pub status: String,
    pub version: Option<String>,
    pub bucket_bytes: Option<i64>,
    pub series_count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfluxIds {
    pub org_id: String,
    pub bucket_id: String,
}

pub struct Influx {
    settings: crate::settings::Settings,
    /// Jeton opérateur, lu au démarrage dans le volume. Il ne va ni en base,
    /// ni dans une réponse d'API, ni dans un log.
    operator_token: String,
    http: reqwest::Client,
    /// `None` tant que le provisionnement n'a pas abouti. Le hub démarre sans
    /// Influx et réessaie : refuser de démarrer parce que la base de séries
    /// n'est pas encore prête rendrait le compose ingérable.
    ids: Mutex<Option<InfluxIds>>,
    /// Empreinte du certificat d'Influx, calculée à la demande puis mise en
    /// cache : un handshake par enrôlement serait du gaspillage.
    tls_fingerprint: Mutex<Option<String>>,
}

impl Influx {
    pub fn new(settings: crate::settings::Settings, operator_token: String) -> Self {
        // Influx est servi en HTTPS avec un certificat auto-signé, sur la
        // machine même. Aucune autorité ne peut le valider : le hub accepte
        // le certificat pour ce saut local, et transmet son empreinte aux
        // sondes pour qu'elles l'épinglent — c'est là que se joue
        // l'authentification du serveur, pas ici.
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_default();
        Self {
            settings,
            operator_token,
            http,
            ids: Mutex::new(None),
            tls_fingerprint: Mutex::new(None),
        }
    }

    /// Résout l'URL d'Influx à annoncer à une sonde, et dit d'où elle vient.
    ///
    /// Un conteneur ne peut pas connaître son port publié : avec `-p
    /// 9086:8086` le processus ne voit que `8086`. Aucune détection
    /// automatique n'est possible, d'où ces niveaux — dont les deux premiers
    /// sont réglables depuis l'interface, sans redémarrage.
    ///
    /// **Le réglage normal est l'adresse publique du hub**, pas celle
    /// d'Influx : l'utilisateur dit par où on le joint, le hub en déduit où
    /// les sondes écrivent. Deux URL voisines à distinguer, c'est une sonde
    /// enrôlée avec la mauvaise et une demi-heure à comprendre pourquoi. La
    /// surcharge Influx ne sert qu'au cas rare où il est exposé ailleurs.
    ///
    /// **Chemin unique** : l'enrôlement et le test de joignabilité passent
    /// tous deux par ici. Deux chemins finiraient par diverger, et le test
    /// validerait alors une URL que la sonde ne reçoit pas.
    pub fn resolve_advertise_url(&self, host_header: Option<&str>) -> (String, AdvertiseSource) {
        if let Some(url) = self.settings.stored_advertise_url() {
            return (
                url.trim().trim_end_matches('/').to_string(),
                AdvertiseSource::Settings,
            );
        }
        if let Ok(url) = std::env::var(ADVERTISE_URL_ENV) {
            let url = url.trim();
            if !url.is_empty() {
                return (url.trim_end_matches('/').to_string(), AdvertiseSource::Env);
            }
        }
        let internal = self.settings.influx_url();
        let (scheme, _, port) = split_url(&internal);
        // Déduction depuis l'adresse publique du hub : même hôte, port
        // d'Influx. C'est le chemin nominal une fois le hub configuré.
        if let Some(public) = self.settings.hub_public_url() {
            let host = split_url(&public).1;
            if !host.is_empty() {
                let port = port.map(|p| format!(":{p}")).unwrap_or_default();
                return (
                    format!("{scheme}://{host}{port}"),
                    AdvertiseSource::HubPublicUrl,
                );
            }
        }
        if let Some(host) = host_header.map(host_without_port).filter(|h| !h.is_empty()) {
            let port = port.map(|p| format!(":{p}")).unwrap_or_default();
            return (format!("{scheme}://{host}{port}"), AdvertiseSource::HostHeader);
        }
        (
            internal.trim_end_matches('/').to_string(),
            AdvertiseSource::InternalUrl,
        )
    }

    /// Tente de joindre `/health` à l'URL annoncée. Un échec est un résultat
    /// de test, pas une panne du hub : la couche HTTP répond `200` avec
    /// `reachable: false`. Timeout court — on diagnostique, on n'attend pas.
    pub async fn check_reachable(&self, url: &str) -> bool {
        let target = format!("{}/health", url.trim_end_matches('/'));
        match self
            .http
            .get(&target)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    /// Applique la rétention réglée au bucket. `0` = illimitée.
    ///
    /// ⚠️ Influx **efface** les points au-delà de la fenêtre. La confirmation
    /// explicite est exigée en amont, par `Settings::put` — ce module ne fait
    /// qu'exécuter une décision déjà prise.
    pub async fn apply_retention(&self, days: i64) -> Result<(), String> {
        let ids = self
            .ids()
            .ok_or_else(|| "InfluxDB pas encore provisionné".to_string())?;
        let rules = if days > 0 {
            serde_json::json!([{ "type": "expire", "everySeconds": days * 86_400 }])
        } else {
            serde_json::json!([])
        };
        self.send_json(
            reqwest::Method::PATCH,
            &format!("/api/v2/buckets/{}", ids.bucket_id),
            None,
            Some(serde_json::json!({ "retentionRules": rules })),
        )
        .await?;
        Ok(())
    }

    /// Oublie ce qui a été déduit d'anciens réglages. Appelé quand l'org, le
    /// bucket ou l'URL changent : garder l'ancien `bucket_id` frapperait des
    /// jetons pour un bucket que l'opérateur vient d'abandonner.
    /// De quoi appeler `influx backup` / `influx restore`. Le jeton
    /// opérateur ne sort pas d'ici autrement : il ne va ni en base, ni dans
    /// une réponse d'API, ni dans un log.
    ///
    /// `skip_verify` est vrai par construction : Influx est servi en HTTPS
    /// avec un certificat auto-signé sur la machine même, et la CLI refuserait
    /// sa propre instance sans cela. C'est le même saut local que le client
    /// HTTP de ce module accepte déjà.
    pub fn backup_target(&self, cli: std::path::PathBuf) -> crate::backup::InfluxTarget {
        crate::backup::InfluxTarget {
            cli,
            host: self.settings.influx_url(),
            org: self.settings.influx_org(),
            bucket: self.settings.influx_bucket(),
            token: self.operator_token.clone(),
            skip_verify: true,
        }
    }

    /// Faux quand le jeton opérateur n'a pas pu être lu au démarrage : sans
    /// lui `influx backup` échouerait, et il vaut mieux le dire à l'avance.
    pub fn has_operator_token(&self) -> bool {
        !self.operator_token.is_empty()
    }

    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.ids.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.tls_fingerprint.lock() {
            *guard = None;
        }
    }

    pub fn tls_fingerprint(&self) -> Option<String> {
        self.tls_fingerprint.lock().ok().and_then(|g| g.clone())
    }

    /// Ouvre un handshake TLS vers Influx pour capturer l'empreinte SHA-256
    /// du certificat qu'il présente. Rend une chaîne vide si Influx est servi
    /// en HTTP clair : il n'y a alors rien à épingler.
    pub async fn ensure_tls_fingerprint(&self) -> Result<String, String> {
        if let Some(cached) = self.tls_fingerprint() {
            return Ok(cached);
        }
        let (scheme, host, port) = split_url(&self.settings.influx_url());
        if scheme != "https" {
            return Ok(String::new());
        }
        let fingerprint = capture_certificate_fingerprint(&host, port.unwrap_or(443)).await?;
        if let Ok(mut guard) = self.tls_fingerprint.lock() {
            *guard = Some(fingerprint.clone());
        }
        Ok(fingerprint)
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
    pub async fn mint_write_token(&self, description: &str) -> Result<MintedToken, String> {
        self.mint_token("write", description).await
    }

    /// Frappe un jeton **en lecture seule** sur le bucket, à coller dans
    /// Grafana. Un jeton d'écriture distribué pour de la lecture serait un
    /// joli trou : la portée est fixée ici, pas côté appelant.
    pub async fn mint_read_token(&self, description: &str) -> Result<MintedToken, String> {
        self.mint_token("read", description).await
    }

    async fn mint_token(&self, action: &str, description: &str) -> Result<MintedToken, String> {
        let ids = self
            .ids()
            .ok_or_else(|| "InfluxDB pas encore provisionné".to_string())?;
        let body = serde_json::json!({
            "orgID": ids.org_id,
            "description": description,
            "permissions": [{
                "action": action,
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
        let token = value["token"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "réponse d'autorisation Influx sans jeton".to_string())?;
        Ok(MintedToken {
            auth_id: value["id"].as_str().unwrap_or_default().to_string(),
            token,
        })
    }

    /// Détruit une autorisation. **Ne supprime aucune mesure** : une
    /// autorisation est une clé, pas une donnée.
    pub async fn delete_authorization(&self, auth_id: &str) -> Result<(), String> {
        if auth_id.is_empty() {
            return Ok(());
        }
        self.send_json(
            reqwest::Method::DELETE,
            &format!("/api/v2/authorizations/{}", urlencode(auth_id)),
            // Déjà absente : le but est atteint, ce n'est pas une erreur.
            Some(&[404]),
            None,
        )
        .await?;
        Ok(())
    }

    /// État d'Influx tel qu'on l'affiche dans les réglages. Aucun jeton dans
    /// cette structure : l'utilisateur doit pouvoir aller voir ses données
    /// sans jamais avoir à administrer Influx.
    pub async fn health(&self) -> InfluxHealth {
        let url = format!("{}/health", self.settings.influx_url().trim_end_matches('/'));
        let value = self
            .http
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .ok();
        let body: Option<serde_json::Value> = match value {
            Some(response) if response.status().is_success() => response.json().await.ok(),
            _ => None,
        };
        InfluxHealth {
            status: body
                .as_ref()
                .and_then(|b| b["status"].as_str())
                .unwrap_or("unreachable")
                .to_string(),
            version: body
                .as_ref()
                .and_then(|b| b["version"].as_str())
                .map(str::to_string),
            // Taille occupée et cardinalité demandent des requêtes au bucket
            // `_monitoring`, que toutes les installations n'exposent pas. On
            // préfère `null` à un chiffre inventé : une jauge fausse est pire
            // qu'une jauge absente.
            bucket_bytes: None,
            series_count: self.series_count().await,
        }
    }

    async fn series_count(&self) -> Option<i64> {
        let bucket = self.settings.influx_bucket();
        let flux = format!(
            "import \"influxdata/influxdb/schema\"\n\
             schema.measurements(bucket: \"{}\") |> count()",
            bucket.replace('"', "")
        );
        let csv = self.query_flux(&flux).await.ok()?;
        csv.lines()
            .filter_map(|line| line.rsplit(',').next())
            .filter_map(|cell| cell.trim().parse::<i64>().ok())
            .next_back()
    }

    /// Proxifie une requête Flux. Le navigateur ne parle jamais directement à
    /// Influx : le jeton de lecture reste côté serveur.
    pub async fn query_flux(&self, flux: &str) -> Result<String, String> {
        let url = format!(
            "{}/api/v2/query?org={}",
            self.settings.influx_url().trim_end_matches('/'),
            urlencode(&self.settings.influx_org())
        );
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Token {}", self.operator_token))
            .header("Content-Type", "application/vnd.flux")
            .header("Accept", "application/csv")
            .body(flux.to_string())
            .send()
            .await
            .map_err(|e| format!("InfluxDB injoignable : {e}"))?;
        let status = response.status();
        let text = response.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            // ⚠️ Le CORPS, pas seulement le code. Influx explique toujours son
            // refus (« invalid import path… », « cannot query an empty
            // range ») et on jetait l'explication : l'opérateur — et le test
            // en échec — n'avaient plus qu'un « 400 Bad Request » muet.
            let detail = text.trim();
            return Err(if detail.is_empty() {
                format!("InfluxDB a répondu {status}")
            } else {
                format!("InfluxDB a répondu {status} : {detail}")
            });
        }
        Ok(text)
    }

    /// Coordonnées à renvoyer à une sonde. `host_header` est celui de la
    /// requête d'enrôlement — c'est l'adresse par laquelle la sonde vient de
    /// joindre le hub, donc le meilleur pari sur l'adresse d'Influx.
    pub fn probe_settings(&self, token: &str, host_header: Option<&str>) -> ProbeInfluxSettings {
        let (url, source) = self.resolve_advertise_url(host_header);
        tracing::info!("URL Influx annoncée à la sonde : {url} (source : {})", source.as_str());
        ProbeInfluxSettings {
            url,
            org: self.settings.influx_org(),
            bucket: self.settings.influx_bucket(),
            token: token.to_string(),
            tls_fingerprint_sha256: self.tls_fingerprint().filter(|f| !f.is_empty()),
        }
    }

    async fn find_org(&self) -> Result<Option<String>, String> {
        let value = self
            .send_json(
                reqwest::Method::GET,
                &format!("/api/v2/orgs?org={}", urlencode(&self.settings.influx_org())),
                Some(&[404]),
                None,
            )
            .await?;
        Ok(first_id(&value["orgs"]))
    }

    async fn create_org(&self) -> Result<String, String> {
        let body = serde_json::json!({ "name": self.settings.influx_org() });
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
                    urlencode(&self.settings.influx_bucket())
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
            "name": self.settings.influx_bucket(),
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
    /// Relaie un lot de points en line protocol vers Influx.
    ///
    /// Les sondes n'écrivent plus dans Influx directement : elles envoient au
    /// hub, qui relaie. Une seule porte, une seule authentification, un seul
    /// certificat — et Influx n'a plus besoin d'être joignable depuis le
    /// réseau des sondes.
    ///
    /// Le corps est **transmis tel quel**, sans être accumulé en mémoire côté
    /// hub au-delà de la requête : le hub est un relais, pas un tampon. S'il
    /// bufferisait, une pointe d'écriture le ferait gonfler, et un
    /// redémarrage perdrait des mesures que la sonde croit livrées.
    pub async fn write_line_protocol(&self, body: String) -> Result<(), String> {
        let url = format!(
            "{}/api/v2/write?org={}&bucket={}&precision=ns",
            self.settings.influx_url().trim_end_matches('/'),
            urlencoding(&self.settings.influx_org()),
            urlencoding(&self.settings.influx_bucket()),
        );
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Token {}", self.operator_token))
            .timeout(std::time::Duration::from_secs(20))
            .body(body)
            .send()
            .await
            .map_err(|e| format!("InfluxDB injoignable : {e}"))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        // Le détail d'Influx est utile au diagnostic (point mal formé, bucket
        // absent) : on le remonte à la sonde, qui le journalisera.
        let detail = response.text().await.unwrap_or_default();
        Err(format!("écriture refusée par InfluxDB ({status}) : {detail}"))
    }

    async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        tolerated: Option<&[u16]>,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}{}", self.settings.influx_url().trim_end_matches('/'), path);
        let mut req = self
            .http
            .request(method, &url)
            .header("Authorization", format!("Token {}", self.operator_token));
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

/// Découpe une URL en (schéma, hôte, port). Volontairement minimal : on ne
/// traite que des URL qu'on a nous-même formées ou reçues d'une variable
/// d'environnement, pas des URL arbitraires du web.
/// Encodage minimal pour un paramètre de requête. `org` et `bucket` viennent
/// des réglages : ils peuvent contenir un espace ou un `&` sans que ce soit
/// une attaque, mais l'URL doit rester valide.
fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn split_url(url: &str) -> (String, String, Option<u16>) {
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    let host = host_without_port(authority);
    // Le port suit le dernier `:`, mais seulement s'il est hors des crochets
    // d'une adresse IPv6.
    let port_part = match authority.rfind(']') {
        Some(close) => authority[close + 1..].strip_prefix(':'),
        None => authority.rsplit_once(':').map(|(_, p)| p),
    };
    let port = port_part
        .and_then(|p| p.parse::<u16>().ok())
        .or(match scheme {
            "https" => Some(443),
            "http" => Some(80),
            _ => None,
        });
    (scheme.to_string(), host, port)
}

/// Retire le port d'une autorité HTTP, en préservant les crochets d'une
/// adresse IPv6 littérale.
fn host_without_port(authority: &str) -> String {
    let authority = authority.trim();
    if let Some(close) = authority.rfind(']') {
        return authority[..=close].to_string();
    }
    match authority.rsplit_once(':') {
        Some((host, _)) => host.to_string(),
        None => authority.to_string(),
    }
}

/// Empreinte SHA-256 d'un certificat, au format `AB:CD:…` — celui qu'affichent
/// les navigateurs et `openssl x509 -fingerprint`, donc celui qu'un opérateur
/// peut comparer à l'œil.
fn fingerprint_of(cert: &rustls::pki_types::CertificateDer<'_>) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, cert.as_ref());
    digest
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Ouvre un handshake TLS et capture le certificat présenté par le serveur.
/// On ne le *valide* pas — il est auto-signé par construction ; on l'observe
/// pour pouvoir le transmettre aux sondes qui, elles, l'épingleront.
async fn capture_certificate_fingerprint(host: &str, port: u16) -> Result<String, String> {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use std::sync::Arc;

    #[derive(Debug)]
    struct Capture {
        seen: Mutex<Option<String>>,
        provider: Arc<rustls::crypto::CryptoProvider>,
    }

    impl ServerCertVerifier for Capture {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            if let Ok(mut guard) = self.seen.lock() {
                *guard = Some(fingerprint_of(end_entity));
            }
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.provider.signature_verification_algorithms.supported_schemes()
        }
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(Capture {
        seen: Mutex::new(None),
        provider: provider.clone(),
    });
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();

    // Les crochets d'une IPv6 littérale ne font pas partie du nom.
    let bare_host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let server_name =
        ServerName::try_from(bare_host.clone()).map_err(|e| format!("hôte Influx invalide : {e}"))?;

    let stream = tokio::net::TcpStream::connect((bare_host.as_str(), port))
        .await
        .map_err(|e| format!("InfluxDB injoignable en TLS : {e}"))?;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    connector
        .connect(server_name, stream)
        .await
        .map_err(|e| format!("handshake TLS avec InfluxDB échoué : {e}"))?;

    verifier
        .seen
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .ok_or_else(|| "aucun certificat présenté par InfluxDB".to_string())
}

/// Charge le jeton opérateur : le fichier du volume d'abord — chemin normal,
/// écrit par l'entrypoint au premier démarrage — puis `INFLUX_TOKEN` en repli
/// pour un lancement à la main hors conteneur.
pub fn load_operator_token(config_dir: &std::path::Path) -> Result<String, String> {
    let path = config_dir.join(OPERATOR_TOKEN_FILE);
    if let Ok(raw) = std::fs::read_to_string(&path) {
        let token = raw.trim();
        if !token.is_empty() {
            return Ok(token.to_string());
        }
    }
    if let Ok(token) = std::env::var(OPERATOR_TOKEN_ENV) {
        let token = token.trim();
        if !token.is_empty() {
            return Ok(token.to_string());
        }
    }
    Err(format!(
        "jeton opérateur InfluxDB introuvable — attendu dans {} ou dans {OPERATOR_TOKEN_ENV}",
        path.display()
    ))
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

    /// `INFLUX_ADVERTISE_URL` est globale au processus alors que les tests
    /// Rust tournent en parallèle. Tout test qui la lit, l'écrit, ou exige son
    /// absence prend ce verrou — y compris ceux des autres modules, sans quoi
    /// la résolution de l'URL annoncée deviendrait un tirage au sort.
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ── Validation Flux de la doublure ─────────────────────────────────────
    //
    // ⚠️ **Ce n'est PAS un moteur Flux, et ça ne doit jamais le devenir.** Une
    // doublure qui simule tout devient un second logiciel à maintenir, qui
    // finit par diverger d'Influx et produit de FAUX ÉCHECS — pires que les
    // faux succès qu'on corrige, parce qu'ils font perdre confiance dans la
    // suite entière. On ne refuse donc que ce qu'un vrai influxd 2.9.1 refuse
    // de façon CERTAINE, constaté en le lui soumettant, et on laisse passer
    // tout le reste.

    /// Paquets Flux **constatés valides** sur influxd 2.9.1.
    ///
    /// ⚠️ Un import absent de cette liste est refusé, non parce qu'Influx le
    /// refuserait forcément, mais parce que personne ne l'a vérifié — et c'est
    /// exactement l'erreur qui a failli partir ce matin. Pour en ajouter un :
    /// le soumettre à un vrai influxd, constater le 200, puis l'inscrire ici.
    const KNOWN_IMPORTS: &[&str] = &[
        "types",
        "math",
        "strings",
        "date",
        "experimental",
        "array",
        "influxdata/influxdb/schema",
    ];

    /// Fonctions acceptées comme `fn:` d'un `aggregateWindow`.
    const KNOWN_AGGREGATES: &[&str] = &[
        "mean", "max", "min", "sum", "count", "first", "last", "median", "mode", "spread",
        "stddev", "quantile", "distinct", "integral", "increase", "skew",
    ];

    /// Durée Flux signée en secondes (`-24h` → −86 400). `None` si ce n'est pas
    /// une durée simple — on s'abstient alors de juger.
    fn duration_secs(token: &str) -> Option<i64> {
        let (sign, rest) = match token.strip_prefix('-') {
            Some(r) => (-1, r),
            None => (1, token),
        };
        let split = rest.find(|c: char| !c.is_ascii_digit())?;
        let (count, unit) = rest.split_at(split);
        let mult = match unit {
            "s" => 1,
            "m" => 60,
            "h" => 3_600,
            "d" => 86_400,
            "w" => 604_800,
            _ => return None,
        };
        Some(sign * count.parse::<i64>().ok()? * mult)
    }

    /// Instant d'une borne `range`, en secondes. `now` vaut 0 : une durée
    /// relative est négative, un epoch absolu est très grand — les deux ne se
    /// comparent donc jamais entre eux, et c'est voulu.
    fn bound_secs(token: &str) -> Option<(bool, i64)> {
        if let Ok(epoch) = token.parse::<i64>() {
            return Some((true, epoch));
        }
        duration_secs(token).map(|d| (false, d))
    }

    /// Contenu des parenthèses de chaque appel `name(...)`.
    fn calls<'a>(flux: &'a str, name: &str) -> Vec<&'a str> {
        let mut out = Vec::new();
        let needle = format!("{name}(");
        let mut from = 0;
        while let Some(i) = flux[from..].find(&needle) {
            let open = from + i + needle.len();
            let mut depth = 1;
            let mut end = open;
            for (j, c) in flux[open..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + j;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            out.push(&flux[open..end]);
            from = open;
        }
        out
    }

    /// Valeur d'un argument nommé, brute.
    fn named_arg<'a>(args: &'a str, name: &str) -> Option<&'a str> {
        let needle = format!("{name}:");
        let start = args.find(&needle)? + needle.len();
        let rest = &args[start..];
        let end = rest.find([',', ')']).unwrap_or(rest.len());
        Some(rest[..end].trim())
    }

    /// Ce qu'un vrai InfluxDB refuserait, ou `None` si la doublure laisse
    /// passer. Le message NOMME la cause : un test qui échoue sans dire
    /// pourquoi coûte plus cher qu'un test absent.
    pub(crate) fn flux_refusal(flux: &str) -> Option<String> {
        for line in flux.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("import ") {
                let path = rest.trim().trim_matches('"');
                if !KNOWN_IMPORTS.contains(&path) {
                    return Some(format!(
                        "invalid import path « {path} » — inconnu de la doublure. \
                         Si ce paquet existe vraiment, constatez-le sur un influxd \
                         réel puis ajoutez-le à KNOWN_IMPORTS."
                    ));
                }
            }
        }

        for args in calls(flux, "aggregateWindow") {
            if named_arg(args, "every").is_none() {
                return Some("missing required argument every (aggregateWindow)".into());
            }
            if let Some(f) = named_arg(args, "fn") {
                if !KNOWN_AGGREGATES.contains(&f) {
                    return Some(format!("undefined identifier {f} (aggregateWindow fn:)"));
                }
            }
        }

        for args in calls(flux, "range") {
            let start = named_arg(args, "start").and_then(bound_secs);
            let stop = named_arg(args, "stop").and_then(bound_secs);
            let empty = match (start, stop) {
                // Même nature de borne : elles se comparent.
                (Some((a_abs, a)), Some((b_abs, b))) if a_abs == b_abs => b <= a,
                // `stop` absent vaut « maintenant » : une durée POSITIVE vise
                // donc le futur, et la fenêtre est vide. C'est `?range=24h`.
                (Some((false, a)), None) => a > 0,
                // Bornes de natures différentes, ou illisibles : on s'abstient.
                _ => false,
            };
            if empty {
                return Some(format!("cannot query an empty range — range({args})"));
            }
        }
        None
    }

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
                guard.calls.push((method.to_string(), path_and_query.clone(), body.clone()));

                if guard.fail_next > 0 {
                    guard.fail_next -= 1;
                    return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "").into_response();
                }

                // `/health` est public côté Influx : c'est précisément ce qui
                // permet de vérifier l'URL annoncée sans distribuer de jeton.
                if uri.path() == "/health" {
                    return (axum::http::StatusCode::OK, "").into_response();
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
                    // Relais d'écriture : Influx répond 204 sans corps.
                    ("POST", "/api/v2/write") => {
                        (axum::http::StatusCode::NO_CONTENT, "").into_response()
                    }
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
                    ("DELETE", p) if p.starts_with("/api/v2/authorizations/") => {
                        json(r#"{}"#)
                    }
                    ("PATCH", p) if p.starts_with("/api/v2/buckets/") => {
                        json(r#"{"id":"bucket-1","name":"lanprobe","orgID":"org-1"}"#)
                    }
                    ("POST", "/api/v2/authorizations") => {
                        json(r#"{"id":"auth-1","token":"jeton-de-sonde-frappe"}"#)
                    }
                    // ⚠️ La doublure LIT le Flux avant de répondre. Elle
                    // répondait 200 à n'importe quoi : aucun test du dépôt ne
                    // pouvait donc attraper une requête malformée, et c'est
                    // ainsi qu'un import inexistant a failli partir au vert.
                    ("POST", "/api/v2/query") => match flux_refusal(&body) {
                        Some(cause) => (
                            axum::http::StatusCode::BAD_REQUEST,
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            serde_json::json!({ "code": "invalid", "message": cause })
                                .to_string(),
                        )
                            .into_response(),
                        None => (
                            axum::http::StatusCode::OK,
                            [(axum::http::header::CONTENT_TYPE, "text/csv")],
                            "result,table,_time,_value\n,0,2026-01-01T00:00:00Z,42\n".to_string(),
                        )
                            .into_response(),
                    },
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

    /// Serveur TLS jetable présentant un certificat auto-signé, comme
    /// l'Influx du conteneur. Rend son adresse et l'empreinte SHA-256
    /// attendue du certificat.
    pub(crate) async fn start_self_signed_tls_server() -> (std::net::SocketAddr, String) {
        use rcgen::{CertificateParams, KeyPair};

        let key_pair = KeyPair::generate().unwrap();
        let params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
        let expected = super::fingerprint_of(&cert_der);

        let key_der =
            rustls::pki_types::PrivateKeyDer::try_from(key_pair.serialize_der()).unwrap();
        // Provider explicite : `ring` et `aws-lc-rs` sont tous deux dans
        // l'arbre (via reqwest), rustls refuse alors de choisir seul.
        let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                // Le handshake suffit : le hub ne veut que le certificat.
                tokio::spawn(async move {
                    let _ = acceptor.accept(stream).await;
                });
            }
        });

        (addr, expected)
    }

    pub(crate) const OPERATOR_TOKEN: &str = "jeton-operateur-secret";

    /// Réglages neufs, en base mémoire, pointant sur `url`.
    pub(crate) fn settings_for(url: &str) -> crate::settings::Settings {
        let db = Arc::new(crate::db::Db::open_in_memory().unwrap());
        let settings = crate::settings::Settings::new(db);
        settings
            .put(crate::settings::keys::INFLUX_URL, url, false)
            .unwrap();
        settings
    }

    pub(crate) fn influx_for(url: &str) -> Influx {
        Influx::new(settings_for(url), OPERATOR_TOKEN.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;


    #[tokio::test]
    async fn provisioning_creates_the_org_and_the_bucket_when_missing() {
        let fake = FakeInflux::start(false, false).await;
        let influx = influx_for(&fake.base_url);

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
        let influx = influx_for(&fake.base_url);

        influx.ensure_provisioned().await.unwrap();

        let posted: Vec<_> = fake.calls().into_iter().filter(|(m, _, _)| m == "POST").collect();
        assert!(posted.is_empty(), "rien ne doit être recréé : {posted:?}");
    }

    #[tokio::test]
    async fn minting_yields_a_token_restricted_to_writing_the_bucket() {
        let fake = FakeInflux::start(true, true).await;
        let influx = influx_for(&fake.base_url);
        influx.ensure_provisioned().await.unwrap();

        let token = influx.mint_write_token("sonde Paris").await.unwrap();
        assert_eq!(token.token, "jeton-de-sonde-frappe");
        assert_eq!(token.auth_id, "auth-1", "l'identifiant permet de révoquer plus tard");
        assert_ne!(
            token.token, OPERATOR_TOKEN,
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
        let influx = influx_for(&fake.base_url);

        let err = influx.mint_write_token("sonde Paris").await.unwrap_err();
        assert!(err.contains("provision"), "message attendu explicite : {err}");
    }

    #[tokio::test]
    async fn an_unreachable_influx_reports_an_error_without_panicking() {
        // Port fermé : c'est l'état normal des premières secondes d'un
        // `docker compose up`, le hub ne doit pas s'écrouler dessus.
        let influx = influx_for("http://127.0.0.1:1");
        assert!(influx.ensure_provisioned().await.is_err());
        assert!(influx.ids().is_none(), "aucun identifiant ne doit être mémorisé");
    }

    #[tokio::test]
    async fn probe_settings_never_carry_the_operator_token() {
        let fake = FakeInflux::start(true, true).await;
        let influx = influx_for(&fake.base_url);

        let settings = influx.probe_settings("jeton-de-sonde", None);
        assert_eq!(settings.org, "lanprobe");
        assert_eq!(settings.bucket, "lanprobe");
        assert_eq!(settings.token, "jeton-de-sonde");

        let rendered = serde_json::to_string(&settings).unwrap();
        assert!(
            !rendered.contains("jeton-operateur-secret"),
            "le jeton opérateur ne doit jamais être sérialisé : {rendered}"
        );
    }

    #[test]
    fn the_hub_public_url_decides_where_probes_write() {
        // Le réglage normal est l'adresse publique du hub : l'utilisateur dit
        // par où on le joint, le hub en déduit où les sondes écrivent. Deux URL
        // voisines à distinguer, c'est une sonde enrôlée avec la mauvaise.
        let settings = settings_for("https://127.0.0.1:8086");
        settings
            .put(crate::settings::keys::HUB_PUBLIC_URL, "https://lanprobe.exemple.fr", false)
            .unwrap();
        let influx = Influx::new(settings, OPERATOR_TOKEN.to_string());
        let (url, source) = influx.resolve_advertise_url(Some("10.0.0.5:8080"));
        assert_eq!(
            url, "https://lanprobe.exemple.fr:8086",
            "l'hôte vient du réglage, le port d'Influx"
        );
        assert_eq!(source.as_str(), "hub_public_url");
    }

    #[test]
    fn an_explicit_influx_url_still_wins_over_the_deduction() {
        // Le cas rare — Influx exposé ailleurs — doit rester possible sans
        // rendre le cas courant plus compliqué.
        let settings = settings_for("https://127.0.0.1:8086");
        settings
            .put(crate::settings::keys::HUB_PUBLIC_URL, "https://lanprobe.exemple.fr", false)
            .unwrap();
        settings
            .put(
                crate::settings::keys::INFLUX_ADVERTISE_URL,
                "https://metrics.exemple.fr:9086",
                false,
            )
            .unwrap();
        let influx = Influx::new(settings, OPERATOR_TOKEN.to_string());
        let (url, source) = influx.resolve_advertise_url(None);
        assert_eq!(url, "https://metrics.exemple.fr:9086");
        assert_eq!(source.as_str(), "settings");
    }

    #[tokio::test]
    async fn advertise_url_prefers_the_setting_over_everything_else() {
        // Les trois sources définies en même temps : l'interface gagne.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let settings = settings_for("https://127.0.0.1:8086");
        settings
            .put(
                crate::settings::keys::INFLUX_ADVERTISE_URL,
                "https://regle-dans-l-app:9086",
                false,
            )
            .unwrap();
        // SAFETY : sérialisé par ENV_LOCK, restauré avant de rendre la main.
        unsafe { std::env::set_var(ADVERTISE_URL_ENV, "https://depuis-env:8086") };
        let influx = Influx::new(settings, OPERATOR_TOKEN.to_string());

        let (url, source) = influx.resolve_advertise_url(Some("autre-hote:8443"));
        unsafe { std::env::remove_var(ADVERTISE_URL_ENV) };

        assert_eq!(url, "https://regle-dans-l-app:9086");
        assert_eq!(source, AdvertiseSource::Settings);
        assert_eq!(source.as_str(), "settings");
    }

    #[tokio::test]
    async fn advertise_url_uses_the_environment_when_no_setting_exists() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var(ADVERTISE_URL_ENV, "https://depuis-env:8086") };
        let influx = influx_for("https://127.0.0.1:8086");

        let (url, source) = influx.resolve_advertise_url(Some("autre-hote:8443"));
        unsafe { std::env::remove_var(ADVERTISE_URL_ENV) };

        assert_eq!(url, "https://depuis-env:8086");
        assert_eq!(source.as_str(), "env");
    }

    #[tokio::test]
    async fn advertise_url_falls_back_to_the_request_host_with_the_influx_port() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var(ADVERTISE_URL_ENV) };
        // Sans variable, l'URL interne (127.0.0.1) ne veut rien dire pour une
        // sonde du LAN : on repart de l'hôte par lequel elle vient de joindre
        // le hub, en substituant le port d'Influx à celui du hub.
        let influx = influx_for("https://127.0.0.1:8086");

        let (url, source) = influx.resolve_advertise_url(Some("hub.example.org:8443"));
        assert_eq!(url, "https://hub.example.org:8086");
        assert_eq!(source, AdvertiseSource::HostHeader);
        assert_eq!(source.as_str(), "host_header");
    }

    #[tokio::test]
    async fn advertise_url_falls_back_to_the_internal_url_without_a_host_header() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var(ADVERTISE_URL_ENV) };
        let influx = influx_for("https://127.0.0.1:8086");

        let (url, source) = influx.resolve_advertise_url(None);
        assert_eq!(url, "https://127.0.0.1:8086");
        assert_eq!(source, AdvertiseSource::InternalUrl);
    }

    #[tokio::test]
    async fn advertise_url_handles_an_ipv6_host_header() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::remove_var(ADVERTISE_URL_ENV) };
        let influx = influx_for("https://127.0.0.1:8086");

        let (url, _) = influx.resolve_advertise_url(Some("[2001:db8::1]:8443"));
        assert_eq!(url, "https://[2001:db8::1]:8086");
    }

    #[tokio::test]
    async fn reachability_is_true_when_health_answers() {
        let fake = FakeInflux::start(true, true).await;
        let influx = influx_for(&fake.base_url);

        assert!(influx.check_reachable(&fake.base_url).await);
        assert!(
            fake.calls().iter().any(|(m, p, _)| m == "GET" && p == "/health"),
            "le test doit interroger /health : {:?}",
            fake.calls()
        );
    }

    #[tokio::test]
    async fn reachability_is_false_on_a_closed_port_without_erroring() {
        // Un échec de joignabilité est un résultat de test, pas une panne du
        // hub : la fonction rend `false`, elle ne remonte pas d'erreur.
        let influx = influx_for("http://127.0.0.1:1");
        assert!(!influx.check_reachable("http://127.0.0.1:1").await);
    }

    #[tokio::test]
    async fn applying_retention_patches_the_bucket_with_the_right_window() {
        let fake = FakeInflux::start(true, true).await;
        let influx = influx_for(&fake.base_url);
        influx.ensure_provisioned().await.unwrap();

        influx.apply_retention(30).await.unwrap();

        let (_, path, body) = fake
            .calls()
            .into_iter()
            .find(|(m, _, _)| m == "PATCH")
            .expect("le bucket doit être modifié");
        assert_eq!(path, "/api/v2/buckets/bucket-1");
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["retentionRules"][0]["everySeconds"], 30 * 86_400);
    }

    #[tokio::test]
    async fn unlimited_retention_clears_the_rules() {
        let fake = FakeInflux::start(true, true).await;
        let influx = influx_for(&fake.base_url);
        influx.ensure_provisioned().await.unwrap();

        influx.apply_retention(0).await.unwrap();

        let (_, _, body) = fake.calls().into_iter().find(|(m, _, _)| m == "PATCH").unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            body["retentionRules"].as_array().unwrap().len(),
            0,
            "0 = illimité : aucune règle d'expiration"
        );
    }

    #[test]
    fn the_operator_token_is_read_from_the_volume_file() {
        let dir = std::env::temp_dir().join(format!("lanprobe-web-optoken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(OPERATOR_TOKEN_FILE), "jeton-du-volume\n").unwrap();

        assert_eq!(load_operator_token(&dir).unwrap(), "jeton-du-volume");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_operator_token_falls_back_to_the_environment() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!("lanprobe-web-optoken-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        assert!(load_operator_token(&dir).is_err(), "sans fichier ni variable : erreur");

        unsafe { std::env::set_var(OPERATOR_TOKEN_ENV, "jeton-depuis-env") };
        let token = load_operator_token(&dir);
        unsafe { std::env::remove_var(OPERATOR_TOKEN_ENV) };

        assert_eq!(token.unwrap(), "jeton-depuis-env");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tls_fingerprint_matches_the_certificate_presented_by_influx() {
        let (addr, expected) = start_self_signed_tls_server().await;
        let influx = influx_for(&format!("https://{addr}"));

        let fingerprint = influx.ensure_tls_fingerprint().await.unwrap();
        assert_eq!(fingerprint, expected);
        assert!(
            fingerprint.contains(':') && fingerprint.len() == 32 * 3 - 1,
            "format AB:CD:… attendu : {fingerprint}"
        );

        // Mise en cache : la sonde suivante ne redéclenche pas de handshake.
        assert_eq!(influx.tls_fingerprint().as_deref(), Some(expected.as_str()));
    }

    #[tokio::test]
    async fn a_plain_http_influx_has_no_fingerprint_to_pin() {
        let fake = FakeInflux::start(true, true).await;
        let influx = influx_for(&fake.base_url);

        assert!(influx.ensure_tls_fingerprint().await.unwrap().is_empty());
        assert!(influx.tls_fingerprint().is_none());

        let settings = influx.probe_settings("jeton-de-sonde", None);
        let rendered = serde_json::to_string(&settings).unwrap();
        assert!(
            !rendered.contains("tls_fingerprint"),
            "pas d'empreinte à épingler en HTTP clair : {rendered}"
        );
    }

    #[tokio::test]
    async fn provisioning_retries_until_influx_answers() {
        // Les premières secondes d'un `docker compose up` : Influx répond 503.
        // Le hub doit démarrer quand même et réessayer.
        let fake = FakeInflux::start_flaky(true, true, 2).await;
        let influx = std::sync::Arc::new(influx_for(&fake.base_url));
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
    async fn the_double_really_refuses_over_http_not_just_in_its_unit_tests() {
        // ⚠️ Les tests de `flux_refusal` valident le VERDICT ; celui-ci valide
        // le CÂBLAGE. Sans lui, la fonction pourrait être parfaite et n'être
        // appelée nulle part — la doublure continuerait de répondre 200 à
        // tout, et rien ne le dirait.
        let fake = FakeInflux::start(true, true).await;
        let influx = influx_for(&fake.base_url);
        influx.ensure_provisioned().await.unwrap();

        // La requête exacte qui serait partie en production ce matin.
        let err = influx
            .query_flux("import \"experimental/types\"\nfrom(bucket:\"lanprobe\")")
            .await
            .expect_err("la doublure doit refuser l'import inconnu");
        assert!(err.contains("400"), "{err}");
        // ⚠️ Et la CAUSE doit remonter : le hub jetait le corps de la réponse
        // d'Influx et ne rendait qu'un « 400 Bad Request » muet.
        assert!(err.contains("experimental/types"), "la cause doit remonter : {err}");

        // La même requête avec le paquet qui existe vraiment passe.
        influx
            .query_flux("import \"types\"\nfrom(bucket:\"lanprobe\")")
            .await
            .expect("`types` existe et doit passer");
    }

    #[tokio::test]
    async fn flux_queries_are_proxied_and_their_result_returned() {
        let fake = FakeInflux::start(true, true).await;
        let influx = influx_for(&fake.base_url);
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

#[cfg(test)]
mod flux_refusal_tests {
    use super::testing::flux_refusal;

    /// ⚠️ Chacune de ces requêtes a été soumise à un **vrai influxd 2.9.1**,
    /// la version que le conteneur embarque, et refusée en 400. Le message
    /// exact d'Influx est cité en commentaire : la doublure ne devine pas ce
    /// qu'Influx refuserait, elle rejoue ce qu'il refuse.
    #[test]
    fn the_import_that_almost_shipped_this_morning_is_refused() {
        // « error @1:1-1:28: invalid import path experimental/types »
        //
        // 🔴 C'est LA requête qui serait partie en production au vert ce
        // matin : le test unitaire passait, la doublure répondait 200, et
        // l'import n'existe pas. Elle aurait fait tomber les trois graphes de
        // la fiche d'un coup.
        let refus = flux_refusal("import \"experimental/types\"\nfrom(bucket:\"b\")")
            .expect("l'import inconnu doit être refusé");
        assert!(refus.contains("experimental/types"), "{refus}");
        assert!(refus.contains("import"), "la cause doit être nommée : {refus}");
    }

    #[test]
    fn the_import_that_actually_exists_passes() {
        // Vérifié en 200 sur influxd 2.9.1, comme `math`, `strings`, `date`,
        // `experimental`, `array` et `influxdata/influxdb/schema`.
        assert_eq!(flux_refusal("import \"types\"\nfrom(bucket:\"b\")"), None);
        assert_eq!(flux_refusal("import \"math\"\nfrom(bucket:\"b\")"), None);
    }

    #[test]
    fn the_bare_duration_that_returned_502_is_refused() {
        // « cannot query an empty range »
        //
        // 🔴 `?range=24h` : une durée POSITIVE part vers l'avant, la fenêtre
        // vise le futur. Ce test-là existait et passait au vert, alors que la
        // requête échoue en réel — c'est l'exemple qui a révélé le défaut.
        let refus = flux_refusal("from(bucket:\"b\") |> range(start: 24h)")
            .expect("une fenêtre dans le futur doit être refusée");
        assert!(refus.contains("range"), "{refus}");
    }

    #[test]
    fn a_stop_before_the_start_is_refused() {
        // « cannot query an empty range », vérifié sous les deux formes.
        assert!(flux_refusal("from(b) |> range(start: -1h, stop: -2h)").is_some());
        assert!(flux_refusal("from(b) |> range(start: 1788600000, stop: 1788000000)").is_some());
    }

    #[test]
    fn an_unknown_aggregate_function_is_refused() {
        // « error @1:80-1:85: undefined identifier nawak »
        let refus = flux_refusal("from(b) |> aggregateWindow(every: 1h, fn: nawak)")
            .expect("une fonction inconnue doit être refusée");
        assert!(refus.contains("nawak"), "la cause doit être nommée : {refus}");
    }

    #[test]
    fn an_aggregate_window_without_a_step_is_refused() {
        // « error @1:49-1:74: missing required argument every »
        let refus = flux_refusal("from(b) |> aggregateWindow(fn: mean)")
            .expect("`every` est obligatoire");
        assert!(refus.contains("every"), "{refus}");
    }

    #[test]
    fn everything_the_hub_actually_sends_still_passes() {
        // ⚠️ Le vrai critère de cette doublure. Une doublure qui refuse à tort
        // est PIRE qu'une doublure permissive : elle fait perdre confiance
        // dans la suite entière. Ces quatre requêtes ont été vérifiées en 200
        // sur influxd 2.9.1.
        for flux in [
            "from(bucket: \"lanprobe\")\n  |> range(start: -24h)",
            "from(bucket: \"lanprobe\")\n  |> range(start: 1788000000, stop: 1788600000)",
            "from(bucket: \"lanprobe\")\n  |> range(start: -24h, stop: -1h)",
            "import \"types\"\nsource = from(bucket: \"lanprobe\")\n  |> range(start: -87d)\n\
             mesures |> aggregateWindow(every: 3h, fn: mean, createEmpty: false) |> yield(name: \"mean\")\n\
             mesures |> aggregateWindow(every: 3h, fn: max, createEmpty: false) |> yield(name: \"max\")",
            "from(bucket: \"lanprobe\")\n  |> range(start: -24h)\n  |> toFloat()\n  |> mean()",
        ] {
            assert_eq!(flux_refusal(flux), None, "refus à tort de : {flux}");
        }
    }
}
