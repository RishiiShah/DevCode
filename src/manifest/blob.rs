use crate::manifest::ManifestError;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::Path;

/// Stores a blob in content-addressed storage `<state dir>/blobs/<sha256>` (see `crate::state`).
/// Flushes and fsyncs both the blob file and its directory.
pub fn store_blob(root: &Path, content: &[u8]) -> Result<String, ManifestError> {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let hash = format!("{:x}", hasher.finalize());

    let blobs_dir = crate::state::dir(root).join("blobs");
    fs::create_dir_all(&blobs_dir).map_err(ManifestError::Storage)?;

    let blob_path = blobs_dir.join(&hash);
    if !blob_path.exists() {
        let mut file = fs::File::create(&blob_path).map_err(ManifestError::Storage)?;
        file.write_all(content).map_err(ManifestError::Storage)?;
        file.flush().map_err(ManifestError::Storage)?;
        file.sync_all().map_err(ManifestError::Storage)?;

        // Fsync parent directory
        if let Ok(dir) = fs::File::open(&blobs_dir) {
            let _ = dir.sync_all();
        }
    }

    Ok(hash)
}

/// Reads a content-addressed blob from `<state dir>/blobs/<sha256>` (see `crate::state`).
pub fn read_blob(root: &Path, hash: &str) -> Result<Vec<u8>, ManifestError> {
    let blob_path = crate::state::dir(root).join("blobs").join(hash);
    fs::read(&blob_path).map_err(|source| ManifestError::BlobNotFound {
        hash: hash.to_string(),
        source,
    })
}
