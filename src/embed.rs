//! 嵌入稳定接口（docs/embeded-codeweb.md §3.2）。
//!
//! 本模块是 codeweb 对 SpinDuck（以及任何宿主进程）暴露的唯一契约面：
//! 不直接暴露 `petgraph::DiGraph`、`GraphStore` 或带 `body_sql`/`columns` 的
//! 内部节点。宿主拿到的是 [`ThinGraph`]（瘦图投影）与结构化的
//! [`DiffStatus`](self::DiffStatus)，配合 [`CodeWebError`] 的
//! `StoreNotFound` / `StoreVersionMismatch` 变体区分「无 store」与「过期 store」。
//!
//! ## ThinNodeKind 字段映射表（实现与测试共用此口径）
//!
//! | graph::Node 变体 | ThinNodeKind | 说明 |
//! |---|---|---|
//! | `Procedure` / `Function` | 同名 | `RoutineId`→schema/package/name；`SourceLocation`→file/line；`partial` |
//! | `Table` / `View` | 同名 | `explicit`/`system` 透传（codeweb 语义完整；SpinDuck 只消费 `system`，其 `from_thin_graph` 会丢弃 explicit） |
//! | `Package` / `MaterializedView` / `Sequence` / `Unresolved` / `BuiltinFunction` | 同名 | 字段同 JSON 导出 |
//! | 其余全部变体 | **跳过** | Java/JSP/mapper/trigger/type/index/synonym/event/custom |
//!
//! ## ThinEdgeKind 映射
//!
//! `DirectCall→Direct`、`DynamicCall→Dynamic`、`TableAccess→TableAccess`
//! （`AccessMode` 按位展开为 `Vec<AccessModeTag>`，顺序与 JSON `modes` 数组一致；
//! `write_kinds` HashSet 经 `write_kind_label` 转字符串后排序）、`DependsOn`、
//! `UsesSequence`、`AnchorsOn`、`ContainsRoutine` 原样；其余边变体跳过。
//! **边两端只要有一端落在被跳过节点集 → 整条边跳过**（`Dynamic` 边的 target
//! 指向发出调用的例程自身，天然保留）。

use petgraph::visit::EdgeRef;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::graph::store::{GraphStore, STORE_VERSION};
use crate::graph::{AccessMode, Edge, Node};
use crate::project::config::StoreFormat;
use crate::project::Project;

/// codeweb crate 版本，替代宿主侧解析 CLI `--version` 输出。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 结构化源码新鲜度状态（docs/embeded-codeweb.md §3.2.3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffStatus {
    /// `.codeweb/store.*` 是否存在
    pub store_exists: bool,
    /// 若 store 存在但版本过期，返回旧版本号；否则为 None。
    /// `Some(0)` 表示 store 文件存在但版本头不可读（legacy 无头文件/损坏），
    /// 按最大过期处理，促使宿主重新 analyze。
    pub stale_store_version: Option<u32>,
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

impl DiffStatus {
    pub fn is_up_to_date(&self) -> bool {
        self.store_exists
            && self.stale_store_version.is_none()
            && self.added.is_empty()
            && self.modified.is_empty()
            && self.deleted.is_empty()
    }
}

/// 项目句柄：薄包装 `crate::project::Project`（docs/embeded-codeweb.md §3.2.2）。
pub struct EmbeddedProject {
    inner: Project,
}

impl std::fmt::Debug for EmbeddedProject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedProject")
            .field("root", &self.inner.root())
            .finish()
    }
}

impl EmbeddedProject {
    /// 从起始目录向上查找 codeweb.toml 并加载项目
    pub fn find(start_dir: &Path) -> Result<Self> {
        Ok(Self {
            inner: Project::find(start_dir)?,
        })
    }

    /// 仅检查指定目录本身是否存在 codeweb.toml（供 SpinDuck 显式指定
    /// `--codeweb-project` 时使用）
    pub fn open_exact(project_root: &Path) -> Result<Self> {
        Ok(Self {
            inner: Project::open_exact(project_root)?,
        })
    }

