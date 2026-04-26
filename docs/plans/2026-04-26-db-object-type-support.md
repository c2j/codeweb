# 数据库对象类型全面支持 — 设计文档

**目标**：将 TYPE、SEQUENCE、INDEX、MATERIALIZED VIEW、SYNONYM、EVENT 六种数据库对象纳入图模型，并提取 Routine 对 TYPE/SEQUENCE 的引用关系。

**架构**：变更涉及 6 层：Model → Extractor → Builder → Export → TUI → CLI。遵循与 Package/Trigger/View 相同的模式。

**前提**：ogsql-parser 已提供所有对应的 `Statement` 枚举变体和 AST 结构体。无需修改上游。

---

## 一、ogsql-parser 能力矩阵

### 1.1 DDL 定义（已支持）

| Statement 变体 | AST 结构体 | 关键字段 |
|---|---|---|
| `CreateType` | `CreateTypeStatement` | `name: ObjectName`, `type_kind: TypeKind` (Composite/Enum/Base/Table/Range/Shell) |
| `CreateSequence` | `CreateSequenceStatement` | `name: ObjectName`, `start`, `increment`, `min/max_value`, `cache`, `cycle` |
| `CreateIndex` | `CreateIndexStatement` | `name: Option<ObjectName>`, `table: ObjectName`, `columns: Vec<IndexColumn>`, `unique` |
| `CreateMaterializedView` | `CreateMaterializedViewStatement` | `name: ObjectName`, `query: Box<SelectStatement>`, `with_data` |
| `CreateSynonym` | `CreateSynonymStatement` | `name: ObjectName`, `target: ObjectName`, `public` |
| `CreateEvent` | `CreateEventStatement` | `name: String`, `raw_rest: String`（字段较粗） |

### 1.2 引用关系（需模式匹配提取）

ogsql-parser **没有**专门的 `visit_type_ref` 或 `visit_sequence_ref`。引用关系需从以下 AST 节点做模式匹配：

| 引用场景 | AST 位置 | 提取方式 |
|---|---|---|
| 变量声明 `v_rec my_type` | `PlDataType::TypeName("my_type")` | 在 `visit_pl_declaration` 中检查 |
| `%TYPE` 锚定 | `PlDataType::PercentType { table, column }` | 表引用 |
| `%ROWTYPE` 锚定 | `PlDataType::PercentRowType("t1")` | 表引用 |
| 类型转换 `expr::my_type` | `Expr::TypeCast { type_name: DataType::Custom(name) }` | 在 `visit_expr` 中检查 |
| `nextval('seq')` | `Expr::FunctionCall { name: ["nextval"], args: [Literal::String("seq")] }` | 匹配函数名 + 参数 |
| `seq.NEXTVAL` | `Expr::FieldAccess { object: ColumnRef(["seq"]), field: "NEXTVAL" }` | 匹配字段名 |
| `currval('seq')` / `setval(...)` | 同 `nextval` | 匹配函数名 |

### 1.3 Visitor 可用钩子

| 方法 | 覆盖范围 | 本次用途 |
|---|---|---|
| `visit_pl_declaration` | ✅ 所有 `PlDeclaration` | 检查 `PlDataType::TypeName` 提取 TYPE 引用 |
| `visit_expr` | ✅ 所有 `Expr` | 检查 `FunctionCall`/`FieldAccess` 提取 SEQUENCE 引用，检查 `TypeCast` 提取 TYPE 引用 |
| `visit_pl_block` | ✅ 进入 PL 块 | 递归遍历 |
| `visit_statement` | ✅ 所有 Statement | 识别 CREATE 语句 |

不存在 `visit_data_type`、`visit_type_ref`、`visit_sequence_ref`。

---

## 二、Model 层设计

### 2.1 新增 Node 变体（`src/graph/mod.rs`）

