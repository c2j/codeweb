# Phase 3: Java Method Extraction + Java↔Mapper Bridge Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extract Java method declarations, method invocations, and class hierarchies from `.java` files using tree-sitter-java, then bridge Java methods to iBatis/MyBatis XML mappers, producing a complete `JavaMethod → MappedStatement → SQL → Procedure` call graph.

**Architecture:** Add a `java` module under `src/parser/` that uses `tree-sitter` + `tree-sitter-java` to parse `.java` files and extract structural information (classes, methods, calls, imports). Extend the `Node`/`Edge` enums with `JavaMethod` and `JavaClass` variants. Add bridge logic in `GraphBuilder` that matches Java methods to MappedStatements via namespace==FQN + method==id, and detects `sqlSession.select*("namespace.id")` call patterns.

**Tech Stack:** Rust, tree-sitter 0.24, tree-sitter-java 0.23 (matching ogsql-parser), petgraph, existing codeweb modules.

---

## Task 1: Add tree-sitter Dependencies

**Files:**
- Modify: `Cargo.toml`

**Step 1: Add tree-sitter and tree-sitter-java to Cargo.toml**

In `Cargo.toml`, add to `[dependencies]`:

```toml
tree-sitter = "0.24"
tree-sitter-java = "0.23"
```

These versions match what ogsql-parser already depends on (resolved to 0.24.7 and 0.23.5 in Cargo.lock). Cargo will unify them — no version conflict.

**Step 2: Run `cargo build` to verify dependency resolution**

Run: `cargo build`
Expected: Compiles successfully, no version conflicts.

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add tree-sitter dependencies for Java method extraction"
```

---

## Task 2: Create Java Method Extractor Module

**Files:**
- Create: `src/parser/java_method.rs`
- Modify: `src/parser/mod.rs`

This is the core module — uses tree-sitter to parse Java source and extract methods, calls, imports, and class info.

### Data Structures

```rust
// src/parser/java_method.rs

use std::path::PathBuf;

/// A Java class or interface declaration found in source.
#[derive(Debug, Clone)]
pub struct JavaClassInfo {
    /// Fully qualified name (e.g., "com.example.service.UserService")
    /// Built from package + class name.
    pub fqn: String,
    /// Simple class name (e.g., "UserService")
    pub name: String,
    /// Package declaration (e.g., "com.example.service")
    pub package: Option<String>,
    /// Superclass FQN (from extends clause), if any
    pub extends: Option<String>,
    /// Implemented interface FQNs, if any
    pub implements: Vec<String>,
    /// Source file path
    pub file: PathBuf,
    /// Line number of class declaration
    pub line: usize,
}

/// A Java method declaration.
#[derive(Debug, Clone)]
pub struct JavaMethodInfo {
    /// Simple method name
    pub name: String,
    /// Class FQN this method belongs to
    pub class_fqn: String,
    /// Method signature text (e.g., "getUser(Long)")
    pub signature: String,
    /// Source file path
    pub file: PathBuf,
    /// Line number of method declaration
    pub line: usize,
    /// Method calls found within this method's body
    pub calls: Vec<MethodCallInfo>,
}

/// A method invocation found inside a Java method body.
#[derive(Debug, Clone)]
pub struct MethodCallInfo {
    /// Object/qualifier part (e.g., "userDao" in "userDao.findById")
    /// None for unqualified calls like "helperMethod()"
    pub object: Option<String>,
    /// Method name being called (e.g., "findById")
    pub method: String,
    /// String literal arguments, if any (for detecting "namespace.id" patterns)
    pub string_args: Vec<String>,
    /// Line number of the call
    pub line: usize,
}

/// Result of parsing a single .java file.
#[derive(Debug, Clone)]
pub struct JavaParseResult {
    pub file: PathBuf,
    pub package: Option<String>,
    pub imports: Vec<String>,
    pub classes: Vec<JavaClassInfo>,
    pub methods: Vec<JavaMethodInfo>,
}
```

### Extraction Logic

**Design decision:** We use **manual tree walking** (same approach as ogsql-parser's `java/extract.rs`) rather than `Query`/`QueryCursor`. This gives us full context tracking — knowing which class a method belongs to, which method a call is inside — which is essential for call graph construction. The `Query` API returns flat matches without ancestry context.

```rust
use tree_sitter::{Parser, Node, Tree};
use tree_sitter_java::LANGUAGE;

pub fn parse_java_file(path: &Path) -> Result<JavaParseResult, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("read error: {}", e))?;

    let mut parser = Parser::new();
    parser.set_language(&LANGUAGE.into())
        .map_err(|e| format!("language error: {}", e))?;

    let tree = parser.parse(&source, None)
        .ok_or("parse failed")?;

    let mut extractor = JavaTreeWalker {
        source: source.as_bytes(),
        file: path.to_path_buf(),
        package: None,
        imports: Vec::new(),
        classes: Vec::new(),
        current_class_stack: Vec::new(), // tracks nested class context
        methods: Vec::new(),
        current_method_fqn: None,
    };
    extractor.walk(tree.root_node());

    Ok(JavaParseResult {
        file: path.to_path_buf(),
        package: extractor.package,
        imports: extractor.imports,
        classes: extractor.classes,
        methods: extractor.methods,
    })
}
```

### Helper: Tree Walker (Manual Walking)

This is the central extraction engine — a single-pass recursive walk over the CST that maintains context (current class, current method).

```rust
struct JavaTreeWalker<'a> {
    source: &'a [u8],
    file: PathBuf,
    package: Option<String>,
    imports: Vec<String>,
    classes: Vec<JavaClassInfo>,
    current_class_stack: Vec<String>, // FQN stack for nested classes
    methods: Vec<JavaMethodInfo>,
    current_method_fqn: Option<String>,
}

