use std::path::Path;

use crate::error::Result;
use crate::project::Project;
use rmcp::transport::io;
use rmcp::ServiceExt;

use super::tools::McpState;

pub fn run(project_path: &Path) -> Result<()> {
    let mut proj = Project::find(project_path)?;

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
                "  → Run `codeweb analyze` in {} to build the code graph.",
                proj.root().display()
            );
            Some(reason)
        }
    };

    let mut store = proj
        .take_store()
        .unwrap_or_else(|| crate::graph::store::GraphStore::new(proj.name()));
    store.ensure_consistency_with_progress();

    let state = McpState::new(store, proj.name().to_string(), empty_reason);

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
