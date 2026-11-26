pub use crate::profile::Profile;
use clap::{Parser, Subcommand};

fn parse_provider_preset(name: &str) -> Result<String, String> {
    if crate::config::preset(name).is_some() {
        Ok(name.to_owned())
    } else {
        Err(format!(
            "unknown provider '{name}'; choose one of: {}",
            crate::config::PRESETS.map(|p| p.name).join(", ")
        ))
    }
}

#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(
    name = "devcode",
    version,
    about = "DevCode agentic harness",
    long_about = None
)]
pub struct Cli {
    #[arg(long, value_enum, default_value_t = Profile::Build)]
    pub profile: Profile,

    /// Startup provider preset (`nim`, `gemini`). Fills connector, base URL, and key env.
    #[arg(long, value_parser = parse_provider_preset)]
    pub provider: Option<String>,

    /// Automatically approve proposed file edits
    #[arg(short = 'y', long = "yes", default_value_t = false)]
    pub yes: bool,

    /// Append metadata-only agent step traces
    #[arg(long, default_value_t = false)]
    pub trace: bool,

    /// Use the plain line-based REPL instead of the full-screen interface
    #[arg(long, default_value_t = false)]
    pub plain: bool,

    /// Continue the most recent conversation in this folder
    #[arg(short = 'c', long = "continue", conflicts_with = "resume")]
    pub continue_last: bool,

    /// Resume a saved conversation by ID; without an ID, pick one from a list
    #[arg(long, value_name = "ID", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,

    #[arg(value_name = "PROMPT")]
    pub prompt: Option<String>,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Extract a retained pre-command snapshot without modifying project files
    Recover { session: String, step: usize },
    /// Manage stored provider credentials
    Auth {
        #[command(subcommand)]
        action: AuthCommand,
    },
    /// Roll back the entire latest session (or step N and later)
    Rollback {
        /// Restore the state before this step
        #[arg(long)]
        step: Option<usize>,
    },
    /// Prune expired checkpoint manifests and unreferenced blobs
    Gc,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum AuthCommand {
    /// Store a connector credential. Do not paste the key with the command; wait for the hidden prompt.
    Login {
        name: Option<String>,
        #[arg(long)]
        with_stdin: bool,
        /// Paste-code Google OAuth. Only `gemini` is supported.
        #[arg(long)]
        oauth: bool,
    },
    /// Select a stored credential. Does not change the provider.
    Switch { name: String },
    /// List stored connector names
    List,
    /// Remove a connector credential
    Logout { name: String },
}

impl Cli {
    pub fn apply_provider_preset(&self, config: &mut crate::config::Config) {
        if let Some(name) = self.provider.as_deref() {
            match &mut config.provider {
                Some(provider) => provider.apply_named_preset(name),
                None => config.provider = crate::config::provider_from_preset(name),
            }
        }
        if let Some(provider) = &mut config.provider {
            provider.infer_connector();
        }
    }
}
