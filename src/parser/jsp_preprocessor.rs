//! JSP 预处理器：将 JSP 源码切分为片段。
//!
//! 仅做片段级切分（不做完整 JSP 语法分析）。
//! 后续 `synthesize_java` 把片段缝合为合法 Java 源以供 ogsql-parser 处理。

use crate::parser::jsp_types::{JspParseResult, JspSegment, JspSegmentKind};
use std::path::{Path, PathBuf};

pub struct JspLexer<'a> {
    source: &'a [u8],
    pos: usize,
    line: usize,
}

impl<'a> JspLexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            pos: 0,
            line: 1,
        }
    }

    pub fn tokenize(&mut self) -> Vec<JspSegment> {
        let mut segments = Vec::new();
        let mut text_start = self.pos;
        let mut text_start_line = self.line;

        while self.pos < self.source.len() {
            if self.starts_with(b"<%") {
                if self.pos > text_start {
                    self.push_text_segment(&mut segments, text_start, self.pos, text_start_line);
                }

                let seg_start_line = self.line;
                if let Some(seg) = self.read_jsp_tag(seg_start_line) {
                    segments.push(seg);
                }
                text_start = self.pos;
                text_start_line = self.line;
            } else {
                if self.source[self.pos] == b'\n' {
                    self.line += 1;
                }
                self.pos += 1;
            }
        }

        if self.pos > text_start {
            self.push_text_segment(&mut segments, text_start, self.pos, text_start_line);
        }

        segments
    }

    fn push_text_segment(
        &self,
        out: &mut Vec<JspSegment>,
        start: usize,
        end: usize,
        start_line: usize,
    ) {
        let raw = std::str::from_utf8(&self.source[start..end])
            .unwrap_or("<invalid utf8>")
            .to_string();
        out.push(JspSegment {
            kind: JspSegmentKind::Text,
            raw: raw.clone(),
            content: raw,
            start_line,
            end_line: self.line,
        });
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.source[self.pos..].starts_with(prefix)
    }

    fn read_jsp_tag(&mut self, start_line: usize) -> Option<JspSegment> {
        debug_assert!(self.starts_with(b"<%"));

        if self.starts_with(b"<%--") {
            self.read_until_close("--%>", start_line, JspSegmentKind::Comment, 4)
        } else if self.starts_with(b"<%@") {
            self.read_until_close("%>", start_line, JspSegmentKind::Directive, 3)
        } else if self.starts_with(b"<%=") {
            self.read_until_close("%>", start_line, JspSegmentKind::Expression, 3)
        } else if self.starts_with(b"<%!") {
            self.read_until_close("%>", start_line, JspSegmentKind::Declaration, 3)
        } else {
            self.read_until_close("%>", start_line, JspSegmentKind::Scriptlet, 2)
        }
    }

    fn read_until_close(
        &mut self,
        close_marker: &str,
        start_line: usize,
        kind: JspSegmentKind,
        skip_prefix: usize,
    ) -> Option<JspSegment> {
        let tag_open_start = self.pos;
        let content_start = self.pos + skip_prefix;
        let close_bytes = close_marker.as_bytes();

        for _ in 0..skip_prefix {
            if self.pos < self.source.len() && self.source[self.pos] == b'\n' {
                self.line += 1;
            }
            self.pos += 1;
        }

        let mut search = self.pos;
        while search + close_bytes.len() <= self.source.len() {
            if &self.source[search..search + close_bytes.len()] == close_bytes {
                let content = std::str::from_utf8(&self.source[content_start..search])
                    .unwrap_or("<invalid utf8>")
                    .to_string();
                let raw_end = search + close_bytes.len();
                let raw = std::str::from_utf8(&self.source[tag_open_start..raw_end])
                    .unwrap_or("<invalid utf8>")
                    .to_string();

                for b in &self.source[self.pos..raw_end] {
                    if *b == b'\n' {
                        self.line += 1;
                    }
                }
                self.pos = raw_end;

                return Some(JspSegment {
                    kind,
                    raw,
                    content: content.trim().to_string(),
                    start_line,
                    end_line: self.line,
                });
            }
            search += 1;
        }

        let content = std::str::from_utf8(&self.source[content_start..])
            .unwrap_or("<invalid utf8>")
            .to_string();
        let raw_end = self.source.len();
        let raw = std::str::from_utf8(&self.source[tag_open_start..raw_end])
            .unwrap_or("<invalid utf8>")
            .to_string();
        for b in &self.source[self.pos..] {
            if *b == b'\n' {
                self.line += 1;
            }
        }
        self.pos = self.source.len();
        Some(JspSegment {
            kind,
            raw,
            content: content.trim().to_string(),
            start_line,
            end_line: self.line,
        })
    }
}

