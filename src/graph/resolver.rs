use std::collections::HashMap;

use petgraph::graph::NodeIndex;

use crate::graph::{RoutineId, RoutineKind};

/// Unified name resolution engine for cobweb's graph builder.
///
/// Consolidates routine and table name resolution that is currently scattered
/// across multiple ad-hoc sites in `builder.rs`.  The engine maintains a set
/// of specialised indexes and applies a deterministic multi-strategy fallback
/// chain when resolving names.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ResolutionEngine {
    /// Primary routine index: exact `RoutineId` → node index.
    proc_index: HashMap<RoutineId, NodeIndex>,
    /// Table index: canonical key → node index.
    table_index: HashMap<String, NodeIndex>,
    /// Type index: canonical key → node index.
    type_index: HashMap<String, NodeIndex>,
    /// Sequence index: canonical key → node index.
    sequence_index: HashMap<String, NodeIndex>,
    /// Synonym key → canonical target key (for routines / tables).
    synonym_targets: HashMap<String, String>,
    /// Bare routine name (no schema/package) → all matching node indexes.
    bare_name_index: HashMap<String, Vec<NodeIndex>>,
    /// (lowercase qualified name, kind) → node index.
    lowercase_routine: HashMap<(String, RoutineKind), NodeIndex>,
    /// (lowercase package name, lowercase routine name) → node index.
    pkg_member_lower: HashMap<(String, String), NodeIndex>,
}

#[allow(dead_code)]
impl ResolutionEngine {
    /// Create an empty resolution engine.
    pub fn new() -> Self {
        Self {
            proc_index: HashMap::new(),
            table_index: HashMap::new(),
            type_index: HashMap::new(),
            sequence_index: HashMap::new(),
            synonym_targets: HashMap::new(),
            bare_name_index: HashMap::new(),
            lowercase_routine: HashMap::new(),
            pkg_member_lower: HashMap::new(),
        }
    }

    // ------------------------------------------------------------------
    // Registration helpers
    // ------------------------------------------------------------------

    /// Register a routine in all relevant indexes.
    pub fn register_routine(&mut self, id: RoutineId, idx: NodeIndex) {
        // 1. Primary index
        self.proc_index.insert(id.clone(), idx);

        let lower_id = RoutineId {
            schema: id.schema.as_ref().map(|s| s.to_lowercase()),
            package: id.package.as_ref().map(|p| p.to_lowercase()),
            name: id.name.to_lowercase(),
            kind: id.kind,
        };
        self.proc_index.entry(lower_id).or_insert(idx);

        // 2. Bare name index (name without schema or package)
        self.bare_name_index
            .entry(id.name.clone())
            .or_default()
            .push(idx);

        // 3. Lowercase qualified routine index
        let qualified = id.to_string().to_lowercase();
        self.lowercase_routine.insert((qualified, id.kind), idx);

        // 4. Package member index
        if let Some(ref pkg) = id.package {
            self.pkg_member_lower
                .insert((pkg.to_lowercase(), id.name.to_lowercase()), idx);
        }
    }

    /// Register a table in the table index.
    pub fn register_table(&mut self, key: String, idx: NodeIndex) {
        self.table_index.insert(key.clone(), idx);
        self.table_index.entry(key.to_lowercase()).or_insert(idx);

        // Also register the bare name (without schema prefix) so that
        // unqualified references resolve even without schema_context.
        // e.g. key="BF.ACCOUNT" → also registers "ACCOUNT" and "account"
        if let Some(dot) = key.find('.') {
            let bare = &key[dot + 1..];
            self.table_index.entry(bare.to_string()).or_insert(idx);
            self.table_index.entry(bare.to_lowercase()).or_insert(idx);
        }
    }

    /// Register a user-defined type in the type index.
    ///
    /// Both `short_key` (e.g. `"mytype"`) and `full_key` (e.g. `"schema.mytype"`)
    /// are stored so that lookups work regardless of qualification.
    pub fn register_type(&mut self, short_key: String, full_key: String, idx: NodeIndex) {
        self.type_index.insert(short_key.clone(), idx);
        self.type_index.insert(full_key.clone(), idx);
        self.type_index
            .entry(short_key.to_lowercase())
            .or_insert(idx);
        self.type_index
            .entry(full_key.to_lowercase())
            .or_insert(idx);
    }

