# #84 + #85 回归修复实施计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 修复两个已通过回归测试捕获的缺陷：Spring DI 边采集（#84）和 impact 批量模式（#85）。

**Architecture:**
- #84：在 `java_method.rs` 中新增 DI 注入信息提取（字段类型 + 构造函数参数类型），在 `builder.rs` 中消费这些信息并创建 `CallsJava` (JavaClass→JavaClass) 边。DI 边自动被 `EdgeCategory::Call` 覆盖，无需额外的遍历/过滤改动。
- #85：将 clap `--file` 参数从 `Option<PathBuf>` 改为 `Vec<PathBuf>`（`ArgAction::Append`），重构 `cmd_impact` 以循环处理多文件并输出 JSON 数组。

**Tech Stack:** Rust, tree-sitter-java, petgraph, clap, serde_json

**两个 Track 完全独立，可并行实施。**

---

## Track A: #84 — Spring DI 边采集

### Task A1: 在 JavaParseResult 中添加 DI 注入数据结构

**Files:**
- Modify: `src/parser/java_method.rs:18-58`

**Step 1: 添加数据结构**

在 `JavaParseResult` struct 的 `content_hash` 字段之前插入：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiInjection {
    /// 注入的类型名（简单名，如 "FilmService"），由 builder 通过 imports 解析为 FQN
    pub type_name: String,
    /// DI 来源：字段注入或构造函数参数注入
    pub source: DiSource,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiSource {
    ConstructorParam,
    Field,
}
```

**Step 2: 添加到 JavaParseResult**

```rust
pub struct JavaParseResult {
    pub file: PathBuf,
    pub package: String,
    pub imports: Vec<String>,
    pub classes: Vec<JavaClassInfo>,
    pub methods: Vec<JavaMethodInfo>,
    pub di_injections: Vec<DiInjection>,   // ← 新增
    pub content_hash: String,
}
```

**Step 3: 添加到 JavaTreeWalker struct**

```rust
struct JavaTreeWalker<'a> {
    source: &'a [u8],
    file: &'a Path,
    package: String,
    imports: Vec<String>,
    classes: Vec<JavaClassInfo>,
    methods: Vec<JavaMethodInfo>,
    di_injections: Vec<DiInjection>,       // ← 新增
    current_class_stack: Vec<String>,
}
```

**Step 4: 初始化 di_injections**

在 `JavaTreeWalker::new()` 中添加：
```rust
di_injections: Vec::new(),
```

**Step 5: 在 parse_java_source 中传递**

```rust
Ok(JavaParseResult {
    file: path.to_path_buf(),
    package: walker.package,
    imports: walker.imports,
    classes: walker.classes,
    methods: walker.methods,
    di_injections: walker.di_injections,  // ← 新增
    content_hash,
})
```

**Step 6: 运行编译**

Run: `cargo build 2>&1`
Expected: PASS (编译通过，di_injections 尚未被消费)

**Step 7: Commit**

```bash
git add src/parser/java_method.rs
git commit -m "feat(java): add DiInjection data structures to JavaParseResult"
```

---

### Task A2: 在 JavaTreeWalker 中提取 DI 注入信息

**Files:**
- Modify: `src/parser/java_method.rs:276-339`

**Step 1: 添加 DI 检测辅助方法**

在 `impl<'a> JavaTreeWalker<'a>` 块中添加（在 `walk_type_body` 之前）：

```rust
fn is_di_annotation(&self, modifier_node: tree_sitter::Node) -> bool {
    if modifier_node.kind() != "annotation" {
        return false;
    }
    let mut cursor = modifier_node.walk();
    for child in modifier_node.children(&mut cursor) {
        if child.kind() == "identifier" {
            if let Ok(text) = child.utf8_text(self.source) {
                match text {
                    "Autowired" | "Inject" | "Resource" => return true,
                    _ => {}
                }
            }
        }
    }
    false
}

