use crate::manifest::blob::read_blob;
use crate::manifest::record::{load_manifest, FileType, StepManifest};
use crate::manifest::ManifestError;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Extracts a pre-command snapshot without overwriting the working tree.
pub fn recover_snapshot(root: &Path, session: &str, step: usize) -> Result<PathBuf, ManifestError> {
    if session.is_empty() || session == "." || session == ".." || session.contains(['/', '\\']) {
        return Err(ManifestError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid session name",
        )));
    }
    let source = crate::state::dir(root)
        .join("recovery")
        .join(session)
        .join(format!("{step}.json"));
    let data = fs::read(source).map_err(ManifestError::Io)?;
    let snapshot: crate::manifest::GuardSetSnapshot =
        serde_json::from_slice(&data).map_err(ManifestError::Serialization)?;
    for (path, record) in &snapshot.files {
        if path.as_os_str().is_empty()
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(ManifestError::Conflict {
                path: path.clone(),
                reason: "invalid recovery path".into(),
            });
        }
        if record.file_type == FileType::File {
            let hash = record
                .sha256
                .as_ref()
                .ok_or_else(|| ManifestError::Conflict {
                    path: path.clone(),
                    reason: "missing recovery blob".into(),
                })?;
            read_blob(root, hash)?;
        }
        if path
            .ancestors()
            .skip(1)
            .any(|p| snapshot.files.contains_key(p))
        {
            return Err(ManifestError::Conflict {
                path: path.clone(),
                reason: "recovery path has a non-directory parent".into(),
            });
        }
    }
    let dest = crate::state::dir(root)
        .join("recovered")
        .join(session)
        .join(step.to_string());
    fs::create_dir_all(
        dest.parent()
            .ok_or_else(|| ManifestError::Io(std::io::Error::other("invalid destination")))?,
    )
    .map_err(ManifestError::Io)?;
    fs::create_dir(&dest).map_err(ManifestError::Io)?;
    for (path, record) in &snapshot.files {
        let target = dest.join(path);
        fs::create_dir_all(
            target
                .parent()
                .ok_or_else(|| ManifestError::Io(std::io::Error::other("invalid path")))?,
        )
        .map_err(ManifestError::Io)?;
        match record.file_type {
            FileType::File => {
                let hash = record
                    .sha256
                    .as_ref()
                    .ok_or_else(|| ManifestError::Io(std::io::Error::other("missing blob")))?;
                fs::write(&target, read_blob(root, hash)?).map_err(ManifestError::Io)?;
                fs::set_permissions(&target, fs::Permissions::from_mode(record.mode))
                    .map_err(ManifestError::Io)?;
            }
            FileType::Symlink => {
                let link = record.symlink_target.as_ref().ok_or_else(|| {
                    ManifestError::Io(std::io::Error::other("missing symlink target"))
                })?;
                symlink(link, &target).map_err(ManifestError::Io)?;
            }
            FileType::Dir => {
                return Err(ManifestError::Io(std::io::Error::other(
                    "unexpected directory",
                )))
            }
        }
    }
    Ok(dest)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RollbackReport {
    pub restored_paths: Vec<PathBuf>,
    pub removed_paths: Vec<PathBuf>,
}

