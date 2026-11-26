pub mod blob;
pub mod record;
pub mod rollback;
pub mod snapshot;

pub use blob::{read_blob, store_blob};
pub use record::{record_path, FileType, PathRecord, StepManifest};
pub use rollback::{
    prune_state, prune_state_at, recover_snapshot, rollback, rollback_latest, GcReport,
    RollbackReport,
};
pub use snapshot::{
    reconcile, snapshot_guard_set, FileFingerprint, GuardSetSnapshot, ReconcileResult,
    MAX_GUARD_SET_FILES, MAX_PER_FILE_BYTES, MAX_TOTAL_BLOB_BYTES,
};

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ManifestError {
    Storage(std::io::Error),
    BlobNotFound {
        hash: String,
        source: std::io::Error,
    },
    ManifestNotFound {
        session_id: String,
        step_id: usize,
    },
    IncompleteStepRecord {
        step_id: usize,
    },
    Conflict {
        path: PathBuf,
        reason: String,
    },
    PartialRecovery {
        restored: Vec<PathBuf>,
        failed_path: PathBuf,
        source: std::io::Error,
    },
    BudgetExceeded(String),
    Serialization(serde_json::Error),
    Io(std::io::Error),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Storage(err) => write!(f, "manifest storage error: {}", err),
            ManifestError::BlobNotFound { hash, source } => {
                write!(f, "blob '{}' not found: {}", hash, source)
            }
            ManifestError::ManifestNotFound {
                session_id,
                step_id,
            } => {
                write!(
                    f,
                    "manifest not found for session '{}' step {}",
                    session_id, step_id
                )
            }
            ManifestError::IncompleteStepRecord { step_id } => {
                write!(
                    f,
                    "step {} record is incomplete (missing post-state)",
                    step_id
                )
            }
            ManifestError::Conflict { path, reason } => {
                write!(f, "conflict detected on '{}': {}", path.display(), reason)
            }
            ManifestError::PartialRecovery {
                restored,
                failed_path,
                source,
            } => {
                write!(
                    f,
                    "partial recovery: restored {} paths, failed at '{}': {}",
                    restored.len(),
                    failed_path.display(),
                    source
                )
            }
            ManifestError::BudgetExceeded(msg) => write!(f, "guard-set budget exceeded: {}", msg),
            ManifestError::Serialization(err) => write!(f, "manifest serialization error: {}", err),
            ManifestError::Io(err) => write!(f, "I/O error in manifest engine: {}", err),
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ManifestError::Storage(err) | ManifestError::Io(err) => Some(err),
            ManifestError::BlobNotFound { source, .. } => Some(source),
            ManifestError::PartialRecovery { source, .. } => Some(source),
            ManifestError::Serialization(err) => Some(err),
            _ => None,
        }
    }
}
