use std::path::PathBuf;
use std::sync::Arc;

use crate::graph::store::GraphStore;
use crate::graph::CodeGraph;
use crate::project::Project;

#[derive(Clone)]
pub struct AppState {
    #[allow(dead_code)]
    project_name: String,
    project_root: PathBuf,
    store: Arc<GraphStore>,
}

impl AppState {
    pub fn new(project: Project) -> Self {
        let name = project.name().to_string();
        let root = project.root().to_path_buf();
        let mut store = project
            .store()
            .cloned()
            .unwrap_or_else(|| GraphStore::new(&name));
        store.ensure_consistency();
        Self {
            project_name: name,
            project_root: root,
            store: Arc::new(store),
        }
    }

    #[allow(dead_code)]
    pub fn from_store(store: GraphStore, root: PathBuf) -> Self {
        let name = store.project_name.clone();
        Self {
            project_name: name,
            project_root: root,
            store: Arc::new(store),
        }
    }

    pub fn store(&self) -> &GraphStore {
        &self.store
    }

    pub fn graph(&self) -> &CodeGraph {
        self.store.graph()
    }

    #[allow(dead_code)]
    pub fn project_name(&self) -> &str {
        &self.project_name
    }

    pub fn project_root(&self) -> &PathBuf {
        &self.project_root
    }
}
