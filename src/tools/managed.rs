//! Managed long-running processes (development servers).
//! start: approval, pre-start snapshot, own process group, readiness probe. Returns once the
//! server answers or the wait ends; a slow server keeps running in the background.
//! status: live state, exit status, recent log output.
//! stop: kill the process group, confirm it is gone, reconcile file changes.
//! Drop (DevCode exit): stops and reconciles every process still running.
//! Reconciliation records changes only; it never rolls files back.

use crate::agent::{Tool, ToolRegistry};
use crate::approval::{check_shell_command, save_reconciled_changes, CommandCheck};
use crate::gate::tool_log::ToolLog;
use crate::guard::strip_env;
use crate::manifest::{reconcile, snapshot_guard_set, GuardSetSnapshot};
use crate::tools::mutate::MutationContext;
use serde::Deserialize;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_READY_SECS: u64 = 20;
const MAX_READY_SECS: u64 = 120;
const GROUP_EXIT_WAIT: Duration = Duration::from_secs(2);
const LOG_TAIL_BYTES: u64 = 4096;

struct Managed {
    id: String,
    command: String,
    port: u16,
    root: PathBuf,
    child: Child,
    pgid: i32,
    log_path: PathBuf,
    before_path: PathBuf,
    before: GuardSetSnapshot,
    session: String,
    step: usize,
    /// Set once the process group is confirmed gone: the leader's exit code.
    closed: Option<Option<i32>>,
    /// Set once changes are reconciled and the recovery file is removed.
    changed: Option<Vec<String>>,
}

#[derive(Default)]
pub struct ManagedProcesses {
    items: Vec<Managed>,
    next_id: usize,
}

impl Drop for ManagedProcesses {
    fn drop(&mut self) {
        let mut log = ToolLog::new();
        for m in &mut self.items {
            let _ = m.finalize(&mut log);
        }
    }
}

impl ManagedProcesses {
    /// One line per managed process: id, state, address, command.
    pub(crate) fn list(&mut self) -> Vec<String> {
        self.items
            .iter_mut()
            .map(|m| {
                let state = if m.running() {
                    format!("running, http://localhost:{}", m.port)
                } else {
                    "stopped".to_string()
                };
                format!("{}  {state}  {}", m.id, m.command)
            })
            .collect()
    }

    /// Processes still running (not stopped and not exited on their own).
    pub(crate) fn running_count(&mut self) -> usize {
        self.items
            .iter_mut()
            .filter_map(|m| m.running().then_some(()))
            .count()
    }
}

impl Managed {
    fn running(&mut self) -> bool {
        self.closed.is_none() && matches!(self.child.try_wait(), Ok(None))
    }

    fn group_alive(&self) -> bool {
        // SAFETY: pgid is the positive pid of a child spawned with process_group(0).
        unsafe { libc::killpg(self.pgid, 0) == 0 }
    }

