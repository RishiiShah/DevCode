use crate::guard::env::strip_env;
use crate::guard::GuardError;
use std::collections::VecDeque;
use std::fs::File;
use std::io::Read;
use std::os::unix::io::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_OUT_CAP: usize = 24_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub exit_code: Option<i32>,
    pub stdout_stderr: Vec<u8>,
    pub timed_out: bool,
}

impl RunResult {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }

    pub fn output_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout_stderr).to_string()
    }
}

pub struct BoundedCollector {
    cap: usize,
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total_bytes: usize,
}

impl BoundedCollector {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            head: Vec::new(),
            tail: VecDeque::new(),
            total_bytes: 0,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.total_bytes += chunk.len();
        let half_cap = self.cap / 2;

        for &b in chunk {
            if self.head.len() < half_cap {
                self.head.push(b);
            } else {
                if self.tail.len() >= half_cap {
                    self.tail.pop_front();
                }
                self.tail.push_back(b);
            }
        }
    }

    pub fn finish(self) -> Vec<u8> {
        if self.total_bytes <= self.cap {
            let mut out = self.head;
            out.extend(self.tail);
            return out;
        }

        let omitted = self
            .total_bytes
            .saturating_sub(self.head.len() + self.tail.len());
        let marker = format!("\n... [output truncated: {} bytes omitted] ...\n", omitted);
        let marker_bytes = marker.as_bytes();

        if marker_bytes.len() >= self.cap {
            return marker_bytes[..self.cap].to_vec();
        }

        let available = self.cap - marker_bytes.len();
        let head_len = available / 2;
        let tail_len = available - head_len;

        let mut out = Vec::with_capacity(self.cap);
        let head_slice = &self.head[..self.head.len().min(head_len)];
        out.extend_from_slice(head_slice);
        out.extend_from_slice(marker_bytes);

        let tail_slice: Vec<u8> = self.tail.into_iter().collect();
        let tail_start = tail_slice.len().saturating_sub(tail_len);
        out.extend_from_slice(&tail_slice[tail_start..]);

        out
    }
}

pub fn run_bounded<S: AsRef<str>>(
    argv: &[S],
    cwd: &Path,
    timeout: Duration,
    out_cap: usize,
) -> Result<RunResult, GuardError> {
    run_bounded_cancellable(argv, cwd, timeout, out_cap, None)
}

