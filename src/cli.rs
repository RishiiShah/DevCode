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

    #[command(subcommand)]
    pub command: Option<Command>,

    #[arg(value_name = "PROMPT")]
    pub prompt: Option<String>,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Roll back the entire latest session (or step N and later)
    Rollback {
        /// Restore the state before this step
        #[arg(long)]
        step: Option<usize>,
    },
}

impl Cli {
    pub fn apply_provider_preset(&self, config: &mut crate::config::Config) {
        if self.provider.as_deref() == Some("nim") {
            if let Some(provider) = &mut config.provider {
                provider.apply_nim_preset();
            }
        }
    }
}
