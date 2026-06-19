//! JSP 预处理器：将 JSP 源码切分为片段。
//!
//! 仅做片段级切分（不做完整 JSP 语法分析）。
//! 后续 `synthesize_java` 把片段缝合为合法 Java 源以供 ogsql-parser 处理。

use crate::parser::jsp_types::{JspParseResult, JspSegment, JspSegmentKind};
use std::path::Path;

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
    let display_name = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.jsp")
        .to_string();

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
