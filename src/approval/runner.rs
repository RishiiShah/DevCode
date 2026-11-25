use crate::approval::lexical::{check_shell_command, CommandCheck};
use crate::approval::prompt::ApprovalPrompt;
use crate::gate::tool_log::ToolLog;
use crate::guard::path::check_write_path;
use crate::guard::process::{run_bounded, DEFAULT_OUT_CAP};
use crate::guard::GuardError;
use crate::manifest::record::{capture, record_path, save_manifest, PathRecord, StepManifest};
use crate::manifest::snapshot::{reconcile, snapshot_guard_set, ReconcileResult};
use crate::manifest::ManifestError;
use std::fmt;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub stdout_stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub reconciled: Option<ReconcileResult>,
}

#[derive(Debug)]
pub enum ExecutionError {
    Denied(String),
    Rejected(String),
    Snapshot(ManifestError),
    Process(GuardError),
    Manifest(ManifestError),
    Io(std::io::Error),
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecutionError::Denied(reason) => write!(f, "command denied: {}", reason),
            ExecutionError::Rejected(reason) => write!(f, "command rejected: {}", reason),
            ExecutionError::Snapshot(err) => write!(f, "snapshot error: {}", err),
            ExecutionError::Process(err) => write!(f, "process execution error: {}", err),
            ExecutionError::Manifest(err) => write!(f, "manifest error: {}", err),
            ExecutionError::Io(err) => write!(f, "I/O error: {}", err),
        }
    }
}

impl std::error::Error for ExecutionError {}

