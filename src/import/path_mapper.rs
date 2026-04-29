use std::path::PathBuf;

/// Maps relative file paths from CGEF documents to local paths.
/// If a prefix is provided, prepends it to all relative paths.
#[derive(Debug, Clone)]
pub struct PathMapper {
    prefix: Option<PathBuf>,
}

impl PathMapper {
    /// Create a new PathMapper.
    /// - `prefix`: Optional path prefix (e.g., "/enterprise/module-a").
    ///   Trailing slashes are normalized away.
    pub fn new(prefix: Option<&str>) -> Self {
        let prefix = prefix.map(|p| {
            let trimmed = p.trim_end_matches('/');
            PathBuf::from(trimmed)
        });
        Self { prefix }
    }

    /// Map a relative path from a CGEF document to a local path.
    /// - If prefix is set: prefix + relative_path
    /// - If no prefix: returns the relative path as-is
    /// - Normalizes `./` prefixes and redundant separators
    pub fn map(&self, relative_path: &str) -> PathBuf {
        let cleaned = relative_path.strip_prefix("./").unwrap_or(relative_path);

        match &self.prefix {
            Some(prefix) => prefix.join(cleaned),
            None => PathBuf::from(cleaned),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_prefix() {
        let mapper = PathMapper::new(Some("/enterprise/module-a"));
        let result = mapper.map("sql/pkg_order.sql");
        assert_eq!(
            result,
            PathBuf::from("/enterprise/module-a/sql/pkg_order.sql")
        );
    }

    #[test]
    fn test_without_prefix() {
        let mapper = PathMapper::new(None);
        let result = mapper.map("sql/pkg_order.sql");
        assert_eq!(result, PathBuf::from("sql/pkg_order.sql"));
    }

    #[test]
    fn test_strip_dot_slash() {
        let mapper = PathMapper::new(Some("/a"));
        let result = mapper.map("./sql/pkg.sql");
        assert_eq!(result, PathBuf::from("/a/sql/pkg.sql"));
    }

    #[test]
    fn test_trailing_slash_prefix() {
        let mapper = PathMapper::new(Some("/a/"));
        let result = mapper.map("sql/pkg.sql");
        assert_eq!(result, PathBuf::from("/a/sql/pkg.sql"));
    }

    #[test]
    fn test_empty_path() {
        let mapper = PathMapper::new(Some("/prefix"));
        let result = mapper.map("");
        assert_eq!(result, PathBuf::from("/prefix"));
    }

    #[test]
    fn test_deeply_nested_path() {
        let mapper = PathMapper::new(Some("/root"));
        let result = mapper.map("a/b/c/d/file.sql");
        assert_eq!(result, PathBuf::from("/root/a/b/c/d/file.sql"));
    }

    #[test]
    fn test_no_prefix_with_dot_slash() {
        let mapper = PathMapper::new(None);
        let result = mapper.map("./sql/pkg.sql");
        assert_eq!(result, PathBuf::from("sql/pkg.sql"));
    }
}
