use similar::{ChangeTag, TextDiff};
use std::io::{BufRead, Write};
use std::path::Path;

/// Returns true if `path` matches any recognized dependency manifest:
/// Cargo.toml, package.json, pyproject.toml, go.mod, or requirements*.txt.
pub fn is_dependency_manifest(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if name == "cargo.toml"
        || name == "package.json"
        || name == "pyproject.toml"
        || name == "go.mod"
    {
        return true;
    }

    if name.starts_with("requirements") && name.ends_with(".txt") {
        return true;
    }

    false
}

/// Detects if `new_content` introduces a new dependency addition compared to `old_content`.
pub fn has_dependency_addition(path: &Path, old_content: &str, new_content: &str) -> bool {
    if !is_dependency_manifest(path) {
        return false;
    }

    let diff = TextDiff::from_lines(old_content, new_content);
    let mut added_lines = Vec::new();

    for change in diff.iter_all_changes() {
        if change.tag() == ChangeTag::Insert {
            let line = change.value().trim();
            if !line.is_empty() && !line.starts_with('#') && !line.starts_with("//") {
                added_lines.push(line.to_string());
            }
        }
    }

    if added_lines.is_empty() {
        return false;
    }

    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if name == "cargo.toml" {
        // Find added lines that reside within [dependencies], [dev-dependencies], [build-dependencies], or [target.*.dependencies]
        let mut in_dep_section = false;
        for line in new_content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_dep_section = trimmed.contains("dependencies");
                continue;
            }
            if in_dep_section && trimmed.contains('=') && added_lines.iter().any(|al| al == trimmed)
            {
                return true;
            }
        }
        false
    } else if name == "package.json" {
        let mut in_dep_section = false;
        for line in new_content.lines() {
            let trimmed = line.trim();
            if trimmed.contains("\"dependencies\"") || trimmed.contains("\"devDependencies\"") {
                in_dep_section = true;
                continue;
            }
            if in_dep_section {
                if trimmed.starts_with('}') {
                    in_dep_section = false;
                } else if trimmed.contains(':') && added_lines.iter().any(|al| al == trimmed) {
                    return true;
                }
            }
        }
        false
    } else if name == "pyproject.toml" {
        let mut in_dep_section = false;
        for line in new_content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_dep_section = trimmed.contains("dependencies");
                continue;
            }
            if trimmed.starts_with("dependencies = [") {
                in_dep_section = true;
                continue;
            }
            if in_dep_section {
                if trimmed == "]" {
                    in_dep_section = false;
                } else if (trimmed.contains('=') || trimmed.contains('"') || trimmed.contains('\''))
                    && added_lines.iter().any(|al| al == trimmed)
                {
                    return true;
                }
            }
        }
        false
    } else if name == "go.mod" {
        let mut in_require_block = false;
        for line in new_content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("require (") {
                in_require_block = true;
                continue;
            }
            if in_require_block {
                if trimmed == ")" {
                    in_require_block = false;
                } else if trimmed.contains('/') && added_lines.iter().any(|al| al == trimmed) {
                    return true;
                }
            } else if trimmed.starts_with("require ") && added_lines.iter().any(|al| al == trimmed)
            {
                return true;
            }
        }
        false
    } else {
        // requirements*.txt: any non-empty, non-comment added line is a dependency
        !added_lines.is_empty()
    }
}

/// Prompts the user for approval and a one-line justification for a dependency addition.
pub fn prompt_dependency_addition<R: BufRead, W: Write>(
    path: &str,
    reader: &mut R,
    writer: &mut W,
    is_tty: bool,
) -> Result<Option<String>, std::io::Error> {
    if !is_tty {
        writeln!(writer, "Dependency addition to '{}' rejected: no TTY", path)?;
        return Ok(None);
    }

    write!(
        writer,
        "Dependency addition detected in '{}'. Enter justification to approve: ",
        path
    )?;
    writer.flush()?;

    let mut input = String::new();
    if reader.read_line(&mut input)? == 0 {
        return Ok(None);
    }

    let trimmed = input.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("n")
        || trimmed.eq_ignore_ascii_case("no")
        || trimmed.eq_ignore_ascii_case("reject")
    {
        return Ok(None);
    }

    Ok(Some(trimmed.to_string()))
}