    pub fn root(&self) -> &Path {
        self.inner.root()
    }

    pub fn name(&self) -> &str {
        self.inner.name()
    }

    /// 结构化源码新鲜度检查（替代 `codeweb diff` + stderr 哨兵匹配）。
    pub fn diff(&self) -> Result<DiffStatus> {
        let store_path = self.inner.store_path();
        let store_exists = store_path.exists();
        let stale_store_version = if store_exists {
            match self.inner.config().store.format {
                StoreFormat::Bincode => stale_version(GraphStore::peek_version(&store_path)),
                StoreFormat::Json => stale_version(GraphStore::json_peek_version(&store_path)),
            }
        } else {
            None
        };
        let changes = self.inner.diff()?;
        Ok(DiffStatus {
            store_exists,
            stale_store_version,
            added: changes.added,
            modified: changes.modified,
            deleted: changes.deleted,
        })
    }

    /// 全量或增量分析并落盘 store（需开启 `analyze` feature）。
    #[cfg(feature = "analyze")]
    pub fn analyze(&mut self) -> Result<crate::project::AnalyzeReport> {
        self.inner.analyze()
    }

    /// 加载 store（若未加载）并投影为瘦图。
    /// 投影时仅保留 SQL/PLSQL 闭包所需的节点与边，不拷贝 columns / body_sql /
    /// Java / JSP。
    pub fn export_thin_graph(&mut self) -> Result<ThinGraph> {
        let store = self.inner.load_store()?;
        Ok(project_thin_graph(store.graph()))
    }

    /// 已加载的 store（若尚未加载则为 None）。供等价测试取参照面；
    /// 宿主进程请走 [`Self::export_thin_graph`]。
    pub fn store(&self) -> Option<&crate::graph::store::GraphStore> {
        self.inner.store()
    }
}

/// peek 到的版本号 → 过期判定（见 `DiffStatus::stale_store_version` 文档）。
fn stale_version(peeked: Option<u32>) -> Option<u32> {
    match peeked {
        Some(found) if found != STORE_VERSION => Some(found),
        Some(_) => None,
        // 文件存在但版本头不可读：按最大过期处理
        None => Some(0),
    }
}

// ── 瘦图模型（docs/embeded-codeweb.md §3.2.4）────────────────────────────────

#[derive(Debug, Clone)]
pub struct ThinGraph {
    pub nodes: Vec<ThinNodeEntry>,
    pub edges: Vec<ThinEdge>,
}