impl<'a> JavaTreeWalker<'a> {
    fn walk(&mut self, node: Node) {
        match node.kind() {
            "package_declaration" => {
                self.package = node.child_by_field_name("name")
                    .and_then(|n| n.utf8_text(self.source).ok())
                    .map(|s| s.to_string());
                // Note: package_declaration has a scoped_identifier child, not a named "name" field.
                // Fall back to scanning children for scoped_identifier.
                if self.package.is_none() {
                    let mut cursor = node.walk();
                    for child in node.children(&mut cursor) {
                        if child.kind() == "scoped_identifier" {
                            self.package = child.utf8_text(self.source)
                                .ok().map(|s| s.to_string());
                        }
                    }
                }
            }
            "import_declaration" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "scoped_identifier" {
                        if let Ok(text) = child.utf8_text(self.source) {
                            self.imports.push(text.to_string());
                        }
                    }
                }
            }
            "class_declaration" | "interface_declaration" => {
                let name = node.child_by_field_name("name")
                    .and_then(|n| n.utf8_text(self.source).ok())
                    .unwrap_or("")
                    .to_string();

                let fqn = match &self.package {
                    Some(pkg) => format!("{}.{}", pkg, name),
                    None => name.clone(),
                };
                let line = node.start_position().row + 1;

                // Extract extends/implements
                let (extends, implements) = self.extract_hierarchy(&node);

                self.classes.push(JavaClassInfo {
                    fqn: fqn.clone(),
                    name,
                    package: self.package.clone(),
                    extends,
                    implements,
                    file: self.file.clone(),
                    line,
                });

                // Push class context, recurse into body, then pop
                self.current_class_stack.push(fqn);
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.walk(child);
                }
                self.current_class_stack.pop();
                return; // already recursed
            }
            "method_declaration" => {
                let method_name = node.child_by_field_name("name")
                    .and_then(|n| n.utf8_text(self.source).ok())
                    .unwrap_or("")
                    .to_string();

                let class_fqn = self.current_class_stack.last()
                    .cloned()
                    .unwrap_or_default();
                let method_fqn = format!("{}.{}", class_fqn, method_name);
                let line = node.start_position().row + 1;
                let signature = self.build_signature(&method_name, &node);

                // Extract method calls within this method's body
                let calls = self.extract_method_calls(&node);

                self.methods.push(JavaMethodInfo {
                    name: method_name,
                    class_fqn: class_fqn.clone(),
                    signature,
                    file: self.file.clone(),
                    line,
                    calls,
                });
            }
            _ => {
                // Default: recurse into children
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    self.walk(child);
                }
                return;
            }
        }
        // Recurse into children for non-returning branches
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child);
        }
    }

    fn extract_hierarchy(&self, class_node: &Node) -> (Option<String>, Vec<String>) {
        let mut extends = None;
        let mut implements = Vec::new();

        let mut cursor = class_node.walk();
        for child in class_node.children(&mut cursor) {
            match child.kind() {
                "superclass" => {
                    // extends clause — contains type_identifier or scoped_identifier
                    let mut inner = child.walk();
                    for c in child.children(&mut inner) {
                        if c.kind() == "type_identifier" || c.kind() == "scoped_identifier" {
                            extends = c.utf8_text(self.source).ok().map(|s| s.to_string());
                        }
                    }
                }
                "super_interfaces" => {
                    // implements clause — contains type_list → type_identifiers
                    let mut inner = child.walk();
                    for c in child.children(&mut inner) {
                        if c.kind() == "type_list" {
                            let mut type_cursor = c.walk();
                            for tc in c.children(&mut type_cursor) {
                                if tc.kind() == "type_identifier" || tc.kind() == "scoped_identifier" {
                                    if let Ok(text) = tc.utf8_text(self.source) {
                                        implements.push(text.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        (extends, implements)
    }

    fn extract_method_calls(&self, method_node: &Node) -> Vec<MethodCallInfo> {
        let mut calls = Vec::new();
        let mut seen = std::collections::HashSet::new();
        self.collect_calls_recursive(*method_node, &mut calls, &mut seen);
        calls
    }

    fn collect_calls_recursive(
        &self,
        node: Node,
        calls: &mut Vec<MethodCallInfo>,
        seen: &mut std::collections::HashSet<(usize, usize)>,
    ) {
        if node.kind() == "method_invocation" {
            let method_name = node.child_by_field_name("name")
                .and_then(|n| n.utf8_text(self.source).ok())
                .unwrap_or("")
                .to_string();

            let key = (node.start_position().row, node.start_position().column);
            if seen.insert(key) {
                let line = node.start_position().row + 1;
                let object = node.child_by_field_name("object")
                    .and_then(|n| n.utf8_text(self.source).ok())
                    .map(|s| s.to_string());
                let string_args = self.extract_string_args(&node);

                calls.push(MethodCallInfo {
                    object,
                    method: method_name,
                    string_args,
                    line,
                });
            }
        }

        // Recurse into children
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_calls_recursive(child, calls, seen);
        }
    }

    fn extract_string_args(&self, call_node: &Node) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(arg_list) = call_node.child_by_field_name("arguments") {
            let mut cursor = arg_list.walk();
            for arg in arg_list.children(&mut cursor) {
                if arg.kind() == "string_literal" {
                    if let Ok(text) = arg.utf8_text(self.source) {
                        // Strip surrounding quotes
                        let stripped = text.trim_matches('"')
                            .replace("\\\"", "\"");
                        args.push(stripped);
                    }
                }
            }
        }
        args
    }

    fn build_signature(&self, name: &str, method_node: &Node) -> String {
        let mut param_types = Vec::new();
        if let Some(params) = method_node.child_by_field_name("parameters") {
            let mut cursor = params.walk();
            for param in params.children(&mut cursor) {
                if param.kind() == "formal_parameter" {
                    // The "type" field of formal_parameter gives us the type
                    if let Some(type_node) = param.child_by_field_name("type") {
                        if let Ok(t) = type_node.utf8_text(self.source) {
                            param_types.push(t.to_string());
                        }
                    }
                }
            }
        }
        format!("{}({})", name, param_types.join(", "))
    }
}
```

**Directory scanning helper:**

```rust
/// Scan a directory for .java files and parse them all.
pub fn parse_java_directory(input: &Path) -> Vec<JavaParseResult> {
    let java_files = collect_java_files(input);
    let mut results = Vec::new();

    for path in java_files {
        match parse_java_file(&path) {
            Ok(result) => results.push(result),
            Err(e) => {
                eprintln!("warning: java method parse {}: {}", path.display(), e);
            }
        }
    }

    results
}

fn collect_java_files(input: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if input.is_file() {
        if input.extension().is_some_and(|ext| ext == "java") {
            files.push(input.to_path_buf());
        }
    } else {
        for entry in walkdir::WalkDir::new(input).into_iter().filter_map(|e| e.ok()) {
            let path = entry.into_path();
            if path.extension().is_some_and(|ext| ext == "java") {
                files.push(path);
            }
        }
    }
    files
}
```

**Step 1: Create `src/parser/java_method.rs` with the full module code above**

**Step 2: Add the module to `src/parser/mod.rs`**

Add at the top of `src/parser/mod.rs`:
```rust
pub mod java_method;
```

And add the public re-export:
```rust
pub use java_method::{
    JavaClassInfo, JavaMethodInfo, JavaParseResult, MethodCallInfo,
    parse_java_directory, parse_java_file,
};
```

**Step 3: Run `cargo build` and fix any compilation errors**

Run: `cargo build`
Expected: Compiles. May need minor adjustments to tree-sitter API calls depending on exact 0.24 API.

**Step 4: Write a unit test for basic method extraction**

Add at the bottom of `src/parser/java_method.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_extract_package() {
        let result = parse_java_file(&PathBuf::from("Test.java")).unwrap();
        // Use the real parse function with a temp file
        // Alternative: test via parse_java_directory with a temp dir
        // For unit tests, we can test the walker directly:
        let source = "package com.example.service;\npublic class Foo {}\n";
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&LANGUAGE.into()).unwrap();
        let tree = parser.parse(source, None).unwrap();

        let mut walker = JavaTreeWalker {
            source: source.as_bytes(),
            file: PathBuf::from("Test.java"),
            package: None,
            imports: Vec::new(),
            classes: Vec::new(),
            current_class_stack: Vec::new(),
            methods: Vec::new(),
            current_method_fqn: None,
        };
        walker.walk(tree.root_node());

        assert_eq!(walker.package.as_deref(), Some("com.example.service"));
    }

    #[test]
    fn test_extract_class_with_extends() {
        let source = "package com.example;\npublic class UserService extends BaseService {}\n";
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&LANGUAGE.into()).unwrap();
        let tree = parser.parse(source, None).unwrap();

        let mut walker = JavaTreeWalker {
            source: source.as_bytes(),
            file: PathBuf::from("Test.java"),
            package: None,
            imports: Vec::new(),
            classes: Vec::new(),
            current_class_stack: Vec::new(),
            methods: Vec::new(),
            current_method_fqn: None,
        };
        walker.walk(tree.root_node());

        assert_eq!(walker.classes.len(), 1);
        assert_eq!(walker.classes[0].name, "UserService");
        assert_eq!(walker.classes[0].fqn, "com.example.UserService");
        assert_eq!(walker.classes[0].extends.as_deref(), Some("BaseService"));
    }

    #[test]
    fn test_extract_method_with_call() {
        let source = r#"package com.example;
public class UserService {
    public User getUser(Long id) {
        return userDao.findById(id);
    }
}"#;
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&LANGUAGE.into()).unwrap();
        let tree = parser.parse(source, None).unwrap();

        let mut walker = JavaTreeWalker {
            source: source.as_bytes(),
            file: PathBuf::from("Test.java"),
            package: None,
            imports: Vec::new(),
            classes: Vec::new(),
            current_class_stack: Vec::new(),
            methods: Vec::new(),
            current_method_fqn: None,
        };
        walker.walk(tree.root_node());

        assert_eq!(walker.methods.len(), 1);
        assert_eq!(walker.methods[0].name, "getUser");
        assert_eq!(walker.methods[0].calls.len(), 1);
        assert_eq!(walker.methods[0].calls[0].object.as_deref(), Some("userDao"));
        assert_eq!(walker.methods[0].calls[0].method, "findById");
    }

    #[test]
    fn test_extract_sqlsession_call() {
        let source = r#"package com.example;
public class UserRepo {
    public List<User> getUsers() {
        return sqlSession.selectList("com.example.dao.UserDao.findAll");
    }
}"#;
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&LANGUAGE.into()).unwrap();
        let tree = parser.parse(source, None).unwrap();

        let mut walker = JavaTreeWalker {
            source: source.as_bytes(),
            file: PathBuf::from("Test.java"),
            package: None,
            imports: Vec::new(),
            classes: Vec::new(),
            current_class_stack: Vec::new(),
            methods: Vec::new(),
            current_method_fqn: None,
        };
        walker.walk(tree.root_node());

        assert_eq!(walker.methods.len(), 1);
        assert_eq!(walker.methods[0].calls.len(), 1);
        assert_eq!(walker.methods[0].calls[0].method, "selectList");
        assert_eq!(
            walker.methods[0].calls[0].string_args[0],
            "com.example.dao.UserDao.findAll"
        );
    }

    #[test]
    fn test_extract_imports() {
        let source = "package com.example;\nimport java.util.List;\nimport com.example.dao.UserDao;\npublic class Foo {}\n";
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&LANGUAGE.into()).unwrap();
        let tree = parser.parse(source, None).unwrap();

        let mut walker = JavaTreeWalker {
            source: source.as_bytes(),
            file: PathBuf::from("Test.java"),
            package: None,
            imports: Vec::new(),
            classes: Vec::new(),
            current_class_stack: Vec::new(),
            methods: Vec::new(),
            current_method_fqn: None,
        };
        walker.walk(tree.root_node());

        assert!(walker.imports.contains(&"java.util.List".to_string()));
        assert!(walker.imports.contains(&"com.example.dao.UserDao".to_string()));
    }
}
```

**Step 5: Run tests**

Run: `cargo test --lib parser::java_method`
Expected: All 5 tests pass.

**Step 6: Commit**

```bash
git add src/parser/java_method.rs src/parser/mod.rs
git commit -m "feat: add Java method extraction via tree-sitter-java"
```

---

## Task 3: Extend Graph Model with JavaMethod and JavaClass Nodes

**Files:**
- Modify: `src/graph/mod.rs`
- Modify: `src/export/dot.rs`
- Modify: `src/export/json.rs`
- Modify: `src/export/mermaid.rs`

### Step 1: Add new Node and Edge variants to `src/graph/mod.rs`

Add to the `Node` enum:
```rust
    /// A Java method declaration.
    JavaMethod {
        fqn: String,           // "com.example.service.UserService.getUser"
        class_fqn: String,     // "com.example.service.UserService"
        name: String,          // "getUser"
        signature: String,     // "getUser(Long)"
        file: PathBuf,
        line: usize,
    },
    /// A Java class or interface declaration.
    JavaClass {
        fqn: String,           // "com.example.service.UserService"
        name: String,          // "UserService"
        package: Option<String>,
        file: PathBuf,
        line: usize,
    },
