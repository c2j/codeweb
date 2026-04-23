use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug)]
pub struct ScannedFiles {
    pub sql_files: Vec<PathBuf>,
    pub java_files: Vec<PathBuf>,
    pub xml_files: Vec<PathBuf>,
}

pub fn scan_directory(input: &Path) -> ScannedFiles {
    if input.is_file() {
        return scan_single_file(input);
    }

    let mut sql_files = Vec::new();
    let mut java_files = Vec::new();
    let mut xml_files = Vec::new();

    for entry in WalkDir::new(input).into_iter().filter_map(|e| e.ok()) {
        let path = entry.into_path();
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
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "sql" => scanned.sql_files.push(input.to_path_buf()),
        "java" => scanned.java_files.push(input.to_path_buf()),
        "xml" => scanned.xml_files.push(input.to_path_buf()),
        _ => {}
    }
    scanned
}
