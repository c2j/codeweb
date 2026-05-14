use std::path::{Path, PathBuf};

use ogsql_parser::java::{
    extract_sql_from_java, ExtractionMethod, JavaExtractConfig, JavaExtractResult,
};

pub struct JavaParsedFile {
    pub path: PathBuf,
    pub result: JavaExtractResult,
    pub content_hash: String,
}

/// Combined result from a single-pass Java parse (SQL extraction + method extraction).
pub struct JavaCombinedResult {
    pub sql_result: JavaExtractResult,
    pub method_result: crate::parser::java_method::JavaParseResult,
    pub content_hash: String,
}

pub fn load_java_files_from_paths(paths: &[PathBuf]) -> Vec<JavaParsedFile> {
    load_java_files_from_paths_with_config(paths, &JavaExtractConfig::default())
}

pub fn load_java_files_from_paths_with_config(
    paths: &[PathBuf],
    config: &JavaExtractConfig,
) -> Vec<JavaParsedFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| {
            load_java_file_with_config(path, config)
                .ok()
                .map(|(result, hash)| JavaParsedFile {
                    path: path.clone(),
                    result,
                    content_hash: hash,
                })
        })
        .collect()
}

/// Parse all Java files in a single pass: reads each file once, runs both SQL extraction
/// and tree-sitter method extraction, returns combined results with content hash.
pub fn load_java_files_combined(paths: &[PathBuf]) -> Vec<(PathBuf, JavaCombinedResult)> {
    load_java_files_combined_with_config(paths, &JavaExtractConfig::default())
}

pub fn load_java_files_combined_with_config(
    paths: &[PathBuf],
    config: &JavaExtractConfig,
) -> Vec<(PathBuf, JavaCombinedResult)> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| {
            parse_java_combined_with_config(path, config)
                .ok()
                .map(|combined| (path.clone(), combined))
        })
        .collect()
}

fn load_java_file_with_config(
    path: &Path,
    config: &JavaExtractConfig,
) -> Result<(JavaExtractResult, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {}", e))?;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();
    let source = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    let file_path = path.to_string_lossy();
    let result = extract_sql_from_java(&source, &file_path, config);

    if !result.errors.is_empty() {
        for err in &result.errors {
            crate::parse_log::warn(&file_path, &err.to_string());
        }
    }

    crate::parse_log::info(
        &file_path,
        &format!("{} SQL extractions", result.extractions.len()),
    );

    Ok((result, content_hash))
}

/// Single-pass Java parsing: reads file once, runs both SQL extraction and method extraction.
fn parse_java_combined_with_config(
    path: &Path,
    config: &JavaExtractConfig,
) -> Result<JavaCombinedResult, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {}", e))?;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();
    let source = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    let file_path_str = path.to_string_lossy();

    let sql_result = extract_sql_from_java(&source, &file_path_str, config);

    if !sql_result.errors.is_empty() {
        for err in &sql_result.errors {
            crate::parse_log::warn(&file_path_str, &err.to_string());
        }
    }
    crate::parse_log::info(
        &file_path_str,
        &format!("{} SQL extractions", sql_result.extractions.len()),
    );

    // Method extraction via tree-sitter
    let method_result = crate::parser::java_method::parse_java_source(path, source.as_bytes())?;

    crate::parse_log::info(
        &file_path_str,
        &format!(
            "{} classes, {} methods",
            method_result.classes.len(),
            method_result.methods.len()
        ),
    );

    Ok(JavaCombinedResult {
        sql_result,
        method_result,
        content_hash,
    })
}

pub fn extraction_method_label(method: &ExtractionMethod) -> &'static str {
    match method {
        ExtractionMethod::Annotation => "annotation",
        ExtractionMethod::MethodCall => "method_call",
        ExtractionMethod::Constant => "constant",
    }
}
