use crate::manifest::blob::store_blob;
use crate::manifest::ManifestError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileType {
    File,
    Symlink,
    Dir,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathRecord {
    pub path: PathBuf,
    pub exists: bool,
    pub file_type: Option<FileType>,
    pub symlink_target: Option<PathBuf>,
    pub mode: Option<u32>,
    pub sha256: Option<String>,
    pub blob_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepManifest {
    pub session_id: String,
    pub step_id: usize,
    pub timestamp: u64,
    pub pre_state: Vec<PathRecord>,
    pub post_state: Option<Vec<PathRecord>>,
}

/// Records the exact pre-step or post-step state of a path on disk.
/// For regular files, the content is captured and stored in blob storage.
pub fn record_path(root: &Path, rel_path: &Path) -> Result<PathRecord, ManifestError> {
    let full_path = root.join(rel_path);

    let meta = match fs::symlink_metadata(&full_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PathRecord {
                path: rel_path.to_path_buf(),
                exists: false,
                file_type: None,
                symlink_target: None,
                mode: None,
                sha256: None,
                blob_ref: None,
            });
        }
        Err(e) => return Err(ManifestError::Io(e)),
    };

    let mode = meta.permissions().mode();

    if meta.file_type().is_symlink() {
        let target = fs::read_link(&full_path).map_err(ManifestError::Io)?;
        Ok(PathRecord {
            path: rel_path.to_path_buf(),
            exists: true,
            file_type: Some(FileType::Symlink),
            symlink_target: Some(target),
            mode: Some(mode),
            sha256: None,
            blob_ref: None,
        })
    } else if meta.file_type().is_dir() {
        Ok(PathRecord {
            path: rel_path.to_path_buf(),
            exists: true,
            file_type: Some(FileType::Dir),
            symlink_target: None,
            mode: Some(mode),
            sha256: None,
            blob_ref: None,
        })
    } else {
        // Regular file
        let content = fs::read(&full_path).map_err(ManifestError::Io)?;
        let blob_hash = store_blob(root, &content)?;
        Ok(PathRecord {
            path: rel_path.to_path_buf(),
            exists: true,
            file_type: Some(FileType::File),
            symlink_target: None,
            mode: Some(mode),
            sha256: Some(blob_hash.clone()),
            blob_ref: Some(blob_hash),
        })
    }
}

/// Captures pre-mutation state for the given paths, publishes the manifest,
/// and fsyncs the file and directory.
pub fn capture(
    root: &Path,
    session_id: &str,
    step_id: usize,
    paths: &[&Path],
) -> Result<StepManifest, ManifestError> {
    let mut pre_state = Vec::new();

    for path in paths {
        let rel_path = if path.is_absolute() {
            path.strip_prefix(root).map_err(|_| {
                ManifestError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "path outside root",
                ))
            })?
        } else {
            path
        };
        let rec = record_path(root, rel_path)?;
        pre_state.push(rec);
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let manifest = StepManifest {
        session_id: session_id.to_string(),
        step_id,
        timestamp,
        pre_state,
        post_state: None,
    };

    save_manifest(root, &manifest)?;
    Ok(manifest)
}

/// Records post-mutation state for all paths present in the manifest.
pub fn record_post(
    root: &Path,
    session_id: &str,
    step_id: usize,
) -> Result<StepManifest, ManifestError> {
    let mut manifest = load_manifest(root, session_id, step_id)?;

    let mut post_state = Vec::new();
    for pre in &manifest.pre_state {
        let post = record_path(root, &pre.path)?;
        post_state.push(post);
    }

    manifest.post_state = Some(post_state);
    save_manifest(root, &manifest)?;
    Ok(manifest)
}

pub fn save_manifest(root: &Path, manifest: &StepManifest) -> Result<(), ManifestError> {
    let manifest_dir = crate::state::dir(root)
        .join("manifests")
        .join(&manifest.session_id);
    fs::create_dir_all(&manifest_dir).map_err(ManifestError::Storage)?;

    let manifest_path = manifest_dir.join(format!("{}.json", manifest.step_id));
    let json = serde_json::to_string_pretty(manifest).map_err(ManifestError::Serialization)?;

    let mut file = fs::File::create(&manifest_path).map_err(ManifestError::Storage)?;
    file.write_all(json.as_bytes())
        .map_err(ManifestError::Storage)?;
    file.flush().map_err(ManifestError::Storage)?;
    file.sync_all().map_err(ManifestError::Storage)?;

    if let Ok(dir) = fs::File::open(&manifest_dir) {
        let _ = dir.sync_all();
    }

    Ok(())
}

pub fn load_manifest(
    root: &Path,
    session_id: &str,
    step_id: usize,
) -> Result<StepManifest, ManifestError> {
    let manifest_path = crate::state::dir(root)
        .join("manifests")
        .join(session_id)
        .join(format!("{}.json", step_id));

    let content = fs::read_to_string(&manifest_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ManifestError::ManifestNotFound {
                session_id: session_id.to_string(),
                step_id,
            }
        } else {
            ManifestError::Io(e)
        }
    })?;

    serde_json::from_str(&content).map_err(ManifestError::Serialization)
}
