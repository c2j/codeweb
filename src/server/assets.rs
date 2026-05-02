use axum::{
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "assets/"]
struct Assets;

pub async fn serve_asset(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() || path == "/" {
        "index.html"
    } else {
        path
    };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let body = content.data.to_vec();
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
                body,
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
