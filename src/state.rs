//! Where DevCode keeps per-project state: rollback snapshots, manifests, recovery copies,
//! managed-process logs, the audit log, traces and saved conversations.
//!
//! The state lives outside the project, as gemini-cli keeps checkpoints in
//! `~/.gemini/history/<project>` and Codex keeps sessions in `~/.codex/sessions`. Inside the
//! workspace, a command such as `rm -rf` could delete the snapshots that rollback depends on.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Completed manifests older than 7 days are eligible for pruning (ARCH §4.2).
pub const MANIFEST_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
/// Maximum completed manifests retained per inactive session (ARCH §4.2).
pub const MAX_SESSION_COMPLETED_MANIFESTS: usize = 20;

/// Base directory for all projects: `DEVCODE_STATE_DIR`, else `$XDG_DATA_HOME/devcode`, else
/// `~/.local/share/devcode`. Without a home directory it falls back to `<root>/.devcode`.
fn base() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    if let Some(dir) = var("DEVCODE_STATE_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = var("XDG_DATA_HOME") {
        return Some(PathBuf::from(dir).join("devcode"));
    }
    var("HOME").map(|home| PathBuf::from(home).join(".local/share/devcode"))
}

/// State directory for the project at `root`: `<base>/projects/<folder>-<hash>`, where the
/// hash is of the canonical project path so two folders with one name stay apart.
pub fn dir(root: &Path) -> PathBuf {
    let Some(base) = base() else {
        return root.join(".devcode");
    };
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let hash = format!(
        "{:x}",
        Sha256::digest(canonical.to_string_lossy().as_bytes())
    );
    let name: String = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    base.join("projects")
        .join(format!("{name}-{}", &hash[..12]))
}

pub fn manifests_dir(root: &Path) -> PathBuf {
    dir(root).join("manifests")
}

pub fn blobs_dir(root: &Path) -> PathBuf {
    dir(root).join("blobs")
}

pub fn recovery_dir(root: &Path) -> PathBuf {
    dir(root).join("recovery")
}

#[cfg(test)]
mod tests {
    #[test]
    fn state_lives_outside_the_project_and_differs_per_project() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let (da, db) = (super::dir(a.path()), super::dir(b.path()));
        assert!(
            !da.starts_with(a.path().canonicalize().unwrap()),
            "{}",
            da.display()
        );
        assert_ne!(da, db);
        assert_eq!(da, super::dir(a.path()), "stable for one project");
    }
}
