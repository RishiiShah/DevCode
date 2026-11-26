//! Terminal UI: transcript, input line, and modal approvals on top of the existing agent loop.
//!
//! The UI thread owns the terminal. A worker thread owns the `Session`. Approval questions
//! flow worker -> UI as text, answers flow UI -> worker as lines (see `worker`).
pub mod app;
mod clipboard;
mod fuzzy;
pub mod markdown;
pub mod view;
pub mod worker;

use crate::approval::ApprovalPrompt;
use crate::{Cli, DevCodeError};
use app::{Action, App};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use crossterm::execute;
use std::sync::mpsc;
use std::time::Duration;
use worker::{ChannelReader, ChannelWriter, Cmd, Ev};

/// True when the interactive TUI can run: both ends are terminals.
pub fn available() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

pub fn run(cli: Cli) -> Result<(), DevCodeError> {
    let (ev_tx, ev_rx) = mpsc::channel::<Ev>();
    let (ans_tx, ans_rx) = mpsc::channel::<String>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();

    let prompt = ApprovalPrompt::new(
        ChannelReader::new(ans_rx, ev_tx.clone()),
        ChannelWriter(ev_tx.clone()),
        true,
        cli.yes,
    );
    let session = crate::open_session(&cli, prompt)?;
    let shared = Shared {
        cancel: session.cancel.clone(),
        steer: session.steer.clone(),
        processes: session.process_control(),
        events: ev_tx.clone(),
    };
    let startup_note = session.startup_note();
    let prefetch = session.has_provider() && startup_note.is_none();
    let mut app = App::new(session.profile, session.provider_label(), startup_note);
    if let Some(notice) = session.legacy_state_notice() {
        app.on_event(Ev::Info(vec![notice]));
    }
    app.context_window = session.context_window();
    app.context_source = session.context_window_source();
    app.capability_status = session.capability_status();
    if let Some(items) = session.statusline() {
        app.statusline = items;
    }
    let _worker = worker::spawn(session, cli.clone(), cmd_rx, ev_tx);
    // Same as peers: load the model catalog as soon as a provider is configured.
    if prefetch {
        let _ = cmd_tx.send(Cmd::ListModels);
    }
    // `--continue` takes the latest conversation, `--resume ID` that one, bare `--resume` a picker.
    match cli.resume.as_deref() {
        Some("") => {
            for action in app.open_resume() {
                if let Action::Send(cmd) = action {
                    let _ = cmd_tx.send(cmd);
                }
            }
        }
        Some(id) => {
            let _ = cmd_tx.send(Cmd::Resume(id.to_string()));
        }
        None if cli.continue_last => {
            let _ = cmd_tx.send(Cmd::Resume(String::new()));
        }
        None => {}
    }

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture);
    let result = event_loop(&mut terminal, &mut app, &cmd_tx, &ans_tx, &ev_rx, &shared);
    let _ = execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        DisableMouseCapture
    );
    ratatui::restore();
    result.map_err(|e| DevCodeError::Config(e.to_string()))
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    cmds: &mpsc::Sender<Cmd>,
    answers: &mpsc::Sender<String>,
    events: &mpsc::Receiver<Ev>,
    shared: &Shared,
) -> std::io::Result<()> {
    let Shared { cancel, steer, .. } = shared;
    loop {
        while let Ok(ev) = events.try_recv() {
            app.on_event(ev);
        }
        if let Some(cmd) = app.next_queued() {
            // The run ended before reading them; they go out as the next prompt instead.
            if let Ok(mut inbox) = steer.lock() {
                inbox.clear();
            }
            cancel.reset();
            let _ = cmds.send(cmd);
        }
        app.tick = app.tick.wrapping_add(1);
        app.on_tick();
        terminal.draw(|f| view::draw(f, app))?;
        if let Some(text) = app.copied.take() {
            let chars = text.chars().count();
            let notice = if clipboard::copy(&text) {
                format!("copied {chars} chars")
            } else {
                format!("sent {chars} chars to the terminal clipboard")
            };
            app.flash = Some((notice, std::time::Instant::now()));
        }
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let actions = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
            Event::Paste(text) => {
                app.on_paste(&text);
                Vec::new()
            }
            Event::Mouse(mouse) => {
                app.on_mouse(mouse);
                Vec::new()
            }
            _ => Vec::new(),
        };
        for action in actions {
            match action {
                Action::Send(cmd) => {
                    if matches!(cmd, Cmd::Prompt(_)) {
                        cancel.reset();
                    }
                    let _ = cmds.send(cmd);
                }
                Action::Answer(line) => {
                    let _ = answers.send(line);
                }
                Action::Init(provider) => init(app, cmds, &provider),
                Action::Steer(text) => {
                    if let Ok(mut inbox) = steer.lock() {
                        inbox.push(text);
                    }
                }
                Action::Unsteer(text) => {
                    if let Ok(mut inbox) = steer.lock() {
                        if let Some(at) = inbox.iter().rposition(|t| *t == text) {
                            inbox.remove(at);
                        }
                    }
                }
                Action::Processes => shared.on_processes(None),
                Action::StopProcess(id) => shared.on_processes(Some(id)),
                Action::Cancel => cancel.cancel(),
                Action::Quit => {
                    cancel.cancel();
                    return Ok(());
                }
            }
        }
    }
}

