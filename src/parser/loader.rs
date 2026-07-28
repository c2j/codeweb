use crate::error::{CodeWebError, Result};
use ogsql_parser::{StatementInfo, Tokenizer};
use std::path::{Path, PathBuf};

pub struct ParsedFile {
    pub path: PathBuf,
    pub statements: Vec<StatementInfo>,
    pub content_hash: String,
}

pub struct AllParsedFiles {
    pub sql_files: Vec<ParsedFile>,
    pub java_files: Vec<crate::parser::java_loader::JavaParsedFile>,
    pub ibatis_files: Vec<crate::parser::ibatis_loader::IbatisParsedFile>,
    pub java_method_results: Vec<crate::parser::java_method::JavaParseResult>,
    #[cfg(feature = "jsp")]
    pub jsp_files: Vec<crate::parser::jsp_loader::JspFileResult>,
}

pub fn load_all_files(input: &Path) -> Result<AllParsedFiles> {
    let scanned = crate::parser::scanner::scan_directory(input, &[]);

    let empty = scanned.sql_files.is_empty()
        && scanned.java_files.is_empty()
        && scanned.xml_files.is_empty();
    #[cfg(feature = "jsp")]
    let empty = empty && scanned.jsp_files.is_empty();
    if empty {
        return Err(CodeWebError::NoFilesFound {
            path: input.to_path_buf(),
        });
    }

    let sql_files = parse_sql_files(&scanned.sql_files);
    let java_files = crate::parser::java_loader::load_java_files_from_paths(&scanned.java_files);
    let ibatis_files =
        crate::parser::ibatis_loader::load_ibatis_files_from_paths(&scanned.xml_files);
    let java_method_results =
        crate::parser::java_method::parse_java_files_from_paths(&scanned.java_files);
    #[cfg(feature = "jsp")]
    let jsp_files = {
        use ogsql_parser::java::JavaExtractConfig;
        crate::parser::jsp_loader::load_jsp_files_from_paths(
            &scanned.jsp_files,
            &JavaExtractConfig::default(),
        )
    };

    Ok(AllParsedFiles {
        sql_files,
        java_files,
        ibatis_files,
        java_method_results,
        #[cfg(feature = "jsp")]
        jsp_files,
    })
}

pub fn load_sql_files(input: &Path) -> Result<Vec<ParsedFile>> {
    let scanned = crate::parser::scanner::scan_directory(input, &[]);
    if scanned.sql_files.is_empty() {
        return Err(CodeWebError::NoFilesFound {
            path: input.to_path_buf(),
        });
    }
    Ok(parse_sql_files(&scanned.sql_files))
}

pub fn parse_sql_files(paths: &[PathBuf]) -> Vec<ParsedFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| {
            parse_file(path).ok().map(|(statements, hash)| ParsedFile {
                path: path.clone(),
                statements,
                content_hash: hash,
            })
        })
        .collect()
}

fn decode_sql_bytes(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            let bytes = e.into_bytes();
            if let Ok(s) = decode_gbk(&bytes) {
                return s;
            }
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }
}

fn decode_gbk(bytes: &[u8]) -> std::result::Result<String, ()> {
    match encoding_rs::GBK.decode_without_bom_handling_and_without_replacement(bytes) {
        Some(cow) => Ok(cow.into_owned()),
        None => Err(()),
    }
}