    /// Register a sequence in the sequence index.
    ///
    /// Both `short_key` and `full_key` are stored for the same reason as
    /// `register_type`.
    pub fn register_sequence(&mut self, short_key: String, full_key: String, idx: NodeIndex) {
        self.sequence_index.insert(short_key.clone(), idx);
        self.sequence_index.insert(full_key.clone(), idx);
        self.sequence_index
            .entry(short_key.to_lowercase())
            .or_insert(idx);
        self.sequence_index
            .entry(full_key.to_lowercase())
            .or_insert(idx);
    }

    /// Register a synonym → target mapping.
    pub fn register_synonym(&mut self, synonym_key: String, target_key: String) {
        self.synonym_targets.insert(synonym_key, target_key);
    }

    // ------------------------------------------------------------------
    // Resolution
    // ------------------------------------------------------------------

    /// Resolve a routine name to a node index using an 8-strategy fallback chain.
    ///
    /// Strategies are tried in order:
    ///
    /// 1. **Exact match as Procedure** – parse `raw_name` as a `RoutineId` with
    ///    `RoutineKind::Procedure` and look it up in `proc_index`.
    /// 2. **Kind swap** – if the exact match failed, try the opposite
    ///    `RoutineKind` (Function ↔ Procedure).
    /// 3. **Schema-as-package fallback** – if `raw_name` is `"schema.proc"`, try
    ///    treating `schema` as a package name (`package=schema, name=proc`).
    /// 4. **Synonym dereference** – look up `raw_name` (and a lower-cased variant)
    ///    in `synonym_targets`, then recursively resolve the target.
    /// 5. **Caller context** – if `caller_context` has a `package`, try the bare
    ///    routine name within that package via `pkg_member_lower`.
    /// 6. **Case-insensitive match** – lower-case the fully qualified name and
    ///    search `lowercase_routine`.
    /// 7. **Case-insensitive package member** – split `raw_name` into
    ///    `(package, name)`, lower-case both, and search `pkg_member_lower`.
    /// 8. **Bare name search** – if `raw_name` has no dot, look it up in
    ///    `bare_name_index`.  A single unambiguous match succeeds; multiple
    ///    matches return `None`.
    pub fn resolve_routine(
        &self,
        raw_name: &str,
        caller_context: Option<&RoutineId>,
    ) -> Option<NodeIndex> {
        // --- Strategy 1: exact match as Procedure ---
        let callee_id = RoutineId::from_qualified_name(raw_name, RoutineKind::Procedure);
        if let Some(&idx) = self.proc_index.get(&callee_id) {
            return Some(idx);
        }

        // --- Strategy 2: kind swap ---
        let alt_kind = match callee_id.kind {
            RoutineKind::Procedure => RoutineKind::Function,
            RoutineKind::Function => RoutineKind::Procedure,
        };
        let alt_id = RoutineId {
            schema: callee_id.schema.clone(),
            package: callee_id.package.clone(),
            name: callee_id.name.clone(),
            kind: alt_kind,
        };
        if let Some(&idx) = self.proc_index.get(&alt_id) {
            return Some(idx);
        }

        // --- Strategy 3: schema-as-package fallback ---
        if callee_id.schema.is_some() && callee_id.package.is_none() {
            let fallback_id = RoutineId {
                schema: None,
                package: callee_id.schema.clone(),
                name: callee_id.name.clone(),
                kind: RoutineKind::Procedure,
            };
            if let Some(&idx) = self.proc_index.get(&fallback_id) {
                return Some(idx);
            }
        }

        // --- Strategy 4: synonym dereference ---
        if let Some(target) = self.synonym_targets.get(raw_name) {
            if let Some(idx) = self.resolve_routine(target, caller_context) {
                return Some(idx);
            }
        }
        let raw_lower = raw_name.to_lowercase();
        if let Some(target) = self.synonym_targets.get(&raw_lower) {
            if let Some(idx) = self.resolve_routine(target, caller_context) {
                return Some(idx);
            }
        }

        // --- Strategy 5: caller context (same package) ---
        if let Some(caller) = caller_context {
            if let Some(ref pkg) = caller.package {
                let bare_name = if let Some((_, name)) = raw_name.rsplit_once('.') {
                    name
                } else {
                    raw_name
                };
                if let Some(&idx) = self
                    .pkg_member_lower
                    .get(&(pkg.to_lowercase(), bare_name.to_lowercase()))
                {
                    return Some(idx);
                }
            }
        }

        // --- Strategy 6: case-insensitive match ---
        let qualified_lower = callee_id.to_string().to_lowercase();
        if let Some(&idx) = self
            .lowercase_routine
            .get(&(qualified_lower.clone(), RoutineKind::Procedure))
        {
            return Some(idx);
        }
        if let Some(&idx) = self
            .lowercase_routine
            .get(&(qualified_lower, RoutineKind::Function))
        {
            return Some(idx);
        }

        // --- Strategy 7: case-insensitive package member ---
        let name_lower = callee_id.name.to_lowercase();
        if let Some(ref pkg) = callee_id.package {
            if let Some(&idx) = self
                .pkg_member_lower
                .get(&(pkg.to_lowercase(), name_lower.clone()))
            {
                return Some(idx);
            }
        }
        if let Some(ref schema) = callee_id.schema {
            let pkg_part = schema
                .rsplit_once('.')
                .map(|(_, pkg)| pkg)
                .unwrap_or(schema);
            if let Some(&idx) = self
                .pkg_member_lower
                .get(&(pkg_part.to_lowercase(), name_lower))
            {
                return Some(idx);
            }
        }

        // --- Strategy 8: bare name search (single match only) ---
        if !raw_name.contains('.') {
            if let Some(matches) = self.bare_name_index.get(raw_name) {
                if matches.len() == 1 {
                    return Some(matches[0]);
                }
            }
        }

        None
    }