```rust
/// A user-defined TYPE (composite, enum, range, base, table-of, shell).
Type {
    schema: Option<String>,
    name: String,
    type_kind: String,        // "composite" | "enum" | "range" | "base" | "table" | "shell"
    location: SourceLocation,
},

/// A database SEQUENCE.
Sequence {
    schema: Option<String>,
    name: String,
    location: SourceLocation,
},

/// A database INDEX.
Index {
    name: Option<String>,
    table_schema: Option<String>,
    table_name: String,
    unique: bool,
    location: SourceLocation,
},

/// A MATERIALIZED VIEW.
MaterializedView {
    schema: Option<String>,
    name: String,
    location: SourceLocation,
},

/// A database SYNONYM (alias for another object).
Synonym {
    schema: Option<String>,
    name: String,
    target_schema: Option<String>,
    target_name: String,
    location: SourceLocation,
},

/// A scheduled EVENT (openGauss equivalent of JOB).
Event {
    name: String,
    location: SourceLocation,
},
```

### 2.2 新增 NodeKey 变体（`src/graph/key.rs`）

```rust
Type           { schema: Option<String>, name: String },
Sequence       { schema: Option<String>, name: String },
Index          { name: Option<String>, table_name: String },
MaterializedView { schema: Option<String>, name: String },
Synonym        { schema: Option<String>, name: String },
Event          { name: String },
```

NodeKey::Display 格式：
- `type:schema.name` / `type:name`
- `seq:schema.name` / `seq:name`
- `idx:table_name[name]` / `idx:table_name`
- `mview:schema.name` / `mview:name`
- `syn:schema.name→target` / `syn:name→target`
- `event:name`

### 2.3 新增 Edge 变体（`src/graph/mod.rs`）

```rust
/// A routine references a custom TYPE (variable/parameter/return type).
ReferencesType {
    location: SourceLocation,
},

/// A routine uses a SEQUENCE (nextval/currval/setval).
UsesSequence {
    location: SourceLocation,
},

/// An INDEX belongs to a TABLE.
IndexesTable {
    location: SourceLocation,
},

/// A SYNONYM aliases another object.
AliasesObject {
    location: SourceLocation,
},
```

### 2.4 边复用策略

| 新对象 → 目标 | 边类型 | 说明 |
|---|---|---|
| MatView → Table | `Edge::TableAccess { modes: Read }` | 与 View 一致 |
| Event → Routine | `Edge::TriggersRoutine` | 与 Trigger 一致 |
| Synonym → target | 🆕 `Edge::AliasesObject` | 新概念 |
| Routine → Type | 🆕 `Edge::ReferencesType` | 核心新增 |
| Routine → Sequence | 🆕 `Edge::UsesSequence` | 核心新增 |
| Index → Table | 🆕 `Edge::IndexesTable` | 元数据关系 |

### 2.5 Node::file() 更新

新增 6 个 match arm，返回 `&location.file`（除 EVENT 可能无文件路径外）。

---

## 三、Extractor 层设计

### 3.1 新增 `TypeSequenceRefExtractor`（`src/parser/extractor.rs`）

**设计决策**：使用单个 Extractor 同时提取 TYPE 和 SEQUENCE 引用，因为它们都遍历相同的 AST 子树（PL block），且数据量小。不拆分为两个 Extractor 避免重复遍历。

```rust
/// 引用信息
#[derive(Debug, Clone)]
pub struct TypeRef {
    pub type_name: String,     // 原始类型名
    pub location: SourceLocation,
}

#[derive(Debug, Clone)]
pub struct SequenceRef {
    pub sequence_name: String, // 序列名
    pub via: SequenceRefVia,   // 通过何种方式引用
    pub location: SourceLocation,
}

#[derive(Debug, Clone)]
pub enum SequenceRefVia {
    Nextval,
    Currval,
    Setval,
    DotNextval,   // seq.NEXTVAL
    DotCurrval,   // seq.CURRVAL
}

pub struct TypeSequenceRefExtractor {
    /// 已知的自定义 TYPE 名称集合（CREATE TYPE 解析后填入）
    known_types: HashSet<String>,
    pub type_refs: Vec<TypeRef>,
    pub sequence_refs: Vec<SequenceRef>,
    file: Arc<PathBuf>,
}
```

### 3.2 TYPE 引用提取策略

**核心问题**：`PlDataType::TypeName(String)` 不区分内置类型（`INTEGER`、`VARCHAR`）和自定义类型（`my_type`）。需要已知 TYPE 集合做过滤。

**两阶段方案**：