pub fn lex_jsp(source: &str, file: &Path) -> JspParseResult {
    let mut lexer = JspLexer::new(source);
    let segments = lexer.tokenize();
    let display_name = compute_display_name(file);

    let mut page_directives = Vec::new();
    for seg in &segments {
        if seg.kind == JspSegmentKind::Directive && seg.content.starts_with("page") {
            for attr in ["session", "contentType", "import"] {
                if let Some(v) = extract_attr(&seg.content, attr) {
                    page_directives.push((attr.to_string(), v));
                }
            }
        }
    }

    JspParseResult {
        file: file.to_path_buf(),
        display_name,
        segments,
        page_directives,
        warnings: Vec::new(),
    }
}

fn extract_attr(content: &str, name: &str) -> Option<String> {
    let pat = format!("{}=\"", name);
    let start = content.find(&pat)? + pat.len();
    let rest = &content[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Compute a human-readable display_name for a JSP file.
///
/// Priority:
/// 1. Relative to WEB-INF directory (if path contains WEB-INF)
/// 2. Relative to git repository root (if inside a git working tree)
/// 3. Fallback to file name only
pub fn compute_display_name(file: &Path) -> String {
    // 1. Try WEB-INF (most meaningful for JSP files)
    let path_str = file.to_string_lossy();
    if let Some(pos) = path_str.find("WEB-INF") {
        let after = &path_str[pos + "WEB-INF".len()..];
        let trimmed = after.trim_start_matches(std::path::MAIN_SEPARATOR);
        return trimmed.to_string();
    }

    // 2. Try git root
    if let Some(git_root) = find_git_root(file) {
        if let Ok(rel) = file.strip_prefix(&git_root) {
            return rel.to_string_lossy().to_string();
        }
    }

    // 3. Fallback: file name only
    file.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.jsp")
        .to_string()
}

/// Walk up from `file` to find the nearest `.git` directory, returning the parent of `.git`.
fn find_git_root(file: &Path) -> Option<PathBuf> {
    let mut current = file.parent()?;
    for _ in 0..16 {
        if current.join(".git").is_dir() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
    None
}

#[derive(Debug, Clone)]
pub struct SynthesizedJava {
    pub source: String,
    pub class_name: String,
}

/// Stitch JSP segments into a parseable Java source so that
/// `ogsql_parser::java::extract_sql_from_java` can walk it.
///
/// - Declarations (`<%! %>`) go to class top-level
/// - Scriptlets (`<% %>`) and Expressions (`<%= %>`) go to `_jspService` body, in order
/// - EL `${foo}` becomes a string literal placeholder `"<EL_FOO>"`
/// - Expressions `<%= e %>` become `out.print(e);`
pub fn synthesize_java(parsed: &JspParseResult) -> SynthesizedJava {
    let path_str = parsed.file.to_string_lossy().to_string();
    let hash = blake3::hash(path_str.as_bytes());
    let hex = hash.to_hex();
    let suffix = &hex.as_str()[..8];
    let class_name = format!("__JspPage_{}", suffix);

    let mut class_body = String::new();
    let mut service_body = String::new();

    for seg in &parsed.segments {
        match seg.kind {
            JspSegmentKind::Declaration => {
                class_body.push_str(&replace_el(&seg.content));
                class_body.push('\n');
            }
            JspSegmentKind::Scriptlet => {
                service_body.push_str(&replace_el(&seg.content));
                service_body.push('\n');
            }
            JspSegmentKind::Expression => {
                let expr = replace_el(&seg.content);
                service_body.push_str(&format!("out.print({});\n", expr.trim()));
            }
            JspSegmentKind::Text
            | JspSegmentKind::Directive
            | JspSegmentKind::Comment
            | JspSegmentKind::JstlSql => {}
        }
    }

    let source = format!(
        r#"package __jsp_synthetic__;

import java.sql.*;
import javax.servlet.*;
import javax.servlet.http.*;
import javax.servlet.jsp.*;

public class {class_name} {{
{class_body}
    public void _jspService(
            HttpServletRequest request,
            HttpServletResponse response,
            PageContext pageContext,
            HttpSession session,
            ServletContext application,
            JspWriter out) throws Throwable {{
{service_body}
    }}
}}
"#,
        class_name = class_name,
        class_body = indent(&class_body, "    "),
        service_body = indent(&service_body, "        "),
    );

    SynthesizedJava { source, class_name }
}

/// Replace `${...}` EL expressions with Java string-literal placeholders.
/// `${param.id}` -> `"<EL_PARAM_ID>"` (uppercased, non-alphanumerics -> `_`).
fn replace_el(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = s[i + 2..].find('}') {
                let expr = &s[i + 2..i + 2 + end];
                let ident: String = expr
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '_' {
                            c.to_ascii_uppercase()
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let trimmed = ident.trim_start_matches('_');
                out.push_str(&format!("\"<EL_{}>\"", trimmed));
                i = i + 2 + end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{}{}", prefix, line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_plain_html_only_text() {
        let src = "<html><body>hello</body></html>";
        let result = lex_jsp(src, Path::new("test.jsp"));
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].kind, JspSegmentKind::Text);
    }

    #[test]
    fn lex_scriptlet_basic() {
        let src = "<% String sql = \"SELECT 1\"; %>";
        let result = lex_jsp(src, Path::new("test.jsp"));
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].kind, JspSegmentKind::Scriptlet);
        assert_eq!(result.segments[0].content, "String sql = \"SELECT 1\";");
        assert_eq!(result.segments[0].start_line, 1);
    }

    #[test]
    fn lex_declaration() {
        let src = "<%! private static final String X = \"1\"; %>";
        let result = lex_jsp(src, Path::new("test.jsp"));
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].kind, JspSegmentKind::Declaration);
    }

    #[test]
    fn lex_expression() {
        let src = "<p>Hello <%= user.getName() %></p>";
        let result = lex_jsp(src, Path::new("test.jsp"));
        assert_eq!(result.segments.len(), 3);
        assert_eq!(result.segments[1].kind, JspSegmentKind::Expression);
        assert_eq!(result.segments[1].content, "user.getName()");
    }

    #[test]
    fn lex_directive_page() {
        let src = "<%@ page import=\"java.sql.*\" session=\"false\" %>";
        let result = lex_jsp(src, Path::new("test.jsp"));
        assert_eq!(result.segments[0].kind, JspSegmentKind::Directive);
        assert!(result.page_directives.iter().any(|(k, _)| k == "session"));
    }

    #[test]
    fn lex_comment_skipped_from_content() {
        let src = "<%-- this is a comment --%><% int x = 1; %>";
        let result = lex_jsp(src, Path::new("test.jsp"));
        assert_eq!(result.segments.len(), 2);
        assert_eq!(result.segments[0].kind, JspSegmentKind::Comment);
        assert_eq!(result.segments[1].kind, JspSegmentKind::Scriptlet);
    }

    #[test]
    fn lex_multiline_tracks_line_numbers() {
        let src = "<%\nString a;\nString b;\n%>";
        let result = lex_jsp(src, Path::new("test.jsp"));
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].start_line, 1);
        assert_eq!(result.segments[0].end_line, 4);
    }

    #[test]
    fn lex_mixed_html_and_scriptlets() {
        let src = r#"
<html>
<body>
<%
String sql = "SELECT * FROM users";
PreparedStatement ps = conn.prepareStatement(sql);
%>
<table>...</table>
</body>
</html>
"#;
        let result = lex_jsp(src, Path::new("test.jsp"));
        assert!(result
            .segments
            .iter()
            .any(|s| s.kind == JspSegmentKind::Scriptlet));
        let scriptlet = result
            .segments
            .iter()
            .find(|s| s.kind == JspSegmentKind::Scriptlet)
            .unwrap();
        assert!(scriptlet.content.contains("prepareStatement"));
    }

    #[test]
    fn lex_unterminated_scriptlet_falls_back_gracefully() {
        let src = "<% String sql = \"...";
        let result = lex_jsp(src, Path::new("test.jsp"));
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].kind, JspSegmentKind::Scriptlet);
    }

    #[test]
    fn lex_split_statement_across_blocks() {
        let src = "<% if (cond) { %> <b>some html</b> <% } %>";
        let result = lex_jsp(src, Path::new("test.jsp"));
        let scriptlets: Vec<_> = result
            .segments
            .iter()
            .filter(|s| s.kind == JspSegmentKind::Scriptlet)
            .collect();
        assert_eq!(scriptlets.len(), 2);
        assert_eq!(scriptlets[0].content, "if (cond) {");
        assert_eq!(scriptlets[1].content, "}");
    }
}

