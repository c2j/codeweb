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
}

pub type Result<T> = std::result::Result<T, CodeWebError>;