```
Pass 1: create_sql_nodes
  - 解析所有 CREATE TYPE 语句 → 填入 type_index（HashSet<String>）
  - 解析所有其他 CREATE 语句 → 创建节点

Pass 1.5: create_type_sequence_edges（新增 Pass）
  - 再次遍历所有文件的 PL blocks
  - TypeSequenceRefExtractor 携带 type_index 做匹配
  - 提取 TYPE 引用和 SEQUENCE 引用
  - 创建边

Pass 2: create_sql_edges（现有，不变）
  - 提取调用关系和表访问
```

**TypeName 匹配规则**：
1. 精确匹配 `type_index`（`"my_type"` 或 `"schema.my_type"`）
2. 若 TypeName 不含 `.`，尝试 `"schema.TypeName"`（当前 schema 补全 — 可选，初期不做）
3. 不匹配 → 忽略（视为内置类型）

**匹配来源**：
- `PlDeclaration::Variable { data_type: PlDataType::TypeName(s) }` → 检查 `known_types`
- `Expr::TypeCast { type_name: DataType::Custom(ObjectName, _) }` → 检查 `known_types`

### 3.3 SEQUENCE 引用提取策略

**无需已知集合**，直接通过模式匹配识别：

```rust
// 模式 1: nextval('seq_name') / currval('seq_name') / setval('seq_name', ...)
fn visit_expr(&mut self, expr: &Expr) -> VisitorResult {
    if let Expr::FunctionCall { name, args, .. } = expr {
        let func_name = name.join(".").to_lowercase();
        let via = match func_name.as_str() {
            "nextval" => Some(SequenceRefVia::Nextval),
            "currval" => Some(SequenceRefVia::Currval),
            "setval" => Some(SequenceRefVia::Setval),
            _ => None,
        };
        if let (Some(via), Some(first_arg)) = (via, args.first()) {
            if let Expr::Literal(Literal::String(s)) = first_arg {
                // 可能带引号或 schema 前缀
                self.sequence_refs.push(SequenceRef {
                    sequence_name: normalize_seq_name(s),
                    via,
                    location: self.make_location(0),
                });
            }
            // 也可能是 Expr::PlVariable(ObjectName) — 动态引用
        }
    }

    // 模式 2: seq_name.NEXTVAL / seq_name.CURRVAL
    if let Expr::FieldAccess { object, field } = expr {
        let field_upper = field.to_uppercase();
        let via = match field_upper.as_str() {
            "NEXTVAL" => Some(SequenceRefVia::DotNextval),
            "CURRVAL" => Some(SequenceRefVia::DotCurrval),
            _ => None,
        };
        if let (Some(via), Expr::ColumnRef(name)) = (via, object.as_ref()) {
            self.sequence_refs.push(SequenceRef {
                sequence_name: name.join("."),
                via,
                location: self.make_location(0),
            });
        }
        // 也可能是 Expr::PlVariable(name)
    }

    VisitorResult::Continue
}
```

### 3.4 Visitor 实现要点

```rust
impl Visitor for TypeSequenceRefExtractor {
    fn visit_pl_declaration(&mut self, decl: &PlDeclaration) -> VisitorResult {
        if let PlDeclaration::Variable(var) = decl {
            if let PlDataType::TypeName(type_name) = &var.data_type {
                if self.is_known_type(type_name) {
                    self.type_refs.push(TypeRef {
                        type_name: type_name.clone(),
                        location: self.make_location(0),
                    });
                }
            }
            // PercentType 和 PercentRowType 是表引用，已在 TableAccessExtractor 中处理
        }
        VisitorResult::Continue
    }

    fn visit_expr(&mut self, expr: &Expr) -> VisitorResult {
        // TYPE 引用：TypeCast to Custom type
        if let Expr::TypeCast { type_name: DataType::Custom(name, _), .. } = expr {
            let type_str = name.join(".");
            if self.is_known_type(&type_str) {
                self.type_refs.push(TypeRef {
                    type_name: type_str,
                    location: self.make_location(0),
                });
            }
        }
        // SEQUENCE 引用：FunctionCall / FieldAccess 模式匹配
        self.extract_sequence_refs(expr);
        VisitorResult::Continue
    }
}
```

---

## 四、Builder 层设计（`src/graph/builder.rs`）

### 4.1 Pass 结构调整

现有 Pass 结构：
```
Pass 1: create_sql_nodes   — 创建所有 SQL 节点（Procedure, Function, Package, Trigger, View）
Pass 2: create_sql_edges   — 创建调用边 + 表访问边
```