/// Rolls back session mutations down to target_step (or the entire session if None) in reverse step order.
/// Preflights every touched path against recorded post-state before applying changes.
/// Never reads HEAD or git index, never mutates git index.
pub fn rollback(
    root: &Path,
    session_id: &str,
    target_step: Option<usize>,
) -> Result<RollbackReport, ManifestError> {
    let session_dir = crate::state::dir(root).join("manifests").join(session_id);
    if !session_dir.exists() {
        return Ok(RollbackReport::default());
    }

    let mut step_ids = Vec::new();
    for entry in fs::read_dir(&session_dir).map_err(ManifestError::Io)? {
        let entry = entry.map_err(ManifestError::Io)?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(id) = stem.parse::<usize>() {
                    step_ids.push(id);
                }
            }
        }
    }

    // Sort descending (reverse order)
    step_ids.sort_unstable_by(|a, b| b.cmp(a));

    let steps_to_rollback: Vec<usize> = match target_step {
        Some(target) => step_ids.into_iter().filter(|&id| id >= target).collect(),
        None => step_ids,
    };

    if steps_to_rollback.is_empty() {
        return Ok(RollbackReport::default());
    }

    let mut manifests: Vec<StepManifest> = Vec::new();
    for &id in &steps_to_rollback {
        let m = load_manifest(root, session_id, id)?;
        manifests.push(m);
    }

    // --- PHASE 1: PREFLIGHT CHECK ---
    // Preflight every touched path against recorded post-state.
    // If any mismatch or incomplete record is detected, abort without changes!
    let mut expected = std::collections::HashMap::new();
    for m in &manifests {
        let post_records = match &m.post_state {
            Some(records) => records,
            None => return Err(ManifestError::IncompleteStepRecord { step_id: m.step_id }),
        };

        for post in post_records {
            if let Some(later_pre) = expected.get(&post.path) {
                if later_pre != post {
                    return Err(ManifestError::Conflict {
                        path: post.path.clone(),
                        reason: "intervening state differs from earlier post-state".into(),
                    });
                }
                continue;
            }
            let full_path = root.join(&post.path);
            let meta_opt = fs::symlink_metadata(&full_path).ok();

            if !post.exists {
                if meta_opt.is_some() {
                    return Err(ManifestError::Conflict {
                        path: post.path.clone(),
                        reason: "path was created externally since step completion".to_string(),
                    });
                }
            } else {
                let meta = match meta_opt {
                    Some(m) => m,
                    None => {
                        return Err(ManifestError::Conflict {
                            path: post.path.clone(),
                            reason: "path was deleted externally since step completion".to_string(),
                        });
                    }
                };

                let cur_type = if meta.file_type().is_symlink() {
                    FileType::Symlink
                } else if meta.file_type().is_dir() {
                    FileType::Dir
                } else {
                    FileType::File
                };

                if Some(cur_type) != post.file_type {
                    return Err(ManifestError::Conflict {
                        path: post.path.clone(),
                        reason: "file type changed externally".to_string(),
                    });
                }

                if cur_type == FileType::File {
                    let content = fs::read(&full_path).map_err(ManifestError::Io)?;
                    let mut hasher = Sha256::new();
                    hasher.update(&content);
                    let cur_hash = format!("{:x}", hasher.finalize());

                    if Some(cur_hash) != post.sha256 {
                        return Err(ManifestError::Conflict {
                            path: post.path.clone(),
                            reason: "file content modified externally since step completion"
                                .to_string(),
                        });
                    }
                } else if cur_type == FileType::Symlink {
                    let target = fs::read_link(&full_path).map_err(ManifestError::Io)?;
                    if Some(target) != post.symlink_target {
                        return Err(ManifestError::Conflict {
                            path: post.path.clone(),
                            reason: "symlink target modified externally".to_string(),
                        });
                    }
                }
            }
        }
        for pre in &m.pre_state {
            if let Some(blob) = &pre.blob_ref {
                read_blob(root, blob)?;
            }
            expected.insert(pre.path.clone(), pre.clone());
        }
    }

    // --- PHASE 2: RESTORATION IN REVERSE ORDER ---
    let mut restored_paths = Vec::new();
    let mut removed_paths = Vec::new();

    for m in &manifests {
        for pre in &m.pre_state {
            let full_path = root.join(&pre.path);

            if !pre.exists {
                // Agent-created file: remove it
                if full_path.exists() || fs::symlink_metadata(&full_path).is_ok() {
                    let res = if full_path.is_dir()
                        && !fs::symlink_metadata(&full_path)
                            .map(|m| m.file_type().is_symlink())
                            .unwrap_or(false)
                    {
                        fs::remove_dir_all(&full_path)
                    } else {
                        fs::remove_file(&full_path)
                    };
                    if let Err(source) = res {
                        return Err(ManifestError::PartialRecovery {
                            restored: restored_paths,
                            failed_path: pre.path.clone(),
                            source,
                        });
                    }
                    removed_paths.push(pre.path.clone());
                }
            } else {
                // File existed before the step: restore bytes/symlink/mode
                if let Some(parent) = full_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }

                match pre.file_type {
                    Some(FileType::File) => {
                        let blob_ref = match &pre.blob_ref {
                            Some(r) => r,
                            None => {
                                return Err(ManifestError::Conflict {
                                    path: pre.path.clone(),
                                    reason: "missing blob reference in pre-state".to_string(),
                                });
                            }
                        };
                        let bytes = read_blob(root, blob_ref)?;
                        if fs::symlink_metadata(&full_path)
                            .is_ok_and(|m| m.file_type().is_symlink())
                        {
                            fs::remove_file(&full_path).map_err(|source| {
                                ManifestError::PartialRecovery {
                                    restored: restored_paths.clone(),
                                    failed_path: pre.path.clone(),
                                    source,
                                }
                            })?;
                        }
                        if let Err(source) = fs::write(&full_path, &bytes) {
                            return Err(ManifestError::PartialRecovery {
                                restored: restored_paths,
                                failed_path: pre.path.clone(),
                                source,
                            });
                        }
                        if let Some(mode) = pre.mode {
                            let _ =
                                fs::set_permissions(&full_path, fs::Permissions::from_mode(mode));
                        }
                        restored_paths.push(pre.path.clone());
                    }
                    Some(FileType::Symlink) => {
                        let target =
                            pre.symlink_target
                                .as_ref()
                                .ok_or_else(|| ManifestError::Conflict {
                                    path: pre.path.clone(),
                                    reason: "missing symlink target in pre-state".into(),
                                })?;
                        if full_path.exists() || fs::symlink_metadata(&full_path).is_ok() {
                            fs::remove_file(&full_path).map_err(|source| {
                                ManifestError::PartialRecovery {
                                    restored: restored_paths.clone(),
                                    failed_path: pre.path.clone(),
                                    source,
                                }
                            })?;
                        }
                        if let Err(source) = symlink(target, &full_path) {
                            return Err(ManifestError::PartialRecovery {
                                restored: restored_paths,
                                failed_path: pre.path.clone(),
                                source,
                            });
                        }
                        restored_paths.push(pre.path.clone());
                    }
                    Some(FileType::Dir) => {
                        if let Err(source) = fs::create_dir_all(&full_path) {
                            return Err(ManifestError::PartialRecovery {
                                restored: restored_paths,
                                failed_path: pre.path.clone(),
                                source,
                            });
                        }
                        if let Some(mode) = pre.mode {
                            let _ =
                                fs::set_permissions(&full_path, fs::Permissions::from_mode(mode));
                        }
                        restored_paths.push(pre.path.clone());
                    }
                    None => {}
                }
            }
        }
    }

    Ok(RollbackReport {
        restored_paths,
        removed_paths,
    })
}

