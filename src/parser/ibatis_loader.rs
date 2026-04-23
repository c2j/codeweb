use std::path::{Path, PathBuf};

use ogsql_parser::ibatis::{parse_mapper_bytes_with_path, ParsedMapper, StatementKind};

pub struct IbatisParsedFile {
    pub result: ParsedMapper,
}

pub fn load_ibatis_files_from_paths(paths: &[PathBuf]) -> Vec<IbatisParsedFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| {
            load_ibatis_file(path)
                .ok()
                .map(|result| IbatisParsedFile { result })
        })
        .collect()
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