新 Pass 结构：
```
Pass 1: create_sql_nodes         — 扩展：增加 Type, Sequence, Index, MatView, Synonym, Event 节点
        ↑ 同时构建 type_index (HashSet<String>) 和 sequence_index (HashMap)
Pass 2: create_sql_edges         — 不变：调用边 + 表访问边
Pass 2.5 (新增): create_object_ref_edges — TYPE/SEQUENCE 引用边
Pass 3+: ibatis/java 节点         — 不变
```

### 4.2 create_sql_nodes 扩展

在 `create_sql_nodes` 的 `match &info.statement` 中新增分支：

```rust
Statement::CreateType(t) => {
    let (schema, name) = split_object_name(&t.name);
    let type_kind_str = match &t.type_kind {
        TypeKind::Composite { .. } => "composite",
        TypeKind::Enum { .. } => "enum",
        TypeKind::Base { .. } => "base",
        TypeKind::Table { .. } => "table",
        TypeKind::Range { .. } => "range",
        TypeKind::Shell => "shell",
    };
    let node = Node::Type { schema, name, type_kind: type_kind_str.to_string(), location };
    let idx = graph.add_node(node);
    // 填入 type_index
    let key = match &schema { Some(s) => format!("{}.{}", s, name), None => name.clone() };
    type_index.insert(key, idx);
    // 也注册短名（无 schema）
    type_index.entry(name.clone()).or_insert(idx);
}

Statement::CreateSequence(s) => {
    let (schema, name) = split_object_name(&s.name);
    let node = Node::Sequence { schema, name, location };
    let idx = graph.add_node(node);
    // 填入 sequence_index
    let key = match &schema { Some(s) => format!("{}.{}", s, name), None => name.clone() };
    sequence_index.insert(key, idx);
}

Statement::CreateIndex(i) => {
    let (table_schema, table_name) = split_object_name(&i.table);
    let name = i.name.as_ref().map(|n| n.last().cloned().unwrap_or_default());
    let node = Node::Index { name, table_schema, table_name, unique: i.unique, location };
    let idx = graph.add_node(node);
    // 创建 IndexesTable 边
    let table_key = match &node.table_schema {
        Some(s) => format!("{}.{}", s, node.table_name),
        None => node.table_name.clone(),
    };
    let table_idx = *table_index.entry(table_key).or_insert_with(|| {
        graph.add_node(Node::Table { schema: ..., name: ... })
    });
    graph.add_edge(idx, table_idx, Edge::IndexesTable { location });
}

Statement::CreateMaterializedView(v) => {
    let (schema, name) = split_object_name(&v.name);
    let node = Node::MaterializedView { schema, name, location };
    let mv_idx = graph.add_node(node);
    // 复用 View 的表引用提取逻辑
    let mut extractor = TableAccessExtractor::new();
    let wrapped = Statement::Select(v.query.as_ref().clone());
    walk_statement(&mut extractor, &wrapped);
    for access in &extractor.accesses {
        // 创建 TableAccess(Read) 边 — 与 View 相同
    }
}

Statement::CreateSynonym(s) => {
    let (schema, name) = split_object_name(&s.name);
    let (target_schema, target_name) = split_object_name(&s.target);
    let node = Node::Synonym { schema, name, target_schema, target_name, location };
    let syn_idx = graph.add_node(node);
    // 尝试解析 target — 可能指向任何对象类型
    // 先尝试 proc_index, table_index, type_index, sequence_index
    // 找到则创建 AliasesObject 边，否则创建 Unresolved 节点
}

Statement::CreateEvent(e) => {
    let node = Node::Event { name: e.name.clone(), location };
    let event_idx = graph.add_node(node);
    // EVENT 的 raw_rest 无法可靠解析，不尝试提取调用关系
    // 如未来 ogsql-parser 增强 EVENT AST，可在此扩展
}
```

### 4.3 新增 Pass: create_object_ref_edges

