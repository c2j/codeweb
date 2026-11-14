use std::path::{Path, PathBuf};

/// Directory to name in the repair command: the nearest ancestor holding a
/// `codeweb.toml`, else the store's own directory, else the cwd.
/// （自 store.rs 迁入：同一语义由 `StoreVersionMismatch` 的 Display 继续承载，
/// 修复提示 `-p <root>` 的行为由本模块测试锁定。）
fn store_project_root(store_path: &Path) -> PathBuf {
    let start = match store_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    // For a single relative component (`.codeweb`) `Path::parent` is the empty
    // path, so the cwd itself has to be probed separately.
    let mut starts: Vec<PathBuf> = vec![start.to_path_buf()];
    if start.is_relative() {
        starts.push(PathBuf::from("."));
    }
    for from in &starts {
        let mut dir = from.clone();
        loop {
            if dir.join(crate::project::CODEWEB_TOML).is_file() {
                return dir;
            }
            match dir.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => dir = parent.to_path_buf(),
                _ => break,
            }
        }
    }
    start.to_path_buf()
}

/// Single-quote a path so a root containing spaces or quotes can be pasted
/// straight into a shell.
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

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

    #[error(
        "refusing to initialize in {path}: the directory is not empty and has no codeweb.toml \
         (re-run with --force to write codeweb.toml and .codeweb/ there)"
    )]
    InitTargetNotEmpty { path: PathBuf },

    #[error("config error: {message}")]
    ConfigError { message: String },

    /// 嵌入契约（docs/embeded-codeweb.md §3.2.1）：宿主进程需要区分「没有 store」
    /// 与「store 版本过期」，此前两者都复用 `ExportError` 文本。
    #[error("store not found at {path}")]
    StoreNotFound { path: PathBuf },

    #[error(
        "store version mismatch at {}: found version {}, expected {}: \
         run `codeweb analyze -p {}` to regenerate \
         (stores are not migrated automatically; {} is unusable)",
        .path.display(),
        .found,
        .expected,
        .path.shell_quoted_project_root(),
        .path.display()
    )]
    StoreVersionMismatch {
        path: PathBuf,
        found: u32,
        expected: u32,
    },
}

/// `StoreVersionMismatch` 的 Display 在渲染时才计算修复命令的项目根（构造点可能
/// 拿到的是 `.codeweb/store.bincode` 这类相对路径），因此挂在 `Path` 的扩展
/// 方法上，供 thiserror 的字段表达式调用。
trait ShellQuotedProjectRoot {
    fn shell_quoted_project_root(&self) -> String;
}

impl ShellQuotedProjectRoot for Path {
    fn shell_quoted_project_root(&self) -> String {
        shell_quote(&store_project_root(self))
    }
}

pub type Result<T> = std::result::Result<T, CodeWebError>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::CODEWEB_TOML;

    /// The repair command must always carry a usable `-p` value, including for a
    /// relative store path: `Path::parent` of `.codeweb` is the empty path, which
    /// used to render as `codeweb analyze -p ` with nothing after the flag.
    /// (Whether the search settles on `.` or on `.codeweb` depends on whether the
    /// cwd holds a `codeweb.toml`, so this only pins the invariant.)
    /// （自 store.rs 迁移：原 `stale_store_message` 的行为守护，改由
    /// `StoreVersionMismatch` 的 Display 承载。）
    #[test]
    fn store_version_mismatch_never_emits_an_empty_p_value() {
        for path in [
            Path::new(".codeweb/store.bincode"),
            Path::new("store.bincode"),
            Path::new("/nonexistent/place/store.bincode"),
        ] {
            let message = CodeWebError::StoreVersionMismatch {
                path: path.to_path_buf(),
                found: 8,
                expected: 15,
            }
            .to_string();
            let value = message
                .split("analyze -p '")
                .nth(1)
                .and_then(|rest| rest.split('\'').next())
                .unwrap_or_else(|| panic!("no quoted -p value in: {message}"));
            assert!(
                !value.trim().is_empty(),
                "empty -p value for {path:?}: {message}"
            );
        }
    }

    /// A root containing spaces must still produce a pasteable command.
    #[test]
    fn store_version_mismatch_quotes_the_project_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("with space");
        let store_dir = root.join(".codeweb");
        std::fs::create_dir_all(&store_dir).unwrap();
        std::fs::write(root.join(CODEWEB_TOML), "[project]\nname = \"t\"\n").unwrap();

        let message = CodeWebError::StoreVersionMismatch {
            path: store_dir.join("store.bincode"),
            found: 8,
            expected: 15,
        }
        .to_string();
        assert!(
            message.contains(&format!("codeweb analyze -p '{}'", root.display())),
            "the root must be shell-quoted: {message}"
        );
    }

    /// The upward search must pick the tree the store actually belongs to, not
    /// just the store's own directory.
    /// （自 store.rs 迁移：原 `store_project_root` 的行为守护，改由 Display 中
    /// 的修复命令承载。）
    #[test]
    fn store_version_mismatch_prefers_the_nearest_codeweb_toml() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("proj");
        let store_dir = root.join(".codeweb");
        std::fs::create_dir_all(&store_dir).unwrap();
        std::fs::write(root.join(CODEWEB_TOML), "[project]\nname = \"t\"\n").unwrap();

        let message = CodeWebError::StoreVersionMismatch {
            path: store_dir.join("store.bincode"),
            found: 8,
            expected: 15,
        }
        .to_string();
        assert!(
            message.contains(&format!("analyze -p '{}'", root.display())),
            "the upward search must pick the tree the store belongs to: {message}"
        );
        // No codeweb.toml anywhere above: fall back to the store's directory.
        let orphan = dir.path().join("elsewhere");
        std::fs::create_dir_all(&orphan).unwrap();
        let message = CodeWebError::StoreVersionMismatch {
            path: orphan.join("store.bincode"),
            found: 8,
            expected: 15,
        }
        .to_string();
        assert!(
            message.contains(&format!("analyze -p '{}'", orphan.display())),
            "no codeweb.toml above: must fall back to the store directory: {message}"
        );
    }
}
