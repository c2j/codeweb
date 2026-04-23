use std::path::{Path, PathBuf};

use ogsql_parser::java::{
    extract_sql_from_java, ExtractionMethod, JavaExtractConfig, JavaExtractResult,
};

pub struct JavaParsedFile {
    pub result: JavaExtractResult,
}

pub fn load_java_files_from_paths(paths: &[PathBuf]) -> Vec<JavaParsedFile> {
    let mut parsed = Vec::new();
    for path in paths {
        match load_java_file(path) {
            Ok(result) => parsed.push(JavaParsedFile { result }),
            Err(e) => {
                eprintln!("warning: skipping {}: {}", path.display(), e);
            }
        }
    }
    parsed
}

fn load_java_file(path: &Path) -> Result<JavaExtractResult, String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("read error: {}", e))?;
    let file_path = path.to_string_lossy();
    let config = JavaExtractConfig::default();
    let result = extract_sql_from_java(&source, &file_path, &config);

    if !result.errors.is_empty() {
        eprintln!(
            "warning: {} error(s) in {}",
            result.errors.len(),
            path.display()
        );
    }

    Ok(result)
}

pub fn extraction_method_label(method: &ExtractionMethod) -> &'static str {
    match method {
        ExtractionMethod::Annotation => "annotation",
        ExtractionMethod::MethodCall => "method_call",
        ExtractionMethod::Constant => "constant",
    }
}