/// Finds the latest active session in the project's state manifests and rolls it back.
pub fn rollback_latest(
    root: &Path,
    target_step: Option<usize>,
) -> Result<RollbackReport, ManifestError> {
    let manifests_dir = crate::state::dir(root).join("manifests");
    if !manifests_dir.exists() {
        return Err(ManifestError::ManifestNotFound {
            session_id: "latest".to_string(),
            step_id: 0,
        });
    }

    let mut sessions: Vec<(String, std::time::SystemTime)> = Vec::new();
    for entry in fs::read_dir(&manifests_dir).map_err(ManifestError::Io)? {
        let entry = entry.map_err(ManifestError::Io)?;
        let path = entry.path();
        if path.is_dir() {
            let has_manifest = fs::read_dir(&path)
                .map_err(ManifestError::Io)?
                .filter_map(Result::ok)
                .any(|entry| entry.path().extension().and_then(|s| s.to_str()) == Some("json"));
            if !has_manifest {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            sessions.push((name, mtime));
        }
    }

    if sessions.is_empty() {
        return Err(ManifestError::ManifestNotFound {
            session_id: "latest".to_string(),
            step_id: 0,
        });
    }

    sessions.sort_by_key(|a| std::cmp::Reverse(a.1));
    let latest_session = &sessions[0].0;

    rollback(root, latest_session, target_step)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GcReport {
    pub pruned_manifests: usize,
    pub pruned_blobs: usize,
}

/// Prunes expired completed manifests and unreferenced blobs for `root` (ARCH §4.2).
pub fn prune_state(root: &Path) -> Result<GcReport, ManifestError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    prune_state_at(root, now)
}

/// Deterministic variant of [`prune_state`] using `now_secs` as the current Unix timestamp.
pub fn prune_state_at(root: &Path, now_secs: u64) -> Result<GcReport, ManifestError> {
    let mut report = GcReport::default();
    let mut all_refs_known = true;
    let mut referenced_blobs: HashSet<String> = HashSet::new();

    let manifests_root = crate::state::manifests_dir(root);
    if manifests_root.exists() {
        for entry in fs::read_dir(&manifests_root).map_err(ManifestError::Io)? {
            let entry = entry.map_err(ManifestError::Io)?;
            let session_dir = entry.path();
            if !session_dir.is_dir() {
                continue;
            }
            prune_session_manifests(
                &session_dir,
                now_secs,
                &mut report.pruned_manifests,
                &mut referenced_blobs,
                &mut all_refs_known,
            )?;
        }
    }

    let recovery_root = crate::state::recovery_dir(root);
    if recovery_root.exists() {
        collect_recovery_blobs(&recovery_root, &mut referenced_blobs, &mut all_refs_known);
    }

    // Never evict a potentially required backup if any manifest or recovery snapshot failed to parse.
    if all_refs_known {
        let blobs_root = crate::state::blobs_dir(root);
        if blobs_root.exists() {
            for entry in fs::read_dir(&blobs_root).map_err(ManifestError::Io)? {
                let entry = entry.map_err(ManifestError::Io)?;
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !referenced_blobs.contains(name) && fs::remove_file(&path).is_ok() {
                    report.pruned_blobs += 1;
                }
            }
        }
    }

    Ok(report)
}

fn manifest_timestamp(path: &Path, manifest: &StepManifest) -> u64 {
    if manifest.timestamp > 0 {
        return manifest.timestamp;
    }
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn collect_manifest_blobs(manifest: &StepManifest, out: &mut HashSet<String>) {
    for rec in &manifest.pre_state {
        if let Some(blob) = &rec.blob_ref {
            out.insert(blob.clone());
        }
        if let Some(sha) = &rec.sha256 {
            out.insert(sha.clone());
        }
    }
    if let Some(post) = &manifest.post_state {
        for rec in post {
            if let Some(blob) = &rec.blob_ref {
                out.insert(blob.clone());
            }
            if let Some(sha) = &rec.sha256 {
                out.insert(sha.clone());
            }
        }
    }
}

fn prune_session_manifests(
    session_dir: &Path,
    now_secs: u64,
    pruned_manifests: &mut usize,
    referenced_blobs: &mut HashSet<String>,
    all_refs_known: &mut bool,
) -> Result<(), ManifestError> {
    let mut completed: Vec<(usize, PathBuf, StepManifest, u64)> = Vec::new();
    let mut total_entries = 0usize;

    for entry in fs::read_dir(session_dir).map_err(ManifestError::Io)? {
        let entry = entry.map_err(ManifestError::Io)?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        total_entries += 1;
        let Ok(content) = fs::read_to_string(&path) else {
            *all_refs_known = false;
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<StepManifest>(&content) else {
            *all_refs_known = false;
            continue;
        };
        if manifest.post_state.is_none() {
            // Incomplete / unresolved recovery manifests are never expired.
            collect_manifest_blobs(&manifest, referenced_blobs);
            continue;
        }
        let ts = manifest_timestamp(&path, &manifest);
        completed.push((manifest.step_id, path, manifest, ts));
    }

    // Keep the newest MAX_SESSION_COMPLETED_MANIFESTS completed steps that are not expired.
    completed.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut kept_completed = 0usize;
    let mut removed_in_session = 0usize;
    for (_, path, manifest, ts) in completed {
        let expired = now_secs.saturating_sub(ts) > crate::state::MANIFEST_RETENTION_SECS;
        let over_cap = kept_completed >= crate::state::MAX_SESSION_COMPLETED_MANIFESTS;
        if expired || over_cap {
            if fs::remove_file(&path).is_ok() {
                *pruned_manifests += 1;
                removed_in_session += 1;
                continue;
            }
            *all_refs_known = false;
        } else {
            kept_completed += 1;
        }
        collect_manifest_blobs(&manifest, referenced_blobs);
    }

    if total_entries > 0 && removed_in_session == total_entries {
        let _ = fs::remove_dir(session_dir);
    }
    Ok(())
}

fn collect_recovery_blobs(
    recovery_root: &Path,
    referenced_blobs: &mut HashSet<String>,
    all_refs_known: &mut bool,
) {
    let Ok(sessions) = fs::read_dir(recovery_root) else {
        *all_refs_known = false;
        return;
    };
    for session in sessions {
        let Ok(session) = session else {
            *all_refs_known = false;
            continue;
        };
        let path = session.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&path) else {
            *all_refs_known = false;
            continue;
        };
        for file in files {
            let Ok(file) = file else {
                *all_refs_known = false;
                continue;
            };
            let fpath = file.path();
            if fpath.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = fs::read(&fpath) else {
                *all_refs_known = false;
                continue;
            };
            let Ok(snap) = serde_json::from_slice::<crate::manifest::GuardSetSnapshot>(&bytes)
            else {
                *all_refs_known = false;
                continue;
            };
            for rec in snap.files.values() {
                if let Some(sha) = &rec.sha256 {
                    referenced_blobs.insert(sha.clone());
                }
            }
        }
    }
}
