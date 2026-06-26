use std::path::Path;

/// Read line `line` (1-based) of `file` with `context` lines on each side, returning
/// an annotated snippet. Target line is prefixed `>`; others with a space. Format:
/// ```text
///    41 |   IF :new.id IS NOT NULL THEN
/// >  42 |     PKG_UTIL.LOG_ORDER(:new.id);
///    43 | END IF;
/// ```
/// Returns `None` if the file is unreadable, `line == 0`, or `line` exceeds the file's
/// line count. Never panics.
pub fn read_snippet(file: &Path, line: usize, context: usize) -> Option<String> {
    if line == 0 {
        return None;
    }
    let content = std::fs::read_to_string(file).ok()?;
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    if line > lines.len() {
        return None;
    }
    let first = line.saturating_sub(context).max(1);
    let last = (line + context).min(lines.len());

    let mut out = String::new();
    for n in first..=last {
        let marker = if n == line { '>' } else { ' ' };
        let raw = lines[n - 1];
        let stripped = raw.strip_suffix('\r').unwrap_or(raw);
        out.push_str(&format!("{}{:>4} | {}\n", marker, n, stripped));
    }
    out.pop();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_file(name: &str, contents: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "cobweb_snippet_test_{}_{}",
            std::process::id(),
            name
        ));
        fs::write(&path, contents).expect("write temp file");
        path
    }

    #[test]
    fn normal_returns_target_plus_context() {
        let path = temp_file(
            "normal",
            "line one\nline two\nline three\nline four\nline five\n",
        );
        let snippet = read_snippet(&path, 3, 1).expect("should return a snippet");
        let lines: Vec<&str> = snippet.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(
            lines[1].starts_with('>'),
            "target marked with '>'; got: {}",
            lines[1]
        );
        assert!(!lines[0].starts_with('>'));
        assert!(lines[1].contains("line three"));
        assert!(lines[0].contains('2'));
        assert!(lines[1].contains('3'));
        assert!(lines[2].contains('4'));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn line_at_start_clamps_context() {
        let path = temp_file("start", "a\nb\nc\n");
        let snippet = read_snippet(&path, 1, 5).expect("snippet for line 1");
        let lines: Vec<&str> = snippet.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with('>'));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn line_beyond_end_returns_none() {
        let path = temp_file("past_end", "only\n two\nlines\n");
        assert_eq!(read_snippet(&path, 10, 1), None);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn line_zero_returns_none() {
        let path = temp_file("zero", "x\n");
        assert_eq!(read_snippet(&path, 0, 1), None);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn missing_file_returns_none() {
        let mut bogus = std::env::temp_dir();
        bogus.push("cobweb_snippet_does_not_exist_xyz");
        assert_eq!(read_snippet(&bogus, 1, 1), None);
    }

    #[test]
    fn context_zero_returns_only_target() {
        let path = temp_file("ctx0", "one\ntwo\nthree\n");
        let snippet = read_snippet(&path, 2, 0).expect("snippet for line 2");
        let lines: Vec<&str> = snippet.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with('>'));
        assert!(lines[0].contains("two"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn blank_line_in_middle_is_preserved_once() {
        let path = temp_file("blank", "keep\n\nafter\n");
        let snippet = read_snippet(&path, 2, 1).expect("snippet");
        let lines: Vec<&str> = snippet.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with('>'));
        let _ = fs::remove_file(&path);
    }
}
