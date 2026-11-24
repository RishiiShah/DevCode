use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskSummary {
    pub steps: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub files_edited: usize,
    pub files_changed_by_commands: usize,
    pub files_outside_scope: usize,
    pub dependencies_added: usize,
}

impl TaskSummary {
    /// Formats the end-of-task summary line matching the spec:
    /// "steps: N, tokens in: X, tokens out: Y, files edited: F, outside scope: S, dependencies added: D"
    pub fn format_line(&self) -> String {
        format!(
            "steps: {}, tokens in: {}, tokens out: {}, files edited: {}, files changed by commands: {}, outside scope: {}, dependencies added: {}",
            self.steps,
            self.tokens_in,
            self.tokens_out,
            self.files_edited,
            self.files_changed_by_commands,
            self.files_outside_scope,
            self.dependencies_added
        )
    }
}
