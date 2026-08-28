//! Interface web embarquée dans le binaire.
//!
//! Le build Vite est dans `../../web-ui/dist`. Le dossier doit exister au
//! moment du `cargo build` — `npm run build:web` avant `cargo build -p
//! lanprobe-web`, aussi bien en CI que dans le Dockerfile. Embarquer plutôt
//! que servir depuis le disque évite d'avoir à monter un volume de fichiers
//! statiques dont la version pourrait diverger de celle du binaire.

use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../web-ui/dist"]
struct Assets;

/// Sert un fichier embarqué, avec repli sur `index.html`.
///
/// Le repli est nécessaire parce que l'interface fait son propre routage :
/// un rechargement sur `/probes/abc` doit rendre l'application, pas un 404.
/// En revanche une requête `/api/...` qui arrive ici n'a pas trouvé sa route —
/// lui rendre `index.html` déguiserait une erreur d'API en page valide, et le
/// client tenterait de parser du HTML comme du JSON.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "route inconnue").into_response();
    }

    let candidate = if path.is_empty() { "index.html" } else { path };

    match Assets::get(candidate).or_else(|| Assets::get("index.html")) {
        Some(file) => {
            let mime = mime_guess::from_path(candidate).first_or_octet_stream();
            // Les fichiers d'assets portent une empreinte dans leur nom
            // (`index-C33p5ak9.js`) : ils sont immuables et peuvent être mis
            // en cache longtemps. `index.html`, lui, ne doit jamais l'être,
            // sinon un navigateur continue de réclamer les assets de la
            // version précédente après une mise à jour du conteneur.
            let cache = if candidate == "index.html" {
                "no-cache"
            } else {
                "public, max-age=31536000, immutable"
            };
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime.as_ref()),
                    (header::CACHE_CONTROL, cache),
                ],
                Body::from(file.data.into_owned()),
            )
                .into_response()
        }
        // Arrive uniquement si le binaire a été construit sans `web-ui/dist`.
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "interface web absente du binaire — lancer `npm run build:web` avant `cargo build`",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn api_paths_are_not_swallowed_by_the_spa_fallback() {
        let response = serve("/api/inconnu".parse::<Uri>().unwrap()).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "une route d'API absente doit rester un 404, pas rendre l'application"
        );
    }

    #[tokio::test]
    async fn index_is_never_cached_but_fingerprinted_assets_are() {
        let response = serve("/".parse::<Uri>().unwrap()).await;
        // Le binaire de test est construit avec le dist réel ; si l'interface
        // n'a pas été buildée, ce test le signale plutôt que de passer.
        assert_eq!(response.status(), StatusCode::OK, "web-ui/dist manquant ?");
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-cache"
        );
    }
}
