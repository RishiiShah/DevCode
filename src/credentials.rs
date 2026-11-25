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

#[derive(Debug)]
enum StoreMode {
    File,
    Auto,
}

fn store_mode() -> io::Result<StoreMode> {
    match std::env::var("DEVCODE_CREDENTIALS_STORE") {
        Err(std::env::VarError::NotPresent) => Ok(StoreMode::File),
        Err(std::env::VarError::NotUnicode(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "DEVCODE_CREDENTIALS_STORE is not valid Unicode",
        )),
        Ok(value) => parse_store_mode(&value),
    }
}

fn parse_store_mode(value: &str) -> io::Result<StoreMode> {
    match value {
        "file" => Ok(StoreMode::File),
        "auto" => Ok(StoreMode::Auto),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "DEVCODE_CREDENTIALS_STORE must be \"file\" or \"auto\"",
        )),
    }
}

enum Get {
    Found(String),
    Missing,
    Unavailable(io::Error),
}

impl std::fmt::Display for Get {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Get::Found(_) => f.write_str("found"),
            Get::Missing => f.write_str("missing"),
            Get::Unavailable(err) => write!(f, "unavailable: {err}"),
        }
    }
}

impl std::fmt::Debug for Get {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Get::Found(_) => f.write_str("Found(..)"),
            Get::Missing => f.write_str("Missing"),
            Get::Unavailable(err) => write!(f, "Unavailable({err})"),
        }
    }
}

/// `Err` from `get` is corrupt keyring data. Callers must not read the file after that.
trait SecretBlob {
    fn get(&self) -> io::Result<Get>;
    fn set(&self, value: &str) -> io::Result<()>;
}

#[derive(Debug)]
struct KeyringUnavailable {
    message: String,
}

impl std::fmt::Display for KeyringUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for KeyringUnavailable {}

fn keyring_unavailable(message: impl Into<String>) -> io::Error {
    io::Error::other(KeyringUnavailable {
        message: message.into(),
    })
}

fn is_keyring_unavailable(err: &io::Error) -> bool {
    err.get_ref()
        .is_some_and(|inner| inner.is::<KeyringUnavailable>())
}

pub fn load(dir: &Path) -> io::Result<BTreeMap<String, String>> {
    load_from(dir, store_mode()?, &OsKeyring)
}

pub fn save(dir: &Path, entries: &BTreeMap<String, String>) -> io::Result<()> {
    save_from(dir, entries, store_mode()?, &OsKeyring)
}

