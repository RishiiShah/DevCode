use crate::agent::Tool;
use crate::guard::canonicalize;
use crate::tools::cap::{is_binary_content, truncate_output, TOOL_OUTPUT_CAP};
use crate::tools::ToolError;
use ignore::WalkBuilder;
use regex::Regex;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_MAX_GREP_RESULTS: usize = 100;

/// Searches file contents for regex pattern, respecting .gitignore and checking workspace containment.
pub fn grep_search(
    root: &Path,
    pattern: &str,
    subpath: Option<&Path>,
    max_results: Option<usize>,
) -> Result<String, ToolError> {
    let re = Regex::new(pattern).map_err(ToolError::Regex)?;
    let limit = max_results.unwrap_or(DEFAULT_MAX_GREP_RESULTS);

    let search_dir = match subpath {
        Some(p) => canonicalize(root, p)?,
        None => root.to_path_buf(),
    };

    let mut builder = WalkBuilder::new(&search_dir);
    builder.hidden(false);
    builder.git_ignore(true);
    builder.git_global(true);
    builder.git_exclude(true);
    builder.require_git(false);

    let mut output = String::new();
    let mut total_matches = 0;

    for result in builder.build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }

        let full_path = entry.path();
        let rel_path = match full_path.strip_prefix(root) {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Skip .devcode and .git
        let is_excluded = rel_path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s.eq_ignore_ascii_case(".devcode") || s == ".git"
        });
        if is_excluded {
            continue;
        }

        let bytes = match fs::read(full_path) {
            Ok(b) => b,
            Err(_) => continue,
        };

        if is_binary_content(&bytes) {
            // Binary files are skipped or reported, not dumped
            continue;
        }

        let content = String::from_utf8_lossy(&bytes);
        for (idx, line) in content.lines().enumerate() {
            if re.is_match(line) {
                let line_num = idx + 1;
                output.push_str(&format!("{}:{}: {}\n", rel_path.display(), line_num, line));
                total_matches += 1;
                if total_matches >= limit {
                    output.push_str(&format!("\n[Reached maximum results limit of {}]\n", limit));
                    return Ok(truncate_output(&output, TOOL_OUTPUT_CAP));
                }
            }
        }
    }

    if output.is_empty() {
        return Ok("No matches found.".to_string());
    }

    Ok(truncate_output(&output, TOOL_OUTPUT_CAP))
}

pub struct GrepTool {
    pub root: PathBuf,
}

#[derive(Deserialize)]
struct GrepArgs {
    pattern: String,
    path: Option<String>,
    max_results: Option<usize>,
}

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Searches file contents using a regular expression pattern. Respects .gitignore."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Optional subdirectory or file path to restrict search within"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Optional maximum number of matching lines to return"
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute(&self, arguments: &str) -> Result<String, String> {
        let args: GrepArgs =
            serde_json::from_str(arguments).map_err(|e| format!("invalid arguments: {}", e))?;

        let sub = args.path.as_deref().map(Path::new);
        grep_search(&self.root, &args.pattern, sub, args.max_results).map_err(|e| e.to_string())
    }
}
