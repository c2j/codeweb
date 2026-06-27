pub mod config;

use crate::error::{CodeWebError, Result};
use crate::graph::builder::{GraphBuildContext, GraphBuilder};
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

        let codeweb_dir = self.root.join(".codeweb");
        crate::parse_log::init(&codeweb_dir);

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
        let current_files = scan_with_fingerprints(&input_paths, &self.config.analysis.exclude);
        let files_scanned = current_files.len();
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

        // Split scanned files by type (no re-scanning — reuse Phase 1 results)
        let mut all_sql_paths: Vec<PathBuf> = Vec::new();
        let mut all_java_paths: Vec<PathBuf> = Vec::new();
        let mut all_xml_paths: Vec<PathBuf> = Vec::new();
        #[cfg(feature = "jsp")]
        let mut all_jsp_paths: Vec<PathBuf> = Vec::new();
        for (path, file_type) in &current_files {
            match file_type {
                FileType::Sql => all_sql_paths.push(path.clone()),
                FileType::Java => all_java_paths.push(path.clone()),
                FileType::Xml => all_xml_paths.push(path.clone()),
                #[cfg(feature = "jsp")]
                FileType::Jsp => all_jsp_paths.push(path.clone()),
            }
        }

        #[cfg(feature = "jsp")]
        let total =
            all_sql_paths.len() + all_java_paths.len() + all_xml_paths.len() + all_jsp_paths.len();
        #[cfg(not(feature = "jsp"))]
        let total = all_sql_paths.len() + all_java_paths.len() + all_xml_paths.len();

        // Phase 3-4: Chunked parsing + streaming graph building
        // SQL files are parsed in chunks to bound peak memory. Each chunk's
        // parsed AST is dropped before the next chunk begins.
        let pb = indicatif::ProgressBar::new(total as u64);
        pb.set_style(
            indicatif::ProgressStyle::with_template(
                "  {bar:40.cyan/blue} {pos}/{len} {wide_msg:.dim}",
            )
            .unwrap()
            .progress_chars("━━╾─"),
        );

        const SQL_CHUNK_SIZE: usize = 500;
        let total_sql = all_sql_paths.len();
        let sql_chunks = total_sql.div_ceil(SQL_CHUNK_SIZE);

        let mut ctx = GraphBuildContext::new();
        let mut all_hashes: Vec<(PathBuf, String, parser::fingerprint::FileType)> = Vec::new();

        pb.set_message(format!("Parsing SQL (1/{})...", sql_chunks));
        for (chunk_idx, chunk) in all_sql_paths.chunks(SQL_CHUNK_SIZE).enumerate() {
            pb.set_message(format!("Parsing SQL ({}/{})...", chunk_idx + 1, sql_chunks));
            let parsed = parser::parse_sql_files(chunk);
            pb.inc(chunk.len() as u64);

            // Collect lightweight hashes before dropping parsed data
            for pf in &parsed {
                all_hashes.push((pf.path.clone(), pf.content_hash.clone(), FileType::Sql));
            }

            GraphBuilder::build_sql_chunk(&mut ctx, &parsed);
            // parsed dropped here — AST memory freed
        }

        // Java (typically fewer files, parse all at once)
        pb.set_message("Parsing Java...");
        let java_config = ogsql_parser::java::JavaExtractConfig {
            extra_sql_methods: self.config.analysis.java.extra_sql_methods.clone(),
            extra_sql_var_patterns: self.config.analysis.java.extra_sql_var_patterns.clone(),
        };
        let java_combined = parser::java_loader::load_java_files_combined_with_config(
            &all_java_paths,
            &java_config,
        );
        pb.inc(all_java_paths.len() as u64);

        let (java_files, java_method_results): (Vec<_>, Vec<_>) = java_combined
            .into_iter()
            .map(|(path, combined)| {
                (
                    parser::JavaParsedFile {
                        path: path.clone(),
                        result: combined.sql_result,
                        content_hash: combined.content_hash,
                    },
                    combined.method_result,
                )
            })
            .unzip();

        // Collect Java hashes
        for pf in &java_files {
            all_hashes.push((pf.path.clone(), pf.content_hash.clone(), FileType::Java));
        }

        // XML (typically fewer files)
        pb.set_message("Parsing XML mappers...");
        let ibatis_files = parser::ibatis_loader::load_ibatis_files_from_paths(&all_xml_paths);
        let structured_ibatis_files =
            parser::ibatis_loader::load_ibatis_structured_files_from_paths(&all_xml_paths);
        pb.inc(all_xml_paths.len() as u64);

        pb.finish_with_message(format!(
            "Parsed {} files ({} SQL, {} Java, {} XML)",
            total_sql + java_files.len() + ibatis_files.len(),
            total_sql,
            java_files.len(),
            ibatis_files.len(),
        ));

        // Phase 5: Add non-SQL nodes to graph
        eprintln!("  Building graph...");
        let source_paths: Vec<PathBuf> = if self.config.analysis.paths.is_empty() {
            vec![self.root.clone()]
        } else {
            self.config
                .analysis
                .paths
                .iter()
                .map(|p| self.root.join(p))
                .collect()
        };
        GraphBuilder::add_ibatis_nodes_from_parsed_with_source_paths(
            &ibatis_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &mut ctx.mapper_index,
            &mut ctx.table_index,
            &mut ctx.builtin_index,
            &source_paths,
        );
        GraphBuilder::add_java_nodes_from_parsed_with_source_paths(
            &java_files,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &ctx.mapper_index,
            &mut ctx.table_index,
            &source_paths,
        );
        GraphBuilder::add_java_method_nodes_from_parsed(
            &java_method_results,
            &mut ctx.graph,
            &mut ctx.proc_index,
            &ctx.mapper_index,
        );

        #[cfg(feature = "jsp")]
        {
            if !all_jsp_paths.is_empty() {
                pb.set_message("Parsing JSP...");
                let jsp_results = crate::parser::jsp_loader::load_jsp_files_from_paths(
                    &all_jsp_paths,
                    &java_config,
                );
                pb.inc(all_jsp_paths.len() as u64);

                for result in &jsp_results {
                    let hash = blake3::hash(result.synthesized.source.as_bytes())
                        .to_hex()
                        .to_string();
                    all_hashes.push((result.file.clone(), hash, FileType::Jsp));
                }

                GraphBuilder::add_jsp_nodes_from_parsed(&jsp_results, &mut ctx);
            }
        }

        GraphBuilder::finalize_graph(&mut ctx);

        // Expand dynamic SQL variants for fingerprint index
        let variant_map = GraphBuilder::add_ibatis_structured_variants(
            &structured_ibatis_files,
            &ctx.mapper_index,
            &source_paths,
        );

        // Collect XML hashes (no AST to drop — ibatis_files is lightweight)
        for pf in &ibatis_files {
            all_hashes.push((pf.path.clone(), pf.content_hash.clone(), FileType::Xml));
        }

        // Phase 6: Build store
        let mut new_store = GraphStore::from_graph(&self.config.project.name, ctx.graph);
        new_store.enrich_fingerprint_index_with_variants(&variant_map);

        // Build manifest from collected hashes (no re-reading files)
        let mut new_records = Vec::new();
        for (path, hash, file_type) in &all_hashes {
            if let Some(record) = FileRecord::from_parts(path.clone(), hash.clone(), *file_type) {
                new_records.push(record);
            }
        }
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

        let current_files = scan_with_fingerprints(&input_paths, &self.config.analysis.exclude);
        let existing_manifest: HashMap<PathBuf, FileRecord> = self
            .try_load_store()
            .map(|s| s.manifest().clone())
            .unwrap_or_default();

        Ok(compute_changes(&current_files, &existing_manifest))
    }

    pub fn store(&self) -> Option<&GraphStore> {
        self.store.as_ref()
    }

    /// Take ownership of the loaded store, avoiding an expensive deep clone.
    /// Returns `None` if no store has been loaded yet.
    pub fn take_store(&mut self) -> Option<GraphStore> {
        self.store.take()
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

    pub fn save_store(&self, store: &GraphStore) -> Result<()> {
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

fn scan_with_fingerprints(paths: &[PathBuf], exclude: &[String]) -> Vec<(PathBuf, FileType)> {
    let mut files = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for path in paths {
        let scanned = parser::scan_directory(path, exclude);
        for p in &scanned.sql_files {
            if seen.insert(p.clone()) {
                files.push((p.clone(), FileType::Sql));
            }
        }
        for p in &scanned.java_files {
            if seen.insert(p.clone()) {
                files.push((p.clone(), FileType::Java));
            }
        }
        for p in &scanned.xml_files {
            if seen.insert(p.clone()) {
                files.push((p.clone(), FileType::Xml));
            }
        }
        #[cfg(feature = "jsp")]
        for p in &scanned.jsp_files {
            if seen.insert(p.clone()) {
                files.push((p.clone(), FileType::Jsp));
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scan_with_fingerprints_deduplicates_overlapping_paths() {
        let tmpdir = tempfile::tempdir().unwrap();
        let src_dir = tmpdir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("test.sql"), "SELECT 1;").unwrap();

        // Pass both the parent dir and the child dir — test.sql should appear only once
        let paths = vec![tmpdir.path().to_path_buf(), src_dir.clone()];
        let result = scan_with_fingerprints(&paths, &[]);

        let sql_count = result
            .iter()
            .filter(|(p, ft)| *ft == FileType::Sql && p.file_name().unwrap() == "test.sql")
            .count();
        assert_eq!(
            sql_count, 1,
            "test.sql should appear exactly once even with overlapping scan paths, got {}",
            sql_count
        );
    }
}