/// Returns true if `cmd` is purely in the read-only allowlist with no redirections.
pub fn is_read_only_command(cmd: &str) -> bool {
    let stripped = crate::approval::lexical::strip_heredocs(cmd);
    if stripped.contains('>') {
        return false;
    }
    let parts: Vec<&str> = stripped.split_whitespace().collect();
    if parts.is_empty() {
        return true;
    }
    let first = Path::new(parts[0])
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(parts[0]);
    match first {
        "ls" | "cat" | "rg" | "grep" | "find" | "head" | "tail" => true,
        "git" => {
            if let Some(sub) = parts.iter().skip(1).find(|w| !w.starts_with('-')) {
                matches!(*sub, "status" | "diff" | "log" | "show" | "branch")
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Executes a shell command guarded by lexical analysis, snapshot checkpointing,
/// and post-command reconciliation.
pub fn execute_guarded_command<R: BufRead, W: Write>(
    root: &Path,
    cmd_str: &str,
    session_id: &str,
    step_id: usize,
    timeout_secs: u64,
    prompt: &mut ApprovalPrompt<R, W>,
    tool_log: &mut ToolLog,
) -> Result<CommandOutput, ExecutionError> {
    let guard_set_paths: Vec<PathBuf> = ignore::WalkBuilder::new(root)
        .hidden(false)
        .build()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.path().strip_prefix(root).ok().map(Path::to_path_buf))
        .collect();

    // 2. Lexical check
    let check = check_shell_command(cmd_str, root, &guard_set_paths);
    match check {
        CommandCheck::Denied(reason) => return Err(ExecutionError::Denied(reason)),
        CommandCheck::Ask(reason) => {
            let approved = prompt
                .request_command(cmd_str, &reason)
                .map_err(ExecutionError::Io)?;
            if !approved {
                return Err(ExecutionError::Rejected(format!(
                    "Destructive command '{}' rejected: {}",
                    cmd_str, reason
                )));
            }
        }
        CommandCheck::Allowed => {}
    }

    let snap_before = snapshot_guard_set(root, None).map_err(ExecutionError::Snapshot)?;

    let argv = ["sh", "-c", cmd_str];
    let timeout = Duration::from_secs(timeout_secs);
    let res =
        run_bounded(&argv, root, timeout, DEFAULT_OUT_CAP).map_err(ExecutionError::Process)?;

    let snap_after =
        snapshot_guard_set(root, Some(&snap_before)).map_err(ExecutionError::Snapshot)?;
    let rec = reconcile(&snap_before, &snap_after);

    let has_mutations =
        !rec.created.is_empty() || !rec.modified.is_empty() || !rec.deleted.is_empty();

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    if has_mutations {
        let mut pre_state = Vec::new();
        let mut post_state = Vec::new();

        for p in &rec.created {
            pre_state.push(PathRecord {
                path: p.clone(),
                exists: false,
                file_type: None,
                symlink_target: None,
                mode: None,
                sha256: None,
                blob_ref: None,
            });
            post_state.push(record_path(root, p).map_err(ExecutionError::Manifest)?);
            tool_log.record_command_file_change(&p.to_string_lossy(), ts);
        }

        for p in &rec.modified {
            let fp = snap_before.files.get(p);
            pre_state.push(PathRecord {
                path: p.clone(),
                exists: true,
                file_type: fp.map(|f| f.file_type),
                symlink_target: fp.and_then(|f| f.symlink_target.clone()),
                mode: fp.map(|f| f.mode),
                sha256: fp.and_then(|f| f.sha256.clone()),
                blob_ref: fp.and_then(|f| f.sha256.clone()),
            });
            post_state.push(record_path(root, p).map_err(ExecutionError::Manifest)?);
            tool_log.record_command_file_change(&p.to_string_lossy(), ts);
        }

        for p in &rec.deleted {
            let fp = snap_before.files.get(p);
            pre_state.push(PathRecord {
                path: p.clone(),
                exists: true,
                file_type: fp.map(|f| f.file_type),
                symlink_target: fp.and_then(|f| f.symlink_target.clone()),
                mode: fp.map(|f| f.mode),
                sha256: fp.and_then(|f| f.sha256.clone()),
                blob_ref: fp.and_then(|f| f.sha256.clone()),
            });
            post_state.push(PathRecord {
                path: p.clone(),
                exists: false,
                file_type: None,
                symlink_target: None,
                mode: None,
                sha256: None,
                blob_ref: None,
            });
            tool_log.record_command_file_change(&p.to_string_lossy(), ts);
        }

        let manifest = StepManifest {
            session_id: session_id.to_string(),
            step_id,
            timestamp: ts,
            pre_state,
            post_state: Some(post_state),
        };
        save_manifest(root, &manifest).map_err(ExecutionError::Manifest)?;
    }

    tool_log.record_command(cmd_str, res.exit_code, ts);

    Ok(CommandOutput {
        stdout_stderr: res.output_text(),
        exit_code: res.exit_code,
        timed_out: res.timed_out,
        reconciled: Some(rec),
    })
}

/// Applies a file edit guarded by boundary checks, unified diff approval,
/// and step-manifest capture.
pub fn apply_guarded_file_edit<R: BufRead, W: Write>(
    root: &Path,
    rel_path: &Path,
    new_content: &str,
    session_id: &str,
    step_id: usize,
    prompt: &mut ApprovalPrompt<R, W>,
    tool_log: &mut ToolLog,
) -> Result<bool, ExecutionError> {
    let full_path = check_write_path(root, rel_path).map_err(ExecutionError::Process)?;

    let old_content = match fs::read_to_string(&full_path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(ExecutionError::Io(e)),
    };

    if old_content == new_content {
        return Ok(true);
    }

    let canonical_root = root.canonicalize().map_err(ExecutionError::Io)?;
    let relative = full_path
        .strip_prefix(&canonical_root)
        .map_err(|e| ExecutionError::Denied(e.to_string()))?;
    capture(root, session_id, step_id, &[relative]).map_err(ExecutionError::Manifest)?;

    let approved = prompt
        .request_edit(&rel_path.to_string_lossy(), &old_content, new_content)
        .map_err(ExecutionError::Io)?;

    if !approved {
        let path = root
            .join(".devcode/manifests")
            .join(session_id)
            .join(format!("{}.json", step_id));
        fs::remove_file(path).map_err(ExecutionError::Io)?;
        return Ok(false);
    }

    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent).map_err(ExecutionError::Io)?;
    }
    fs::write(&full_path, new_content.as_bytes()).map_err(ExecutionError::Io)?;

    // Record post-mutation state
    crate::manifest::record::record_post(root, session_id, step_id)
        .map_err(ExecutionError::Manifest)?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    tool_log.record_file_edit(&rel_path.to_string_lossy(), ts);

    Ok(true)
}
