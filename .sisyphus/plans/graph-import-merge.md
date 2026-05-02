# Graph Import & Merge — Codeweb Graph Exchange Format (CGEF)

## TL;DR

> **Quick Summary**: 为 codeweb 新增 Graph 导入/合并能力。定义一种全新的、自描述的 JSON 交换格式（CGEF），企业内部工具按此格式输出私有代码的 Graph 数据，codeweb 通过 `import` 子命令解析为独立 GraphStore，再通过 `merge` 子命令与自有 Graph 合并，实现跨公有/私有边界的链路追溯。
>
> **Deliverables**:
> - CGEF JSON 格式规范文档（含 JSON Schema）
> - `src/import/` 模块：格式解析器 + 自定义节点类型支持
> - `NodeKey::Custom` + `Node::Custom` 变体 + 对应的 Edge/导出扩展
> - `codeweb import` CLI 子命令
> - `codeweb merge` CLI 子命令（复用 + 扩展现有 GraphStore::merge）
> - 完整的 TDD 测试套件
>
> **Estimated Effort**: Large
> **Parallel Execution**: YES — 4 waves
> **Critical Path**: T1(格式定义) → T2(类型扩展) → T3(解析器) → T4(导入CLI) → T5(合并扩展) → T6(合并CLI) → Final

---

## Context

### Original Request
企业私有代码不方便提供扫描，但希望能利用 codeweb 的强大能力。需要定义一种格式，让企业内工具按 codeweb 定义的格式要求输出 Graph 结果，并将该 Graph 与 codeweb 支持的 graph 进行合并。

### Interview Summary
**Key Discussions**:
- 格式选择: 全新独立 JSON 格式，不复用现有 export/json.rs
- 自定义节点: JSON 内嵌 schema（自描述式），codeweb 按注册 schema 解析
- 路径处理: 相对路径 + `--prefix` 挂载点前缀映射
- CLI: 新增 `codeweb import` 和 `codeweb merge` 子命令
- 导入产出: 独立 GraphStore 文件（.bincode/.json）
- 版本化: format_version 字段
- 测试: TDD

**Research Findings**:
- `GraphStore::merge()` 已存在（src/graph/store.rs:242-313），能按 NodeKey 去重、边三元组去重、TableAccess 边合并 modes
- `Node` enum 有 ~20 种变体，`Edge` enum 有 ~15 种变体，`NodeKey` enum 有 ~18 种变体
- JSON 导出已存在（src/export/json.rs），但仅导出无导入
- `SourceLocation` 使用 `Arc<PathBuf>`，导入时需处理路径映射
- `ResolutionEngine` 有 8 策略回退链
- 当前 GraphStore version = 2
- `AccessMode` 使用 bitflags，`WriteKind` 是 enum

### Self-Performed Gap Analysis (替代 Metis)

**Identified Gaps** (已处理):
- 自定义节点的边类型也需要可扩展 → 格式中定义 `custom_edge_types`，Node 新增 `Custom` 变体，Edge 新增 `CustomEdge` 变体
- 导入幂等性 → 同一文件导入两次产生相同 GraphStore
- 空图导入 → 正确处理，产生空 store
- 自定义节点与标准节点之间的边 → 支持标准边类型连接自定义节点
- 格式版本兼容 → 严格版本检查，不支持的高版本直接报错
- 大规模图导入（100K+ 节点）→ 解析器需流式或分批处理
- NodeKey 去重时自定义节点的行为 → Custom 节点按 (type_name, key_fields) 去重

---

## Work Objectives

### Core Objective
定义 CGEF（Codeweb Graph Exchange Format）JSON 格式并实现完整的导入/合并管道，使企业无需暴露源码即可将私有代码的语义关系融入 codeweb 的分析图谱。

### Concrete Deliverables
- `src/import/mod.rs` — import 模块入口
- `src/import/format.rs` — CGEF 格式的 Rust 类型定义（serde 结构体）
- `src/import/schema.rs` — 内嵌 schema 解析与自定义类型注册
- `src/import/parser.rs` — CGEF JSON → 内部 Node/Edge 的转换器
- `src/import/validator.rs` — 格式校验 + 版本检查
- `src/import/path_mapper.rs` — 相对路径 → 绝对路径映射
- `src/graph/mod.rs` — Node/Edge/NodeKey 新增 Custom 变体
- `src/graph/store.rs` — merge 扩展以支持 Custom 节点
- `src/export/json.rs` — JSON 导出支持 Custom 节点
- `src/export/dot.rs` — DOT 导出支持 Custom 节点
- `src/export/mermaid.rs` — Mermaid 导出支持 Custom 节点
- `src/main.rs` — `import` 和 `merge` CLI 子命令
- `tests/import_*.rs` — 集成测试
- `docs/cgef-schema.json` — JSON Schema 定义文件

### Definition of Done
- [ ] `cargo test` 全部通过
- [ ] `cargo clippy -- -D warnings` 无警告
- [ ] `cargo fmt -- --check` 通过
- [ ] 标准 CGEF JSON 文件可成功 import 为独立 GraphStore
- [ ] 自定义节点类型的 CGEF JSON 可成功 import
- [ ] 导入的 GraphStore 可与自有 GraphStore 成功 merge
- [ ] merge 后的 graph 可 trace 跨公有/私有节点的调用链
- [ ] CGEF 格式文档（JSON Schema）完整

### Must Have
- CGEF JSON 格式定义（含 format_version、node_schemas、nodes、edges）
- 支持全部现有 Node 类型（Procedure, Function, Table, View, MappedStatement, JavaMethod 等）
- 支持全部现有 Edge 类型（DirectCall, DynamicCall, TableAccess 等）
- 企业自定义 node type 通过内嵌 schema 支持
- 相对路径 + --prefix 挂载点映射
- `codeweb import` 产生独立 GraphStore
- `codeweb merge` 合并多个 GraphStore
- TDD 测试覆盖

### Must NOT Have (Guardrails)
- ❌ 不要修改现有 Node/Edge 变体的语义或字段（只能新增 Custom 变体）
- ❌ 不要构建插件系统运行时（只做 schema 注册 + 数据透传）
- ❌ 不要实现企业端工具（那是企业的事）
- ❌ 不要在导入时做名称解析（ResolutionEngine 不应用于导入数据）
- ❌ 不要构建图转换/查询引擎（仅存储和合并）
- ❌ 不要实现导入文件的加密/认证（文件安全性由企业保障）
- ❌ 不要过度抽象 — 自定义节点用简单的 label + properties map，不要搞成完整的类型系统

---

## Verification Strategy

> **ZERO HUMAN INTERVENTION** — ALL verification is agent-executed. No exceptions.

### Test Decision
- **Infrastructure exists**: YES（cargo test）
- **Automated tests**: TDD — RED → GREEN → REFACTOR
- **Framework**: Rust built-in `#[cfg(test)]` + `tests/` integration tests
- **Each task**: 先写失败测试 → 最小实现通过 → 重构

### QA Policy
Every task MUST include agent-executed QA scenarios.
Evidence saved to `.sisyphus/evidence/task-{N}-{scenario-slug}.{ext}`.

- **Parser/Module**: Use Bash (cargo test) — 运行测试，断言通过
- **CLI**: Use Bash (cargo run) — 运行命令，检查输出和退出码
- **Format**: Use Bash — 构造 JSON 文件，import，验证结果

---

## Execution Strategy

### Parallel Execution Waves

```
Wave 1 (Start Immediately — 基础设施 + 类型定义):
├── Task 1: CGEF JSON 格式规范 + JSON Schema 文件 [writing]
├── Task 2: Node/Edge/NodeKey 新增 Custom 变体 [deep]
├── Task 3: CGEF Rust 类型定义 (import/format.rs) [quick]
└── Task 4: 路径映射模块 (import/path_mapper.rs) [quick]

Wave 2 (After Wave 1 — 核心解析):
├── Task 5: 内嵌 schema 解析 (import/schema.rs) (depends: 3) [deep]
├── Task 6: CGEF JSON 解析器 (import/parser.rs) (depends: 2, 3, 4) [deep]
├── Task 7: 格式校验器 (import/validator.rs) (depends: 3) [quick]
└── Task 8: GraphStore::merge 扩展 (depends: 2) [unspecified-high]

Wave 3 (After Wave 2 — CLI + 集成):
├── Task 9: codeweb import CLI 子命令 (depends: 5, 6, 7) [unspecified-high]
├── Task 10: codeweb merge CLI 子命令 (depends: 8) [quick]
├── Task 11: 导出格式支持 Custom 节点 (depends: 2) [unspecified-high]
└── Task 12: 端到端集成测试 (depends: 9, 10) [deep]

Wave FINAL (After ALL tasks — 4 parallel reviews):
├── Task F1: Plan compliance audit (oracle)
├── Task F2: Code quality review (unspecified-high)
├── Task F3: Real manual QA (unspecified-high)
└── Task F4: Scope fidelity check (deep)
-> Present results -> Get explicit user okay

Critical Path: T1 → T3 → T6 → T9 → T12 → Final
               T2 → T6 → T9 → T12
Parallel Speedup: ~60% faster than sequential
Max Concurrent: 4 (Wave 1)
```

