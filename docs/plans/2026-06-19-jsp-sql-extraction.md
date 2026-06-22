# JSP SQL 抽取与图谱集成 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 从 `.jsp` 源文件中提取内嵌 SQL（scriptlet JDBC 调用、字符串拼接、JSTL `<sql:query>` 标签），复用 ogsql-parser 的 `extract_sql_from_java()` 完成解析，并把 `JspPage` / `JspSql` 节点纳入现有调用图谱，使 Phase 4 的 `trace()` / `impact()` 能够反向追溯到 JSP 入口。

**Architecture:** 在 `src/parser/` 下新增 `jsp_loader.rs` 模块，实现三层管线：(1) JSP 预处理器将 JSP 切分为 scriptlet / declaration / expression / directive / JSTL 片段，剥离 HTML/EL 并合成合法 Java 源；(2) 直接复用 ogsql-parser 的 `extract_sql_from_java()`，零改动获得与 `.java` 文件相同的 SQL 检测能力（JDBC 方法、注解、字符串拼接、StringBuilder 等）；(3) `GraphBuilder` 新增 `add_jsp_nodes_from_parsed()` 把结果挂到图上。JSP 支持放在可选 Cargo feature `jsp` 后面，默认关闭，对现有用户零影响。

**Tech Stack:** Rust, ogsql-parser（已有 `java` feature）, tree-sitter-java（已有，经 ogsql-parser 间接复用）, petgraph, 现有 cobweb 模块。**无新依赖。**

---

## 背景与动机

### 为什么需要 JSP 支持

cobweb 围绕 openGauss/GaussDB 构建，主要服务国产化迁移与 legacy 系统重构场景。这类项目里 JSP 内嵌 SQL（通过 `<% %>` scriptlet 直接 JDBC 调用）是**普遍现象**而非边缘 case —— 银行、电信、政企的早期 Java Web 后台几乎都有。

### 现状缺口

Phase 1–4 已实现 SQL / iBatis XML / Java 三源解析，但 `src/parser/scanner.rs` 当前只识别 `.sql / .java / .xml`。某些存储过程的**唯一调用入口就在 JSP** 里 —— 缺失 JSP 会导致：
- `trace --from <proc>` 反向追溯在 Web 层断链
- `impact()` 变更影响分析漏报真实入口
- 整体图谱对 legacy 项目不完整

### 与既有架构的关系

JSP 是 Phase 3（Java Method Extraction）的自然延伸 —— JSP 本质是"带 HTML 包装的 Java 片段"，scriptlet 里的 JDBC 调用模式与 `.java` 文件**完全一致**（`prepareStatement` / `executeQuery` / 字符串拼接 / StringBuilder）。ogsql-parser 的 `extract_sql_from_java()` 内部走 tree-sitter-java AST，已识别 11+ 类 SQL 载体模式，复用率预估 70–80%。

### 设计原则

1. **可选 feature**：`jsp` feature 默认关闭，避免影响纯 Java/SQL 用户
2. **零 ogsql-parser 改动**：JSP 预处理在 cobweb 侧完成，输出合法 Java 源喂给既有 API
3. **遵循既有 loader 模式**：参照 `ibatis_loader.rs` 的结构，让代码风格一致
4. **MVP 优先**：先做 scriptlet JDBC 抽取（覆盖大多数场景），JSTL SQL 标签作为可选子任务

---

## 范围

### MVP 范围（必做）

| 能力 | 说明 |
|---|---|
| Scriptlet JDBC 抽取 | `<% Connection conn=...; PreparedStatement ps=conn.prepareStatement("SELECT..."); %>` |
| Scriptlet 字符串拼接 | `<% String sql = "SELECT * FROM t WHERE id=" + request.getParameter("id"); %>` |
| Scriptlet StringBuilder | `<% StringBuilder sb = new StringBuilder(); sb.append("CALL pkg.x()"); %>` |
| Declaration 抽取 | `<%! private static final String SQL = "SELECT ..."; %>` |
| Expression 占位 | `<%= ... %>` 转译为合成方法内的表达式语句 |
| EL 占位替换 | `${param.id}` → `__JSP_EL_PARAM_ID__` 占位符 |
| 跨 `<% %>` 块缝合 | 同一语句被 HTML 切开时按顺序拼接 |

### 可选范围（JSTL 子任务）

| 能力 | 说明 |
|---|---|
| JSTL `<sql:query>` | `<sql:query var="result" sql="SELECT * FROM ..." />` |
| JSTL `<sql:update>` | `<sql:update>UPDATE t SET ...</sql:update>` |
| JSTL `<sql:query>` body SQL | 标签体内的 SQL 文本 |

### 明确不做（Out of Scope）

- ❌ `<%@ include file="..." %>` / `<jsp:include>` 跨文件 include 解析（二期）
- ❌ JSP EL `${...}` 类型推断（用占位符即可）
- ❌ Taglib 自定义标签的 SQL 抽取
- ❌ JSP → Servlet 的 .java 编译产物解析（直接走源码）
- ❌ JSP 隐式对象（request/session/...）的类型绑定

---

## 高层设计

### 三层管线

```
 .jsp 源文件
     │
     ▼
 ┌────────────────────────────────────────────┐
 │ [1] JSP 预处理器（cobweb 新增）              │
 │     - Lexer：切分 TEXT/SCRIPTLET/DECL/EXPR  │
 │              /DIRECTIVE/JSTL_TAG            │
 │     - 缝合跨块语句、EL 占位替换              │
 │     - 合成 Java 包装                        │
 └────────────────────────────────────────────┘
     │ 合成 Java 源（String）
     ▼
 ┌────────────────────────────────────────────┐
 │ [2] ogsql-parser extract_sql_from_java()    │
 │     零改动复用：识别 JDBC/注解/拼接          │
 │     输出 ExtractedSql { sql, parse_result } │
 └────────────────────────────────────────────┘
     │ Vec<ExtractedSql>
     ▼
 ┌────────────────────────────────────────────┐
 │ [3] GraphBuilder.add_jsp_nodes_from_parsed()│
 │     - 创建 Node::JspPage                    │
 │     - 创建 Node::JspSql                     │
 │     - 添加 Edge: ContainsSql                │
 │     - 复用既有 CallsProcedure 边构建        │
 └────────────────────────────────────────────┘
```

### 合成 Java 包装策略

JSP 在编译期会被翻译成 `HttpJspBase` 子类的 `_jspService()` 方法体。我们模仿这一结构合成 Java，让 ogsql-parser 的 tree-sitter-java 能解析：

