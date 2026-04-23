use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use ogsql_parser::ibatis::{parse_mapper_bytes_with_path, ParsedMapper, StatementKind};

pub struct IbatisParsedFile {
    pub result: ParsedMapper,
}

pub fn load_ibatis_files(input: &Path) -> Vec<IbatisParsedFile> {
    let xml_files = collect_xml_files(input);
    let mut parsed = Vec::new();

    for path in xml_files {
        match load_ibatis_file(&path) {
            Ok(result) => parsed.push(IbatisParsedFile { result }),
            Err(e) => {
                eprintln!("warning: skipping {}: {}", path.display(), e);
            }
        }
    }

    parsed
}

fn collect_xml_files(input: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if input.is_file() {
        if input.extension().is_some_and(|ext| ext == "xml") {
            files.push(input.to_path_buf());
        }
    } else {
        for entry in WalkDir::new(input).into_iter().filter_map(|e| e.ok()) {
            let path = entry.into_path();
            if path.extension().is_some_and(|ext| ext == "xml") {
                files.push(path);
            }
        }
    }
    files
}

fn load_ibatis_file(path: &Path) -> Result<ParsedMapper, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {}", e))?;
    let file_path = path.to_string_lossy().to_string();
    let result = parse_mapper_bytes_with_path(&bytes, Some(&file_path));

    if !result.errors.is_empty() {
        eprintln!(
            "warning: {} error(s) in {}",
            result.errors.len(),
            path.display()
        );
    }

    Ok(result)
}

pub fn statement_kind_label(kind: &StatementKind) -> &'static str {
    match kind {
        StatementKind::Select => "select",
        StatementKind::Insert => "insert",
        StatementKind::Update => "update",
        StatementKind::Delete => "delete",
    }
}
