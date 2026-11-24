pub mod agent;
pub mod approval;
pub mod cli;
pub mod config;
pub mod discipline;
pub mod gate;
pub mod guard;
pub mod manifest;
pub mod profile;
pub mod tools;

pub use cli::{Cli, Command};
pub use config::{
    default_test_globs, load_config, parse_config, Config, ConfigError, SandboxConfig,
    VerificationConfig,
};
pub use profile::{handle_slash_command, Profile};

#[derive(Debug, PartialEq, Eq)]
pub enum DevCodeError {
    Config(String),
    NoManifest,
}
impl std::fmt::Display for DevCodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(msg) => write!(f, "{}", msg),
            Self::NoManifest => write!(f, "no manifest found"),
        }
    }
}
impl std::error::Error for DevCodeError {}

pub struct Session {
    pub profile: Profile,
    config: Config,
    root: std::path::PathBuf,
    tools: agent::ToolRegistry,
    model: Box<dyn agent::Model>,
    pub trace: bool,
}
impl Session {
    pub fn new(
        root: std::path::PathBuf,
        config: Config,
        profile: Profile,
        model: Box<dyn agent::Model>,
        tools: agent::ToolRegistry,
    ) -> Self {
        Self {
            profile,
            config,
            root,
            tools,
            model,
            trace: false,
        }
    }
    pub fn prompt(&self, prompt: &str) -> Result<agent::LoopResult, DevCodeError> {
        let limits = agent::LoopLimits {
            context_window: self.config.provider.as_ref().map(|p| p.context_window),
            trace: self.trace,
            ..Default::default()
        };
        agent::run_agent_with_config(
            self.profile,
            self.model.as_ref(),
            &self.tools,
            prompt,
            &limits,
            &self.root,
            &self.config,
        )
        .map_err(|e| DevCodeError::Config(e.to_string()))
    }
}

pub enum RunOutput {
    Rollback(manifest::RollbackReport),
    Prompt(agent::LoopResult),
    Repl(Session),
}

pub fn run(cli: Cli) -> Result<RunOutput, DevCodeError> {
    let root = std::env::current_dir().map_err(|e| DevCodeError::Config(e.to_string()))?;
    if let Some(Command::Rollback { step }) = cli.command {
        return manifest::rollback_latest(&root, step)
            .map(RunOutput::Rollback)
            .map_err(|e| match e {
                manifest::ManifestError::ManifestNotFound { .. } => DevCodeError::NoManifest,
                other => DevCodeError::Config(other.to_string()),
            });
    }
    let mut config = load_config(&root).map_err(|e| DevCodeError::Config(e.to_string()))?;
    cli.apply_provider_preset(&mut config);
    let provider = config
        .provider
        .clone()
        .ok_or_else(|| DevCodeError::Config("provider configuration is missing".into()))?;
    let model = agent::openai_compat::OpenAiCompat::new(provider.clone())
        .map_err(|e| DevCodeError::Config(e.to_string()))?;
    let model: Box<dyn agent::Model> = if let Some(rpm) = provider.max_rpm {
        Box::new(agent::session::RateLimitedModel::new(
            Box::new(model),
            std::time::Duration::from_secs_f64(60.0 / f64::from(rpm)),
        ))
    } else {
        Box::new(model)
    };
    let tools = tools::registry(root.clone(), cli.yes);
    let mut session = Session::new(root, config, cli.profile, model, tools);
    session.trace = cli.trace;
    match cli.prompt {
        Some(prompt) => session.prompt(&prompt).map(RunOutput::Prompt),
        None => Ok(RunOutput::Repl(session)),
    }
}