```java
// 合成模板
package __jsp_synthetic__;
import java.sql.*;
import javax.servlet.http.*;
import javax.servlet.jsp.*;

public class __JspPage__<fingerprint> {
    // <%! declaration %> 内容放这里（class-level）

    public void _jspService(HttpServletRequest request,
                            HttpServletResponse response,
                            PageContext pageContext,
                            HttpSession session,
                            ServletContext application,
                            JspWriter out) throws Throwable {
        // <% scriptlet %> 和 <%= expression %> 内容按出现顺序放这里
    }
}
```

**关键决策：**
- 合成类名加文件指纹后缀，避免多个 JSP 互相冲突
- 服务方法签名提供所有 JSP 隐式对象，让 scriptlet 里的 `request.getParameter()` 等调用语法上合法
- 类型解析仍然失败（tree-sitter 不做语义分析），但这**不影响** SQL 抽取 —— 我们只关心字符串字面量和方法调用名

### 图模型扩展

新增两个 Node variant 和一个 Edge variant：

```rust
pub enum Node {
    // ... 既有 variants ...

    /// JSP 页面节点
    JspPage {
        path: PathBuf,           // JSP 文件相对路径
        display_name: String,    // 用于显示的名称（如 "user/list.jsp"）
        url_pattern: Option<String>, // 来自 web.xml 或注解的 URL 模式（可选）
    },

    /// JSP 内嵌 SQL（与 JavaSql 平行，但保留独立 variant 便于过滤）
    JspSql {
        sql: String,             // 原始 SQL 文本（含占位符）
        file: PathBuf,           // 来源 JSP 文件
        line: usize,             // 在 JSP 中的行号
        kind: JspSqlKind,        // Scriptlet | Declaration | JstlQuery | JstlUpdate
    },
}

pub enum JspSqlKind {
    Scriptlet,    // 来自 <% %>
    Declaration,  // 来自 <%! %>
    JstlQuery,    // 来自 <sql:query>
    JstlUpdate,   // 来自 <sql:update>
}

pub enum Edge {
    // ... 既有 variants ...
    ContainsSql,  // JspPage → JspSql（包含关系）
    // 注：JspSql → Procedure 的调用关系复用既有 CallsProcedure / DirectCall
}
```

### NodeKey 扩展

参照 `JavaSql` 的 `NodeKey::JavaSql { file, line, sql_hash }` 模式，新增：

```rust
pub enum NodeKey {
    // ...
    JspPage { path: String },
    JspSql  { file: String, line: usize, sql_hash: String },
}
```

---

## Task 1: 添加 `jsp` Feature Flag

**Files:**
- Modify: `Cargo.toml`

**Step 1: 在 `Cargo.toml` 添加 `jsp` feature**

在 `[features]` 段添加（保持字母序）：

```toml
[features]
default = ["cli", "tui"]
cli = ["dep:clap"]
tui = ["dep:ratatui", "dep:crossterm"]
serve = ["dep:axum", "dep:tokio", "dep:tower-http", "dep:rust-embed"]
mcp = ["dep:rmcp"]
jsp = []   # ← 新增：JSP 内嵌 SQL 抽取
search-sql-v2 = []
full = ["cli", "tui", "serve", "mcp", "jsp", "search-sql-v2"]   # ← 同步更新 full
```

**Step 2: 验证 `cargo build` 成功**

Run: `cargo build`
Expected: 编译成功，无新依赖（feature 体为空）。

Run: `cargo build --features jsp`
Expected: 编译成功。

**Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "chore: add jsp feature flag for JSP SQL extraction"
```

---

## Task 2: 定义 JSP 数据结构

**Files:**
- Create: `src/parser/jsp_types.rs`
- Modify: `src/parser/mod.rs`

本任务定义 JSP 解析过程中的中间数据结构，**与 ogsql-parser 解耦** —— 这些结构只用在 cobweb 侧的预处理阶段。

**Step 1: 创建 `src/parser/jsp_types.rs`**

```rust
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
        assert_eq!(JspSqlKind::JstlQuery.as_str(), "jstl_query");
    }
}
```

**Step 2: 在 `src/parser/mod.rs` 添加模块声明**

```rust
#[cfg(feature = "jsp")]
pub mod jsp_types;
#[cfg(feature = "jsp")]
pub mod jsp_loader;
#[cfg(feature = "jsp")]
pub mod jsp_preprocessor;
```

（`jsp_loader` 和 `jsp_preprocessor` 会在后续任务中创建。）

**Step 3: 运行测试**

Run: `cargo test --features jsp jsp_types`
Expected: 测试通过。

**Step 4: Commit**

```bash
git add src/parser/jsp_types.rs src/parser/mod.rs
git commit -m "feat(jsp): add JSP parsing intermediate data structures"
```

---

## Task 3: 实现 JSP Lexer（片段切分）

**Files:**
- Create: `src/parser/jsp_preprocessor.rs`
- Test: 同文件 `#[cfg(test)]` 模块

JSP 语法相对简单，**手写状态机 lexer** 足够 —— 不需要 tree-sitter-JSP grammar（社区版本质量参差）。

**Step 1: 写失败测试**

在 `src/parser/jsp_preprocessor.rs`：

