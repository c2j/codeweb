use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectMeta,
    #[serde(default)]
    pub analysis: AnalysisConfig,
    #[serde(default)]
    pub store: StoreConfig,
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
            .map(|p| format!("\"{}\"", p))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"[project]
name = "{}"

[analysis]
paths = [{}]
# exclude = ["**/test/**", "**/generated/**"]

# [analysis.java]
# extra_sql_methods = []
# extra_sql_var_patterns = []

[store]
path = ".codeweb/store.bincode"
format = "bincode"
"#,
            name, paths_toml
        )
    }
}
