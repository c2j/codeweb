pub mod assets;
pub mod handlers;
pub mod state;

use std::net::SocketAddr;
use std::path::Path;

use crate::error::Result;
use crate::project::Project;

pub fn run(project_path: &Path, addr: &str, open_browser: bool) -> Result<()> {
    let mut proj = Project::find(project_path)?;
    let _ = proj.load_store()?;
    let state = state::AppState::new(proj);

    let runtime =
        tokio::runtime::Runtime::new().map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("failed to create tokio runtime: {}", e),
        })?;

    runtime.block_on(async {
        let app = handlers::router(state);

        let socket_addr: SocketAddr =
            addr.parse()
                .map_err(|e| crate::error::CodeWebError::ExportError {
                    message: format!("invalid address '{}': {}", addr, e),
                })?;

        let listener = tokio::net::TcpListener::bind(socket_addr)
            .await
            .map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("failed to bind to {}: {}", addr, e),
            })?;

        eprintln!("Server listening on http://{}", socket_addr);

        if open_browser {
            let url = format!("http://{}", socket_addr);
            let _ = open_browser_url(&url);
        }

        axum::serve(listener, app)
            .await
            .map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("server error: {}", e),
            })
    })
}

#[cfg(target_os = "macos")]
fn open_browser_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "linux")]
fn open_browser_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "windows")]
fn open_browser_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("cmd")
        .args(["/c", "start", url])
        .spawn()
        .map(|_| ())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn open_browser_url(_url: &str) -> std::io::Result<()> {
    Ok(())
}