```rust
//! JSP 预处理器：将 JSP 源码切分为片段，
//! 然后合成为合法 Java 源以供 ogsql-parser 处理。

use crate::parser::jsp_types::{
    JspParseResult, JspSegment, JspSegmentKind,
};
use std::path::Path;

/// JSP lexer 状态机
pub struct JspLexer<'a> {
    source: &'a [u8],
    pos: usize,
    line: usize,        // 1-based
}

/// 简单的 lexer，识别 JSP 标签边界
/// 不做完整 JSP 语法分析，只切分到片段级
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
            // 检测 JSP 标签起始 `<%`
            if self.starts_with(b"<%") {
                // 先 flush 累积的文本段
                if self.pos > text_start {
                    let raw = std::str::from_utf8(&self.source[text_start..self.pos])
                        .unwrap_or("<invalid utf8>")
                        .to_string();
                    segments.push(JspSegment {
                        kind: JspSegmentKind::Text,
                        raw: raw.clone(),
                        content: raw,
                        start_line: text_start_line,
                        end_line: self.line,
                    });
                }

                // 解析 JSP 标签
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

        // flush 尾部文本
        if self.pos > text_start {
            let raw = std::str::from_utf8(&self.source[text_start..self.pos])
                .unwrap_or("<invalid utf8>")
                .to_string();
            segments.push(JspSegment {
                kind: JspSegmentKind::Text,
                raw: raw.clone(),
                content: raw,
                start_line: text_start_line,
                end_line: self.line,
            });
        }

        segments
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.source[self.pos..].starts_with(prefix)
    }

    /// 读取一个 JSP 标签，调用时 self.pos 指向 `<%`。
    /// 返回 Some(segment) 表示成功消费，None 表示无法识别（应跳过）。
    fn read_jsp_tag(&mut self, start_line: usize) -> Option<JspSegment> {
        debug_assert!(self.starts_with(b"<%"));

        // 判定标签类型
        let (kind, content_offset, is_jstl) = if self.starts_with(b"<%--") {
            // 注释：<%-- ... --%>
            return self.read_until_close("--%>", start_line, JspSegmentKind::Comment, 4);
        } else if self.starts_with(b"<%@") {
            // directive：<%@ ... %>
            return self.read_until_close("%>", start_line, JspSegmentKind::Directive, 3);
        } else if self.starts_with(b"<%=") {
            // expression：<%= ... %>
            return self.read_until_close("%>", start_line, JspSegmentKind::Expression, 3);
        } else if self.starts_with(b"<%!") {
            // declaration：<%! ... %>
            return self.read_until_close("%>", start_line, JspSegmentKind::Declaration, 3);
        } else if self.starts_with(b"<%") {
            // scriptlet：<% ... %>
            return self.read_until_close("%>", start_line, JspSegmentKind::Scriptlet, 2);
        } else {
            // JSTL 或自定义标签，例如 <sql:query>, <c:forEach>
            // 交给专门的 XML 解析路径处理（见 Task 8）
            return self.read_xml_tag(start_line);
        };
        // unreachable, but keep type checker happy
        #[allow(unreachable_code, unused_variables)]
        {
            let _ = (kind, content_offset, is_jstl);
            None
        }
    }

    /// 从 `tag_start_offset` 之后开始读取，直到遇到 `close_marker`。
    /// `skip_prefix` 是 `<%` 类前缀的长度。
    fn read_until_close(
        &mut self,
        close_marker: &str,
        start_line: usize,
        kind: JspSegmentKind,
        skip_prefix: usize,
    ) -> Option<JspSegment> {
        let content_start = self.pos + skip_prefix;
        let close_bytes = close_marker.as_bytes();

        // 推进 self.pos 跳过前缀，并更新行号
        for _ in 0..skip_prefix {
            if self.pos < self.source.len() && self.source[self.pos] == b'\n' {
                self.line += 1;
            }
            self.pos += 1;
        }

        // 从 content_start 开始扫描 close_marker
        let mut search = self.pos;
        while search + close_bytes.len() <= self.source.len() {
            if &self.source[search..search + close_bytes.len()] == close_bytes {
                // 找到闭合
                let content = std::str::from_utf8(&self.source[content_start..search])
                    .unwrap_or("<invalid utf8>")
                    .to_string();
                let raw = std::str::from_utf8(&self.source[self.pos - skip_prefix..search + close_bytes.len()])
                    .unwrap_or("<invalid utf8>")
                    .to_string();

                // 推进行号到闭合处
                for b in &self.source[self.pos..search + close_bytes.len()] {
                    if *b == b'\n' {
                        self.line += 1;
                    }
                }
                self.pos = search + close_bytes.len();

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

        // 未闭合 —— 退化处理：吃到文件末尾
        let content = std::str::from_utf8(&self.source[content_start..])
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
            raw: format!("<unterminated>{}", content),
            content: content.trim().to_string(),
            start_line,
            end_line: self.line,
        })
    }

    /// 读取 XML 风格的标签（JSTL `<sql:query>` 等），先做占位实现，
    /// Task 8 会替换为完整的 JSTL SQL 解析。
    fn read_xml_tag(&mut self, start_line: usize) -> Option<JspSegment> {
        // 简单实现：读到匹配的 `>`（不考虑属性内的引号，留作后续优化）
        let tag_start = self.pos;
        while self.pos < self.source.len() {
            if self.source[self.pos] == b'>' {
                self.pos += 1;
                break;
            }
            if self.source[self.pos] == b'\n' {
                self.line += 1;
            }
            self.pos += 1;
        }
        let raw = std::str::from_utf8(&self.source[tag_start..self.pos])
            .unwrap_or("<invalid utf8>")
            .to_string();
        // 默认归类为 Text（Task 8 会判定是否为 sql:* 并改归类）
        Some(JspSegment {
            kind: JspSegmentKind::Text,
            raw: raw.clone(),
            content: raw,
            start_line,
            end_line: self.line,
        })
    }
}

/// 入口：把 JSP 源码切分为片段
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
            // 简单提取 page directive 的属性，留作扩展
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
        // 应当有 3 段：Text, Expression, Text
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
        // 第一段 Text + Scriptlet + Text
        assert!(result.segments.iter().any(|s| s.kind == JspSegmentKind::Scriptlet));
        let scriptlet = result.segments.iter()
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
        // 一个语句被 HTML 切开的情况：
        // <% if (cond) { %> <b>some html</b> <% } %>
        let src = "<% if (cond) { %> <b>some html</b> <% } %>";
        let result = lex_jsp(src, Path::new("test.jsp"));
        let scriptlets: Vec<_> = result.segments.iter()
            .filter(|s| s.kind == JspSegmentKind::Scriptlet)
            .collect();
        assert_eq!(scriptlets.len(), 2);
        assert_eq!(scriptlets[0].content, "if (cond) {");
        assert_eq!(scriptlets[1].content, "}");
    }
}
```

**Step 2: 运行测试，确认全部通过**

Run: `cargo test --features jsp jsp_preprocessor`
Expected: 全部 9 个测试通过。

**Step 3: Commit**

```bash
git add src/parser/jsp_preprocessor.rs src/parser/mod.rs
git commit -m "feat(jsp): add JSP lexer for segment tokenization"
```

---

## Task 4: 合成 Java 源（喂给 ogsql-parser）

**Files:**
- Modify: `src/parser/jsp_preprocessor.rs`（追加 `synthesize_java` 模块）

把片段流缝合为合法 Java，让 `extract_sql_from_java()` 能解析。

**Step 1: 写失败测试**

追加到 `src/parser/jsp_preprocessor.rs`：

