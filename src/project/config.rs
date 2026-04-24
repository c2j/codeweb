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
        format!(
            r#"[project]
name = "{}"

[analysis]
paths = ["."]

[store]
path = ".codeweb/store.bincode"
format = "bincode"
"#,
            name
        )
    }
}
