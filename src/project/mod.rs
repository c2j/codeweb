pub mod config;

use config::ProjectConfig;
use crate::error::{CodeWebError, Result};
use crate::graph::builder::GraphBuilder;
use crate::graph::store::GraphStore;
use crate::parser;
use std::path::{Path, PathBuf};

const CODEWEB_TOML: &str = "codeweb.toml";

pub struct Project {
    root: PathBuf,
    config: ProjectConfig,
    store: Option<GraphStore>,
}

#[derive(Debug)]
pub struct AnalyzeReport {
    pub files_scanned: usize,
    pub nodes: usize,
    pub edges: usize,
    pub is_full_build: bool,
    pub elapsed_ms: u64,
}

impl Project {
    pub fn find(dir: &Path) -> Result<Self> {
        let mut current = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(dir)
        };

        loop {
            let toml_path = current.join(CODEWEB_TOML);
            if toml_path.exists() {
                return Self::load_from(&toml_path);
            }
            match current.parent() {
                Some(parent) => current = parent.to_path_buf(),
                None => break,
            }
        }

        Err(CodeWebError::ProjectNotFound {
            search_from: dir.to_path_buf(),
        })
    }

    pub fn init(dir: &Path, name: &str) -> Result<Self> {
        let dir = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(dir)
        };

        let toml_path = dir.join(CODEWEB_TOML);
        if toml_path.exists() {
            return Err(CodeWebError::ProjectAlreadyExists {
                path: toml_path,
            });
        }

        let content = ProjectConfig::default_template(name);
        std::fs::write(&toml_path, &content).map_err(|e| CodeWebError::FileRead {
            path: toml_path.clone(),
            source: e,
        })?;

        let codeweb_dir = dir.join(".codeweb");
        std::fs::create_dir_all(&codeweb_dir).map_err(|e| CodeWebError::FileRead {
            path: codeweb_dir,
            source: e,
        })?;

        Self::load_from(&toml_path)
    }

    fn load_from(toml_path: &Path) -> Result<Self> {
        let root = toml_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let content =
            std::fs::read_to_string(toml_path).map_err(|e| CodeWebError::FileRead {
                path: toml_path.to_path_buf(),
                source: e,
            })?;
        let config = ProjectConfig::load(&content).map_err(|e| CodeWebError::ConfigError {
            message: e.to_string(),
        })?;

        Ok(Project {
            root,
            config,
            store: None,
        })
    }

    pub fn analyze(&mut self) -> Result<AnalyzeReport> {
        let start = std::time::Instant::now();

        let input_paths: Vec<PathBuf> = if self.config.analysis.paths.is_empty() {
            vec![self.root.clone()]
        } else {
            self.config
                .analysis
                .paths
                .iter()
                .map(|p| self.root.join(p))
                .collect()
        };

        let mut all_files = parser::AllParsedFiles {
            sql_files: Vec::new(),
            java_files: Vec::new(),
            ibatis_files: Vec::new(),
            java_method_results: Vec::new(),
        };

        for input in &input_paths {
            let loaded = parser::load_all_files(input)?;
            all_files.sql_files.extend(loaded.sql_files);
            all_files.java_files.extend(loaded.java_files);
            all_files.ibatis_files.extend(loaded.ibatis_files);
            all_files.java_method_results.extend(loaded.java_method_results);
        }

        let files_scanned = all_files.sql_files.len()
            + all_files.java_files.len()
            + all_files.ibatis_files.len();

        let builder = GraphBuilder::new();
        let new_store = builder.build_store(&all_files, &self.config.project.name);

        let nodes = new_store.graph().node_count();
        let edges = new_store.graph().edge_count();

        self.save_store(&new_store)?;
        self.store = Some(new_store);

        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(AnalyzeReport {
            files_scanned,
            nodes,
            edges,
            is_full_build: true,
            elapsed_ms,
        })
    }

    pub fn store(&self) -> Option<&GraphStore> {
        self.store.as_ref()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn name(&self) -> &str {
        &self.config.project.name
    }

    fn store_path(&self) -> PathBuf {
        self.root.join(&self.config.store.path)
    }

    fn save_store(&self, store: &GraphStore) -> Result<()> {
        let path = self.store_path();
        match self.config.store.format {
            config::StoreFormat::Bincode => store.save_bincode(&path)?,
            config::StoreFormat::Json => store.save_json(&path)?,
        }
        Ok(())
    }
}
