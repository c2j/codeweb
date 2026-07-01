//! JSP 解析过程中的中间数据结构。
//!
//! 这些类型仅用于 cobweb 侧的 JSP 预处理，
//! 最终会通过 ogsql-parser 的 `extract_sql_from_java()` 转化为
//! `StatementInfo`。

use std::path::PathBuf;

/// JSP 片段类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JspSegmentKind {
    /// HTML / 纯文本（被剥离，不进入合成 Java）
    Text,
    /// `<% ... %>` scriptlet（合成到 `_jspService` 方法体）
    Scriptlet,
    /// `<%! ... %>` declaration（合成到 class 顶层）
    Declaration,
    /// `<%= ... %>` expression（合成到方法体，包裹为 `out.print(...)`）
    Expression,
    /// `<%@ ... %>` directive（仅记录，不进入合成 Java）
    Directive,
    /// `<sql:query ... />` 或 `<sql:update ... />` JSTL SQL 标签
    JstlSql,
    /// 注释 `<%-- ... --%>`（剥离）
    Comment,
}

/// 从 JSP 源码中切分出的一个片段
#[derive(Debug, Clone)]
pub struct JspSegment {
    pub kind: JspSegmentKind,
    /// 片段原始文本（含标签，如 `<% String sql="x"; %>`）
    pub raw: String,
    /// 片段内部内容（去除外层标签，如 `String sql="x";`）
    pub content: String,
    /// 在 JSP 文件中的起始行号（1-based）
    pub start_line: usize,
    /// 在 JSP 文件中的结束行号（1-based，含）
    pub end_line: usize,
}

/// 单个 JSP 文件的解析结果
#[derive(Debug, Clone)]
pub struct JspParseResult {
    pub file: PathBuf,
    pub display_name: String,
    /// 按出现顺序排列的所有片段
    pub segments: Vec<JspSegment>,
    /// `<%@ page %>` 中提取的 info（如 session=true/false），保留扩展位
    pub page_directives: Vec<(String, String)>,
    /// 解析过程中产生的告警（不致命）
    pub warnings: Vec<String>,
}

/// A reference from JSP to a Java class/method (extracted from synthesized Java).
#[derive(Debug, Clone)]
pub struct JspJavaRef {
    /// Simple class name as it appears in the JSP (e.g. "FilmJdbcDao").
    pub class_name: String,
    /// Method name: actual method name, or "&lt;init&gt;" for constructor calls.
    pub method_name: String,
    /// Line in the synthesized Java source (approximate).
    pub line: usize,
}

/// JSP SQL 的来源子类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum JspSqlKind {
    Scriptlet,
    Declaration,
    JstlQuery,
    JstlUpdate,
}

impl JspSqlKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            JspSqlKind::Scriptlet => "scriptlet",
            JspSqlKind::Declaration => "declaration",
            JspSqlKind::JstlQuery => "jstl_query",
            JspSqlKind::JstlUpdate => "jstl_update",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsp_sql_kind_as_str_roundtrip() {
        assert_eq!(JspSqlKind::Scriptlet.as_str(), "scriptlet");
        assert_eq!(JspSqlKind::Declaration.as_str(), "declaration");
        assert_eq!(JspSqlKind::JstlQuery.as_str(), "jstl_query");
        assert_eq!(JspSqlKind::JstlUpdate.as_str(), "jstl_update");
    }

    #[test]
    fn jsp_segment_kind_eq() {
        assert_eq!(JspSegmentKind::Scriptlet, JspSegmentKind::Scriptlet);
        assert_ne!(JspSegmentKind::Scriptlet, JspSegmentKind::Declaration);
    }
}