/// What the UI thread shares with the worker's session.
struct Shared {
    cancel: crate::agent::CancelToken,
    steer: crate::agent::SteerInbox,
    processes: Option<crate::tools::managed::ProcessControl>,
    events: mpsc::Sender<Ev>,
}

impl Shared {
    /// /ps and /stop run beside the agent: a helper thread waits for the tool context (free
    /// between tool calls) and reports back, so the UI never blocks.
    fn on_processes(&self, stop: Option<String>) {
        let Some(control) = self.processes.clone() else {
            let _ = self
                .events
                .send(Ev::Info(vec!["no background processes".into()]));
            return;
        };
        let events = self.events.clone();
        std::thread::spawn(move || {
            let lines = match stop {
                Some(id) => vec![control.stop(&id).unwrap_or_else(|e| format!("✗ {e}"))],
                None => {
                    let lines = control.list();
                    if lines.is_empty() {
                        vec!["no background processes".into()]
                    } else {
                        lines
                    }
                }
            };
            let _ = events.send(Ev::Info(lines));
            let _ = events.send(Ev::Processes(control.running()));
        });
    }
}

/// Writes `.devcode.json` (never overwrites) and asks the worker to reload it.
fn init(app: &mut App, cmds: &mpsc::Sender<Cmd>, provider: &str) {
    use app::Item;
    let root = match std::env::current_dir() {
        Ok(r) => r,
        Err(e) => {
            app.items.push(Item::Error(e.to_string()));
            return;
        }
    };
    let path = root.join(".devcode.json");
    if path.exists() {
        app.items.push(Item::Error(
            ".devcode.json already exists; edit it directly".into(),
        ));
        return;
    }
    let test_command = app::detect_test_command(&root);
    let Some(text) = app::init_template(provider, test_command) else {
        app.items.push(Item::Error(format!(
            "unknown provider '{provider}'; choose one of: {}",
            crate::config::PRESETS.map(|p| p.name).join(", ")
        )));
        return;
    };
    if let Err(e) = std::fs::write(&path, text) {
        app.items
            .push(Item::Error(format!("cannot write .devcode.json: {e}")));
        return;
    }
    app.items.push(Item::Info(format!(
        "wrote .devcode.json for {provider}{}",
        match test_command {
            Some(cmd) => format!(" (test command: {cmd})"),
            None =>
                ". No test command detected; set verification.test_command for build runs".into(),
        }
    )));
    app.items
        .push(Item::Info("next: /connect to store your API key".into()));
    let _ = cmds.send(Cmd::Reload);
}
