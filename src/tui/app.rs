//! TUI state and input handling. Pure: no terminal and no threads, so it is unit-tested.
use super::worker::{Cmd, Ev};
use crate::agent::{ExitReason, LoopResult, ProgressEvent};
use crate::Profile;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    User(String),
    Assistant(String),
    Tool { name: String, summary: String },
    ToolResult { ok: bool, preview: String },
    Info(String),
    Error(String),
    Note(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    /// Edit approval: yes / always / no, with the diff text.
    Approve {
        text: String,
        scroll: u16,
    },
    /// Yes / no question (destructive command, out-of-scope edit).
    Confirm {
        text: String,
        scroll: u16,
    },
    /// Free-text answer (dependency justification).
    Text {
        text: String,
        input: String,
    },
    /// Masked API key entry.
    Secret {
        name: Option<String>,
        input: String,
    },
    ConfirmRollback {
        step: Option<usize>,
    },
    Help,
}

/// What the event loop must do after the app handled an input.
pub enum Action {
    Send(Cmd),
    /// Answer line for the blocked approval prompt.
    Answer(String),
    /// Write `.devcode.json` for this provider, then reload.
    Init(String),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Working { step: usize },
}

pub struct App {
    pub items: Vec<Item>,
    pub input: String,
    /// Cursor position in chars.
    pub cursor: usize,
    /// Lines scrolled up from the bottom; 0 follows new output.
    pub scroll: usize,
    pub profile: Profile,
    pub provider: String,
    pub status: Status,
    pub started: Option<Instant>,
    pub run_in: u64,
    pub run_out: u64,
    pub total_in: u64,
    pub total_out: u64,
    pub modal: Option<Modal>,
    pub history: Vec<String>,
    history_pos: Option<usize>,
    pending_prompt: String,
    quit_armed: bool,
    pub tick: usize,
}

const HELP: &str = "Enter       send message or command
/ask /plan /build   switch profile (ask and plan are read-only)
/connect [NAME]     store an API key (masked input)
/connections        list stored connections
/init [nim|gemini]  write .devcode.json for this folder
/rollback [N]       undo the latest run (or step N and later)
/clear              clear the screen
/exit               quit
PgUp/PgDn           scroll    Up/Down  input history
Ctrl-C              cancel prompt, or quit (twice while working)";

impl App {
    pub fn new(profile: Profile, provider: String) -> Self {
        let mut app = Self {
            items: Vec::new(),
            input: String::new(),
            cursor: 0,
            scroll: 0,
            profile,
            provider,
            status: Status::Idle,
            started: None,
            run_in: 0,
            run_out: 0,
            total_in: 0,
            total_out: 0,
            modal: None,
            history: Vec::new(),
            history_pos: None,
            pending_prompt: String::new(),
            quit_armed: false,
            tick: 0,
        };
        app.items.push(Item::Note(
            "Describe a task, or type /help. Edits ask for approval and can be undone with /rollback."
                .into(),
        ));
        if app.provider == "no provider" {
            app.items.push(Item::Info(
                "No provider configured. Run /init nim (or /init gemini), then /connect.".into(),
            ));
        }
        app
    }

    pub fn is_working(&self) -> bool {
        matches!(self.status, Status::Working { .. })
    }

    // ---------- worker events ----------

    pub fn on_event(&mut self, ev: Ev) {
        match ev {
            Ev::Progress(p) => self.on_progress(p),
            Ev::PromptText(text) => self.pending_prompt.push_str(&text),
            Ev::NeedInput => {
                let text = std::mem::take(&mut self.pending_prompt);
                self.modal = Some(classify_prompt(text));
            }
            Ev::Done(result) => self.on_done(*result),
            Ev::Failed(message) => {
                self.status = Status::Idle;
                self.pending_prompt.clear();
                self.items.push(Item::Error(hint_for(&message)));
            }
            Ev::Info(lines) => {
                for line in lines {
                    self.items.push(Item::Info(line));
                }
            }
            Ev::Provider(label) => {
                self.provider = label;
                self.items.push(Item::Info("provider ready".into()));
            }
        }
        self.scroll = 0;
    }

