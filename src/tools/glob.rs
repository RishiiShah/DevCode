use crate::agent::Tool;
use crate::tools::cap::{truncate_output, TOOL_OUTPUT_CAP};
use crate::tools::ToolError;
use ignore::WalkBuilder;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Enumerates workspace files matching the glob pattern, respecting .gitignore.
pub fn glob_paths(root: &Path, pattern: &str) -> Result<Vec<PathBuf>, ToolError> {
    let glob = globset::Glob::new(pattern).map_err(|e| ToolError::Glob(e.to_string()))?;
    let matcher = glob.compile_matcher();

    let mut builder = WalkBuilder::new(root);
    builder.hidden(false);
    builder.git_ignore(true);
    builder.git_global(true);
    builder.git_exclude(true);
    builder.require_git(false);

    let mut matches = Vec::new();

    for result in builder.build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }

        let path = entry.path();
        let rel_path = match path.strip_prefix(root) {
            Ok(r) => r,
            Err(_) => continue,
        };

        if rel_path.as_os_str().is_empty() {
            continue;
        }

        // Exclude .devcode/ and .git/
        let is_excluded = rel_path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s.eq_ignore_ascii_case(".devcode") || s == ".git"
        });
        if is_excluded {
            continue;
        }

        if matcher.is_match(rel_path) {
            matches.push(rel_path.to_path_buf());
        }
    }

    matches.sort();
    Ok(matches)
}

pub struct GlobTool {
    pub root: PathBuf,
}

#[derive(Deserialize)]
struct GlobArgs {
    pattern: String,
}

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Finds files in the workspace matching a glob pattern (e.g. '**/*.rs', 'src/**/*.ts'). Respects .gitignore."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match file paths relative to root"
                }
            },
            "required": ["pattern"]
        })
    }

    fn execute(&self, arguments: &str) -> Result<String, String> {
        let args: GlobArgs =
            serde_json::from_str(arguments).map_err(|e| format!("invalid arguments: {}", e))?;

        let paths = glob_paths(&self.root, &args.pattern).map_err(|e| e.to_string())?;
        if paths.is_empty() {
            return Ok("No files matched pattern.".to_string());
        }

        let mut out = String::new();
        for p in paths {
            out.push_str(&p.display().to_string());
            out.push('\n');
        }

        Ok(truncate_output(&out, TOOL_OUTPUT_CAP))
    }
}
