pub mod audit;
pub mod classifier;
pub mod tool_log;

pub use audit::{append_skip_tests_audit, verify_audit_chain, AuditEntry, GENESIS_PREV_SHA};
pub use classifier::{classify_diff, ChangeClass};
pub use tool_log::{ToolKind, ToolLog, ToolLogEntry};

use crate::config::Config;
use globset::Glob;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Fresh,
    Stale,
    NotRun,
    Failed(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationSummary {
    pub test_command: Option<String>,
    pub status: CheckStatus,
    pub classification: ChangeClass,
    pub test_files_touched: Vec<PathBuf>,
    pub agent_test_paths: Vec<PathBuf>,
    pub test_lines_added: usize,
    pub test_lines_removed: usize,
    pub existing_tests_red_green: bool,
}

impl fmt::Display for VerificationSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Verification Summary:")?;
        let cmd = self.test_command.as_deref().unwrap_or("<none configured>");
        let status_str = match &self.status {
            CheckStatus::Fresh => "fresh (passed after last edit)".to_string(),
            CheckStatus::Stale => "stale (edits were made after this run)".to_string(),
            CheckStatus::NotRun => "not run".to_string(),
            CheckStatus::Failed(code) => format!("failed (exit code: {})", code),
        };
        writeln!(f, "  Test Command: '{}' -> [{}]", cmd, status_str)?;
        let class_str = match self.classification {
            ChangeClass::ClassA => "Class A (Behavior/API/Config: tests required)",
            ChangeClass::ClassC => "Class C (Whitespace/Inert: exempt from tests)",
        };
        writeln!(f, "  Classification: {}", class_str)?;
        if !self.test_files_touched.is_empty() {
            writeln!(f, "  Test Files Touched: {:?}", self.test_files_touched)?;
        }
        if !self.agent_test_paths.is_empty() {
            let paths = self
                .agent_test_paths
                .iter()
                .map(|p| p.to_string_lossy())
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(f, "Tests changed by the agent: {} file(s), +{}/-{} lines: {}. Review them; passing tests here were written in the same run.", self.agent_test_paths.len(), self.test_lines_added, self.test_lines_removed, paths)?;
        } else if self.existing_tests_red_green {
            writeln!(f, "No tests changed by the agent. Gate satisfied by a failing-then-passing run of the existing tests.")?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum GateError {
    AuditChainBroken {
        line: usize,
        expected: String,
        found: String,
    },
    AuditLogTampered(String),
    MissingTestAssertion(String),
    TestCommandNotRun,
    TestCommandStale,
    TestCommandFailed(i32),
    NoTestCommandConfigured,
    Io(std::io::Error),
}

impl fmt::Display for GateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateError::AuditChainBroken {
                line,
                expected,
                found,
            } => {
                write!(
                    f,
                    "audit chain broken at line {}: expected prev_sha '{}', found '{}'",
                    line, expected, found
                )
            }
            GateError::AuditLogTampered(msg) => {
                write!(f, "audit log tampered or corrupted: {}", msg)
            }
            GateError::MissingTestAssertion(msg) => write!(f, "gate rejected: {}", msg),
            GateError::TestCommandNotRun => write!(f, "gate rejected: test command was not run"),
            GateError::TestCommandStale => {
                write!(
                    f,
                    "gate rejected: test command is stale (edits occurred after last test run)"
                )
            }
            GateError::TestCommandFailed(code) => {
                write!(
                    f,
                    "gate rejected: test command failed with exit code {}",
                    code
                )
            }
            GateError::NoTestCommandConfigured => {
                write!(
                    f,
                    "gate rejected: no test command configured for Class A change"
                )
            }
            GateError::Io(e) => write!(f, "I/O error in gate: {}", e),
        }
    }
}

