pub mod env;
pub mod path;
pub mod process;
pub mod tty;

pub use env::{is_sensitive_env_var, strip_env};
pub use path::{canonicalize, check_write_path, deny_devcode, is_devcode_path};
pub use process::{run_bounded, BoundedCollector, RunResult, DEFAULT_OUT_CAP};

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum GuardError {
    OutsideRoot { path: PathBuf, root: PathBuf },
    DevcodeAccessDenied { path: PathBuf },
    Io(std::io::Error),
    Spawn(std::io::Error),
    EmptyCommand,
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GuardError::OutsideRoot { path, root } => {
                write!(
                    f,
                    "access denied: path '{}' escapes repository root '{}'",
                    path.display(),
                    root.display()
                )
            }
            GuardError::DevcodeAccessDenied { path } => {
                write!(
                    f,
                    "access denied: agent write to protected directory '{}'",
                    path.display()
                )
            }
            GuardError::Io(err) => write!(f, "I/O error in guard: {}", err),
            GuardError::Spawn(err) => write!(f, "failed to spawn process: {}", err),
            GuardError::EmptyCommand => write!(f, "cannot execute empty command"),
        }
    }
}

impl std::error::Error for GuardError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GuardError::Io(err) | GuardError::Spawn(err) => Some(err),
            _ => None,
        }
    }
}
