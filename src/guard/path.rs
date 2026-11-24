use crate::guard::GuardError;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Canonicalizes `path` relative to `root`, resolving symlinks and `..`.
/// For a path that does not exist, canonicalizes the nearest existing parent
/// then appends the checked remainder. Returns an error if the resulting
/// canonical path is outside `root`.
pub fn canonicalize(root: &Path, path: &Path) -> Result<PathBuf, GuardError> {
    let canonical_root = root.canonicalize().map_err(GuardError::Io)?;

    let rel_path = if path.is_absolute() {
        if let Ok(rel) = path.strip_prefix(&canonical_root) {
            rel
        } else if let Ok(rel) = path.strip_prefix(root) {
            rel
        } else {
            return Err(GuardError::OutsideRoot {
                path: path.to_path_buf(),
                root: canonical_root,
            });
        }
    } else {
        path
    };

    let mut current = canonical_root.clone();

    for comp in rel_path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if current == canonical_root {
                    return Err(GuardError::OutsideRoot {
                        path: current
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| current.clone()),
                        root: canonical_root,
                    });
                }
                current.pop();
            }
            Component::Normal(part) => {
                let candidate = current.join(part);
                if candidate.exists() {
                    let canonical_cand = candidate.canonicalize().map_err(GuardError::Io)?;
                    if !canonical_cand.starts_with(&canonical_root) {
                        return Err(GuardError::OutsideRoot {
                            path: canonical_cand,
                            root: canonical_root,
                        });
                    }
                    current = canonical_cand;
                } else if let Ok(meta) = candidate.symlink_metadata() {
                    // Check if it's a symlink (possibly broken)
                    if meta.file_type().is_symlink() {
                        let link_target = fs::read_link(&candidate).map_err(GuardError::Io)?;
                        let resolved = if link_target.is_absolute() {
                            link_target
                        } else {
                            current.join(link_target)
                        };
                        let normalized = normalize_path(&resolved);
                        if !normalized.starts_with(&canonical_root) {
                            return Err(GuardError::OutsideRoot {
                                path: normalized,
                                root: canonical_root,
                            });
                        }
                        current = normalized;
                    } else {
                        current = candidate;
                    }
                } else {
                    current = candidate;
                }
            }
            Component::RootDir | Component::Prefix(_) => {}
        }
    }

    if !current.starts_with(&canonical_root) {
        return Err(GuardError::OutsideRoot {
            path: current,
            root: canonical_root,
        });
    }

    Ok(current)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir => out.push(Component::RootDir),
            Component::Prefix(p) => out.push(Component::Prefix(p)),
        }
    }
    out
}

/// Checks whether a path resolves into `.devcode` under root (case-insensitively).
pub fn is_devcode_path(root: &Path, path: &Path) -> bool {
    let canonical_root = match root.canonicalize() {
        Ok(r) => r,
        Err(_) => return false,
    };

    let canonical_path = match canonicalize(root, path) {
        Ok(p) => p,
        Err(_) => return false,
    };

    deny_devcode(&canonical_root, &canonical_path).is_err()
}

/// Denies any canonical path inside `<canonical_root>/.devcode` (case-insensitively).
pub fn deny_devcode(canonical_root: &Path, canonical_path: &Path) -> Result<(), GuardError> {
    if let Ok(rel) = canonical_path.strip_prefix(canonical_root) {
        if let Some(Component::Normal(first)) = rel.components().next() {
            if first.to_string_lossy().eq_ignore_ascii_case(".devcode") {
                return Err(GuardError::DevcodeAccessDenied {
                    path: canonical_path.to_path_buf(),
                });
            }
        }
    }
    Ok(())
}

/// Checks that `path` is within `root` and not inside `.devcode/`.
pub fn check_write_path(root: &Path, path: &Path) -> Result<PathBuf, GuardError> {
    let canonical = canonicalize(root, path)?;
    let canonical_root = root.canonicalize().map_err(GuardError::Io)?;
    deny_devcode(&canonical_root, &canonical)?;
    Ok(canonical)
}
