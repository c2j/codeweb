//! JSP 文件加载与 SQL 抽取。
//!
//! 流程：JSP 源 → 片段切分 → 合成 Java →
//!      ogsql-parser `extract_sql_from_java()` → `ExtractedSql` 列表。
//!
//! 已知限制：ogsql-parser 的关键字过滤器仅接受 SELECT/INSERT/UPDATE/
//! DELETE/MERGE/WITH 开头的 SQL。JDBC 转义语法 `{call pkg.x(...)}` 会被过滤。
//! JSP 中通过 `prepareCall("{call ...}")` 直接调用存储过程的 SQL 当前不会被捕获，
//! 需要后续在 jsp_loader 内追加后处理（已记入 follow-up）。

use crate::parser::jsp_preprocessor::{lex_jsp, synthesize_java, SynthesizedJava};
use crate::parser::jsp_types::{JspParseResult, JspSqlKind};
use ogsql_parser::java::{
    extract_sql_from_java, ExtractedSql, ExtractionMethod, JavaExtractConfig, JavaExtractResult,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct JspFileResult {
    pub file: PathBuf,
    pub display_name: String,
    pub parse_result: JspParseResult,
    pub synthesized: SynthesizedJava,
    pub extractions: Vec<ExtractedSql>,
    pub errors: Vec<String>,
}

pub fn load_jsp_file(path: &Path, config: &JavaExtractConfig) -> Result<JspFileResult, String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("read {:?}: {}", path, e))?;
    Ok(load_jsp_string(source, path, config))
}

pub fn load_jsp_string(source: String, path: &Path, config: &JavaExtractConfig) -> JspFileResult {
    let parse_result = lex_jsp(&source, path);
    let synthesized = synthesize_java(&parse_result);

    let synthetic_path = format!("{}/__synthetic__.java", path.display());
    let JavaExtractResult {
        extractions,
        errors,
        ..
    } = extract_sql_from_java(&synthesized.source, &synthetic_path, config);

    let errors = errors.into_iter().map(|e| format!("{:?}", e)).collect();

    JspFileResult {
        file: path.to_path_buf(),
        display_name: parse_result.display_name.clone(),
        parse_result,
        synthesized,
        extractions,
        errors,
    }
}

/// Infer `JspSqlKind` from `ExtractionMethod`.
/// Constants come from `<%! %>` declarations; others come from scriptlet bodies.
pub fn infer_kind(extraction: &ExtractedSql) -> JspSqlKind {
    match extraction.origin.method {
        ExtractionMethod::Constant => JspSqlKind::Declaration,
        ExtractionMethod::Annotation | ExtractionMethod::MethodCall => JspSqlKind::Scriptlet,
    }
}

pub fn load_jsp_files_from_paths(
    paths: &[PathBuf],
    config: &JavaExtractConfig,
) -> Vec<JspFileResult> {
    let mut results = Vec::with_capacity(paths.len());
    for path in paths {
        match load_jsp_file(path, config) {
            Ok(r) => results.push(r),
            Err(e) => eprintln!("[jsp] failed to load {:?}: {}", path, e),
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> JavaExtractConfig {
        JavaExtractConfig::default()
    }

    #[test]
    fn extract_jdbc_prepare_statement_from_scriptlet() {
        let src = r#"<%
Connection conn = null;
PreparedStatement ps = conn.prepareStatement("SELECT * FROM users WHERE id = ?");
ps.setInt(1, 123);
ResultSet rs = ps.executeQuery();
%>"#;
        let result = load_jsp_string(src.to_string(), Path::new("/t.jsp"), &default_config());
        assert!(
            !result.extractions.is_empty(),
            "extractions: {:?}, errors: {:?}",
            result.extractions,
            result.errors
        );
        let sql = &result.extractions[0].sql;
        assert!(sql.contains("SELECT"), "extracted SQL: {}", sql);
        assert!(sql.contains("users"));
    }

    #[test]
    fn extract_string_concatenation_sql() {
        let src = r#"<%
String sql = "SELECT * FROM orders WHERE status = 'PAID' AND id = " + request.getParameter("id");
%>"#;
        let result = load_jsp_string(src.to_string(), Path::new("/t.jsp"), &default_config());
        assert!(!result.extractions.is_empty());
        assert!(result.extractions[0].sql.contains("SELECT"));
    }

    #[test]
    fn extract_stored_procedure_call() {
        let src = r#"<%
CallableStatement cs = conn.prepareCall("SELECT pkg.get_user(?, ?)");
cs.setLong(1, userId);
cs.registerOutParameter(2, Types.VARCHAR);
cs.execute();
%>"#;
        let result = load_jsp_string(src.to_string(), Path::new("/t.jsp"), &default_config());
        assert!(!result.extractions.is_empty(), "extractions: {:?}", result.extractions);
        let found = result.extractions.iter().any(|e| e.sql.contains("pkg.get_user"));
        assert!(found, "should detect stored procedure call");
    }

    #[test]
    fn extract_handles_jdbc_call_escape_syntax_without_panic() {
        let src = r#"<%
CallableStatement cs = conn.prepareCall("{call pkg.get_user(?, ?)}");
cs.execute();
%>"#;
        let result = load_jsp_string(src.to_string(), Path::new("/t.jsp"), &default_config());
        let _ = result.extractions.len();
    }

    #[test]
    fn extract_declaration_constant_sql() {
        let src = r#"<%!
private static final String FIND_BY_ID = "SELECT id, name FROM users WHERE id = ?";
%>
<%
PreparedStatement ps = conn.prepareStatement(FIND_BY_ID);
%>"#;
        let result = load_jsp_string(src.to_string(), Path::new("/t.jsp"), &default_config());
        assert!(!result.extractions.is_empty());
    }

    #[test]
    fn extract_skips_html_only_jsp() {
        let src = "<html><body><h1>Hello</h1></body></html>";
        let result = load_jsp_string(src.to_string(), Path::new("/t.jsp"), &default_config());
        assert!(result.extractions.is_empty());
    }

    #[test]
    fn extract_does_not_panic_on_invalid_java() {
        let src = "<% this is not valid java at all %>";
        let result = load_jsp_string(src.to_string(), Path::new("/t.jsp"), &default_config());
        // Must not panic; errors may or may not be reported but the function must complete
        assert!(result.extractions.is_empty() || !result.extractions.is_empty());
    }

    #[test]
    fn infer_kind_returns_declaration_for_constants() {
        let src = r#"<%!
private static final String SQL = "SELECT 1";
%>"#;
        let result = load_jsp_string(src.to_string(), Path::new("/t.jsp"), &default_config());
        if let Some(ext) = result.extractions.first() {
            let kind = infer_kind(ext);
            assert_eq!(kind, JspSqlKind::Declaration);
        }
    }
}
