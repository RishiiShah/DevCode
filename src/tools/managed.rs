//! Managed long-running processes (development servers).
//! start: approval, pre-start snapshot, own process group, readiness probe.
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
use std::net::{SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_READY_SECS: u64 = 30;
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

impl Managed {
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

fn port_open(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
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
}

struct ManagedTool {
    op: Op,
    context: Arc<Mutex<MutationContext>>,
}

pub fn register(tools: &mut ToolRegistry, context: Arc<Mutex<MutationContext>>) {
    for op in [Op::Start, Op::Status, Op::Stop] {
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
        }
    }
    fn description(&self) -> &str {
        match self.op {
            Op::Start => "Start a long-running server with approval; returns once 127.0.0.1:port accepts connections, or stops it and fails",
            Op::Status => "Report a managed process: running or exit status, port state, recent output",
            Op::Stop => "Stop a managed process group and report workspace changes it made",
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
            ..
        } = &mut *ctx;
        match self.op {
            Op::Start => {
                *step += 1;
                let args: StartArgs = serde_json::from_str(arguments)
                    .map_err(|e| format!("invalid arguments: {e}"))?;
                start(processes, root, session.as_str(), *step, args, prompt, log)
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
        }
    }
}

fn start(
    processes: &mut ManagedProcesses,
    root: &Path,
    session: &str,
    step: usize,
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
        session: session.to_string(),
        step,
        closed: None,
        changed: None,
    };

    let deadline = Instant::now() + Duration::from_secs(ready_secs);
    let outcome = loop {
        if matches!(m.child.try_wait(), Ok(Some(_))) {
            break Err(format!(
                "{id} exited before 127.0.0.1:{} was ready",
                args.port
            ));
        }
        if port_open(args.port) {
            break Ok(());
        }
        if Instant::now() >= deadline {
            break Err(format!(
                "127.0.0.1:{} not ready within {ready_secs}s",
                args.port
            ));
        }
        thread::sleep(Duration::from_millis(100));
    };
    if let Ok(()) = outcome {
        let msg = format!(
            "started {id}: pid {pgid}, 127.0.0.1:{} ready, log {}",
            args.port,
            log_path.display()
        );
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
        "{}: {state}; 127.0.0.1:{} {ready}; {changes}; command: {}\nlog:\n{}",
        m.id,
        m.port,
        m.command,
        log_tail(&m.log_path)
    ))
}

fn stop(processes: &mut ManagedProcesses, id: &str, log: &mut ToolLog) -> Result<String, String> {
    let m = find(processes, id)?;
    m.finalize(log)?;
    Ok(format!(
        "stopped {}: {}; {}",
        m.id,
        m.exit_label(),
        changed_label(m.changed.as_deref().unwrap_or(&[]))
    ))
}