fn extract_type_name(&self, node: tree_sitter::Node) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_identifier" | "scoped_type_identifier" | "generic_type" => {
                return child.utf8_text(self.source).ok().map(|s| s.to_string());
            }
            _ => {}
        }
    }
    None
}
```

**Step 2: 在 walk_type_body 中处理 field_declaration 和 constructor_declaration**

修改 `walk_type_body` 方法，在 `match child.kind()` 中添加：

```rust
"field_declaration" => {
    self.handle_field_declaration(child);
}
"constructor_declaration" => {
    self.handle_constructor_declaration(child);
}
```

**Step 3: 实现 handle_field_declaration**

```rust
fn handle_field_declaration(&mut self, node: tree_sitter::Node) {
    let line = node.start_position().row + 1;

    // 检查是否有 DI 注解
    let has_di_annotation = {
        let mut cursor = node.walk();
        let mut found = false;
        for child in node.children(&mut cursor) {
            if child.kind() == "modifiers" {
                let mut mc = child.walk();
                for mod_child in child.children(&mut mc) {
                    if self.is_di_annotation(mod_child) {
                        found = true;
                        break;
                    }
                }
            }
        }
        found
    };

    if !has_di_annotation {
        return;
    }

    if let Some(type_name) = self.extract_type_name(node) {
        self.di_injections.push(DiInjection {
            type_name,
            source: DiSource::Field,
            line,
        });
    }
}
```

**Step 4: 实现 handle_constructor_declaration**

```rust
fn handle_constructor_declaration(&mut self, node: tree_sitter::Node) {
    let line = node.start_position().row + 1;

    // 遍历 formal_parameters → formal_parameter → type
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "formal_parameters" {
            let mut pc = child.walk();
            for param in child.children(&mut pc) {
                if param.kind() == "formal_parameter" {
                    if let Some(type_name) = self.extract_type_name(param) {
                        self.di_injections.push(DiInjection {
                            type_name,
                            source: DiSource::ConstructorParam,
                            line,
                        });
                    }
                }
            }
        }
    }
}
```

**Step 5: 添加树遍历**

在 `walk_type_body` 的 `_ => { self.walk_type_body(child); }` 之前，递归进入 `field_declaration` 和 `constructor_declaration` 的子节点以发现嵌套类型声明：

```rust
"field_declaration" => {
    self.handle_field_declaration(child);
    self.walk_type_body(child);  // 递归查找匿名类等
}
"constructor_declaration" => {
    self.handle_constructor_declaration(child);
    self.walk_type_body(child);  // 递归
}
```

实际代码中 `walk_type_body` 的 `_ =>` 分支已经处理了递归，但需要确保 `field_declaration`/`constructor_declaration` 不会被 `_` 分支忽略。当前代码中 `_` 分支会递归进入任何未匹配的节点，所以所有子节点都会被遍历。

**Step 6: 运行编译**

Run: `cargo build 2>&1`
Expected: PASS

**Step 7: Commit**

```bash
git add src/parser/java_method.rs
git commit -m "feat(java): extract DI injection info from fields and constructor params"
```

---

### Task A3: 在 builder 中消费 DI 注入信息创建 CallGraph 边

**Files:**
- Modify: `src/graph/builder.rs:2827-3061`

**Step 1: 在 add_java_method_nodes_from_parsed 末尾添加 DI 边创建逻辑**

在函数末尾（`}` 之前，`method_index` 构建完成后）添加：

```rust
// ── DI injection edges ──────────────────────────────────────────
for result in java_results {
    if result.di_injections.is_empty() {
        continue;
    }
    let file_imports = import_map.get(&result.file);

    // 找到当前文件中的类（取当前文件的第一个类作为 DI 所有者）
    let owning_class = result.classes.first();
    let Some(owning_class) = owning_class else { continue };
    let Some(&owning_class_idx) = class_index.get(&owning_class.fqn) else { continue };

    for injection in &result.di_injections {
        // 解析注入类型名 → FQN
        let target_fqn = resolve_fqn(
            &injection.type_name,
            &simple_name_to_fqn,
            file_imports,
        );

        let Some(target_fqn) = target_fqn else { continue };
        let Some(&target_class_idx) = class_index.get(&target_fqn) else { continue };

        let location = SourceLocation {
            file: Arc::new(result.file.clone()),
            line: injection.line,
        };
        graph.add_edge(
            owning_class_idx,
            target_class_idx,
            Edge::CallsJava { location },
        );
    }
}
```

**Step 2: 运行编译**

Run: `cargo build 2>&1`
Expected: PASS

**Step 3: 使用回归测试验证**

Run: `cargo test --test regress_issue_84_spring_di_edges -- --ignored 2>&1`
Expected:
- `regress_di_constructor_injection_creates_edge`: PASS
- `regress_di_field_injection_creates_edge`: PASS
- `regress_same_class_method_call_creates_edge`: PASS

**Step 4: 移除测试的 #[ignore]**

Run AST-grep or edit to remove the `#[ignore = "..."]` attributes from the two DI regression tests:
- `regress_di_constructor_injection_creates_edge`
- `regress_di_field_injection_creates_edge`

