//! 静态表结构导出的公开接口契约。

use codeweb::embed::EmbeddedProject;

fn project(sql: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("codeweb.toml"),
        "[project]\nname = 'schema'\n[analysis]\npaths = ['schema.sql']\n",
    )
    .unwrap();
    std::fs::write(root.path().join("schema.sql"), sql).unwrap();
    root
}

#[test]
fn should_mark_implicit_serial_defaults_as_opaque() {
    let root = project("CREATE TABLE items(id BIGSERIAL, n SERIAL);");
    let schema = EmbeddedProject::open_exact(root.path())
        .unwrap()
        .export_table_schemas()
        .unwrap();
    assert!(schema.tables[0].has_opaque_ddl, "{schema:?}");
}

#[test]
fn should_report_missing_or_undecodable_sources_instead_of_claiming_completeness() {
    let root = project("");
    std::fs::write(
        root.path().join("codeweb.toml"),
        "[project]\nname = 'schema'\n[analysis]\npaths = ['missing.sql']\n",
    )
    .unwrap();
    let error = EmbeddedProject::open_exact(root.path())
        .unwrap()
        .export_table_schemas()
        .unwrap_err()
        .to_string();
    assert!(error.contains("missing.sql"), "{error}");

    let root = project("");
    std::fs::write(root.path().join("schema.sql"), [0xff, 0xfe, 0x00]).unwrap();
    let error = EmbeddedProject::open_exact(root.path())
        .unwrap()
        .export_table_schemas()
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("schema.sql") && error.contains("UTF-8/GBK"),
        "{error}"
    );
}

#[cfg(feature = "analyze")]
#[test]
fn should_export_schema_without_loading_or_rewriting_existing_store() {
    let root = project("CREATE TABLE items(id BIGINT);");
    std::fs::write(
        root.path().join("codeweb.toml"),
        "[project]\nname = 'schema'\n[analysis]\npaths = ['schema.sql']\n[store]\npath = '.codeweb/store.bincode'\nformat = 'bincode'\n",
    ).unwrap();
    let mut project = EmbeddedProject::open_exact(root.path()).unwrap();
    project.analyze().unwrap();
    let store_path = root.path().join(".codeweb/store.bincode");
    let before = std::fs::read(&store_path).unwrap();
    let schema = project.export_table_schemas().unwrap();
    assert_eq!(schema.tables.len(), 1);
    assert_eq!(std::fs::read(&store_path).unwrap(), before);
    let _thin = project.export_thin_graph().unwrap();
    assert_eq!(project.export_table_schemas().unwrap(), schema);
    assert_eq!(std::fs::read(&store_path).unwrap(), before);
}

#[test]
fn should_honor_configured_source_roots_and_directory_exclusions() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("codeweb.toml"),
        "[project]\nname = 'schema'\n[analysis]\npaths = ['ddl', 'shared']\nexclude = ['ignored/']\n",
    ).unwrap();
    std::fs::create_dir_all(root.path().join("ddl")).unwrap();
    std::fs::create_dir_all(root.path().join("shared/ignored")).unwrap();
    std::fs::write(
        root.path().join("ddl/tables.sql"),
        "CREATE TABLE items(id BIGINT);",
    )
    .unwrap();
    std::fs::write(
        root.path().join("shared/indexes.sql"),
        "CREATE UNIQUE INDEX ux ON items(id);",
    )
    .unwrap();
    std::fs::write(
        root.path().join("shared/ignored/bad.sql"),
        "CREATE UNIQUE INDEX broken ON items (;",
    )
    .unwrap();
    std::fs::write(
        root.path().join("outside.sql"),
        "ALTER TABLE items ADD COLUMN hidden BIGINT;",
    )
    .unwrap();
    let schema = EmbeddedProject::open_exact(root.path())
        .unwrap()
        .export_table_schemas()
        .unwrap();
    assert!(schema.diagnostics.is_empty(), "{schema:?}");
    assert_eq!(schema.tables.len(), 1);
    assert_eq!(schema.tables[0].indexes.len(), 1);
    assert!(schema.tables[0].indexes_complete && !schema.tables[0].has_opaque_ddl);
}

