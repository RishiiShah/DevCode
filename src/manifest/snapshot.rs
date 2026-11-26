use crate::manifest::blob::store_blob;
use crate::manifest::record::FileType;
use crate::manifest::ManifestError;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const MAX_GUARD_SET_FILES: usize = 5_000;
pub const MAX_TOTAL_BLOB_BYTES: u64 = 50 * 1024 * 1024; // 50 MB
pub const MAX_PER_FILE_BYTES: u64 = 10 * 1024 * 1024; // 10 MB

pub const DERIVED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    "__pycache__",
    ".cache",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFingerprint {
    pub file_type: FileType,
    pub size: u64,
    pub mtime_ns: u128,
    pub mode: u32,
    pub sha256: Option<String>,
    #[serde(default)]
    pub symlink_target: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GuardSetSnapshot {
    pub files: HashMap<PathBuf, FileFingerprint>,
    pub total_blob_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcileResult {
    pub created: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

/// Snapshots the guard set (all non-ignored workspace files excluding .devcode/).
/// Checks budgets (5,000 files, 50 MB blob storage). Uses previous snapshot for
/// incremental hashing of changed paths only.
pub fn snapshot_guard_set(
    root: &Path,
    previous: Option<&GuardSetSnapshot>,
) -> Result<GuardSetSnapshot, ManifestError> {
    let mut builder = WalkBuilder::new(root);
    builder.hidden(false);
    builder.git_ignore(true);
    builder.git_global(true);
    builder.git_exclude(true);

    let is_git = root.join(".git").exists();
    if !is_git {
        let mut ov = ignore::overrides::OverrideBuilder::new(root);
        for dir in DERIVED_DIRS {
            let _ = ov.add(&format!("!{}/**", dir));
            let _ = ov.add(&format!("!{}", dir));
        }
        if let Ok(built) = ov.build() {
            builder.overrides(built);
        }
    }

    let mut current_files: HashMap<PathBuf, FileFingerprint> = HashMap::new();
    let mut total_blob_bytes = previous.map(|p| p.total_blob_bytes).unwrap_or(0);

    for result in builder.build() {
        let entry = result.map_err(|e| ManifestError::Io(std::io::Error::other(e)))?;

        let path = entry.path();
        let rel_path = match path.strip_prefix(root) {
            Ok(r) => r,
            Err(_) => continue,
        };

        if rel_path.as_os_str().is_empty() {
            continue;
        }

        // Exclude .devcode/ case-insensitively
        let is_devcode = rel_path.components().any(|c| {
            c.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(".devcode")
        });
        if is_devcode {
            continue;
        }

        // Exclude .git directory
        let is_git_dir = rel_path
            .components()
            .any(|c| c.as_os_str().to_string_lossy() == ".git");
        if is_git_dir {
            continue;
        }

        let meta = fs::symlink_metadata(path).map_err(ManifestError::Io)?;

        // If directory, skip adding to files map
        if meta.file_type().is_dir() {
            continue;
        }

        // Budget check for files count
        if current_files.len() >= MAX_GUARD_SET_FILES {
            return Err(ManifestError::BudgetExceeded(format!(
                "guard set exceeded file count ceiling of {}",
                MAX_GUARD_SET_FILES
            )));
        }

        // Per-file cap 10 MB: larger files are outside guard set (reported as unprotected)
        let size = meta.len();
        if size > MAX_PER_FILE_BYTES {
            if previous.is_some_and(|p| p.files.contains_key(rel_path)) {
                use sha2::{Digest, Sha256};
                let mut file = fs::File::open(path).map_err(ManifestError::Io)?;
                let mut hash = Sha256::new();
                let mut buffer = [0u8; 8192];
                loop {
                    let count = file.read(&mut buffer).map_err(ManifestError::Io)?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&buffer[..count]);
                }
                current_files.insert(
                    rel_path.to_path_buf(),
                    FileFingerprint {
                        file_type: FileType::File,
                        size,
                        mtime_ns: meta
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map(|d| d.as_nanos())
                            .unwrap_or(0),
                        mode: meta.permissions().mode(),
                        sha256: Some(format!("{:x}", hash.finalize())),
                        symlink_target: None,
                    },
                );
            }
            continue;
        }

        let mode = meta.permissions().mode();
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);

        let file_type = if meta.file_type().is_symlink() {
            FileType::Symlink
        } else {
            FileType::File
        };

        // Check if unchanged in previous snapshot
        let prev_fp = previous.and_then(|p| p.files.get(rel_path));
        let sha256 = if let Some(prev) = prev_fp {
            if prev.file_type == file_type
                && prev.size == size
                && prev.mtime_ns == mtime_ns
                && prev.mode == mode
                && prev.sha256.is_some()
            {
                prev.sha256.clone()
            } else if file_type == FileType::File {
                let content = fs::read(path).map_err(ManifestError::Io)?;
                total_blob_bytes += content.len() as u64;
                if total_blob_bytes > MAX_TOTAL_BLOB_BYTES {
                    return Err(ManifestError::BudgetExceeded(
                        "total blob bytes exceeded ceiling of 50 MB".to_string(),
                    ));
                }
                Some(store_blob(root, &content)?)
            } else {
                None
            }
        } else if file_type == FileType::File {
            let content = fs::read(path).map_err(ManifestError::Io)?;
            total_blob_bytes += content.len() as u64;
            if total_blob_bytes > MAX_TOTAL_BLOB_BYTES {
                return Err(ManifestError::BudgetExceeded(
                    "total blob bytes exceeded ceiling of 50 MB".to_string(),
                ));
            }
            Some(store_blob(root, &content)?)
        } else {
            None
        };

        current_files.insert(
            rel_path.to_path_buf(),
            FileFingerprint {
                file_type,
                size,
                mtime_ns,
                mode,
                sha256,
                symlink_target: if file_type == FileType::Symlink {
                    Some(fs::read_link(path).map_err(ManifestError::Io)?)
                } else {
                    None
                },
            },
        );
    }

    Ok(GuardSetSnapshot {
        files: current_files,
        total_blob_bytes,
    })
}

/// Reconciles differences between pre-command and post-command snapshots.
pub fn reconcile(before: &GuardSetSnapshot, after: &GuardSetSnapshot) -> ReconcileResult {
    let mut created = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();

    for (path, after_fp) in &after.files {
        match before.files.get(path) {
            None => created.push(path.clone()),
            Some(before_fp) => {
                if before_fp != after_fp {
                    modified.push(path.clone());
                }
            }
        }
    }

    for path in before.files.keys() {
        if !after.files.contains_key(path) {
            deleted.push(path.clone());
        }
    }

    created.sort();
    modified.sort();
    deleted.sort();

    ReconcileResult {
        created,
        modified,
        deleted,
    }
}