```

Add to the `Edge` enum:
```rust
    /// A Java method calls another Java method.
    CallsJava { location: SourceLocation },
    /// A Java class contains a Java method.
    ContainsMethod,
    /// A Java class extends another class.
    Extends { location: SourceLocation },
    /// A Java class implements an interface.
    Implements { location: SourceLocation },
```

### Step 2: Update DOT exporter

In `src/export/dot.rs`, add match arms for the new node types in the node rendering loop:

```rust
            Node::JavaMethod { name, class_fqn, .. } => {
                (format!("{}.{}", class_fqn, name), "diamond", "")
            }
            Node::JavaClass { fqn, .. } => {
                (fqn.clone(), "folder", "")
            }
```

And for edges:
```rust
            Edge::CallsJava { .. } => ("", "color=orange,"),
            Edge::ContainsMethod => ("", "style=dotted,"),
            Edge::Extends { .. } => ("label=\"extends\"", "style=bold,"),
            Edge::Implements { .. } => ("label=\"implements\"", "style=dashed,"),
```

### Step 3: Update JSON exporter

In `src/export/json.rs`, add variants to `NodeKindJson`:
```rust
    JavaMethod {
        fqn: String,
        class_fqn: String,
        name: String,
        signature: String,
        file: String,
        line: usize,
    },
    JavaClass {
        fqn: String,
        name: String,
        package: Option<String>,
        file: String,
        line: usize,
    },
