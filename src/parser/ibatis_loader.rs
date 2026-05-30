use std::path::{Path, PathBuf};

use ogsql_parser::ibatis::{
    parse_mapper_bytes_structured_with_path, parse_mapper_bytes_with_path, ParsedMapper,
    StatementKind, StructuredMapper,
};

pub struct IbatisParsedFile {
    pub path: PathBuf,
    pub result: ParsedMapper,
    pub content_hash: String,
}

pub fn load_ibatis_files_from_paths(paths: &[PathBuf]) -> Vec<IbatisParsedFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| {
            load_ibatis_file(path)
                .ok()
                .map(|(result, hash)| IbatisParsedFile {
                    path: path.clone(),
                    result,
                    content_hash: hash,
                })
        })
        .collect()
}

fn load_ibatis_file(path: &Path) -> Result<(ParsedMapper, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {}", e))?;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();
    let file_path = path.to_string_lossy().to_string();
    let result = parse_mapper_bytes_with_path(&bytes, Some(&file_path));

    if result.namespace.is_empty() && result.statements.is_empty() && result.errors.is_empty() {
        crate::parse_log::info(&file_path, "skipped: not a mapper file");
        return Err("not a mapper file".to_string());
    }

    if !result.errors.is_empty() {
        for err in &result.errors {
            crate::parse_log::warn(&file_path, &err.to_string());
        }
    }

    crate::parse_log::info(
        &file_path,
        &format!(
            "namespace={}, {} statements",
            result.namespace,
            result.statements.len()
        ),
    );

    Ok((result, content_hash))
}

pub fn statement_kind_label(kind: &StatementKind) -> &'static str {
    match kind {
        StatementKind::Select => "select",
        StatementKind::Insert => "insert",
        StatementKind::Update => "update",
        StatementKind::Delete => "delete",
    }
}

pub struct IbatisStructuredFile {
    pub path: PathBuf,
    pub result: StructuredMapper,
    pub content_hash: String,
}

pub fn load_ibatis_structured_files_from_paths(paths: &[PathBuf]) -> Vec<IbatisStructuredFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| {
            load_ibatis_structured_file(path)
                .ok()
                .map(|(result, hash)| IbatisStructuredFile {
                    path: path.clone(),
                    result,
                    content_hash: hash,
                })
        })
        .collect()
}

fn load_ibatis_structured_file(path: &Path) -> Result<(StructuredMapper, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {}", e))?;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();
    let file_path = path.to_string_lossy().to_string();
    let result = parse_mapper_bytes_structured_with_path(&bytes, Some(&file_path));

    if result.namespace.is_empty() && result.statements.is_empty() && result.errors.is_empty() {
        crate::parse_log::info(&file_path, "skipped: not a mapper file (structured)");
        return Err("not a mapper file".to_string());
    }

    if !result.errors.is_empty() {
        for err in &result.errors {
            crate::parse_log::warn(&file_path, &err.to_string());
        }
    }

    crate::parse_log::info(
        &file_path,
        &format!(
            "namespace={}, {} statements (structured)",
            result.namespace,
            result.statements.len()
        ),
    );

    Ok((result, content_hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_xml(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::with_suffix(".xml").unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    const VALID_MAPPER: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<!DOCTYPE mapper PUBLIC "-//mybatis.org//DTD Mapper 3.0//EN" "http://mybatis.org/dtd/mybatis-3-mapper.dtd">
<mapper namespace="com.example.dao.OrderDao">
    <select id="findById" resultType="map">
        SELECT * FROM t_orders WHERE id = #{id}
    </select>
    <update id="createOrder">
        CALL pkg_order.create_order(#{userId}, #{productId})
    </update>
</mapper>"#;

    #[test]
    fn load_ibatis_file_valid_mapper() {
        let f = write_temp_xml(VALID_MAPPER);
        let (mapper, hash) = load_ibatis_file(f.path()).expect("valid mapper should parse");

        assert_eq!(mapper.namespace, "com.example.dao.OrderDao");
        assert_eq!(mapper.statements.len(), 2);
        assert_eq!(mapper.statements[0].id, "findById");
        assert!(matches!(mapper.statements[0].kind, StatementKind::Select));
        assert_eq!(mapper.statements[1].id, "createOrder");
        assert!(matches!(mapper.statements[1].kind, StatementKind::Update));
        assert!(!hash.is_empty());
    }

    #[test]
    fn load_ibatis_file_call_in_mapper_has_parse_result() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" ?>
<!DOCTYPE mapper PUBLIC "-//mybatis.org//DTD Mapper 3.0//EN" "http://mybatis.org/dtd/mybatis-3-mapper.dtd">
<mapper namespace="test">
    <update id="doCall">
        CALL pkg_test.run_it()
    </update>
</mapper>"#;
        let f = write_temp_xml(xml);
        let (mapper, _) = load_ibatis_file(f.path()).expect("should parse");

        let stmt = &mapper.statements[0];
        assert_eq!(stmt.id, "doCall");
        assert!(stmt.parse_result.is_some(), "CALL should have parse_result");
        let (infos, _errors) = stmt.parse_result.as_ref().unwrap();
        assert!(!infos.is_empty());
    }

    #[test]
    fn load_ibatis_structured_file_preserves_tree() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" ?>
<!DOCTYPE mapper PUBLIC "-//mybatis.org//DTD Mapper 3.0//EN" "http://mybatis.org/dtd/mybatis-3-mapper.dtd">
<mapper namespace="test">
    <select id="dynamicQuery">
        SELECT * FROM users WHERE id = #{id}
        <if test="name != null">
            AND name = #{name}
        </if>
    </select>
</mapper>"#;
        let f = write_temp_xml(xml);
        let (mapper, _) = load_ibatis_structured_file(f.path()).expect("should parse structured");

        assert_eq!(mapper.namespace, "test");
        assert_eq!(mapper.statements.len(), 1);
        assert!(
            mapper.statements[0].has_dynamic_elements,
            "<if> should set has_dynamic_elements"
        );
    }

    #[test]
    fn load_ibatis_file_non_mapper_xml_returns_no_statements() {
        // ogsql-parser may produce errors but still return Ok for non-mapper XML.
        // The contract is: no statements are extracted.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<configuration><property name="foo">bar</property></configuration>"#;
        let f = write_temp_xml(xml);
        let (mapper, _) = load_ibatis_file(f.path()).expect("should return Ok even for non-mapper");
        assert!(mapper.statements.is_empty(), "non-mapper should have zero statements");
        assert!(mapper.namespace.is_empty(), "non-mapper should have empty namespace");
    }

    #[test]
    fn load_ibatis_file_empty_file_returns_no_statements() {
        // Empty XML: ogsql-parser may produce errors but still return Ok.
        let f = write_temp_xml("");
        let result = load_ibatis_file(f.path());
        // Either rejected (Err) or accepted with no statements (Ok)
        match result {
            Ok((mapper, _)) => {
                assert!(mapper.statements.is_empty());
            }
            Err(_) => {}
        }
    }

    #[test]
    fn load_ibatis_file_nonexistent_path() {
        assert!(load_ibatis_file(Path::new("/nonexistent/path.xml")).is_err());
    }

    #[test]
    fn statement_kind_label_correct() {
        assert_eq!(statement_kind_label(&StatementKind::Select), "select");
        assert_eq!(statement_kind_label(&StatementKind::Insert), "insert");
        assert_eq!(statement_kind_label(&StatementKind::Update), "update");
        assert_eq!(statement_kind_label(&StatementKind::Delete), "delete");
    }
}
