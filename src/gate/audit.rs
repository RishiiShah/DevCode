use crate::gate::GateError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const GENESIS_PREV_SHA: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub actor: String,
    pub files: Vec<PathBuf>,
    pub diff_sha: String,
    pub prev_entry_sha: String,
    pub reason: String,
}

fn sha256_digest(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Appends a new JSON audit log entry with an unbroken hash chain to `.devcode/audit.log`.
pub fn append_skip_tests_audit(
    root: &Path,
    actor: &str,
    files: &[PathBuf],
    diff_sha: &str,
    reason: &str,
) -> std::io::Result<()> {
    let audit_dir = root.join(".devcode");
    fs::create_dir_all(&audit_dir)?;
    let audit_path = audit_dir.join("audit.log");

    let prev_sha = if audit_path.exists() {
        let content = fs::read_to_string(&audit_path)?;
        let non_empty_lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        if let Some(last_line) = non_empty_lines.last() {
            sha256_digest(last_line)
        } else {
            GENESIS_PREV_SHA.to_string()
        }
    } else {
        GENESIS_PREV_SHA.to_string()
    };

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let entry = AuditEntry {
        timestamp,
        actor: actor.to_string(),
        files: files.to_vec(),
        diff_sha: diff_sha.to_string(),
        prev_entry_sha: prev_sha,
        reason: reason.to_string(),
    };

    let line = serde_json::to_string(&entry)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&audit_path)?;
    writeln!(file, "{}", line)?;
    file.flush()?;
    file.sync_all()?;

    if let Ok(dir) = fs::File::open(&audit_dir) {
        let _ = dir.sync_all();
    }

    Ok(())
}

/// Verifies that `.devcode/audit.log` contains an unbroken hash chain.
/// Returns an error if any entry was tampered with, deleted, or corrupted.
pub fn verify_audit_chain(root: &Path) -> Result<(), GateError> {
    let audit_path = root.join(".devcode").join("audit.log");
    if !audit_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&audit_path).map_err(GateError::Io)?;
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();

    let mut expected_prev = GENESIS_PREV_SHA.to_string();

    for (idx, line) in lines.iter().enumerate() {
        let entry: AuditEntry = serde_json::from_str(line).map_err(|e| {
            GateError::AuditLogTampered(format!("line {}: invalid JSON ({})", idx + 1, e))
        })?;

        if entry.prev_entry_sha != expected_prev {
            return Err(GateError::AuditChainBroken {
                line: idx + 1,
                expected: expected_prev,
                found: entry.prev_entry_sha,
            });
        }

        expected_prev = sha256_digest(line);
    }

    Ok(())
}
