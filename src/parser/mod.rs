mod extractor;
pub mod fingerprint;
#[cfg(feature = "jsp")]
pub mod ibatis_loader;
#[cfg(feature = "java")]
pub mod java_loader;
#[cfg(feature = "java")]
pub mod java_method;
#[cfg(feature = "jsp")]
pub mod jsp_loader;
#[cfg(feature = "jsp")]
pub mod jsp_preprocessor;
#[cfg(feature = "jsp")]
pub mod jsp_types;
mod loader;
mod predicates;
mod routine;
pub mod scanner;
pub mod snippet;

#[allow(unused_imports)]
pub use extractor::{
    anchor_from_pl_data_type, anchor_targets_in_pl_type_decl, extract_body_sql,
    parse_anchor_from_type_string, pl_type_decl_name, AnchorExtractor, AnchorKind, AnchorRef,
    AnchorSite, CallEdge, CallExtractor, ColumnAccessExtractor, ColumnAnalysis, ColumnContext,
    ColumnMapping, ColumnRef, ColumnSource, CrossTableEquality, CursorColumn, EnumMapping,
    EqualitySide, FilterOperator, FilterTransform, FilterValue, HardFilter, InsertColumnInfo,
    JoinCondition, JoinConditionSource, JoinType, MappingKind, ProcedureBodySql,
    ProcedureSqlExtractor, ProcedureVarContext, SelectIntoMapping, SequenceRef, SequenceRefVia,
    TableAccessExtractor, TableAlias, TypeRef, TypeSequenceRefExtractor, UpdateColumnInfo,
};
#[cfg_attr(not(feature = "jsp"), allow(unused_imports))]
pub use ibatis_loader::{
    load_ibatis_files_from_paths, load_ibatis_structured_files_from_paths, IbatisParsedFile,
    IbatisStructuredFile,
};
#[cfg(feature = "java")]
#[allow(unused_imports)]
pub use java_loader::{
    load_java_files_combined, load_java_files_from_paths, JavaCombinedResult, JavaParsedFile,
};
#[cfg(feature = "java")]
#[allow(unused_imports)]
pub use java_method::{
    parse_java_file, parse_java_files_from_paths, parse_java_source, JavaClassInfo, JavaMethodInfo,
    JavaParseResult, MethodCallInfo,
};

// ── feature 关闭时的占位类型 ────────────────────────────────────────────────
// 无 `jsp` / `java` 的构建（如 SpinDuck 嵌入形态）仍要能编译 AllParsedFiles 与
// GraphBuilder 的签名；下面的占位模块提供同名类型与空实现，不承载解析行为、
// 不进入任何图。feature 开启时这些模块不存在（与上方真实模块互斥）。
#[cfg(not(feature = "jsp"))]
pub mod ibatis_loader {
    use std::path::PathBuf;

    /// 占位：真实类型见 feature 开启时的 `parser::ibatis_loader`。
    #[derive(Debug, Clone)]
    pub struct IbatisParsedFile;
    #[derive(Debug, Clone)]
    pub struct IbatisStructuredFile;

    pub fn load_ibatis_files_from_paths(_paths: &[PathBuf]) -> Vec<IbatisParsedFile> {
        Vec::new()
    }
    pub fn load_ibatis_structured_files_from_paths(
        _paths: &[PathBuf],
    ) -> Vec<IbatisStructuredFile> {
        Vec::new()
    }
}
#[cfg(not(feature = "java"))]
pub mod java_loader {
    use std::path::PathBuf;

    /// 占位：真实类型见 feature 开启时的 `parser::java_loader`。
    #[derive(Debug, Clone)]
    pub struct JavaParsedFile;
    #[derive(Debug, Clone)]
    pub struct JavaCombinedResult;

    pub fn load_java_files_from_paths(_paths: &[PathBuf]) -> Vec<JavaParsedFile> {
        Vec::new()
    }
    pub fn load_java_files_combined(_paths: &[PathBuf]) -> Vec<JavaCombinedResult> {
        Vec::new()
    }
}
#[cfg(not(feature = "java"))]
pub mod java_method {
    use std::path::PathBuf;

    /// 占位：真实类型见 feature 开启时的 `parser::java_method`。
    #[derive(Debug, Clone)]
    pub struct JavaParseResult;
    #[derive(Debug, Clone)]
    pub struct JavaClassInfo;
    #[derive(Debug, Clone)]
    pub struct JavaMethodInfo;
    #[derive(Debug, Clone)]
    pub struct MethodCallInfo;

    pub fn parse_java_files_from_paths(_paths: &[PathBuf]) -> Vec<JavaParseResult> {
        Vec::new()
    }
}
pub use loader::{load_all_files, load_sql_files, parse_sql_files, AllParsedFiles, ParsedFile};
#[allow(unused_imports)]
pub use predicates::{
    extract_predicates, Confidence, ParamTableHint, PlPredicate, PredicateClause,
    PredicateExtractor, PredicateKind, TablePredicate,
};
#[allow(unused_imports)]
pub use routine::RoutineParameter;
#[allow(unused_imports)]
pub use scanner::{build_exclude_matcher, scan_directory, ScannedFiles};