```rust
fn create_object_ref_edges(
    files: &[ParsedFile],
    graph: &mut CodeGraph,
    proc_index: &HashMap<RoutineId, NodeIndex>,
    type_index: &HashMap<String, NodeIndex>,
    sequence_index: &HashMap<String, NodeIndex>,
) {
    for file in files {
        let file_arc: Arc<PathBuf> = Arc::new(file.path.clone());
        for info in &file.statements {
            // 找到当前语句关联的 routine node（如果有的话）
            let routine_idx = get_routine_index(info, proc_index);
            if routine_idx.is_none() { continue; }
            let routine_idx = routine_idx.unwrap();

            let mut extractor = TypeSequenceRefExtractor::new(file_arc.clone(), &type_index.keys().cloned().collect());
            walk_statement(&mut extractor, &info.statement);

            // 创建 TYPE 引用边
            for type_ref in &extractor.type_refs {
                if let Some(&type_idx) = resolve_type(&type_ref.type_name, type_index) {
                    graph.add_edge(routine_idx, type_idx, Edge::ReferencesType {
                        location: type_ref.location.clone(),
                    });
                }
                // 找不到则忽略 — 可能是内置类型或跨文件引用
            }

            // 创建 SEQUENCE 引用边
            for seq_ref in &extractor.sequence_refs {
                let seq_idx = sequence_index.get(&seq_ref.sequence_name)
                    .or_else(|| sequence_index.get(&seq_ref.sequence_name.to_lowercase()));
                if let Some(&seq_idx) = seq_idx {
                    graph.add_edge(routine_idx, seq_idx, Edge::UsesSequence {
                        location: seq_ref.location.clone(),
                    });
                }
            }
        }
    }
}
```

### 4.4 Package 内 Routine 的引用

对于 Package 内的 Procedure/Function，需在遍历其 PL block 时设置 `TypeSequenceRefExtractor` 的 current_procedure 上下文。复用 `collect_package_call_edges` 的模式：

```rust
fn collect_package_type_sequence_refs(
    pkg_name: &ObjectName,
    pkg_items: &[PackageItem],
    extractor: &mut TypeSequenceRefExtractor,
) {
    for item in pkg_items {
        match item {
            PackageItem::Procedure(p) => {
                if let Some(ref block) = p.block {
                    walk_pl_block(extractor, block);
                }
            }
            PackageItem::Function(f) => {
                if let Some(ref block) = f.block {
                    walk_pl_block(extractor, block);
                }
            }
            PackageItem::Raw(_) => {}
        }
    }
}
```

---

## 五、全层变更清单

### 5.1 文件变更清单

| 文件 | 变更类型 | 内容 |
|---|---|---|
| `src/graph/mod.rs` | 修改 | +6 Node 变体, +4 Edge 变体, Node::file() 更新 |
| `src/graph/key.rs` | 修改 | +6 NodeKey 变体, Display, from_node 更新 |
| `src/graph/builder.rs` | 修改 | +6 Statement 分支, +1 新 Pass |
| `src/parser/extractor.rs` | 修改 | +TypeSequenceRefExtractor |
| `src/parser/mod.rs` | 修改 | 导出新类型 |
| `src/export/json.rs` | 修改 | +6 NodeKindJson, +4 EdgeKindJson |
| `src/export/dot.rs` | 修改 | +6 node shape, +4 edge style |
| `src/export/mermaid.rs` | 修改 | +6 node shape, +4 edge arrow |
| `src/tui/app.rs` | 修改 | +6 node type label + color |
| `src/main.rs` | 修改 | +6 stats counter, +6 list label |
| `src/graph/store.rs` | 可能修改 | 如有序列化 schema 变更 |

### 5.2 DOT/Mermaid 可视化约定

| Node 类型 | DOT shape | DOT fillcolor | Mermaid shape | TUI Color |
|---|---|---|---|---|
| Type | `parallelogram` | `lightyellow` | `[/", "/]` | `Yellow` |
| Sequence | `box3d` | `lightgreen` | `([`, `])` | `LightGreen` |
| Index | `house` | 无 | `{{`, `}}` | `Gray` |
| MaterializedView | `cylinder` | `lightcyan` | `([`, `])` | `Cyan` |
| Synonym | `trapezium` | `lavender` | `[/`, `/]` | `Magenta` |
| Event | `octagon` | `lightsalmon` | `{{`, `}}` | `LightRed` |

### 5.3 Edge 可视化约定

