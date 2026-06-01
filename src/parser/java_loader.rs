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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_java(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::with_suffix(".java").unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn load_java_file_annotation_extraction() {
        let source = r#"package com.example;
import org.apache.ibatis.annotations.Select;
public interface UserDao {
    @Select("SELECT * FROM users WHERE id = #{id}")
    Object findById(Long id);
}"#;
        let f = write_temp_java(source);
        let (result, hash) = load_java_file_with_config(f.path(), &JavaExtractConfig::default())
            .expect("should parse");

        assert!(
            !result.extractions.is_empty(),
            "should find SQL extractions"
        );
        let sql_found = result.extractions.iter().any(|e| e.sql.contains("SELECT"));
        assert!(sql_found, "at least one extraction should contain SELECT");
        assert!(!hash.is_empty());
    }

    #[test]
    fn load_java_file_jdbc_extraction() {
        let source = r#"package com.example;
import java.sql.*;
public class ReportDao {
    public void find() throws SQLException {
        PreparedStatement ps = conn.prepareStatement("SELECT * FROM t_reports WHERE id = ?");
        ps.setInt(1, 42);
    }
}"#;
        let f = write_temp_java(source);
        let (result, _) = load_java_file_with_config(f.path(), &JavaExtractConfig::default())
            .expect("should parse");

        assert!(
            !result.extractions.is_empty(),
            "JDBC extraction should find SQL"
        );
        let found = result
            .extractions
            .iter()
            .any(|e| e.sql.contains("SELECT") && e.sql.contains("t_reports"));
        assert!(found, "should extract SELECT with t_reports");
    }

    #[test]
    fn load_java_file_no_sql() {
        let source = r#"package com.example;
public class Util {
    public String hello() { return "world"; }
}"#;
        let f = write_temp_java(source);
        let (result, _) = load_java_file_with_config(f.path(), &JavaExtractConfig::default())
            .expect("should parse");

        assert!(
            result.extractions.is_empty(),
            "plain Java should have no extractions"
        );
    }

    #[test]
    fn parse_java_combined_both_results() {
        // Use JDBC prepareStatement which is reliably extracted
        let source = r#"package com.example;
import java.sql.*;
public class Service {
    public void doWork() throws SQLException {
        PreparedStatement ps = conn.prepareStatement("SELECT * FROM t_orders WHERE id = ?");
    }
}"#;
        let f = write_temp_java(source);
        let combined = parse_java_combined_with_config(f.path(), &JavaExtractConfig::default())
            .expect("should parse combined");

        assert!(
            !combined.sql_result.extractions.is_empty(),
            "should extract SQL from JDBC"
        );
        assert!(
            !combined.method_result.methods.is_empty(),
            "should extract methods"
        );
        assert!(
            !combined.method_result.classes.is_empty(),
            "should extract classes"
        );
        assert!(!combined.content_hash.is_empty());
    }

    #[test]
    fn extraction_method_label_correct() {
        assert_eq!(
            extraction_method_label(&ExtractionMethod::Annotation),
            "annotation"
        );
        assert_eq!(
            extraction_method_label(&ExtractionMethod::MethodCall),
            "method_call"
        );
        assert_eq!(
            extraction_method_label(&ExtractionMethod::Constant),
            "constant"
        );
    }

    #[test]
    fn load_java_file_nonexistent_path() {
        let result = load_java_file_with_config(
            Path::new("/nonexistent/File.java"),
            &JavaExtractConfig::default(),
        );
        assert!(result.is_err(), "nonexistent path should return Err");
    }

    #[test]
    fn load_java_file_jdbc_sql_has_parse_result() {
        // JDBC prepareStatement with SELECT — reliably extracted with parse_result
        let source = r#"package com.example;
import java.sql.*;
public class Dao {
    public void run() throws SQLException {
        PreparedStatement ps = conn.prepareStatement("SELECT * FROM t_users WHERE id = ?");
    }
}"#;
        let f = write_temp_java(source);
        let (result, _) = load_java_file_with_config(f.path(), &JavaExtractConfig::default())
            .expect("should parse");

        assert!(!result.extractions.is_empty(), "should find extractions");
        let with_pr = result.extractions.iter().any(|e| e.parse_result.is_some());
        assert!(with_pr, "at least one extraction should have parse_result");
    }
}