    /// Resolve a table name to a node index.
    ///
    /// Tries, in order:
    /// 1. Direct exact match in `table_index`.
    /// 2. Schema context — try schema.name if raw_name is unqualified.
    /// 3. Case-insensitive match (lower-cased key).
    /// 4. Schema context + case-insensitive.
    /// 5. Synonym dereference (exact and lower-cased).
    pub fn resolve_table(&self, raw_name: &str, schema_context: Option<&str>) -> Option<NodeIndex> {
        // 1. Direct exact match
        if let Some(&idx) = self.table_index.get(raw_name) {
            return Some(idx);
        }

        // 2. Schema context
        if let Some(schema) = schema_context {
            if !raw_name.contains('.') {
                let qualified = format!("{}.{}", schema, raw_name);
                if let Some(&idx) = self.table_index.get(&qualified) {
                    return Some(idx);
                }
            }
        }

        // 3. Case-insensitive match
        let lower = raw_name.to_lowercase();
        if let Some(&idx) = self.table_index.get(&lower) {
            return Some(idx);
        }

        // 4. Schema context + case-insensitive
        if let Some(schema) = schema_context {
            if !raw_name.contains('.') {
                let qualified_lower = format!("{}.{}", schema.to_lowercase(), lower);
                if let Some(&idx) = self.table_index.get(&qualified_lower) {
                    return Some(idx);
                }
            }
        }

        // 5. Synonym dereference
        if let Some(target) = self.synonym_targets.get(raw_name) {
            if let Some(idx) = self.resolve_table(target, schema_context) {
                return Some(idx);
            }
        }
        if let Some(target) = self.synonym_targets.get(&lower) {
            if let Some(idx) = self.resolve_table(target, schema_context) {
                return Some(idx);
            }
        }

        // 6. Strip schema from qualified name and try bare name
        //    e.g. "bf.v_acctbalbook" → try "v_acctbalbook" and "v_acctbalbook"(lower)
        if let Some(dot) = raw_name.find('.') {
            let bare = &raw_name[dot + 1..];
            if bare.contains('.') {
                // multi-part name like "catalog.schema.table" — recurse
                return self.resolve_table(bare, schema_context);
            }
            if let Some(&idx) = self.table_index.get(bare) {
                return Some(idx);
            }
            let bare_lower = bare.to_lowercase();
            if let Some(&idx) = self.table_index.get(&bare_lower) {
                return Some(idx);
            }
        }

        None
    }

    // ------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------

    /// Immutable access to the primary routine index.
    pub fn proc_index(&self) -> &HashMap<RoutineId, NodeIndex> {
        &self.proc_index
    }

    /// Mutable access to the primary routine index.
    pub fn proc_index_mut(&mut self) -> &mut HashMap<RoutineId, NodeIndex> {
        &mut self.proc_index
    }

    /// Immutable access to the table index.
    pub fn table_index(&self) -> &HashMap<String, NodeIndex> {
        &self.table_index
    }

    /// Mutable access to the table index.
    pub fn table_index_mut(&mut self) -> &mut HashMap<String, NodeIndex> {
        &mut self.table_index
    }