| Edge 类型 | DOT 颜色 | Mermaid 箭头 |
|---|---|---|
| ReferencesType | `color=teal` | `-.->` |
| UsesSequence | `color=olive` | `-.->` |
| IndexesTable | `color=gray, style=dotted` | `-.->` |
| AliasesObject | `color=purple, style=dashed` | `==>` |

---

## 六、测试 Fixture SQL

### F1: TYPE 定义 + 引用

```sql
CREATE TYPE address_t AS (
    street  VARCHAR(200),
    city    VARCHAR(100),
    zip     VARCHAR(20)
);

CREATE OR REPLACE PROCEDURE print_address(p_addr address_t) AS $$
BEGIN
    RAISE NOTICE 'City: %', p_addr.city;
END;
$$;
```

**期望图**：
- Node `Type(address_t, composite)` — 1 节点
- Node `Procedure(print_address)` — 1 节点
- Edge `print_address → address_t` (ReferencesType)

### F2: SEQUENCE 定义 + 引用

```sql
CREATE SEQUENCE user_id_seq START WITH 1 INCREMENT BY 1;

CREATE OR REPLACE FUNCTION next_user_id() RETURNS BIGINT AS $$
BEGIN
    RETURN nextval('user_id_seq');
END;
$$ LANGUAGE plpgsql;
```

**期望图**：
- Node `Sequence(user_id_seq)` — 1 节点
- Node `Function(next_user_id)` — 1 节点
- Edge `next_user_id → user_id_seq` (UsesSequence)

### F3: SEQUENCE 的 .NEXTVAL 引用

```sql
CREATE SEQUENCE order_seq;

CREATE OR REPLACE PROCEDURE create_order() AS $$
BEGIN
    INSERT INTO t_orders(id, name) VALUES (order_seq.NEXTVAL, 'test');
END;
$$;
```

**期望图**：
- Edge `create_order → order_seq` (UsesSequence, via=DotNextval)

### F4: INDEX 节点

```sql
CREATE UNIQUE INDEX idx_users_email ON t_users(email) WHERE active = true;
```

**期望图**：
- Node `Index(idx_users_email, table=t_users, unique=true)` — 1 节点
- Node `Table(t_users)` — 1 节点（如已存在则复用）
- Edge `idx_users_email → t_users` (IndexesTable)

### F5: MATERIALIZED VIEW + 表引用

```sql
CREATE MATERIALIZED VIEW mv_order_summary AS
SELECT user_id, COUNT(*) as order_count
FROM t_orders
GROUP BY user_id
WITH DATA;
```

**期望图**：
- Node `MaterializedView(mv_order_summary)` — 1 节点
- Node `Table(t_orders)` — 1 节点
- Edge `mv_order_summary → t_orders` (TableAccess { modes: Read })

### F6: SYNONYM 解析

```sql
CREATE OR REPLACE PROCEDURE remote_pkg.do_work(p_id INT) AS $$
BEGIN
    NULL;
END;
$$;

CREATE SYNONYM my_work FOR remote_pkg.do_work;
```

**期望图**：
- Node `Procedure(remote_pkg.do_work)` — 1 节点
- Node `Synonym(my_work → remote_pkg.do_work)` — 1 节点
- Edge `my_work → remote_pkg.do_work` (AliasesObject)

### F7: EVENT（最小化）

```sql
CREATE EVENT nightly_cleanup ON SCHEDULE EVERY 1 DAY DO CALL cleanup_proc();
```

**期望图**：
- Node `Event(nightly_cleanup)` — 1 节点
- 注意：`raw_rest` 包含 `ON SCHEDULE EVERY 1 DAY DO CALL cleanup_proc()`
- 初期**不解析** EVENT body，仅创建节点。如果 ogsql-parser 未来增强 EVENT AST，可在此扩展。

---

## 七、实现任务分解（TDD）

### Task 1: Model — Node/Edge/NodeKey 新增变体

**文件**：`src/graph/mod.rs`, `src/graph/key.rs`

**步骤**：
1. 写失败测试：构造新增的 Node 变体，验证 Node::file() 返回正确路径
2. 添加 6 个 Node 变体 + 4 个 Edge 变体
3. 更新 Node::file() match
4. 添加 6 个 NodeKey 变体 + Display + from_node
5. `cargo test --lib graph` — PASS
6. Commit: `feat: add Type/Sequence/Index/MatView/Synonym/Event node and edge types`

