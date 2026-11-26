//! Saved conversations, so a later `devcode --continue`, `devcode --resume` or `/resume` can pick
//! one up again. Codex keeps rollouts in `~/.codex/sessions`, Claude Code keeps transcripts per
//! project, and opencode keeps sessions in its data dir; DevCode keeps one JSON file per
//! conversation in the project's state dir (`crate::state`).

use crate::agent::{Message, Role};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Saved {
    id: String,
    /// Seconds since the Unix epoch of the last save.
    updated: u64,
    title: String,
    messages: Vec<Message>,
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
    let dir = dir(root);
    std::fs::create_dir_all(&dir)?;
    let saved = Saved {
        id: id.to_string(),
        updated: now(),
        title: title.to_string(),
        messages: messages.to_vec(),
    };
    let json = serde_json::to_vec(&saved).map_err(std::io::Error::other)?;
    let tmp = dir.join(format!("{id}.json.tmp"));
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, dir.join(format!("{id}.json")))
}

/// The conversation's id, title and messages.
pub fn load(root: &Path, id: &str) -> std::io::Result<(String, String, Vec<Message>)> {
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
    Ok((saved.id, saved.title, saved.messages))
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
