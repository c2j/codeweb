use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileType {
    Sql,
    Java,
    Xml,
    #[cfg(feature = "jsp")]
    Jsp,
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

/// Canonical spelling used only for identity comparison. Resolves symlinks
/// through the deepest existing ancestor so a deleted file still maps to a
/// stable key; falls back to the lexical path when nothing resolves.
///
/// Only the parent directory is canonicalized: symlinked ancestors (macOS
/// `/var` vs `/private/var`) still compare equal, while a symlinked leaf
/// file keeps its own key instead of collapsing into its target.
fn canonical_key(path: &Path) -> PathBuf {
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Ok(canon) = std::fs::canonicalize(parent) {
            return canon.join(name);
        }
    }
    let mut ancestor = path;
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while let Some(parent) = ancestor.parent() {
        tail.push(ancestor.file_name().unwrap_or_default().to_os_string());
        ancestor = parent;
        if let Ok(canon) = std::fs::canonicalize(ancestor) {
            let mut resolved = canon;
            for part in tail.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
    }
    path.to_path_buf()
}

#[allow(dead_code)]
pub fn compute_changes(
    current_files: &[(PathBuf, FileType)],
    manifest: &HashMap<PathBuf, FileRecord>,
) -> FileChangeSet {
    let mut changes = FileChangeSet::default();
    let current_set: std::collections::HashSet<_> =
        current_files.iter().map(|(p, _)| p.clone()).collect();

    // Canonical views are built lazily on the first lexical miss: the same
    // file can be spelled through different symlinked ancestors (macOS
    // `/var` vs `/private/var`), and the manifest may carry either spelling.
    // Exact matches keep the common path free of extra syscalls.
    let mut manifest_by_canon: Option<HashMap<PathBuf, PathBuf>> = None;
    let mut current_by_canon: Option<std::collections::HashSet<PathBuf>> = None;

    for (path, file_type) in current_files {
        let record = match manifest.get(path) {
            Some(record) => Some(record),
            None => {
                let canon = canonical_key(path);
                manifest_by_canon
                    .get_or_insert_with(|| {
                        manifest
                            .keys()
                            .map(|stored| (canonical_key(stored), stored.clone()))
                            .collect()
                    })
                    .get(&canon)
                    .and_then(|stored| manifest.get(stored))
            }
        };
        match record {
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
        if current_set.contains(path) {
            continue;
        }
        if !current_by_canon
            .get_or_insert_with(|| {
                current_files
                    .iter()
                    .map(|(p, _)| canonical_key(p))
                    .collect()
            })
            .contains(&canonical_key(path))
        {
            changes.deleted.push(path.clone());
        }
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same file reachable through a symlinked ancestor (macOS `/var` vs
    /// `/private/var`, user-created project symlinks) must compare equal in
    /// both directions; otherwise every file reports added + deleted.
    #[cfg(unix)]
    #[test]
    fn compute_changes_matches_files_across_symlinked_path_aliases() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(real.join("sql")).unwrap();
        let file = real.join("sql").join("a.sql");
        std::fs::write(&file, "SELECT 1;").unwrap();
        let alias = tmp.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();

        let record = FileRecord::compute(&file, FileType::Sql).unwrap();
        let manifest = HashMap::from([(file.clone(), record)]);
        let aliased = alias.join("sql").join("a.sql");
        let current = vec![(aliased, FileType::Sql)];

        let changes = compute_changes(&current, &manifest);
        assert!(
            changes.is_empty(),
            "symlink-aliased spellings must compare equal: {changes:?}"
        );

        // Reverse direction: manifest under the alias, scan under the real path.
        let record = FileRecord::compute(&file, FileType::Sql).unwrap();
        let manifest = HashMap::from([(alias.join("sql").join("a.sql"), record)]);
        let current = vec![(file.clone(), FileType::Sql)];

        let changes = compute_changes(&current, &manifest);
        assert!(
            changes.is_empty(),
            "reverse alias direction must compare equal: {changes:?}"
        );
    }

    /// 叶文件符号链接不并入目标文件：新链接路径必须出现在 added，
    /// 目标路径按自身键独立判定 deleted/unchanged。
    #[cfg(unix)]
    #[test]
    fn compute_changes_keeps_leaf_symlinks_as_independent_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let target = real.join("a.sql");
        std::fs::write(&target, "SELECT 1;").unwrap();
        let link = real.join("b.sql");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        // 清单只有 a.sql，扫描面换成指向它的 b.sql：不能判成 unchanged。
        let record = FileRecord::compute(&target, FileType::Sql).unwrap();
        let manifest = HashMap::from([(target.clone(), record)]);
        let current = vec![(link.clone(), FileType::Sql)];

        let changes = compute_changes(&current, &manifest);
        assert_eq!(changes.added, vec![link.clone()]);
        assert_eq!(changes.deleted, vec![target.clone()]);
        assert!(changes.modified.is_empty() && changes.unchanged.is_empty());
    }
}