#[cfg(test)]
mod synthesize_tests {
    use super::*;

    fn parse_and_synthesize(src: &str) -> SynthesizedJava {
        let parsed = lex_jsp(src, Path::new("/test/sample.jsp"));
        synthesize_java(&parsed)
    }

    #[test]
    fn synthesize_produces_valid_class_skeleton() {
        let syn = parse_and_synthesize("<% int x = 1; %>");
        assert!(syn.source.contains("public class __JspPage_"));
        assert!(syn.source.contains("_jspService"));
        assert!(syn.source.contains("int x = 1;"));
    }

    #[test]
    fn synthesize_declaration_goes_to_class_level() {
        let src = "<%! private static final String SQL = \"SELECT 1\"; %>";
        let syn = parse_and_synthesize(src);
        let decl_pos = syn.source.find("private static final String SQL");
        let service_pos = syn.source.find("_jspService").unwrap();
        assert!(decl_pos.unwrap() < service_pos);
    }

    #[test]
    fn synthesize_expression_becomes_out_print() {
        let src = "<p>Hello <%= user.getName() %></p>";
        let syn = parse_and_synthesize(src);
        assert!(syn.source.contains("out.print(user.getName());"));
    }

    #[test]
    fn synthesize_el_expression_replaced() {
        let src = "<% String sql = \"WHERE id=\" + ${param.id}; %>";
        let syn = parse_and_synthesize(src);
        assert!(syn.source.contains("WHERE id="));
        assert!(
            !syn.source.contains("${"),
            "EL markers must be stripped: {}",
            syn.source
        );
    }

    #[test]
    fn synthesize_split_scriptlets_preserve_order() {
        let src = "<% if (x > 0) { %><b>positive</b><% } else { %><b>else</b><% } %>";
        let syn = parse_and_synthesize(src);
        let if_pos = syn.source.find("if (x > 0)").unwrap();
        let else_pos = syn.source.find("} else {").unwrap();
        assert!(if_pos < else_pos);
    }

    #[test]
    fn synthesize_produces_parseable_java() {
        let src = r#"<%!
private static final String SQL = "SELECT * FROM users";
%>
<%
Connection conn = DriverManager.getConnection("...");
PreparedStatement ps = conn.prepareStatement(SQL);
ResultSet rs = ps.executeQuery();
%>"#;
        let syn = parse_and_synthesize(src);

        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(&syn.source, None);
        assert!(tree.is_some(), "tree-sitter should parse synthesized Java");
        let tree = tree.unwrap();
        let root = tree.root_node();
        assert!(
            !root.has_error(),
            "synthesized Java should parse without errors:\n{}",
            syn.source
        );
    }
}
