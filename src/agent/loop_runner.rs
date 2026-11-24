use crate::agent::model::{Message, Model, ModelError, Request, Role};
use crate::agent::tool::ToolRegistry;
use crate::discipline::summary::TaskSummary;
use crate::profile::Profile;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Completed,
    StepLimitExceeded,
    TimeLimitExceeded,
    FormatErrorLimitExceeded,
    RateLimited,
    NoProgress,
}

#[derive(Debug, Clone)]
pub struct LoopLimits {
    pub max_steps: usize,
    pub max_wall_time: Duration,
    pub max_consecutive_format_errors: usize,
    pub context_window: Option<u64>,
    pub trace: bool,
}

impl Default for LoopLimits {
    fn default() -> Self {
        Self {
            max_steps: 50,
            max_wall_time: Duration::from_secs(30 * 60), // 30 minutes
            max_consecutive_format_errors: 3,
            context_window: None,
            trace: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopResult {
    pub exit_reason: ExitReason,
    pub steps: usize,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub messages: Vec<Message>,
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
    if let Some((root, _)) = context {
        let dir = root.join(".devcode");
        std::fs::create_dir_all(&dir).map_err(|e| ModelError::Other(e.to_string()))?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(".gitignore"))
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(b"*\n")
                    .map_err(|e| ModelError::Other(e.to_string()))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(ModelError::Other(e.to_string())),
        }
    }
    let mut verification = None;
    let mut no_progress = None;
    let mut seen = std::collections::HashMap::<(String, String), usize>::new();

    let mut messages = vec![Message::user(initial_user_message)];
    let mut steps = 0;
    let mut total_input_tokens = 0;
    let mut total_output_tokens = 0;
    let mut consecutive_format_errors = 0;
    let start_time = Instant::now();

    // Filter tool schemas based on the active profile's allowlist
    let tool_schemas: Vec<_> = tools
        .schemas()
        .into_iter()
        .filter(|t| profile.is_tool_allowed(&t.name))
        .collect();

    let exit_reason = 'agent: loop {
        if steps >= limits.max_steps {
            break ExitReason::StepLimitExceeded;
        }

        if start_time.elapsed() >= limits.max_wall_time {
            break ExitReason::TimeLimitExceeded;
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
        let response = match model.complete(&req) {
            Ok(resp) => {
                consecutive_format_errors = 0;
                resp
            }
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

        let model_time = step_start.elapsed();
        total_input_tokens += response.usage.input_tokens;
        total_output_tokens += response.usage.output_tokens;
        steps += 1;

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
            let observation = if !profile.is_tool_allowed(&call.name) {
                format!(
                    "tool '{}' is disabled in '{}' profile",
                    call.name,
                    profile.name()
                )
            } else if let Some(cmd) = baseline_required {
                format!("Run the configured test command first: {}. This records whether the problem reproduces.", cmd)
            } else {
                match tools.execute(call) {
                    Ok(output) => output,
                    Err(err) => format!("Error executing tool '{}': {}", call.name, err),
                }
            };

            output_bytes += observation.len();
            messages.push(Message::tool(observation, &call.id));
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
                total_input_tokens + total_output_tokens,
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
    let (files_edited, files_changed_by_commands, files_outside_scope, dependencies_added) =
        crate::agent::session::summary_counts(&state.log);
    let summary = TaskSummary {
        steps,
        tokens_in: total_input_tokens,
        tokens_out: total_output_tokens,
        files_edited,
        files_changed_by_commands,
        files_outside_scope,
        dependencies_added,
    };

    Ok(LoopResult {
        exit_reason,
        steps,
        total_input_tokens,
        total_output_tokens,
        messages,
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
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(".devcode/trace.jsonl"))
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
