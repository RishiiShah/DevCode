use crate::agent::Tool;
use crate::guard::canonicalize;
use crate::tools::cap::{is_binary_content, truncate_output, TOOL_OUTPUT_CAP};
use crate::tools::ToolError;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Reads file contents with exact 1-based inclusive line slicing.
/// Paths are canonicalized against root to prevent directory traversal.
/// Binary files are reported, not dumped. Per-call output cap is enforced.
pub fn read_file(
    root: &Path,
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String, ToolError> {
    let canonical = canonicalize(root, path)?;

    let bytes = fs::read(&canonical).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ToolError::NotFound(path.to_path_buf())
        } else {
            ToolError::Io(e)
        }
    })?;

    if is_binary_content(&bytes) {
        return Ok(format!(
            "[binary file: {} bytes, path: '{}']",
            bytes.len(),
            path.display()
        ));
    }

    let content = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let start = match start_line {
        Some(0) => 1,
        Some(s) => s,
        None => 1,
    };

    let end = match end_line {
        Some(e) => e.min(total_lines),
        None => total_lines,
    };

    if start > end && total_lines > 0 {
        return Err(ToolError::InvalidRange(format!(
            "start_line ({}) cannot be greater than end_line ({})",
            start, end
        )));
    }

    let mut out = String::new();
    if total_lines > 0 && start <= total_lines {
        for line_idx in start..=end {
            let line_content = lines[line_idx - 1];
            out.push_str(&format!("{:4} | {}\n", line_idx, line_content));
        }
    }

    Ok(truncate_output(&out, TOOL_OUTPUT_CAP))
}

pub struct ReadFileTool {
    pub root: PathBuf,
}

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
}

impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Reads file contents with optional 1-based inclusive start_line and end_line bounds."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Optional 1-based starting line number (inclusive)"
                },
                "end_line": {
                    "type": "integer",
                    "description": "Optional 1-based ending line number (inclusive)"
                }
            },
            "required": ["path"]
        })
    }

    fn execute(&self, arguments: &str) -> Result<String, String> {
        let args: ReadFileArgs =
            serde_json::from_str(arguments).map_err(|e| format!("invalid arguments: {}", e))?;

        read_file(
            &self.root,
            Path::new(&args.path),
            args.start_line,
            args.end_line,
        )
        .map_err(|e| e.to_string())
    }
}
