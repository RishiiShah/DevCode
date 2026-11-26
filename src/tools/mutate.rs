use crate::agent::{CancelToken, Tool, ToolRegistry};
use crate::approval::runner::execute_guarded_command_cancellable;
use crate::approval::ApprovalPrompt;
use crate::discipline::{
    apply_disciplined_file_edit, DeclareScopeTool, ScopeContract, TaskSummary,
};
use crate::gate::tool_log::ToolLog;
use crate::guard::check_write_path;
use serde::Deserialize;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct MutationContext {
    pub(crate) root: PathBuf,
    pub(crate) session: String,
    pub(crate) step: usize,
    pub(crate) prompt: ApprovalPrompt<Box<dyn BufRead + Send>, Box<dyn Write + Send>>,
    pub(crate) log: ToolLog,
    pub(crate) processes: super::managed::ManagedProcesses,
    pub(crate) scope: ScopeContract,
    pub(crate) approval_wait: Arc<Mutex<Duration>>,
    pub(crate) cancel: CancelToken,
    /// Unified diff of the last applied edit, for display only; the model sees "Updated …".
    pub(crate) last_diff: Option<String>,
}

impl MutationContext {
    pub fn new<R: BufRead + Send + 'static, W: Write + Send + 'static>(
        root: PathBuf,
        session: String,
        prompt: ApprovalPrompt<R, W>,
    ) -> Self {
        let approval_wait = Arc::new(Mutex::new(Duration::ZERO));
        Self {
            root,
            session,
            step: 0,
            prompt: ApprovalPrompt {
                reader: Box::new(TimedReader {
                    inner: prompt.reader,
                    elapsed: Arc::clone(&approval_wait),
                }),
                writer: Box::new(prompt.writer),
                is_tty: prompt.is_tty,
                auto_approve_edits: prompt.auto_approve_edits,
                always_approved: prompt.always_approved,
            },
            log: ToolLog::new(),
            processes: Default::default(),
            last_diff: None,
            scope: ScopeContract::new(),
            approval_wait,
            cancel: CancelToken::new(),
        }
    }
}

/// Unique id for one run's manifests and blobs.
pub fn new_session_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    )
}

/// Builds the concrete tool registry for one session.
pub fn registry(root: PathBuf, auto_approve_edits: bool) -> ToolRegistry {
    registry_with_context(MutationContext::new(
        root,
        new_session_id(),
        crate::approval::terminal_prompt(auto_approve_edits),
    ))
}

pub fn registry_with_context(context: MutationContext) -> ToolRegistry {
    let root = context.root.clone();
    let scope = context.scope.clone();
    let cancel = context.cancel.clone();
    let shared = Arc::new(Mutex::new(context));
    let mut tools = ToolRegistry::new();
    tools.context = Some(Arc::clone(&shared));
    tools.register(DeclareScopeTool::new(scope));
    tools.register(super::ReadFileTool { root: root.clone() });
    tools.register(super::GlobTool { root: root.clone() });
    tools.register(super::GrepTool { root });
    for kind in [Kind::Write, Kind::Edit, Kind::Command] {
        tools.register(MutationTool {
            kind,
            context: Arc::clone(&shared),
            cancel: cancel.clone(),
        });
    }
    super::managed::register(&mut tools, Arc::clone(&shared));
    tools
}

#[derive(Clone, Copy)]
enum Kind {
    Write,
    Edit,
    Command,
}
struct MutationTool {
    kind: Kind,
    context: Arc<Mutex<MutationContext>>,
    cancel: CancelToken,
}
#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}
#[derive(Deserialize)]
struct EditArgs {
    path: String,
    old_string: String,
    new_string: String,
}
#[derive(Deserialize)]
struct CommandArgs {
    command: String,
}

