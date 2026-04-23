#![allow(dead_code)]
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tree_sitter::Parser;

thread_local! {
    static JAVA_PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        let _ = p.set_language(&tree_sitter_java::LANGUAGE.into());
        p.set_timeout_micros(5_000_000);
        p
    });
}

// Public data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct JavaClassInfo {
    pub fqn: String,
    pub name: String,
    pub package: String,
    pub extends: Option<String>,
    pub implements: Vec<String>,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct MethodCallInfo {
    pub object: Option<String>,
    pub method: String,
    pub string_args: Vec<String>,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct JavaMethodInfo {
    pub name: String,
    pub class_fqn: String,
    pub signature: String,
    pub file: PathBuf,
    pub line: usize,
    pub calls: Vec<MethodCallInfo>,
}

#[derive(Debug, Clone)]
pub struct JavaParseResult {
    pub file: PathBuf,
    pub package: String,
    pub imports: Vec<String>,
    pub classes: Vec<JavaClassInfo>,
    pub methods: Vec<JavaMethodInfo>,
}

// Internal extraction engine
// ---------------------------------------------------------------------------

struct JavaTreeWalker<'a> {
    source: &'a [u8],
    file: &'a Path,
    package: String,
    imports: Vec<String>,
    classes: Vec<JavaClassInfo>,
    methods: Vec<JavaMethodInfo>,
    current_class_stack: Vec<String>,
}

impl<'a> JavaTreeWalker<'a> {
    fn new(source: &'a [u8], file: &'a Path) -> Self {
        Self {
            source,
            file,
            package: String::new(),
            imports: Vec::new(),
            classes: Vec::new(),
            methods: Vec::new(),
            current_class_stack: Vec::new(),
        }
    }

    fn current_fqn(&self) -> String {
        let class_part = self.current_class_stack.join(".");
        if self.package.is_empty() {
            class_part
        } else {
            format!("{}.{}", self.package, class_part)
        }
    }

    fn handle_package(&mut self, node: tree_sitter::Node) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "scoped_identifier" {
                if let Ok(text) = child.utf8_text(self.source) {
                    self.package = text.to_string();
                }
                return;
            }
        }
    }

    fn handle_import(&mut self, node: tree_sitter::Node) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "scoped_identifier" {
                if let Ok(text) = child.utf8_text(self.source) {
                    self.imports.push(text.to_string());
                }
                return;
            }
        }
    }

    fn extract_extends(&self, node: tree_sitter::Node) -> Option<String> {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "superclass" {
                let mut cur2 = child.walk();
                for sc in child.children(&mut cur2) {
                    if sc.kind() == "type_identifier" {
                        return sc.utf8_text(self.source).ok().map(|s| s.to_string());
                    }
                }
            }
        }
        None
    }

    fn extract_implements(&self, node: tree_sitter::Node) -> Vec<String> {
        let mut result = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "super_interfaces" {
                let mut cur2 = child.walk();
                for si_child in child.children(&mut cur2) {
                    if si_child.kind() == "type_list" {
                        let mut cur3 = si_child.walk();
                        for tl_child in si_child.children(&mut cur3) {
                            if tl_child.kind() == "type_identifier" {
                                if let Ok(text) = tl_child.utf8_text(self.source) {
                                    result.push(text.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        result
    }

    fn handle_method(&mut self, node: tree_sitter::Node) {
        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(self.source).ok())
            .unwrap_or("<unknown>")
            .to_string();

        let class_fqn = self.current_fqn();
        let line = node.start_position().row + 1;
        let signature = self.build_signature(node);
        let calls = self.collect_method_calls(node);

        self.methods.push(JavaMethodInfo {
            name,
            class_fqn,
            signature,
            file: self.file.to_path_buf(),
            line,
            calls,
        });
    }

    fn build_signature(&self, node: tree_sitter::Node) -> String {
        let return_type = node
            .child_by_field_name("type")
            .and_then(|n| n.utf8_text(self.source).ok())
            .unwrap_or("void")
            .to_string();

        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(self.source).ok())
            .unwrap_or("<unknown>")
            .to_string();

        let params = node
            .child_by_field_name("parameters")
            .and_then(|n| n.utf8_text(self.source).ok())
            .unwrap_or("()")
            .to_string();

        format!("{} {}{}", return_type, name, params)
    }

    fn collect_method_calls(&self, node: tree_sitter::Node) -> Vec<MethodCallInfo> {
        let mut calls = Vec::new();
        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        self.collect_calls_recursive(node, &mut calls, &mut seen);
        calls
    }

    fn collect_calls_recursive(
        &self,
        node: tree_sitter::Node,
        calls: &mut Vec<MethodCallInfo>,
        seen: &mut HashSet<(usize, usize)>,
    ) {
        if node.kind() == "method_invocation" {
            let pos = node.start_position();
            let key = (pos.row, pos.column);
            if seen.insert(key) {
                if let Some(call) = self.extract_method_call(node) {
                    calls.push(call);
                }
            }
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.collect_calls_recursive(child, calls, seen);
        }
    }

    fn extract_method_call(&self, node: tree_sitter::Node) -> Option<MethodCallInfo> {
        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(self.source).ok())
            .unwrap_or("")
            .to_string();

        if name.is_empty() {
            return None;
        }

        let object = node
            .child_by_field_name("object")
            .and_then(|n| n.utf8_text(self.source).ok())
            .map(|s| s.to_string());

        let string_args = self.extract_string_args(node);
        let line = node.start_position().row + 1;

        Some(MethodCallInfo {
            object,
            method: name,
            string_args,
            line,
        })
    }

    fn extract_string_args(&self, node: tree_sitter::Node) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(arg_list) = node.child_by_field_name("arguments") {
            let mut cursor = arg_list.walk();
            for child in arg_list.children(&mut cursor) {
                if child.kind() == "string_literal" {
                    if let Ok(text) = child.utf8_text(self.source) {
                        let inner = text
                            .strip_prefix('"')
                            .and_then(|s| s.strip_suffix('"'))
                            .unwrap_or(text);
                        args.push(inner.to_string());
                    }
                }
            }
        }
        args
    }

    fn walk_with_methods(&mut self, node: tree_sitter::Node) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "package_declaration" => self.handle_package(child),
                "import_declaration" => self.handle_import(child),
                "class_declaration" | "interface_declaration" => {
                    self.handle_type_with_methods(child);
                }
                _ => {
                    self.walk_with_methods(child);
                }
            }
        }
    }

    fn handle_type_with_methods(&mut self, node: tree_sitter::Node) {
        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(self.source).ok())
            .unwrap_or("")
            .to_string();

        let fqn = if self.package.is_empty() {
            name.clone()
        } else {
            format!("{}.{}", self.package, name)
        };

        let line = node.start_position().row + 1;
        let extends = self.extract_extends(node);
        let implements = self.extract_implements(node);

        self.current_class_stack.push(name.clone());
        self.classes.push(JavaClassInfo {
            fqn,
            name,
            package: self.package.clone(),
            extends,
            implements,
            file: self.file.to_path_buf(),
            line,
        });

        self.walk_type_body(node);
        self.current_class_stack.pop();
    }

    fn walk_type_body(&mut self, node: tree_sitter::Node) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "method_declaration" => {
                    self.handle_method(child);
                }
                "class_declaration" | "interface_declaration" => {
                    self.handle_type_with_methods(child);
                }
                _ => {
                    self.walk_type_body(child);
                }
            }
        }
    }
}

