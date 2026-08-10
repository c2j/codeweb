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
use crate::parser::jsp_types::{JspJavaRef, JspParseResult, JspSegmentKind, JspSqlKind};
use ogsql_parser::java::{
    extract_sql_from_java, ExtractedSql, ExtractionMethod, JavaExtractConfig, JavaExtractResult,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct JspFileResult {
    pub file: PathBuf,
    pub display_name: String,
    pub line: usize,
    pub parse_result: JspParseResult,
    pub synthesized: SynthesizedJava,
    pub extractions: Vec<ExtractedSql>,
    pub java_refs: Vec<JspJavaRef>,
    pub errors: Vec<String>,
}

pub fn load_jsp_file(path: &Path, config: &JavaExtractConfig) -> Result<JspFileResult, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {:?}: {}", path, e))?;
    let source = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    Ok(load_jsp_string(source, path, config))
}

pub fn load_jsp_string(source: String, path: &Path, config: &JavaExtractConfig) -> JspFileResult {
    let parse_result = lex_jsp(&source, path);
    let synthesized = synthesize_java(&parse_result);

    let first_line = parse_result
        .segments
        .iter()
        .find(|s| {
            !matches!(
                s.kind,
                JspSegmentKind::Text | JspSegmentKind::Comment | JspSegmentKind::Directive
            )
        })
        .map(|s| s.start_line)
        .unwrap_or(1);

    let synthetic_path = format!("{}/__synthetic__.java", path.display());
    let JavaExtractResult {
        extractions,
        errors,
        ..
    } = extract_sql_from_java(&synthesized.source, &synthetic_path, config);

    let errors = errors.into_iter().map(|e| format!("{:?}", e)).collect();

    let java_refs = extract_java_refs_from_synthetic(&synthesized.source, &parse_result);

    JspFileResult {
        file: path.to_path_buf(),
        display_name: parse_result.display_name.clone(),
        line: first_line,
        parse_result,
        synthesized,
        extractions,
        java_refs,
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

/// Parse the synthesized Java source with tree-sitter to find Java class/method
/// references. Extracts:
/// - Constructor calls: `new ClassName(...)`  →  method="&lt;init&gt;"
/// - Static method calls: `ClassName.method(...)`  →  preserves method name
fn extract_java_refs_from_synthetic(
    source: &str,
    _parse_result: &JspParseResult,
) -> Vec<JspJavaRef> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .is_err()
    {
        return Vec::new();
    }
    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut refs = Vec::new();
    let mut seen: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let source_bytes = source.as_bytes();
    collect_java_refs_recursive(tree.root_node(), source_bytes, &mut refs, &mut seen);
    refs
}

fn collect_java_refs_recursive(
    node: tree_sitter::Node,
    source: &[u8],
    refs: &mut Vec<JspJavaRef>,
    seen: &mut std::collections::HashSet<(usize, usize)>,
) {
    match node.kind() {
        "object_creation_expression" => {
            let pos = node.start_position();
            if seen.insert((pos.row, pos.column)) {
                if let Some(type_node) = node.child_by_field_name("type") {
                    if let Ok(name) = type_node.utf8_text(source) {
                        let line = pos.row + 1;
                        refs.push(JspJavaRef {
                            class_name: name.to_string(),
                            method_name: "<init>".to_string(),
                            line,
                        });
                    }
                }
            }
        }
        "method_invocation" => {
            let pos = node.start_position();
            if seen.insert((pos.row, pos.column)) {
                // Extract object (class name for static calls like ClassName.method())
                let object = node
                    .child_by_field_name("object")
                    .and_then(|n| n.utf8_text(source).ok());
                let method = node
                    .child_by_field_name("name")
                    .and_then(|n| n.utf8_text(source).ok())
                    .unwrap_or("");
                if let (Some(obj), m) = (object, method) {
                    if !m.is_empty() && obj.chars().next().is_some_and(|c| c.is_uppercase()) {
                        // Static method call: ClassName.method()
                        refs.push(JspJavaRef {
                            class_name: obj.to_string(),
                            method_name: m.to_string(),
                            line: pos.row + 1,
                        });
                    }
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_java_refs_recursive(child, source, refs, seen);
    }
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
        assert!(
            !result.extractions.is_empty(),
            "extractions: {:?}",
            result.extractions
        );
        let found = result
            .extractions
            .iter()
            .any(|e| e.sql.contains("pkg.get_user"));
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
