pub mod cap;
pub mod glob;
pub mod grep;
pub mod mutate;
pub mod read;
pub use mutate::{new_session_id, registry, registry_with_context, MutationContext};

pub use cap::{truncate_output, TOOL_OUTPUT_CAP};
pub use glob::{glob_paths, GlobTool};
pub use grep::{grep_search, GrepTool};
pub use read::{read_file, ReadFileTool};

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ToolError {
    Guard(crate::guard::GuardError),
    Io(std::io::Error),
    Regex(regex::Error),
    Glob(String),
    Json(serde_json::Error),
    NotFound(PathBuf),
    InvalidRange(String),
    BinaryFile(PathBuf),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::Guard(e) => write!(f, "{}", e),
            ToolError::Io(e) => write!(f, "I/O error: {}", e),
            ToolError::Regex(e) => write!(f, "invalid regex pattern: {}", e),
            ToolError::Glob(msg) => write!(f, "invalid glob pattern: {}", msg),
            ToolError::Json(e) => write!(f, "invalid tool arguments JSON: {}", e),
            ToolError::NotFound(p) => write!(f, "file not found: '{}'", p.display()),
            ToolError::InvalidRange(msg) => write!(f, "invalid line range: {}", msg),
            ToolError::BinaryFile(p) => write!(f, "binary file detected: '{}'", p.display()),
        }
    }
}

impl std::error::Error for ToolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ToolError::Guard(e) => Some(e),
            ToolError::Io(e) => Some(e),
            ToolError::Regex(e) => Some(e),
            ToolError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<crate::guard::GuardError> for ToolError {
    fn from(err: crate::guard::GuardError) -> Self {
        ToolError::Guard(err)
    }
}