```rust
/// 合成 Java 源的产物
#[derive(Debug, Clone)]
pub struct SynthesizedJava {
    /// 完整合成 Java 源（可直接喂给 extract_sql_from_java）
    pub source: String,
    /// 类名（如 `__JspPage__ab12cd`）
    pub class_name: String,
}

/// 从 JSP 片段合成 Java 源
///
/// 策略：
/// - Declaration `<%! %>` 放到 class 顶层
/// - Scriptlet `<% %>` 和 Expression `<%= %>` 放到 `_jspService()` 方法体内
/// - 跨多个 scriptlet 块的语句保持顺序缝合
/// - EL 表达式 `${foo}` 替换为占位符 `__JSP_EL_FOO__`
/// - Expression `<%= x %>` 转译为 `out.print(x);`
pub fn synthesize_java(parsed: &JspParseResult) -> SynthesizedJava {
    // 用文件路径算短 hash 作为类名后缀，避免重名
    let path_str = parsed.file.to_string_lossy().to_string();
    let mut hash = blake3::hash(path_str.as_bytes());
    let hex = hash.to_hex();
    let suffix = &hex.as_str()[..8];
    let class_name = format!("__JspPage_{}", suffix);

    let mut class_body = String::new();
    let mut service_body = String::new();

    for seg in &parsed.segments {
        match seg.kind {
            JspSegmentKind::Declaration => {
                // 直接放到 class 顶层
                class_body.push_str(&replace_el(&seg.content));
                class_body.push('\n');
            }
            JspSegmentKind::Scriptlet => {
                service_body.push_str(&replace_el(&seg.content));
                service_body.push('\n');
            }
            JspSegmentKind::Expression => {
                // <%= x %> → out.print(x);
                let expr = replace_el(&seg.content);
                service_body.push_str(&format!("out.print({});\n", expr.trim()));
            }
            JspSegmentKind::Text
            | JspSegmentKind::Directive
            | JspSegmentKind::Comment
            | JspSegmentKind::JstlSql => {
                // 不参与 Java 合成
            }
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

/// 把 `${...}` EL 表达式替换为合法 Java 占位符
/// 例如 `${param.id}` → `__JSP_EL_PARAM_ID__`
/// 例如 `${user.name}` → `__JSP_EL_USER_NAME__`
fn replace_el(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            // 找匹配的 `}`
            if let Some(end) = s[i + 2..].find('}') {
                let expr = &s[i + 2..i + 2 + end];
                let ident = expr
                    .chars()
                    .map(|c| if c.is_alphanumeric() || c == '_' { c.to_ascii_uppercase() } else { '_' })
                    .collect::<String>();
                out.push_str(&format!("\"<EL_{}>\"", ident.trim_start_matches('_')));
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
        // declaration 应当出现在 _jspService 之前
        let decl_pos = syn.source.find("private static final String SQL");
        let service_pos = syn.source.find("_jspService").unwrap();
        let decl_pos = decl_pos.unwrap();
        assert!(decl_pos < service_pos);
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
        // EL ${param.id} 应当被替换为占位符字面量
        assert!(syn.source.contains("WHERE id="));
        // 不应当包含原始 ${...} 语法（会破坏 Java 解析）
        assert!(!syn.source.contains("${"));
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
        // 关键测试：合成产物能被 tree-sitter-java 解析
        // （ogsql-parser 内部用 tree-sitter，这里直接验证可解析性）
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
        parser.set_language(&tree_sitter_java::LANGUAGE.into()).unwrap();
        let tree = parser.parse(&syn.source, None);
        assert!(tree.is_some(), "tree-sitter should parse synthesized Java");
        let tree = tree.unwrap();
        // 检查没有顶层 parse error
        let root = tree.root_node();
        assert!(!root.has_error(), "synthesized Java should parse without errors");
    }
}
```

**Step 2: 运行测试**

Run: `cargo test --features jsp synthesize`
Expected: 全部通过（特别是 `synthesize_produces_parseable_java` 这一关键测试）。

**Step 3: Commit**

```bash
git add src/parser/jsp_preprocessor.rs
git commit -m "feat(jsp): synthesize parseable Java from JSP segments"
```

---

## Task 5: 调用 ogsql-parser 提取 SQL

**Files:**
- Create: `src/parser/jsp_loader.rs`

把合成 Java 喂给 `ogsql_parser::java::extract_sql_from_java()`，得到统一的 `ExtractedSql` 列表。

**Step 1: 实现 `jsp_loader.rs`**

```rust
//! JSP 文件加载与 SQL 抽取。
//!
//! 流程：JSP 源 → 片段切分 → 合成 Java →
//!      ogsql-parser extract_sql_from_java() → ExtractedSql 列表。

use crate::parser::jsp_preprocessor::{lex_jsp, synthesize_java, SynthesizedJava};
use crate::parser::jsp_types::{JspParseResult, JspSqlKind};
use ogsql_parser::java::{
    extract_sql_from_java, ExtractedSql, JavaExtractConfig, JavaExtractResult,
};
use std::path::{Path, PathBuf};

/// 单个 JSP 文件的完整解析产物
#[derive(Debug, Clone)]
pub struct JspFileResult {
    pub file: PathBuf,
    pub display_name: String,
    /// JSP 片段（用于行号回溯和告警）
    pub parse_result: JspParseResult,
    /// 合成的 Java 源（调试用）
    pub synthesized: SynthesizedJava,
    /// ogsql-parser 抽取出的 SQL 列表
    pub extractions: Vec<ExtractedSql>,
    /// ogsql-parser 报告的错误
    pub errors: Vec<String>,
}

/// 解析单个 JSP 文件
pub fn load_jsp_file(path: &Path, config: &JavaExtractConfig) -> Result<JspFileResult, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("read {:?}: {}", path, e))?;

    Ok(load_jsp_string(source, path, config))
}

/// 解析 JSP 字符串（便于测试）
pub fn load_jsp_string(source: String, path: &Path, config: &JavaExtractConfig) -> JspFileResult {
    let parse_result = lex_jsp(&source, path);
    let synthesized = synthesize_java(&parse_result);

    // 合成的 Java 喂给 ogsql-parser
    let synthetic_path = format!("{}/__synthetic__.java", path.display());
    let JavaExtractResult { extractions, errors } =
        extract_sql_from_java(&synthesized.source, &synthetic_path, config);

    // 为每个 extraction 标注 JSP 来源 kind
    // （ogsql-parser 不知道片段边界，但绝大多数 SQL 都来自 scriptlet）
    // 高级版本可以通过行号映射回原 JSP 片段，MVP 先统一打 Scriptlet 标签
    let extractions = extractions
        .into_iter()
        .map(|e| annotate_extraction(e, &parse_result))
        .collect::<Vec<_>>();

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

/// 给每个 extraction 打上 JspSqlKind 标签。
///
/// MVP 实现：默认打 Scriptlet；如果 SQL 出现在 declaration 段（罕见），打 Declaration。
/// 完整实现可通过行号映射（合成 Java 行号 → 原 JSP 行号 → 段类型）。
fn annotate_extraction(mut extraction: ExtractedSql, _parsed: &JspParseResult) -> ExtractedSql {
    // 当前 ogsql-parser 的 ExtractedSql 不带 JSP kind 字段，
    // 我们把 kind 信息编码到 origin 的 file_path 后缀里，避免修改 ogsql-parser。
    // 例如 `path/__synthetic__.java` → `path/sample.jsp#scriptlet`
    //
    // 后续在 GraphBuilder 里会解码这个后缀。
    extraction
}