```

And to `EdgeKindJson`:
```rust
    #[serde(rename = "calls_java")]
    CallsJava { file: String, line: usize },
    #[serde(rename = "contains_method")]
    ContainsMethod,
    #[serde(rename = "extends")]
    Extends { file: String, line: usize },
    #[serde(rename = "implements")]
    Implements { file: String, line: usize },
```

Add the corresponding match arms in the `to_json` function for node/edge conversion.

### Step 4: Update Mermaid exporter

In `src/export/mermaid.rs`, add node rendering:
```rust
            Node::JavaMethod { name, class_fqn, .. } => {
                (format!("{}.{}", class_fqn, name), ("{{\"", "\"}}"))
            }
            Node::JavaClass { fqn, .. } => {
                (fqn.clone(), ("[/", "/]"))
            }
```

And edge arrows:
```rust
            Edge::CallsJava { .. } => "-.->",
            Edge::ContainsMethod => "-.->",
            Edge::Extends { .. } => "==>",
            Edge::Implements { .. } => "-->",
```

### Step 5: Run `cargo build` and `cargo clippy -- -D warnings`

Expected: Clean compilation.

### Step 6: Commit

```bash
git add src/graph/mod.rs src/export/dot.rs src/export/json.rs src/export/mermaid.rs
git commit -m "feat: extend graph model with JavaMethod/JavaClass nodes and edges"
```

---

## Task 4: Integrate Java Method Extraction into GraphBuilder

**Files:**
- Modify: `src/graph/builder.rs`

### Step 1: Add `add_java_method_nodes` method to `GraphBuilder`

This method:
1. Parses all `.java` files via `parse_java_directory()`
2. Creates `JavaClass` nodes and `JavaMethod` nodes
3. Creates `ContainsMethod` edges (class → method)
4. Creates `CallsJava` edges (method → method) where resolvable
5. Creates `Extends`/`Implements` edges (class → class)
6. Bridges Java methods to MappedStatements via two patterns:
   - **Pattern A**: `sqlSession.selectList("namespace.id", ...)` — detect sqlSession calls with string args containing dots
   - **Pattern B**: `userDao.findById(...)` — match object to mapper namespace (using import resolution)

Add to `GraphBuilder`:

```rust
use crate::parser::java_method::{
    JavaClassInfo, JavaMethodInfo, JavaParseResult, MethodCallInfo, parse_java_directory,
};
use std::collections::HashMap;

