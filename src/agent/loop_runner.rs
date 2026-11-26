use crate::agent::model::{CancelToken, Message, Model, ModelError, Request, Role, StreamChunk};
use crate::agent::tool::ToolRegistry;
use crate::discipline::summary::TaskSummary;
use crate::profile::Profile;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Messages the user sends while a run works. The loop hands them to the model at its next
/// step, as Codex steers a turn, instead of waiting for the run to end.
pub type SteerInbox = Arc<Mutex<Vec<String>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Completed,
    StepLimitExceeded,
    TimeLimitExceeded,
    FormatErrorLimitExceeded,
    RateLimited,
    NoProgress,
    Cancelled,
}

/// Live progress notifications for a UI. Never carries file contents beyond a short preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEvent {
    /// A model request started (1-based step number).
    Thinking { step: usize },
    /// The model emitted reasoning / thinking tokens.
    Reasoning { text: String },
    /// The model produced text (a final answer or commentary before tool calls).
    Assistant { text: String },
    /// A tool call is about to run.
    ToolCall { name: String, summary: String },
    /// A tool call finished; `preview` is the first line(s) of its output.
    ToolResult {
        name: String,
        ok: bool,
        preview: String,
        /// Unified diff when the call edited a file (shown to the user, not the model).
        diff: Option<String>,
    },
    /// Cumulative token usage after a model response.
    Usage { input: u64, output: u64 },
    /// The model request failed on a dropped connection and is being retried from the start;
    /// text streamed by the failed attempt is void.
    Retry {
        attempt: usize,
        max: usize,
        reason: String,
    },
    /// A message the user sent during the run went to the model.
    Steered { text: String },
}

/// Retries for a model request that fails on a dropped connection (reset, timeout, early end).
const NETWORK_RETRIES: usize = 3;

/// Connection-level failures are worth retrying; HTTP status errors were already retried (or
/// are final, like 401) inside the provider.
fn retryable_network_error(reason: &str) -> bool {
    !reason.contains("HTTP ")
}

/// Cloneable callback wrapper so `LoopLimits` keeps `Debug` and `Clone`.
#[derive(Clone)]
pub struct ProgressSink(pub std::sync::Arc<dyn Fn(ProgressEvent) + Send + Sync>);
impl std::fmt::Debug for ProgressSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProgressSink")
    }
}

/// Up to three non-empty lines of a tool result, each cut to 110 chars, indentation kept.
pub fn preview_lines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut out: Vec<String> = lines
        .iter()
        .take(3)
        .map(|l| {
            let l = l.replace('\t', "    ");
            if l.chars().count() > 110 {
                format!("{}…", l.chars().take(110).collect::<String>())
            } else {
                l
            }
        })
        .collect();
    if lines.len() > 3 {
        out.push(format!("… {} more lines", lines.len() - 3));
    }
    out.join("\n")
}

fn short(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// One-line, content-free-ish description of a call: its path/pattern/command, never file bodies.
pub fn call_summary(arguments: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
    for key in ["path", "command", "pattern", "query"] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            return short(text, 100);
        }
    }
    if let Some(files) = value.get("files").and_then(|v| v.as_array()) {
        let names: Vec<_> = files.iter().filter_map(|f| f.as_str()).collect();
        return short(&names.join(", "), 100);
    }
    String::new()
}

#[derive(Debug, Clone)]
pub struct LoopLimits {
    pub max_steps: usize,
    pub max_wall_time: Duration,
    pub max_consecutive_format_errors: usize,
    pub context_window: Option<u64>,
    pub trace: bool,
    pub progress: Option<ProgressSink>,
    /// When a progress sink is set, deliver assistant text as it arrives.
    /// No sink (for example `--plain`) stays on one `complete` call.
    pub stream: bool,
    pub cancel: Option<CancelToken>,
    /// Earlier turns of the conversation; the new user message is appended to them.
    pub history: Vec<Message>,
    pub steer: Option<SteerInbox>,
}

impl LoopLimits {
    fn emit(&self, event: ProgressEvent) {
        if let Some(sink) = &self.progress {
            (sink.0)(event);
        }
    }

    /// Moves messages the user sent during the run into the conversation. True if any came.
    fn take_steering(&self, messages: &mut Vec<Message>) -> bool {
        let Some(inbox) = &self.steer else {
            return false;
        };
        let texts: Vec<String> = match inbox.lock() {
            Ok(mut queue) => queue.drain(..).collect(),
            Err(_) => return false,
        };
        for text in &texts {
            messages.push(Message::user(text.clone()));
            self.emit(ProgressEvent::Steered { text: text.clone() });
        }
        !texts.is_empty()
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
    }
}

