pub mod agent;
pub mod approval;
pub mod auth;
pub mod cli;
pub mod config;
pub mod credentials;
pub mod discipline;
pub mod gate;
pub mod guard;
pub mod manifest;
pub mod profile;
mod settings;
pub mod state;
pub mod tools;
pub mod tui;

pub use cli::{Cli, Command};
pub use config::{
    default_test_globs, load_config, parse_config, provider_from_preset, resolve_config,
    resolve_config_at, Config, ConfigError, SandboxConfig, VerificationConfig,
};
pub use profile::{handle_slash_command, Profile};
pub use settings::{load_user_settings, save_user_settings, UserSettings};

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

/// `DEVCODE_STREAM` gate. `0`, `false`, `off`, and `no` (any case) disable streaming.
/// Unset and every other value leave it on. Callers pass the variable's value;
/// this does not read the process environment.
pub fn stream_enabled(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return true;
    };
    !matches!(
        value.to_ascii_lowercase().as_str(),
        "0" | "false" | "off" | "no"
    )
}

pub struct Session {
    pub profile: Profile,
    config: Config,
    root: std::path::PathBuf,
    tools: agent::ToolRegistry,
    model: Box<dyn agent::Model>,
    pub trace: bool,
    pub progress: Option<agent::ProgressSink>,
    pub cancel: agent::CancelToken,
    /// Conversation so far, carried into the next prompt.
    history: Vec<agent::Message>,
    /// Messages typed during a run; the loop reads them at its next step.
    pub steer: agent::SteerInbox,
}

/// A turn cut short (cancel, step limit) can end on tool calls without results, which providers
/// reject on the next request. Answer each one so the conversation stays valid.
fn close_dangling_tool_calls(messages: &mut Vec<agent::Message>) {
    let Some(last_call) = messages
        .iter()
        .rposition(|m| m.role == agent::Role::Assistant && !m.tool_calls.is_empty())
    else {
        return;
    };
    let answered: std::collections::HashSet<String> = messages[last_call + 1..]
        .iter()
        .filter_map(|m| m.tool_call_id.clone())
        .collect();
    let missing: Vec<String> = messages[last_call]
        .tool_calls
        .iter()
        .map(|call| call.id.clone())
        .filter(|id| !answered.contains(id))
        .collect();
    for id in missing {
        messages.push(agent::Message::tool(
            "Error: not run (the turn stopped)",
            id,
        ));
    }
}
impl Session {
    pub fn new(
        root: std::path::PathBuf,
        config: Config,
        profile: Profile,
        model: Box<dyn agent::Model>,
        tools: agent::ToolRegistry,
    ) -> Self {
        let cancel = tools.cancel_token().unwrap_or_default();
        Self {
            profile,
            config,
            root,
            tools,
            model,
            trace: false,
            progress: None,
            cancel,
            history: Vec::new(),
            steer: Default::default(),
        }
    }
    /// Runs one user turn on top of the conversation so far and records the turn in it.
    pub fn prompt(&mut self, prompt: &str) -> Result<agent::LoopResult, DevCodeError> {
        let limits = agent::LoopLimits {
            history: self.history.clone(),
            context_window: self.config.provider.as_ref().map(|p| p.context_window),
            trace: self.trace,
            progress: self.progress.clone(),
            stream: stream_enabled(std::env::var("DEVCODE_STREAM").ok().as_deref()),
            cancel: Some(self.cancel.clone()),
            steer: Some(self.steer.clone()),
            ..Default::default()
        };
        let result = agent::run_agent_with_config(
            self.profile,
            self.model.as_ref(),
            &self.tools,
            prompt,
            &limits,
            &self.root,
            &self.config,
        )
        .map_err(|e| DevCodeError::Config(e.to_string()));
        match &result {
            Ok(run) => self.history = run.messages.clone(),
            Err(e) => {
                // The run's own messages are lost with the error; keep the request and the
                // failure so the next turn ("continue") still knows what was asked.
                self.history.push(agent::Message::user(prompt));
                self.history.push(agent::Message::assistant(
                    format!("(This turn stopped early: {e}. Changes made before the stop are still on disk.)"),
                    Vec::new(),
                ));
            }
        }
        close_dangling_tool_calls(&mut self.history);
        result
    }

