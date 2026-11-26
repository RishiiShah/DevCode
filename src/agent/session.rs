use crate::agent::sse::sleep_cancellable;
use crate::agent::{CancelToken, Model, ModelError, Request, Response, StreamChunk, ToolRegistry};
use crate::gate::{ToolKind, ToolLog};
use crate::manifest::record::{load_manifest, PathRecord, StepManifest};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct SessionState {
    pub log: ToolLog,
    pub manifest: Option<StepManifest>,
    pub changes: Vec<(PathBuf, String, String)>,
}

pub fn session_state(tools: &ToolRegistry) -> Result<SessionState, ModelError> {
    let Some(shared) = &tools.context else {
        return Ok(SessionState {
            log: ToolLog::new(),
            manifest: None,
            changes: Vec::new(),
        });
    };
    let ctx = shared
        .lock()
        .map_err(|e| ModelError::Other(e.to_string()))?;
    let mut pre: BTreeMap<PathBuf, PathRecord> = BTreeMap::new();
    let mut manifest = None;
    for step in 1..=ctx.step {
        match load_manifest(&ctx.root, &ctx.session, step) {
            Ok(m) => {
                if m.post_state.is_none() {
                    return Err(ModelError::Other(format!(
                        "incomplete manifest step {}",
                        m.step_id
                    )));
                }
                for record in &m.pre_state {
                    pre.entry(record.path.clone())
                        .or_insert_with(|| record.clone());
                }
                manifest = Some(m);
            }
            Err(crate::manifest::ManifestError::ManifestNotFound { .. }) => continue,
            Err(e) => return Err(ModelError::Other(e.to_string())),
        }
    }
    let mut changes = Vec::new();
    for (path, record) in pre {
        let old = match record.blob_ref {
            Some(hash) => match crate::manifest::read_blob(&ctx.root, &hash) {
                Ok(bytes) => bytes,
                // A snapshot deleted from `.devcode/blobs` (for example by `rm -rf`) must not
                // fail every later step; that path just has no diff to show or restore.
                Err(crate::manifest::ManifestError::BlobNotFound { .. }) => continue,
                Err(e) => return Err(ModelError::Other(e.to_string())),
            },
            None => Vec::new(),
        };
        let new = match std::fs::read(ctx.root.join(&path)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(ModelError::Other(e.to_string())),
        };
        if old != new {
            changes.push((
                path,
                String::from_utf8_lossy(&old).into_owned(),
                String::from_utf8_lossy(&new).into_owned(),
            ));
        }
    }
    Ok(SessionState {
        log: ctx.log.clone(),
        manifest,
        changes,
    })
}

pub fn summary_counts(log: &ToolLog) -> (usize, usize, usize, usize) {
    let files = log
        .entries
        .iter()
        .filter(|e| e.kind == ToolKind::FileEdit)
        .map(|e| &e.detail)
        .collect::<std::collections::HashSet<_>>()
        .len();
    let deps = log
        .entries
        .iter()
        .filter(|e| e.kind == ToolKind::DependencyJustification)
        .count();
    let outside = log
        .entries
        .iter()
        .filter(|e| e.kind == ToolKind::OutsideScope)
        .map(|e| &e.detail)
        .collect::<std::collections::HashSet<_>>()
        .len();
    let command_files = log
        .entries
        .iter()
        .filter(|e| e.kind == ToolKind::CommandFileChange)
        .map(|e| &e.detail)
        .collect::<std::collections::HashSet<_>>()
        .len();
    (files, command_files, outside, deps)
}