fn parse_file(path: &Path) -> std::result::Result<(Vec<StatementInfo>, String), String> {
    let parse_sw = std::time::Instant::now();
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {}", e))?;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();
    let sql = decode_sql_bytes(bytes);

    let tokens = Tokenizer::new(&sql)
        .tokenize()
        .map_err(|e| format!("tokenize error: {}", e))?;

    let mut parser = ogsql_parser::Parser::with_source(tokens, sql);
    let stmts = parser.parse_with_text();

    let file_str = path.to_string_lossy().to_string();
    let elapsed = parse_sw.elapsed();
    if elapsed > crate::parse_log::SLOW_FILE_THRESHOLD {
        crate::parse_log::warn(
            &file_str,
            &format!(
                "slow parse: {:.2}s ({} statements) — inspect for pathological nesting / huge \
                 statements",
                elapsed.as_secs_f64(),
                stmts.len()
            ),
        );
    }
    if !parser.errors().is_empty() {
        for err in parser.errors() {
            crate::parse_log::warn(&file_str, &err.to_string());
        }
    }
    crate::parse_log::info(&file_str, &format!("{} statements parsed", stmts.len()));

    Ok((stmts, content_hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_sql(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::with_suffix(".sql").unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn parse_file_create_procedure() {
        let sql = r#"CREATE OR REPLACE PROCEDURE pkg_users.get_user(p_id IN INTEGER)
AS
BEGIN
    SELECT * FROM t_users WHERE id = p_id;
END;
/"#;
        let f = write_temp_sql(sql);
        let (stmts, hash) = parse_file(f.path()).expect("should parse");

        assert!(!stmts.is_empty(), "should find statements");
        assert!(!hash.is_empty(), "should have content hash");
    }

    #[test]
    fn parse_file_call_statement() {
        let sql = r#"CREATE OR REPLACE PROCEDURE pkg_orders.process_order(p_id INTEGER)
AS
BEGIN
    CALL pkg_inventory.check_stock(p_id);
END;
/"#;
        let f = write_temp_sql(sql);
        let (stmts, _) = parse_file(f.path()).expect("should parse");

        assert!(!stmts.is_empty(), "should find the CREATE PROCEDURE");
    }

    #[test]
    fn parse_file_multiple_statements() {
        let sql = r#"CREATE TABLE t_users (id INTEGER PRIMARY KEY, name VARCHAR(100));

CREATE OR REPLACE PROCEDURE pkg_users.list_users()
AS
BEGIN
    SELECT * FROM t_users;
END;
/"#;
        let f = write_temp_sql(sql);
        let (stmts, _) = parse_file(f.path()).expect("should parse");

        assert!(
            stmts.len() >= 2,
            "should find at least 2 statements, got {}",
            stmts.len()
        );
    }

    #[test]
    fn parse_file_whitespace_only() {
        let f = write_temp_sql("   \n\n  \t  \n");
        let result = parse_file(f.path());
        assert!(result.is_ok(), "whitespace-only SQL should not panic");
    }

    #[test]
    fn parse_file_nonexistent_path() {
        assert!(
            parse_file(Path::new("/nonexistent/file.sql")).is_err(),
            "nonexistent file should return Err"
        );
    }

    #[test]
    fn parse_file_invalid_encoding_no_panic() {
        let mut f = tempfile::NamedTempFile::with_suffix(".sql").unwrap();
        // Write invalid UTF-8 bytes — from_utf8_lossy handles this
        f.write_all(&[0xFF, 0xFE, 0x00, 0x00]).unwrap();
        f.flush().unwrap();
        // Must not panic; Ok or Err both acceptable
        let _ = parse_file(f.path());
    }

    #[test]
    fn parse_sql_files_batch() {
        let f1 = write_temp_sql("CREATE PROCEDURE proc1() AS BEGIN NULL; END; /");
        let f2 = write_temp_sql("CREATE PROCEDURE proc2() AS BEGIN NULL; END; /");

        let results = parse_sql_files(&[f1.path().to_path_buf(), f2.path().to_path_buf()]);
        assert_eq!(results.len(), 2, "should parse both files");

        for pf in &results {
            assert!(
                !pf.statements.is_empty(),
                "each file should have statements"
            );
        }
    }

    #[test]
    fn parse_sql_files_skips_unparseable() {
        let valid = write_temp_sql("CREATE PROCEDURE proc1() AS BEGIN NULL; END; /");

        let results = parse_sql_files(&[
            valid.path().to_path_buf(),
            PathBuf::from("/nonexistent/file.sql"),
        ]);
        assert_eq!(results.len(), 1, "unparseable file should be filtered out");
    }

    #[test]
    fn decode_utf8_sql() {
        let s = decode_sql_bytes("CREATE TABLE t (id INTEGER)".as_bytes().to_vec());
        assert_eq!(s, "CREATE TABLE t (id INTEGER)");
    }

    #[test]
    fn decode_utf8_chinese() {
        let s = decode_sql_bytes("COMMENT ON COLUMN t.id IS '客户ID'".as_bytes().to_vec());
        assert_eq!(s, "COMMENT ON COLUMN t.id IS '客户ID'");
    }

    #[test]
    fn decode_gbk_chinese() {
        // "客户ID" in GBK: BF CD BB A7 49 44
        let gbk_bytes: Vec<u8> = b"COMMENT ON COLUMN t.id IS '"
            .iter()
            .copied()
            .chain([0xBF, 0xCD, 0xBB, 0xA7, 0x49, 0x44])
            .chain(b"'".iter().copied())
            .collect();
        let s = decode_sql_bytes(gbk_bytes);
        assert_eq!(s, "COMMENT ON COLUMN t.id IS '客户ID'");
    }

    #[test]
    fn decode_lossy_fallback() {
        // Invalid bytes that are neither UTF-8 nor GBK
        let bad: Vec<u8> = b"SELECT ".iter().copied().chain([0xFF, 0xFE]).collect();
        let s = decode_sql_bytes(bad);
        assert!(s.contains("SELECT"), "ASCII prefix preserved");
        assert!(
            s.contains('\u{FFFD}'),
            "invalid bytes replaced with replacement char"
        );
    }
}
