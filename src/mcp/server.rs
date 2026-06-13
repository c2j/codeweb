use std::path::Path;

use crate::error::Result;
use crate::project::Project;
use rmcp::transport::io;
use rmcp::ServiceExt;

use super::tools::McpState;

pub fn run(project_path: &Path) -> Result<()> {
    let mut proj = Project::find(project_path)?;
    let _ = proj.load_store()?;
    let mut store = proj
        .take_store()
        .unwrap_or_else(|| crate::graph::store::GraphStore::new(proj.name()));
    store.ensure_consistency_with_progress();

    let state = McpState::new(store);

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
