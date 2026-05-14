use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug)]
pub struct ScannedFiles {
    pub sql_files: Vec<PathBuf>,
    pub java_files: Vec<PathBuf>,
    pub xml_files: Vec<PathBuf>,
}

/// Ensure path is UTF-8 representable by lossy-converting non-UTF-8 bytes.
/// This guarantees bincode serialization (which requires PathBuf.to_str() to succeed) never fails.
pub fn sanitize_path(p: &Path) -> PathBuf {
    PathBuf::from(p.to_string_lossy().into_owned())
}

pub fn scan_directory(input: &Path) -> ScannedFiles {
    if input.is_file() {
        return scan_single_file(input);
    }

    let mut sql_files = Vec::new();
    let mut java_files = Vec::new();
    let mut xml_files = Vec::new();

    for entry in WalkDir::new(input).into_iter().filter_map(|e| e.ok()) {
        let path = sanitize_path(&entry.into_path());
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "sql" => sql_files.push(path),
            "java" => java_files.push(path),
            "xml" => xml_files.push(path),
            _ => {}
        }
    }

    ScannedFiles {
        sql_files,
        java_files,
        xml_files,
    }
}

fn scan_single_file(input: &Path) -> ScannedFiles {
    let mut scanned = ScannedFiles {
        sql_files: Vec::new(),
        java_files: Vec::new(),
        xml_files: Vec::new(),
    };
    let path = sanitize_path(input);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "sql" => scanned.sql_files.push(path),
        "java" => scanned.java_files.push(path),
        "xml" => scanned.xml_files.push(path),
        _ => {}
    }
    scanned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_path_preserves_utf8() {
        let path = PathBuf::from("/some/utf8/path.sql");
        let sanitized = sanitize_path(&path);
        assert_eq!(sanitized, path);
    }

    #[test]
    fn sanitize_path_converts_non_utf8() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let os_str = std::ffi::OsStr::from_bytes(b"/some/\xff\xfe/path.sql");
            let path = PathBuf::from(os_str);
            assert!(path.to_str().is_none(), "path should not be valid UTF-8");
            let sanitized = sanitize_path(&path);
            assert!(
                sanitized.to_str().is_some(),
                "sanitized path should be valid UTF-8"
            );
        }
    }
}
