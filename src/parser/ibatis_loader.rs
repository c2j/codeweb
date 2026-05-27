use std::path::{Path, PathBuf};

use ogsql_parser::ibatis::{
    parse_mapper_bytes_structured_with_path, parse_mapper_bytes_with_path, ParsedMapper,
    StatementKind, StructuredMapper,
};

pub struct IbatisParsedFile {
    pub path: PathBuf,
    pub result: ParsedMapper,
    pub content_hash: String,
}

pub fn load_ibatis_files_from_paths(paths: &[PathBuf]) -> Vec<IbatisParsedFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| {
            load_ibatis_file(path)
                .ok()
                .map(|(result, hash)| IbatisParsedFile {
                    path: path.clone(),
                    result,
                    content_hash: hash,
                })
        })
        .collect()
}

fn load_ibatis_file(path: &Path) -> Result<(ParsedMapper, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {}", e))?;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();
    let file_path = path.to_string_lossy().to_string();
    let result = parse_mapper_bytes_with_path(&bytes, Some(&file_path));

    if result.namespace.is_empty() && result.statements.is_empty() && result.errors.is_empty() {
        crate::parse_log::info(&file_path, "skipped: not a mapper file");
        return Err("not a mapper file".to_string());
    }

    if !result.errors.is_empty() {
        for err in &result.errors {
            crate::parse_log::warn(&file_path, &err.to_string());
        }
    }

    crate::parse_log::info(
        &file_path,
        &format!(
            "namespace={}, {} statements",
            result.namespace,
            result.statements.len()
        ),
    );

    Ok((result, content_hash))
}

pub fn statement_kind_label(kind: &StatementKind) -> &'static str {
    match kind {
        StatementKind::Select => "select",
        StatementKind::Insert => "insert",
        StatementKind::Update => "update",
        StatementKind::Delete => "delete",
    }
}

pub struct IbatisStructuredFile {
    pub path: PathBuf,
    pub result: StructuredMapper,
    pub content_hash: String,
}

pub fn load_ibatis_structured_files_from_paths(paths: &[PathBuf]) -> Vec<IbatisStructuredFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| {
            load_ibatis_structured_file(path)
                .ok()
                .map(|(result, hash)| IbatisStructuredFile {
                    path: path.clone(),
                    result,
                    content_hash: hash,
                })
        })
        .collect()
}

fn load_ibatis_structured_file(path: &Path) -> Result<(StructuredMapper, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {}", e))?;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();
    let file_path = path.to_string_lossy().to_string();
    let result = parse_mapper_bytes_structured_with_path(&bytes, Some(&file_path));

    if result.namespace.is_empty() && result.statements.is_empty() && result.errors.is_empty() {
        crate::parse_log::info(&file_path, "skipped: not a mapper file (structured)");
        return Err("not a mapper file".to_string());
    }

    if !result.errors.is_empty() {
        for err in &result.errors {
            crate::parse_log::warn(&file_path, &err.to_string());
        }
    }

    crate::parse_log::info(
        &file_path,
        &format!(
            "namespace={}, {} statements (structured)",
            result.namespace,
            result.statements.len()
        ),
    );

    Ok((result, content_hash))
}