pub struct RateLimitedModel {
    model: Box<dyn Model>,
    interval: Duration,
    last: Mutex<Option<Instant>>,
}
impl RateLimitedModel {
    pub fn new(model: Box<dyn Model>, interval: Duration) -> Self {
        Self {
            model,
            interval,
            last: Mutex::new(None),
        }
    }
}
impl Model for RateLimitedModel {
    fn redact_sensitive(&self, text: &str) -> String {
        self.model.redact_sensitive(text)
    }
    fn list_models(&self) -> Result<Vec<String>, ModelError> {
        self.model.list_models()
    }
    fn complete(&self, request: &Request) -> Result<Response, ModelError> {
        self.complete_cancellable(request, None)
    }
    fn complete_cancellable(
        &self,
        request: &Request,
        cancel: Option<&CancelToken>,
    ) -> Result<Response, ModelError> {
        let wait = {
            let mut last = self
                .last
                .lock()
                .map_err(|e| ModelError::Other(e.to_string()))?;
            let wait = last
                .map(|previous| self.interval.saturating_sub(previous.elapsed()))
                .unwrap_or(Duration::ZERO);
            *last = Some(Instant::now());
            wait
        };
        sleep_cancellable(cancel, wait)?;
        self.model.complete_cancellable(request, cancel)
    }
    fn complete_stream(
        &self,
        request: &Request,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<Response, ModelError> {
        self.complete_stream_cancellable(request, None, &mut |chunk| {
            if let StreamChunk::Text(piece) = chunk {
                on_delta(piece);
            }
        })
    }
    fn complete_stream_cancellable(
        &self,
        request: &Request,
        cancel: Option<&CancelToken>,
        on_chunk: &mut dyn FnMut(StreamChunk<'_>),
    ) -> Result<Response, ModelError> {
        let wait = {
            let mut last = self
                .last
                .lock()
                .map_err(|e| ModelError::Other(e.to_string()))?;
            let wait = last
                .map(|previous| self.interval.saturating_sub(previous.elapsed()))
                .unwrap_or(Duration::ZERO);
            *last = Some(Instant::now());
            wait
        };
        sleep_cancellable(cancel, wait)?;
        self.model
            .complete_stream_cancellable(request, cancel, on_chunk)
    }
}

/// Hashes tracked and non-ignored workspace paths without reading .env files.
pub fn workspace_hash(root: &std::path::Path) -> Result<String, ModelError> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let mut paths = Vec::new();
    let mut index_state = Vec::new();
    if root.join(".git").exists() {
        let result = std::process::Command::new("git")
            .current_dir(root)
            .args([
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ])
            .output()
            .map_err(|e| ModelError::Other(e.to_string()))?;
        if !result.status.success() {
            return Err(ModelError::Other("failed to list workspace files".into()));
        }
        let index = std::process::Command::new("git")
            .current_dir(root)
            .args(["ls-files", "--stage", "-z"])
            .output()
            .map_err(|e| ModelError::Other(e.to_string()))?;
        if !index.status.success() {
            return Err(ModelError::Other(
                "failed to inspect workspace index".into(),
            ));
        }
        index_state = index
            .stdout
            .split(|b| *b == 0)
            .filter(|row| {
                !row.split(|b| *b == b'\t')
                    .next_back()
                    .is_some_and(|name| name.split(|b| *b == b'/').any(|part| part == b".env"))
            })
            .flat_map(|row| row.iter().copied().chain(std::iter::once(0)))
            .collect();
        use std::os::unix::ffi::OsStrExt;
        for path in result.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            paths.push(PathBuf::from(std::ffi::OsStr::from_bytes(path)));
        }
    } else {
        let mut builder = ignore::WalkBuilder::new(root);
        builder.hidden(false).require_git(false);
        for item in builder.build() {
            let entry = item.map_err(|e| ModelError::Other(e.to_string()))?;
            if entry.file_type().is_some_and(|t| !t.is_dir()) {
                let rel = entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|e| ModelError::Other(e.to_string()))?;
                paths.push(rel.to_path_buf());
            }
        }
    }
    paths.sort();
    paths.dedup();
    let mut digest = Sha256::new();
    digest.update(index_state);
    for path in paths {
        if path.components().any(|c| {
            c.as_os_str() == ".git" || c.as_os_str() == ".devcode" || c.as_os_str() == ".env"
        }) {
            continue;
        }
        use std::os::unix::ffi::OsStrExt;
        let name = path.as_os_str().as_bytes();
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name);
        let full = root.join(&path);
        match std::fs::symlink_metadata(&full) {
            Ok(meta) => {
                digest.update(meta.permissions().mode().to_le_bytes());
                if meta.file_type().is_symlink() {
                    digest.update(b"link");
                    let target =
                        std::fs::read_link(full).map_err(|e| ModelError::Other(e.to_string()))?;
                    digest.update(target.as_os_str().as_bytes());
                } else if meta.is_file() {
                    digest.update(b"file");
                    let data = std::fs::read(full).map_err(|e| ModelError::Other(e.to_string()))?;
                    digest.update((data.len() as u64).to_le_bytes());
                    digest.update(data);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => digest.update(b"missing"),
            Err(e) => return Err(ModelError::Other(e.to_string())),
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}
