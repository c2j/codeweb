use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileType {
    Sql,
    Java,
    Xml,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: PathBuf,
    pub content_hash: String,
    pub mtime_ns: u128,
    pub size: u64,
    pub file_type: FileType,
    pub parse_ok: bool,
    pub node_count: usize,
}

#[allow(dead_code)]
impl FileRecord {
    pub fn from_parts(path: PathBuf, content_hash: String, file_type: FileType) -> Option<Self> {
        let metadata = std::fs::metadata(&path).ok()?;
        let mtime_ns = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        let size = metadata.len();

        Some(FileRecord {
            path,
            content_hash,
            mtime_ns,
            size,
            file_type,
            parse_ok: false,
            node_count: 0,
        })
    }

    pub fn compute(path: &std::path::Path, file_type: FileType) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let mtime_ns = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        let size = metadata.len();

        let bytes = std::fs::read(path).ok()?;
        let content_hash = blake3::hash(&bytes).to_hex().to_string();

        Some(FileRecord {
            path: path.to_path_buf(),
            content_hash,
            mtime_ns,
            size,
            file_type,
            parse_ok: false,
            node_count: 0,
        })
    }

    pub fn mtime_matches(&self, path: &std::path::Path) -> bool {
        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        let Ok(modified) = metadata.modified() else {
            return false;
        };
        let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) else {
            return false;
        };
        duration.as_nanos() == self.mtime_ns
    }
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct FileChangeSet {
    pub unchanged: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub added: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

#[allow(dead_code)]
impl FileChangeSet {
    pub fn is_empty(&self) -> bool {
        self.modified.is_empty() && self.added.is_empty() && self.deleted.is_empty()
    }

    pub fn total_changes(&self) -> usize {
        self.modified.len() + self.added.len() + self.deleted.len()
    }
}

#[allow(dead_code)]
pub fn compute_changes(
    current_files: &[(PathBuf, FileType)],
    manifest: &HashMap<PathBuf, FileRecord>,
) -> FileChangeSet {
    let mut changes = FileChangeSet::default();
    let current_set: std::collections::HashSet<_> =
        current_files.iter().map(|(p, _)| p.clone()).collect();

    for (path, file_type) in current_files {
        match manifest.get(path) {
            None => {
                changes.added.push(path.clone());
            }
            Some(record) => {
                if record.mtime_matches(path) {
                    changes.unchanged.push(path.clone());
                } else if let Some(current) = FileRecord::compute(path, *file_type) {
                    if current.content_hash == record.content_hash {
                        changes.unchanged.push(path.clone());
                    } else {
                        changes.modified.push(path.clone());
                    }
                } else {
                    changes.modified.push(path.clone());
                }
            }
        }
    }

    for path in manifest.keys() {
        if !current_set.contains(path) {
            changes.deleted.push(path.clone());
        }
    }

    changes
}