### Task 2: Export — JSON/DOT/Mermaid 更新

**文件**：`src/export/json.rs`, `src/export/dot.rs`, `src/export/mermaid.rs`

**步骤**：
1. 添加 NodeKindJson 和 EdgeKindJson 变体
2. 更新 to_json / to_dot / to_mermaid 的 match arms
3. `cargo test` — PASS
4. Commit: `feat: export support for new database object types`

### Task 3: Extractor — TypeSequenceRefExtractor

**文件**：`src/parser/extractor.rs`

**步骤**：
1. 写失败测试：从包含 `v_rec my_type` 和 `nextval('seq')` 的 PL block 中提取引用
2. 实现 TypeSequenceRefExtractor
3. 写测试：seq.NEXTVAL 模式、TypeCast 模式、nextval/currval/setval 模式
4. `cargo test --lib parser::extractor` — PASS
5. Commit: `feat: TypeSequenceRefExtractor for TYPE and SEQUENCE references`

### Task 4: Builder — DDL 节点创建

**文件**：`src/graph/builder.rs`

**步骤**：
1. 写失败测试：CREATE TYPE → Type 节点、CREATE SEQUENCE → Sequence 节点、etc.
2. 在 create_sql_nodes 中添加 6 个 Statement match 分支
3. 构建 type_index 和 sequence_index
4. `cargo test --lib graph::builder` — PASS
5. Commit: `feat: builder creates Type/Sequence/Index/MatView/Synonym/Event nodes`

### Task 5: Builder — 引用边创建

**文件**：`src/graph/builder.rs`

**步骤**：
1. 写失败测试：Procedure 中引用 TYPE/SEQUENCE → 创建 ReferencesType/UsesSequence 边
2. 实现 create_object_ref_edges 新 Pass
3. 在 build_graph_internal 中调用新 Pass
4. `cargo test --lib graph::builder` — PASS
5. Commit: `feat: builder creates TYPE reference and SEQUENCE usage edges`

### Task 6: TUI + CLI 更新

**文件**：`src/tui/app.rs`, `src/main.rs`

**步骤**：
1. 添加新 node type label 和 color
2. 添加新 stats counter
3. `cargo test` — PASS
4. Commit: `feat: TUI and CLI support for new database object types`

### Task 7: 集成测试

**文件**：`tests/integration_test.rs`, `lib/codeweb-e2e-demo/sql/`

**步骤**：
1. 添加 fixture SQL 文件（types.sql, sequences.sql, indexes.sql, matviews.sql, synonyms.sql, events.sql）
2. 写集成测试验证完整 pipeline
3. `cargo test --test integration_test` — PASS
4. Commit: `test: integration tests for all new database object types`

### Task 8: 验证

1. `cargo clippy -- -D warnings` — 无警告
2. `cargo fmt -- --check` — 无变更
3. `cargo test` — 全部 PASS

---

## 八、风险与限制

| 风险 | 影响 | 应对 |
|---|---|---|
| `PlDataType::TypeName` 含内置类型名 | 误报 TYPE 引用 | 使用 type_index 过滤，仅匹配已解析的 CREATE TYPE |
| SEQUENCE 名可能拼接到动态 SQL 中 | 遗漏 | 接受局限，与 DynamicCall 一致 |
| EVENT body 是 `raw_rest` 字符串 | 无法提取调用关系 | 仅创建 Event 节点，等待上游增强 |
| SYNONYM target 可能指向任何对象类型 | 解析复杂度 | 多策略查找：proc → table → type → sequence，找不到则标记 Unresolved |
| `%TYPE` / `%ROWTYPE` 引用的是表列类型 | 概念混淆 | 这些是表引用，归入 TableAccessExtractor，不在 TYPE 引用中处理 |

## 九、时间估计

| 任务 | 估计时间 |
|---|---|
| Task 1: Model | 0.5 天 |
| Task 2: Export | 0.5 天 |
| Task 3: Extractor | 1 天 |
| Task 4: Builder DDL | 0.5 天 |
| Task 5: Builder 引用 | 0.5 天 |
| Task 6: TUI + CLI | 0.5 天 |
| Task 7: 集成测试 | 0.5 天 |
| Task 8: 验证 | 0.5 天 |
| **总计** | **~4 天** |
