//! Saved conversations, so a later `devcode --continue`, `devcode --resume` or `/resume` can pick
//! one up again. Codex keeps rollouts in `~/.codex/sessions`, Claude Code keeps transcripts per
//! project, and opencode keeps sessions in its data dir; DevCode keeps one JSON file per
//! conversation in the project's state dir (`crate::state`).

use crate::agent::{token_ledger::LedgerEntry, Message, Role, ToolOutcome};
use crate::discipline::ScopeState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Saved {
    id: String,
    /// Seconds since the Unix epoch of the last save.
    updated: u64,
    title: String,
    messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<ScopeState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    tool_outcomes: BTreeMap<String, ToolOutcome>,
    #[serde(default)]
    total_in: u64,
    #[serde(default)]
    total_out: u64,
    #[serde(default)]
    context_used: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    token_ledger: Vec<LedgerEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    private_policy: Option<String>,
}

/// Full conversation payload restored on resume.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoadedConversation {
    pub id: String,
    pub title: String,
    pub messages: Vec<Message>,
    pub scope: Option<ScopeState>,
    pub tool_outcomes: BTreeMap<String, ToolOutcome>,
    pub total_in: u64,
    pub total_out: u64,
    pub context_used: u64,
    pub token_ledger: Vec<LedgerEntry>,
    pub private_policy: Option<String>,
}

/// Token counters persisted with a conversation for `/status` and the status line on resume.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConversationUsage {
    pub total_in: u64,
    pub total_out: u64,
    pub context_used: u64,
    pub token_ledger: Vec<LedgerEntry>,
    pub private_policy: Option<String>,
}

/// One saved conversation, newest first in `list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    pub updated: u64,
    pub title: String,
    /// User prompts in the conversation.
    pub turns: usize,
}

fn dir(root: &Path) -> PathBuf {
    crate::state::dir(root).join("conversations")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A new conversation id; sorts by start time.
pub fn new_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

/// The first line of the first prompt, as the picker shows it.
pub fn title_for(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut title: String = line.chars().take(72).collect();
    if line.chars().count() > 72 {
        title.push('…');
    }
    title
}

/// Writes the conversation (temp file, then rename, so a crash never leaves half a file).
pub fn save(root: &Path, id: &str, title: &str, messages: &[Message]) -> std::io::Result<()> {
    save_with_meta(
        root,
        id,
        title,
        messages,
        &BTreeMap::new(),
        ConversationUsage::default(),
    )
}

/// Writes the conversation with per-tool outcomes (`ok`, `diff`) and token usage metadata.
pub fn save_with_meta(
    root: &Path,
    id: &str,
    title: &str,
    messages: &[Message],
    tool_outcomes: &BTreeMap<String, ToolOutcome>,
    usage: ConversationUsage,
) -> std::io::Result<()> {
    save_with_scope(root, id, title, messages, tool_outcomes, usage, None)
}

pub(crate) fn save_with_scope(
    root: &Path,
    id: &str,
    title: &str,
    messages: &[Message],
    tool_outcomes: &BTreeMap<String, ToolOutcome>,
    usage: ConversationUsage,
    scope: Option<ScopeState>,
) -> std::io::Result<()> {
    let dir = dir(root);
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let saved = Saved {
        id: id.to_string(),
        updated: now(),
        title: title.to_string(),
        messages: messages.to_vec(),
        scope,
        tool_outcomes: tool_outcomes.clone(),
        total_in: usage.total_in,
        total_out: usage.total_out,
        context_used: usage.context_used,
        token_ledger: usage.token_ledger,
        private_policy: usage.private_policy,
    };
    let json = serde_json::to_vec(&saved).map_err(std::io::Error::other)?;
    let tmp = dir.join(format!("{id}.{}.json.tmp", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let write_res = (|| {
        let mut file = opts.open(&tmp)?;
        file.write_all(&json)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, dir.join(format!("{id}.json")))
    })();
    if write_res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_res
}

/// The conversation's id, title and messages.
pub fn load(root: &Path, id: &str) -> std::io::Result<(String, String, Vec<Message>)> {
    let loaded = load_full(root, id)?;
    Ok((loaded.id, loaded.title, loaded.messages))
}

/// The conversation's id, title, messages, per-tool outcomes, and token usage.
pub fn load_full(root: &Path, id: &str) -> std::io::Result<LoadedConversation> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("no saved conversation '{id}'"),
        ));
    }
    let bytes = std::fs::read(dir(root).join(format!("{id}.json"))).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            std::io::Error::new(e.kind(), format!("no saved conversation '{id}'"))
        } else {
            e
        }
    })?;
    let saved: Saved = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    Ok(LoadedConversation {
        id: saved.id,
        title: saved.title,
        messages: saved.messages,
        scope: saved.scope,
        tool_outcomes: saved.tool_outcomes,
        total_in: saved.total_in,
        total_out: saved.total_out,
        context_used: saved.context_used,
        token_ledger: saved.token_ledger,
        private_policy: saved.private_policy,
    })
}

/// Saved conversations for this project, most recently updated first.
pub fn list(root: &Path) -> Vec<Summary> {
    let Ok(entries) = std::fs::read_dir(dir(root)) else {
        return Vec::new();
    };
    let mut all: Vec<Summary> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<Saved>(&bytes).ok())
        .map(|s| Summary {
            turns: s
                .messages
                .iter()
                .filter(|m| m.role == Role::User && m.tool_call_id.is_none())
                .count(),
            id: s.id,
            updated: s.updated,
            title: s.title,
        })
        .collect();
    all.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
    all
}

/// "3m ago", "2h ago", "5d ago" for the picker.
pub fn age(updated: u64) -> String {
    let secs = now().saturating_sub(updated);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}