    /// Signals the group and waits until the leader is reaped and the group is empty.
    fn kill_group(&mut self) -> Result<Option<i32>, String> {
        let mut code = self.closed.flatten();
        for signal in [libc::SIGTERM, libc::SIGKILL] {
            // SAFETY: see group_alive.
            unsafe { libc::killpg(self.pgid, signal) };
            let deadline = Instant::now() + GROUP_EXIT_WAIT;
            loop {
                if let Ok(Some(status)) = self.child.try_wait() {
                    code = status.code();
                }
                if !self.group_alive() {
                    return Ok(code);
                }
                if Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        Err(format!(
            "process group {} still alive after SIGKILL",
            self.pgid
        ))
    }

    /// Stops the group and reconciles changes against the pre-start snapshot.
    /// Idempotent: returns early once reconciled. On failure the recovery file is kept.
    fn finalize(&mut self, log: &mut ToolLog) -> Result<(), String> {
        if self.changed.is_some() {
            return Ok(());
        }
        let code = self.kill_group()?;
        self.closed = Some(code);
        let keep = format!("recovery data kept at {}", self.before_path.display());
        let after = snapshot_guard_set(&self.root, Some(&self.before))
            .map_err(|e| format!("reconcile failed: {e}; {keep}"))?;
        let rec = reconcile(&self.before, &after);
        let ts = now_ms();
        save_reconciled_changes(
            &self.root,
            &self.session,
            self.step,
            &self.before,
            &rec,
            ts,
            log,
            &self.before_path,
        )
        .map_err(|e| format!("{e}; {keep}"))?;
        let mut changed: Vec<String> = Vec::new();
        for p in rec.created.iter().chain(&rec.modified).chain(&rec.deleted) {
            changed.push(p.to_string_lossy().into_owned());
            log.record_command_file_change(&p.to_string_lossy(), ts);
        }
        fs::remove_file(&self.before_path).map_err(|e| format!("{e}; {keep}"))?;
        self.changed = Some(changed);
        Ok(())
    }

    fn exit_label(&self) -> String {
        match self.closed {
            Some(Some(c)) => format!("exit code {c}"),
            Some(None) => "killed by signal".to_string(),
            None => "running".to_string(),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// True when a local listener accepts connections on IPv4 or IPv6 loopback. `localhost` often
/// resolves to `::1` only (Vite on macOS), so probing 127.0.0.1 alone misses ready servers.
fn port_open(port: u16) -> bool {
    [
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ]
    .iter()
    .any(|addr| TcpStream::connect_timeout(addr, Duration::from_millis(200)).is_ok())
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI sequence: ESC [ parameters final-letter.
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Port of the last local URL the server printed. Dev servers move to the next free port when
/// the requested one is busy (Vite: 5173 -> 5174), so the log is the source of truth.
fn logged_port(log: &str) -> Option<u16> {
    let text = strip_ansi(log);
    let mut found = None;
    let mut best = 0;
    for host in ["localhost:", "127.0.0.1:", "[::1]:", "0.0.0.0:"] {
        for (at, _) in text.match_indices(host) {
            let digits: String = text[at + host.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(port) = digits.parse::<u16>() {
                if port > 0 && at >= best {
                    best = at;
                    found = Some(port);
                }
            }
        }
    }
    found
}

fn log_tail(path: &Path) -> String {
    let Ok(mut f) = File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL_BYTES)));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).trim_end().to_string()
}

fn changed_label(changed: &[String]) -> String {
    if changed.is_empty() {
        "no workspace changes".to_string()
    } else {
        format!("changed files: {}", changed.join(", "))
    }
}

#[derive(Deserialize)]
struct StartArgs {
    command: String,
    port: u16,
    ready_timeout_secs: Option<u64>,
}
#[derive(Deserialize)]
struct IdArgs {
    id: String,
}

#[derive(Clone, Copy)]
enum Op {
    Start,
    Status,
    Stop,
    List,
}

struct ManagedTool {
    op: Op,
    context: Arc<Mutex<MutationContext>>,
}

pub fn register(tools: &mut ToolRegistry, context: Arc<Mutex<MutationContext>>) {
    for op in [Op::Start, Op::Status, Op::Stop, Op::List] {
        tools.register(ManagedTool {
            op,
            context: Arc::clone(&context),
        });
    }
}

impl Tool for ManagedTool {
    fn name(&self) -> &str {
        match self.op {
            Op::Start => "start_process",
            Op::Status => "process_status",
            Op::Stop => "stop_process",
            Op::List => "list_processes",
        }
    }
    fn description(&self) -> &str {
        match self.op {
            Op::Start => "Start a long-running server (dev server, watcher) in the background with approval. Returns once localhost:port answers or the wait ends; a slow server keeps running. Never run servers with run_command.",
            Op::Status => "Report a managed process: running or exit status, port state, recent output",
            Op::Stop => "Stop a managed process group and report workspace changes it made",
            Op::List => "List managed processes: id, state, address, command",
        }
    }
    fn parameters_schema(&self) -> serde_json::Value {
        match self.op {
            Op::Start => serde_json::json!({"type":"object","properties":{
                "command":{"type":"string"},
                "port":{"type":"integer","minimum":1,"maximum":65535},
                "ready_timeout_secs":{"type":"integer","minimum":1,"maximum":MAX_READY_SECS}},
                "required":["command","port"]}),
            Op::Status | Op::Stop => serde_json::json!({"type":"object","properties":{
                "id":{"type":"string"}},"required":["id"]}),
            Op::List => serde_json::json!({"type":"object","properties":{}}),
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
            processes,
            cancel,
            ..
        } = &mut *ctx;
        match self.op {
            Op::Start => {
                *step += 1;
                let args: StartArgs = serde_json::from_str(arguments)
                    .map_err(|e| format!("invalid arguments: {e}"))?;
                let started = Started {
                    session: session.as_str(),
                    step: *step,
                    cancel,
                };
                start(processes, root, started, args, prompt, log)
            }
            Op::Status => {
                let args: IdArgs = serde_json::from_str(arguments)
                    .map_err(|e| format!("invalid arguments: {e}"))?;
                status(processes, &args.id, log)
            }
            Op::Stop => {
                let args: IdArgs = serde_json::from_str(arguments)
                    .map_err(|e| format!("invalid arguments: {e}"))?;
                stop(processes, &args.id, log)
            }
            Op::List => {
                let lines = processes.list();
                Ok(if lines.is_empty() {
                    "no managed processes".into()
                } else {
                    lines.join("\n")
                })
            }
        }
    }
}

struct Started<'a> {
    session: &'a str,
    step: usize,
    cancel: &'a crate::agent::CancelToken,
}

fn start(
    processes: &mut ManagedProcesses,
    root: &Path,
    started: Started<'_>,
    args: StartArgs,
    prompt: &mut crate::approval::ApprovalPrompt<
        Box<dyn std::io::BufRead + Send>,
        Box<dyn std::io::Write + Send>,
    >,
    log: &mut ToolLog,
) -> Result<String, String> {
    if args.port == 0 {
        return Err("port must be 1-65535".into());
    }
    let ready_secs = args.ready_timeout_secs.unwrap_or(DEFAULT_READY_SECS);
    if !(1..=MAX_READY_SECS).contains(&ready_secs) {
        return Err(format!("ready_timeout_secs must be 1-{MAX_READY_SECS}"));
    }

    let guard_set: Vec<PathBuf> = ignore::WalkBuilder::new(root)
        .hidden(false)
        .build()
        .filter_map(Result::ok)
        .filter_map(|e| e.path().strip_prefix(root).ok().map(Path::to_path_buf))
        .collect();
    if let CommandCheck::Denied(reason) = check_shell_command(&args.command, root, &guard_set) {
        return Err(format!("command denied: {reason}"));
    }
    let approved = prompt
        .request_command(&args.command, "starts a managed long-running process")
        .map_err(|e| e.to_string())?;
    if !approved {
        return Err(format!(
            "managed process start rejected: '{}'",
            args.command
        ));
    }

    processes.next_id += 1;
    let id = format!("proc-{}", processes.next_id);
    let dir = root.join(".devcode/managed");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let log_path = dir.join(format!("{id}.log"));
    let before_path = dir.join(format!("{id}.before.json"));

    // Pre-start state: kept on disk until the process is stopped and reconciled.
    let before = snapshot_guard_set(root, None).map_err(|e| e.to_string())?;
    let mut f = File::create(&before_path).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut f, &before).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;

    let out = File::create(&log_path).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let child = Command::new("sh")
        .arg("-c")
        .arg(&args.command)
        .current_dir(root)
        .env_clear()
        .envs(strip_env(std::env::vars()))
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .process_group(0)
        .spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = fs::remove_file(&before_path);
            return Err(format!("failed to spawn: {e}"));
        }
    };
    let pgid = child.id() as i32;
    let mut m = Managed {
        id: id.clone(),
        command: args.command.clone(),
        port: args.port,
        root: root.to_path_buf(),
        child,
        pgid,
        log_path: log_path.clone(),
        before_path,
        before,
        session: started.session.to_string(),
        step: started.step,
        closed: None,
        changed: None,
    };

