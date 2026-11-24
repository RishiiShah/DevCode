use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeClass {
    ClassA,
    ClassC,
}

/// Classifies a set of file diffs.
/// Class C only when the change is whitespace-only (equal after stripping whitespace).
/// Everything else is Class A. Class B is empty.
pub fn classify_diff(modified_files: &[(PathBuf, String, String)]) -> ChangeClass {
    if modified_files.is_empty() {
        return ChangeClass::ClassC;
    }

    let is_all_whitespace = modified_files.iter().all(|(_, old_text, new_text)| {
        let old_clean: String = old_text.chars().filter(|c| !c.is_whitespace()).collect();
        let new_clean: String = new_text.chars().filter(|c| !c.is_whitespace()).collect();
        old_clean == new_clean
    });

    if is_all_whitespace {
        ChangeClass::ClassC
    } else {
        ChangeClass::ClassA
    }
}