// Public API
// ---------------------------------------------------------------------------

pub fn parse_java_file(path: &Path) -> Result<JavaParseResult, String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("read error: {}", e))?;
    let source_bytes = source.as_bytes();

    let tree = JAVA_PARSER
        .with(|p| p.borrow_mut().parse(source_bytes, None))
        .ok_or_else(|| format!("parse timeout or failure: {}", path.display()))?;

    let mut walker = JavaTreeWalker::new(source_bytes, path);
    walker.walk_with_methods(tree.root_node());

    Ok(JavaParseResult {
        file: path.to_path_buf(),
        package: walker.package,
        imports: walker.imports,
        classes: walker.classes,
        methods: walker.methods,
    })
}

pub fn parse_java_files_from_paths(paths: &[PathBuf]) -> Vec<JavaParseResult> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| parse_java_file(path).ok())
        .collect()
}

// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse_source(source: &str) -> JavaParseResult {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .unwrap();

        let source_bytes = source.as_bytes();
        let tree = parser.parse(source_bytes, None).unwrap();

        let dummy_path = PathBuf::from("Test.java");
        let mut walker = JavaTreeWalker::new(source_bytes, &dummy_path);
        walker.walk_with_methods(tree.root_node());

        JavaParseResult {
            file: dummy_path.clone(),
            package: walker.package,
            imports: walker.imports,
            classes: walker.classes,
            methods: walker.methods,
        }
    }

    #[test]
    fn test_extract_package() {
        let source = r#"
package com.example.service;

public class Foo {
    public void bar() {}
}
"#;
        let result = parse_source(source);
        assert_eq!(result.package, "com.example.service");
    }

    #[test]
    fn test_extract_class_with_extends() {
        let source = r#"
package com.example;

public class Dog extends Animal implements Serializable, Comparable {
    public void bark() {}
}
"#;
        let result = parse_source(source);
        assert_eq!(result.classes.len(), 1);

        let cls = &result.classes[0];
        assert_eq!(cls.name, "Dog");
        assert_eq!(cls.fqn, "com.example.Dog");
        assert_eq!(cls.extends.as_deref(), Some("Animal"));
        assert_eq!(cls.implements, vec!["Serializable", "Comparable"]);
    }

    #[test]
    fn test_extract_method_with_call() {
        let source = r#"
package com.example;

public class UserService {
    public User getUser(int id) {
        return userDao.findById(id);
    }
}
"#;
        let result = parse_source(source);
        assert_eq!(result.methods.len(), 1);

        let method = &result.methods[0];
        assert_eq!(method.name, "getUser");
        assert_eq!(method.class_fqn, "com.example.UserService");

        assert_eq!(method.calls.len(), 1);
        let call = &method.calls[0];
        assert_eq!(call.object.as_deref(), Some("userDao"));
        assert_eq!(call.method, "findById");
    }

    #[test]
    fn test_extract_sqlsession_call() {
        let source = r#"
package com.example;

public class Repository {
    public void query() {
        sqlSession.selectList("ns.id");
    }
}
"#;
        let result = parse_source(source);
        assert_eq!(result.methods.len(), 1);

        let method = &result.methods[0];
        assert_eq!(method.calls.len(), 1);

        let call = &method.calls[0];
        assert_eq!(call.object.as_deref(), Some("sqlSession"));
        assert_eq!(call.method, "selectList");
        assert_eq!(call.string_args, vec!["ns.id"]);
    }

    #[test]
    fn test_extract_imports() {
        let source = r#"
package com.example;

import java.util.List;
import java.util.ArrayList;

public class Foo {
    public void bar() {}
}
"#;
        let result = parse_source(source);
        assert_eq!(result.imports.len(), 2);
        assert_eq!(result.imports[0], "java.util.List");
        assert_eq!(result.imports[1], "java.util.ArrayList");
    }
}
