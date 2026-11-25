pub use crate::profile::Profile;
use clap::{Parser, Subcommand};

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

    #[arg(long, value_parser = ["nim"])]
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

    #[command(subcommand)]
    pub command: Option<Command>,

    #[arg(value_name = "PROMPT")]
    pub prompt: Option<String>,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Command {
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
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum AuthCommand {
    /// Store a connector credential. Do not paste the key with the command; wait for the hidden prompt.
    Login {
        name: Option<String>,
        #[arg(long)]
        with_stdin: bool,
    },
    /// List stored connector names
    List,
    /// Remove a connector credential
    Logout { name: String },
}

impl Cli {
    pub fn apply_provider_preset(&self, config: &mut crate::config::Config) {
        if self.provider.as_deref() == Some("nim") {
            if let Some(provider) = &mut config.provider {
                provider.apply_nim_preset();
            }
        }
        if let Some(provider) = &mut config.provider {
            provider.infer_connector();
        }
    }
}