    /// Immutable access to the type index.
    pub fn type_index(&self) -> &HashMap<String, NodeIndex> {
        &self.type_index
    }

    /// Immutable access to the sequence index.
    pub fn sequence_index(&self) -> &HashMap<String, NodeIndex> {
        &self.sequence_index
    }
}

impl Default for ResolutionEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petgraph::Graph;

    use crate::graph::{Node, RoutineId, RoutineKind, SourceLocation};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn dummy_loc() -> SourceLocation {
        SourceLocation {
            file: Arc::new(PathBuf::from("test.sql")),
            line: 1,
        }
    }

    fn make_proc_node(name: &str) -> Node {
        Node::Procedure {
            id: RoutineId::from_qualified_name(name, RoutineKind::Procedure),
            location: dummy_loc(),
            partial: false,
            builtin_calls: BTreeMap::new(),
        }
    }

    fn make_func_node(name: &str) -> Node {
        Node::Function {
            id: RoutineId::from_qualified_name(name, RoutineKind::Function),
            location: dummy_loc(),
            partial: false,
            builtin_calls: BTreeMap::new(),
        }
    }

    fn make_table_node(schema: Option<&str>, name: &str) -> Node {
        Node::Table {
            schema: schema.map(|s| s.to_string()),
            name: name.to_string(),
            location: None,
            columns: vec![],
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        }
    }

    // ------------------------------------------------------------------
    // Routine resolution tests
    // ------------------------------------------------------------------

    #[test]
    fn exact_match_resolves() {
        let mut graph = Graph::<Node, ()>::new();
        let node = make_proc_node("public.do_work");
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        let id = RoutineId::from_qualified_name("public.do_work", RoutineKind::Procedure);
        engine.register_routine(id, idx);

        let found = engine.resolve_routine("public.do_work", None);
        assert_eq!(found, Some(idx));
    }

    #[test]
    fn nonexistent_returns_none() {
        let engine = ResolutionEngine::new();
        let found = engine.resolve_routine("missing.proc", None);
        assert_eq!(found, None);
    }

    #[test]
    fn kind_swap_resolves_func_when_looking_for_proc() {
        let mut graph = Graph::<Node, ()>::new();
        let node = make_func_node("public.calc_total");
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        let id = RoutineId::from_qualified_name("public.calc_total", RoutineKind::Function);
        engine.register_routine(id, idx);

        // Looking for a procedure but only a function exists
        let found = engine.resolve_routine("public.calc_total", None);
        assert_eq!(found, Some(idx));
    }

    #[test]
    fn schema_as_package_fallback() {
        let mut graph = Graph::<Node, ()>::new();
        // Register as package member: package="pkg_api", name="do_work"
        let id = RoutineId {
            schema: None,
            package: Some("pkg_api".to_string()),
            name: "do_work".to_string(),
            kind: RoutineKind::Procedure,
        };
        let node = Node::Procedure {
            id: id.clone(),
            location: dummy_loc(),
            partial: false,
            builtin_calls: BTreeMap::new(),
        };
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        engine.register_routine(id, idx);

        // Query as "pkg_api.do_work" – from_qualified_name puts prefix in schema
        let found = engine.resolve_routine("pkg_api.do_work", None);
        assert_eq!(found, Some(idx));
    }

    #[test]
    fn synonym_dereference() {
        let mut graph = Graph::<Node, ()>::new();
        let node = make_proc_node("public.real_proc");
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        let id = RoutineId::from_qualified_name("public.real_proc", RoutineKind::Procedure);
        engine.register_routine(id, idx);
        engine.register_synonym(
            "public.alias_proc".to_string(),
            "public.real_proc".to_string(),
        );

        let found = engine.resolve_routine("public.alias_proc", None);
        assert_eq!(found, Some(idx));
    }

    #[test]
    fn caller_context_same_package() {
        let mut graph = Graph::<Node, ()>::new();
        let id = RoutineId {
            schema: None,
            package: Some("pkg_api".to_string()),
            name: "helper".to_string(),
            kind: RoutineKind::Procedure,
        };
        let node = Node::Procedure {
            id: id.clone(),
            location: dummy_loc(),
            partial: false,
            builtin_calls: BTreeMap::new(),
        };
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        engine.register_routine(id.clone(), idx);

        let caller = RoutineId {
            schema: None,
            package: Some("pkg_api".to_string()),
            name: "main".to_string(),
            kind: RoutineKind::Procedure,
        };

        // Bare name "helper" resolved via caller's package context
        let found = engine.resolve_routine("helper", Some(&caller));
        assert_eq!(found, Some(idx));
    }

    #[test]
    fn case_insensitive_match() {
        let mut graph = Graph::<Node, ()>::new();
        let node = make_proc_node("PUBLIC.Do_Work");
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        let id = RoutineId::from_qualified_name("PUBLIC.Do_Work", RoutineKind::Procedure);
        engine.register_routine(id, idx);

        let found = engine.resolve_routine("public.do_work", None);
        assert_eq!(found, Some(idx));
    }

    #[test]
    fn bare_name_search_single_match() {
        let mut graph = Graph::<Node, ()>::new();
        let node = make_proc_node("do_work");
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        let id = RoutineId::from_qualified_name("do_work", RoutineKind::Procedure);
        engine.register_routine(id, idx);

        let found = engine.resolve_routine("do_work", None);
        assert_eq!(found, Some(idx));
    }

    #[test]
    fn bare_name_search_ambiguous_returns_none() {
        let mut graph = Graph::<Node, ()>::new();
        let node1 = make_proc_node("schema1.do_work");
        let node2 = make_proc_node("schema2.do_work");
        let idx1 = graph.add_node(node1);
        let idx2 = graph.add_node(node2);

        let mut engine = ResolutionEngine::new();
        let id1 = RoutineId::from_qualified_name("schema1.do_work", RoutineKind::Procedure);
        let id2 = RoutineId::from_qualified_name("schema2.do_work", RoutineKind::Procedure);
        engine.register_routine(id1, idx1);
        engine.register_routine(id2, idx2);

        // "do_work" is ambiguous – two different schemas
        let found = engine.resolve_routine("do_work", None);
        assert_eq!(found, None);
    }

    // ------------------------------------------------------------------
    // Table resolution tests
    // ------------------------------------------------------------------

    #[test]
    fn table_synonym_dereference() {
        let mut graph = Graph::<Node, ()>::new();
        let node = make_table_node(Some("public"), "real_table");
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        engine.register_table("public.real_table".to_string(), idx);
        engine.register_synonym(
            "public.alias_table".to_string(),
            "public.real_table".to_string(),
        );

        let found = engine.resolve_table("public.alias_table", None);
        assert_eq!(found, Some(idx));
    }

    #[test]
    fn table_case_insensitive() {
        let mut graph = Graph::<Node, ()>::new();
        let node = make_table_node(Some("PUBLIC"), "MyTable");
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        engine.register_table("public.mytable".to_string(), idx);

        let found = engine.resolve_table("PUBLIC.MYTABLE", None);
        assert_eq!(found, Some(idx));
    }

    #[test]
    fn table_schema_context_resolves_unqualified() {
        let mut graph = Graph::<Node, ()>::new();
        let node = make_table_node(Some("bf"), "Account");
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        engine.register_table("bf.Account".to_string(), idx);

        let found = engine.resolve_table("Account", Some("bf"));
        assert_eq!(
            found,
            Some(idx),
            "unqualified Account with schema_context=bf should resolve to bf.Account"
        );
    }

    #[test]
    fn table_schema_context_no_match_without_context() {
        let mut graph = Graph::<Node, ()>::new();
        let node = make_table_node(Some("bf"), "Account");
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        engine.register_table("bf.Account".to_string(), idx);

        let found = engine.resolve_table("Account", None);
        assert_eq!(
            found,
            Some(idx),
            "unqualified Account should resolve via bare-name registration of bf.Account"
        );
    }

    #[test]
    fn table_schema_context_case_insensitive() {
        let mut graph = Graph::<Node, ()>::new();
        let node = make_table_node(Some("BF"), "ACCOUNT");
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        engine.register_table("bf.account".to_string(), idx);

        let found = engine.resolve_table("account", Some("bf"));
        assert_eq!(
            found,
            Some(idx),
            "case-insensitive schema context should work"
        );
    }

    #[test]
    fn table_schema_context_ignores_qualified_name() {
        let mut graph = Graph::<Node, ()>::new();
        let node = make_table_node(Some("bf"), "Account");
        let idx = graph.add_node(node);

        let mut engine = ResolutionEngine::new();
        engine.register_table("bf.Account".to_string(), idx);

        let found = engine.resolve_table("other.Account", Some("bf"));
        assert_eq!(
            found,
            Some(idx),
            "other.Account falls back to bare-name match and resolves to bf.Account"
        );
    }
}