    fn on_progress(&mut self, p: ProgressEvent) {
        match p {
            ProgressEvent::Thinking { step } => self.status = Status::Working { step },
            ProgressEvent::Assistant { text } => self.items.push(Item::Assistant(text)),
            ProgressEvent::ToolCall { name, summary } => {
                self.items.push(Item::Tool { name, summary })
            }
            ProgressEvent::ToolResult { ok, preview, .. } => {
                self.items.push(Item::ToolResult { ok, preview })
            }
            ProgressEvent::Usage { input, output } => {
                self.run_in = input;
                self.run_out = output;
            }
        }
    }

    fn on_done(&mut self, result: LoopResult) {
        self.status = Status::Idle;
        self.pending_prompt.clear();
        self.total_in += result.total_input_tokens;
        self.total_out += result.total_output_tokens;
        self.run_in = 0;
        self.run_out = 0;
        if let Some(v) = &result.verification {
            for line in v.lines().filter(|l| !l.trim().is_empty()) {
                self.items.push(Item::Note(line.to_string()));
            }
        }
        if let Some(s) = &result.summary {
            self.items.push(Item::Note(s.format_line()));
        }
        if let Some((path, count)) = &result.no_progress {
            self.items.push(Item::Error(format!(
                "no progress: {path} repeated {count} times"
            )));
        }
        match result.exit_reason {
            ExitReason::Completed | ExitReason::NoProgress => {}
            ExitReason::RateLimited => self.items.push(Item::Error(format!(
                "rate limited after {} steps; try again shortly",
                result.steps
            ))),
            other => self
                .items
                .push(Item::Error(format!("stopped early: {other:?}"))),
        }
    }

    // ---------- keyboard ----------