/// 批量解析多个 JSP 文件
pub fn load_jsp_files_from_paths(
    paths: &[PathBuf],
    config: &JavaExtractConfig,
) -> Vec<JspFileResult> {
    let mut results = Vec::with_capacity(paths.len());
    for path in paths {
        match load_jsp_file(path, config) {
            Ok(r) => results.push(r),
            Err(e) => {
                // 不致命，记录告警继续
                eprintln!("[jsp] failed to load {:?}: {}", path, e);
            }
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> JavaExtractConfig {
        JavaExtractConfig {
            extra_sql_methods: Vec::new(),
            extra_sql_var_patterns: Vec::new(),
        }
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
        assert!(!result.extractions.is_empty(), "should extract SQL");
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
CallableStatement cs = conn.prepareCall("{call pkg.get_user(?, ?)}");
cs.setLong(1, userId);
cs.registerOutParameter(2, Types.VARCHAR);
cs.execute();
%>"#;
        let result = load_jsp_string(src.to_string(), Path::new("/t.jsp"), &default_config());
        // 应当至少抽到 prepareCall 的 SQL
        assert!(!result.extractions.is_empty());
        let found_proc = result.extractions.iter()
            .any(|e| e.sql.contains("pkg.get_user") || e.sql.contains("call"));
        assert!(found_proc, "should detect stored procedure call");
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
        // ogsql-parser 应当能追踪常量到使用点
        assert!(!result.extractions.is_empty());
    }

    #[test]
    fn extract_skips_html_only_jsp() {
        let src = "<html><body><h1>Hello</h1></body></html>";
        let result = load_jsp_string(src.to_string(), Path::new("/t.jsp"), &default_config());
        assert!(result.extractions.is_empty());
        // 即使没有 Java 内容，也不应当报致命错误
    }

    #[test]
    fn extract_handles_invalid_java_gracefully() {
        // scriptlet 内是不完整的 Java，ogsql-parser 应当报错但不 panic
        let src = "<% String sql = \"SELECT\"; %>";
        let result = load_jsp_string(src.to_string(), Path::new("/t.jsp"), &default_config());
        // 即使 extraction 为空，函数不应 panic
        assert!(result.errors.is_empty() || !result.extractions.is_empty());
    }
}
```

**Step 2: 运行测试**

Run: `cargo test --features jsp jsp_loader`
Expected: 全部通过。如果 ogsql-parser 的 API 签名与上述不完全一致（例如 `JavaExtractConfig` 字段名不同），按实际签名调整。

**Step 3: Commit**

```bash
git add src/parser/jsp_loader.rs src/parser/mod.rs
git commit -m "feat(jsp): integrate ogsql-parser SQL extraction"
```

---

## Task 6: 扩展图模型（Node / Edge / NodeKey）

**Files:**
- Modify: `src/graph/mod.rs`
- Modify: `src/graph/key.rs`

**Step 1: 在 `Node` enum 添加 `JspPage` 和 `JspSql` variant**

打开 `src/graph/mod.rs`，找到 `pub enum Node { ... }`，添加（保持与既有 variant 风格一致）：

```rust
#[cfg(feature = "jsp")]
JspPage {
    path: PathBuf,
    display_name: String,
    /// 从 web.xml 或 @WebServlet 注解映射的 URL（可选，MVP 可留空）
    url_pattern: Option<String>,
},

#[cfg(feature = "jsp")]
JspSql {
    /// 抽取出的 SQL 文本（含 EL 占位符）
    sql: String,
    /// 来源 JSP 文件路径
    file: PathBuf,
    /// 在合成 Java 中的行号（近似原 JSP 行号）
    line: usize,
    /// SQL 来源子类型
    kind: crate::parser::jsp_types::JspSqlKind,
    /// 是否被 ogsql-parser 成功解析为 StatementInfo
    parsed: bool,
},
```

**Step 2: 在 `Edge` enum 添加 `ContainsSql`**

```rust
#[cfg(feature = "jsp")]
ContainsSql,
```

（`JspSql → Procedure` 的调用边**复用既有 `CallsProcedure` / `DirectCall`**，不新增。）

**Step 3: 在 `key.rs` 添加 `NodeKey` variant**

```rust
#[cfg(feature = "jsp")]
JspPage {
    path: String,
},

#[cfg(feature = "jsp")]
JspSql {
    file: String,
    line: usize,
    sql_hash: String,
},
```

参考既有 `JavaSql` 的 NodeKey 实现方式补齐 `tag()` / `path()` / Hash 等方法。

**Step 4: 添加节点类型 tag 常量**

在 `Node` 的 `impl` 里（参考 `JavaMethod` 的 `tag()` 返回 `"method"`）：

```rust
#[cfg(feature = "jsp")]
pub fn tag(&self) -> &'static str {
    match self {
        Node::JspPage { .. } => "jsp",
        Node::JspSql { .. } => "jsql",  // jsp-sql
        // ... 既有分支 ...
    }
}
```

**Step 5: 运行 `cargo check --features jsp` 确认编译**

Run: `cargo check --features jsp`
Expected: 编译通过。

Run: `cargo check`（默认 feature）
Expected: 编译通过（验证 `#[cfg(feature = "jsp")]` 正确门控）。

**Step 6: Commit**

```bash
git add src/graph/mod.rs src/graph/key.rs
git commit -m "feat(jsp): add JspPage and JspSql node variants"
```

---

## Task 7: 扩展 scanner 识别 `.jsp` 文件

**Files:**
- Modify: `src/parser/scanner.rs`

**Step 1: 在 `FileType` enum 添加 Jsp**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Sql,
    Java,
    Xml,
    #[cfg(feature = "jsp")]
    Jsp,
}
```

**Step 2: 在 `ScannedFiles` 添加 jsp_files 字段**

```rust
#[derive(Debug, Clone, Default)]
pub struct ScannedFiles {
    pub sql_files: Vec<PathBuf>,
    pub java_files: Vec<PathBuf>,
    pub xml_files: Vec<PathBuf>,
    #[cfg(feature = "jsp")]
    pub jsp_files: Vec<PathBuf>,
}
```

**Step 3: 在扩展名匹配逻辑添加 `.jsp`**

参考既有 `.java` 的匹配分支，添加：

```rust
#[cfg(feature = "jsp")]
"jsp" => {
    file_type = Some(FileType::Jsp);
    scanned.jsp_files.push(path.to_path_buf());
}
```

**Step 4: 写测试**

```rust
#[cfg(test)]
mod jsp_tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    #[cfg(feature = "jsp")]
    fn scan_recognizes_jsp_extension() {
        // 用 tempfile 创建一个 .jsp 文件然后扫描
        // ...（参考既有 scan_recognizes_java_extension 的写法）
    }
}
```

**Step 5: 运行测试**

Run: `cargo test --features jsp scanner`
Expected: 全部通过。

**Step 6: Commit**

```bash
git add src/parser/scanner.rs
git commit -m "feat(jsp): scanner recognizes .jsp files"
```

---

## Task 8: GraphBuilder 集成（核心接线）

**Files:**
- Modify: `src/graph/builder.rs`

把 JSP 解析结果接入图谱构建管线。

**Step 1: 在 `GraphBuilder` 添加 `add_jsp_nodes_from_parsed`**

参考 `add_java_nodes_from_parsed()` 的写法：

```rust
#[cfg(feature = "jsp")]
fn add_jsp_nodes_from_parsed(&mut self, jsp_results: &[JspFileResult]) {
    use crate::parser::jsp_types::JspSqlKind;
    use ogsql_parser::java::SqlOrigin;

    for file_result in jsp_results {
        // 1. 创建 JspPage 节点
        let page_key = NodeKey::JspPage {
            path: file_result.file.to_string_lossy().to_string(),
        };
        let page_node = Node::JspPage {
            path: file_result.file.clone(),
            display_name: file_result.display_name.clone(),
            url_pattern: None, // MVP 不解析 web.xml
        };
        let page_idx = self.ensure_node(&page_key, page_node);

        // 2. 为每个 extraction 创建 JspSql 节点 + 边
        for extraction in &file_result.extractions {
            let sql_hash = blake3::hash(extraction.sql.as_bytes()).to_hex().as_str()[..16].to_string();
            let sql_key = NodeKey::JspSql {
                file: file_result.file.to_string_lossy().to_string(),
                line: extraction.origin.line(),
                sql_hash: sql_hash.clone(),
            };

            // 推断 JspSqlKind
            let kind = match extraction.origin {
                SqlOrigin::Field | SqlOrigin::Constant => JspSqlKind::Declaration,
                _ => JspSqlKind::Scriptlet,
            };

            let parsed = extraction.parse_result.is_some();
            let sql_node = Node::JspSql {
                sql: extraction.sql.clone(),
                file: file_result.file.clone(),
                line: extraction.origin.line(),
                kind,
                parsed,
            };
            let sql_idx = self.ensure_node(&sql_key, sql_node);

            // 3. 添加 ContainsSql 边：page → sql
            self.graph.update_edge(
                page_idx,
                sql_idx,
                Edge::ContainsSql,
            );

            // 4. 如果 ogsql-parser 解析出了 StatementInfo，
            //    复用既有逻辑把 CALL/EXECUTE 关系挂上
            if let Some(parse_result) = &extraction.parse_result {
                for stmt in &parse_result.statements {
                    // 复用 extractor.rs 中的调用关系提取逻辑
                    self.process_statement_info_for_calls(
                        stmt,
                        sql_idx,
                        &file_result.file,
                    );
                }
            }
        }

        // 5. 把 errors 推到 parse_log
        for err in &file_result.errors {
            self.parse_log.add_warning(
                file_result.file.to_string_lossy().as_ref(),
                format!("[jsp] {}", err),
            );
        }
    }
}
```

**Step 2: 在 `build_graph_internal` 中调用**

找到 `build_graph_internal()` 的现有流程（在 Java 处理之后）：

```rust
#[cfg(feature = "jsp")]
{
    if !scanned.jsp_files.is_empty() {
        let config = JavaExtractConfig::default();
        let jsp_results = load_jsp_files_from_paths(&scanned.jsp_files, &config);
        self.add_jsp_nodes_from_parsed(&jsp_results);
    }
}
```

**Step 3: 确保 `process_statement_info_for_calls` 已存在或抽取**

如果 `extractor.rs` 中已有从 `StatementInfo` 提取 CALL 边的函数（很可能 `add_java_nodes_from_parsed` 已经用到），抽取为可复用方法并在此调用。**避免重复实现。**

**Step 4: 写集成测试**

在 `tests/jsp_integration_test.rs`：

```rust
#![cfg(feature = "jsp")]