fn add_java_method_nodes(
    input: &Path,
    graph: &mut CodeGraph,
    proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
    mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
) {
    let java_results = parse_java_directory(input);

    // Build class FQN → NodeIndex mapping
    let mut class_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
    // Build method FQN → NodeIndex mapping
    let mut method_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
    // Build simple class name → FQN mapping for import resolution
    let mut simple_name_to_fqn: HashMap<String, String> = HashMap::new();
    // Build import map: file → (simple_name → FQN)
    let mut import_map: HashMap<std::path::PathBuf, HashMap<String, String>> = HashMap::new();

    // First pass: collect class info and imports
    for result in &java_results {
        let mut file_imports = HashMap::new();
        for import in &result.imports {
            if let Some(simple) = import.rsplit('.').next() {
                file_imports.insert(simple.to_string(), import.clone());
            }
        }
        import_map.insert(result.file.clone(), file_imports);

        for class in &result.classes {
            simple_name_to_fqn.insert(class.name.clone(), class.fqn.clone());
        }
    }

    // Second pass: create JavaClass nodes
    for result in &java_results {
        for class in &result.classes {
            let node = Node::JavaClass {
                fqn: class.fqn.clone(),
                name: class.name.clone(),
                package: class.package.clone(),
                file: class.file.clone(),
                line: class.line,
            };
            let idx = graph.add_node(node);
            class_index.insert(class.fqn.clone(), idx);
        }
    }

    // Create Extends/Implements edges
    for result in &java_results {
        for class in &result.classes {
            let class_idx = class_index[&class.fqn];

            // Resolve extends
            if let Some(extends_name) = &class.extends {
                if let Some(parent_fqn) = resolve_fqn(extends_name, &class.fqn, &simple_name_to_fqn, import_map.get(&result.file)) {
                    if let Some(&parent_idx) = class_index.get(&parent_fqn) {
                        graph.add_edge(class_idx, parent_idx, Edge::Extends {
                            location: SourceLocation {
                                file: class.file.clone(),
                                line: class.line,
                            },
                        });
                    }
                }
            }

            // Resolve implements
            for iface_name in &class.implements {
                if let Some(iface_fqn) = resolve_fqn(iface_name, &class.fqn, &simple_name_to_fqn, import_map.get(&result.file)) {
                    if let Some(&iface_idx) = class_index.get(&iface_fqn) {
                        graph.add_edge(class_idx, iface_idx, Edge::Implements {
                            location: SourceLocation {
                                file: class.file.clone(),
                                line: class.line,
                            },
                        });
                    }
                }
            }
        }
    }

    // Third pass: create JavaMethod nodes and edges
    for result in &java_results {
        for method in &result.methods {
            let method_fqn = format!("{}.{}", method.class_fqn, method.name);
            let node = Node::JavaMethod {
                fqn: method_fqn.clone(),
                class_fqn: method.class_fqn.clone(),
                name: method.name.clone(),
                signature: method.signature.clone(),
                file: method.file.clone(),
                line: method.line,
            };
            let method_idx = graph.add_node(node);
            method_index.insert(method_fqn, method_idx);

            // ContainsMethod edge: class → method
            if let Some(&class_idx) = class_index.get(&method.class_fqn) {
                graph.add_edge(class_idx, method_idx, Edge::ContainsMethod);
            }

            // Process method calls
            for call in &method.calls {
                process_method_call(
                    call,
                    method_idx,
                    &method.class_fqn,
                    graph,
                    proc_index,
                    mapper_index,
                    &method_index,
                    &class_index,
                    &simple_name_to_fqn,
                    import_map.get(&result.file),
                );
            }
        }
    }
}