impl Default for LoopLimits {
    fn default() -> Self {
        Self {
            max_steps: 50,
            max_wall_time: Duration::from_secs(30 * 60), // 30 minutes
            max_consecutive_format_errors: 3,
            context_window: None,
            trace: false,
            progress: None,
            stream: true,
            cancel: None,
            history: Vec::new(),
            steer: None,
        }
    }
}

/// Persisted outcome metadata for one tool call, so resumed transcripts match live runs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolOutcome {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopResult {
    pub exit_reason: ExitReason,
    pub steps: usize,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub last_input_tokens: u64,
    pub messages: Vec<Message>,
    pub tool_outcomes: std::collections::BTreeMap<String, ToolOutcome>,
    pub summary: Option<TaskSummary>,
    pub verification: Option<String>,
    pub no_progress: Option<(String, usize)>,
}

/// Executes the core agent loop with profile-based permissions and system prompts.
/// Tools outside the active profile allowlist are excluded from the request schema
/// and rejected at execution time if called anyway (defense in depth).
pub fn run_agent_loop(
    profile: Profile,
    model: &dyn Model,
    tools: &ToolRegistry,
    initial_user_message: &str,
    limits: &LoopLimits,
) -> Result<LoopResult, ModelError> {
    run_loop(profile, model, tools, initial_user_message, limits, None)
}

pub fn run_agent_with_config(
    profile: Profile,
    model: &dyn Model,
    tools: &ToolRegistry,
    initial_user_message: &str,
    limits: &LoopLimits,
    root: &std::path::Path,
    config: &crate::config::Config,
) -> Result<LoopResult, ModelError> {
    run_loop(
        profile,
        model,
        tools,
        initial_user_message,
        limits,
        Some((root, config)),
    )
}

