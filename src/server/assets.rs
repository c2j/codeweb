use axum::{
    extract::Request,
    http::StatusCode,
    response::IntoResponse,
};

pub async fn serve_asset(_req: Request) -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "Not Found")
}