/// Resolve a simple name to a fully qualified name.
fn resolve_fqn<'a>(
    simple_name: &str,
    _current_class_fqn: &str,
    simple_name_to_fqn: &HashMap<String, String>,
    file_imports: Option<&'a HashMap<String, String>>,
) -> Option<String> {
    // If it's already qualified (contains dots), return as-is
    if simple_name.contains('.') {
        return Some(simple_name.to_string());
    }

    // Try file imports first (most specific)
    if let Some(imports) = file_imports {
        if let Some(fqn) = imports.get(simple_name) {
            return Some(fqn.clone());
        }
    }

    // Try the project's own classes
    if let Some(fqn) = simple_name_to_fqn.get(simple_name) {
        return Some(fqn.clone());
    }

    // Give up — return the simple name as a fallback
    None
}

fn process_method_call(
    call: &MethodCallInfo,
    caller_idx: petgraph::graph::NodeIndex,
    caller_class_fqn: &str,
    graph: &mut CodeGraph,
    proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
    mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
    method_index: &HashMap<String, petgraph::graph::NodeIndex>,
    class_index: &HashMap<String, petgraph::graph::NodeIndex>,
    simple_name_to_fqn: &HashMap<String, String>,
    file_imports: Option<&HashMap<String, String>>,
) {
    // Pattern A: SqlSession direct call
    // sqlSession.selectList("namespace.id", ...)
    // sqlSession.selectOne("namespace.id", ...)
    // sqlSession.insert("namespace.id", ...)
    // sqlSession.update("namespace.id", ...)
    // sqlSession.delete("namespace.id", ...)
    if is_sqlsession_method(&call.method) {
        if let Some(namespace_id) = call.string_args.first() {
            if let Some(&mapper_idx) = mapper_index.get(namespace_id) {
                graph.add_edge(caller_idx, mapper_idx, Edge::InvokesMapper {
                    location: SourceLocation {
                        file: graph[caller_idx].file().to_path_buf(),
                        line: call.line,
                    },
                });
                return;
            }
        }
    }

    // Pattern B: Mapper interface proxy call
    // userDao.findById(id) — match object to mapper namespace
    if let Some(obj) = &call.object {
        // Try resolving the object to a class FQN
        if let Some(obj_fqn) = resolve_fqn(obj, caller_class_fqn, simple_name_to_fqn, file_imports) {
            let mapper_key = format!("{}.{}", obj_fqn, call.method);
            if let Some(&mapper_idx) = mapper_index.get(&mapper_key) {
                graph.add_edge(caller_idx, mapper_idx, Edge::InvokesMapper {
                    location: SourceLocation {
                        file: graph[caller_idx].file().to_path_buf(),
                        line: call.line,
                    },
                });
                return;
            }

            // Also try: object simple name matches namespace suffix
            // (e.g., "userDao" → namespace "com.example.dao.UserDao")
            for (key, &mapper_idx) in mapper_index.iter() {
                if key.ends_with(&format!(".{}", call.method)) {
                    // Check if object name matches namespace class part
                    let ns_class = key.rsplit_once('.').map(|(_, after)| after);
                    let ns_parts = key.rsplit_once('.').map(|(before, _)| before);
                    if let (Some(_method), Some(ns)) = (ns_class, ns_parts) {
                        let ns_simple = ns.rsplit('.').next().unwrap_or(ns);
                        // Case-insensitive comparison + strip "Dao"/"Mapper" suffix
                        if names_match(obj, ns_simple) {
                            graph.add_edge(caller_idx, mapper_idx, Edge::InvokesMapper {
                                location: SourceLocation {
                                    file: graph[caller_idx].file().to_path_buf(),
                                    line: call.line,
                                },
                            });
                            return;
                        }
                    }
                }
            }

            // Not a mapper call — try JavaMethod → JavaMethod
            let callee_fqn = format!("{}.{}", obj_fqn, call.method);
            if let Some(&callee_idx) = method_index.get(&callee_fqn) {
                graph.add_edge(caller_idx, callee_idx, Edge::CallsJava {
                    location: SourceLocation {
                        file: graph[caller_idx].file().to_path_buf(),
                        line: call.line,
                    },
                });
                return;
            }
        }

        // Unqualified call within same class: this.method() or bare method()
        let callee_fqn = format!("{}.{}", caller_class_fqn, call.method);
        if let Some(&callee_idx) = method_index.get(&callee_fqn) {
            graph.add_edge(caller_idx, callee_idx, Edge::CallsJava {
                location: SourceLocation {
                    file: graph[caller_idx].file().to_path_buf(),
                    line: call.line,
                },
            });
        }
    }
}

fn is_sqlsession_method(method: &str) -> bool {
    matches!(
        method,
        "selectList" | "selectOne" | "selectMap" | "select"
        | "insert" | "update" | "delete"
        | "query" | "queryForList" | "queryForObject"
    )
}

