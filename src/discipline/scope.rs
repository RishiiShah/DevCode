use crate::agent::tool::Tool;
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScopeState {
    pub declared_files: HashSet<PathBuf>,
    pub reason: String,
    pub has_declared: bool,
}

#[derive(Clone, Default)]
pub struct ScopeContract {
    pub state: Arc<Mutex<ScopeState>>,
}

fn normalize_scope_path(path: &Path, root: &Path) -> PathBuf {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

impl ScopeContract {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ScopeState::default())),
        }
    }

    pub fn declare(&self, files: Vec<PathBuf>, reason: String) {
        let mut st = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        st.declared_files = files.into_iter().collect();
        st.reason = reason;
        st.has_declared = true;
    }

    pub fn is_declared(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .has_declared
    }

    pub fn is_in_scope(&self, path: &Path, root: &Path) -> bool {
        let st = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !st.has_declared {
            // Before declaration, edits are not in scope
            return false;
        }

        let norm_path = normalize_scope_path(path, root);
        let rel_path = path.strip_prefix(root).unwrap_or(path);
        st.declared_files.contains(rel_path)
            || st.declared_files.contains(path)
            || st.declared_files.iter().any(|df| {
                df == rel_path || df == path || normalize_scope_path(df, root) == norm_path
            })
    }

    pub fn get_reason(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reason
            .clone()
    }
}

pub struct DeclareScopeTool {
    pub contract: ScopeContract,
}

impl DeclareScopeTool {
    pub fn new(contract: ScopeContract) -> Self {
        Self { contract }
    }
}

impl Tool for DeclareScopeTool {
    fn name(&self) -> &str {
        "declare_scope"
    }

    fn description(&self) -> &str {
        "Declares the intended file scope and reason for changes before editing files in build mode."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of relative file paths intended to be modified"
                },
                "reason": {
                    "type": "string",
                    "description": "Justification for the chosen scope"
                }
            },
            "required": ["files", "reason"]
        })
    }

    fn execute(&self, arguments: &str) -> Result<String, String> {
        let val: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| format!("invalid arguments json: {}", e))?;

        let files_val = val
            .get("files")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "missing 'files' array in arguments".to_string())?;
        let reason = val
            .get("reason")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'reason' string in arguments".to_string())?
            .to_string();
        let paths = files_val
            .iter()
            .map(|f| {
                f.as_str()
                    .map(PathBuf::from)
                    .ok_or_else(|| "files must contain only strings".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;

        self.contract.declare(paths.clone(), reason.clone());
        Ok(format!(
            "Scope declared: {} file(s) ({})",
            paths.len(),
            reason
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::ScopeContract;
    use std::path::{Path, PathBuf};

    #[test]
    fn scope_normalizes_dot_slash_and_root_prefix() {
        let root = Path::new("/tmp/project");
        let scope = ScopeContract::new();
        scope.declare(
            vec![PathBuf::from("./src/components/About.tsx")],
            "style".into(),
        );
        assert!(scope.is_in_scope(Path::new("src/components/About.tsx"), root));
        assert!(scope.is_in_scope(Path::new("/tmp/project/src/components/About.tsx"), root));
        assert!(!scope.is_in_scope(Path::new("src/components/Hero.tsx"), root));
    }
}
