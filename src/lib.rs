pub mod agent;
pub mod approval;
pub mod cli;
pub mod config;
pub mod credentials;
pub mod discipline;
pub mod gate;
pub mod guard;
pub mod manifest;
pub mod profile;
pub mod tools;
pub mod tui;

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
    pub progress: Option<agent::ProgressSink>,
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
            progress: None,
        }
    }
    pub fn prompt(&self, prompt: &str) -> Result<agent::LoopResult, DevCodeError> {
        let limits = agent::LoopLimits {
            context_window: self.config.provider.as_ref().map(|p| p.context_window),
            trace: self.trace,
            progress: self.progress.clone(),
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
    /// Re-reads `.devcode.json` and rebuilds the model; keeps tools and approval state.
    pub fn reload(&mut self, cli: &Cli) -> Result<(), DevCodeError> {
        let (config, model) = load_provider(&self.root, cli)?;
        self.config = config;
        self.model = model;
        Ok(())
    }
    pub fn has_provider(&self) -> bool {
        self.config.provider.is_some()
    }
    pub fn provider_label(&self) -> String {
        match &self.config.provider {
            Some(p) => format!(
                "{} · {}",
                p.connector.as_deref().unwrap_or("custom"),
                p.model
            ),
            None => "no provider".into(),
        }
    }
    pub fn connections(&self) -> Result<Vec<String>, DevCodeError> {
        let dir = credentials::config_dir().map_err(|e| DevCodeError::Config(e.to_string()))?;
        credentials::ensure_outside_workspace(&self.root, &dir)
            .map_err(|e| DevCodeError::Config(e.to_string()))?;
        Ok(credentials::load(&dir)
            .map_err(|e| DevCodeError::Config(e.to_string()))?
            .keys()
            .map(|name| format!("{} stored", name))
            .collect())
    }
    pub fn connect(&mut self, name: Option<&str>) -> Result<(), DevCodeError> {
        use std::io::IsTerminal;
        self.connect_with(name, std::io::stdin().is_terminal(), || {
            guard::tty::read_secret().map_err(|e| DevCodeError::Config(e.to_string()))
        })
    }
    pub fn connect_with<F>(
        &mut self,
        name: Option<&str>,
        is_tty: bool,
        read: F,
    ) -> Result<(), DevCodeError>
    where
        F: FnOnce() -> Result<String, DevCodeError>,
    {
        let mut provider = self.config.provider.clone().ok_or_else(|| {
            DevCodeError::Config("provider.model is missing from configuration".into())
        })?;
        if provider.model.trim().is_empty() {
            return Err(DevCodeError::Config(
                "provider.model is missing from configuration".into(),
            ));
        }
        if !is_tty {
            return Err(DevCodeError::Config(
                "/connect requires a TTY for hidden input".into(),
            ));
        }
        let name = name
            .map(str::to_owned)
            .or_else(|| provider.connector.clone())
            .ok_or_else(|| DevCodeError::Config("connector name required".into()))?;
        let dir = credentials::config_dir().map_err(|e| DevCodeError::Config(e.to_string()))?;
        credentials::ensure_outside_workspace(&self.root, &dir)
            .map_err(|e| DevCodeError::Config(e.to_string()))?;
        let mut entries =
            credentials::load(&dir).map_err(|e| DevCodeError::Config(e.to_string()))?;
        let key = read()?;
        if key.is_empty() {
            return Err(DevCodeError::Config("API key cannot be empty".into()));
        }
        entries.insert(name.clone(), key);
        credentials::save(&dir, &entries).map_err(|e| DevCodeError::Config(e.to_string()))?;
        provider.connector = Some(name);
        self.model = build_model(provider.clone())?;
        self.config.provider = Some(provider);
        Ok(())
    }
}

/// Opens a session whose approval prompts use the given reader and writer (the TUI's channels).
/// A missing or unusable provider does not fail here; the first prompt reports it.
pub fn open_session<R, W>(
    cli: &Cli,
    prompt: approval::ApprovalPrompt<R, W>,
) -> Result<Session, DevCodeError>
where
    R: std::io::BufRead + Send + 'static,
    W: std::io::Write + Send + 'static,
{
    let root = std::env::current_dir().map_err(|e| DevCodeError::Config(e.to_string()))?;
    let (config, model) = load_provider(&root, cli)?;
    let tools = tools::registry_with_context(tools::MutationContext::new(
        root.clone(),
        tools::new_session_id(),
        prompt,
    ));
    let mut session = Session::new(root, config, cli.profile, model, tools);
    session.trace = cli.trace;
    Ok(session)
}

fn load_provider(
    root: &std::path::Path,
    cli: &Cli,
) -> Result<(Config, Box<dyn agent::Model>), DevCodeError> {
    let mut config = load_config(root).map_err(|e| DevCodeError::Config(e.to_string()))?;
    cli.apply_provider_preset(&mut config);
    let model = config
        .provider
        .clone()
        .ok_or_else(|| DevCodeError::Config("provider.model is missing from configuration".into()))
        .and_then(build_model)
        .unwrap_or_else(|err| Box::new(UnavailableModel(err.to_string())));
    Ok((config, model))
}

pub enum RunOutput {
    Rollback(manifest::RollbackReport),
    Prompt(agent::LoopResult),
    Repl(Session),
    Auth(Vec<String>),
}

pub fn run(cli: Cli) -> Result<RunOutput, DevCodeError> {
    let root = std::env::current_dir().map_err(|e| DevCodeError::Config(e.to_string()))?;
    if let Some(Command::Auth { ref action }) = cli.command {
        return auth(&root, &cli, action).map(RunOutput::Auth);
    }
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
    let model = match config
        .provider
        .clone()
        .ok_or_else(|| DevCodeError::Config("provider.model is missing from configuration".into()))
        .and_then(build_model)
    {
        Ok(model) => model,
        Err(err) if cli.prompt.is_none() => Box::new(UnavailableModel(err.to_string())),
        Err(err) => return Err(err),
    };
    let tools = tools::registry(root.clone(), cli.yes);
    let mut session = Session::new(root, config, cli.profile, model, tools);
    session.trace = cli.trace;
    match cli.prompt {
        Some(prompt) => session.prompt(&prompt).map(RunOutput::Prompt),
        None => Ok(RunOutput::Repl(session)),
    }
}

fn auth(
    root: &std::path::Path,
    cli: &Cli,
    action: &cli::AuthCommand,
) -> Result<Vec<String>, DevCodeError> {
    use std::io::{BufRead, IsTerminal};
    let dir = credentials::config_dir().map_err(|e| DevCodeError::Config(e.to_string()))?;
    let absolute = if dir.is_absolute() {
        dir
    } else {
        root.join(dir)
    };
    let mut ancestor = absolute.as_path();
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or_else(|| DevCodeError::Config("invalid credentials directory".into()))?;
    }
    let canonical = ancestor
        .canonicalize()
        .map_err(|e| DevCodeError::Config(e.to_string()))?;
    let workspace = root
        .canonicalize()
        .map_err(|e| DevCodeError::Config(e.to_string()))?;
    if canonical.starts_with(&workspace) {
        return Err(DevCodeError::Config(
            "credentials directory must be outside the workspace".into(),
        ));
    }
    let mut entries =
        credentials::load(&absolute).map_err(|e| DevCodeError::Config(e.to_string()))?;
    match action {
        cli::AuthCommand::List => Ok(entries
            .keys()
            .map(|name| format!("{} stored", name))
            .collect()),
        cli::AuthCommand::Logout { name } => {
            if entries.remove(name).is_some() {
                credentials::save(&absolute, &entries)
                    .map_err(|e| DevCodeError::Config(e.to_string()))?;
            }
            Ok(Vec::new())
        }
        cli::AuthCommand::Login { name, with_stdin } => {
            let configured = load_config(root)
                .map_err(|e| DevCodeError::Config(e.to_string()))?
                .provider
                .and_then(|p| p.connector);
            let name = name
                .clone()
                .or_else(|| (cli.provider.as_deref() == Some("nim")).then(|| "nim".to_string()))
                .or(configured)
                .ok_or_else(|| DevCodeError::Config("connector name required".into()))?;
            let stdin = std::io::stdin();
            let key = if *with_stdin {
                let mut key = String::new();
                stdin
                    .lock()
                    .read_line(&mut key)
                    .map_err(|e| DevCodeError::Config(e.to_string()))?;
                key.trim_end_matches(['\r', '\n']).to_string()
            } else if stdin.is_terminal() {
                guard::tty::read_secret().map_err(|e| DevCodeError::Config(e.to_string()))?
            } else {
                return Err(DevCodeError::Config(
                    "use --with-stdin when no TTY is available".into(),
                ));
            };
            if key.is_empty() {
                return Err(DevCodeError::Config("API key cannot be empty".into()));
            }
            entries.insert(name, key);
            credentials::save(&absolute, &entries)
                .map_err(|e| DevCodeError::Config(e.to_string()))?;
            Ok(Vec::new())
        }
    }
}

struct UnavailableModel(String);
impl agent::Model for UnavailableModel {
    fn complete(&self, _: &agent::Request) -> Result<agent::Response, agent::ModelError> {
        Err(agent::ModelError::Other(self.0.clone()))
    }
}
fn build_model(provider: config::ProviderConfig) -> Result<Box<dyn agent::Model>, DevCodeError> {
    if provider.model.trim().is_empty() {
        return Err(DevCodeError::Config(
            "provider.model is missing from configuration".into(),
        ));
    }
    let model = agent::openai_compat::OpenAiCompat::new(provider.clone())
        .map_err(|e| DevCodeError::Config(e.to_string()))?;
    if let Some(rpm) = provider.max_rpm {
        Ok(Box::new(agent::session::RateLimitedModel::new(
            Box::new(model),
            std::time::Duration::from_secs_f64(60.0 / f64::from(rpm)),
        )))
    } else {
        Ok(Box::new(model))
    }
}