use codeweb::graph::builder::GraphBuilder;
use codeweb::parser::scanner::scan_directories;
use std::fs;
use tempfile::TempDir;

#[test]
fn jsp_sql_links_to_stored_procedure() {
    let tmp = TempDir::new().unwrap();

    // 准备一个 JSP 文件，包含对存储过程的调用
    let jsp = r#"<%@ page import="java.sql.*" %>
<%
Connection conn = DriverManager.getConnection("jdbc:default");
CallableStatement cs = conn.prepareCall("{call pkg.get_user(?, ?)}");
cs.setLong(1, 1);
cs.execute();
%>"#;
    fs::write(tmp.path().join("user.jsp"), jsp).unwrap();

    // 准备对应的 SQL 文件
    let sql = r#"
CREATE PROCEDURE pkg.get_user(p_id IN BIGINT, p_name OUT VARCHAR)
AS
BEGIN
  SELECT name INTO p_name FROM users WHERE id = p_id;
END;
"#;
    fs::write(tmp.path().join("pkg.sql"), sql).unwrap();

    let scanned = scan_directories(&[tmp.path().to_path_buf()]);
    let mut builder = GraphBuilder::new();
    builder.build_from_scanned(&scanned);

    let graph = builder.graph();
    // 应当存在 JspPage 节点
    assert!(graph.node_indices().any(|i| {
        matches!(graph[i], codeweb::graph::Node::JspPage { .. })
    }));
    // 应当存在 JspSql 节点
    assert!(graph.node_indices().any(|i| {
        matches!(graph[i], codeweb::graph::Node::JspSql { .. })
    }));
    // 应当存在从 JspSql 到 Procedure 的调用边
    // (具体断言形式取决于既有 CallsProcedure 边的实现)
}
```

**Step 5: 运行测试**

Run: `cargo test --features jsp --test jsp_integration_test`
Expected: 通过。

**Step 6: Commit**

```bash
git add src/graph/builder.rs tests/jsp_integration_test.rs
git commit -m "feat(jsp): wire JSP parsing into graph builder"
```

---

## Task 9: Export 层支持新节点

**Files:**
- Modify: `src/export/dot.rs`
- Modify: `src/export/json.rs`
- Modify: `src/export/mermaid.rs`

**Step 1: DOT 导出**

在 `dot.rs` 找到节点渲染逻辑（match `Node::*`），添加：

```rust
#[cfg(feature = "jsp")]
Node::JspPage { display_name, path, .. } => {
    format!(
        r#"[shape=component, style=filled, fillcolor="#FFE4B5", label="JSP\n{}"];"#,
        display_name
    )
}
#[cfg(feature = "jsp")]
Node::JspSql { sql, kind, .. } => {
    let short_sql: String = sql.chars().take(40).collect();
    format!(
        r#"[shape=note, style=filled, fillcolor="#FFFACD", label="JSP-SQL [{}]\n{}"];"#,
        kind.as_str(),
        escape_dot(&short_sql)
    )
}
```

**Step 2: JSON 导出**

JSON 通常通过 serde 自动处理。确认 `Node` 和 `Edge` 都 derive `Serialize`：
- 新加的 `JspSqlKind` 已 derive `Serialize`/`Deserialize`（Task 2 中已加）
- `Node::JspPage` / `Node::JspSql` 应当跟随 `Node` 的 serde 派生

Run: `cargo check --features jsp` 确保序列化无问题。

**Step 3: Mermaid 导出**

在 `mermaid.rs` 添加分支：

```rust
#[cfg(feature = "jsp")]
Node::JspPage { display_name, .. } => {
    format!("    n{}[\"📁 {}\"]:::jsp", idx, escape_mermaid(display_name))
}
#[cfg(feature = "jsp")]
Node::JspSql { sql, kind, .. } => {
    let short: String = sql.chars().take(30).collect();
    format!("    n{}[\"📄 {}<br/>{}\"]:::jsql", idx, kind.as_str(), escape_mermaid(&short))
}
```

并在 `classDef` 部分添加样式：

```
classDef jsp fill:#FFE4B5,stroke:#333
classDef jsql fill:#FFFACD,stroke:#333
```

**Step 4: 写测试**

```rust
#[cfg(test)]
mod jsp_export_tests {
    use super::*;
    // 测试 JSP 节点能正常导出为 DOT/JSON/Mermaid 而不 panic
    #[test]
    fn dot_export_handles_jsp_page() { ... }
    #[test]
    fn json_export_handles_jsp_sql() { ... }
    #[test]
    fn mermaid_export_handles_jsp_page() { ... }
}
```

**Step 5: Commit**

```bash
git add src/export/
git commit -m "feat(jsp): support JSP nodes in DOT/JSON/Mermaid exporters"
```

---

## Task 10: CLI stats 与输出适配

**Files:**
- Modify: `src/main.rs`（stats 命令）
- Modify: `src/graph/traverse.rs`（链路格式化）

**Step 1: stats 命令统计 JSP 节点**

在 `cmd_stats`（或同名函数）里，找到按节点类型统计的部分，添加：

```rust
#[cfg(feature = "jsp")]
let jsp_page_count = graph.node_indices().filter(|i| {
    matches!(graph[*i], Node::JspPage { .. })
}).count();
#[cfg(feature = "jsp")]
let jsp_sql_count = graph.node_indices().filter(|i| {
    matches!(graph[*i], Node::JspSql { .. })
}).count();