/// Heuristic name matching for Java field → Mapper namespace.
/// Matches "userDao" to "UserDao", "userMapper" to "UserMapper", etc.
fn names_match(field_name: &str, class_name: &str) -> bool {
    // Direct match
    if field_name == class_name {
        return true;
    }
    // Lowercase first char: userDao → UserDao
    let mut chars = field_name.chars();
    if let Some(first) = chars.next() {
        let capitalized = first.to_uppercase().to_string() + chars.as_str();
        if capitalized == class_name {
            return true;
        }
    }
    false
}
```

**Important**: We need a `file()` method on `Node` to get the source file. Add to `src/graph/mod.rs`:

```rust
impl Node {
    pub fn file(&self) -> &Path {
        match self {
            Node::Procedure { location, .. } => &location.file,
            Node::Unresolved { .. } => Path::new(""),
            Node::MappedStatement { xml_file, .. } => xml_file,
            Node::JavaSql { java_file, .. } => java_file,
            Node::JavaMethod { file, .. } => file,
            Node::JavaClass { file, .. } => file,
        }
    }
}
```

### Step 2: Update `build_all()` to call `add_java_method_nodes()`

In `src/graph/builder.rs`, modify `build_all()`:

```rust
    pub fn build_all(&self, all: &AllParsedFiles, input: &Path) -> CodeGraph {
        let mut graph = CodeGraph::new();
        let mut proc_index: HashMap<ProcedureId, petgraph::graph::NodeIndex> = HashMap::new();
        let mut mapper_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

        Self::create_procedure_nodes(&all.sql_files, &mut graph, &mut proc_index);
        let edges = Self::collect_call_edges(&all.sql_files);
        Self::create_edges(&edges, &mut graph, &mut proc_index);

        Self::add_ibatis_nodes(input, &mut graph, &mut proc_index, &mut mapper_index);
        Self::add_java_nodes(input, &mut graph, &mut proc_index, &mapper_index);

        // Phase 3: Java method extraction and bridging
        Self::add_java_method_nodes(input, &mut graph, &mut proc_index, &mapper_index);

        graph
    }
```

### Step 3: Run `cargo build` and `cargo clippy -- -D warnings`

Fix any compilation errors. The `file()` helper on `Node` must return `&Path` which requires a `use std::path::Path;` in `mod.rs`.

### Step 4: Commit

```bash
git add src/graph/mod.rs src/graph/builder.rs
git commit -m "feat: integrate Java method extraction into graph builder with mapper bridging"
```

---

## Task 5: Update CLI Stats Display

**Files:**
- Modify: `src/main.rs`

### Step 1: Add JavaMethod and JavaClass to stats counting

In `print_stats()` in `src/main.rs`, add counters:

```rust
fn print_stats(graph: &CodeGraph, include_unresolved: bool) {
    let mut procedures = 0usize;
    let mut unresolved = 0usize;
    let mut mappers = 0usize;
    let mut java_sql = 0usize;
    let mut java_methods = 0usize;
    let mut java_classes = 0usize;

    for idx in graph.node_indices() {
        match &graph[idx] {
            Node::Procedure { .. } => procedures += 1,
            Node::Unresolved { .. } => unresolved += 1,
            Node::MappedStatement { .. } => mappers += 1,
            Node::JavaSql { .. } => java_sql += 1,
            Node::JavaMethod { .. } => java_methods += 1,
            Node::JavaClass { .. } => java_classes += 1,
        }
    }

    let edges = graph.edge_count();

    if include_unresolved {
        eprintln!(
            "graph: {} procedures, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} unresolved, {} edges",
            procedures, mappers, java_sql, java_methods, java_classes, unresolved, edges
        );
    } else {
        eprintln!(
            "graph: {} procedures, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} edges",
            procedures, mappers, java_sql, java_methods, java_classes, edges
        );
    }
}
```

### Step 2: Commit

```bash
git add src/main.rs
git commit -m "feat: update CLI stats to show JavaMethod and JavaClass counts"
```

---

## Task 6: Write Integration Tests

**Files:**
- Modify: `tests/integration_test.rs`

### Step 1: Add test for Java method extraction end-to-end

```rust
fn write_java(dir: &TempDir, filename: &str, java: &str) -> std::path::PathBuf {
    let path = dir.path().join(filename);
    // Create parent directories if needed
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, java).unwrap();
    path
}

fn write_xml(dir: &TempDir, filename: &str, xml: &str) -> std::path::PathBuf {
    let path = dir.path().join(filename);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, xml).unwrap();
    path
}

