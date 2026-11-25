use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn config_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("DEVCODE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir).join("devcode"));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(PathBuf::from(home).join(".config/devcode"))
}

pub fn load(dir: &Path) -> io::Result<BTreeMap<String, String>> {
    let path = dir.join("credentials.json");
    let mut opts = OpenOptions::new();
    opts.read(true).custom_flags(libc::O_NOFOLLOW);
    let file = match opts.open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e),
    };
    if file.metadata()?.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "credentials file permissions are too open; run chmod 600 {}",
                path.display()
            ),
        ));
    }
    serde_json::from_reader(file)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid credentials file"))
}

pub fn save(dir: &Path, entries: &BTreeMap<String, String>) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = dir.join(format!(".credentials-{}-{nonce}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        serde_json::to_writer(&mut file, entries).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, dir.join("credentials.json"))?;
        fs::File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

/// Account rows for `/connections` and `auth list` (OpenCode-style source column).
/// Never includes key material. Active connector is marked with ` *`.
pub fn account_lines(entries: &BTreeMap<String, String>, active: Option<&str>) -> Vec<String> {
    let mut lines = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for preset in &crate::config::PRESETS {
        seen.insert(preset.name.to_string());
        let stored = entries.contains_key(preset.name);
        let env_ok = !preset.api_key_env.is_empty()
            && std::env::var(preset.api_key_env)
                .ok()
                .is_some_and(|v| !v.is_empty());
        let source = match (stored, env_ok) {
            (true, true) => "stored+env",
            (true, false) => "stored",
            (false, true) => "env",
            (false, false) => "missing",
        };
        let mark = if active == Some(preset.name) {
            " *"
        } else {
            ""
        };
        lines.push(format!("{}  {}{}", preset.name, source, mark));
    }
    for name in entries.keys() {
        if seen.contains(name.as_str()) {
            continue;
        }
        let mark = if active == Some(name.as_str()) {
            " *"
        } else {
            ""
        };
        lines.push(format!("{name}  stored (custom){mark}"));
    }
    lines
}

pub fn ensure_outside_workspace(root: &Path, dir: &Path) -> io::Result<()> {
    let absolute = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        root.join(dir)
    };
    let mut parent = absolute.as_path();
    while !parent.exists() {
        parent = parent.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid credentials directory")
        })?;
    }
    if parent.canonicalize()?.starts_with(root.canonicalize()?) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "credentials directory must be outside the workspace",
        ));
    }
    Ok(())
}