    let deadline = Instant::now() + Duration::from_secs(ready_secs);
    let outcome = loop {
        if matches!(m.child.try_wait(), Ok(Some(_))) {
            break Err(format!("{id} exited before it was ready"));
        }
        let logged = logged_port(&log_tail(&log_path)).filter(|&p| p != m.port);
        if let Some(port) = logged.filter(|&p| port_open(p)) {
            m.port = port;
            break Ok(true);
        }
        if port_open(m.port) {
            break Ok(true);
        }
        if Instant::now() >= deadline || started.cancel.is_cancelled() {
            break Ok(false);
        }
        thread::sleep(Duration::from_millis(100));
    };
    if let Ok(ready) = outcome {
        let moved = if m.port == args.port {
            String::new()
        } else {
            format!(" (asked for {}; the server chose {})", args.port, m.port)
        };
        let msg = if ready {
            format!(
                "started {id}: http://localhost:{}{moved} is ready; pid {pgid}; log {}",
                m.port,
                log_path.display()
            )
        } else {
            format!(
                "started {id}: still starting, localhost:{} not answering yet; it keeps running in the background. Check with process_status, stop with stop_process.\nlog:\n{}",
                m.port,
                log_tail(&log_path)
            )
        };
        processes.items.push(m);
        return Ok(msg);
    }
    // Failed start: stop, reconcile, and keep the entry so status can report it.
    let note = m.finalize(log).err().unwrap_or_default();
    let failure = format!(
        "{}; {id} stopped ({}). {note}\nlog:\n{}",
        outcome.unwrap_err(),
        m.exit_label(),
        log_tail(&log_path)
    );
    processes.items.push(m);
    Err(failure)
}

