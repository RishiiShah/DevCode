use crate::agent::model::{Message, Role};
use crate::config::Config;
use crate::gate::tool_log::{ToolKind, ToolLog};
use crate::manifest::record::StepManifest;
use std::collections::HashSet;

pub struct CompactionContext<'a> {
    pub context_window: u64,
    pub active_manifest: Option<&'a StepManifest>,
    pub current_diff: &'a str,
    pub config: &'a Config,
    pub tool_log: &'a ToolLog,
}

/// Returns true when total tokens burned exceeds 75% of the configured context window.
pub fn should_compact(total_tokens: u64, context_window: u64) -> bool {
    if context_window == 0 {
        return false;
    }
    total_tokens * 4 > context_window * 3
}

/// Produces a deterministic, model-call-free compacted message history.
/// Pins the active manifest, current diff, config, and preserves the last test result verbatim.
pub fn compact_messages(messages: &[Message], ctx: &CompactionContext) -> Vec<Message> {
    if messages.is_empty() {
        return Vec::new();
    }

    // 1. Initial user goal
    let initial_goal = messages
        .iter()
        .find(|m| m.role == Role::User)
        .map(|m| m.content.clone())
        .unwrap_or_else(|| "<no initial user instruction>".to_string());

    // 2. Files modified from tool log
    let mut modified_files = HashSet::new();
    for entry in &ctx.tool_log.entries {
        if matches!(entry.kind, ToolKind::FileEdit | ToolKind::CommandFileChange) {
            modified_files.insert(entry.detail.clone());
        }
    }
    let mut files_list: Vec<String> = modified_files.into_iter().collect();
    files_list.sort();

    // 3. Last test result preserved verbatim
    let last_test_verbatim = ctx
        .tool_log
        .entries
        .iter()
        .rfind(|e| e.kind == ToolKind::RunCommand)
        .map(|e| {
            format!(
                "Command: '{}', Exit Code: {:?}, Timestamp: {}",
                e.detail, e.exit_code, e.timestamp
            )
        })
        .unwrap_or_else(|| "No test command executed".to_string());

    // 4. Pinned Configuration
    let pinned_config = format!(
        "Test Command: {:?}, Test Globs: {:?}",
        ctx.config.verification.test_command, ctx.config.verification.test_globs
    );

    // 5. Pinned Active Manifest
    let pinned_manifest = match ctx.active_manifest {
        Some(m) => format!(
            "Session: {}, Step: {}, Pre-Paths: {}, Post-Paths: {}",
            m.session_id,
            m.step_id,
            m.pre_state.len(),
            m.post_state.as_ref().map(|p| p.len()).unwrap_or(0)
        ),
        None => "No active manifest pinned".to_string(),
    };

    // 6. Build harness summary text
    let summary_text = format!(
        "--- HARNESS DETERMINISTIC COMPACTION SUMMARY ---\n\
         Goal: {}\n\
         Files Modified: {:?}\n\
         Last Test Result: {}\n\
         Pinned Configuration: {}\n\
         Pinned Active Manifest: {}\n\
         Pinned Current Diff:\n\
         {}\n\
         ------------------------------------------------",
        initial_goal,
        files_list,
        last_test_verbatim,
        pinned_config,
        pinned_manifest,
        ctx.current_diff
    );

    // 7. Compact intermediate messages:
    // Retain initial user goal, insert compaction summary, then retain recent turns with truncated tool outputs
    let mut compacted = Vec::new();
    compacted.push(Message::user(&initial_goal));
    compacted.push(Message {
        role: Role::Assistant,
        content: summary_text,
        tool_calls: Vec::new(),
        tool_call_id: None,
    });

    // Retain up to last 2 messages (e.g. latest assistant call and tool output)
    let tail_len = if messages.len() > 2 { 2 } else { 0 };
    if tail_len > 0 {
        for msg in &messages[messages.len() - tail_len..] {
            let mut m = msg.clone();
            // Truncate long tool output if needed (keep head & tail with marker)
            if m.content.len() > 1000 {
                let head = &m.content[..300];
                let tail = &m.content[m.content.len() - 300..];
                m.content = format!(
                    "{}\n[... output truncated by compaction ...]\n{}",
                    head, tail
                );
            }
            compacted.push(m);
        }
    }

    compacted
}