    pub fn on_paste(&mut self, text: &str) {
        let clean: String = text.replace(['\r', '\n'], " ");
        match &mut self.modal {
            Some(Modal::Secret { input, .. }) | Some(Modal::Text { input, .. }) => {
                input.push_str(clean.trim());
            }
            Some(_) => {}
            None => {
                for c in clean.chars() {
                    self.insert(c);
                }
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return self.on_ctrl_c();
        }
        self.quit_armed = false;
        if self.modal.is_some() {
            return self.on_modal_key(key);
        }
        match key.code {
            KeyCode::Enter => return self.submit(),
            KeyCode::Char('d') if ctrl => {
                if self.input.is_empty() && !self.is_working() {
                    return vec![Action::Quit];
                }
            }
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.input.chars().count(),
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Char(c) if !ctrl => self.insert(c),
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let at = self.byte_at(self.cursor - 1);
                    self.input.remove(at);
                    self.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.input.chars().count() {
                    let at = self.byte_at(self.cursor);
                    self.input.remove(at);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.chars().count()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Esc => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Up => self.history_prev(),
            KeyCode::Down => self.history_next(),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            _ => {}
        }
        Vec::new()
    }

    fn on_ctrl_c(&mut self) -> Vec<Action> {
        match self.modal.take() {
            Some(Modal::Approve { .. })
            | Some(Modal::Confirm { .. })
            | Some(Modal::Text { .. }) => {
                self.items
                    .push(Item::Info("cancelled; edit rejected".into()));
                return vec![Action::Answer("n".into())];
            }
            Some(_) => return Vec::new(),
            None => {}
        }
        if !self.is_working() || self.quit_armed {
            return vec![Action::Quit];
        }
        self.quit_armed = true;
        self.items.push(Item::Info(
            "A run is in progress. Press Ctrl-C again to quit (completed edits stay; /rollback undoes them)."
                .into(),
        ));
        Vec::new()
    }

    fn on_modal_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Some(modal) = self.modal.take() else {
            return Vec::new();
        };
        match modal {
            Modal::Approve { text, scroll } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.answer("y", "edit approved"),
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.answer("a", "edit approved; will approve all edits this session")
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.answer("n", "edit rejected")
                }
                code => {
                    self.modal = Some(Modal::Approve {
                        scroll: scrolled(scroll, code),
                        text,
                    });
                    Vec::new()
                }
            },
            Modal::Confirm { text, scroll } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.answer("y", "approved"),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.answer("n", "rejected")
                }
                code => {
                    self.modal = Some(Modal::Confirm {
                        scroll: scrolled(scroll, code),
                        text,
                    });
                    Vec::new()
                }
            },
            Modal::Text { text, mut input } => match key.code {
                KeyCode::Enter => {
                    let line = input.trim().to_string();
                    if line.is_empty() {
                        self.answer("n", "rejected")
                    } else {
                        self.answer(&line, "justification recorded")
                    }
                }
                KeyCode::Esc => self.answer("n", "rejected"),
                KeyCode::Backspace => {
                    input.pop();
                    self.modal = Some(Modal::Text { text, input });
                    Vec::new()
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    self.modal = Some(Modal::Text { text, input });
                    Vec::new()
                }
                _ => {
                    self.modal = Some(Modal::Text { text, input });
                    Vec::new()
                }
            },
            Modal::Secret { name, mut input } => match key.code {
                KeyCode::Enter => {
                    if input.is_empty() {
                        self.items
                            .push(Item::Error("API key cannot be empty".into()));
                        return Vec::new();
                    }
                    self.items.push(Item::Info("storing key…".into()));
                    vec![Action::Send(Cmd::Connect { name, key: input })]
                }
                KeyCode::Esc => Vec::new(),
                KeyCode::Backspace => {
                    input.pop();
                    self.modal = Some(Modal::Secret { name, input });
                    Vec::new()
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    self.modal = Some(Modal::Secret { name, input });
                    Vec::new()
                }
                _ => {
                    self.modal = Some(Modal::Secret { name, input });
                    Vec::new()
                }
            },
            Modal::ConfirmRollback { step } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    vec![Action::Send(Cmd::Rollback(step))]
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Vec::new(),
                _ => {
                    self.modal = Some(Modal::ConfirmRollback { step });
                    Vec::new()
                }
            },
            Modal::Help => Vec::new(),
        }
    }

    fn answer(&mut self, line: &str, note: &str) -> Vec<Action> {
        self.items.push(Item::Info(note.into()));
        vec![Action::Answer(line.to_string())]
    }

    // ---------- input line ----------

    fn byte_at(&self, char_index: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_index)
            .map_or(self.input.len(), |(i, _)| i)
    }

    fn insert(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        let at = self.byte_at(self.cursor);
        self.input.insert(at, c);
        self.cursor += 1;
    }

    fn delete_word(&mut self) {
        let chars: Vec<char> = self.input.chars().collect();
        let mut start = self.cursor;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let rest: String = chars[self.cursor..].iter().collect();
        let head: String = chars[..start].iter().collect();
        self.input = head + &rest;
        self.cursor = start;
    }

    fn set_input(&mut self, text: String) {
        self.cursor = text.chars().count();
        self.input = text;
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let pos = match self.history_pos {
            None => self.history.len() - 1,
            Some(0) => 0,
            Some(p) => p - 1,
        };
        self.history_pos = Some(pos);
        self.set_input(self.history[pos].clone());
    }

    fn history_next(&mut self) {
        match self.history_pos {
            None => {}
            Some(p) if p + 1 < self.history.len() => {
                self.history_pos = Some(p + 1);
                self.set_input(self.history[p + 1].clone());
            }
            Some(_) => {
                self.history_pos = None;
                self.set_input(String::new());
            }
        }
    }

    fn submit(&mut self) -> Vec<Action> {
        let line = self.input.trim().to_string();
        self.input.clear();
        self.cursor = 0;
        self.history_pos = None;
        if line.is_empty() {
            return Vec::new();
        }
        if self.history.last() != Some(&line) {
            self.history.push(line.clone());
        }
        self.scroll = 0;
        if let Some(rest) = line.strip_prefix('/') {
            return self.slash(rest.trim());
        }
        if self.is_working() {
            self.items
                .push(Item::Info("still working; wait for the current run".into()));
            return Vec::new();
        }
        self.items.push(Item::User(line.clone()));
        self.status = Status::Working { step: 0 };
        self.started = Some(Instant::now());
        self.run_in = 0;
        self.run_out = 0;
        vec![Action::Send(Cmd::Prompt(line))]
    }

    fn slash(&mut self, rest: &str) -> Vec<Action> {
        let mut words = rest.split_whitespace();
        let name = words.next().unwrap_or("");
        let arg = words.next();
        let busy = self.is_working();
        let busy_block = |items: &mut Vec<Item>| {
            items.push(Item::Info("wait for the current run to finish".into()));
            Vec::new()
        };
        match name {
            "ask" | "plan" | "build" => {
                if busy {
                    return busy_block(&mut self.items);
                }
                let profile = match name {
                    "ask" => Profile::Ask,
                    "plan" => Profile::Plan,
                    _ => Profile::Build,
                };
                self.profile = profile;
                self.items
                    .push(Item::Info(format!("profile: {}", profile.name())));
                vec![Action::Send(Cmd::SetProfile(profile))]
            }
            "connect" => {
                if busy {
                    return busy_block(&mut self.items);
                }
                if self.provider == "no provider" {
                    self.items.push(Item::Error(
                        "No provider configured. Run /init nim (or /init gemini) first.".into(),
                    ));
                    return Vec::new();
                }
                self.modal = Some(Modal::Secret {
                    name: arg.map(str::to_owned),
                    input: String::new(),
                });
                Vec::new()
            }
            "connections" => vec![Action::Send(Cmd::Connections)],
            "init" => {
                if busy {
                    return busy_block(&mut self.items);
                }
                vec![Action::Init(arg.unwrap_or("nim").to_string())]
            }
            "rollback" => {
                if busy {
                    return busy_block(&mut self.items);
                }
                let step = arg.and_then(|s| s.parse().ok());
                self.modal = Some(Modal::ConfirmRollback { step });
                Vec::new()
            }
            "clear" => {
                self.items.clear();
                Vec::new()
            }
            "help" | "?" => {
                self.modal = Some(Modal::Help);
                Vec::new()
            }
            "exit" | "quit" => vec![Action::Quit],
            other => {
                self.items
                    .push(Item::Error(format!("unknown command /{other}; try /help")));
                Vec::new()
            }
        }
    }
}

