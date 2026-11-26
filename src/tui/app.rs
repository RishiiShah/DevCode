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
    /// Filterable list: models (fetched from the provider), providers, reasoning effort.
    Picker {
        kind: PickKind,
        items: Vec<String>,
        filter: String,
        selected: usize,
        loading: bool,
    },
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    Model,
    Provider,
    Effort,
    Command,
}

impl Modal {
    /// Items matching the filter via fuzzy subsequence rank. Empty for non-pickers.
    pub fn visible(&self) -> Vec<&str> {
        match self {
            Modal::Picker { items, filter, .. } => super::fuzzy::filter_ranked(items, filter),
            _ => Vec::new(),
        }
    }
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
    /// A new model call started; its assistant text must not merge into an earlier partial bubble.
    fresh_bubble: bool,
    pub tick: usize,
    /// Index in `items` of an `Item::Tool` row shown expanded; `None` = all tool cards collapsed.
    pub expanded_tool: Option<usize>,
    /// Last model catalog from the provider (`/models` prefetch after connect/switch).
    pub models: Vec<String>,
}

const HELP: &str = "Enter       send message or command
/ask /plan /build   switch profile (ask and plan are read-only)
/model [ID]         pick a model (catalog loads on connect/switch) or set one
/provider [NAME]    switch provider (saved as your default; refreshes models)
/effort [LEVEL]     reasoning effort; lists only what the current model accepts
/connect [NAME]     store an API key (masked input; saves as default)
/connections        list accounts (stored|env|missing; * = active)
/init [NAME]        optional project scaffold (.devcode.json)
/rollback [N]       undo the latest run (or step N and later)
/clear              clear the screen
/exit               quit
PgUp/PgDn           scroll transcript
Ctrl-End            jump to latest (when scrolled up)
Ctrl-T              expand/collapse latest tool card
Ctrl-K              command palette (search slash commands)
Up/Down             transcript scroll (non-empty: move line; top/bottom edge scrolls)
Ctrl-P / Ctrl-N     input history (works even when typing)
Shift+Enter         new line in message (Alt+Enter also)
Shift-Tab           cycle ask → plan → build
Ctrl-C              cancel prompt, or quit (twice while working)";

impl App {
    pub fn new(profile: Profile, provider: String, startup_note: Option<String>) -> Self {
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
            fresh_bubble: false,
            tick: 0,
            expanded_tool: None,
            models: Vec::new(),
        };
        app.items.push(Item::Note(
            "Describe a task, or type /help. Edits ask for approval and can be undone with /rollback."
                .into(),
        ));
        if let Some(note) = startup_note {
            app.items.push(Item::Info(note));
        } else if app.provider == "no provider" {
            app.items.push(Item::Info(
                "Run /provider nim or /provider gemini, then /connect.".into(),
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
                // Drop the previous provider's catalog; prefetch fills models next.
                self.models.clear();
                self.items.push(Item::Info(format!("now using {label}")));
                self.provider = label;
            }
            Ev::Efforts(levels) => {
                if matches!(
                    self.modal,
                    Some(Modal::Picker {
                        kind: PickKind::Effort,
                        ..
                    })
                ) {
                    if levels.is_empty() {
                        self.modal = None;
                        self.items.push(Item::Info(
                            "this model has no selectable reasoning effort".into(),
                        ));
                    } else if let Some(Modal::Picker { items, loading, .. }) = &mut self.modal {
                        *items = std::iter::once("default".to_string())
                            .chain(levels)
                            .collect();
                        *loading = false;
                    }
                }
            }
            Ev::Models(result) => self.on_models(result),
        }
    }

    fn on_models(&mut self, result: Result<Vec<String>, String>) {
        let picker_open = matches!(
            self.modal,
            Some(Modal::Picker {
                kind: PickKind::Model,
                ..
            })
        );
        match result {
            Ok(list) => {
                let n = list.len();
                self.models = list.clone();
                if let Some(Modal::Picker {
                    kind: PickKind::Model,
                    items,
                    loading,
                    ..
                }) = &mut self.modal
                {
                    *items = list;
                    *loading = false;
                } else if n > 0 {
                    self.items
                        .push(Item::Info(format!("{n} models ready — /model to pick")));
                }
            }
            Err(message) => {
                self.models.clear();
                if picker_open {
                    self.modal = None;
                    self.items.push(Item::Error(format!(
                        "cannot list models: {message}. Use /model ID to set one directly."
                    )));
                } else {
                    // Background prefetch (connect/provider) without a key is common.
                    self.items.push(Item::Info(format!(
                        "model list unavailable ({message}); /connect then /model"
                    )));
                }
            }
        }
    }

    fn merge_assistant_progress(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let fresh = std::mem::take(&mut self.fresh_bubble);
        if let (false, Some(Item::Assistant(last))) = (fresh, self.items.last_mut()) {
            if text == *last {
                return;
            }
            if text.starts_with(last.as_str()) {
                *last = text;
                return;
            }
            last.push_str(&text);
            return;
        }
        self.items.push(Item::Assistant(text));
    }

