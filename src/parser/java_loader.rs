use std::path::{Path, PathBuf};

use ogsql_parser::java::{
    extract_sql_from_java, ExtractionMethod, JavaExtractConfig, JavaExtractResult,
};

pub struct JavaParsedFile {
    pub result: JavaExtractResult,
}

pub fn load_java_files_from_paths(paths: &[PathBuf]) -> Vec<JavaParsedFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| {
            load_java_file(path)
                .ok()
                .map(|result| JavaParsedFile { result })
        })
        .collect()
}

fn load_java_file(path: &Path) -> Result<JavaExtractResult, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {}", e))?;
    let source = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    let file_path = path.to_string_lossy();
    let config = JavaExtractConfig::default();
    let result = extract_sql_from_java(&source, &file_path, &config);

    if !result.errors.is_empty() {
        for err in &result.errors {
            crate::parse_log::warn(&file_path, &err.to_string());
        }
    }

    crate::parse_log::info(
        &file_path,
        &format!("{} SQL extractions", result.extractions.len()),
    );

    Ok(result)
}

pub fn extraction_method_label(method: &ExtractionMethod) -> &'static str {
    match method {
        ExtractionMethod::Annotation => "annotation",
        ExtractionMethod::MethodCall => "method_call",
        ExtractionMethod::Constant => "constant",
    }
}