**Step 5: 运行全部测试**

Run: `cargo test 2>&1`
Expected: 0 failures, the two previously-ignored DI tests now pass

**Step 6: Commit**

```bash
git add src/graph/builder.rs tests/regress_issue_84_spring_di_edges.rs
git commit -m "fix(graph): create CallsJava edges from Spring DI field/constructor injections (#84)"
```

---

## Track B: #85 — Impact 批量模式

### Task B1: 修改 clap 参数定义和分发逻辑

**Files:**
- Modify: `src/main.rs:537-559` (Impact 命令定义)
- Modify: `src/main.rs:750-766` (Impact 命令分发)

**Step 1: 修改 Impact struct 的 file 字段**

从：
```rust
file: Option<PathBuf>,
```

改为：
```rust
#[arg(long, action = clap::ArgAction::Append)]
file: Vec<PathBuf>,
```

**Step 2: 修改分发逻辑**

从：
```rust
match (&file, &node) {
    (Some(_), Some(_)) => { ... }
    (None, None) => { ... }
    _ => cmd_impact(file.as_deref(), node.as_deref(), ...),
}
```

改为：
```rust
match (file.is_empty(), node.is_some()) {
    (false, true) => {
        eprintln!("Error: --file and --node are mutually exclusive. Pass exactly one.");
        std::process::exit(2);
    }
    (true, false) => {
        eprintln!("Error: must pass exactly one of --file <path> or --node <name>.");
        std::process::exit(2);
    }
    _ => cmd_impact(&file, node.as_deref(), &project, &format, depth),
}
```

**Step 3: 运行编译**