impl Tool for MutationTool {
    fn name(&self) -> &str {
        match self.kind {
            Kind::Write => "write_file",
            Kind::Edit => "edit_file",
            Kind::Command => "run_command",
        }
    }
    fn description(&self) -> &str {
        match self.kind {
            Kind::Write => "Write a file with approval",
            Kind::Edit => "Replace one exact string with approval",
            Kind::Command => "Run a guarded shell command; stops after 120 seconds. Use start_process for development servers",
        }
    }
    fn parameters_schema(&self) -> serde_json::Value {
        match self.kind {
            Kind::Write => {
                serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]})
            }
            Kind::Edit => {
                serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"}},"required":["path","old_string","new_string"]})
            }
            Kind::Command => {
                serde_json::json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]})
            }
        }
    }
    fn execute(&self, arguments: &str) -> Result<String, String> {
        let mut ctx = self.context.lock().map_err(|e| e.to_string())?;
        let MutationContext {
            root,
            session,
            step,
            prompt,
            log,
            scope,
            last_diff,
            ..
        } = &mut *ctx;
        *step += 1;
        match self.kind {
            Kind::Command => {
                let args: CommandArgs = serde_json::from_str(arguments)
                    .map_err(|e| format!("invalid arguments: {e}"))?;
                let result = execute_guarded_command_cancellable(
                    root,
                    &args.command,
                    session,
                    *step,
                    120,
                    prompt,
                    log,
                    Some(&self.cancel),
                )
                .map_err(|e| e.to_string())?;
                let output = format!(
                    "{}\nexit code: {:?}; timed out: {}",
                    result.stdout_stderr, result.exit_code, result.timed_out
                );
                if result.timed_out {
                    Err(format!("{output}\nCommand timed out and its process group was stopped. Start long-running servers in a separate terminal."))
                } else if result.exit_code != Some(0) {
                    Err(output)
                } else {
                    Ok(output)
                }
            }
            kind => {
                let (path, old, content) = match kind {
                    Kind::Write => {
                        let args: WriteArgs = serde_json::from_str(arguments)
                            .map_err(|e| format!("invalid arguments: {e}"))?;
                        let old = check_write_path(root, Path::new(&args.path))
                            .ok()
                            .and_then(|full| std::fs::read_to_string(full).ok())
                            .unwrap_or_default();
                        (args.path, old, args.content)
                    }
                    Kind::Edit => {
                        let args: EditArgs = serde_json::from_str(arguments)
                            .map_err(|e| format!("invalid arguments: {e}"))?;
                        let full = check_write_path(root, Path::new(&args.path))
                            .map_err(|e| e.to_string())?;
                        let old = std::fs::read_to_string(full).map_err(|e| e.to_string())?;
                        if args.old_string.is_empty() || old.matches(&args.old_string).count() != 1
                        {
                            return Err("old_string must match exactly once".into());
                        }
                        let new = old.replacen(&args.old_string, &args.new_string, 1);
                        (args.path, old, new)
                    }
                    Kind::Command => return Err("invalid tool".into()),
                };
                let approved = apply_disciplined_file_edit(
                    root,
                    Path::new(&path),
                    &content,
                    session,
                    *step,
                    scope,
                    prompt,
                    log,
                    &mut TaskSummary::default(),
                )
                .map_err(|e| e.to_string())?;
                if approved {
                    *last_diff = Some(crate::approval::diff::format_diff(&old, &content, &path));
                    Ok(format!("Updated {path}"))
                } else {
                    Err("edit rejected".into())
                }
            }
        }
    }
}

struct TimedReader<R> {
    inner: R,
    elapsed: Arc<Mutex<Duration>>,
}
impl<R: BufRead> Read for TimedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}
impl<R: BufRead> BufRead for TimedReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }
    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt);
    }
    fn read_line(&mut self, buf: &mut String) -> io::Result<usize> {
        let start = Instant::now();
        let result = self.inner.read_line(buf);
        if let Ok(mut elapsed) = self.elapsed.lock() {
            *elapsed += start.elapsed();
        }
        result
    }
}