pub fn run_bounded_cancellable<S: AsRef<str>>(
    argv: &[S],
    cwd: &Path,
    timeout: Duration,
    out_cap: usize,
    cancel: Option<&crate::agent::CancelToken>,
) -> Result<RunResult, GuardError> {
    if argv.is_empty() {
        return Err(GuardError::EmptyCommand);
    }

    let prog = argv[0].as_ref();
    let args: Vec<&str> = argv[1..].iter().map(|s| s.as_ref()).collect();

    // Create a pipe for merged stdout and stderr
    let (read_fd, write_fd, write_fd_err) = {
        let mut fds = [0i32; 2];
        // SAFETY: `fds` is a stack-allocated array of two c_int values, valid for writes by libc::pipe.
        let pipe_res = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if pipe_res != 0 {
            return Err(GuardError::Io(std::io::Error::last_os_error()));
        }
        let read_fd = fds[0];
        let write_fd = fds[1];

        // Duplicate write_fd for stderr so both write to the same kernel pipe
        // SAFETY: `write_fd` is a valid open file descriptor created by libc::pipe above.
        let write_fd_err = unsafe { libc::dup(write_fd) };
        if write_fd_err < 0 {
            // SAFETY: Both read_fd and write_fd are open file descriptors that must be closed on error.
            unsafe {
                libc::close(read_fd);
                libc::close(write_fd);
            }
            return Err(GuardError::Io(std::io::Error::last_os_error()));
        }

        (read_fd, write_fd, write_fd_err)
    };

    // SAFETY: `read_fd` is a valid open file descriptor transferred to File.
    let mut reader_file = unsafe { File::from_raw_fd(read_fd) };
    // SAFETY: read_fd is open and owned by reader_file.
    if unsafe { libc::fcntl(read_fd, libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
        // SAFETY: these descriptors have not yet been transferred to Stdio.
        unsafe {
            libc::close(write_fd);
            libc::close(write_fd_err);
        }
        return Err(GuardError::Io(std::io::Error::last_os_error()));
    }

    // Spawn child with write_fd and write_fd_err attached to stdout/stderr.
    // The Command object is scoped and dropped immediately after spawn,
    // which closes the parent's write ends so reader thread receives EOF when child dies.
    let mut child = {
        // SAFETY: `write_fd` and `write_fd_err` are valid open file descriptors transferred to Stdio.
        let stdout_stdio = unsafe { Stdio::from_raw_fd(write_fd) };
        let stderr_stdio = unsafe { Stdio::from_raw_fd(write_fd_err) };

        let mut cmd = Command::new(prog);
        cmd.args(args);
        cmd.current_dir(cwd);
        cmd.stdout(stdout_stdio);
        cmd.stderr(stderr_stdio);
        cmd.process_group(0);

        // Apply stripped environment
        cmd.env_clear();
        for (k, v) in strip_env(std::env::vars()) {
            cmd.env(k, v);
        }

        cmd.spawn().map_err(GuardError::Spawn)?
    };

    let collector = Arc::new(Mutex::new(BoundedCollector::new(out_cap)));
    let collector_clone = Arc::clone(&collector);
    let stop_reader = Arc::new(AtomicBool::new(false));
    let reader_stop = Arc::clone(&stop_reader);

    // Reader thread reading merged stdout/stderr until EOF
    let reader_handle = thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            if reader_stop.load(Ordering::Relaxed) {
                break;
            }
            match reader_file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut col) = collector_clone.lock() {
                        col.push(&buf[..n]);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if reader_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });

    let start = Instant::now();
    let mut timed_out = false;

    let mut child_status = None;
    let status = loop {
        if child_status.is_none() {
            child_status = child.try_wait().map_err(GuardError::Io)?;
        }
        if let Some(status) = child_status {
            if reader_handle.is_finished() {
                break status;
            }
        }

        if start.elapsed() >= timeout || cancel.is_some_and(|c| c.is_cancelled()) {
            timed_out = true;
            let pid = child.id() as libc::pid_t;

            // SAFETY: `pid` is the positive PID of the child process spawned with
            // CommandExt::process_group(0), making `pid` the process group ID (PGID).
            unsafe {
                libc::killpg(pid, libc::SIGTERM);
            }

            // Grace period: wait up to 50ms for process group to exit cleanly
            let grace_deadline = Instant::now() + Duration::from_millis(50);
            let mut exited_status = child_status;
            while Instant::now() < grace_deadline {
                if exited_status.is_some() {
                    break;
                }
                if let Some(s) = child.try_wait().map_err(GuardError::Io)? {
                    exited_status = Some(s);
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }

            // SAFETY: kill remaining members even if the shell already exited.
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
            }
            let final_status = if let Some(s) = exited_status {
                s
            } else {
                child.wait().map_err(GuardError::Io)?
            };

            // Ensure all processes in the group are dead
            let group_deadline = Instant::now() + Duration::from_millis(150);
            while Instant::now() < group_deadline {
                // SAFETY: `pid` is the positive PGID being queried for existence.
                let ret = unsafe { libc::killpg(pid, 0) };
                if ret != 0 {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }

            stop_reader.store(true, Ordering::Relaxed);
            break final_status;
        }

        thread::sleep(Duration::from_millis(10));
    };

    let _ = reader_handle.join();

    let collected_output = {
        let mut empty = BoundedCollector::new(out_cap);
        match collector.lock() {
            Ok(mut guard) => {
                std::mem::swap(&mut *guard, &mut empty);
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                std::mem::swap(&mut *guard, &mut empty);
            }
        }
        empty.finish()
    };

    Ok(RunResult {
        exit_code: status.code(),
        stdout_stderr: collected_output,
        timed_out,
    })
}
