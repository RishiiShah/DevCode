use crate::gate::CheckStatus;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    FileEdit,
    CommandFileChange,
    RunCommand,
    DependencyJustification,
    OutsideScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolLogEntry {
    pub order: usize,
    pub kind: ToolKind,
    pub detail: String,
    pub exit_code: Option<i32>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolLog {
    pub entries: Vec<ToolLogEntry>,
    #[serde(default)]
    pub test_workspace_hash: Option<String>,
}

impl ToolLog {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            test_workspace_hash: None,
        }
    }

    pub fn record_file_edit(&mut self, path: &str, timestamp: u64) {
        let order = self.entries.len() + 1;
        self.entries.push(ToolLogEntry {
            order,
            kind: ToolKind::FileEdit,
            detail: path.to_string(),
            exit_code: None,
            timestamp,
        });
    }

    pub fn record_command_file_change(&mut self, path: &str, timestamp: u64) {
        let order = self.entries.len() + 1;
        self.entries.push(ToolLogEntry {
            order,
            kind: ToolKind::CommandFileChange,
            detail: path.to_string(),
            exit_code: None,
            timestamp,
        });
    }

    pub fn record_command(&mut self, command: &str, exit_code: Option<i32>, timestamp: u64) {
        let order = self.entries.len() + 1;
        self.entries.push(ToolLogEntry {
            order,
            kind: ToolKind::RunCommand,
            detail: command.to_string(),
            exit_code,
            timestamp,
        });
    }

    pub fn record_outside_scope(&mut self, path: &str, timestamp: u64) {
        let order = self.entries.len() + 1;
        self.entries.push(ToolLogEntry {
            order,
            kind: ToolKind::OutsideScope,
            detail: path.to_string(),
            exit_code: None,
            timestamp,
        });
    }

    pub fn record_dependency_justification(
        &mut self,
        file: &str,
        justification: &str,
        timestamp: u64,
    ) {
        let order = self.entries.len() + 1;
        self.entries.push(ToolLogEntry {
            order,
            kind: ToolKind::DependencyJustification,
            detail: format!("{}: {}", file, justification),
            exit_code: None,
            timestamp,
        });
    }

    pub fn red_then_edit_then_green(&self, test_cmd: &str) -> bool {
        let mut red = false;
        let mut edit_after_red = false;
        for entry in &self.entries {
            if entry.kind == ToolKind::RunCommand && entry.detail == test_cmd {
                if entry.exit_code.is_some_and(|code| code != 0) {
                    red = true;
                }
                if entry.exit_code == Some(0) && edit_after_red {
                    return true;
                }
            } else if red && matches!(entry.kind, ToolKind::FileEdit | ToolKind::CommandFileChange)
            {
                edit_after_red = true;
            }
        }
        false
    }

    /// Evaluates the freshness status of a given test command against the log.
    pub fn check_status_for_command(&self, test_cmd: &str) -> CheckStatus {
        let last_edit_order = self
            .entries
            .iter()
            .filter(|e| matches!(e.kind, ToolKind::FileEdit | ToolKind::CommandFileChange))
            .map(|e| e.order)
            .max();

        // Find the latest execution of the test command
        let latest_cmd_run = self
            .entries
            .iter()
            .rfind(|e| e.kind == ToolKind::RunCommand && e.detail == test_cmd);

        let run = match latest_cmd_run {
            Some(r) => r,
            None => return CheckStatus::NotRun,
        };

        if run.exit_code != Some(0) {
            return CheckStatus::Failed(run.exit_code.unwrap_or(1));
        }

        if let Some(edit_order) = last_edit_order {
            if run.order < edit_order {
                return CheckStatus::Stale;
            }
        }

        CheckStatus::Fresh
    }
}
