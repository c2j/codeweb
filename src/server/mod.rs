pub mod access_log;
pub mod assets;
pub mod handlers;
pub mod state;

use std::net::SocketAddr;
use std::path::Path;

use crate::error::Result;
use crate::project::Project;

pub fn run(project_path: &Path, addr: &str, open_browser: bool, log_level: &str) -> Result<()> {
    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(
        indicatif::ProgressStyle::default_spinner()
            .template("{spinner} {msg}")
            .unwrap(),
    );

    pb.set_message("Finding project...");
    let mut proj = Project::find(project_path)?;

    let codeweb_dir = proj.root().join(".codeweb");
    let level = match log_level {
        "debug" => access_log::LogLevel::Debug,
        _ => access_log::LogLevel::Info,
    };
    access_log::init(&codeweb_dir, level);

    pb.set_message("Loading graph store...");
    let _ = proj.load_store()?;

    pb.set_message("Preparing server (checking indexes)...");
    let state = state::AppState::new(proj);

    let node_count = state.store().graph().node_count();
    let edge_count = state.store().graph().edge_count();
    pb.finish_with_message(format!("Loaded {} nodes, {} edges", node_count, edge_count));

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

        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
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
