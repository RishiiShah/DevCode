use super::{Model, ModelError, Request, ToolCall};
use std::path::{Path, PathBuf};

pub(crate) struct PrivatePaths {
    root: PathBuf,
    paths: Vec<PathBuf>,
}

impl PrivatePaths {
    pub(crate) fn new(root: &Path, paths: &[PathBuf]) -> Result<Self, ModelError> {
        let paths = paths
            .iter()
            .map(|path| crate::guard::canonicalize(root, path))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ModelError::Other(format!("invalid private path: {error}")))?;
        Ok(Self {
            root: root.to_path_buf(),
            paths,
        })
    }

    pub(crate) fn can_offer(&self, name: &str) -> bool {
        // Without OS confinement, searches and subprocesses can read arbitrary workspace files.
        matches!(
            name,
            "read_file" | "edit_file" | "write_file" | "declare_scope"
        )
    }

    pub(crate) fn check_call(&self, call: &ToolCall) -> Result<(), &'static str> {
        if !self.can_offer(&call.name) {
            return Err("tool disabled by the private-path policy");
        }
        if call.name == "declare_scope" {
            return Ok(());
        }
        let candidate = serde_json::from_str::<serde_json::Value>(&call.arguments)
            .ok()
            .and_then(|args| args.get("path")?.as_str().map(PathBuf::from))
            .and_then(|path| crate::guard::canonicalize(&self.root, &path).ok())
            .ok_or("private-path policy could not verify the tool path")?;
        if self.paths.iter().any(|path| candidate.starts_with(path)) {
            return Err("private-path policy blocked access");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if std::fs::metadata(&candidate).is_ok_and(|meta| meta.is_file() && meta.nlink() > 1) {
                return Err("private-path policy cannot verify a hard-linked file");
            }
        }
        Ok(())
    }
}

pub(crate) fn fingerprint(paths: &[PathBuf]) -> Option<String> {
    use sha2::{Digest, Sha256};
    if paths.is_empty() {
        return None;
    }
    let mut paths = paths.to_vec();
    paths.sort();
    paths.dedup();
    serde_json::to_vec(&paths)
        .ok()
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn redact_request(request: &mut Request, model: &dyn Model) {
    request.system_prompt = model.redact_sensitive(&request.system_prompt);
    for message in &mut request.messages {
        message.content = model.redact_sensitive(&message.content);
        for call in &mut message.tool_calls {
            call.arguments = model.redact_sensitive(&call.arguments);
        }
    }
}