Run: `cargo build 2>&1`
Expected: type mismatch error — `cmd_impact` 的签名需要更新

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "refactor(cli): change impact --file from Option<PathBuf> to Vec<PathBuf>"
```

---

### Task B2: 重构 cmd_impact 支持多文件和批量输出

**Files:**
- Modify: `src/main.rs:2421-2492`

**Step 1: 修改函数签名**

从：
```rust
fn cmd_impact(
    file: Option<&Path>,
    node: Option<&str>,
    ...
```

改为：
```rust
fn cmd_impact(
    files: &[PathBuf],
    node: Option<&str>,
    ...
```

**Step 2: 重构函数体，将单文件逻辑提取为返回 ImpactResult 的闭包**

将第 2431-2491 行之间的逻辑重构为：

```rust
fn cmd_impact(
    files: &[PathBuf],
    node: Option<&str>,
    project: &Path,
    format: &str,
    depth: usize,
) -> Result<()> {
    use crate::graph::query::filter::EdgeFilter;
    use petgraph::Direction;

    let mut proj = project::Project::find(project)?;
    let store = match proj.load_store() {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Project not analyzed. Run `codeweb analyze` first.");
            return Ok(());
        }
    };

    let graph = store.graph();
    let calls_filter = EdgeFilter::calls_only();
    let file_nodes = store.file_nodes();
    let key_index = store.node_key_index();

    let compute_impact = |file_path: &Path| -> Option<ImpactResult> {
        let (start_nodes, target) =
            resolve_file_target(graph, file_nodes, key_index, file_path).ok()?;

        if start_nodes.is_empty() {
            return Some(build_impact_result(&target, vec![], vec![]));
        }

        let mut upstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();
        let mut downstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();

        collect_impact_entries(
            graph, &start_nodes, Direction::Incoming,
            depth, &calls_filter, &mut upstream_map,
        );
        collect_impact_entries(
            graph, &start_nodes, Direction::Outgoing,
            depth, &calls_filter, &mut downstream_map,
        );

        let mut upstream: Vec<ImpactEntry> = upstream_map.into_values().collect();
        let mut downstream: Vec<ImpactEntry> = downstream_map.into_values().collect();
        upstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));
        downstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));

        Some(build_impact_result(&target, upstream, downstream))
    };

    match (files.is_empty(), node) {
        (false, None) => {
            // Batch file mode
            let mut results: Vec<ImpactResult> = Vec::with_capacity(files.len());
            for file in files {
                match compute_impact(file) {
                    Some(result) => results.push(result),
                    None => eprintln!("Warning: file '{}' not found in graph", file.display()),
                }
            }
            emit_batch_result(&results, format)?;
        }
        (true, Some(name)) => {
            // Single node mode (unchanged)
            let (start_nodes, target) = resolve_node_target(&store, name)?;
            if start_nodes.is_empty() {
                emit_empty_result(&target, format)?;
                return Ok(());
            }
            // ... existing single-node impact logic (same as before) ...
            let mut upstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();
            let mut downstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();
            collect_impact_entries(graph, &start_nodes, Direction::Incoming, depth, &calls_filter, &mut upstream_map);
            collect_impact_entries(graph, &start_nodes, Direction::Outgoing, depth, &calls_filter, &mut downstream_map);
            let mut upstream: Vec<ImpactEntry> = upstream_map.into_values().collect();
            let mut downstream: Vec<ImpactEntry> = downstream_map.into_values().collect();
            upstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));
            downstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));
            let result = build_impact_result(&target, upstream, downstream);
            emit_result(&result, format)?;
        }
        _ => unreachable!("clap layer guarantees exactly one of --file/--node"),
    }

    Ok(())
}
```

**Step 3: 添加 emit_batch_result 函数**

在 `emit_result` 附近添加：

```rust
fn emit_batch_result(results: &[ImpactResult], format: &str) -> Result<()> {
    if format == "json" {
        let json = serde_json::to_string_pretty(results)
            .map_err(|e| error::CodeWebError::ExportError {
                message: format!("JSON serialization: {}", e),
            })?;
        println_stdout!("{}", json);
    } else {
        for (i, result) in results.iter().enumerate() {
            if i > 0 {
                println_stdout!("\n---\n");
            }
            print_impact_text(result);
        }
    }
    Ok(())
}
```

**Step 4: 运行编译**

Run: `cargo build 2>&1`
Expected: PASS

**Step 5: 使用回归测试验证**

Run: `cargo test --test regress_issue_85_impact_batch -- --ignored 2>&1`
Expected:
- `regress_impact_batch_mode_expected_behavior`: PASS (3-file batch returns JSON array)
- `regress_impact_multiple_file_args_rejected`: FAIL (不再报错 — 预期行为)
- `regress_impact_single_file_workaround`: PASS

**Step 6: 移除/更新测试的 #[ignore]**

- 移除 `regress_impact_batch_mode_expected_behavior` 的 `#[ignore]`
- 移除 `regress_impact_multiple_file_args_rejected` 测试（不再需要，它验证旧的错误行为）

**Step 7: 运行全部测试**

Run: `cargo test 2>&1`
Expected: 0 failures

**Step 8: Commit**

```bash
git add src/main.rs tests/regress_issue_85_impact_batch.rs
git commit -m "feat(impact): support multiple --file args for batch impact queries (#85)"
```

---

## 验证矩阵

修复完成后运行：

```sh
cargo build --features full         # 跨 feature 编译
cargo test                           # 全量测试（0 failures）
cargo test --test regress_issue_84_spring_di_edges  # 3/3 pass
cargo test --test regress_issue_85_impact_batch     # 2/2 pass (移除旧测试后)
cargo clippy --features full -- -D warnings         # lint clean
cargo fmt -- --check                                # format clean
```

---

## 关键设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| DI 边类型 | 复用 `CallsJava` (JavaClass→JavaClass) | 无需修改遍历/过滤/导出代码；`EdgeCategory::Call` 自动覆盖 |
| 构造函数参数 DI | 始终提取（无需注解） | Spring 单构造函数自动装配原则 |
| 字段 DI | 仅提取带 `@Autowired`/`@Inject`/`@Resource` 注解的字段 | 避免对所有字段类型创建误报边 |
| 批量输出格式 | JSON 数组 `[...]`，文本用 `---` 分隔 | JSON 便于程序消费，文本向后兼容 |
| 批量加载优化 | 单次 store 加载，多文件循环 | 消除 B 问题中 92 次 store 反序列化开销 |