#[test]
fn test_java_method_to_mapper_bridge() {
    let dir = TempDir::new().unwrap();

    // Write a mapper XML
    write_xml(&dir, "UserDao.xml", r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE mapper PUBLIC "-//mybatis.org//DTD Mapper 3.0//EN" "http://mybatis.org/dtd/mybatis-3-mapper.dtd">
<mapper namespace="com.example.dao.UserDao">
    <select id="findById" resultType="User">
        SELECT * FROM users WHERE id = #{id}
    </select>
</mapper>"#);

    // Write a Java service that calls the mapper
    write_java(&dir, "UserService.java", r#"package com.example.service;
import com.example.dao.UserDao;
public class UserService {
    private UserDao userDao;
    public Object getUser(Long id) {
        return userDao.findById(id);
    }
}"#);

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let nodes = parsed["nodes"].as_array().unwrap();

    // Should have at least: mapper node + java method + java class
    assert!(nodes.len() >= 3, "Expected at least 3 nodes, got {}", nodes.len());

    // Check that we have a java_method node
    let has_java_method = nodes.iter().any(|n| n["type"] == "java_method");
    assert!(has_java_method, "Expected a java_method node");

    // Check that we have an invokes_mapper edge
    let edges = parsed["edges"].as_array().unwrap();
    let has_bridge_edge = edges.iter().any(|e| e["type"] == "invokes_mapper");
    assert!(has_bridge_edge, "Expected an invokes_mapper edge bridging Java method to mapper");
}

#[test]
fn test_sqlsession_bridge() {
    let dir = TempDir::new().unwrap();

    write_xml(&dir, "UserMapper.xml", r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE mapper PUBLIC "-//mybatis.org//DTD Mapper 3.0//EN" "http://mybatis.org/dtd/mybatis-3-mapper.dtd">
<mapper namespace="com.example.dao.UserMapper">
    <select id="findAll" resultType="User">
        SELECT * FROM users
    </select>
</mapper>"#);

    write_java(&dir, "UserRepo.java", r#"package com.example.repo;
public class UserRepo {
    public Object getUsers() {
        return sqlSession.selectList("com.example.dao.UserMapper.findAll");
    }
}"#);

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let edges = parsed["edges"].as_array().unwrap();
    let has_bridge = edges.iter().any(|e| e["type"] == "invokes_mapper");
    assert!(has_bridge, "Expected sqlSession → mapper bridge edge");
}

#[test]
fn test_java_method_to_method_call() {
    let dir = TempDir::new().unwrap();

    write_java(&dir, "Service.java", r#"package com.example;
public class Service {
    public void doWork() {
        helper();
    }
    public void helper() {}
}"#);

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let edges = parsed["edges"].as_array().unwrap();
    let has_java_call = edges.iter().any(|e| e["type"] == "calls_java");
    assert!(has_java_call, "Expected calls_java edge between methods");
}

#[test]
fn test_java_extends_implements() {
    let dir = TempDir::new().unwrap();

    write_java(&dir, "Base.java", r#"package com.example;
public class Base {}"#);

    write_java(&dir, "Iface.java", r#"package com.example;
public interface Iface {}"#);

    write_java(&dir, "Child.java", r#"package com.example;
public class Child extends Base implements Iface {}"#);

    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let edges = parsed["edges"].as_array().unwrap();

    let has_extends = edges.iter().any(|e| e["type"] == "extends");
    let has_implements = edges.iter().any(|e| e["type"] == "implements");
    assert!(has_extends, "Expected extends edge");
    assert!(has_implements, "Expected implements edge");
}
```

### Step 2: Run all tests

Run: `cargo test`
Expected: All tests pass (8 existing + 4 new = 12 total).

### Step 3: Run clippy

Run: `cargo clippy -- -D warnings`
Expected: Clean.

### Step 4: Commit

```bash
git add tests/integration_test.rs
git commit -m "test: add Phase 3 integration tests for Java method extraction and bridging"
```

---

## Task 7: End-to-End Validation with Real Project

**Files:** None (validation only)

### Step 1: Run against ruoyi-springboot3-pro

```bash
cargo build && ./target/debug/codeweb ~/Projects/DB/ogsql-parser/by-rust/lib/ruoyi-springboot3-pro/ --format json --output /tmp/codeweb-phase3.json
```

Expected output (in stderr):
- `loaded 10 SQL, 314 Java, 36 XML file(s)`
- Stats showing `java-methods` and `java-classes` counts > 0
- Some `invokes_mapper` edges bridging Java methods to mapper statements

### Step 2: Verify the JSON output has JavaMethod and JavaClass nodes

```bash
cat /tmp/codeweb-phase3.json | python3 -c "
import json, sys
data = json.load(sys.stdin)
types = {}
for n in data['nodes']:
    t = n['type']
    types[t] = types.get(t, 0) + 1
edge_types = {}
for e in data['edges']:
    t = e['type']
    edge_types[t] = edge_types.get(t, 0) + 1
print('Nodes:', types)
print('Edges:', edge_types)
"
```

Expected: `java_method`, `java_class`, `invokes_mapper`, `calls_java`, `contains_method` types all present.

### Step 3: Run clippy and fmt

```bash
cargo clippy -- -D warnings
cargo fmt -- --check
```

Expected: Both clean.

---

## Summary of Deliverables

| Task | Files Changed | Key Output |
|---|---|---|
| Task 1 | `Cargo.toml` | tree-sitter 0.24 + tree-sitter-java 0.23 dependencies |
| Task 2 | `src/parser/java_method.rs`, `src/parser/mod.rs` | Java method/class/call extraction via tree-sitter |
| Task 3 | `src/graph/mod.rs`, `src/export/*.rs` | JavaMethod/JavaClass nodes, CallsJava/Extends/Implements edges |
| Task 4 | `src/graph/builder.rs` | Java→Mapper bridge (Pattern A + B), method→method edges |
| Task 5 | `src/main.rs` | Updated CLI stats display |
| Task 6 | `tests/integration_test.rs` | 4 new integration tests |
| Task 7 | None | E2E validation with ruoyi project |

## Key Design Decisions

1. **No full type resolution** — Uses heuristic matching (import resolution + simple name matching). Accepts some false negatives. No JVM/jdtls needed.
2. **tree-sitter version alignment** — Matches ogsql-parser's versions (0.24/0.23) to avoid conflicts.
3. **Bridge patterns** — Two patterns for Java→Mapper: (A) `sqlSession.selectList("ns.id")` string literal matching, (B) `obj.method()` with object→namespace heuristic matching.
4. **Extends/Implements** — Only resolved within the scanned project. External dependencies (java.util.*, etc.) are not resolved.
5. **Existing JavaSql nodes preserved** — The existing `JavaSql` node type (from ogsql-parser Java SQL extraction) remains. `JavaMethod` is a separate, richer node type for the full method extraction.