### Dependency Matrix

| Task | Depends On | Blocks | Wave |
|------|-----------|--------|------|
| 1 | - | 3, 5, 6 | 1 |
| 2 | - | 6, 8, 11 | 1 |
| 3 | 1 | 5, 6, 7 | 1 |
| 4 | - | 6 | 1 |
| 5 | 3 | 9 | 2 |
| 6 | 2, 3, 4 | 9 | 2 |
| 7 | 3 | 9 | 2 |
| 8 | 2 | 10 | 2 |
| 9 | 5, 6, 7 | 12 | 3 |
| 10 | 8 | 12 | 3 |
| 11 | 2 | - | 3 |
| 12 | 9, 10 | Final | 3 |

### Agent Dispatch Summary

- **Wave 1**: **4** — T1 → `writing`, T2 → `deep`, T3 → `quick`, T4 → `quick`
- **Wave 2**: **4** — T5 → `deep`, T6 → `deep`, T7 → `quick`, T8 → `unspecified-high`
- **Wave 3**: **4** — T9 → `unspecified-high`, T10 → `quick`, T11 → `unspecified-high`, T12 → `deep`
- **FINAL**: **4** — F1 → `oracle`, F2 → `unspecified-high`, F3 → `unspecified-high`, F4 → `deep`

---

## TODOs

- [ ] 1. CGEF JSON 格式规范 + JSON Schema 文件

  **What to do**:
  - 设计完整的 CGEF（Codeweb Graph Exchange Format）JSON 格式规范
  - 编写 JSON Schema 文件 `docs/cgef-schema.json`，可被企业用于验证其输出
  - 格式核心结构：

  ```json
  {
    "format_version": 1,
    "metadata": {
      "source": "enterprise-tool-name",
      "generated_at": "2026-01-15T10:30:00Z",
      "description": "Optional human-readable description"
    },
    "node_schemas": {
      "dubbo_service": {
        "display_name": "Dubbo RPC Service",
        "key_fields": ["interface", "version", "group"],
        "properties": {
          "registry": { "type": "string", "description": "Service registry address" },
          "timeout_ms": { "type": "integer" }
        }
      }
    },
    "edge_schemas": {
      "dubbo_invokes": {
        "display_name": "Dubbo RPC Invocation",
        "source_types": ["java_method", "dubbo_service"],
        "target_types": ["dubbo_service"],
        "properties": {
          "protocol": { "type": "string" }
        }
      }
    },
    "nodes": [
      {
        "id": "node-001",
        "type": "procedure",
        "key": { "schema": "pkg_order", "package": null, "name": "create_order" },
        "location": { "file": "sql/pkg_order.sql", "line": 42 },
        "properties": { "partial": false }
      },
      {
        "id": "node-002",
        "type": "dubbo_service",
        "key": { "interface": "com.example.OrderService", "version": "1.0", "group": "" },
        "location": { "file": "java/OrderService.java", "line": 15 },
        "properties": { "registry": "nacos://127.0.0.1:8848", "timeout_ms": 3000 }
      }
    ],
    "edges": [
      {
        "source": "node-001",
        "target": "node-002",
        "type": "calls_procedure",
        "location": { "file": "java/OrderService.java", "line": 45 }
      },
      {
        "source": "node-002",
        "target": "node-003",
        "type": "dubbo_invokes",
        "location": { "file": "java/OrderService.java", "line": 50 },
        "properties": { "protocol": "dubbo" }
      }
    ]
  }
  ```

  - 标准节点类型枚举值：`procedure`, `function`, `package`, `trigger`, `type`, `sequence`, `index`, `materialized_view`, `synonym`, `event`, `table`, `view`, `mapped_statement`, `java_sql`, `java_method`, `java_class`
  - 标准边类型枚举值：`direct`, `dynamic`, `calls_procedure`, `invokes_mapper`, `calls_java`, `contains_method`, `extends`, `implements`, `table_access`, `contains_routine`, `triggers_routine`, `references_type`, `uses_sequence`, `indexes_table`, `aliases_object`
  - 自定义节点类型在 `node_schemas` 中声明，`type` 字段使用自定义名称
  - 自定义边类型在 `edge_schemas` 中声明，`type` 字段使用自定义名称
  - 所有 `location.file` 使用相对路径

  **Must NOT do**:
  - 不要实现任何代码逻辑（纯文档/规范工作）
  - 不要在 JSON Schema 中硬编码 codeweb 内部实现细节

  **Recommended Agent Profile**:
  - **Category**: `writing`
    - Reason: 纯格式规范设计，需要精确的 JSON Schema 编写
  - **Skills**: [`brainstorming`]
    - `brainstorming`: 格式设计需要创造性思考，确保扩展性和易用性

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 1 (with Tasks 2, 3, 4)
  - **Blocks**: Tasks 3, 5, 6
  - **Blocked By**: None

  **References**:

  **Pattern References** (existing code to follow):
  - `src/export/json.rs:14-213` — 现有 JSON 导出格式的完整结构，理解节点/边的字段命名约定
  - `src/graph/mod.rs:131-254` — Node enum 所有变体定义，确保格式能覆盖每种节点类型的关键字段
  - `src/graph/mod.rs:256-318` — Edge enum 所有变体定义，确保格式能覆盖每种边类型的关键字段
  - `src/graph/key.rs:5-73` — NodeKey enum 定义，标准节点的 key 字段结构由此决定

  **API/Type References**:
  - `src/graph/mod.rs:61-122` — RoutineId 结构，procedure/function 的 key = { schema, package, name }
  - `src/graph/mod.rs:14-37` — AccessMode bitflags + WriteKind enum，table_access 边需要 modes 和 write_kinds 字段

  **WHY Each Reference Matters**:
  - json.rs: 理解现有导出如何序列化每种节点类型，确保导入格式覆盖相同的字段
  - Node enum: 格式必须能无损表示所有现有节点类型
  - Edge enum: 格式必须能无损表示所有现有边类型
  - NodeKey: 标准节点的唯一标识方式，决定格式中 `key` 字段的结构
  - RoutineId: procedure/function 的 key 结构需与内部 RoutineId 对应

  **Acceptance Criteria**:

  **QA Scenarios (MANDATORY):**

  ```
  Scenario: JSON Schema 文件语法正确
    Tool: Bash
    Preconditions: docs/cgef-schema.json 存在
    Steps:
      1. 安装 JSON Schema 验证工具 (check-jsonschema 或类似)
      2. 构造一个包含标准 procedure 节点和 direct 边的最小合法 CGEF JSON
      3. 用 schema 验证该 JSON
    Expected Result: 验证通过，无错误
    Failure Indicators: schema 解析错误或验证失败
    Evidence: .sisyphus/evidence/task-1-schema-valid.txt

  Scenario: JSON Schema 拒绝无效格式
    Tool: Bash
    Preconditions: docs/cgef-schema.json 存在
    Steps:
      1. 构造缺少 format_version 字段的 JSON
      2. 构造 nodes 中缺少 id 字段的 JSON
      3. 用 schema 验证
    Expected Result: 验证失败，报告缺少必填字段
    Failure Indicators: 无效 JSON 通过了验证
    Evidence: .sisyphus/evidence/task-1-schema-reject.txt

  Scenario: 自定义节点类型符合 schema 约定
    Tool: Bash
    Preconditions: docs/cgef-schema.json 存在
    Steps:
      1. 构造包含 node_schemas 和自定义 type 节点的 CGEF JSON
      2. 用 schema 验证
    Expected Result: 验证通过
    Failure Indicators: 自定义节点被拒绝
    Evidence: .sisyphus/evidence/task-1-custom-schema.txt
  ```

  **Commit**: YES (groups with T2-T4)
  - Message: `feat(import): define CGEF format specification and JSON schema`
  - Files: `docs/cgef-schema.json`