pub fn help_text() -> &'static str {
    HELP
}

fn scrolled(scroll: u16, code: KeyCode) -> u16 {
    match code {
        KeyCode::Up => scroll.saturating_sub(1),
        KeyCode::Down => scroll.saturating_add(1),
        KeyCode::PageUp => scroll.saturating_sub(10),
        KeyCode::PageDown => scroll.saturating_add(10),
        _ => scroll,
    }
}

/// Picks the modal for question text that the approval code wrote before blocking on input.
pub fn classify_prompt(text: String) -> Modal {
    if text.contains("[a]lways") {
        Modal::Approve { text, scroll: 0 }
    } else if text.contains("[y]es / [n]o") {
        Modal::Confirm { text, scroll: 0 }
    } else {
        Modal::Text {
            text,
            input: String::new(),
        }
    }
}

/// Turns terse core errors into next steps.
fn hint_for(message: &str) -> String {
    if message.contains("provider.model is missing") {
        format!("{message}. Run /init nim (or /init gemini), then /connect.")
    } else if message.contains("HTTP 401") || message.contains("HTTP 403") {
        format!("{message}. The key was rejected; run /connect to store a new one.")
    } else if message.contains("no API key")
        || message.contains("API key") && message.contains("not set")
    {
        format!("{message}. Run /connect to store a key.")
    } else {
        message.to_string()
    }
}

