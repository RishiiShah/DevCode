pub mod compaction;
pub mod dependency;
pub mod scope;
pub mod summary;

pub use compaction::{compact_messages, should_compact, CompactionContext};
pub use dependency::{has_dependency_addition, is_dependency_manifest, prompt_dependency_addition};
pub use scope::{DeclareScopeTool, ScopeContract, ScopeState};
pub use summary::TaskSummary;

use crate::approval::prompt::ApprovalPrompt;
use crate::approval::runner::ExecutionError;
use crate::gate::tool_log::ToolLog;
use crate::guard::path::check_write_path;
use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Applies a file edit with full discipline controls:
/// 1. Scope contract check: out-of-scope edits prompt with "outside declared scope" even with auto-approve.
/// 2. Dependency check: additions to dependency manifests require approval and justification logged to ToolLog.
/// 3. Unified diff and step-manifest rollback capture.
#[allow(clippy::too_many_arguments)]
pub fn apply_disciplined_file_edit<R: BufRead, W: Write>(
    root: &Path,
    rel_path: &Path,
    new_content: &str,
    session_id: &str,
    step_id: usize,
    scope: &ScopeContract,
    prompt: &mut ApprovalPrompt<R, W>,
    tool_log: &mut ToolLog,
    summary: &mut TaskSummary,
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

    if !scope.is_declared() {
        return Err(ExecutionError::Denied(
            "declare_scope must be called before editing".into(),
        ));
    }
    let in_scope = scope.is_in_scope(rel_path, root);
    if !in_scope {
        summary.files_outside_scope += 1;
        // Out-of-scope edits must prompt even if auto_approve_edits is active
        if !prompt.is_tty {
            writeln!(
                prompt.writer,
                "Edit to '{}' rejected: outside declared scope and no TTY",
                rel_path.display()
            )
            .map_err(ExecutionError::Io)?;
            return Ok(false);
        }

        write!(
            prompt.writer,
            "Edit to '{}' is outside declared scope. Approve? [y]es / [n]o: ",
            rel_path.display()
        )
        .map_err(ExecutionError::Io)?;
        prompt.writer.flush().map_err(ExecutionError::Io)?;

        let mut input = String::new();
        if prompt
            .reader
            .read_line(&mut input)
            .map_err(ExecutionError::Io)?
            == 0
        {
            return Ok(false);
        }

        let trimmed = input.trim().to_lowercase();
        if !matches!(trimmed.as_str(), "y" | "yes" | "once" | "o") {
            return Ok(false);
        }
    }

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    // Check for dependency addition
    let mut justification = None;
    if has_dependency_addition(rel_path, &old_content, new_content) {
        let just_opt = prompt_dependency_addition(
            &rel_path.to_string_lossy(),
            &mut prompt.reader,
            &mut prompt.writer,
            prompt.is_tty,
        )
        .map_err(ExecutionError::Io)?;

        match just_opt {
            Some(justification_text) => {
                justification = Some(justification_text);
            }
            None => {
                return Ok(false);
            }
        }
    }

    let approved = crate::approval::apply_guarded_file_edit(
        root,
        rel_path,
        new_content,
        session_id,
        step_id,
        prompt,
        tool_log,
    )?;
    if !approved {
        return Ok(false);
    }
    if let Some(justification) = justification {
        summary.dependencies_added += 1;
        tool_log.record_dependency_justification(&rel_path.to_string_lossy(), &justification, ts);
    }
    if !in_scope {
        tool_log.record_outside_scope(&rel_path.to_string_lossy(), ts);
    }
    summary.files_edited += 1;
    Ok(true)
}