- [ ] 2. Node/Edge/NodeKey 新增 Custom 变体

  **What to do**:
  - 在 `Node` enum 中新增 `Custom` 变体：

  ```rust
  Custom {
      type_name: String,         // e.g., "dubbo_service"
      label: String,             // 显示名称，如 "com.example.OrderService"
      key_fields: BTreeMap<String, String>,  // 唯一标识字段
      properties: BTreeMap<String, serde_json::Value>,  // 任意属性
      location: Option<SourceLocation>,
  },
  ```

  - 在 `Edge` enum 中新增 `CustomEdge` 变体：

  ```rust
  CustomEdge {
      type_name: String,         // e.g., "dubbo_invokes"
      properties: BTreeMap<String, serde_json::Value>,
      location: Option<SourceLocation>,
  },
  ```

  - 在 `NodeKey` enum 中新增 `Custom` 变体：

  ```rust
  Custom {
      type_name: String,
      key: String,               // key_fields 的确定性序列化（如 sorted JSON）
  },
  ```

  - 更新 `NodeKey::from_node()` 以处理 Custom 变体
  - 更新 `NodeKey` 的 Display impl 以处理 Custom（格式：`custom:type_name:key`）
  - 更新 `src/graph/traverse.rs` 中所有 match Node/Edge 的地方，确保 Custom 变体被正确处理
  - 更新 `src/graph/builder.rs` 中的 dedup 逻辑以处理 Custom 节点
  - 所有新增变体添加 `#[derive(Serialize, Deserialize)]`
  - **TDD**: 先写测试，确认 Custom 节点能创建、序列化、反序列化

  **Must NOT do**:
  - 不要修改现有 Node/Edge/NodeKey 变体的字段或语义
  - 不要为 Custom 变体实现 ResolutionEngine 策略（自定义节点不做名称解析）
  - 不要添加超出 BTreeMap<String, Value> 的类型系统

  **Recommended Agent Profile**:
  - **Category**: `deep`
    - Reason: 需要理解所有 match 站点，确保不遗漏任何地方
  - **Skills**: [`test-driven-development`]
    - `test-driven-development`: TDD 策略，先写测试

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 1 (with Tasks 1, 3, 4)
  - **Blocks**: Tasks 6, 8, 11
  - **Blocked By**: None

  **References**:

  **Pattern References**:
  - `src/graph/mod.rs:131-254` — Node enum 完整定义，Custom 变体需遵循相同的字段模式
  - `src/graph/mod.rs:256-318` — Edge enum 完整定义
  - `src/graph/key.rs:5-144` — NodeKey 完整定义 + Display impl + from_node()

  **API/Type References**:
  - `src/graph/traverse.rs:121-135` — trace_chain 函数，需处理 Custom 节点的 edge_label
  - `src/graph/builder.rs:796-961` — dedup 逻辑，Custom 节点需参与去重
  - `src/graph/store.rs:441-462` — StoreStats，需新增 custom_nodes 字段

  **Test References**:
  - `src/graph/mod.rs` 底部的 `#[cfg(test)]` — 如果存在，遵循其测试模式

  **WHY Each Reference Matters**:
  - Node/Edge enum: Custom 变体必须与现有变体保持一致的 derive 和字段风格
  - NodeKey: Display 格式需与现有格式统一（`prefix:identifier` 模式）
  - traverse.rs: Custom 节点在 trace 结果中需要合理的显示
  - builder.rs dedup: Custom 节点也需去重，使用 type_name + key 作为去重键
  - StoreStats: 需要统计自定义节点数量

  **Acceptance Criteria**:

  **If TDD:**
  - [ ] Test: 创建 Custom 节点，序列化为 JSON，反序列化回来，字段一致
  - [ ] Test: NodeKey::Custom 的 Display 输出为 `custom:type_name:key`
  - [ ] Test: NodeKey::from_node() 对 Custom 节点返回正确的 NodeKey::Custom
  - [ ] Test: Custom 节点参与 dedup 不会 panic
  - [ ] `cargo test` PASS

  **QA Scenarios:**

  ```
  Scenario: Custom 节点可序列化和反序列化
    Tool: Bash (cargo test)
    Preconditions: Custom 变体已添加
    Steps:
      1. 运行 cargo test test_custom_node_serde
      2. 验证测试通过
    Expected Result: 测试通过，序列化 → 反序列化字段完全一致
    Failure Indicators: 测试失败或字段丢失
    Evidence: .sisyphus/evidence/task-2-serde.txt

  Scenario: 所有 match 站点处理 Custom 变体
    Tool: Bash (cargo clippy)
    Preconditions: Custom 变体已添加
    Steps:
      1. 运行 cargo clippy -- -D warnings
    Expected Result: 无 exhaustive match 相关警告
    Failure Indicators: "variant `Custom` not handled" 警告
    Evidence: .sisyphus/evidence/task-2-clippy.txt
  ```

  **Commit**: YES (groups with T1, T3, T4)
  - Message: `feat(import): add Custom variant to Node, Edge, NodeKey`
  - Files: `src/graph/mod.rs`, `src/graph/key.rs`, `src/graph/traverse.rs`, `src/graph/builder.rs`, `src/graph/store.rs`

- [ ] 3. CGEF Rust 类型定义 (import/format.rs)

  **What to do**:
  - 创建 `src/import/` 目录和 `mod.rs` 入口
  - 创建 `src/import/format.rs`，定义 CGEF JSON 对应的 Rust serde 结构体：

  ```rust
  // 顶层结构
  pub struct CgefDocument {
      pub format_version: u32,
      pub metadata: CgefMetadata,
      pub node_schemas: HashMap<String, CgefNodeSchema>,
      pub edge_schemas: HashMap<String, CgefEdgeSchema>,
      pub nodes: Vec<CgefNode>,
      pub edges: Vec<CgefEdge>,
  }

  pub struct CgefMetadata {
      pub source: String,
      pub generated_at: String,
      pub description: Option<String>,
  }

  pub struct CgefNodeSchema {
      pub display_name: String,
      pub key_fields: Vec<String>,
      pub properties: HashMap<String, CgefPropertyDef>,
  }

  pub struct CgefEdgeSchema {
      pub display_name: String,
      pub source_types: Vec<String>,
      pub target_types: Vec<String>,
      pub properties: HashMap<String, CgefPropertyDef>,
  }

  pub struct CgefPropertyDef {
      #[serde(rename = "type")]
      pub prop_type: String,
      pub description: Option<String>,
  }

  pub struct CgefNode {
      pub id: String,
      #[serde(rename = "type")]
      pub node_type: String,
      pub key: serde_json::Value,
      pub location: Option<CgefLocation>,
      pub properties: Option<serde_json::Value>,
  }

  pub struct CgefEdge {
      pub source: String,
      pub target: String,
      #[serde(rename = "type")]
      pub edge_type: String,
      pub location: Option<CgefLocation>,
      pub properties: Option<serde_json::Value>,
  }

  pub struct CgefLocation {
      pub file: String,
      pub line: usize,
  }
  ```

  - 所有结构体 derive `Serialize, Deserialize, Debug, Clone`
  - 实现从 CGEF 标准节点类型字符串到内部 Node 类型的转换常量
  - **TDD**: 先写测试 — 构造合法 JSON → 反序列化 → 验证字段正确

  **Must NOT do**:
  - 不要在此文件中实现解析逻辑（那是 parser.rs 的职责）
  - 不要引用 codeweb 内部类型（如 Node, Edge）— 这是纯数据结构定义

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: 纯数据结构定义 + serde derive，机械性工作
  - **Skills**: [`test-driven-development`]
    - `test-driven-development`: TDD

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 1 (with Tasks 1, 2, 4)
  - **Blocks**: Tasks 5, 6, 7
  - **Blocked By**: Task 1 (格式规范先出，类型定义才有参照)

  **References**:

  **Pattern References**:
  - `src/graph/mod.rs:61-129` — RoutineId + SourceLocation 的 serde 模式，作为参考
  - `Cargo.toml` — serde + serde_json 依赖已存在

  **External References**:
  - Task 1 产出的 `docs/cgef-schema.json` — Rust 类型定义必须与 JSON Schema 完全对应

  **WHY Each Reference Matters**:
  - RoutineId: 展示了如何用 serde 属性定义可序列化的结构体
  - JSON Schema: 类型定义的字段名、嵌套结构必须与 schema 一一对应

  **Acceptance Criteria**:

  **If TDD:**
  - [ ] Test: 最小合法 CGEF JSON 可反序列化为 CgefDocument
  - [ ] Test: 包含 node_schemas 和自定义节点的 CGEF JSON 可正确解析
  - [ ] Test: 缺少 format_version 的 JSON 反序列化失败
  - [ ] `cargo test` PASS

  **QA Scenarios:**

  ```
  Scenario: 最小 CGEF JSON 反序列化
    Tool: Bash (cargo test)
    Steps:
      1. 运行 cargo test test_cgef_deserialize_minimal
    Expected Result: 测试通过，CgefDocument 所有字段正确
    Evidence: .sisyphus/evidence/task-3-deserialize.txt

  Scenario: 包含自定义节点的 CGEF JSON 反序列化
    Tool: Bash (cargo test)
    Steps:
      1. 运行 cargo test test_cgef_deserialize_custom_nodes
    Expected Result: 测试通过，node_schemas 和 custom nodes 正确解析
    Evidence: .sisyphus/evidence/task-3-custom.txt
  ```

  **Commit**: YES (groups with T1, T2, T4)
  - Message: part of `feat(import): define CGEF format and extend node types`
  - Files: `src/import/mod.rs`, `src/import/format.rs`