#[test]
fn should_not_invalidate_schemas_for_unexecuted_routine_definitions() {
    let root = project(
        "CREATE TABLE items(id BIGINT);\n\
        CREATE PROCEDURE insert_item() IS BEGIN INSERT INTO items VALUES(1); END;",
    );
    let schema = EmbeddedProject::open_exact(root.path())
        .unwrap()
        .export_table_schemas()
        .unwrap();
    assert_eq!(schema.tables.len(), 1);
    assert!(
        schema.tables[0].indexes_complete && !schema.tables[0].has_opaque_ddl,
        "{schema:?}"
    );
    assert!(schema.diagnostics.is_empty());
}

#[test]
fn should_preserve_quoted_unicode_identifiers_and_multiline_raw_defaults() {
    let root = project(
        "-- 中文注释\r\nCREATE TABLE \"Sales\".\"条目\"(\"Mixed\" VARCHAR(24) DEFAULT '张三',\r\n\
        \"a\"\"b\" BIGINT, PLAIN TEXT DEFAULT '多\n行');\r\n\
        CREATE UNIQUE INDEX \"Sales\".\"索引\" ON \"Sales\".\"条目\"(\"Mixed\");",
    );
    let schema = EmbeddedProject::open_exact(root.path())
        .unwrap()
        .export_table_schemas()
        .unwrap();
    assert!(schema.diagnostics.is_empty(), "{schema:?}");
    let table = &schema.tables[0];
    assert_eq!(table.name, ["Sales", "条目"]);
    assert_eq!(
        table
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.default_expr_raw.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("Mixed", Some("'张三'")),
            ("a\"b", None),
            ("plain", Some("'多\n行'")),
        ]
    );
    assert_eq!(
        table.indexes[0].name,
        Some(vec!["Sales".into(), "索引".into()])
    );
    assert!(table.indexes_complete && !table.has_opaque_ddl);
}

#[test]
fn should_not_treat_unmodeled_or_invalid_ddl_as_absent_indexes() {
    for suffix in [
        "ALTER TABLE items ADD CONSTRAINT uq UNIQUE(id);",
        "ALTER TABLE items ADD COLUMN hidden BIGINT;",
        "DROP INDEX ix;",
        "CREATE TABLE inherited (LIKE items);",
        "CREATE UNIQUE INDEX broken ON items (;",
    ] {
        let root = project(&format!("CREATE TABLE items(id BIGINT); {suffix}"));
        let schema = EmbeddedProject::open_exact(root.path())
            .unwrap()
            .export_table_schemas()
            .unwrap();
        let table = schema
            .tables
            .iter()
            .find(|table| table.name == ["items"])
            .unwrap();
        assert!(
            table.has_opaque_ddl && !table.indexes_complete,
            "{suffix}: {schema:?}"
        );
        assert!(!schema.diagnostics.is_empty(), "{suffix}: {schema:?}");
    }
}

#[test]
fn should_mark_duplicate_object_definitions_as_opaque() {
    for sql in [
        "CREATE TABLE items(id BIGINT); CREATE TABLE ITEMS(id TEXT);",
        "CREATE TABLE items(id BIGINT, ID TEXT);",
        "CREATE TABLE items(id BIGINT); CREATE INDEX ix ON items(id); CREATE UNIQUE INDEX ix ON items(id);",
    ] {
        let root = project(sql);
        let schema = EmbeddedProject::open_exact(root.path()).unwrap().export_table_schemas().unwrap();
        assert!(!schema.tables.is_empty());
        assert!(schema.tables.iter().all(|table| table.has_opaque_ddl && !table.indexes_complete), "{schema:?}");
        assert!(schema.diagnostics.iter().any(|message| message.contains("duplicate")), "{schema:?}");
    }
}

