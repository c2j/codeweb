use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CodeWebError {
    #[error("failed to read file {path}: {source}")]
    FileRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("no SQL files found in {path}")]
    NoFilesFound { path: PathBuf },

    #[error("export error: {message}")]
    ExportError { message: String },

    #[error("project not found (searched from {search_from})")]
    ProjectNotFound { search_from: PathBuf },

    #[error("project already exists at {path}")]
    ProjectAlreadyExists { path: PathBuf },

    #[error("config error: {message}")]
    ConfigError { message: String },
}

pub type Result<T> = std::result::Result<T, CodeWebError>;