impl std::error::Error for GateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GateError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Evaluates gate status for finalization.
pub fn evaluate_gate(
    root: &Path,
    config: &Config,
    tool_log: &ToolLog,
    modified_files: &[(PathBuf, String, String)],
    skip_tests: bool,
    skip_reason: Option<&str>,
    diff_sha: &str,
) -> Result<VerificationSummary, GateError> {
    // 1. Verify audit chain at start and before finalization
    verify_audit_chain(root)?;

    // 2. Classify diff
    let classification = classify_diff(modified_files);

    // 3. Find matching test files in modified_files
    let test_globs = &config.verification.test_globs;
    let mut matchers = Vec::new();
    for g in test_globs {
        if let Ok(glob) = Glob::new(g) {
            matchers.push(glob.compile_matcher());
        }
    }

    let mut test_files_touched = Vec::new();
    for (path, _, _) in modified_files {
        if matchers.iter().any(|m| m.is_match(path)) {
            test_files_touched.push(path.clone());
        }
    }

    // 4. Determine test command status from tool log
    let configured_cmd = config.verification.test_command.as_deref();
    let status = match configured_cmd {
        None => CheckStatus::NotRun,
        Some(cmd) => tool_log.check_status_for_command(cmd),
    };

    let (agent_test_paths, test_lines_added, test_lines_removed) =
        agent_test_changes(root, config, tool_log, modified_files);
    let existing_tests_red_green = classification == ChangeClass::ClassA
        && status == CheckStatus::Fresh
        && test_files_touched.is_empty()
        && configured_cmd.is_some_and(|cmd| tool_log.red_then_edit_then_green(cmd));
    let summary = VerificationSummary {
        test_command: configured_cmd.map(|s| s.to_string()),
        status: status.clone(),
        classification,
        test_files_touched: test_files_touched.clone(),
        agent_test_paths,
        test_lines_added,
        test_lines_removed,
        existing_tests_red_green,
    };

    // If Class C (whitespace only): exempt from tests
    if classification == ChangeClass::ClassC {
        return Ok(summary);
    }

    // If --skip-tests provided: record audit entry and allow
    if skip_tests {
        let touched_paths: Vec<PathBuf> =
            modified_files.iter().map(|(p, _, _)| p.clone()).collect();
        let reason = skip_reason.unwrap_or("user override --skip-tests");
        append_skip_tests_audit(root, "user_cli_flag", &touched_paths, diff_sha, reason)
            .map_err(GateError::Io)?;
        return Ok(summary);
    }

    // Class A rules v0:
    // (a) Configured test command must have run after last edit and exited 0 (Fresh)
    match &status {
        CheckStatus::Fresh => {}
        CheckStatus::Stale => return Err(GateError::TestCommandStale),
        CheckStatus::NotRun => return Err(GateError::TestCommandNotRun),
        CheckStatus::Failed(code) => return Err(GateError::TestCommandFailed(*code)),
    }

    // (b) Diff must touch a file matching test_globs
    if test_files_touched.is_empty()
        && !configured_cmd.is_some_and(|cmd| tool_log.red_then_edit_then_green(cmd))
    {
        return Err(GateError::MissingTestAssertion(
            "No failing run followed by an edit is recorded. Either add or update a test, or run the test command at a point where it fails and then make the fix."
                .to_string(),
        ));
    }

    Ok(summary)
}

/// Reports net line changes for test paths edited by file tools in this session.
pub fn agent_test_changes(
    root: &Path,
    config: &Config,
    log: &ToolLog,
    changes: &[(PathBuf, String, String)],
) -> (Vec<PathBuf>, usize, usize) {
    use similar::{ChangeTag, TextDiff};
    let matchers: Vec<_> = config
        .verification
        .test_globs
        .iter()
        .filter_map(|g| Glob::new(g).ok())
        .map(|g| g.compile_matcher())
        .collect();
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut paths: Vec<_> = log
        .entries
        .iter()
        .filter(|e| e.kind == ToolKind::FileEdit)
        .filter_map(|e| {
            let resolved = crate::guard::canonicalize(root, Path::new(&e.detail)).ok()?;
            let path = resolved.strip_prefix(&canonical_root).ok()?.to_path_buf();
            matchers.iter().any(|m| m.is_match(&path)).then_some(path)
        })
        .collect();
    paths.sort();
    paths.dedup();
    let (mut added, mut removed) = (0, 0);
    for (path, old, new) in changes {
        if paths.binary_search(path).is_ok() {
            for change in TextDiff::from_lines(old, new).iter_all_changes() {
                match change.tag() {
                    ChangeTag::Insert => added += 1,
                    ChangeTag::Delete => removed += 1,
                    ChangeTag::Equal => {}
                }
            }
        }
    }
    (paths, added, removed)
}