fn run_loop(
    profile: Profile,
    model: &dyn Model,
    tools: &ToolRegistry,
    initial_user_message: &str,
    limits: &LoopLimits,
    context: Option<(&std::path::Path, &crate::config::Config)>,
) -> Result<LoopResult, ModelError> {
    let mut verification = None;
    let mut no_progress = None;
    let mut seen = std::collections::HashMap::<(String, String), usize>::new();
    let mut last_tool_fingerprint: Option<(String, String, String)> = None;
    let mut identical_tool_streak = 0usize;

    let mut messages = limits.history.clone();
    messages.push(Message::user(initial_user_message));
    // The tool log spans the whole session; this run's summary counts only its own entries.
    let log_start = crate::agent::session::session_state(tools)
        .map(|s| s.log.entries.len())
        .unwrap_or(0);
    let mut steps = 0;
    let mut total_input_tokens = 0;
    let mut total_output_tokens = 0;
    let mut last_input_tokens = 0;
    let mut tool_outcomes = std::collections::BTreeMap::new();
    let mut consecutive_format_errors = 0;
    let start_time = Instant::now();

    // Filter tool schemas based on the active profile's allowlist
    let tool_schemas: Vec<_> = tools
        .schemas()
        .into_iter()
        .filter(|t| profile.is_tool_allowed(&t.name))
        .collect();

    let exit_reason = 'agent: loop {
        if limits.is_cancelled() {
            break ExitReason::Cancelled;
        }
        if steps >= limits.max_steps {
            break ExitReason::StepLimitExceeded;
        }

        if start_time.elapsed() >= limits.max_wall_time {
            break ExitReason::TimeLimitExceeded;
        }
        if steps > 0 {
            limits.take_steering(&mut messages);
        }

        let req = Request {
            system_prompt: match context.and_then(|(_, c)| c.verification.test_command.as_deref()) {
                Some(cmd) if profile == Profile::Build => format!(
                    "{}\nConfigured test command: {}",
                    profile.system_prompt(),
                    cmd
                ),
                _ => profile.system_prompt().to_string(),
            },
            messages: messages.clone(),
            tools: tool_schemas.clone(),
        };

        let step_start = Instant::now();
        limits.emit(ProgressEvent::Thinking { step: steps + 1 });
        let mut network_retries = 0;
        let completion = loop {
            let attempt = if limits.stream && limits.progress.is_some() {
                let mut buffer = String::new();
                let mut reasoning_buffer = String::new();
                model.complete_stream_cancellable(&req, limits.cancel.as_ref(), &mut |chunk| {
                    match chunk {
                        StreamChunk::Text(delta) => {
                            buffer.push_str(delta);
                            if !buffer.trim().is_empty() {
                                limits.emit(ProgressEvent::Assistant {
                                    text: buffer.clone(),
                                });
                            }
                        }
                        StreamChunk::Reasoning(delta) => {
                            reasoning_buffer.push_str(delta);
                            if !reasoning_buffer.trim().is_empty() {
                                limits.emit(ProgressEvent::Reasoning {
                                    text: reasoning_buffer.clone(),
                                });
                            }
                        }
                    }
                })
            } else {
                model.complete_cancellable(&req, limits.cancel.as_ref())
            };
            match attempt {
                Err(ModelError::Network(reason))
                    if network_retries < NETWORK_RETRIES && retryable_network_error(&reason) =>
                {
                    network_retries += 1;
                    limits.emit(ProgressEvent::Retry {
                        attempt: network_retries,
                        max: NETWORK_RETRIES,
                        reason: reason.clone(),
                    });
                    let backoff = Duration::from_secs(1 << (network_retries - 1));
                    if let Err(e) =
                        crate::agent::sse::sleep_cancellable(limits.cancel.as_ref(), backoff)
                    {
                        break Err(e);
                    }
                }
                other => break other,
            }
        };
        let mut response = match completion {
            Ok(resp) => {
                consecutive_format_errors = 0;
                resp
            }
            Err(ModelError::Cancelled) => break ExitReason::Cancelled,
            Err(ModelError::FormatError(err_msg)) => {
                consecutive_format_errors += 1;
                if consecutive_format_errors >= limits.max_consecutive_format_errors {
                    break ExitReason::FormatErrorLimitExceeded;
                }
                messages.push(Message {
                    role: Role::User,
                    content: format!("Format error: {}. Please output valid format.", err_msg),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                });
                continue;
            }
            Err(ModelError::RateLimited) => break ExitReason::RateLimited,
            Err(e) => return Err(e),
        };

        if let Some(text) = &mut response.text {
            *text = model.redact_sensitive(text);
        }
        for call in &mut response.tool_calls {
            call.arguments = model.redact_sensitive(&call.arguments);
        }
        let model_time = step_start.elapsed();
        total_input_tokens += response.usage.input_tokens;
        total_output_tokens += response.usage.output_tokens;
        last_input_tokens = response.usage.input_tokens;
        steps += 1;
        limits.emit(ProgressEvent::Usage {
            input: total_input_tokens,
            output: total_output_tokens,
        });
        if let Some(text) = response.text.as_deref().filter(|t| !t.trim().is_empty()) {
            limits.emit(ProgressEvent::Assistant {
                text: text.to_string(),
            });
        }

        if response.tool_calls.is_empty() {
            if limits.trace {
                if let Some((root, _)) = context {
                    trace_step(
                        root,
                        steps,
                        "(final)",
                        0,
                        response.text.as_ref().map_or(0, String::len),
                        &response.usage,
                        (model_time, Duration::ZERO, Duration::ZERO),
                    )?;
                }
            }
            // Model produced a final response without any tool calls
            if let Some(text) = response.text {
                messages.push(Message {
                    role: Role::Assistant,
                    content: text,
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                });
            }
            // A message sent while the model wrote its answer continues the same run.
            if limits.take_steering(&mut messages) {
                continue 'agent;
            }
            if profile == Profile::Build {
                if let Some((root, config)) = context {
                    let state = crate::agent::session::session_state(tools)?;
                    if let Some(hash) = &state.log.test_workspace_hash {
                        if crate::gate::classify_diff(&state.changes)
                            == crate::gate::ChangeClass::ClassA
                            && config
                                .verification
                                .test_command
                                .as_deref()
                                .is_some_and(|cmd| {
                                    state.log.check_status_for_command(cmd)
                                        == crate::gate::CheckStatus::Fresh
                                })
                            && *hash != crate::agent::session::workspace_hash(root)?
                        {
                            let reason = "Workspace changed since the test command ran. Run the test command again.";
                            verification = Some(format!("Verification Summary: {}", reason));
                            messages.push(Message::user(reason));
                            continue;
                        }
                    }
                    match crate::gate::evaluate_gate(
                        root,
                        config,
                        &state.log,
                        &state.changes,
                        false,
                        None,
                        "",
                    ) {
                        Ok(summary) => verification = Some(summary.to_string()),
                        // No test command configured: the model cannot satisfy the gate, so
                        // finish honestly as unverified instead of looping until the step limit.
                        Err(crate::gate::GateError::TestCommandNotRun)
                            if config.verification.test_command.is_none() =>
                        {
                            verification = Some(
                                "Verification Summary: UNVERIFIED. No test command is configured, so these changes were not tested. Set verification.test_command in .devcode.json."
                                    .to_string(),
                            );
                        }
                        Err(e) => {
                            verification = Some(format!("Verification Summary: {}", e));
                            messages.push(Message::user(format!("Verification blocked: {}", e)));
                            continue;
                        }
                    }
                }
            }
            break ExitReason::Completed;
        }

        // Assistant invoked one or more tools
        messages.push(Message {
            role: Role::Assistant,
            content: response.text.unwrap_or_default(),
            tool_calls: response.tool_calls.clone(),
            tool_call_id: None,
        });

        let tool_start = Instant::now();
        let wait_before = tools.approval_wait();
        let mut output_bytes = 0;
        for call in &response.tool_calls {
            if limits.is_cancelled() {
                break 'agent ExitReason::Cancelled;
            }
            let edit_path = if matches!(call.name.as_str(), "write_file" | "edit_file") {
                serde_json::from_str::<serde_json::Value>(&call.arguments)
                    .ok()
                    .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(str::to_owned))
            } else {
                None
            };
            let log = crate::agent::session::session_state(tools)?.log;
            let baseline_required = if profile == Profile::Build
                && matches!(call.name.as_str(), "write_file" | "edit_file")
            {
                context
                    .and_then(|(_, config)| config.verification.test_command.as_deref())
                    .filter(|cmd| {
                        !log.entries.iter().any(|e| {
                            e.kind == crate::gate::ToolKind::RunCommand && e.detail == *cmd
                        })
                    })
            } else {
                None
            };
            let before = if baseline_required.is_none() {
                if let (Some((root, _)), Some(path)) = (context, edit_path.as_deref()) {
                    state_hash(root, path)?
                } else {
                    None
                }
            } else {
                None
            };
            let edits_before = log.entries.len();
            // Defense in depth: reject any tool outside profile allowlist
            limits.emit(ProgressEvent::ToolCall {
                name: call.name.clone(),
                summary: call_summary(&call.arguments),
            });
            let mut tool_ok = true;
            let observation = if !profile.is_tool_allowed(&call.name) {
                tool_ok = false;
                format!(
                    "tool '{}' is disabled in '{}' profile",
                    call.name,
                    profile.name()
                )
            } else if let Some(cmd) = baseline_required {
                tool_ok = false;
                format!("Run the configured test command first: {}. This records whether the problem reproduces.", cmd)
            } else {
                tools.take_diff();
                match tools.execute(call) {
                    Ok(output) => output,
                    Err(err) => {
                        tool_ok = false;
                        format!("Error executing tool '{}': {}", call.name, err)
                    }
                }
            };

            let observation = model.redact_sensitive(&observation);
            let diff = tools.take_diff().map(|d| model.redact_sensitive(&d));
            tool_outcomes.insert(
                call.id.clone(),
                ToolOutcome {
                    ok: tool_ok,
                    diff: diff.clone(),
                },
            );
            limits.emit(ProgressEvent::ToolResult {
                name: call.name.clone(),
                ok: tool_ok,
                preview: preview_lines(&observation),
                diff,
            });
            output_bytes += observation.len();
            messages.push(Message::tool(observation.clone(), &call.id));
            if let Some((root, config)) = context {
                let state = crate::agent::session::session_state(tools)?;
                if call.name == "run_command" {
                    if let Some(cmd) = config.verification.test_command.as_deref() {
                        if state
                            .log
                            .entries
                            .iter()
                            .skip(edits_before)
                            .any(|e| e.kind == crate::gate::ToolKind::RunCommand && e.detail == cmd)
                        {
                            tools
                                .set_test_workspace_hash(crate::agent::session::workspace_hash(
                                    root,
                                )?)
                                .map_err(ModelError::Other)?;
                        }
                    }
                }
                if let Some(path) = edit_path {
                    if state
                        .log
                        .entries
                        .iter()
                        .skip(edits_before)
                        .any(|e| e.kind == crate::gate::ToolKind::FileEdit)
                    {
                        if let Some(hash) = before {
                            seen.entry((path.clone(), hash)).or_insert(1);
                        }
                        if let Some(hash) = state_hash(root, &path)? {
                            let count = seen.entry((path.clone(), hash)).or_default();
                            if *count >= 2 {
                                no_progress = Some((path, *count + 1));
                                break 'agent ExitReason::NoProgress;
                            }
                            *count += 1;
                        }
                    }
                }
            }
            if tool_ok && matches!(call.name.as_str(), "read_file" | "glob" | "grep") {
                let fingerprint = (call.name.clone(), call.arguments.clone(), observation);
                if last_tool_fingerprint.as_ref() == Some(&fingerprint) {
                    identical_tool_streak += 1;
                } else {
                    identical_tool_streak = 1;
                    last_tool_fingerprint = Some(fingerprint);
                }
                if identical_tool_streak >= 3 {
                    let label = format!("{} {}", call.name, call_summary(&call.arguments));
                    no_progress = Some((label.trim().to_string(), identical_tool_streak));
                    break 'agent ExitReason::NoProgress;
                }
            } else {
                last_tool_fingerprint = None;
                identical_tool_streak = 0;
            }
        }

        if limits.trace {
            if let Some((root, _)) = context {
                let names = response
                    .tool_calls
                    .iter()
                    .map(|c| {
                        if tool_schemas.iter().any(|s| s.name == c.name) {
                            c.name.as_str()
                        } else {
                            "unknown"
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let wait = tools.approval_wait().saturating_sub(wait_before);
                trace_step(
                    root,
                    steps,
                    &names,
                    response.tool_calls.iter().map(|c| c.arguments.len()).sum(),
                    output_bytes,
                    &response.usage,
                    (model_time, tool_start.elapsed().saturating_sub(wait), wait),
                )?;
            }
        }
        // Context compaction check if context_window limit is set
        if let Some(window) = limits.context_window {
            if crate::discipline::compaction::should_compact(
                response
                    .usage
                    .input_tokens
                    .saturating_add(response.usage.output_tokens),
                window,
            ) {
                let state = crate::agent::session::session_state(tools)?;
                let default_config = crate::config::Config::default();
                let config = context.map(|(_, config)| config).unwrap_or(&default_config);
                let diff = state
                    .changes
                    .iter()
                    .map(|(path, old, new)| {
                        crate::approval::format_diff(old, new, &path.to_string_lossy())
                    })
                    .collect::<String>();
                let ctx = crate::discipline::compaction::CompactionContext {
                    context_window: window,
                    active_manifest: state.manifest.as_ref(),
                    current_diff: &diff,
                    config,
                    tool_log: &state.log,
                };
                messages = crate::discipline::compaction::compact_messages(&messages, &ctx);
            }
        }
    };

    let state = crate::agent::session::session_state(tools)?;
    let turn_log = crate::gate::tool_log::ToolLog {
        entries: state
            .log
            .entries
            .get(log_start..)
            .unwrap_or_default()
            .to_vec(),
        test_workspace_hash: state.log.test_workspace_hash.clone(),
    };
    let (files_edited, files_changed_by_commands, files_outside_scope, dependencies_added) =
        crate::agent::session::summary_counts(&turn_log);
    let default_config = crate::config::Config::default();
    let (root, config) = context.unwrap_or((std::path::Path::new("."), &default_config));
    let tests_changed = crate::gate::agent_test_changes(root, config, &turn_log, &state.changes)
        .0
        .len();
    let summary = TaskSummary {
        steps,
        tokens_in: total_input_tokens,
        tokens_out: total_output_tokens,
        files_edited,
        tests_changed,
        files_changed_by_commands,
        files_outside_scope,
        dependencies_added,
    };

    Ok(LoopResult {
        exit_reason,
        steps,
        total_input_tokens,
        total_output_tokens,
        last_input_tokens,
        messages,
        tool_outcomes,
        summary: Some(summary),
        verification,
        no_progress,
    })
}

#[allow(clippy::too_many_arguments)]
fn trace_step(
    root: &std::path::Path,
    step: usize,
    tool_name: &str,
    argument_bytes: usize,
    output_bytes: usize,
    usage: &crate::agent::Usage,
    timings: (Duration, Duration, Duration),
) -> Result<(), ModelError> {
    use std::io::Write;
    let dir = crate::state::dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| ModelError::Other(e.to_string()))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("trace.jsonl"))
        .map_err(|e| ModelError::Other(e.to_string()))?;
    let entry = serde_json::json!({"step": step, "tool_name": tool_name, "argument_bytes": argument_bytes, "output_bytes": output_bytes, "input_tokens": usage.input_tokens, "output_tokens": usage.output_tokens, "model_ms": timings.0.as_millis(), "tool_ms": timings.1.as_millis(), "approval_wait_ms": timings.2.as_millis()});
    writeln!(file, "{}", entry).map_err(|e| ModelError::Other(e.to_string()))
}

fn state_hash(root: &std::path::Path, path: &str) -> Result<Option<String>, ModelError> {
    use sha2::{Digest, Sha256};
    let path = std::path::Path::new(path);
    if path.components().any(|c| c.as_os_str() == ".env") {
        return Ok(None);
    }
    let path = match crate::guard::check_write_path(root, path) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    match std::fs::read(path) {
        Ok(data) => Ok(Some(format!("{:x}", Sha256::digest(data)))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ModelError::Other(e.to_string())),
    }
}
