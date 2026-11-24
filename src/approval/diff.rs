use similar::TextDiff;

/// Formats a unified diff between old and new text with standard diff headers.
pub fn format_diff(old: &str, new: &str, path: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    diff.unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_diff_addition_and_deletion() {
        let old = "line 1\nline 2\nline 3\n";
        let new = "line 1\nline 2 modified\nline 3\nline 4\n";
        let diff = format_diff(old, new, "test.txt");

        assert!(diff.contains("--- a/test.txt"));
        assert!(diff.contains("+++ b/test.txt"));
        assert!(diff.contains("-line 2"));
        assert!(diff.contains("+line 2 modified"));
        assert!(diff.contains("+line 4"));
    }

    #[test]
    fn test_format_diff_new_file() {
        let old = "";
        let new = "hello world\n";
        let diff = format_diff(old, new, "new.txt");

        assert!(diff.contains("--- a/new.txt"));
        assert!(diff.contains("+++ b/new.txt"));
        assert!(diff.contains("+hello world"));
    }
}
