use std::path::{Path, PathBuf};

use crate::error::{CodeWebError, Result};
use crate::graph::store::GraphStore;
use crate::project::Project;
use rmcp::transport::io;
use rmcp::ServiceExt;

use super::tools::McpState;

/// Resolve the directory the server may write into (issue #171).
///
/// Absolutizes against the cwd and normalizes `..` lexically. Deliberately
/// does not `canonicalize`: the permitted root must stay lexically identical
/// to `Project::root()` so the write guard can compare the two.
fn resolve_workspace(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(path)
    };
    super::tools::normalize_lexically(&absolute)
}

pub fn run(project_path: &Path) -> Result<()> {
    let mut project = match Project::find(project_path) {
        Ok(project) => Some(project),
        Err(CodeWebError::ProjectNotFound { .. }) => None,
        Err(other) => return Err(other),
    };

    let (workspace, store, project_name, empty_reason) = match project.as_mut() {
        Some(proj) => {
            // Intentionally swallow store load errors — MCP server must stay alive for JSON-RPC.
            let empty_reason = match proj.try_load_store() {
                Some(_) => None,
                None => {
                    let store_path = proj.store_path();
                    let reason = if store_path.exists() {
                        format!(
                            "Code graph store at {} exists but could not be loaded (corrupted or incompatible format).",
                            store_path.display()
                        )
                    } else {
                        format!(
                            "Code graph has not been built yet (no store at {}).",
                            store_path.display()
                        )
                    };
                    eprintln!("codeweb mcp: {}", reason);
                    eprintln!(
                        "  → Call the `codeweb_analyze` MCP tool (or run `codeweb analyze` in {}) to build the code graph.",
                        proj.root().display()
                    );
                    Some(reason)
                }
            };

            let mut store = proj
                .take_store()
                .unwrap_or_else(|| GraphStore::new(proj.name()));
            store.ensure_consistency_with_progress();

            let workspace = resolve_workspace(proj.root());
            let name = proj.name().to_string();
            (workspace, store, name, empty_reason)
        }
        None => {
            let workspace = resolve_workspace(project_path);
            let name = workspace
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("project")
                .to_string();
            let reason = format!(
                "No codeweb.toml found from {} upward — the project is not initialized.",
                workspace.display()
            );
            eprintln!("codeweb mcp: {}", reason);
            eprintln!("  → Call the `codeweb_init` MCP tool to create one (this server will not exit).");
            (workspace, GraphStore::new(&name), name, Some(reason))
        }
    };

    let state = McpState::new(workspace, project, store, project_name, empty_reason);

    let runtime =
        tokio::runtime::Runtime::new().map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("failed to create tokio runtime: {}", e),
        })?;

    runtime.block_on(async {
        let transport = io::stdio();
        let server =
            state
                .serve(transport)
                .await
                .map_err(|e| crate::error::CodeWebError::ExportError {
                    message: format!("MCP server error: {}", e),
                })?;
        server
            .waiting()
            .await
            .map(|_| ())
            .map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("MCP server wait error: {}", e),
            })
    })
}