#[derive(Debug, Clone)]
pub struct ThinNodeEntry {
    /// `petgraph::NodeIndex::index()`，与 JSON 导出同名同义
    pub id: usize,
    pub kind: ThinNodeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinNodeKind {
    Procedure {
        schema: Option<String>,
        package: Option<String>,
        name: String,
        file: PathBuf,
        line: usize,
        partial: bool,
    },
    Function {
        schema: Option<String>,
        package: Option<String>,
        name: String,
        file: PathBuf,
        line: usize,
        partial: bool,
    },
    Package {
        schema: Option<String>,
        name: String,
    },
    Table {
        schema: Option<String>,
        name: String,
        explicit: bool,
        system: bool,
    },
    View {
        schema: Option<String>,
        name: String,
        explicit: bool,
        system: bool,
    },
    MaterializedView {
        schema: Option<String>,
        name: String,
    },
    Sequence {
        schema: Option<String>,
        name: String,
        explicit: bool,
    },
    Unresolved {
        raw_expr: String,
    },
    BuiltinFunction {
        name: String,
    },
}

impl ThinNodeKind {
    /// 与 JSON 导出的 `type` 字段同拼写，供等价测试与宿主侧日志使用。
    pub fn tag(&self) -> &'static str {
        match self {
            ThinNodeKind::Procedure { .. } => "procedure",
            ThinNodeKind::Function { .. } => "function",
            ThinNodeKind::Package { .. } => "package",
            ThinNodeKind::Table { .. } => "table",
            ThinNodeKind::View { .. } => "view",
            ThinNodeKind::MaterializedView { .. } => "materialized_view",
            ThinNodeKind::Sequence { .. } => "sequence",
            ThinNodeKind::Unresolved { .. } => "unresolved",
            ThinNodeKind::BuiltinFunction { .. } => "builtin_function",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThinEdge {
    pub source: usize,
    pub target: usize,
    pub kind: ThinEdgeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinEdgeKind {
    Direct {
        file: PathBuf,
        line: usize,
    },
    Dynamic {
        raw_expr: String,
        file: PathBuf,
        line: usize,
    },
    TableAccess {
        modes: Vec<AccessModeTag>,
        write_kinds: Vec<String>,
        file: PathBuf,
        line: usize,
    },
    DependsOn {
        file: PathBuf,
        line: usize,
    },
    UsesSequence {
        file: PathBuf,
        line: usize,
    },
    AnchorsOn {
        file: PathBuf,
        line: usize,
    },
    ContainsRoutine,
}

impl ThinEdgeKind {
    /// 与 JSON 导出的 `type` 字段同拼写。
    pub fn tag(&self) -> &'static str {
        match self {
            ThinEdgeKind::Direct { .. } => "direct",
            ThinEdgeKind::Dynamic { .. } => "dynamic",
            ThinEdgeKind::TableAccess { .. } => "table_access",
            ThinEdgeKind::DependsOn { .. } => "depends_on",
            ThinEdgeKind::UsesSequence { .. } => "uses_sequence",
            ThinEdgeKind::AnchorsOn { .. } => "anchors_on",
            ThinEdgeKind::ContainsRoutine => "contains_routine",
        }
    }
}

/// `AccessMode` 按位展开的标签枚举。展开顺序与 JSON `modes` 数组一致
/// （export/json.rs 的 mode_strs 表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessModeTag {
    Read,
    Write,
    LockRead,
    AccessExclusive,
    ShareUpdateExclusive,
    Share,
    ShareRowExclusive,
    Exclusive,
}

impl AccessModeTag {
    fn of_flag(flag: AccessMode) -> Self {
        if flag == AccessMode::Read {
            Self::Read
        } else if flag == AccessMode::Write {
            Self::Write
        } else if flag == AccessMode::LockRead {
            Self::LockRead
        } else if flag == AccessMode::AccessExclusive {
            Self::AccessExclusive
        } else if flag == AccessMode::ShareUpdateExclusive {
            Self::ShareUpdateExclusive
        } else if flag == AccessMode::Share {
            Self::Share
        } else if flag == AccessMode::ShareRowExclusive {
            Self::ShareRowExclusive
        } else {
            Self::Exclusive
        }
    }
}

/// 把 `AccessMode` 位掩码按 JSON `modes` 数组的同一顺序展开为标签序列。
fn expand_modes(modes: AccessMode) -> Vec<AccessModeTag> {
    [
        AccessMode::Read,
        AccessMode::Write,
        AccessMode::LockRead,
        AccessMode::AccessExclusive,
        AccessMode::ShareUpdateExclusive,
        AccessMode::Share,
        AccessMode::ShareRowExclusive,
        AccessMode::Exclusive,
    ]
    .into_iter()
    .filter(|flag| modes.contains(*flag))
    .map(AccessModeTag::of_flag)
    .collect()
}

/// 瘦图投影：遍历已加载 store 的图，按模块头注释的映射表过滤/转换。
fn project_thin_graph(graph: &crate::graph::CodeGraph) -> ThinGraph {
    let mut nodes = Vec::new();
    let mut kept: HashSet<usize> = HashSet::new();
    for idx in graph.node_indices() {
        let kind = match &graph[idx] {
            Node::Procedure {
                id,
                location,
                partial,
                ..
            } => ThinNodeKind::Procedure {
                schema: id.schema.clone(),
                package: id.package.clone(),
                name: id.name.clone(),
                file: location.file.as_ref().clone(),
                line: location.line,
                partial: *partial,
            },
            Node::Function {
                id,
                location,
                partial,
                ..
            } => ThinNodeKind::Function {
                schema: id.schema.clone(),
                package: id.package.clone(),
                name: id.name.clone(),
                file: location.file.as_ref().clone(),
                line: location.line,
                partial: *partial,
            },
            Node::Package { schema, name, .. } => ThinNodeKind::Package {
                schema: schema.clone(),
                name: name.clone(),
            },
            Node::Table {
                schema,
                name,
                explicit,
                system,
                ..
            } => ThinNodeKind::Table {
                schema: schema.clone(),
                name: name.clone(),
                explicit: *explicit,
                system: *system,
            },
            Node::View {
                schema,
                name,
                explicit,
                system,
                ..
            } => ThinNodeKind::View {
                schema: schema.clone(),
                name: name.clone(),
                explicit: *explicit,
                system: *system,
            },
            Node::MaterializedView { schema, name, .. } => ThinNodeKind::MaterializedView {
                schema: schema.clone(),
                name: name.clone(),
            },
            Node::Sequence {
                schema,
                name,
                explicit,
                ..
            } => ThinNodeKind::Sequence {
                schema: schema.clone(),
                name: name.clone(),
                explicit: *explicit,
            },
            Node::Unresolved { raw_expr, .. } => ThinNodeKind::Unresolved {
                raw_expr: (**raw_expr).clone(),
            },
            Node::BuiltinFunction { name, .. } => {
                ThinNodeKind::BuiltinFunction { name: name.clone() }
            }
            // Java/JSP/mapper/trigger/type/index/synonym/event/custom：跳过
            _ => continue,
        };
        kept.insert(idx.index());
        nodes.push(ThinNodeEntry {
            id: idx.index(),
            kind,
        });
    }

    let mut edges = Vec::new();
    for edge_ref in graph.edge_references() {
        let (source, target) = (edge_ref.source().index(), edge_ref.target().index());
        if !kept.contains(&source) || !kept.contains(&target) {
            continue;
        }
        let kind = match edge_ref.weight() {
            Edge::DirectCall { location, .. } => ThinEdgeKind::Direct {
                file: location.file.as_ref().clone(),
                line: location.line,
            },
            Edge::DynamicCall { raw_expr, location } => ThinEdgeKind::Dynamic {
                raw_expr: raw_expr.clone(),
                file: location.file.as_ref().clone(),
                line: location.line,
            },
            Edge::TableAccess {
                modes,
                write_kinds,
                location,
                ..
            } => {
                let mut wk: Vec<String> = write_kinds
                    .iter()
                    .map(crate::graph::write_kind_label)
                    .map(String::from)
                    .collect();
                wk.sort();
                ThinEdgeKind::TableAccess {
                    modes: expand_modes(*modes),
                    write_kinds: wk,
                    file: location.file.as_ref().clone(),
                    line: location.line,
                }
            }
            Edge::DependsOn { location, .. } => ThinEdgeKind::DependsOn {
                file: location.file.as_ref().clone(),
                line: location.line,
            },
            Edge::UsesSequence { location } => ThinEdgeKind::UsesSequence {
                file: location.file.as_ref().clone(),
                line: location.line,
            },
            Edge::AnchorsOn { location, .. } => ThinEdgeKind::AnchorsOn {
                file: location.file.as_ref().clone(),
                line: location.line,
            },
            Edge::ContainsRoutine => ThinEdgeKind::ContainsRoutine,
            _ => continue,
        };
        edges.push(ThinEdge {
            source,
            target,
            kind,
        });
    }

    ThinGraph { nodes, edges }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T2.7.4：DiffStatus 真值表——五种组合的 is_up_to_date 判定。
    #[test]
    fn diff_status_up_to_date_requires_store_and_no_changes() {
        let base = DiffStatus {
            store_exists: true,
            stale_store_version: None,
            added: vec![],
            modified: vec![],
            deleted: vec![],
        };
        assert!(base.is_up_to_date());

        let no_store = DiffStatus {
            store_exists: false,
            ..base.clone()
        };
        assert!(!no_store.is_up_to_date());

        let stale = DiffStatus {
            stale_store_version: Some(7),
            ..base.clone()
        };
        assert!(!stale.is_up_to_date());

        let added = DiffStatus {
            added: vec![PathBuf::from("a.sql")],
            ..base.clone()
        };
        assert!(!added.is_up_to_date());

        let modified = DiffStatus {
            modified: vec![PathBuf::from("a.sql")],
            ..base.clone()
        };
        assert!(!modified.is_up_to_date());

        let deleted = DiffStatus {
            deleted: vec![PathBuf::from("a.sql")],
            ..base
        };
        assert!(!deleted.is_up_to_date());
    }

    /// stale_version 的三态映射（含 legacy 无头文件 → Some(0)）。
    #[test]
    fn stale_version_maps_peek_result_to_staleness() {
        assert_eq!(stale_version(Some(7)), Some(7), "过期版本透传");
        assert_eq!(stale_version(Some(STORE_VERSION)), None, "当前版本不算过期");
        assert_eq!(stale_version(None), Some(0), "无头/损坏文件按最大过期处理");
    }

    /// T2.7.5 等价测试的核心部分：手工构造覆盖**全部** Node/Edge 变体的图，
    /// 断言投影后的 ThinGraph 与 JSON 口径 1:1——锁死映射表，防止 codeweb
    /// 演进时 ThinGraph 与 `export/json.rs` 漂移。
    #[test]
    fn thin_projection_pins_the_full_variant_mapping_table() {
        use crate::graph::{
            CodeGraph, DataFlowKind, Edge, Node, RoutineId, SourceLocation, WriteKind,
        };
        use std::sync::Arc;

        let loc = |file: &str, line: usize| SourceLocation {
            file: Arc::new(PathBuf::from(file)),
            line,
        };
        let mut g = CodeGraph::new();

        // 保留类节点（顺序即 petgraph NodeIndex 0..n）
        let proc = g.add_node(Node::Procedure {
            id: RoutineId {
                schema: Some("sch".into()),
                package: Some("pkg".into()),
                name: "p1".into(),
                kind: crate::graph::RoutineKind::Procedure,
            },
            location: loc("a.sql", 1),
            partial: true,
            body_sql: vec![],
        });
        let func = g.add_node(Node::Function {
            id: RoutineId {
                schema: None,
                package: None,
                name: "f1".into(),
                kind: crate::graph::RoutineKind::Function,
            },
            location: loc("b.sql", 2),
            partial: false,
            body_sql: vec![],
        });
        let pkg = g.add_node(Node::Package {
            schema: Some("s".into()),
            name: "pkg".into(),
            location: loc("c.sql", 3),
        });
        let table = g.add_node(Node::Table {
            schema: None,
            name: "t1".into(),
            explicit: true,
            system: false,
            location: None,
            columns: Box::new(vec![]),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });
        let view = g.add_node(Node::View {
            schema: None,
            name: "v1".into(),
            explicit: false,
            system: true,
            location: None,
            columns: Box::new(vec![]),
            ddl_source: None,
        });
        let mview = g.add_node(Node::MaterializedView {
            schema: None,
            name: "mv1".into(),
            location: loc("d.sql", 4),
            columns: Box::new(vec![]),
            ddl_source: None,
        });
        let seq = g.add_node(Node::Sequence {
            schema: None,
            name: "s1".into(),
            explicit: true,
            location: None,
        });
        let unresolved = g.add_node(Node::Unresolved {
            raw_expr: Box::new("dyn_proc()".into()),
            context: Box::new("ctx".into()),
        });
        let builtin = g.add_node(Node::BuiltinFunction {
            name: "SUBSTR".into(),
            category: "string".into(),
            domain: "core".into(),
            location: loc("e.sql", 5),
        });
        let _all_kept = [
            proc.index(),
            func.index(),
            pkg.index(),
            table.index(),
            view.index(),
            mview.index(),
            seq.index(),
            unresolved.index(),
            builtin.index(),
        ];

        // 跳过类节点（Java/JSP/mapper/trigger/type/index/synonym/event/custom）
        let java_sql = g.add_node(Node::JavaSql {
            class_name: None,
            method_name: None,
            extraction_method: "jdbc".into(),
            java_file: PathBuf::from("X.java"),
            line: 1,
            sql: None,
        });
        let trigger = g.add_node(Node::Trigger {
            name: "trg".into(),
            table: vec![],
            location: loc("f.sql", 6),
        });
        let typ = g.add_node(Node::Type {
            schema: None,
            name: "ty".into(),
            type_kind: "composite".into(),
            location: loc("g.sql", 7),
        });
        let index = g.add_node(Node::Index {
            name: None,
            table_schema: None,
            table_name: "t1".into(),
            unique: false,
            global: false,
            index_method: None,
            columns: vec![],
            tablespace: None,
            where_clause: None,
            constraint: None,
            location: loc("h.sql", 8),
        });
        let synonym = g.add_node(Node::Synonym {
            schema: None,
            name: "sy".into(),
            target_schema: None,
            target_name: "t1".into(),
            location: loc("i.sql", 9),
        });
        let event = g.add_node(Node::Event {
            name: "ev".into(),
            location: loc("j.sql", 10),
        });
        let custom = g.add_node(Node::Custom {
            type_name: Box::new("custom".into()),
            label: Box::new("lbl".into()),
            key_fields: Box::new(Default::default()),
            properties: Box::new(crate::graph::JsonMap::new()),
            location: None,
        });
        let java_method = g.add_node(Node::JavaMethod {
            fqn: "F".into(),
            class_fqn: "C".into(),
            name: "m".into(),
            signature: "()".into(),
            file: PathBuf::from("Y.java"),
            line: 2,
        });
        let java_class = g.add_node(Node::JavaClass {
            fqn: "C".into(),
            name: "C".into(),
            package: None,
            file: PathBuf::from("Z.java"),
            line: 3,
        });
        let mapper = g.add_node(Node::MappedStatement {
            namespace: "ns".into(),
            statement_id: "st".into(),
            kind: "select".into(),
            xml_file: PathBuf::from("m.xml"),
            line: 4,
            sql: None,
        });
        let jsp_page = g.add_node(Node::JspPage {
            path: PathBuf::from("p.jsp"),
            display_name: "p".into(),
            line: 5,
            url_pattern: None,
        });
        let jsp_sql = g.add_node(Node::JspSql {
            sql: "select 1".into(),
            file: PathBuf::from("p.jsp"),
            line: 6,
            kind: crate::parser::jsp_types::JspSqlKind::Scriptlet,
            parsed: false,
        });
        let _all_skipped = [
            java_sql.index(),
            trigger.index(),
            typ.index(),
            index.index(),
            synonym.index(),
            event.index(),
            custom.index(),
            java_method.index(),
            java_class.index(),
            mapper.index(),
            jsp_page.index(),
            jsp_sql.index(),
        ];

        // 保留边：7 类，覆盖各字段形态
        g.add_edge(
            proc,
            func,
            Edge::DirectCall {
                scope: crate::graph::CallScope::External,
                location: loc("a.sql", 11),
            },
        );
        g.add_edge(
            proc,
            unresolved,
            Edge::DynamicCall {
                raw_expr: "EXECUTE IMMEDIATE 'call dyn_proc()'".into(),
                location: loc("a.sql", 12),
            },
        );
        g.add_edge(
            func,
            table,
            Edge::TableAccess {
                flow_kind: DataFlowKind::DmlAccess,
                modes: AccessMode::Read | AccessMode::Write,
                write_kinds: HashSet::from([WriteKind::Insert, WriteKind::SelectInto]),
                location: loc("b.sql", 13),
                column_analysis: None,
            },
        );
        g.add_edge(
            func,
            view,
            Edge::DependsOn {
                location: loc("b.sql", 14),
                column_analysis: None,
            },
        );
        g.add_edge(
            func,
            seq,
            Edge::UsesSequence {
                location: loc("b.sql", 15),
            },
        );
        g.add_edge(
            func,
            mview,
            Edge::AnchorsOn {
                kind: crate::parser::AnchorKind::PercentRowType,
                column: None,
                site: crate::parser::AnchorSite::ReturnType,
                location: loc("b.sql", 16),
            },
        );
        g.add_edge(pkg, proc, Edge::ContainsRoutine);

        // 应被过滤的边：非保留 kind × 保留端点
        g.add_edge(
            proc,
            func,
            Edge::UsesBuiltinFunction {
                location: loc("a.sql", 17),
            },
        );
        g.add_edge(
            proc,
            trigger,
            Edge::TriggersRoutine {
                location: loc("a.sql", 18),
            },
        );
        // 保留 kind 但一端在跳过节点集
        g.add_edge(
            java_sql,
            table,
            Edge::TableAccess {
                flow_kind: DataFlowKind::DmlAccess,
                modes: AccessMode::Read,
                write_kinds: HashSet::new(),
                location: loc("X.java", 7),
                column_analysis: None,
            },
        );

        let thin = project_thin_graph(&g);

        // 节点：9 保留 + 0 跳过，逐项全等断言
        assert_eq!(thin.nodes.len(), 9, "kept node count");
        let by_id: std::collections::HashMap<usize, &ThinNodeKind> =
            thin.nodes.iter().map(|n| (n.id, &n.kind)).collect();
        assert_eq!(
            by_id[&proc.index()],
            &ThinNodeKind::Procedure {
                schema: Some("sch".into()),
                package: Some("pkg".into()),
                name: "p1".into(),
                file: PathBuf::from("a.sql"),
                line: 1,
                partial: true,
            }
        );
        assert_eq!(
            by_id[&table.index()],
            &ThinNodeKind::Table {
                schema: None,
                name: "t1".into(),
                explicit: true,
                system: false,
            }
        );
        assert_eq!(
            by_id[&builtin.index()],
            &ThinNodeKind::BuiltinFunction {
                name: "SUBSTR".into()
            }
        );
        assert_eq!(thin.nodes.iter().map(|n| n.kind.tag()).count(), 9);

        // 边：仅 7 条保留边；非保留 kind / 一端跳过的边全部消失
        assert_eq!(thin.edges.len(), 7, "kept edge count: {:#?}", thin.edges);
        let direct = thin
            .edges
            .iter()
            .find(|e| e.kind.tag() == "direct")
            .expect("direct edge kept");
        assert_eq!((direct.source, direct.target), (proc.index(), func.index()));
        match &direct.kind {
            ThinEdgeKind::Direct { file, line } => {
                assert_eq!((file, *line), (&PathBuf::from("a.sql"), 11));
            }
            other => panic!("wrong kind: {other:?}"),
        }

        let table_access = thin
            .edges
            .iter()
            .find(|e| e.kind.tag() == "table_access")
            .expect("table_access kept");
        match &table_access.kind {
            ThinEdgeKind::TableAccess {
                modes,
                write_kinds,
                file,
                line,
            } => {
                // 展开顺序与 JSON mode_strs 表一致；write_kinds 排序后转标签
                assert_eq!(
                    modes,
                    &[AccessModeTag::Read, AccessModeTag::Write],
                    "bitflags must expand in the JSON order"
                );
                assert_eq!(write_kinds, &["insert", "select_into"], "sorted labels");
                assert_eq!((file, *line), (&PathBuf::from("b.sql"), 13));
            }
            other => panic!("wrong kind: {other:?}"),
        }

        // contains_routine 边（无 location 载荷）
        let contains = thin
            .edges
            .iter()
            .find(|e| e.kind.tag() == "contains_routine")
            .expect("contains_routine kept");
        assert_eq!(
            (contains.source, contains.target),
            (pkg.index(), proc.index())
        );
    }
}