#[cfg(feature = "jsp")]
println!("  JSP Pages:      {}", jsp_page_count);
#[cfg(feature = "jsp")]
println!("  JSP SQL:        {}", jsp_sql_count);
```

**Step 2: traverse 的链路格式化**

在 `traverse.rs` 找到节点格式化为字符串的函数，添加：

```rust
#[cfg(feature = "jsp")]
Node::JspPage { display_name, .. } => format!("JSP:{}", display_name),
#[cfg(feature = "jsp")]
Node::JspSql { sql, kind, .. } => format!("JSP-SQL[{}]:{}", kind.as_str(), truncate(sql, 40)),
```

**Step 3: 更新 `nodes` 命令的 type filter**

如果 `nodes -t <type>` 命令支持按 tag 过滤，确保 `jsp` 和 `jsql` 也是合法值。更新帮助文本和过滤逻辑。

**Step 4: Commit**

```bash
git add src/main.rs src/graph/traverse.rs
git commit -m "feat(jsp): stats and trace formatting for JSP nodes"
```

---

## Task 11: 文档与 README 更新

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/plans/roadmap.md`

**Step 1: README.md 更新**

在 Feature Flags 表格添加：

```markdown
| `jsp` | JSP 内嵌 SQL 抽取（scriptlet JDBC、JSTL `<sql:query>`） | ❌ |
```

更新 `full` 描述：`cli + tui + serve + mcp + jsp + search-sql-v2`。

在 Node Types 表格添加：

```markdown
| JspPage | `jsp` | JSP 页面 |
| JspSql | `jsql` | JSP 内嵌 SQL |
```

在 Quick Start 添加示例（可选）：

```bash
codeweb init legacy-app -d ./WebRoot -d ./src/main/java -d ./sql
codeweb analyze  # 自动识别 .jsp 文件
codeweb trace --from "pkg.get_user"  # 反向追溯到 JSP 入口
```

**Step 2: AGENTS.md 添加 Phase 3b**

在 Phase 3 和 Phase 4 之间插入：

```markdown
### Phase 3b: + JSP Scriptlet SQL Extraction

Extract SQL embedded in JSP files via scriptlets (`<% %>`), declarations (`<%! %>`), and JSTL `<sql:query>` tags. Reuses ogsql-parser's `extract_sql_from_java()` by pre-processing JSP into synthetic Java.

Bridge rule: `JspPage → JspSql → Procedure` (via reused CallsProcedure edges).

**Status**: Implemented behind `jsp` feature flag.
```

**Step 3: roadmap.md 更新**

在 Phase 3 之后添加 `## Phase 3b: JSP 内嵌 SQL 抽取` 章节，参考其他 Phase 的结构（目标 / 工期 / 依赖 / 模块 / 验收标准）。