fn load_file(dir: &Path) -> io::Result<BTreeMap<String, String>> {
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

fn save_file(dir: &Path, entries: &BTreeMap<String, String>) -> io::Result<()> {
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

fn load_from(
    dir: &Path,
    mode: StoreMode,
    blob: &impl SecretBlob,
) -> io::Result<BTreeMap<String, String>> {
    match mode {
        StoreMode::File => load_file(dir),
        StoreMode::Auto => load_with(dir, blob),
    }
}

fn save_from(
    dir: &Path,
    entries: &BTreeMap<String, String>,
    mode: StoreMode,
    blob: &impl SecretBlob,
) -> io::Result<()> {
    match mode {
        StoreMode::File => save_file(dir, entries),
        StoreMode::Auto => save_with(dir, entries, blob),
    }
}

fn load_with(dir: &Path, blob: &impl SecretBlob) -> io::Result<BTreeMap<String, String>> {
    match blob.get()? {
        Get::Found(raw) => parse_entries(&raw),
        Get::Missing | Get::Unavailable(_) => load_file(dir),
    }
}

fn save_with(
    dir: &Path,
    entries: &BTreeMap<String, String>,
    blob: &impl SecretBlob,
) -> io::Result<()> {
    let payload = encode_entries(entries)?;
    match blob.set(&payload) {
        Ok(()) => match fs::remove_file(dir.join("credentials.json")) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        },
        Err(err) if is_keyring_unavailable(&err) => save_file(dir, entries),
        Err(err) => Err(err),
    }
}

fn encode_entries(entries: &BTreeMap<String, String>) -> io::Result<String> {
    let mut json = serde_json::to_string(entries).map_err(io::Error::other)?;
    json.push('\n');
    Ok(json)
}

fn parse_entries(raw: &str) -> io::Result<BTreeMap<String, String>> {
    serde_json::from_str(raw)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid credentials file"))
}

const KEYRING_SERVICE: &str = "devcode";
const KEYRING_ACCOUNT: &str = "credentials";

struct OsKeyring;

enum Classified {
    Missing,
    Unavailable(io::Error),
    Hard(io::Error),
}

fn classify(err: keyring::Error) -> Classified {
    match err {
        keyring::Error::NoEntry => Classified::Missing,
        keyring::Error::NoStorageAccess(inner) | keyring::Error::PlatformFailure(inner) => {
            Classified::Unavailable(keyring_unavailable(inner.to_string()))
        }
        keyring::Error::Ambiguous(_) => {
            Classified::Unavailable(keyring_unavailable("multiple keyring credentials match"))
        }
        keyring::Error::BadEncoding(_) => Classified::Hard(io::Error::new(
            io::ErrorKind::InvalidData,
            "keyring data is not UTF-8",
        )),
        other => Classified::Hard(io::Error::other(other.to_string())),
    }
}

fn get_from_classified(class: Classified) -> io::Result<Get> {
    match class {
        Classified::Missing => Ok(Get::Missing),
        Classified::Unavailable(err) => Ok(Get::Unavailable(err)),
        Classified::Hard(err) => Err(err),
    }
}

fn set_error(err: keyring::Error) -> io::Error {
    match classify(err) {
        Classified::Unavailable(err) | Classified::Hard(err) => err,
        Classified::Missing => io::Error::other("keyring reported no entry"),
    }
}

impl SecretBlob for OsKeyring {
    fn get(&self) -> io::Result<Get> {
        let entry = match keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT) {
            Ok(entry) => entry,
            Err(err) => return get_from_classified(classify(err)),
        };
        match entry.get_password() {
            Ok(secret) => Ok(Get::Found(secret)),
            Err(err) => get_from_classified(classify(err)),
        }
    }

    fn set(&self, value: &str) -> io::Result<()> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(set_error)?;
        entry.set_password(value).map_err(set_error)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    enum ScriptedGet {
        Found(String),
        Missing,
        Unavailable,
        Hard,
    }

    enum ScriptedSet {
        Ok,
        Unavailable,
        Hard,
    }

    struct MemoryBlob {
        get: Mutex<ScriptedGet>,
        set: ScriptedSet,
        sets: Mutex<Vec<String>>,
    }

    impl MemoryBlob {
        fn new(get: ScriptedGet, set: ScriptedSet) -> Self {
            Self {
                get: Mutex::new(get),
                set,
                sets: Mutex::new(Vec::new()),
            }
        }
    }

    impl SecretBlob for MemoryBlob {
        fn get(&self) -> io::Result<Get> {
            match &*self.get.lock().unwrap() {
                ScriptedGet::Found(value) => Ok(Get::Found(value.clone())),
                ScriptedGet::Missing => Ok(Get::Missing),
                ScriptedGet::Unavailable => {
                    Ok(Get::Unavailable(keyring_unavailable("keyring unavailable")))
                }
                ScriptedGet::Hard => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "keyring data is not UTF-8",
                )),
            }
        }

        fn set(&self, value: &str) -> io::Result<()> {
            self.sets.lock().unwrap().push(value.to_string());
            match self.set {
                ScriptedSet::Ok => Ok(()),
                ScriptedSet::Unavailable => Err(keyring_unavailable("keyring unavailable")),
                ScriptedSet::Hard => Err(io::Error::other("keyring rejected credential")),
            }
        }
    }

    struct PanicBlob;

    impl SecretBlob for PanicBlob {
        fn get(&self) -> io::Result<Get> {
            panic!("file mode called blob get");
        }

        fn set(&self, _value: &str) -> io::Result<()> {
            panic!("file mode called blob set");
        }
    }

    fn nim(value: &str) -> BTreeMap<String, String> {
        let mut entries = BTreeMap::new();
        entries.insert("nim".into(), value.into());
        entries
    }

    #[test]
    fn file_mode_never_calls_blob() {
        let dir = tempfile::tempdir().unwrap();
        let entries = nim("file-only");
        save_from(dir.path(), &entries, StoreMode::File, &PanicBlob).unwrap();
        assert_eq!(
            load_from(dir.path(), StoreMode::File, &PanicBlob).unwrap(),
            entries
        );
    }

    #[test]
    fn auto_set_ok_deletes_file() {
        let dir = tempfile::tempdir().unwrap();
        let entries = nim("kept-in-blob");
        save_file(dir.path(), &nim("stale-file")).unwrap();
        let blob = MemoryBlob::new(ScriptedGet::Missing, ScriptedSet::Ok);
        save_from(dir.path(), &entries, StoreMode::Auto, &blob).unwrap();
        assert!(!dir.path().join("credentials.json").exists());
        let stored = blob.sets.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(parse_entries(&stored[0]).unwrap(), entries);
    }

    #[test]
    fn auto_set_unavailable_writes_file() {
        let dir = tempfile::tempdir().unwrap();
        let entries = nim("file-fallback");
        let blob = MemoryBlob::new(ScriptedGet::Missing, ScriptedSet::Unavailable);
        save_from(dir.path(), &entries, StoreMode::Auto, &blob).unwrap();
        assert_eq!(load_file(dir.path()).unwrap(), entries);
        assert_eq!(blob.sets.lock().unwrap().len(), 1);
    }

    #[test]
    fn auto_set_hard_error_leaves_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let secret = "hard-set-secret-17ab";
        let blob = MemoryBlob::new(ScriptedGet::Missing, ScriptedSet::Hard);
        let err = save_from(dir.path(), &nim(secret), StoreMode::Auto, &blob).unwrap_err();
        assert!(!err.to_string().contains(secret));
        assert!(!dir.path().join("credentials.json").exists());
    }

    #[test]
    fn found_beats_file() {
        let dir = tempfile::tempdir().unwrap();
        save_file(dir.path(), &nim("from-file")).unwrap();
        let blob_entries = nim("from-keyring");
        let blob = MemoryBlob::new(
            ScriptedGet::Found(encode_entries(&blob_entries).unwrap()),
            ScriptedSet::Ok,
        );
        assert_eq!(
            load_from(dir.path(), StoreMode::Auto, &blob).unwrap(),
            blob_entries
        );
        assert!(blob.sets.lock().unwrap().is_empty());
        assert_eq!(load_file(dir.path()).unwrap(), nim("from-file"));
    }

    #[test]
    fn missing_or_unavailable_returns_file_without_set() {
        let dir = tempfile::tempdir().unwrap();
        let entries = nim("from-file");
        save_file(dir.path(), &entries).unwrap();
        for scripted in [ScriptedGet::Missing, ScriptedGet::Unavailable] {
            let blob = MemoryBlob::new(scripted, ScriptedSet::Ok);
            assert_eq!(
                load_from(dir.path(), StoreMode::Auto, &blob).unwrap(),
                entries
            );
            assert!(blob.sets.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn found_bad_json_errors_without_file_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let secret = "bad-json-secret-44c1";
        let entries = nim("from-file");
        save_file(dir.path(), &entries).unwrap();
        let blob = MemoryBlob::new(ScriptedGet::Found(format!("{{{secret}")), ScriptedSet::Ok);
        let err = load_from(dir.path(), StoreMode::Auto, &blob).unwrap_err();
        assert!(!err.to_string().contains(secret));
        assert_eq!(load_file(dir.path()).unwrap(), entries);
        assert!(blob.sets.lock().unwrap().is_empty());
    }

    #[test]
    fn hard_get_does_not_fall_back_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let entries = nim("from-file");
        save_file(dir.path(), &entries).unwrap();
        let blob = MemoryBlob::new(ScriptedGet::Hard, ScriptedSet::Ok);
        assert!(load_from(dir.path(), StoreMode::Auto, &blob).is_err());
        assert_eq!(load_file(dir.path()).unwrap(), entries);
        assert!(blob.sets.lock().unwrap().is_empty());
    }

    #[test]
    fn display_has_no_secret() {
        let secret = "display-secret-9f3a";
        let found = Get::Found(secret.to_string());
        assert!(!found.to_string().contains(secret));
        assert!(!format!("{found:?}").contains(secret));

        let mode_err = parse_store_mode(secret).unwrap_err();
        assert!(!mode_err.to_string().contains(secret));

        let bytes = format!("\u{fffd}{secret}").into_bytes();
        let err = match classify(keyring::Error::BadEncoding(bytes)) {
            Classified::Hard(err) => err,
            Classified::Missing | Classified::Unavailable(_) => {
                panic!("bad encoding must be a hard error")
            }
        };
        assert!(!err.to_string().contains(secret));
        assert!(!format!("{err:?}").contains(secret));

        match classify(keyring::Error::Ambiguous(Vec::new())) {
            Classified::Unavailable(err) => {
                assert!(is_keyring_unavailable(&err));
                assert!(!err.to_string().contains("Entry is matched"));
                assert!(!err.to_string().contains(secret));
            }
            Classified::Missing | Classified::Hard(_) => panic!("ambiguous must be unavailable"),
        }
    }

    #[test]
    fn store_mode_accepts_only_file_or_auto() {
        assert!(matches!(parse_store_mode("file").unwrap(), StoreMode::File));
        assert!(matches!(parse_store_mode("auto").unwrap(), StoreMode::Auto));
        assert!(parse_store_mode("").is_err());
        assert!(parse_store_mode("keyring").is_err());
    }
}