/// `.devcode.json` for a provider preset, or None for an unknown name.
pub fn init_template(provider: &str, test_command: Option<&str>) -> Option<String> {
    let (connector, base_url, model) = match provider {
        "nim" => (
            "nim",
            "https://integrate.api.nvidia.com/v1",
            "nvidia/nemotron-3.5-super-vl-120b-a12b",
        ),
        "gemini" => (
            "gemini",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-3.8-flash",
        ),
        _ => return None,
    };
    let mut value = serde_json::json!({
        "provider": {
            "kind": "openai_compat",
            "base_url": base_url,
            "connector": connector,
            "model": model,
            "context_window": 32000
        }
    });
    if let Some(cmd) = test_command {
        value["verification"] = serde_json::json!({ "test_command": cmd });
    }
    serde_json::to_string_pretty(&value).ok().map(|s| s + "\n")
}

/// Guesses a test command from marker files; None when unsure.
pub fn detect_test_command(root: &std::path::Path) -> Option<&'static str> {
    let has = |name: &str| root.join(name).exists();
    if has("Cargo.toml") {
        Some("cargo test")
    } else if has("package.json") {
        Some("npm test")
    } else if has("go.mod") {
        Some("go test ./...")
    } else if has("pyproject.toml") || has("pytest.ini") {
        Some("python3 -m pytest -q")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Press)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new_with_kind(KeyCode::Char(c), KeyModifiers::CONTROL, KeyEventKind::Press)
    }
    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }
    fn app() -> App {
        App::new(Profile::Build, "nim · m".into())
    }

    #[test]
    fn prompt_goes_to_worker_and_marks_working() {
        let mut a = app();
        type_str(&mut a, "fix the bug");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&actions[..], [Action::Send(Cmd::Prompt(p))] if p == "fix the bug"));
        assert!(a.is_working());
        assert!(a.items.contains(&Item::User("fix the bug".into())));
        assert!(a.input.is_empty());
    }

    #[test]
    fn second_prompt_while_working_is_not_sent() {
        let mut a = app();
        type_str(&mut a, "one");
        a.on_key(key(KeyCode::Enter));
        type_str(&mut a, "two");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
    }

    #[test]
    fn edit_prompt_opens_approve_modal_and_answers() {
        let mut a = app();
        a.on_event(Ev::PromptText(
            "--- a/x\n+++ b/x\nApprove edit to 'x'? [y]es / [a]lways / [n]o: ".into(),
        ));
        a.on_event(Ev::NeedInput);
        assert!(matches!(a.modal, Some(Modal::Approve { .. })));
        let actions = a.on_key(key(KeyCode::Char('a')));
        assert!(matches!(&actions[..], [Action::Answer(s)] if s == "a"));
        assert!(a.modal.is_none());
    }

    #[test]
    fn command_prompt_is_yes_no_and_esc_rejects() {
        let mut a = app();
        a.on_event(Ev::PromptText(
            "Destructive command detected: 'rm x' (r). Approve? [y]es / [n]o: ".into(),
        ));
        a.on_event(Ev::NeedInput);
        assert!(matches!(a.modal, Some(Modal::Confirm { .. })));
        let actions = a.on_key(key(KeyCode::Esc));
        assert!(matches!(&actions[..], [Action::Answer(s)] if s == "n"));
    }

    #[test]
    fn dependency_prompt_takes_text() {
        let mut a = app();
        a.on_event(Ev::PromptText(
            "Dependency addition detected in 'Cargo.toml'. Enter justification to approve: ".into(),
        ));
        a.on_event(Ev::NeedInput);
        assert!(matches!(a.modal, Some(Modal::Text { .. })));
        type_str(&mut a, "needed for tui");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&actions[..], [Action::Answer(s)] if s == "needed for tui"));
    }

    #[test]
    fn secret_modal_sends_connect_and_keeps_key_out_of_transcript() {
        let mut a = app();
        type_str(&mut a, "/connect gemini");
        a.on_key(key(KeyCode::Enter));
        assert!(matches!(a.modal, Some(Modal::Secret { .. })));
        a.on_paste("sk-secret-123");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(
            matches!(&actions[..], [Action::Send(Cmd::Connect { name: Some(n), key })] if n == "gemini" && key == "sk-secret-123")
        );
        let dump = format!("{:?}", a.items);
        assert!(!dump.contains("sk-secret-123"));
        assert!(!format!("{:?}", a.history).contains("sk-secret-123"));
    }

    #[test]
    fn ctrl_c_cancels_prompt_then_quits_when_idle() {
        let mut a = app();
        a.on_event(Ev::PromptText("Approve? [y]es / [n]o: ".into()));
        a.on_event(Ev::NeedInput);
        let actions = a.on_key(ctrl('c'));
        assert!(matches!(&actions[..], [Action::Answer(s)] if s == "n"));
        assert!(matches!(&a.on_key(ctrl('c'))[..], [Action::Quit]));
    }

    #[test]
    fn ctrl_c_needs_two_presses_while_working() {
        let mut a = app();
        type_str(&mut a, "go");
        a.on_key(key(KeyCode::Enter));
        assert!(a.on_key(ctrl('c')).is_empty());
        assert!(matches!(&a.on_key(ctrl('c'))[..], [Action::Quit]));
    }

    #[test]
    fn profile_slash_commands_switch_profile() {
        let mut a = app();
        type_str(&mut a, "/plan");
        let actions = a.on_key(key(KeyCode::Enter));
        assert_eq!(a.profile, Profile::Plan);
        assert!(matches!(
            &actions[..],
            [Action::Send(Cmd::SetProfile(Profile::Plan))]
        ));
    }

    #[test]
    fn rollback_asks_first() {
        let mut a = app();
        type_str(&mut a, "/rollback");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        assert!(matches!(
            a.modal,
            Some(Modal::ConfirmRollback { step: None })
        ));
        assert!(matches!(
            &a.on_key(key(KeyCode::Char('y')))[..],
            [Action::Send(Cmd::Rollback(None))]
        ));
    }

    #[test]
    fn history_and_editing() {
        let mut a = app();
        type_str(&mut a, "/clear");
        a.on_key(key(KeyCode::Enter));
        a.on_key(key(KeyCode::Up));
        assert_eq!(a.input, "/clear");
        a.on_key(key(KeyCode::Backspace));
        a.on_key(ctrl('w'));
        assert_eq!(a.input, "");
        type_str(&mut a, "héllo");
        a.on_key(key(KeyCode::Left));
        a.on_key(key(KeyCode::Backspace));
        assert_eq!(a.input, "hélo");
    }

    #[test]
    fn done_records_summary_and_tokens() {
        use crate::agent::LoopResult;
        let mut a = app();
        a.on_event(Ev::Done(Box::new(LoopResult {
            exit_reason: ExitReason::StepLimitExceeded,
            steps: 3,
            total_input_tokens: 10,
            total_output_tokens: 4,
            messages: Vec::new(),
            summary: None,
            verification: Some("Verification Summary: ok\n".into()),
            no_progress: None,
        })));
        assert_eq!((a.total_in, a.total_out), (10, 4));
        assert!(a
            .items
            .iter()
            .any(|i| matches!(i, Item::Error(e) if e.contains("stopped early"))));
        assert!(a
            .items
            .contains(&Item::Note("Verification Summary: ok".into())));
    }

    #[test]
    fn init_template_is_valid_config() {
        let json = init_template("gemini", Some("cargo test")).unwrap();
        let parsed = crate::parse_config(&json).expect("template must parse");
        assert_eq!(
            parsed.provider.unwrap().connector.as_deref(),
            Some("gemini")
        );
        assert!(init_template("nope", None).is_none());
    }
}
