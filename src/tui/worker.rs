//! Worker thread: owns the `Session`, runs prompts, and bridges approval prompts to the UI.
//!
//! The approval code reads answers from a `BufRead` and writes questions to a `Write`. Here both
//! are channels: the writer forwards question text to the UI, and the reader blocks until the UI
//! sends an answer line. No approval logic changes; only where the text goes.
use crate::agent::{LoopResult, ProgressEvent, ProgressSink};
use crate::{Cli, Profile, Session};
use std::io::{self, BufRead, Read, Write};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// Work for the worker. No `Debug`: `Connect` carries a secret.
pub enum Cmd {
    Prompt(String),
    SetProfile(Profile),
    Connect {
        name: Option<String>,
        key: String,
    },
    Connections,
    Rollback(Option<usize>),
    /// Re-read `.devcode.json` (after `/init` wrote it).
    Reload,
    ListModels,
    ListEfforts,
    SetModel(String),
    SetProvider(String),
    /// `None` clears the effort setting.
    SetEffort(Option<String>),
}

pub enum Ev {
    Progress(ProgressEvent),
    /// Question text from the approval code; shown when `NeedInput` follows.
    PromptText(String),
    /// The worker is blocked until the UI answers.
    NeedInput,
    Done(Box<LoopResult>),
    Failed(String),
    Info(Vec<String>),
    /// Provider label after a connect, reload or switch.
    Provider(String),
    Models(Result<Vec<String>, String>),
    /// Effort values the current model accepts (empty: none).
    Efforts(Vec<String>),
}

pub struct ChannelWriter(pub Sender<Ev>);
impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = self
            .0
            .send(Ev::PromptText(String::from_utf8_lossy(buf).into_owned()));
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct ChannelReader {
    pub answers: Receiver<String>,
    pub events: Sender<Ev>,
    buf: Vec<u8>,
    pos: usize,
}
impl ChannelReader {
    pub fn new(answers: Receiver<String>, events: Sender<Ev>) -> Self {
        Self {
            answers,
            events,
            buf: Vec::new(),
            pos: 0,
        }
    }
}
impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let n = available.len().min(out.len());
        out[..n].copy_from_slice(&available[..n]);
        self.consume(n);
        Ok(n)
    }
}
impl BufRead for ChannelReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos >= self.buf.len() {
            let _ = self.events.send(Ev::NeedInput);
            match self.answers.recv() {
                Ok(mut line) => {
                    if !line.ends_with('\n') {
                        line.push('\n');
                    }
                    self.buf = line.into_bytes();
                    self.pos = 0;
                }
                // UI is gone: behave like closed stdin so the edit is rejected.
                Err(_) => return Ok(&[]),
            }
        }
        Ok(&self.buf[self.pos..])
    }
    fn consume(&mut self, amt: usize) {
        self.pos = (self.pos + amt).min(self.buf.len());
    }
}

pub fn spawn(
    mut session: Session,
    cli: Cli,
    commands: Receiver<Cmd>,
    events: Sender<Ev>,
) -> std::thread::JoinHandle<()> {
    let progress = events.clone();
    session.progress = Some(ProgressSink(Arc::new(move |e| {
        let _ = progress.send(Ev::Progress(e));
    })));
    std::thread::spawn(move || {
        while let Ok(cmd) = commands.recv() {
            handle(&mut session, &cli, cmd, &events);
        }
    })
}

fn switched(
    session: &Session,
    result: Result<Option<String>, crate::DevCodeError>,
    events: &Sender<Ev>,
) {
    let _ = events.send(match result {
        Ok(_) => Ev::Provider(session.provider_label()),
        Err(e) => Ev::Failed(e.to_string()),
    });
}

/// After connect / provider switch / reload, fetch the catalog so `/model` is ready
/// (same pattern as peer TUIs that populate a model picker on auth).
fn prefetch_models(session: &Session, events: &Sender<Ev>) {
    let _ = events.send(Ev::Models(session.list_models().map_err(|e| e.to_string())));
}

fn handle(session: &mut Session, cli: &Cli, cmd: Cmd, events: &Sender<Ev>) {
    match cmd {
        Cmd::Prompt(text) => {
            let ev = match session.prompt(&text) {
                Ok(result) => Ev::Done(Box::new(result)),
                Err(e) => Ev::Failed(e.to_string()),
            };
            let _ = events.send(ev);
        }
        Cmd::SetProfile(profile) => session.profile = profile,
        Cmd::Connect { name, key } => {
            let result = session.connect_with(name.as_deref(), true, move || Ok(key));
            match result {
                Ok(()) => {
                    let _ = events.send(Ev::Provider(session.provider_label()));
                    prefetch_models(session, events);
                }
                Err(e) => {
                    let _ = events.send(Ev::Failed(e.to_string()));
                }
            }
        }
        Cmd::Connections => {
            let _ = events.send(match session.connections() {
                Ok(lines) if lines.is_empty() => Ev::Info(vec!["no stored connections".into()]),
                Ok(lines) => Ev::Info(lines),
                Err(e) => Ev::Failed(e.to_string()),
            });
        }
        Cmd::Reload => match session.reload(cli) {
            Ok(()) => {
                let _ = events.send(Ev::Provider(session.provider_label()));
                prefetch_models(session, events);
            }
            Err(e) => {
                let _ = events.send(Ev::Failed(e.to_string()));
            }
        },
        Cmd::ListModels => {
            prefetch_models(session, events);
        }
        Cmd::ListEfforts => {
            let _ = events.send(Ev::Efforts(session.effort_levels()));
        }
        Cmd::SetModel(id) => {
            let result = session.set_model(&id);
            switched(session, result, events);
        }
        Cmd::SetProvider(name) => {
            match session.set_provider(&name) {
                Ok(_) => {
                    let _ = events.send(Ev::Provider(session.provider_label()));
                    // A switch without a stored key works until the first request; say so now.
                    let stored = session.connections().unwrap_or_default();
                    let has_key = stored
                        .iter()
                        .any(|line| line.starts_with(&format!("{name} ")));
                    let env_set = crate::config::preset(&name).is_some_and(|p| {
                        !p.api_key_env.is_empty() && std::env::var(p.api_key_env).is_ok()
                    });
                    if !has_key && !env_set {
                        let _ = events.send(Ev::Info(vec![format!(
                            "no stored key for {name}; run /connect {name}"
                        )]));
                    }
                    prefetch_models(session, events);
                }
                Err(e) => {
                    let _ = events.send(Ev::Failed(e.to_string()));
                }
            }
        }
        Cmd::SetEffort(level) => {
            let result = session.set_effort(level.as_deref());
            switched(session, result, events);
        }
        Cmd::Rollback(step) => {
            let root = match std::env::current_dir() {
                Ok(r) => r,
                Err(e) => {
                    let _ = events.send(Ev::Failed(e.to_string()));
                    return;
                }
            };
            let _ = events.send(match crate::manifest::rollback_latest(&root, step) {
                Ok(report) => {
                    let mut lines: Vec<String> = report
                        .restored_paths
                        .iter()
                        .map(|p| format!("restored {}", p.display()))
                        .collect();
                    lines.extend(
                        report
                            .removed_paths
                            .iter()
                            .map(|p| format!("removed {}", p.display())),
                    );
                    if lines.is_empty() {
                        lines.push("nothing to restore".into());
                    }
                    Ev::Info(lines)
                }
                Err(crate::manifest::ManifestError::ManifestNotFound { .. }) => {
                    Ev::Info(vec!["nothing to roll back".into()])
                }
                Err(e) => Ev::Failed(e.to_string()),
            });
        }
    }
}
