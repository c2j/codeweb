use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectMeta,
    #[serde(default)]
    pub analysis: AnalysisConfig,
    #[serde(default)]
    pub store: StoreConfig,
    #[serde(default)]
    pub lineage: LineageConfig,
}

#[derive(Debug, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct AnalysisConfig {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub encoding: HashMap<String, String>,
    #[serde(default)]
    pub java: JavaConfig,
    /// Number of SQL files parsed per memory chunk. Smaller = lower peak memory.
    /// Default: 100. Upper bound: 100 (set lower only).
    #[serde(default = "default_sql_chunk_size")]
    pub sql_chunk_size: usize,
}

fn default_sql_chunk_size() -> usize {
    100
}

/// Java SQL extraction tuning.
///
/// ```toml
/// [analysis.java]
/// extra_sql_methods = ["doQuery", "runSql"]
/// extra_sql_var_patterns = ["QUERY", "CMD", "STMT"]
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct JavaConfig {
    /// Additional method names whose first string argument is treated as SQL.
    /// Appended to ogsql-parser's built-in list (prepareStatement, createNativeQuery, etc.).
    #[serde(default)]
    pub extra_sql_methods: Vec<String>,

    /// Additional variable-name substrings (case-insensitive) that signal SQL content.
    /// The built-in pattern "SQL" is always active; these are appended to it.
    #[serde(default)]
    pub extra_sql_var_patterns: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct StoreConfig {
    #[serde(default = "default_store_path")]
    pub path: String,
    #[serde(default = "default_store_format")]
    pub format: StoreFormat,
}

fn default_store_path() -> String {
    ".codeweb/store.bincode".to_string()
}

fn default_store_format() -> StoreFormat {
    StoreFormat::Bincode
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: default_store_path(),
            format: default_store_format(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreFormat {
    Bincode,
    Json,
}

/// Lineage display tuning (flow/reference classification, issue #146).
///
/// ```toml
/// [lineage]
/// flow_min_overlap = 8      # absolute minimum column overlap for a flow source
/// flow_min_ratio = 0.15     # minimum overlap ratio of the target's written columns
/// ignore_columns = ["id", "curr_date"]
/// ```
#[derive(Debug, Deserialize)]
pub struct LineageConfig {
    /// Absolute minimum column overlap for a source edge to count as a flow source.
    /// Effective threshold is `min(target_cols, max(flow_min_overlap, ceil(target_cols × ratio)))`
    /// so narrow tables are not excluded by the absolute floor alone.
    #[serde(default = "default_flow_min_overlap")]
    pub flow_min_overlap: usize,
    /// Minimum overlap ratio of the target's written columns for a flow source.
    #[serde(default = "default_flow_min_ratio")]
    pub flow_min_ratio: f64,
    /// Column names excluded from overlap computation (project-wide same-name noise,
    /// e.g. `id`/`curr_date` present in nearly every table).
    #[serde(default)]
    pub ignore_columns: Vec<String>,
}

fn default_flow_min_overlap() -> usize {
    8
}

fn default_flow_min_ratio() -> f64 {
    0.15
}

impl Default for LineageConfig {
    fn default() -> Self {
        Self {
            flow_min_overlap: default_flow_min_overlap(),
            flow_min_ratio: default_flow_min_ratio(),
            ignore_columns: Vec::new(),
        }
    }
}

impl ProjectConfig {
    pub fn load(toml_content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(toml_content)
    }

    pub fn default_template(name: &str) -> String {
        Self::template_with_paths(name, &[".".to_string()])
    }

    pub fn template_with_paths(name: &str, paths: &[String]) -> String {
        let paths_toml = paths
            .iter()
            .map(|p| {
                let escaped = p.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{}\"", escaped)
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"[project]
name = "{}"

[analysis]
paths = [{}]
# exclude = ["**/test/**", "**/generated/**"]
# sql_chunk_size = 100

# [analysis.java]
# extra_sql_methods = []
# extra_sql_var_patterns = []

[store]
path = ".codeweb/store.bincode"
format = "bincode"

# [lineage]
# flow_min_overlap = 8
# flow_min_ratio = 0.15
# ignore_columns = []
"#,
            name, paths_toml
        )
    }
}