#[test]
fn should_export_inline_and_table_constraints_without_losing_definition_text() {
    use codeweb::embed::ConstraintKind::{Check, ForeignKey, PrimaryKey, Unique};
    let root = project(
        "CREATE TABLE items(id BIGINT PRIMARY KEY, code TEXT UNIQUE, \
        parent_id BIGINT REFERENCES parents(id), label TEXT CHECK(label <> 'O''Brien'), \
        CONSTRAINT uq_pair UNIQUE (code, parent_id) DEFERRABLE, \
        CHECK (id > 0), FOREIGN KEY (code) REFERENCES codes(code));",
    );
    let schema = EmbeddedProject::open_exact(root.path())
        .unwrap()
        .export_table_schemas()
        .unwrap();
    let table = &schema.tables[0];
    assert_eq!(
        table
            .constraints
            .iter()
            .map(|c| (c.kind, c.column.as_deref(), c.declaration_sql.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (PrimaryKey, Some("id"), "id BIGINT PRIMARY KEY"),
            (Unique, Some("code"), "code TEXT UNIQUE"),
            (
                ForeignKey,
                Some("parent_id"),
                "parent_id BIGINT REFERENCES parents(id)"
            ),
            (
                Check,
                Some("label"),
                "label TEXT CHECK(label <> 'O''Brien')"
            ),
            (
                Unique,
                None,
                "CONSTRAINT uq_pair UNIQUE (code, parent_id) DEFERRABLE"
            ),
            (Check, None, "CHECK (id > 0)"),
            (
                ForeignKey,
                None,
                "FOREIGN KEY (code) REFERENCES codes(code)"
            ),
        ]
    );
    assert!(!table.columns[0].nullable);
    assert!(!table.has_opaque_ddl);
    assert!(schema.diagnostics.is_empty());
}

#[test]
fn should_export_independent_expression_and_partial_unique_indexes() {
    let root = project(
        "CREATE TABLE public.items(id BIGINT, label TEXT);\n\
        CREATE UNIQUE INDEX public.ux ON public.items ((lower(label))) WHERE id IS NOT NULL;\n\
        CREATE INDEX public.ix ON public.items (id DESC NULLS LAST);",
    );
    let schema = EmbeddedProject::open_exact(root.path())
        .unwrap()
        .export_table_schemas()
        .unwrap();
    let table = &schema.tables[0];
    assert_eq!(table.indexes.iter().map(|index| (index.name.clone(), index.unique, index.definition_sql.as_str())).collect::<Vec<_>>(), vec![
        (Some(vec!["public".into(), "ux".into()]), true, "CREATE UNIQUE INDEX public.ux ON public.items ((lower(label))) WHERE id IS NOT NULL;"),
        (Some(vec!["public".into(), "ix".into()]), false, "CREATE INDEX public.ix ON public.items (id DESC NULLS LAST);"),
    ]);
    assert!(table.indexes_complete);
    assert!(!table.has_opaque_ddl);
    assert!(schema.diagnostics.is_empty());
}

#[test]
fn should_preserve_raw_default_expressions_and_declared_types() {
    let root = project(
        "CREATE TABLE items(label VARCHAR2(24) DEFAULT 'O''Brien, (x)' NOT NULL, \
         n NUMBER(10, 2) DEFAULT CAST('1.25' AS NUMERIC(10,2)), \
         missing BIGINT DEFAULT NULL, tick BIGINT DEFAULT nextval('seq'));",
    );
    let schema = EmbeddedProject::open_exact(root.path())
        .unwrap()
        .export_table_schemas()
        .unwrap();
    let table = &schema.tables[0];
    assert_eq!(
        table
            .columns
            .iter()
            .map(|c| (c.source_type.as_str(), c.default_expr_raw.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("VARCHAR2(24)", Some("'O''Brien, (x)'")),
            ("NUMBER(10, 2)", Some("CAST('1.25' AS NUMERIC(10,2))")),
            ("BIGINT", Some("NULL")),
            ("BIGINT", Some("nextval('seq')")),
        ]
    );
    assert!(!table.columns[0].nullable);
    assert!(!table.has_opaque_ddl);
    assert!(schema.diagnostics.is_empty());
}

#[test]
fn should_export_ordered_columns_without_loading_or_creating_graph_store() {
    let root = project("CREATE TABLE public.items(id BIGINT NOT NULL, label VARCHAR(24));");
    let project = EmbeddedProject::open_exact(root.path()).unwrap();
    let schema = project.export_table_schemas().unwrap();
    assert_eq!(schema.tables.len(), 1);
    let table = &schema.tables[0];
    assert_eq!(table.name, vec!["public", "items"]);
    assert_eq!(
        table
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.source_type.as_str(), c.nullable))
            .collect::<Vec<_>>(),
        vec![("id", "BIGINT", false), ("label", "VARCHAR(24)", true)]
    );
    assert!(table.indexes_complete);
    assert!(!table.has_opaque_ddl, "{schema:?}");
    assert!(!root.path().join(".codeweb").exists());
}