- [ ] 4. 路径映射模块 (import/path_mapper.rs)

  **What to do**:
  - 创建 `src/import/path_mapper.rs`
  - 实现路径映射逻辑：

  ```rust
  pub struct PathMapper {
      prefix: PathBuf,
  }

  impl PathMapper {
      pub fn new(prefix: Option<&str>) -> Self;
      
      /// 将 CGEF 中的相对路径映射为绝对路径
      /// 如果有 prefix，则 prefix + relative_path
      /// 如果无 prefix，则保持原样
      pub fn map(&self, relative_path: &str) -> PathBuf;
  }
  ```

  - 边界情况处理：
    - prefix 为 None → 路径原样返回
    - 相对路径以 `./` 开头 → 去掉前缀
    - prefix 尾部有无 `/` 均可正确处理
    - 路径规范化（去除 `//`, `./` 等）
  - **TDD**: 先写测试 — 各种路径组合

  **Must NOT do**:
  - 不要检查文件是否实际存在（映射是纯字符串操作）
  - 不要处理绝对路径输入（CGEF 规范要求相对路径）

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: 纯字符串/路径操作，逻辑简单
  - **Skills**: [`test-driven-development`]

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 1 (with Tasks 1, 2, 3)
  - **Blocks**: Task 6
  - **Blocked By**: None

  **References**:

  **Pattern References**:
  - `src/graph/mod.rs:124-129` — SourceLocation 使用 `Arc<PathBuf>`，PathMapper 的输出需兼容
  - `src/parser/scanner.rs` — 文件路径处理模式

  **WHY Each Reference Matters**:
  - SourceLocation: 理解内部路径表示方式，确保 mapper 输出可直接用于构造 SourceLocation

  **Acceptance Criteria**:

  **If TDD:**
  - [ ] Test: prefix="/enterprise/module-a" + "sql/pkg.sql" → "/enterprise/module-a/sql/pkg.sql"
  - [ ] Test: prefix=None + "sql/pkg.sql" → "sql/pkg.sql"
  - [ ] Test: prefix="/a/" + "./sql/pkg.sql" → "/a/sql/pkg.sql"
  - [ ] Test: 路径规范化 (去除 // 和 .)
  - [ ] `cargo test` PASS

  **QA Scenarios:**

  ```
  Scenario: 路径映射正确性
    Tool: Bash (cargo test)
    Steps:
      1. 运行 cargo test test_path_mapper
    Expected Result: 所有路径映射测试通过
    Evidence: .sisyphus/evidence/task-4-path-mapper.txt
  ```

  **Commit**: YES (groups with T1-T3)
  - Message: part of `feat(import): define CGEF format and extend node types`
  - Files: `src/import/path_mapper.rs`

- [ ] 5. 内嵌 schema 解析 (import/schema.rs)

  **What to do**:
  - 创建 `src/import/schema.rs`
  - 将 `CgefNodeSchema` / `CgefEdgeSchema` 注册到运行时 registry：

  ```rust
  pub struct SchemaRegistry {
      node_schemas: HashMap<String, CgefNodeSchema>,
      edge_schemas: HashMap<String, CgefEdgeSchema>,
  }

  impl SchemaRegistry {
      /// 从 CgefDocument 的 node_schemas/edge_schemas 构建注册表
      pub fn from_document(doc: &CgefDocument) -> Result<Self, SchemaError>;
      
      /// 检查节点类型是否为标准类型
      pub fn is_standard_node_type(type_name: &str) -> bool;
      
      /// 检查边类型是否为标准类型
      pub fn is_standard_edge_type(type_name: &str) -> bool;
      
      /// 获取自定义节点 schema
      pub fn get_node_schema(&self, type_name: &str) -> Option<&CgefNodeSchema>;
      
      /// 获取自定义边 schema
      pub fn get_edge_schema(&self, type_name: &str) -> Option<&CgefEdgeSchema>;
      
      /// 验证节点是否符合其 schema 声明
      pub fn validate_node(&self, node: &CgefNode) -> Result<(), SchemaError>;
      
      /// 验证边是否符合其 schema 声明
      pub fn validate_edge(&self, edge: &CgefEdge) -> Result<(), SchemaError>;
  }
  ```

  - 标准类型常量数组：`STANDARD_NODE_TYPES`, `STANDARD_EDGE_TYPES`
  - 验证逻辑：
    - 自定义节点的 `key` 字段必须包含 schema 中声明的所有 `key_fields`
    - 自定义边的 source/target 类型必须在 schema 声明的 `source_types`/`target_types` 范围内
    - 标准类型节点/边不做 schema 验证（交给 parser 处理）
  - **TDD**: 先写测试

  **Must NOT do**:
  - 不要实现完整的 JSON Schema 验证引擎（只验证 key_fields 完整性和类型声明一致性）
  - 不要在 registry 中存储 Node/Edge 实例（只存 schema 元数据）

  **Recommended Agent Profile**:
  - **Category**: `deep`
    - Reason: 需要设计合理的验证规则边界，不过度也不遗漏
  - **Skills**: [`test-driven-development`]

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 2 (with Tasks 6, 7, 8)
  - **Blocks**: Task 9
  - **Blocked By**: Task 3 (format.rs 中的 CgefNodeSchema 等类型)

  **References**:

  **Pattern References**:
  - `src/graph/resolver.rs:14-32` — ResolutionEngine 的 index 模式，SchemaRegistry 遵循类似的 HashMap 索引模式

  **API/Type References**:
  - `src/import/format.rs` (Task 3) — CgefNodeSchema, CgefEdgeSchema, CgefNode, CgefEdge 定义

  **WHY Each Reference Matters**:
  - ResolutionEngine: 参考内部索引构建模式，registry 设计应与之风格一致
  - format.rs 类型: registry 直接消费这些类型

  **Acceptance Criteria**:

  **If TDD:**
  - [ ] Test: 标准类型正确识别（is_standard_node_type("procedure") == true）
  - [ ] Test: 自定义类型正确识别（is_standard_node_type("dubbo_service") == false）
  - [ ] Test: 节点 key 缺少必需字段时 validate_node 报错
  - [ ] Test: 边 source 类型不在 schema 声明中时 validate_edge 报错
  - [ ] Test: 完整合法的自定义节点/边验证通过
  - [ ] `cargo test` PASS

  **QA Scenarios:**

  ```
  Scenario: Schema 验证正确性
    Tool: Bash (cargo test)
    Steps:
      1. 运行 cargo test test_schema_registry
    Expected Result: 所有 schema 验证测试通过
    Evidence: .sisyphus/evidence/task-5-schema.txt
  ```

  **Commit**: YES (groups with T6-T8)
  - Message: part of `feat(import): implement CGEF parser, schema, validator, and merge extension`
  - Files: `src/import/schema.rs`, `src/import/mod.rs`

- [ ] 6. CGEF JSON 解析器 (import/parser.rs)

  **What to do**:
  - 创建 `src/import/parser.rs` — 核心解析器，将 CgefDocument 转换为 codeweb 内部 Node + Edge
  - 这是整个功能的**核心模块**：

  ```rust
  pub struct CgefParser {
      path_mapper: PathMapper,
      schema_registry: SchemaRegistry,
  }

  pub struct ParsedCgef {
      pub graph: CodeGraph,
      pub id_map: HashMap<String, NodeIndex>,  // CGEF node id → petgraph NodeIndex
  }

  impl CgefParser {
      pub fn new(path_mapper: PathMapper, schema_registry: SchemaRegistry) -> Self;
      
      /// 解析完整 CgefDocument 为 CodeGraph
      pub fn parse(&self, doc: CgefDocument) -> Result<ParsedCgef, ParseError>;
  }
  ```

  - **标准节点转换逻辑**（每种类型一个转换函数）：
    - `procedure` → `Node::Procedure` (key 中读取 schema/package/name)
    - `function` → `Node::Function`
    - `table` → `Node::Table`
    - `view` → `Node::View`
    - `mapped_statement` → `Node::MappedStatement` (key 中读取 namespace/statement_id/kind)
    - `java_method` → `Node::JavaMethod` (key 中读取 fqn/class_fqn/name/signature)
    - `java_class` → `Node::JavaClass`
    - `java_sql` → `Node::JavaSql`
    - `package` → `Node::Package`
    - `trigger` → `Node::Trigger`
    - `type` → `Node::Type`
    - `sequence` → `Node::Sequence`
    - `index` → `Node::Index`
    - `materialized_view` → `Node::MaterializedView`
    - `synonym` → `Node::Synonym`
    - `event` → `Node::Event`

  - **自定义节点转换**：
    - 任何 `is_standard_node_type() == false` 的 type → `Node::Custom`
    - `type_name` = CGEF 中的 type 字段
    - `label` = key_fields 中的主要标识（第一个 key_field 的值），或 type_name 本身
    - `key_fields` = key JSON value 转为 BTreeMap<String, String>
    - `properties` = properties JSON value 转为 BTreeMap<String, Value>
    - `location` = 使用 PathMapper 映射后的 SourceLocation

  - **标准边转换逻辑**：
    - `direct` → `Edge::DirectCall`
    - `dynamic` → `Edge::DynamicCall` (properties 中读 raw_expr)
    - `table_access` → `Edge::TableAccess` (properties 中读 modes/write_kinds)
    - ... 其余标准边类型类似

  - **自定义边转换**：
    - 任何 `is_standard_edge_type() == false` → `Edge::CustomEdge`

  - **错误处理**：
    - 未知的节点类型（非标准且无 schema）→ 返回 ParseError
    - 边引用的 source/target id 不存在 → 返回 ParseError
    - 标准 key 字段缺失 → 返回 ParseError
    - 所有错误使用 `thiserror`，不使用 `anyhow`

  - **TDD**: 这是最关键的模块，测试用例需覆盖：
    - 每种标准节点类型的转换
    - 自定义节点转换
    - 每种标准边类型的转换
    - 自定义边转换
    - 错误场景（缺失字段、未知类型、悬空引用）

  **Must NOT do**:
  - 不要在解析器中做 ResolutionEngine 名称解析（导入数据是已解析的）
  - 不要在解析器中做 dedup（那是 GraphStore 的职责）
  - 不要静默忽略错误节点/边 — 明确报错让调用方决定

  **Recommended Agent Profile**:
  - **Category**: `deep`
    - Reason: 核心转换逻辑，需覆盖 16+ 种标准节点和 15+ 种标准边的精确映射，错误处理路径多
  - **Skills**: [`test-driven-development`]

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 2 (with Tasks 5, 7, 8)
  - **Blocks**: Task 9
  - **Blocked By**: Tasks 2 (Custom 变体), 3 (format.rs), 4 (path_mapper)

  **References**:

  **Pattern References**:
  - `src/graph/builder.rs:215-716` — Pass 1 创建 SQL 节点的完整逻辑，理解每种节点类型需要哪些字段
  - `src/graph/builder.rs:1043-1125` — Pass 2 创建 SQL 边的逻辑
  - `src/graph/mod.rs:131-254` — Node enum 每个变体的精确字段定义
  - `src/graph/mod.rs:256-318` — Edge enum 每个变体的精确字段定义

  **API/Type References**:
  - `src/import/format.rs` (Task 3) — CgefNode, CgefEdge 输入类型
  - `src/import/path_mapper.rs` (Task 4) — PathMapper
  - `src/import/schema.rs` (Task 5) — SchemaRegistry
  - `src/graph/mod.rs:61-122` — RoutineId 构造方法 (from_qualified_name, from_object_name)
  - `src/graph/mod.rs:14-37` — AccessMode bitflags 构造, WriteKind 枚举

  **Test References**:
  - `tests/` 目录下现有集成测试模式

  **WHY Each Reference Matters**:
  - builder.rs: 理解"从外部数据构造 Node"的完整模式，解析器必须产生与 builder 相同结构的节点
  - Node/Edge enum: 每种变体的字段必须精确匹配，否则后续 merge/export 会出错
  - RoutineId: procedure/function 节点需要构造 RoutineId，使用其 from_qualified_name 方法
  - AccessMode: table_access 边需要从 JSON 字符串 ["read","write"] 构造 AccessMode bitflags

  **Acceptance Criteria**:

  **If TDD:**
  - [ ] Test: 每种标准节点类型 (16种) 各有一个转换测试
  - [ ] Test: 自定义节点转换正确
  - [ ] Test: 每种标准边类型 (15种) 各有一个转换测试
  - [ ] Test: 自定义边转换正确
  - [ ] Test: 缺失 key 字段报错
  - [ ] Test: 悬空 edge (source/target id 不存在) 报错
  - [ ] Test: 路径映射在节点 location 中正确应用
  - [ ] `cargo test` PASS

  **QA Scenarios:**

  ```
  Scenario: 完整标准 CGEF 解析
    Tool: Bash (cargo test)
    Steps:
      1. 运行 cargo test test_cgef_parser_standard
    Expected Result: 所有 16 种标准节点 + 15 种标准边的转换测试通过
    Evidence: .sisyphus/evidence/task-6-parser-standard.txt

  Scenario: 混合标准+自定义节点解析
    Tool: Bash (cargo test)
    Steps:
      1. 运行 cargo test test_cgef_parser_mixed
    Expected Result: 标准节点映射到对应 Node 变体，自定义节点映射到 Node::Custom
    Evidence: .sisyphus/evidence/task-6-parser-mixed.txt

  Scenario: 错误场景覆盖
    Tool: Bash (cargo test)
    Steps:
      1. 运行 cargo test test_cgef_parser_errors
    Expected Result: 所有错误场景返回明确的 ParseError
    Evidence: .sisyphus/evidence/task-6-parser-errors.txt
  ```

  **Commit**: YES (groups with T5, T7, T8)
  - Message: part of `feat(import): implement CGEF parser, schema, validator, and merge extension`
  - Files: `src/import/parser.rs`

- [ ] 7. 格式校验器 (import/validator.rs)

  **What to do**:
  - 创建 `src/import/validator.rs`
  - 实现 CGEF 文档的预校验（在解析前快速失败）：

  ```rust
  pub struct CgefValidator;

  impl CgefValidator {
      /// 校验 CgefDocument 的基本合法性
      pub fn validate(doc: &CgefDocument) -> Result<Vec<ValidationWarning>, ValidationError>;
  }

  pub enum ValidationError {
      UnsupportedVersion { found: u32, max_supported: u32 },
      EmptyDocument,
      DuplicateNodeId { id: String },
      DuplicateEdgeId { source: String, target: String, edge_type: String },
      InvalidNodeReference { edge_id: String, missing_ref: String },
  }

  pub struct ValidationWarning {
      pub message: String,
      pub severity: Severity,  // Warning, Info
  }
  ```

  - 校验项：
    1. format_version 检查：只支持 version 1（常量 CURRENT_FORMAT_VERSION）
    2. 非空检查：nodes 和 edges 至少有一个非空
    3. 节点 ID 唯一性：所有 node.id 必须唯一
    4. 边引用完整性：每条 edge 的 source/target 必须在 nodes 中存在
    5. 自定义类型一致性：使用了自定义 type 的节点/边，必须在 node_schemas/edge_schemas 中有声明
  - Warnings（不阻断导入）：
    - 自定义节点的 properties 包含 schema 中未声明的字段 → Warning
    - 标准节点有多余的 properties → Info
  - **TDD**

  **Must NOT do**:
  - 不要在此做 schema 字段值类型验证（那是 schema.rs 的职责）
  - 不要在此做节点内容转换（那是 parser.rs 的职责）

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: 逻辑清晰，校验规则明确，主要是集合操作和字符串比较
  - **Skills**: [`test-driven-development`]

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 2 (with Tasks 5, 6, 8)
  - **Blocks**: Task 9
  - **Blocked By**: Task 3 (format.rs 类型)

  **References**:

  **Pattern References**:
  - `src/import/format.rs` (Task 3) — CgefDocument, CgefNode, CgefEdge 类型

  **WHY Each Reference Matters**:
  - format.rs: validator 直接消费这些类型进行校验

  **Acceptance Criteria**:

  **If TDD:**
  - [ ] Test: version=1 通过，version=2 报 UnsupportedVersion
  - [ ] Test: 重复 node id 报 DuplicateNodeId
  - [ ] Test: 边引用不存在的 node 报 InvalidNodeReference
  - [ ] Test: 自定义 type 未在 schemas 中声明报错
  - [ ] Test: 合法文档通过校验（可能返回 warnings 但无 errors）
  - [ ] `cargo test` PASS

  **QA Scenarios:**

  ```
  Scenario: 校验器正确性
    Tool: Bash (cargo test)
    Steps:
      1. 运行 cargo test test_cgef_validator
    Expected Result: 所有校验测试通过
    Evidence: .sisyphus/evidence/task-7-validator.txt
  ```

  **Commit**: YES (groups with T5, T6, T8)
  - Message: part of `feat(import): implement CGEF parser, schema, validator, and merge extension`
  - Files: `src/import/validator.rs`

- [ ] 8. GraphStore::merge 扩展以支持 Custom 节点

  **What to do**:
  - 修改 `src/graph/store.rs` 中的 `GraphStore::merge()` 以正确处理 Custom 节点/边：
    - 现有 merge 按	NodeKey 去重 → `NodeKey::Custom` 的去重逻辑：(type_name, key) 相同视为重复
    - 现有 merge 按 (src_key, dst_key, edge_type) 去重 → `Edge::CustomEdge` 的去重需考虑 type_name
    - `Edge::CustomEdge` 的 properties 合并策略：如果两边有同名属性，后者的值覆盖前者
  - 在 `StoreStats` 中新增 `pub custom_nodes: usize` 和 `pub custom_edges: usize`
  - 确保 `GraphStore::from_graph()` 正确索引 `NodeKey::Custom` 节点
  - 确保 `file_nodes` 和 `file_edges` 正确跟踪 Custom 节点
  - **TDD**

  **Must NOT do**:
  - 不要修改现有去重逻辑的核心行为（只扩展 Custom 变体的处理分支）
  - 不要在 merge 中做名称解析

  **Recommended Agent Profile**:
  - **Category**: `unspecified-high`
    - Reason: 修改已有核心代码，需理解现有 merge 逻辑的每一步
  - **Skills**: [`test-driven-development`]

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 2 (with Tasks 5, 6, 7)
  - **Blocks**: Task 10
  - **Blocked By**: Task 2 (Custom 变体定义)

  **References**:

  **Pattern References**:
  - `src/graph/store.rs:242-313` — 现有 merge 实现的完整逻辑，必须逐行理解
  - `src/graph/store.rs:11-26` — GraphStore 结构定义
  - `src/graph/store.rs:315-400` — from_graph() 和 node_key_index 构建

  **API/Type References**:
  - `src/graph/key.rs` — NodeKey::Custom 变体（Task 2 产出）
  - `src/graph/mod.rs` — Edge::CustomEdge 变体（Task 2 产出）

  **WHY Each Reference Matters**:
  - merge(): 每一步都需审查，确保 Custom 节点/边在去重、合并、索引构建中被正确处理
  - from_graph(): Custom 节点的 NodeKey 索引构建需正确
  - StoreStats: 新增统计字段

  **Acceptance Criteria**:

  **If TDD:**
  - [ ] Test: 两个包含相同 Custom 节点的 store 合并后只有一个节点
  - [ ] Test: 两个包含不同 Custom 节点的 store 合并后保留两个
  - [ ] Test: CustomEdge 的 properties 合并正确（后者覆盖前者）
  - [ ] Test: Custom + 标准 节点混合合并正确
  - [ ] Test: StoreStats 正确统计 custom_nodes 和 custom_edges
  - [ ] `cargo test` PASS

  **QA Scenarios:**

  ```
  Scenario: Custom 节点合并正确性
    Tool: Bash (cargo test)
    Steps:
      1. 运行 cargo test test_merge_custom_nodes
    Expected Result: 去重、属性合并、统计均正确
    Evidence: .sisyphus/evidence/task-8-merge.txt

  Scenario: 混合节点合并不破坏现有行为
    Tool: Bash (cargo test)
    Steps:
      1. 运行 cargo test test_merge_mixed (同时包含标准和自定义节点)
      2. 运行 cargo test test_merge_existing (仅标准节点，确保原有 merge 仍正确)
    Expected Result: 两种场景均通过
    Evidence: .sisyphus/evidence/task-8-merge-mixed.txt
  ```

  **Commit**: YES (groups with T5-T7)
  - Message: part of `feat(import): implement CGEF parser, schema, validator, and merge extension`
  - Files: `src/graph/store.rs`

- [ ] 9. codeweb import CLI 子命令

  **What to do**:
  - 在 `src/main.rs` 中添加 `Import` CLI 子命令（使用 clap derive）：

  ```rust
  #[derive(Parser)]
  pub enum Commands {
      // ... 现有命令 ...
      
      /// Import a CGEF JSON graph file into a standalone GraphStore
      Import {
          /// Path to the CGEF JSON file to import
          #[arg(short, long)]
          file: PathBuf,
          
          /// Output path for the generated GraphStore (.bincode or .json)
          #[arg(short, long)]
          output: PathBuf,
          
          /// Path prefix to prepend to all relative file paths in the CGEF document
          #[arg(short, long)]
          prefix: Option<String>,
          
          /// Project name for the imported GraphStore (default: derived from filename)
          #[arg(short, long)]
          name: Option<String>,
      },
  }
  ```

  - 实现 import 命令的执行逻辑：
    1. 读取 CGEF JSON 文件
    2. 反序列化为 `CgefDocument`
    3. 调用 `CgefValidator::validate()` — 有 error 则退出并报告
    4. 构建 `SchemaRegistry::from_document()`
    5. 构建 `PathMapper::new(prefix)`
    6. 构建 `CgefParser::new(path_mapper, schema_registry)`
    7. 调用 `parser.parse(doc)` 得到 `ParsedCgef`
    8. 将 `ParsedCgef.graph` 包装为 `GraphStore::from_graph()`
    9. 根据 output 扩展名选择 `save_bincode()` 或 `save_json()`
    10. 输出摘要信息（节点数、边数、自定义类型数）
  - 错误处理：所有错误使用 `thiserror`，CLI 层面提供人类可读的错误消息
  - 参考 `src/main.rs` 中现有命令的实现模式（如 `Analyze` 命令）
  - **TDD**: 测试 CLI 命令的正确执行路径

  **Must NOT do**:
  - 不要在 CLI 层做业务逻辑（CLI 只是胶水代码，调用 import/ 模块）
  - 不要修改现有命令的行为

  **Recommended Agent Profile**:
  - **Category**: `unspecified-high`
    - Reason: 需要理解 main.rs 中 clap derive 模式和现有命令结构，整合多个模块
  - **Skills**: [`test-driven-development`]

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 3 (with Tasks 10, 11, 12)
  - **Blocks**: Task 12
  - **Blocked By**: Tasks 5 (schema), 6 (parser), 7 (validator)

  **References**:

  **Pattern References**:
  - `src/main.rs` — 现有 CLI 命令结构（Clap derive 模式），新命令需遵循相同模式
  - `src/graph/store.rs:164-234` — save_bincode/save_json 方法签名

  **API/Type References**:
  - `src/import/mod.rs` — 模块入口，暴露公共 API
  - `src/import/format.rs` (Task 3) — CgefDocument
  - `src/import/validator.rs` (Task 7) — CgefValidator
  - `src/import/schema.rs` (Task 5) — SchemaRegistry
  - `src/import/parser.rs` (Task 6) — CgefParser
  - `src/import/path_mapper.rs` (Task 4) — PathMapper

  **WHY Each Reference Matters**:
  - main.rs: 遵循现有命令的模式保持一致性
  - store.rs: 理解 save_bincode/save_json 的接口
  - import 模块: CLI 是这些模块的消费者，需正确串联调用链

  **Acceptance Criteria**:

  **If TDD:**
  - [ ] Test: `cargo run -- import --file valid.cgef.json --output test.bincode` 成功
  - [ ] Test: 输出文件可被 `GraphStore::load_bincode()` 正确加载
  - [ ] Test: `--prefix` 参数正确应用于节点路径
  - [ ] Test: 无效文件路径返回清晰错误消息
  - [ ] Test: format_version 不匹配返回清晰错误消息
  - [ ] `cargo test` PASS

  **QA Scenarios:**

  ```
  Scenario: 标准 CGEF 导入成功
    Tool: Bash (cargo run)
    Preconditions: 准备一个包含 procedure + table + direct call 的 CGEF JSON 测试文件
    Steps:
      1. cargo run -- import --file tests/fixtures/standard.cgef.json --output /tmp/imported.bincode
      2. 验证退出码为 0
      3. 验证输出文件存在
    Expected Result: 成功导入，输出摘要信息
    Evidence: .sisyphus/evidence/task-9-import-standard.txt

  Scenario: 自定义节点导入成功
    Tool: Bash (cargo run)
    Preconditions: 准备一个包含 node_schemas + 自定义节点的 CGEF JSON
    Steps:
      1. cargo run -- import --file tests/fixtures/custom.cgef.json --output /tmp/custom.bincode --prefix /enterprise/module-a
    Expected Result: 成功导入，--prefix 应用于节点路径
    Evidence: .sisyphus/evidence/task-9-import-custom.txt

  Scenario: 无效格式版本被拒绝
    Tool: Bash (cargo run)
    Preconditions: 准备 format_version=99 的 CGEF JSON
    Steps:
      1. cargo run -- import --file tests/fixtures/bad-version.cgef.json --output /tmp/bad.bincode
    Expected Result: 退出码非 0，错误消息明确指出版本不兼容
    Evidence: .sisyphus/evidence/task-9-import-bad-version.txt
  ```

  **Commit**: YES (groups with T10-T12)
  - Message: part of `feat(import): add import/merge CLI commands and integration tests`
  - Files: `src/main.rs`, `src/import/mod.rs`

- [ ] 10. codeweb merge CLI 子命令

  **What to do**:
  - 在 `src/main.rs` 中添加 `Merge` CLI 子命令：

  ```rust
  Merge {
      /// GraphStore files to merge (at least 2)
      #[arg(short, long, num_args = 2..)]
      stores: Vec<PathBuf>,
      
      /// Output path for the merged GraphStore (.bincode or .json)
      #[arg(short, long)]
      output: PathBuf,
      
      /// Name for the merged GraphStore
      #[arg(short, long)]
      name: Option<String>,
  }
  ```

  - 实现 merge 命令的执行逻辑：
    1. 加载所有指定的 GraphStore 文件（根据扩展名判断 bincode/json）
    2. 调用 `GraphStore::merge(stores, name)`
    3. 保存合并结果
    4. 输出合并摘要（总节点数、总边数、去重统计）
  - 参考 `src/main.rs` 中现有命令的实现模式
  - **TDD**

  **Must NOT do**:
  - 不要修改 `GraphStore::merge()` 的逻辑（在 Task 8 中已完成）
  - 不要添加 merge 策略选项（当前使用默认的"保留首个"策略）

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: CLI 胶水代码，核心逻辑已在 GraphStore::merge 中实现
  - **Skills**: [`test-driven-development`]

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 3 (with Tasks 9, 11, 12)
  - **Blocks**: Task 12
  - **Blocked By**: Task 8 (merge 扩展)

  **References**:

  **Pattern References**:
  - `src/main.rs` — 现有 CLI 命令结构
  - `src/graph/store.rs:242-313` — merge 方法签名和返回值

  **WHY Each Reference Matters**:
  - main.rs: 遵循 CLI 模式
  - store.rs merge: 理解输入输出接口

  **Acceptance Criteria**:

  **If TDD:**
  - [ ] Test: 合并两个有效 store 成功
  - [ ] Test: 合并结果可正确加载
  - [ ] Test: 至少需要 2 个 store（clap 校验）
  - [ ] Test: 不存在的 store 文件返回清晰错误
  - [ ] `cargo test` PASS

  **QA Scenarios:**

  ```
  Scenario: 两个 store 合并成功
    Tool: Bash (cargo run)
    Preconditions: 准备两个有效的 .bincode GraphStore 文件
    Steps:
      1. cargo run -- merge --stores store-a.bincode store-b.bincode --output /tmp/merged.bincode
    Expected Result: 成功合并，输出摘要
    Evidence: .sisyphus/evidence/task-10-merge.txt

  Scenario: 单个 store 报错
    Tool: Bash (cargo run)
    Steps:
      1. cargo run -- merge --stores only-one.bincode --output /tmp/merged.bincode
    Expected Result: 退出码非 0，提示至少需要 2 个 store
    Evidence: .sisyphus/evidence/task-10-merge-single.txt
  ```

  **Commit**: YES (groups with T9, T11, T12)
  - Message: part of `feat(import): add import/merge CLI commands and integration tests`
  - Files: `src/main.rs`

- [ ] 11. 导出格式支持 Custom 节点

  **What to do**:
  - 更新 `src/export/json.rs` 以支持 Custom 节点/边的导出：
    - `Node::Custom` → JSON node: `{ "type": "custom", "custom_type": "dubbo_service", "label": "...", "key_fields": {...}, "properties": {...}, ... }`
    - `Edge::CustomEdge` → JSON edge: `{ "type": "custom", "custom_type": "dubbo_invokes", "properties": {...}, ... }`
  - 更新 `src/export/dot.rs` 以支持 Custom 节点：
    - Custom 节点形状：`box` with style `filled` + color derived from type_name hash
    - Custom 边：虚线箭头 + type_name 标签
  - 更新 `src/export/mermaid.rs` 以支持 Custom 节点：
    - Custom 节点：`[label]` 格式 + type_name 前缀
    - Custom 边：`-.->` 虚线 + type_name 标签
  - 确保包含 Custom 节点的 graph 可以完整导出为 DOT/JSON/Mermaid
  - **TDD**

  **Must NOT do**:
  - 不要修改现有节点/边的导出格式
  - 不要为 Custom 节点添加复杂的颜色/形状映射逻辑（简单 hash 即可）

  **Recommended Agent Profile**:
  - **Category**: `unspecified-high`
    - Reason: 需要修改 3 个导出模块，每个模块有不同的格式要求
  - **Skills**: [`test-driven-development`]

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 3 (with Tasks 9, 10, 12)
  - **Blocks**: None (但影响 Task 12 的导出验证)
  - **Blocked By**: Task 2 (Custom 变体)

  **References**:

  **Pattern References**:
  - `src/export/json.rs:14-213` — 现有 JSON 导出模式，Custom 节点遵循相同的 node/edge JSON 结构
  - `src/export/dot.rs:3-234` — 现有 DOT 导出模式，理解节点形状和边颜色映射
  - `src/export/mermaid.rs:3-196` — 现有 Mermaid 导出模式

  **WHY Each Reference Matters**:
  - 三个导出模块: Custom 节点在每个导出格式中都需要对应的处理分支

  **Acceptance Criteria**:

  **If TDD:**
  - [ ] Test: JSON 导出包含 Custom 节点且字段完整
  - [ ] Test: DOT 导出包含 Custom 节点且语法正确
  - [ ] Test: Mermaid 导出包含 Custom 节点且语法正确
  - [ ] Test: 混合标准+Custom 节点的导出完整
  - [ ] `cargo test` PASS

  **QA Scenarios:**

  ```
  Scenario: Custom 节点导出正确性
    Tool: Bash (cargo test)
    Steps:
      1. 运行 cargo test test_export_custom
    Expected Result: 所有导出格式的 Custom 节点测试通过
    Evidence: .sisyphus/evidence/task-11-export.txt
  ```

  **Commit**: YES (groups with T9, T10, T12)
  - Message: part of `feat(import): add import/merge CLI commands and integration tests`
  - Files: `src/export/json.rs`, `src/export/dot.rs`, `src/export/mermaid.rs`

- [ ] 12. 端到端集成测试

  **What to do**:
  - 创建集成测试文件 `tests/import_merge.rs`
  - 测试场景覆盖：

  **场景 A: 标准 CGEF 完整流程**
  1. 准备包含多种标准节点的 CGEF JSON fixture (`tests/fixtures/standard.cgef.json`)
  2. import → 验证 GraphStore 节点数和边数
  3. 将 import 结果导出为 JSON → 验证内容完整
  4. 与 codeweb 扫描生成的 store merge → 验证合并结果
  5. 对合并结果执行 trace → 验证跨 store 的链路可达

  **场景 B: 自定义节点完整流程**
  1. 准备包含 node_schemas + custom 节点的 CGEF JSON fixture (`tests/fixtures/custom.cgef.json`)
  2. import → 验证 Custom 节点正确创建
  3. 验证 Custom 节点通过标准边与标准节点连接
  4. merge → 验证 Custom 节点在合并后保留
  5. trace → 验证可追溯到 Custom 节点

  **场景 C: 跨域追溯**
  1. 准备 CGEF JSON（企业端）+ codeweb scan 结果（公有端）
  2. 企业端: `java_method → custom:dubbo_service → procedure`
  3. 公有端: `procedure → table`
  4. merge 后: `java_method → dubbo_service → procedure → table` 完整链路可达

  **场景 D: 边界和错误场景**
  1. 空 nodes 的 CGEF → 报错或产生空 store
  2. format_version 不匹配 → 明确错误
  3. 重复导入同一文件 → 幂等（两次 import 产出相同 store）
  4. 悬空边引用 → 报错
  5. 大量节点（1000+）性能可接受

  - 每个场景准备对应的 fixture 文件放在 `tests/fixtures/`
  - **此任务不做 TDD**（它是最终验证，不需要为验证写测试）

  **Must NOT do**:
  - 不要在集成测试中 mock 内部模块（使用真实的文件 I/O 和解析）
  - 不要在集成测试中测试单元逻辑（那是 Task 5-8 的职责）

  **Recommended Agent Profile**:
  - **Category**: `deep`
    - Reason: 需要理解全链路并构造端到端测试场景，涉及多个模块的集成
  - **Skills**: []

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 3 (with Tasks 9, 10, 11) — 但最好在 T9/T10 之后执行
  - **Blocks**: Final Verification Wave
  - **Blocked By**: Tasks 9 (import CLI), 10 (merge CLI)

  **References**:

  **Pattern References**:
  - `tests/` 目录下现有集成测试 — 遵循项目的集成测试模式

  **API/Type References**:
  - `src/import/mod.rs` — import 模块公共 API
  - `src/graph/store.rs` — GraphStore load/save/merge
  - `src/graph/traverse.rs` — trace_chain, find_nodes_by_name

  **WHY Each Reference Matters**:
  - 现有测试模式: 保持一致性
  - import API: 集成测试通过公共 API 调用
  - GraphStore: 测试 save/load/merge 的端到端正确性
  - traverse: 验证 merge 后的跨 store trace 可达性

  **Acceptance Criteria**:

  - [ ] 场景 A: 标准 CGEF import → export → merge → trace 全链路通过
  - [ ] 场景 B: 自定义节点 import → merge → trace 全链路通过
  - [ ] 场景 C: 跨域追溯成功（企业 → 公有完整链路）
  - [ ] 场景 D: 所有边界和错误场景行为正确
  - [ ] `cargo test --test import_merge` PASS
  - [ ] Fixture 文件组织合理，可复用

  **QA Scenarios:**

  ```
  Scenario: 端到端集成测试全部通过
    Tool: Bash (cargo test)
    Steps:
      1. cargo test --test import_merge
    Expected Result: 所有 4 个场景的测试通过
    Evidence: .sisyphus/evidence/task-12-e2e.txt

  Scenario: 全量测试无回归
    Tool: Bash (cargo test)
    Steps:
      1. cargo test (运行所有测试，包括原有的)
    Expected Result: 原有测试不受影响，新测试全部通过
    Evidence: .sisyphus/evidence/task-12-full-regression.txt
  ```

  **Commit**: YES
  - Message: part of `feat(import): add import/merge CLI commands and integration tests`
  - Files: `tests/import_merge.rs`, `tests/fixtures/*.cgef.json`

> 4 review agents run in PARALLEL. ALL must APPROVE. Present consolidated results to user and get explicit "okay" before completing.

- [ ] F1. **Plan Compliance Audit** — `oracle`
  Read the plan end-to-end. For each "Must Have": verify implementation exists (read file, run command). For each "Must NOT Have": search codebase for forbidden patterns — reject with file:line if found. Check evidence files exist in .sisyphus/evidence/. Compare deliverables against plan.
  Output: `Must Have [N/N] | Must NOT Have [N/N] | Tasks [N/N] | VERDICT: APPROVE/REJECT`

- [ ] F2. **Code Quality Review** — `unspecified-high`
  Run `cargo clippy -- -D warnings` + `cargo fmt -- --check` + `cargo test`. Review all changed files for: `unwrap()` in non-test code, empty error handlers, `todo!()` / `unimplemented!()`, unused imports, over-abstraction. Check AI slop: excessive comments, generic names.
  Output: `Build [PASS/FAIL] | Clippy [PASS/FAIL] | Tests [N pass/N fail] | Files [N clean/N issues] | VERDICT`

- [ ] F3. **Real Manual QA** — `unspecified-high`
  Start from clean state. Execute EVERY QA scenario from EVERY task — follow exact steps, capture evidence. Test cross-task integration: import standard graph + import custom graph + merge both + trace across boundaries. Test edge cases: empty graph, duplicate nodes, invalid format version. Save to `.sisyphus/evidence/final-qa/`.
  Output: `Scenarios [N/N pass] | Integration [N/N] | Edge Cases [N tested] | VERDICT`

- [ ] F4. **Scope Fidelity Check** — `deep`
  For each task: read "What to do", read actual diff (git log/diff). Verify 1:1 — everything in spec was built (no missing), nothing beyond spec was built (no creep). Check "Must NOT do" compliance. Detect cross-task contamination. Flag unaccounted changes.
  Output: `Tasks [N/N compliant] | Contamination [CLEAN/N issues] | Unaccounted [CLEAN/N files] | VERDICT`

---

## Commit Strategy

| # | Message | Key Files | Pre-commit |
|---|---------|-----------|------------|
| T1-T4 | `feat(import): define CGEF format and extend node types` | docs/cgef-schema.json, src/graph/mod.rs, src/import/ | `cargo test` |
| T5-T8 | `feat(import): implement CGEF parser, schema, validator, and merge extension` | src/import/*.rs, src/graph/store.rs | `cargo test` |
| T9-T12 | `feat(import): add import/merge CLI commands and integration tests` | src/main.rs, tests/ | `cargo test && cargo clippy -- -D warnings` |

---

## Success Criteria

### Verification Commands
```bash
cargo test                                          # Expected: all tests pass
cargo clippy -- -D warnings                         # Expected: no warnings
cargo fmt -- --check                                # Expected: no diff
cargo run -- import --file test.cgef.json --output imported.bincode   # Expected: success
cargo run -- merge --stores native.bincode imported.bincode --output merged.bincode  # Expected: success
```

### Final Checklist
- [ ] All "Must Have" present
- [ ] All "Must NOT Have" absent
- [ ] All tests pass
- [ ] CGEF JSON Schema 文件存在且可用于外部验证
- [ ] 自定义节点类型在 import → merge → trace 全链路中工作正常
- [ ] 路径映射 (--prefix) 正确工作
- [ ] format_version 版本检查正确拦截不兼容版本