    /// Forgets the conversation (`/clear`).
    pub fn clear_history(&mut self) {
        self.history.clear();
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
    /// First-run note when a provider is configured but no API key is available.
    pub fn startup_note(&self) -> Option<String> {
        let provider = self.config.provider.as_ref()?;
        if build_model(provider.clone()).is_ok() {
            return None;
        }
        let connector = provider.connector.as_deref().unwrap_or("nim");
        let name = credentials::config_dir()
            .ok()
            .and_then(|dir| credentials::selected_slot_name(&dir, connector).ok())
            .unwrap_or_else(|| connector.to_owned());
        Some(format!(
            "No API key for {name}. Run /connect to add one, or /provider to choose another provider."
        ))
    }
    /// Configured context window of the current model, in tokens.
    pub fn context_window(&self) -> Option<u64> {
        self.config.provider.as_ref().map(|p| p.context_window)
    }

    /// Status line items saved by `/statusline`, if any.
    pub fn statusline(&self) -> Option<Vec<String>> {
        let dir = credentials::config_dir().ok()?;
        settings::load_user_settings(&dir).ok()?.statusline
    }

    pub fn save_statusline(&self, items: Vec<String>) -> Result<(), DevCodeError> {
        let dir = credentials::config_dir().map_err(|e| DevCodeError::Config(e.to_string()))?;
        let mut user =
            settings::load_user_settings(&dir).map_err(|e| DevCodeError::Config(e.to_string()))?;
        user.statusline = Some(items);
        settings::save_user_settings(&dir, &user).map_err(|e| DevCodeError::Config(e.to_string()))
    }

    pub fn provider_label(&self) -> String {
        match &self.config.provider {
            Some(p) => {
                let mut label = format!(
                    "{} · {}",
                    p.connector.as_deref().unwrap_or("custom"),
                    p.model
                );
                if let Some(effort) = p
                    .extra_body
                    .get("reasoning_effort")
                    .and_then(|v| v.as_str())
                {
                    label.push_str(&format!(" · effort {effort}"));
                }
                label
            }
            None => "no provider".into(),
        }
    }
    /// Rebuilds the model for `provider` and stores it. A missing key does not block the switch;
    /// the warning comes back so the UI can say how to fix it.
    fn apply_provider(&mut self, provider: config::ProviderConfig) -> Option<String> {
        let (model, warning): (Box<dyn agent::Model>, Option<String>) =
            match build_model(provider.clone()) {
                Ok(model) => (model, None),
                Err(e) => {
                    let text = e.to_string();
                    (Box::new(UnavailableModel(text.clone())), Some(text))
                }
            };
        self.model = model;
        self.config.provider = Some(provider);
        warning
    }
    /// Effort values the current model accepts (empty: none selectable).
    pub fn effort_levels(&self) -> Vec<String> {
        self.config
            .provider
            .as_ref()
            .map(|p| p.effort_levels().into_iter().map(String::from).collect())
            .unwrap_or_default()
    }
    pub fn list_models(&self) -> Result<Vec<String>, DevCodeError> {
        self.model
            .list_models()
            .map_err(|e| DevCodeError::Config(e.to_string()))
    }
    /// Switches the model id. Session only; `.devcode.json` is not rewritten.
    pub fn set_model(&mut self, id: &str) -> Result<Option<String>, DevCodeError> {
        let id = id.trim();
        let mut provider = self.config.provider.clone().ok_or_else(|| {
            DevCodeError::Config("no provider configured; run /provider first".into())
        })?;
        if id.is_empty() {
            return Err(DevCodeError::Config("model id cannot be empty".into()));
        }
        provider.model = id.to_owned();
        // An effort the new model does not accept would make every request fail.
        let still_valid = provider
            .extra_body
            .get("reasoning_effort")
            .and_then(|v| v.as_str())
            .is_none_or(|e| provider.effort_levels().contains(&e));
        if !still_valid {
            provider.extra_body.remove("reasoning_effort");
        }
        Ok(self.apply_provider(provider))
    }
    /// Switches to a preset provider with its default model. Session only.
    pub fn set_provider(&mut self, name: &str) -> Result<Option<String>, DevCodeError> {
        let preset = config::preset(name).ok_or_else(|| {
            DevCodeError::Config(format!(
                "unknown provider '{name}'; choose one of: {}",
                config::PRESETS.map(|p| p.name).join(", ")
            ))
        })?;
        let old = self.config.provider.clone();
        let provider = config::ProviderConfig {
            kind: preset.kind,
            base_url: preset.base_url.into(),
            api_key_env: preset.api_key_env.into(),
            connector: Some(preset.name.into()),
            model: preset.default_model.into(),
            context_window: old.as_ref().map_or(32000, |p| p.context_window.max(1)),
            max_rpm: old.and_then(|p| p.max_rpm),
            // Provider-specific options such as reasoning_effort do not carry over.
            extra_body: Default::default(),
        };
        let warning = self.apply_provider(provider.clone());
        if let Ok(dir) = credentials::config_dir() {
            if let Err(e) = crate::settings::persist_provider(&dir, &provider) {
                let msg = format!("could not save default provider: {e}");
                return Ok(Some(match warning {
                    Some(w) => format!("{w}; {msg}"),
                    None => msg,
                }));
            }
        }
        Ok(warning)
    }
    /// Sets or clears reasoning effort (low, medium, high). Session only; Gemini endpoint only.
    pub fn set_effort(&mut self, level: Option<&str>) -> Result<Option<String>, DevCodeError> {
        let mut provider = self.config.provider.clone().ok_or_else(|| {
            DevCodeError::Config("no provider configured; run /provider first".into())
        })?;
        if !provider.supports_effort() {
            return Err(DevCodeError::Config(
                "reasoning effort is not supported for this provider".into(),
            ));
        }
        match level {
            None => {
                provider.extra_body.remove("reasoning_effort");
            }
            Some(l) if provider.effort_levels().contains(&l) => {
                provider
                    .extra_body
                    .insert("reasoning_effort".into(), serde_json::json!(l));
            }
            Some(other) => {
                let levels = provider.effort_levels();
                return Err(DevCodeError::Config(if levels.is_empty() {
                    format!("{} has no selectable reasoning effort", provider.model)
                } else {
                    format!(
                        "{} does not take effort '{other}'; choose default or {}",
                        provider.model,
                        levels.join(", ")
                    )
                }));
            }
        }
        Ok(self.apply_provider(provider))
    }
    /// Handle the TUI uses for /ps and /stop while a run works.
    pub(crate) fn process_control(&self) -> Option<tools::managed::ProcessControl> {
        self.tools.process_control()
    }

    /// Managed background processes, one line each.
    pub fn processes(&self) -> Vec<String> {
        self.tools
            .with_processes(|processes, _| processes.list())
            .unwrap_or_default()
    }

    /// Number of managed background processes still running.
    pub fn running_processes(&self) -> usize {
        self.tools
            .with_processes(|processes, _| processes.running_count())
            .unwrap_or(0)
    }

    /// Stops a managed background process by id.
    pub fn stop_process(&self, id: &str) -> Result<String, String> {
        self.tools
            .with_processes(|processes, log| tools::managed::stop(processes, id, log))
            .unwrap_or_else(|| Err("no managed processes in this session".into()))
    }

    pub fn connections(&self) -> Result<Vec<String>, DevCodeError> {
        let dir = credentials::config_dir().map_err(|e| DevCodeError::Config(e.to_string()))?;
        credentials::ensure_outside_workspace(&self.root, &dir)
            .map_err(|e| DevCodeError::Config(e.to_string()))?;
        let entries = credentials::load(&dir).map_err(|e| DevCodeError::Config(e.to_string()))?;
        let settings = crate::settings::load_user_settings(&dir)
            .map_err(|e| DevCodeError::Config(e.to_string()))?;
        let connector = self
            .config
            .provider
            .as_ref()
            .and_then(|p| p.connector.as_deref());
        let active_buf = connector
            .map(|c| credentials::selected_key(settings.active_slot.as_deref(), c).to_owned());
        let active = active_buf.as_deref().or(settings.active_slot.as_deref());
        let has_oauth = auth::has_oauth_gemini(&dir);
        Ok(credentials::account_lines_with_oauth(
            &entries, active, has_oauth,
        ))
    }
    /// True when a non-empty stored key or OAuth grant is available for `connector`.
    pub fn has_stored_key(&self, connector: &str) -> bool {
        credentials::config_dir().ok().is_some_and(|dir| {
            credentials::stored_key(&dir, connector)
                .ok()
                .flatten()
                .is_some()
                || (connector == "gemini" && auth::has_oauth_gemini(&dir))
        })
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
        let dir = credentials::config_dir().map_err(|e| DevCodeError::Config(e.to_string()))?;
        credentials::ensure_outside_workspace(&self.root, &dir)
            .map_err(|e| DevCodeError::Config(e.to_string()))?;
        let name = match name {
            Some(n) => n.to_owned(),
            None => {
                let connector = provider
                    .connector
                    .clone()
                    .ok_or_else(|| DevCodeError::Config("connector name required".into()))?;
                credentials::selected_slot_name(&dir, &connector)
                    .map_err(|e| DevCodeError::Config(e.to_string()))?
            }
        };
        let named_slot = credentials::parse_slot(&name).is_some();
        let connector = credentials::connector_of(&name).to_owned();
        let mut entries =
            credentials::load(&dir).map_err(|e| DevCodeError::Config(e.to_string()))?;
        let key = read()?;
        if key.is_empty() {
            return Err(DevCodeError::Config("API key cannot be empty".into()));
        }
        entries.insert(name.clone(), key);
        credentials::save(&dir, &entries).map_err(|e| DevCodeError::Config(e.to_string()))?;
        provider.connector = Some(connector.clone());
        // Match /provider: remember this account as the user default (best-effort).
        // Slot selection follows that write so persist_provider cannot drop it.
        let _ = crate::settings::persist_provider(&dir, &provider);
        if named_slot {
            crate::settings::set_active_slot(&dir, Some(&name))
                .map_err(|e| DevCodeError::Config(e.to_string()))?;
        } else {
            // Plain /connect saves the default slot; a selected alias for this connector would shadow it.
            let active = crate::settings::load_user_settings(&dir)
                .map_err(|e| DevCodeError::Config(e.to_string()))?
                .active_slot;
            if active
                .as_deref()
                .is_some_and(|slot| credentials::selected_key(Some(slot), &connector) != connector)
            {
                crate::settings::set_active_slot(&dir, None)
                    .map_err(|e| DevCodeError::Config(e.to_string()))?;
            }
        }
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
    let config = resolve_config(root, cli).map_err(|e| DevCodeError::Config(e.to_string()))?;
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
    if let Some(Command::Recover { ref session, step }) = cli.command {
        return manifest::recover_snapshot(&root, session, step)
            .map(|path| RunOutput::Auth(vec![format!("Recovered files to {}", path.display())]))
            .map_err(|e| DevCodeError::Config(e.to_string()));
    }
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
    let config = resolve_config(&root, &cli).map_err(|e| DevCodeError::Config(e.to_string()))?;
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
        cli::AuthCommand::List => {
            let settings = crate::settings::load_user_settings(&absolute)
                .map_err(|e| DevCodeError::Config(e.to_string()))?;
            let configured = resolve_config(root, cli)
                .ok()
                .and_then(|c| c.provider)
                .and_then(|p| p.connector);
            let active = settings.active_slot.as_deref().or(configured.as_deref());
            let has_oauth = auth::has_oauth_gemini(&absolute);
            Ok(credentials::account_lines_with_oauth(
                &entries, active, has_oauth,
            ))
        }
        cli::AuthCommand::Switch { name } => {
            let has_oauth = name == auth::OAUTH_SLOT && auth::has_oauth_gemini(&absolute);
            if !entries.contains_key(name) && !has_oauth {
                return Err(DevCodeError::Config(format!(
                    "no stored credential for {name}"
                )));
            }
            crate::settings::set_active_slot(&absolute, Some(name))
                .map_err(|e| DevCodeError::Config(e.to_string()))?;
            Ok(vec![format!("active {name}")])
        }
        cli::AuthCommand::Logout { name } => {
            let removed = entries.remove(name).is_some();
            if removed {
                credentials::save(&absolute, &entries)
                    .map_err(|e| DevCodeError::Config(e.to_string()))?;
            }
            if name == auth::OAUTH_SLOT {
                auth::delete_oauth_gemini(&absolute)
                    .map_err(|e| DevCodeError::Config(e.to_string()))?;
            }
            let settings = crate::settings::load_user_settings(&absolute)
                .map_err(|e| DevCodeError::Config(e.to_string()))?;
            if settings.active_slot.as_deref() == Some(name.as_str()) {
                crate::settings::set_active_slot(&absolute, None)
                    .map_err(|e| DevCodeError::Config(e.to_string()))?;
            }
            Ok(Vec::new())
        }
        cli::AuthCommand::Login {
            name,
            with_stdin,
            oauth,
        } => {
            let configured = resolve_config(root, cli)
                .map_err(|e| DevCodeError::Config(e.to_string()))?
                .provider
                .and_then(|p| p.connector);
            let name = name
                .clone()
                .or_else(|| cli.provider.clone())
                .or(configured)
                .ok_or_else(|| DevCodeError::Config("connector name required".into()))?;
            if *oauth && name != "gemini" {
                return Err(DevCodeError::Config(
                    "oauth login is only supported for gemini".into(),
                ));
            }
            if *oauth && name == "gemini" {
                let pkce =
                    auth::generate_pkce().map_err(|e| DevCodeError::Config(e.to_string()))?;
                let stdin = std::io::stdin();
                let no_browser =
                    *with_stdin || !std::env::var("NO_BROWSER").unwrap_or_default().is_empty();
                if !no_browser && stdin.is_terminal() {
                    let server = auth::LoopbackServer::bind()
                        .map_err(|e| DevCodeError::Config(e.to_string()))?;
                    let redirect_uri = server.redirect_uri.clone();
                    let url = auth::authorize_url_with_redirect(&pkce, &redirect_uri);
                    eprintln!(
                        "Opening browser for Google login. If it does not open, visit:\n{url}"
                    );
                    auth::open_browser(&url);
                    let code = server
                        .wait_for_code(&pkce.state, std::time::Duration::from_secs(300))
                        .map_err(DevCodeError::Config)?;
                    auth::login_gemini_with_redirect(
                        &absolute,
                        &code,
                        auth::DEFAULT_TOKEN_URL,
                        &pkce,
                        &redirect_uri,
                    )
                    .map_err(DevCodeError::Config)?;
                } else {
                    eprintln!(
                        "Visit this URL, then paste the authorization code:\n{}",
                        auth::authorize_url(&pkce)
                    );
                    let code = if *with_stdin {
                        let mut code = String::new();
                        stdin
                            .lock()
                            .read_line(&mut code)
                            .map_err(|e| DevCodeError::Config(e.to_string()))?;
                        code.trim().to_string()
                    } else if stdin.is_terminal() {
                        use std::os::fd::AsRawFd;
                        // SAFETY: stdin fd; TCIFLUSH drops typed-ahead bytes before the paste prompt.
                        if unsafe { libc::tcflush(stdin.as_raw_fd(), libc::TCIFLUSH) } != 0 {
                            return Err(DevCodeError::Config(
                                std::io::Error::last_os_error().to_string(),
                            ));
                        }
                        let mut code = String::new();
                        stdin
                            .lock()
                            .read_line(&mut code)
                            .map_err(|e| DevCodeError::Config(e.to_string()))?;
                        code.trim().to_string()
                    } else {
                        return Err(DevCodeError::Config(
                            "use --with-stdin when no TTY is available".into(),
                        ));
                    };
                    if code.is_empty() {
                        return Err(DevCodeError::Config(
                            "authorization code cannot be empty".into(),
                        ));
                    }
                    auth::login_gemini(&absolute, &code, auth::DEFAULT_TOKEN_URL, &pkce)
                        .map_err(DevCodeError::Config)?;
                }
                return Ok(vec![
                    "saved google oauth tokens to oauth-gemini.json (active for gemini when no API key is set, or via `devcode auth switch gemini:oauth`)".into(),
                ]);
            }
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
    let is_gemini = provider.connector.as_deref() == Some("gemini")
        || provider
            .base_url
            .contains("generativelanguage.googleapis.com")
        || provider.base_url.contains("cloudcode-pa.googleapis.com");
    let model: Box<dyn agent::Model> = match provider.kind {
        config::ProviderKind::OpenaiCompat if is_gemini => {
            let oauth_selected = credentials::config_dir()
                .ok()
                .and_then(|dir| crate::settings::load_user_settings(&dir).ok())
                .and_then(|s| s.active_slot)
                .is_some_and(|slot| slot == auth::OAUTH_SLOT);
            if oauth_selected {
                Box::new(
                    agent::code_assist::CodeAssist::new(provider.clone())
                        .map_err(|e| DevCodeError::Config(e.to_string()))?,
                )
            } else {
                match agent::openai_compat::OpenAiCompat::new(provider.clone()) {
                    Ok(m) => Box::new(m),
                    Err(compat_err) => {
                        let has_oauth = credentials::config_dir()
                            .ok()
                            .is_some_and(|dir| auth::has_oauth_gemini(&dir));
                        if has_oauth {
                            Box::new(
                                agent::code_assist::CodeAssist::new(provider.clone())
                                    .map_err(|e| DevCodeError::Config(e.to_string()))?,
                            )
                        } else {
                            return Err(DevCodeError::Config(compat_err.to_string()));
                        }
                    }
                }
            }
        }
        config::ProviderKind::OpenaiCompat => Box::new(
            agent::openai_compat::OpenAiCompat::new(provider.clone())
                .map_err(|e| DevCodeError::Config(e.to_string()))?,
        ),
        config::ProviderKind::Anthropic => Box::new(
            agent::anthropic::Anthropic::new(provider.clone())
                .map_err(|e| DevCodeError::Config(e.to_string()))?,
        ),
    };
    if let Some(rpm) = provider.max_rpm {
        Ok(Box::new(agent::session::RateLimitedModel::new(
            model,
            std::time::Duration::from_secs_f64(60.0 / f64::from(rpm)),
        )))
    } else {
        Ok(model)
    }
}

#[cfg(test)]
mod stream_enabled_tests {
    #[test]
    fn dangling_tool_calls_get_an_answer() {
        use crate::agent::{Message, ToolCall};
        let call = |id: &str| ToolCall {
            id: id.into(),
            name: "read_file".into(),
            arguments: "{}".into(),
            thought_signature: None,
        };
        let mut messages = vec![
            Message::user("go"),
            Message::assistant("", vec![call("a"), call("b")]),
            Message::tool("ok", "a"),
        ];
        super::close_dangling_tool_calls(&mut messages);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[3].tool_call_id.as_deref(), Some("b"));
        super::close_dangling_tool_calls(&mut messages);
        assert_eq!(messages.len(), 4, "already answered calls stay as they are");
    }

    use super::stream_enabled;

    #[test]
    fn unset_and_other_values_enable_streaming() {
        assert!(stream_enabled(None));
        assert!(stream_enabled(Some("")));
        assert!(stream_enabled(Some("1")));
        assert!(stream_enabled(Some("true")));
        assert!(stream_enabled(Some("yes")));
        assert!(stream_enabled(Some("on")));
        assert!(stream_enabled(Some("false ")));
        assert!(stream_enabled(Some(" off")));
    }

    #[test]
    fn off_switches_are_case_insensitive() {
        for value in [
            "0", "false", "FALSE", "False", "off", "OFF", "no", "No", "nO",
        ] {
            assert!(!stream_enabled(Some(value)), "{value}");
        }
    }
}