**Step 4: Commit**

```bash
git add README.md AGENTS.md docs/plans/roadmap.md
git commit -m "docs: document JSP SQL extraction feature"
```

---

## 可选 Task 12: JSTL SQL 标签抽取

> 仅在 MVP 完成且实际项目数据中 JSTL 占比 > 10% 时实施。

**Files:**
- Modify: `src/parser/jsp_preprocessor.rs`（替换 `read_xml_tag` 的占位实现）

**实现要点：**

1. 在 `read_xml_tag` 中检测标签名前缀 `sql:`，识别 `<sql:query>` / `<sql:update>`
2. 解析 XML 属性提取 `sql="..."` 属性
3. 处理带 body 的形式：`<sql:query>SELECT ...</sql:query>`
4. 把抽取到的 SQL 包装为合成 Java 中的字符串字面量，让 ogsql-parser 通过变量追踪识别
5. 设置 `JspSqlKind::JstlQuery` / `JspSqlKind::JstlUpdate`

**测试用例：**

```jsp
<!-- 属性形式 -->
<sql:query var="users" sql="SELECT * FROM users WHERE id = ?" >
    <sql:param value="${param.id}" />
</sql:query>

<!-- body 形式 -->
<sql:update>
    UPDATE users SET last_login = NOW() WHERE id = ${param.id}
</sql:update>
```

---

## 验收标准（Acceptance Criteria）

实现完成后，以下场景必须通过：

### 功能验收

- [ ] `cargo build`（默认 features）成功，无 JSP 相关 warning
- [ ] `cargo build --features jsp` 成功
- [ ] `cargo build --features full` 成功
- [ ] `cargo test --features jsp` 全部通过
- [ ] `cargo clippy --features jsp -- -D warnings` 通过
- [ ] `cargo fmt -- --check` 通过

### 端到端场景

- [ ] **场景 1**：纯 scriptlet JDBC —— JSP 中 `prepareStatement("SELECT...")`，trace 该 SELECT 能反向追溯到 JspPage 节点
- [ ] **场景 2**：存储过程调用 —— JSP 中 `prepareCall("{call pkg.x()}")`，图谱中存在 `JspPage → JspSql → Procedure` 边
- [ ] **场景 3**：字符串拼接 —— JSP 中 `String sql = "..." + request.getParameter()`，能抽出完整 SQL（含占位符）
- [ ] **场景 4**：纯 HTML JSP —— 无 Java 内容的 JSP 不产生 JspSql 节点，但产生 JspPage 节点
- [ ] **场景 5**：不完整 Java —— `<% %>` 内 Java 语法不完整时不 panic，记入 parse_log
- [ ] **场景 6**：导出 —— DOT/JSON/Mermaid 三种格式都能正确渲染 JSP 节点
- [ ] **场景 7**：stats 命令正确统计 JSP 节点数

### 兼容性

- [ ] 关闭 `jsp` feature 时，既有 SQL/Java/XML 流程完全不受影响
- [ ] 默认 features 构建产物不包含任何 JSP 相关代码（`#[cfg(feature = "jsp")]` 正确门控）
- [ ] 现有所有测试在默认 features 下仍然通过

---

## 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| ogsql-parser 的 `JavaExtractConfig` 字段与计划文档不一致 | Task 5 编译失败 | 实施前先读 `ogsql-parser` 实际源码确认签名，必要时调整 |
| 合成 Java 包含 JSP 隐式对象，tree-sitter-java 解析失败 | 抽取率低 | 测试覆盖各类隐式对象；失败时 fallback 到纯字符串 regex 抽取 |
| EL 表达式替换破坏字符串拼接语义 | SQL 抽取不完整 | 用语义中性的占位符 `<EL_XXX>` 而非变量引用 |
| 跨 `<% %>` 块的语句被错误缝合 | Java 编译错误，影响后续 SQL 抽取 | 缝合时保持源码顺序；对常见模式（`if {}` 跨块）写专门测试 |
| 行号映射失真 | `trace` 显示位置不准 | MVP 接受近似行号；二期做合成 Java 行号 ↔ 原 JSP 行号的映射表 |
| 大型 JSP 文件性能 | 分析变慢 | 合成 Java 是字符串操作，O(n) 复杂度，预估单文件 < 10ms |

---

## 工期估算

| 阶段 | 估算 |
|---|---|
| Task 1–2（feature flag + 数据结构） | 0.5 天 |
| Task 3（JSP lexer） | 1 天 |
| Task 4（合成 Java） | 1 天 |
| Task 5（ogsql-parser 集成） | 0.5 天 |
| Task 6（图模型扩展） | 0.5 天 |
| Task 7（scanner） | 0.5 天 |
| Task 8（builder 集成） | 1 天 |
| Task 9–10（export + CLI） | 1 天 |
| Task 11（文档） | 0.5 天 |
| **MVP 合计** | **6.5 天** |
| Task 12（JSTL，可选） | +2 天 |

---

## 后续路线（Out of Scope for MVP）

- **`<%@ include %>` 跨文件解析**：解析静态包含，把被包含文件的片段合并到主 JSP
- **`<jsp:include>` 动态包含**：建立 JspPage → JspPage 的 Include 边
- **JSP EL 类型推断**：从 `pageContext` / TLD 推断 `${xxx}` 的类型，让 SQL 拼接更精确
- **Taglib 自定义标签**：扫描 `.tld` 文件识别项目内自定义的 SQL 标签
- **行号精确映射**：合成 Java 行号 ↔ 原 JSP 行号的双向映射表
- **JSP → Servlet class 解析**：分析编译产物 `.class` 文件（仅当源码不可得时）

---

## 执行说明

> **For Claude:** 用 `superpowers:executing-plans` 技能按 Task 顺序执行。

每个 Task 应当独立可提交，遵循 TDD：先写失败测试 → 实现 → 测试通过 → commit。

**关键依赖顺序：**
- Task 1 → 所有后续（feature flag 先立）
- Task 2 → Task 3, 4, 5（数据结构基础）
- Task 3 → Task 4（合成依赖 lexer 输出）
- Task 4 → Task 5（ogsql-parser 集成依赖合成 Java）
- Task 6 → Task 8（图模型 variant 是 builder 的前提）
- Task 7 → Task 8（scanner 提供文件列表）
- Task 8 → Task 9, 10（builder 走通才能验证导出）

Task 9 和 Task 10 可并行。Task 11 可在任意时机进行（建议最后做）。

Task 12（JSTL）是独立可选任务，不阻塞 MVP。
