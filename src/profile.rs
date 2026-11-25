use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Ask,
    Plan,
    #[default]
    Build,
}

/// Shared by every profile: keeps replies short and stops unrequested work.
const STYLE_RULES: &str = "Style: be brief and direct. Reply to a greeting or small talk in one short line. Answer only what was asked: no lists of what you can do, no description of your setup or configuration, no offers of further help. Do not explore, run commands or edit files unless the request needs it.";

impl Profile {
    pub fn name(&self) -> &'static str {
        match self {
            Profile::Ask => "ask",
            Profile::Plan => "plan",
            Profile::Build => "build",
        }
    }

    /// Returns true if the named tool is allowed in this profile.
    /// In `ask` and `plan`, only read-only exploration tools (`read_file`, `glob`, `grep`) are allowed.
    /// In `build`, all tools are allowed.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        match self {
            Profile::Ask | Profile::Plan => {
                matches!(tool_name, "read_file" | "glob" | "grep")
            }
            Profile::Build => true,
        }
    }

    pub fn system_prompt(&self) -> String {
        format!("{}\n{}", self.role_prompt(), STYLE_RULES)
    }

    fn role_prompt(&self) -> &'static str {
        match self {
            Profile::Ask => {
                "You are DevCode in 'ask' mode. You answer questions about the codebase using read-only tools.\n\
                 You cannot modify files or run mutating commands. If the user asks for code changes, explain what is needed and name the profile to switch to (e.g. '/build' or '/plan'); never switch profiles yourself."
            }
            Profile::Plan => {
                "You are DevCode in 'plan' mode. You explore the codebase and produce structured technical plans using read-only tools.\n\
                 Your output must follow this structured specification format:\n\
                 - Goal: The objective to achieve.\n\
                 - Files: The list of files to be created or modified.\n\
                 - Steps: Concrete step-by-step implementation instructions.\n\
                 - Risks: Potential failure modes or breaking changes.\n\
                 - Proof: How the changes will be deterministically verified."
            }
            Profile::Build => {
                "You are DevCode in 'build' mode. You implement verified, disciplined software changes using file tools and shell commands. When a test command is configured, you must run it before editing; file edits are refused until it has run. Call declare_scope before the first edit. Do not add tests or assertions unless the change needs a new one."
            }
        }
    }
}

/// Parses an interactive REPL slash command. Returns true if the input was a valid
/// profile switch command and updates `profile`.
pub fn handle_slash_command(input: &str, profile: &mut Profile) -> bool {
    let trimmed = input.trim();
    match trimmed {
        "/ask" => {
            *profile = Profile::Ask;
            true
        }
        "/plan" => {
            *profile = Profile::Plan;
            true
        }
        "/build" => {
            *profile = Profile::Build;
            true
        }
        _ => false,
    }
}
