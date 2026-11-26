//! Terminal UI: transcript, input line, and modal approvals on top of the existing agent loop.
//!
//! The UI thread owns the terminal. A worker thread owns the `Session`. Approval questions
//! flow worker -> UI as text, answers flow UI -> worker as lines (see `worker`).
pub mod app;
mod fuzzy;
pub mod markdown;
pub mod view;
pub mod worker;

use crate::approval::ApprovalPrompt;
use crate::{Cli, DevCodeError};
use app::{Action, App};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind, MouseEventKind,
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
    let cancel = session.cancel.clone();
    let prefetch = session.has_provider();
    let mut app = App::new(
        session.profile,
        session.provider_label(),
        session.startup_note(),
    );
    let _worker = worker::spawn(session, cli.clone(), cmd_rx, ev_tx);
    // Same as peers: load the model catalog as soon as a provider is configured.
    if prefetch {
        let _ = cmd_tx.send(Cmd::ListModels);
    }

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture);
    let result = event_loop(
        &mut terminal,
        &mut app,
        &cmd_tx,
        &ans_tx,
        &ev_rx,
        &cancel,
        &cli,
    );
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
    cancel: &crate::agent::CancelToken,
    cli: &Cli,
) -> std::io::Result<()> {
    loop {
        while let Ok(ev) = events.try_recv() {
            app.on_event(ev);
        }
        app.tick = app.tick.wrapping_add(1);
        terminal.draw(|f| view::draw(f, app))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let actions = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key),
            Event::Paste(text) => {
                app.on_paste(&text);
                Vec::new()
            }
            Event::Mouse(mouse) if app.modal.is_none() => {
                match mouse.kind {
                    MouseEventKind::ScrollUp => app.on_mouse_scroll(true),
                    MouseEventKind::ScrollDown => app.on_mouse_scroll(false),
                    _ => {}
                }
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
                Action::Init(provider) => init(app, cmds, &provider, cli),
                Action::Cancel => cancel.cancel(),
                Action::Quit => {
                    cancel.cancel();
                    return Ok(());
                }
            }
        }
    }
}

/// Writes `.devcode.json` (never overwrites) and asks the worker to reload it.
fn init(app: &mut App, cmds: &mpsc::Sender<Cmd>, provider: &str, _cli: &Cli) {
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
