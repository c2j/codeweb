pub mod config;

use crate::error::{CodeWebError, Result};
use crate::graph::builder::GraphBuilder;
use crate::graph::store::GraphStore;
use crate::parser;
use crate::parser::fingerprint::{compute_changes, FileChangeSet, FileRecord, FileType};
use config::ProjectConfig;
use std::collections::HashMap;
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
    pub files_unchanged: usize,
    pub files_changed: usize,
    pub files_added: usize,
    pub files_deleted: usize,
    pub nodes: usize,
    pub edges: usize,
    pub is_full_build: bool,
    pub is_up_to_date: bool,
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

    pub fn init(source_dirs: &[PathBuf], name: &str) -> Result<Self> {
        let cwd = std::env::current_dir().unwrap_or_default();

        let toml_path = cwd.join(CODEWEB_TOML);
        if toml_path.exists() {
            return Err(CodeWebError::ProjectAlreadyExists { path: toml_path });
        }

        let paths: Vec<String> = if source_dirs.is_empty() {
            vec![".".to_string()]
        } else {
            source_dirs
                .iter()
                .map(|d| {
                    if d.is_absolute() {
                        d.to_string_lossy().to_string()
                    } else {
                        let relative = pathdiff::diff_paths(d, &cwd).unwrap_or_else(|| d.clone());
                        relative.to_string_lossy().to_string()
                    }
                })
                .collect()
        };

        let content = ProjectConfig::template_with_paths(name, &paths);
        std::fs::write(&toml_path, &content).map_err(|e| CodeWebError::FileRead {
            path: toml_path.clone(),
            source: e,
        })?;

        let codeweb_dir = cwd.join(".codeweb");
        std::fs::create_dir_all(&codeweb_dir).map_err(|e| CodeWebError::FileRead {
            path: codeweb_dir,
            source: e,
        })?;

        Self::load_from(&toml_path)
    }

    fn load_from(toml_path: &Path) -> Result<Self> {
        let root = toml_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let content = std::fs::read_to_string(toml_path).map_err(|e| CodeWebError::FileRead {
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

        // Phase 1: Scan files
        let pb = indicatif::ProgressBar::new_spinner();
        pb.set_style(
            indicatif::ProgressStyle::default_spinner()
                .template("{spinner} {msg}")
                .unwrap(),
        );
        pb.set_message("Scanning files...");
        let current_files = scan_with_fingerprints(&input_paths);
        let files_scanned = current_files.len();
        let total = files_scanned;
        pb.finish_with_message(format!("Scanned {} files", files_scanned));

        // Phase 2: Load existing store and diff against manifest
        let existing_manifest: HashMap<PathBuf, FileRecord> = self
            .try_load_store()
            .map(|s| s.manifest().clone())
            .unwrap_or_default();

        let changes = compute_changes(&current_files, &existing_manifest);

        let is_up_to_date = changes.is_empty();
        let is_full_build = existing_manifest.is_empty();

        if is_up_to_date && !is_full_build {
            return Ok(AnalyzeReport {
                files_scanned,
                files_unchanged: changes.unchanged.len(),
                files_changed: 0,
                files_added: 0,
                files_deleted: 0,
                nodes: self
                    .store
                    .as_ref()
                    .map(|s| s.graph().node_count())
                    .unwrap_or(0),
                edges: self
                    .store
                    .as_ref()
                    .map(|s| s.graph().edge_count())
                    .unwrap_or(0),
                is_full_build: false,
                is_up_to_date: true,
                elapsed_ms: start.elapsed().as_millis() as u64,
            });
        }

        // Phase 3: Parse files with progress
        let pb = indicatif::ProgressBar::new(total as u64);
        pb.set_style(
            indicatif::ProgressStyle::with_template(
                "  {bar:40.cyan/blue} {pos}/{len} {wide_msg:.dim}",
            )
            .unwrap()
            .progress_chars("━━╾─"),
        );

        let mut all_files = parser::AllParsedFiles {
            sql_files: Vec::new(),
            java_files: Vec::new(),
            ibatis_files: Vec::new(),
            java_method_results: Vec::new(),
        };

        for input in &input_paths {
            let scanned = parser::scan_directory(input);

            pb.set_message("Parsing SQL...");
            for path in &scanned.sql_files {
                pb.set_message(format!(
                    "{}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                pb.inc(1);
            }
            all_files
                .sql_files
                .extend(parser::parse_sql_files(&scanned.sql_files));

            pb.set_message("Parsing Java...");
            for path in &scanned.java_files {
                pb.set_message(format!(
                    "{}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                pb.inc(1);
            }
            all_files
                .java_files
                .extend(parser::java_loader::load_java_files_from_paths(
                    &scanned.java_files,
                ));

            pb.set_message("Parsing XML mappers...");
            for path in &scanned.xml_files {
                pb.set_message(format!(
                    "{}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                pb.inc(1);
            }
            all_files
                .ibatis_files
                .extend(parser::ibatis_loader::load_ibatis_files_from_paths(
                    &scanned.xml_files,
                ));

            pb.set_message("Extracting Java methods...");
            all_files
                .java_method_results
                .extend(parser::java_method::parse_java_files_from_paths(
                    &scanned.java_files,
                ));
        }
        pb.finish_with_message(format!(
            "Parsed {} files ({} SQL, {} Java, {} XML)",
            all_files.sql_files.len() + all_files.java_files.len() + all_files.ibatis_files.len(),
            all_files.sql_files.len(),
            all_files.java_files.len(),
            all_files.ibatis_files.len(),
        ));

        // Phase 4: Build graph
        eprintln!("  Building graph...");
        let builder = GraphBuilder::new();
        let mut new_store = builder.build_store(&all_files, &self.config.project.name);

        // Phase 4: Update manifest with new fingerprints
        let new_records = compute_all_records(&current_files);
        new_store.update_manifest(new_records);
        new_store.remove_manifest_entries(&changes.deleted);

        let nodes = new_store.graph().node_count();
        let edges = new_store.graph().edge_count();

        self.save_store(&new_store)?;
        self.store = Some(new_store);

        let elapsed_ms = start.elapsed().as_millis() as u64;

        Ok(AnalyzeReport {
            files_scanned,
            files_unchanged: changes.unchanged.len(),
            files_changed: changes.modified.len(),
            files_added: changes.added.len(),
            files_deleted: changes.deleted.len(),
            nodes,
            edges,
            is_full_build,
            is_up_to_date: false,
            elapsed_ms,
        })
    }

    pub fn diff(&mut self) -> Result<FileChangeSet> {
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

        let current_files = scan_with_fingerprints(&input_paths);
        let existing_manifest: HashMap<PathBuf, FileRecord> = self
            .try_load_store()
            .map(|s| s.manifest().clone())
            .unwrap_or_default();

        Ok(compute_changes(&current_files, &existing_manifest))
    }

    pub fn store(&self) -> Option<&GraphStore> {
        self.store.as_ref()
    }

    pub fn load_store(&mut self) -> Result<&GraphStore> {
        let store_path = self.store_path();
        if self.store.is_none() && store_path.exists() {
            let loaded = match self.config.store.format {
                config::StoreFormat::Bincode => GraphStore::load_bincode(&store_path)?,
                config::StoreFormat::Json => GraphStore::load_json(&store_path)?,
            };
            self.store = Some(loaded);
        }
        self.store
            .as_ref()
            .ok_or_else(|| CodeWebError::ExportError {
                message: format!(
                    "no store found at {} — run `codeweb analyze` first",
                    store_path.display()
                ),
            })
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

    fn try_load_store(&mut self) -> Option<&GraphStore> {
        if self.store.is_none() {
            let store_path = self.store_path();
            if store_path.exists() {
                let loaded = match self.config.store.format {
                    config::StoreFormat::Bincode => GraphStore::load_bincode(&store_path).ok(),
                    config::StoreFormat::Json => GraphStore::load_json(&store_path).ok(),
                };
                self.store = loaded;
            }
        }
        self.store.as_ref()
    }
}

fn scan_with_fingerprints(paths: &[PathBuf]) -> Vec<(PathBuf, FileType)> {
    let mut files = Vec::new();
    for path in paths {
        let scanned = parser::scan_directory(path);
        for p in &scanned.sql_files {
            files.push((p.clone(), FileType::Sql));
        }
        for p in &scanned.java_files {
            files.push((p.clone(), FileType::Java));
        }
        for p in &scanned.xml_files {
            files.push((p.clone(), FileType::Xml));
        }
    }
    files
}

fn compute_all_records(files: &[(PathBuf, FileType)]) -> Vec<FileRecord> {
    files
        .iter()
        .filter_map(|(path, ft)| FileRecord::compute(path, *ft))
        .collect()
}