    fn on_progress(&mut self, p: ProgressEvent) {
        match p {
            ProgressEvent::Thinking { step } => {
                self.status = Status::Working { step };
                self.fresh_bubble = true;
            }
            ProgressEvent::Assistant { text } => self.merge_assistant_progress(text),
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
        // Verification details matter only when something changed.
        let changed = result
            .summary
            .as_ref()
            .is_some_and(|s| s.files_edited > 0 || s.files_changed_by_commands > 0);
        if let Some(v) = result.verification.as_ref().filter(|_| changed) {
            for line in v.lines().filter(|l| !l.trim().is_empty()) {
                self.items.push(Item::Note(line.to_string()));
            }
        }
        if let Some(s) = &result.summary {
            let mut parts = vec![
                format!("{} step{}", s.steps, if s.steps == 1 { "" } else { "s" }),
                format!("{} in / {} out tokens", s.tokens_in, s.tokens_out),
            ];
            let counts = [
                (s.files_edited, "file(s) edited"),
                (s.tests_changed, "test file(s) changed"),
                (s.files_changed_by_commands, "file(s) changed by commands"),
                (s.files_outside_scope, "outside scope"),
                (s.dependencies_added, "dependencies added"),
            ];
            parts.extend(
                counts
                    .iter()
                    .filter(|(n, _)| *n > 0)
                    .map(|(n, what)| format!("{n} {what}")),
            );
            self.items.push(Item::Note(parts.join(" · ")));
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
        let clean = text.replace("\r\n", "\n").replace('\r', "\n");
        match &mut self.modal {
            Some(Modal::Secret { input, .. }) | Some(Modal::Text { input, .. }) => {
                input.push_str(clean.replace('\n', " ").trim());
            }
            Some(_) => {}
            None => {
                for c in clean.chars() {
                    if c == '\n' {
                        self.insert_newline();
                    } else {
                        self.insert(c);
                    }
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
            KeyCode::Enter
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    || key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.insert_newline();
            }
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
            KeyCode::Char('t') if ctrl => self.toggle_expanded_tool(),
            KeyCode::Char('k') if ctrl => {
                self.open_command_palette();
            }
            KeyCode::Char('p') if ctrl => self.history_prev(),
            KeyCode::Char('n') if ctrl => self.history_next(),
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
            KeyCode::Home => self.cursor = self.line_start(self.cursor),
            KeyCode::End if ctrl => self.scroll = 0,
            KeyCode::End => self.cursor = self.line_end(self.cursor),
            KeyCode::Esc => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Up => self.input_up(),
            KeyCode::Down => self.input_down(),
            KeyCode::PageUp => self.scroll_transcript(10),
            KeyCode::PageDown => self.scroll_transcript(-10),
            KeyCode::BackTab if !self.is_working() => return self.cycle_profile(),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) && !self.is_working() => {
                return self.cycle_profile();
            }
            _ => {}
        }
        Vec::new()
    }

    fn cycle_profile(&mut self) -> Vec<Action> {
        let next = match self.profile {
            Profile::Ask => Profile::Plan,
            Profile::Plan => Profile::Build,
            Profile::Build => Profile::Ask,
        };
        self.profile = next;
        self.items
            .push(Item::Info(format!("profile: {}", next.name())));
        vec![Action::Send(Cmd::SetProfile(next))]
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
            Modal::Picker {
                kind,
                items,
                mut filter,
                mut selected,
                loading,
            } => {
                let mut modal = Modal::Picker {
                    kind,
                    items,
                    filter: filter.clone(),
                    selected,
                    loading,
                };
                let count = modal.visible().len();
                match key.code {
                    KeyCode::Esc => return Vec::new(),
                    KeyCode::Enter => {
                        let chosen = modal.visible().get(selected).map(|s| s.to_string());
                        // Nothing matches: for models, use the typed text as an id.
                        let chosen = chosen.or_else(|| {
                            (kind == PickKind::Model && !filter.trim().is_empty())
                                .then(|| filter.trim().to_string())
                        });
                        let Some(value) = chosen else {
                            self.modal = Some(modal);
                            return Vec::new();
                        };
                        return match kind {
                            PickKind::Command => {
                                let name = value.strip_prefix('/').unwrap_or(&value);
                                self.slash(name)
                            }
                            PickKind::Model => {
                                vec![Action::Send(Cmd::SetModel(value))]
                            }
                            PickKind::Provider => vec![Action::Send(Cmd::SetProvider(value))],
                            PickKind::Effort => vec![Action::Send(Cmd::SetEffort(
                                (value != "default").then_some(value),
                            ))],
                        };
                    }
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected = (selected + 1).min(count.saturating_sub(1)),
                    KeyCode::PageUp => selected = selected.saturating_sub(10),
                    KeyCode::PageDown => selected = (selected + 10).min(count.saturating_sub(1)),
                    KeyCode::Backspace => {
                        filter.pop();
                        selected = 0;
                    }
                    KeyCode::Char(c) if !c.is_control() => {
                        filter.push(c);
                        selected = 0;
                    }
                    _ => {}
                }
                if let Modal::Picker {
                    filter: f,
                    selected: s,
                    ..
                } = &mut modal
                {
                    *f = filter;
                    *s = selected;
                }
                self.modal = Some(modal);
                Vec::new()
            }
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

    fn insert_newline(&mut self) {
        let at = self.byte_at(self.cursor);
        self.input.insert(at, '\n');
        self.cursor += 1;
    }

    /// Character index of the start of the line that contains `cursor`.
    fn line_start(&self, cursor: usize) -> usize {
        self.input[..self.byte_at(cursor)]
            .rfind('\n')
            .map(|i| self.input[..=i].chars().count())
            .unwrap_or(0)
    }

    /// Character index just past the last character on the line that contains `cursor`.
    fn line_end(&self, cursor: usize) -> usize {
        let tail = &self.input[self.byte_at(cursor)..];
        tail.find('\n')
            .map(|i| cursor + tail[..i].chars().count())
            .unwrap_or_else(|| self.input.chars().count())
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

    /// Scroll the transcript away from the bottom (`delta` > 0 = older).
    pub fn scroll_transcript(&mut self, delta: i32) {
        if delta > 0 {
            self.scroll = self.scroll.saturating_add(delta as usize);
        } else if delta < 0 {
            self.scroll = self.scroll.saturating_sub((-delta) as usize);
        }
    }

    /// Mouse wheel: up = older transcript lines.
    pub fn on_mouse_scroll(&mut self, lines_up: bool) {
        if self.modal.is_some() {
            return;
        }
        let delta = if lines_up { 3 } else { -3 };
        self.scroll_transcript(delta);
    }

    fn input_up(&mut self) {
        if self.input.is_empty() {
            self.history_prev();
            return;
        }
        let line = self.cursor_line_index();
        if line > 0 {
            self.move_cursor_vert(-1);
        } else {
            self.scroll_transcript(1);
        }
    }

    fn input_down(&mut self) {
        if self.input.is_empty() {
            self.history_next();
            return;
        }
        let line = self.cursor_line_index();
        let last = self.input_line_count().saturating_sub(1);
        if line < last {
            self.move_cursor_vert(1);
        } else {
            self.scroll_transcript(-1);
        }
    }

    fn input_line_count(&self) -> usize {
        self.input.split('\n').count()
    }

    fn cursor_line_index(&self) -> usize {
        self.input[..self.byte_at(self.cursor)]
            .matches('\n')
            .count()
    }

    fn move_cursor_vert(&mut self, delta: i32) {
        let (line, col) = self.cursor_line_col();
        let lines = self.input_line_count();
        let new_line = (line as i32 + delta).clamp(0, lines as i32 - 1) as usize;
        if new_line == line {
            return;
        }
        let line_chars = self
            .input
            .split('\n')
            .nth(new_line)
            .map(|l| l.chars().count())
            .unwrap_or(0);
        self.cursor = self.cursor_at_line_col(new_line, col.min(line_chars));
    }

    fn cursor_line_col(&self) -> (usize, usize) {
        let before = &self.input[..self.byte_at(self.cursor)];
        let line = before.matches('\n').count();
        let col = before
            .rsplit_once('\n')
            .map_or(before.chars().count(), |(_, tail)| tail.chars().count());
        (line, col)
    }

    fn cursor_at_line_col(&self, target_line: usize, target_col: usize) -> usize {
        let mut char_idx = 0;
        for (line_idx, line) in self.input.split('\n').enumerate() {
            if line_idx == target_line {
                return char_idx + target_col.min(line.chars().count());
            }
            char_idx += line.chars().count() + 1;
        }
        char_idx
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

    fn open_command_palette(&mut self) {
        self.modal = Some(Modal::Picker {
            kind: PickKind::Command,
            items: slash_command_items()
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            filter: String::new(),
            selected: 0,
            loading: false,
        });
    }

    fn toggle_expanded_tool(&mut self) {
        let Some(idx) = self
            .items
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, item)| matches!(item, Item::Tool { .. }).then_some(i))
        else {
            return;
        };
        self.expanded_tool = if self.expanded_tool == Some(idx) {
            None
        } else {
            Some(idx)
        };
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
                        "Run /provider nim or /provider gemini, then /connect.".into(),
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
            "model" | "provider" | "effort" => {
                if busy {
                    return busy_block(&mut self.items);
                }
                self.pick(name, arg)
            }
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
                self.expanded_tool = None;
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

impl App {
    /// `/model`, `/provider`, `/effort`: apply an argument directly, or open the picker.
    fn pick(&mut self, command: &str, arg: Option<&str>) -> Vec<Action> {
        let picker = |kind, items: Vec<String>, loading| Modal::Picker {
            kind,
            items,
            filter: String::new(),
            selected: 0,
            loading,
        };
        match (command, arg) {
            ("model", Some(id)) => vec![Action::Send(Cmd::SetModel(id.to_string()))],
            ("provider", Some(name)) => vec![Action::Send(Cmd::SetProvider(name.to_string()))],
            ("effort", Some(level)) => vec![Action::Send(Cmd::SetEffort(
                (level != "default").then(|| level.to_string()),
            ))],
            ("model", None) => {
                if self.provider == "no provider" {
                    self.items.push(Item::Error(
                        "No provider yet. Run /provider nim or /provider gemini first.".into(),
                    ));
                    return Vec::new();
                }
                // Cache from connect/provider prefetch opens instantly; still refresh.
                if self.models.is_empty() {
                    self.modal = Some(picker(PickKind::Model, Vec::new(), true));
                } else {
                    self.modal = Some(picker(PickKind::Model, self.models.clone(), false));
                }
                vec![Action::Send(Cmd::ListModels)]
            }
            ("provider", None) => {
                let names = crate::config::PRESETS
                    .iter()
                    .map(|p| p.name.to_string())
                    .collect();
                self.modal = Some(picker(PickKind::Provider, names, false));
                Vec::new()
            }
            _ => {
                if self.provider == "no provider" {
                    self.items.push(Item::Error(
                        "No provider yet. Run /provider nim or /provider gemini first.".into(),
                    ));
                    return Vec::new();
                }
                // The worker knows the model, so it says which levels exist.
                self.modal = Some(picker(PickKind::Effort, Vec::new(), true));
                vec![Action::Send(Cmd::ListEfforts)]
            }
        }
    }
}

pub fn help_text() -> &'static str {
    HELP
}

/// Slash commands shown in the command palette (same set as `/help`).
pub fn slash_command_items() -> &'static [&'static str] {
    &[
        "/ask",
        "/plan",
        "/build",
        "/model",
        "/provider",
        "/effort",
        "/connect",
        "/connections",
        "/init",
        "/rollback",
        "/clear",
        "/help",
        "/exit",
    ]
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
    let message = &message.replace("model error: model error: ", "model error: ");
    if message.contains("devcode auth login") {
        format!("{message}. Run /connect or devcode auth login.")
    } else if message.contains("provider.model is missing") {
        format!("{message}. Run /provider nim or /provider gemini, then /connect.")
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
    let preset = crate::config::preset(provider)?;
    let (connector, base_url, model) = (preset.name, preset.base_url, preset.default_model);
    let mut value = serde_json::json!({
        "provider": {
            "kind": preset.kind.as_str(),
            "base_url": base_url,
            "connector": connector,
            "api_key_env": preset.api_key_env,
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
fn assistant_bubble_texts(items: &[Item]) -> Vec<&str> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Assistant(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
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
    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::CONTROL, KeyEventKind::Press)
    }
    fn shift_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::SHIFT, KeyEventKind::Press)
    }
    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }
    fn app() -> App {
        App::new(Profile::Build, "nim · m".into(), None)
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
    fn approve_modal_y_and_n_still_answer() {
        let mut a = app();
        a.on_event(Ev::PromptText(
            "--- a/x\n+++ b/x\nApprove edit to 'x'? [y]es / [a]lways / [n]o: ".into(),
        ));
        a.on_event(Ev::NeedInput);
        assert!(matches!(
            &a.on_key(key(KeyCode::Char('y')))[..],
            [Action::Answer(s)] if s == "y"
        ));
        assert!(a.modal.is_none());

        a.on_event(Ev::PromptText(
            "--- a/x\n+++ b/x\nApprove edit to 'x'? [y]es / [a]lways / [n]o: ".into(),
        ));
        a.on_event(Ev::NeedInput);
        assert!(matches!(
            &a.on_key(key(KeyCode::Char('n')))[..],
            [Action::Answer(s)] if s == "n"
        ));
        assert!(a.modal.is_none());
    }

    #[test]
    fn approve_modal_scroll_bounds_survive_page_keys() {
        use crate::tui::view::draw;
        use ratatui::{backend::TestBackend, Terminal};
        let mut text = String::from("--- a/x\n+++ b/x\n");
        for i in 0..80 {
            text.push_str(&format!("+line-{i:04}\n"));
        }
        text.push_str("Approve edit to 'x'? [y]es / [a]lways / [n]o: ");
        let mut a = app();
        a.on_event(Ev::PromptText(text));
        a.on_event(Ev::NeedInput);
        for _ in 0..200 {
            a.on_key(key(KeyCode::PageDown));
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let Modal::Approve { scroll, .. } = a.modal.clone().expect("modal") else {
            panic!("expected approve modal");
        };
        assert!(scroll < 200);
        let before = scroll;
        a.on_key(key(KeyCode::PageDown));
        terminal.draw(|f| draw(f, &mut a)).unwrap();
        let Modal::Approve { scroll, .. } = a.modal.clone().expect("modal") else {
            panic!("expected approve modal");
        };
        assert_eq!(scroll, before);
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
    fn empty_up_recalls_history() {
        let mut a = app();
        type_str(&mut a, "saved");
        a.on_key(key(KeyCode::Enter));
        a.on_key(key(KeyCode::Up));
        assert_eq!(a.input, "saved");
    }

    #[test]
    fn nonempty_up_does_not_recall_history() {
        let mut a = app();
        type_str(&mut a, "saved");
        a.on_key(key(KeyCode::Enter));
        type_str(&mut a, "typing");
        a.on_key(key(KeyCode::Up));
        assert_eq!(a.input, "typing");
        assert!(a.history_pos.is_none());
    }

    #[test]
    fn ctrl_p_recalls_history_while_typing() {
        let mut a = app();
        type_str(&mut a, "saved");
        a.on_key(key(KeyCode::Enter));
        type_str(&mut a, "partial");
        a.on_key(ctrl('p'));
        assert_eq!(a.input, "saved");
    }

    #[test]
    fn mouse_scroll_helper_moves_transcript() {
        let mut a = app();
        assert_eq!(a.scroll, 0);
        a.on_mouse_scroll(true);
        assert_eq!(a.scroll, 3);
        a.on_mouse_scroll(false);
        assert_eq!(a.scroll, 0);
    }

    #[test]
    fn multiline_up_moves_cursor_not_history() {
        let mut a = app();
        type_str(&mut a, "first");
        a.on_key(shift_key(KeyCode::Enter));
        type_str(&mut a, "second");
        a.on_key(key(KeyCode::Up));
        assert_eq!(a.cursor, 5);
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
            summary: Some(crate::discipline::TaskSummary {
                steps: 3,
                tokens_in: 10,
                tokens_out: 4,
                files_edited: 1,
                ..Default::default()
            }),
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
        assert!(a.items.contains(&Item::Note(
            "3 steps · 10 in / 4 out tokens · 1 file(s) edited".into()
        )));
    }

    #[test]
    fn missing_key_error_explains_connector_and_drops_double_prefix() {
        let mut a = app();
        a.on_event(Ev::Failed(
            "model error: model error: run `devcode auth login NAME` or set NVIDIA_API_KEY".into(),
        ));
        let Some(Item::Error(text)) = a.items.last() else {
            panic!("expected error item")
        };
        assert!(!text.contains("model error: model error"));
        assert!(text.contains("/connect"));
        assert!(text.contains("devcode auth login"));
    }

    fn picker(a: &App) -> &Modal {
        a.modal.as_ref().expect("modal open")
    }

    fn model_picker_modal(items: Vec<String>, filter: &str) -> Modal {
        Modal::Picker {
            kind: PickKind::Model,
            items,
            filter: filter.to_string(),
            selected: 0,
            loading: false,
        }
    }

    #[test]
    fn fuzzy_model_picker_typo_surfaces_nemotron() {
        let items = vec![
            "openai/gpt-4".into(),
            "meta/llama-3".into(),
            "nvidia/nemotron-70b-instruct".into(),
        ];
        let modal = model_picker_modal(items, "nemtron");
        assert_eq!(modal.visible(), vec!["nvidia/nemotron-70b-instruct"]);
        let modal = model_picker_modal(
            vec!["anthropic/claude".into(), "nvidia/nemotron-mini".into()],
            "nemo",
        );
        assert_eq!(modal.visible(), vec!["nvidia/nemotron-mini"]);
    }

    #[test]
    fn fuzzy_picker_empty_filter_keeps_item_order() {
        let items = vec!["first".into(), "second".into(), "third".into()];
        let modal = model_picker_modal(items.clone(), "");
        assert_eq!(
            modal.visible(),
            items.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fuzzy_picker_up_down_enter_unchanged() {
        let mut a = app();
        type_str(&mut a, "/model");
        a.on_key(key(KeyCode::Enter));
        a.on_event(Ev::Models(Ok(vec![
            "nvidia/nemotron-70b".into(),
            "openai/gpt-4".into(),
        ])));
        type_str(&mut a, "nemtron");
        assert_eq!(picker(&a).visible(), vec!["nvidia/nemotron-70b"]);
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(
            &actions[..],
            [Action::Send(Cmd::SetModel(m))] if m == "nvidia/nemotron-70b"
        ));
    }

    #[test]
    fn model_picker_loads_filters_and_selects() {
        let mut a = app();
        type_str(&mut a, "/model");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&actions[..], [Action::Send(Cmd::ListModels)]));
        assert!(matches!(picker(&a), Modal::Picker { loading: true, .. }));
        a.on_event(Ev::Models(Ok(vec![
            "alpha".into(),
            "beta-large".into(),
            "beta-small".into(),
        ])));
        type_str(&mut a, "beta");
        assert_eq!(picker(&a).visible(), vec!["beta-large", "beta-small"]);
        a.on_key(key(KeyCode::Down));
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&actions[..], [Action::Send(Cmd::SetModel(m))] if m == "beta-small"));
    }

    #[test]
    fn model_picker_accepts_typed_id_when_nothing_matches() {
        let mut a = app();
        type_str(&mut a, "/model");
        a.on_key(key(KeyCode::Enter));
        a.on_event(Ev::Models(Ok(vec!["alpha".into()])));
        type_str(&mut a, "custom-id");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&actions[..], [Action::Send(Cmd::SetModel(m))] if m == "custom-id"));
    }

    #[test]
    fn model_list_failure_closes_picker_with_hint() {
        let mut a = app();
        type_str(&mut a, "/model");
        a.on_key(key(KeyCode::Enter));
        a.on_event(Ev::Models(Err("HTTP 401".into())));
        assert!(a.modal.is_none());
        assert!(a
            .items
            .iter()
            .any(|i| matches!(i, Item::Error(e) if e.contains("/model ID"))));
    }

    #[test]
    fn connect_prefetch_caches_models_without_picker() {
        let mut a = app();
        a.on_event(Ev::Provider("gemini · gemini-3-flash-preview".into()));
        assert!(a.models.is_empty(), "provider switch clears catalog");
        a.on_event(Ev::Models(Ok(vec![
            "gemini-3-flash-preview".into(),
            "gemini-flash-lite-latest".into(),
        ])));
        assert_eq!(a.models.len(), 2);
        assert!(a.modal.is_none());
        assert!(a
            .items
            .iter()
            .any(|i| matches!(i, Item::Info(s) if s.contains("2 models ready"))));
    }

    #[test]
    fn model_picker_opens_from_cache_then_refreshes() {
        let mut a = app();
        a.models = vec!["cached-a".into(), "cached-b".into()];
        type_str(&mut a, "/model");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&actions[..], [Action::Send(Cmd::ListModels)]));
        match &a.modal {
            Some(Modal::Picker {
                kind: PickKind::Model,
                items,
                loading: false,
                ..
            }) => assert_eq!(items, &a.models),
            other => panic!("expected cached model picker, got {other:?}"),
        }
    }

    #[test]
    fn background_model_list_failure_is_info_not_hard_error() {
        let mut a = app();
        a.on_event(Ev::Models(Err("run auth login".into())));
        assert!(a.modal.is_none());
        assert!(a.models.is_empty());
        assert!(a
            .items
            .iter()
            .any(|i| matches!(i, Item::Info(s) if s.contains("model list unavailable"))));
        assert!(!a.items.iter().any(|i| matches!(i, Item::Error(_))));
    }

    #[test]
    fn provider_and_effort_commands() {
        let mut a = app();
        type_str(&mut a, "/provider gemini");
        assert!(
            matches!(&a.on_key(key(KeyCode::Enter))[..], [Action::Send(Cmd::SetProvider(p))] if p == "gemini")
        );
        type_str(&mut a, "/effort high");
        assert!(
            matches!(&a.on_key(key(KeyCode::Enter))[..], [Action::Send(Cmd::SetEffort(Some(l)))] if l == "high")
        );
        type_str(&mut a, "/effort default");
        assert!(matches!(
            &a.on_key(key(KeyCode::Enter))[..],
            [Action::Send(Cmd::SetEffort(None))]
        ));
        type_str(&mut a, "/provider");
        a.on_key(key(KeyCode::Enter));
        a.on_key(key(KeyCode::Down));
        assert!(
            matches!(&a.on_key(key(KeyCode::Enter))[..], [Action::Send(Cmd::SetProvider(p))] if p == "gemini")
        );
    }

    #[test]
    fn effort_picker_shows_only_levels_the_model_accepts() {
        let mut a = app();
        type_str(&mut a, "/effort");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&actions[..], [Action::Send(Cmd::ListEfforts)]));
        a.on_event(Ev::Efforts(vec!["low".into(), "high".into()]));
        assert_eq!(picker(&a).visible(), vec!["default", "low", "high"]);
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Down));
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&actions[..], [Action::Send(Cmd::SetEffort(Some(l)))] if l == "high"));
    }

    #[test]
    fn effort_picker_closes_when_model_has_no_levels() {
        let mut a = app();
        type_str(&mut a, "/effort");
        a.on_key(key(KeyCode::Enter));
        a.on_event(Ev::Efforts(Vec::new()));
        assert!(a.modal.is_none());
        assert!(a
            .items
            .iter()
            .any(|i| matches!(i, Item::Info(t) if t.contains("no selectable"))));
    }

    #[test]
    fn model_picker_needs_a_provider() {
        let mut a = App::new(Profile::Build, "no provider".into(), None);
        type_str(&mut a, "/model");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        assert!(a.modal.is_none());
    }

    #[test]
    fn assistant_progress_burst_appends_into_one_bubble() {
        use crate::agent::ProgressEvent;
        let mut a = app();
        for chunk in ["hel", "lo", ", ", "world"] {
            a.on_event(Ev::Progress(ProgressEvent::Assistant {
                text: chunk.into(),
            }));
        }
        assert_eq!(assistant_bubble_texts(&a.items), vec!["hello, world"]);
    }

    #[test]
    fn assistant_progress_full_text_growth_replaces_last() {
        use crate::agent::ProgressEvent;
        let mut a = app();
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "hel".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "hello".into(),
        }));
        assert_eq!(assistant_bubble_texts(&a.items), vec!["hello"]);
    }

    #[test]
    fn assistant_progress_after_tool_starts_new_bubble() {
        use crate::agent::ProgressEvent;
        let mut a = app();
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "first".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::ToolCall {
            name: "read_file".into(),
            summary: "x".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "second".into(),
        }));
        assert_eq!(assistant_bubble_texts(&a.items), vec!["first", "second"]);
    }

    #[test]
    fn assistant_progress_after_user_starts_new_bubble() {
        let mut a = app();
        a.items.push(Item::User("question".into()));
        a.on_event(Ev::Progress(crate::agent::ProgressEvent::Assistant {
            text: "answer".into(),
        }));
        assert_eq!(assistant_bubble_texts(&a.items), vec!["answer"]);
        a.items.push(Item::User("follow-up".into()));
        a.on_event(Ev::Progress(crate::agent::ProgressEvent::Assistant {
            text: "more".into(),
        }));
        assert_eq!(assistant_bubble_texts(&a.items), vec!["answer", "more"]);
    }

    #[test]
    fn assistant_progress_after_tool_result_starts_new_bubble() {
        use crate::agent::ProgressEvent;
        let mut a = app();
        a.on_event(Ev::Progress(ProgressEvent::Assistant { text: "a".into() }));
        a.on_event(Ev::Progress(ProgressEvent::ToolResult {
            name: "t".into(),
            ok: true,
            preview: "ok".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant { text: "b".into() }));
        assert_eq!(assistant_bubble_texts(&a.items), vec!["a", "b"]);
    }

    #[test]
    fn assistant_progress_duplicate_final_is_deduped() {
        use crate::agent::ProgressEvent;
        let mut a = app();
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "hello".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "hello".into(),
        }));
        assert_eq!(assistant_bubble_texts(&a.items), vec!["hello"]);
    }

    #[test]
    fn retry_after_partial_stream_starts_a_new_bubble() {
        use crate::agent::ProgressEvent;
        let mut a = app();
        a.on_event(Ev::Progress(ProgressEvent::Thinking { step: 1 }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "Checking the file.".into(),
        }));
        // Format error: the loop retries with a new model call.
        a.on_event(Ev::Progress(ProgressEvent::Thinking { step: 1 }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "Try".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "Try again".into(),
        }));
        assert_eq!(
            assistant_bubble_texts(&a.items),
            vec!["Checking the file.", "Try again"]
        );
    }

    #[test]
    fn done_after_streamed_assistant_keeps_single_bubble() {
        use crate::agent::{LoopResult, ProgressEvent};
        let mut a = app();
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "done text".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "done text".into(),
        }));
        a.on_event(Ev::Done(Box::new(LoopResult {
            exit_reason: ExitReason::Completed,
            steps: 1,
            total_input_tokens: 1,
            total_output_tokens: 1,
            messages: Vec::new(),
            summary: None,
            verification: None,
            no_progress: None,
        })));
        assert_eq!(assistant_bubble_texts(&a.items), vec!["done text"]);
    }

    #[test]
    fn worker_progress_keeps_scroll_at_bottom_when_following() {
        use crate::agent::ProgressEvent;
        let mut a = app();
        assert_eq!(a.scroll, 0);
        a.on_event(Ev::Progress(ProgressEvent::Thinking { step: 1 }));
        assert_eq!(a.scroll, 0);
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "hello".into(),
        }));
        assert_eq!(a.scroll, 0);
        a.on_event(Ev::Progress(ProgressEvent::ToolCall {
            name: "read_file".into(),
            summary: "x".into(),
        }));
        assert_eq!(a.scroll, 0);
    }

    #[test]
    fn worker_progress_does_not_reset_pinned_scroll() {
        use crate::agent::ProgressEvent;
        let mut a = app();
        a.scroll = 25;
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "more output".into(),
        }));
        assert_eq!(a.scroll, 25);
        a.on_event(Ev::Progress(ProgressEvent::ToolResult {
            name: "read_file".into(),
            ok: true,
            preview: "…".into(),
        }));
        assert_eq!(a.scroll, 25);
    }

    #[test]
    fn ctrl_end_jumps_to_latest_when_pinned() {
        let mut a = app();
        a.scroll = 30;
        a.on_key(ctrl_key(KeyCode::End));
        assert_eq!(a.scroll, 0);
    }

    #[test]
    fn ctrl_t_toggles_latest_tool_card() {
        let mut a = app();
        assert_eq!(a.expanded_tool, None);
        a.items.push(Item::Tool {
            name: "grep".into(),
            summary: "pattern".into(),
        });
        a.on_key(ctrl_key(KeyCode::Char('t')));
        assert_eq!(a.expanded_tool, Some(a.items.len() - 1));
        a.on_key(ctrl_key(KeyCode::Char('t')));
        assert_eq!(a.expanded_tool, None);
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

    fn command_picker(a: &App) -> &Modal {
        match a.modal.as_ref() {
            Some(Modal::Picker {
                kind: PickKind::Command,
                ..
            }) => a.modal.as_ref().unwrap(),
            _ => panic!("expected command palette"),
        }
    }

    #[test]
    fn command_palette_opens_on_ctrl_k() {
        let mut a = app();
        assert!(a.modal.is_none());
        assert!(a.on_key(ctrl('k')).is_empty());
        assert!(matches!(
            a.modal,
            Some(Modal::Picker {
                kind: PickKind::Command,
                loading: false,
                ..
            })
        ));
        assert_eq!(
            command_picker(&a).visible().len(),
            slash_command_items().len()
        );
        assert!(help_text().contains("Ctrl-K"));
    }

    #[test]
    fn command_palette_filter_narrows_list() {
        let mut a = app();
        a.on_key(ctrl('k'));
        type_str(&mut a, "roll");
        let shown = command_picker(&a).visible();
        assert_eq!(shown, vec!["/rollback"]);
        type_str(&mut a, "zzz");
        assert!(command_picker(&a).visible().is_empty());
    }

    #[test]
    fn command_palette_enter_invokes_slash_handler() {
        let mut a = app();
        a.on_key(ctrl('k'));
        type_str(&mut a, "plan");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(a.modal.is_none());
        assert_eq!(a.profile, Profile::Plan);
        assert!(matches!(
            &actions[..],
            [Action::Send(Cmd::SetProfile(Profile::Plan))]
        ));
    }

    #[test]
    fn command_palette_esc_closes_without_action() {
        let mut a = app();
        let before = a.items.len();
        a.on_key(ctrl('k'));
        assert!(a.on_key(key(KeyCode::Esc)).is_empty());
        assert!(a.modal.is_none());
        assert_eq!(a.items.len(), before);
        assert_eq!(a.profile, Profile::Build);
    }

    #[test]
    fn command_palette_down_moves_selection_before_enter() {
        let mut a = app();
        a.on_key(ctrl('k'));
        let all = command_picker(&a).visible();
        let clear_pos = all.iter().position(|c| *c == "/clear").expect("/clear");
        for _ in 0..clear_pos {
            a.on_key(key(KeyCode::Down));
        }
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        assert!(a.modal.is_none());
        assert!(a.items.is_empty());
    }

    #[test]
    fn paste_preserves_multiline_input() {
        let mut a = App::new(Profile::Ask, "p".into(), None);
        a.on_paste("one\r\ntwo\nthree");
        assert_eq!(a.input, "one\ntwo\nthree");
    }

    #[test]
    fn cursor_returns_to_empty_final_line() {
        let mut a = App::new(Profile::Ask, "p".into(), None);
        a.on_paste("one\n");
        a.input_up();
        a.input_down();
        assert_eq!(a.cursor, 4);
    }

    #[test]
    fn shift_enter_inserts_newline_without_submitting() {
        let mut a = app();
        type_str(&mut a, "hello");
        assert!(a.on_key(shift_key(KeyCode::Enter)).is_empty());
        assert!(!a.is_working());
        assert_eq!(a.input, "hello\n");
        assert_eq!(a.cursor, 6);
    }

    #[test]
    fn alt_enter_inserts_newline_without_submitting() {
        let mut a = app();
        type_str(&mut a, "x");
        let key = KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::ALT, KeyEventKind::Press);
        assert!(a.on_key(key).is_empty());
        assert_eq!(a.input, "x\n");
    }

    #[test]
    fn plain_enter_still_submits_after_multiline_typing() {
        let mut a = app();
        type_str(&mut a, "line1");
        a.on_key(shift_key(KeyCode::Enter));
        type_str(&mut a, "line2");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(
            &actions[..],
            [Action::Send(Cmd::Prompt(p))] if p == "line1\nline2"
        ));
        assert!(matches!(a.items.last(), Some(Item::User(u)) if u == "line1\nline2"));
    }

    #[test]
    fn help_documents_shift_enter_multiline() {
        assert!(help_text().contains("Shift+Enter"));
    }

    #[test]
    fn help_documents_ctrl_p_history() {
        assert!(help_text().contains("Ctrl-P"));
    }

    #[test]
    fn help_documents_shift_tab_profile_cycle() {
        assert!(help_text().contains("Shift-Tab"));
    }

    #[test]
    fn shift_tab_cycles_ask_plan_build() {
        let mut a = App::new(Profile::Build, "nim · m".into(), None);
        assert_eq!(a.profile, Profile::Build);
        let actions = a.on_key(shift_key(KeyCode::Tab));
        assert_eq!(a.profile, Profile::Ask);
        assert!(matches!(
            &actions[..],
            [Action::Send(Cmd::SetProfile(Profile::Ask))]
        ));
        a.on_key(shift_key(KeyCode::Tab));
        assert_eq!(a.profile, Profile::Plan);
        a.on_key(shift_key(KeyCode::Tab));
        assert_eq!(a.profile, Profile::Build);
    }

    #[test]
    fn back_tab_cycles_profile() {
        let mut a = App::new(Profile::Ask, "nim · m".into(), None);
        a.on_key(KeyEvent::new_with_kind(
            KeyCode::BackTab,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        ));
        assert_eq!(a.profile, Profile::Plan);
    }

    #[test]
    fn startup_note_shows_connect_not_init() {
        let a = App::new(
            Profile::Build,
            "nim · model".into(),
            Some("No API key for nim. Run /connect (or: devcode auth login nim).".into()),
        );
        assert!(a.items.iter().any(|i| matches!(
            i,
            Item::Info(t) if t.contains("/connect") && !t.contains("/init")
        )));
    }

    #[test]
    fn startup_without_nim_label_skips_no_provider_init_hint() {
        let a = App::new(Profile::Build, "nim · model".into(), None);
        assert!(!a.items.iter().any(|i| matches!(
            i,
            Item::Info(t) if t.contains("/init nim")
        )));
    }
}