fn find<'a>(processes: &'a mut ManagedProcesses, id: &str) -> Result<&'a mut Managed, String> {
    processes
        .items
        .iter_mut()
        .find(|m| m.id == id)
        .ok_or_else(|| format!("no managed process '{id}'"))
}

fn status(processes: &mut ManagedProcesses, id: &str, log: &mut ToolLog) -> Result<String, String> {
    let m = find(processes, id)?;
    if m.closed.is_none() && matches!(m.child.try_wait(), Ok(Some(_))) {
        m.finalize(log)?;
    }
    let state = if m.closed.is_some() {
        format!("stopped ({})", m.exit_label())
    } else {
        format!("running (pid {})", m.pgid)
    };
    let ready = if m.closed.is_none() && port_open(m.port) {
        "responding"
    } else {
        "not responding"
    };
    let changes = match &m.changed {
        Some(c) => changed_label(c),
        None => "changes pending reconcile".to_string(),
    };
    Ok(format!(
        "{}: {state}; localhost:{} {ready}; {changes}; command: {}\nlog:\n{}",
        m.id,
        m.port,
        m.command,
        log_tail(&m.log_path)
    ))
}

/// Lists and stops managed processes from the UI thread while a run works. Each call waits for
/// the tool context, so it lands between tool calls; run it off the UI thread.
#[derive(Clone)]
pub struct ProcessControl(pub(crate) Arc<Mutex<MutationContext>>);

impl ProcessControl {
    pub fn list(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|mut ctx| ctx.processes.list())
            .unwrap_or_default()
    }

    pub fn running(&self) -> usize {
        self.0
            .lock()
            .map(|mut ctx| ctx.processes.running_count())
            .unwrap_or_default()
    }

    pub fn stop(&self, id: &str) -> Result<String, String> {
        let mut guard = self.0.lock().map_err(|e| e.to_string())?;
        let ctx = &mut *guard;
        stop(&mut ctx.processes, id, &mut ctx.log)
    }
}

pub(crate) fn stop(
    processes: &mut ManagedProcesses,
    id: &str,
    log: &mut ToolLog,
) -> Result<String, String> {
    let m = find(processes, id)?;
    m.finalize(log)?;
    Ok(format!(
        "stopped {}: {}; {}",
        m.id,
        m.exit_label(),
        changed_label(m.changed.as_deref().unwrap_or(&[]))
    ))
}

#[cfg(test)]
mod port_tests {
    use super::logged_port;

    #[test]
    fn logged_port_reads_last_local_url_through_ansi_codes() {
        let vite = "  VITE v8 ready\n  \x1b[32m➜\x1b[39m  Local:   \x1b[36mhttp://localhost:\x1b[1m5174\x1b[22m/\x1b[39m\n";
        assert_eq!(logged_port(vite), Some(5174));
        assert_eq!(
            logged_port("Port 3000 in use\nListening on http://127.0.0.1:3001"),
            Some(3001)
        );
        assert_eq!(logged_port("compiling..."), None);
    }
}
