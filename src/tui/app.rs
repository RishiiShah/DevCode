//! TUI state and input handling. Pure: no terminal and no threads, so it is unit-tested.
use super::worker::{Cmd, Ev};
use crate::agent::{ExitReason, LoopResult, ProgressEvent};
use crate::Profile;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// Startup card: name, version, model, folder (as Claude Code and Codex open).
    Welcome,
    /// A slash command the user ran; its output (Info lines) nests under it.
    Command(String),
    User(String),
    Reasoning(String),
    Assistant(String),
    Tool {
        name: String,
        summary: String,
    },
    ToolResult {
        ok: bool,
        preview: String,
        diff: Option<String>,
    },
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
    /// Saved conversations (`/resume`).
    Conversation,
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
    /// Hand a message typed during a run to the loop's next step.
    Steer(String),
    /// Take back a steering message that the loop has not read yet.
    Unsteer(String),
    /// List managed background processes (works during a run).
    Processes,
    /// Stop a managed background process by id (works during a run).
    StopProcess(String),
    /// Cancel an in-flight model stream / agent run.
    Cancel,
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
    /// Items holding the current model call's answer and reasoning. Streamed text replaces
    /// these in place; tracking them by index keeps a late reasoning tail (held back for key
    /// redaction) from starting a second copy after the answer has begun.
    step_text: Option<usize>,
    step_reasoning: Option<usize>,
    pub tick: usize,
    /// Index in `items` of an `Item::Tool` row shown expanded; `None` = all tool cards collapsed.
    pub expanded_tool: Option<usize>,
    /// Last model catalog from the provider (`/models` prefetch after connect/switch).
    pub models: Vec<String>,
    /// Show model reasoning in the transcript (`/thinking`). Hidden by default, as in Codex.
    pub show_thinking: bool,
    /// When the current call's reasoning began; drives the footer's "thinking" label.
    pub thinking_since: Option<Instant>,
    /// Transcript area from the last draw, for mapping mouse positions to cells.
    pub transcript_area: Rect,
    /// Mouse text selection as (anchor, head), each (screen column, transcript row). Rows count
    /// wrapped transcript lines from the top, so the selection survives scrolling.
    pub selection: Option<((u16, usize), (u16, usize))>,
    /// A left-button drag is in progress; at the top or bottom edge it scrolls (-1 / 1).
    pub dragging: bool,
    /// Configured context window and the size of the latest request, for the status line.
    pub context_window: Option<u64>,
    pub context_source: Option<&'static str>,
    pub context_used: u64,
    /// Git branch of the working folder, read at start and after each run.
    pub branch: Option<String>,
    /// Items shown on the right of the status line, in order (`/statusline`).
    pub statusline: Vec<String>,
    /// Saved conversations as (picker label, id) while the `/resume` picker is open.
    pub conversations: Vec<(String, String)>,
    pub drag_edge: i8,
    /// First transcript row on screen, set by each draw.
    pub transcript_top: usize,
    /// Set on mouse release; the next draw reads the selected cells into `copied`.
    pub copy_requested: bool,
    /// Selected text for the event loop to put on the clipboard.
    pub copied: Option<String>,
    /// Short footer notice and when it was set.
    pub flash: Option<(String, Instant)>,
    /// Messages typed while a run was working; sent together when it ends.
    pub queued: Vec<String>,
    /// Tool call in progress and when it started; drives the footer timer.
    pub tool_since: Option<(String, Instant)>,
    /// Managed background processes still running.
    pub bg_processes: usize,
    /// Highlighted option in the approval panel.
    pub approval_choice: usize,
    /// Highlighted slash command suggestion while typing `/…`.
    pub suggest: usize,
}

/// One option in the approval panel: label shown, line sent to the approval code, transcript note.
pub struct ApprovalChoice {
    pub label: &'static str,
    pub key: &'static str,
    pub answer: &'static str,
    pub note: &'static str,
}

pub const EDIT_CHOICES: &[ApprovalChoice] = &[
    ApprovalChoice {
        label: "Yes, apply this edit",
        key: "y",
        answer: "y",
        note: "edit approved",
    },
    ApprovalChoice {
        label: "Yes, and approve every edit this session",
        key: "a",
        answer: "a",
        note: "edit approved; later edits this session are approved too",
    },
    ApprovalChoice {
        label: "No, and tell DevCode what to do instead",
        key: "esc",
        answer: "n",
        note: "edit rejected; type what to do instead",
    },
];

pub const CONFIRM_CHOICES: &[ApprovalChoice] = &[
    ApprovalChoice {
        label: "Yes, run it",
        key: "y",
        answer: "y",
        note: "approved",
    },
    ApprovalChoice {
        label: "No, and tell DevCode what to do instead",
        key: "esc",
        answer: "n",
        note: "rejected; type what to do instead",
    },
];

pub const SCOPE_CHOICES: &[ApprovalChoice] = &[
    ApprovalChoice {
        label: "Yes, apply this edit",
        key: "y",
        answer: "y",
        note: "approved",
    },
    ApprovalChoice {
        label: "No, and tell DevCode what to do instead",
        key: "esc",
        answer: "n",
        note: "rejected; type what to do instead",
    },
];

const HELP: &str = "Enter       send message or command (queued while a run works)
/ask /plan /build   switch profile (ask and plan are read-only)
/model [ID]         pick a model (catalog loads on connect/switch) or set one
/provider [NAME]    switch provider (saved as your default; refreshes models)
/effort [LEVEL]     reasoning effort; lists only what the current model accepts
/connect [NAME]     store an API key (masked input; saves as default)
/connections        list accounts (stored|env|missing; * = active)
/init [NAME]        optional project scaffold (.devcode.json)
/rollback [N]       undo the latest run (or step N and later)
/thinking           show or hide model thinking (hidden by default)
/ps                 list background processes (dev servers)
/stop ID            stop a background process
/status             model, context use, tokens, branch, folder
/statusline [ITEM]  show or hide a status line item
/clear              clear the screen and start a new conversation
/compact            compact older tool outputs to free context
/export PATH        save the conversation as markdown
/resume [ID]        continue a saved conversation (devcode -c: latest)
/exit               quit
PgUp/PgDn           scroll transcript
Ctrl-End            jump to latest (when scrolled up)
Ctrl-T              expand/collapse latest tool card
Ctrl-K              command palette (search slash commands)
Up/Down             input history when empty; otherwise move line (edges scroll)
Mouse drag          select transcript text; release copies it
Ctrl-P / Ctrl-N     input history (works even when typing)
Shift+Enter         new line in message (Alt+Enter also)
Shift-Tab           cycle ask → plan → build
Ctrl-C              cancel prompt, or quit (twice while working)";

impl App {
    pub fn new(profile: Profile, provider: String, startup_note: Option<String>) -> Self {
        let mut app = Self {
            items: Vec::new(),
            conversations: Vec::new(),
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
            step_text: None,
            step_reasoning: None,
            tick: 0,
            expanded_tool: None,
            models: Vec::new(),
            show_thinking: false,
            thinking_since: None,
            transcript_area: Rect::default(),
            selection: None,
            dragging: false,
            context_window: None,
            context_source: None,
            context_used: 0,
            branch: git_branch(),
            statusline: STATUSLINE_ITEMS
                .iter()
                .map(|(n, _)| n.to_string())
                .collect(),
            drag_edge: 0,
            transcript_top: 0,
            copy_requested: false,
            copied: None,
            flash: None,
            queued: Vec::new(),
            tool_since: None,
            bg_processes: 0,
            approval_choice: 0,
            suggest: 0,
        };
        app.items.push(Item::Welcome);
        if let Some(note) = startup_note {
            app.items.push(Item::Info(note));
        } else if app.provider == "no provider" {
            app.items.push(Item::Info(
                "Run /provider to choose a provider, then /connect to add an API key.".into(),
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
                let modal = classify_prompt(text);
                // Enter must not run a risky shell command by default; edits, dev servers,
                // and out-of-scope file edits start on Yes.
                self.approval_choice = match &modal {
                    Modal::Confirm { text, .. }
                        if text.contains("Destructive command detected:")
                            && !text.contains("long-running") =>
                    {
                        CONFIRM_CHOICES.len() - 1
                    }
                    _ => 0,
                };
                self.modal = Some(modal);
            }
            Ev::Done(result) => self.on_done(*result),
            Ev::Processes(count) => self.bg_processes = count,
            Ev::Failed(message) => {
                self.status = Status::Idle;
                self.tool_since = None;
                self.thinking_since = None;
                self.pending_prompt.clear();
                self.items.push(Item::Error(hint_for(&message)));
            }
            Ev::Info(lines) => {
                for line in lines {
                    self.items.push(Item::Info(line));
                }
            }
            Ev::Provider(label, window) => {
                self.context_window = window;
                self.context_source = None;
                self.context_used = 0;
                // Drop the previous provider's catalog; prefetch fills models next.
                self.models.clear();
                self.items.push(Item::Info(format!("now using {label}")));
                self.provider = label;
            }
            Ev::ContextSource(source) => self.context_source = source,
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
                    self.focus_current();
                }
            }
            Ev::Models(result) => self.on_models(result),
            Ev::Conversations(mut list) => {
                let mut counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for (label, _) in &list {
                    *counts.entry(label.clone()).or_default() += 1;
                }
                for (label, id) in &mut list {
                    if counts.get(label).copied().unwrap_or(0) > 1 {
                        label.push_str(&format!(" · #{id}"));
                    }
                }
                let open = matches!(
                    self.modal,
                    Some(Modal::Picker {
                        kind: PickKind::Conversation,
                        ..
                    })
                );
                if list.is_empty() {
                    if open {
                        self.modal = None;
                    }
                    self.items
                        .push(Item::Info("no saved conversations in this folder".into()));
                } else if let Some(Modal::Picker { items, loading, .. }) =
                    self.modal.as_mut().filter(|_| open)
                {
                    *items = list.iter().map(|(label, _)| label.clone()).collect();
                    *loading = false;
                }
                self.conversations = list;
            }
            Ev::Resumed(loaded) => self.on_resumed(&loaded),
            Ev::Compacted(msg) => {
                self.context_used = 0;
                self.items.push(Item::Note(msg));
            }
        }
    }

    /// Opens the `/resume` picker; the worker fills it with saved conversations.
    pub fn open_resume(&mut self) -> Vec<Action> {
        self.modal = Some(Modal::Picker {
            kind: PickKind::Conversation,
            items: Vec::new(),
            filter: String::new(),
            selected: 0,
            loading: true,
        });
        vec![Action::Send(Cmd::ListConversations)]
    }

    /// Rebuilds the transcript from a saved conversation, as Codex and Claude Code replay it.
    fn on_resumed(&mut self, loaded: &crate::conversation::LoadedConversation) {
        use crate::agent::Role;
        use std::collections::{HashMap, HashSet};
        self.items.clear();
        self.items.push(Item::Welcome);
        self.expanded_tool = None;
        self.scroll = 0;
        self.total_in = loaded.total_in;
        self.total_out = loaded.total_out;
        self.context_used = loaded.context_used;

        let tool_results: HashMap<&str, &crate::agent::Message> = loaded
            .messages
            .iter()
            .filter_map(|m| m.tool_call_id.as_deref().map(|id| (id, m)))
            .collect();
        let mut consumed_results: HashSet<&str> = HashSet::new();

        let mut prompts = 0;
        for message in &loaded.messages {
            match message.role {
                Role::User if message.tool_call_id.is_none() => {
                    prompts += 1;
                    self.items.push(Item::User(message.content.clone()));
                }
                Role::Assistant => {
                    if !message.content.trim().is_empty() {
                        self.items.push(Item::Assistant(message.content.clone()));
                    }
                    for call in &message.tool_calls {
                        self.items.push(Item::Tool {
                            name: call.name.clone(),
                            summary: crate::agent::loop_runner::call_summary(&call.arguments),
                        });
                        if let Some(res) = tool_results.get(call.id.as_str()) {
                            consumed_results.insert(call.id.as_str());
                            let outcome = loaded.tool_outcomes.get(&call.id);
                            let preview = crate::agent::preview_lines(&res.content);
                            let ok = outcome.map(|o| o.ok).unwrap_or_else(|| {
                                !res.content
                                    .trim_start()
                                    .to_ascii_lowercase()
                                    .starts_with("error")
                            });
                            let diff = outcome.and_then(|o| o.diff.clone());
                            self.items.push(Item::ToolResult { ok, preview, diff });
                        }
                    }
                }
                Role::Tool | Role::User => {
                    if let Some(id) = message.tool_call_id.as_deref() {
                        if consumed_results.contains(id) {
                            continue;
                        }
                    }
                    let preview = crate::agent::preview_lines(&message.content);
                    let ok = !message
                        .content
                        .trim_start()
                        .to_ascii_lowercase()
                        .starts_with("error");
                    let diff = message
                        .tool_call_id
                        .as_deref()
                        .and_then(|id| loaded.tool_outcomes.get(id))
                        .and_then(|o| o.diff.clone());
                    self.items.push(Item::ToolResult { ok, preview, diff });
                }
                _ => {}
            }
        }
        self.items.push(Item::Note(format!(
            "Resumed conversation · {}. Continue where you left off.",
            plural(prompts, "prompt", "prompts")
        )));
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
            Ok(mut list) => {
                list.retain(|id| crate::config::is_chat_model(id));
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
                }
                self.focus_current();
            }
            Err(message) => {
                self.models.clear();
                if picker_open {
                    self.modal = None;
                    self.items.push(Item::Error(format!(
                        "cannot list models: {message}. Use /model ID to set one directly."
                    )));
                } else {
                    self.items.push(Item::Info(format!(
                        "model list unavailable ({message}). Run /model to retry."
                    )));
                }
            }
        }
    }

    /// Merges streamed text into the current call's answer (`reasoning == false`) or reasoning
    /// item: cumulative text replaces it, a delta appends. A user message or tool card after that
    /// item starts a new one, so text never jumps above later activity.
    fn merge_step_text(&mut self, text: String, reasoning: bool) {
        if text.is_empty() {
            return;
        }
        let slot = if reasoning {
            self.step_reasoning
        } else {
            self.step_text
        };
        let open = slot.filter(|&i| {
            self.items.get(i + 1..).is_some_and(|after| {
                !after.iter().any(|item| {
                    matches!(
                        item,
                        Item::User(_) | Item::Tool { .. } | Item::ToolResult { .. }
                    )
                })
            })
        });
        if let Some(item) = open.and_then(|i| self.items.get_mut(i)) {
            match (item, reasoning) {
                (Item::Reasoning(last), true) | (Item::Assistant(last), false) => {
                    if text.starts_with(last.as_str()) {
                        *last = text;
                    } else if !last.ends_with(&text) {
                        last.push_str(&text);
                    }
                    return;
                }
                _ => {}
            }
        }
        let index = Some(self.items.len());
        if reasoning {
            self.items.push(Item::Reasoning(text));
            self.step_reasoning = index;
        } else {
            self.items.push(Item::Assistant(text));
            self.step_text = index;
        }
    }

    /// Mouse input: the wheel scrolls; a left-button drag inside the transcript selects text,
    /// and releasing the button asks the next draw to copy it.
    pub fn on_mouse(&mut self, event: MouseEvent) {
        if self.modal.is_some() {
            return;
        }
        let area = self.transcript_area;
        if area.is_empty() {
            return;
        }
        let (col, row) = (event.column, event.row);
        let inside = area.contains(Position::new(col, row));
        let x = col.clamp(area.x, area.right() - 1);
        let y = row.clamp(area.y, area.bottom() - 1);
        let at = (x, self.transcript_top + (y - area.y) as usize);
        match event.kind {
            MouseEventKind::ScrollUp => self.on_mouse_scroll(true),
            MouseEventKind::ScrollDown => self.on_mouse_scroll(false),
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection = inside.then_some((at, at));
                self.dragging = inside;
                self.drag_edge = 0;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                // Past the top or bottom edge the transcript scrolls under the pointer.
                self.drag_edge = if row <= area.y {
                    -1
                } else if row >= area.bottom() - 1 {
                    1
                } else {
                    0
                };
                if let Some((_, head)) = &mut self.selection {
                    *head = at;
                }
                self.drag_scroll();
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.dragging = false;
                self.drag_edge = 0;
                match self.selection {
                    Some((anchor, head)) if anchor != head => self.copy_requested = true,
                    _ => self.selection = None,
                }
            }
            _ => {}
        }
    }

    /// Share of the context window still free after the latest request, as Gemini CLI shows it.
    pub fn context_left(&self) -> Option<u64> {
        let window = self.context_window.filter(|w| *w > 0)?;
        (self.context_used > 0).then(|| 100 - (self.context_used.min(window) * 100 / window))
    }

    fn status_lines(&self) -> Vec<String> {
        let cwd = std::env::current_dir().unwrap_or_default();
        let source = self
            .context_source
            .map(|s| format!(" ({s})"))
            .unwrap_or_default();
        let context = match (self.context_window, self.context_used) {
            (Some(w), 0) => format!("{} window{source}, nothing sent yet", tokens(w)),
            (Some(w), used) => format!(
                "{} of {} used ({}% left{source})",
                tokens(used),
                tokens(w),
                self.context_left().unwrap_or(0)
            ),
            (None, _) => "unknown window".into(),
        };
        vec![
            format!(
                "DevCode v{} · profile {}",
                env!("CARGO_PKG_VERSION"),
                self.profile.name()
            ),
            format!("model       {}", self.provider),
            format!("context     {context}"),
            format!(
                "tokens      {} in / {} out this session",
                tokens(self.total_in),
                tokens(self.total_out)
            ),
            format!(
                "branch      {}",
                self.branch.as_deref().unwrap_or("(not a git repo)")
            ),
            format!("folder      {}", cwd.display()),
            format!("background  {} running (/ps)", self.bg_processes),
        ]
    }

    /// `/statusline` lists the items; `/statusline ITEM` shows or hides one and saves the set.
    fn statusline_command(&mut self, arg: Option<&str>) -> Vec<Action> {
        let Some(name) = arg else {
            for (name, about) in STATUSLINE_ITEMS {
                let on = if self.statusline.iter().any(|s| s == name) {
                    "on "
                } else {
                    "off"
                };
                self.items
                    .push(Item::Info(format!("{on}  {name:<10} {about}")));
            }
            self.items
                .push(Item::Info("/statusline ITEM shows or hides one".into()));
            return Vec::new();
        };
        if !STATUSLINE_ITEMS.iter().any(|(n, _)| *n == name) {
            let names: Vec<&str> = STATUSLINE_ITEMS.iter().map(|(n, _)| *n).collect();
            self.items.push(Item::Error(format!(
                "unknown item '{name}'; one of: {}",
                names.join(", ")
            )));
            return Vec::new();
        }
        if let Some(at) = self.statusline.iter().position(|s| s == name) {
            self.statusline.remove(at);
            self.flash = Some((format!("status line: {name} hidden"), Instant::now()));
        } else {
            // Keep the default order so the drop order on narrow rows stays predictable.
            self.statusline.push(name.to_string());
            self.statusline
                .sort_by_key(|s| STATUSLINE_ITEMS.iter().position(|(n, _)| n == s));
            self.flash = Some((format!("status line: {name} shown"), Instant::now()));
        }
        vec![Action::Send(Cmd::SaveStatusline(self.statusline.clone()))]
    }

    /// Called every frame: keeps scrolling while a drag rests on the top or bottom edge.
    pub fn on_tick(&mut self) {
        if self.dragging && self.drag_edge != 0 {
            self.drag_scroll();
        }
    }

    fn drag_scroll(&mut self) {
        let Some((_, head)) = &mut self.selection else {
            return;
        };
        match self.drag_edge {
            -1 if self.transcript_top > 0 => {
                head.1 = head.1.saturating_sub(1);
                self.transcript_top -= 1;
                self.scroll_transcript(1);
            }
            1 if self.scroll > 0 => {
                head.1 += 1;
                self.transcript_top += 1;
                self.scroll_transcript(-1);
            }
            _ => {}
        }
    }

    fn on_progress(&mut self, p: ProgressEvent) {
        match p {
            ProgressEvent::Thinking { step } => {
                self.status = Status::Working { step };
                self.tool_since = None;
                self.step_text = None;
                self.step_reasoning = None;
                self.thinking_since = None;
            }
            ProgressEvent::Reasoning { text } => {
                self.thinking_since.get_or_insert_with(Instant::now);
                self.merge_step_text(text, true);
            }
            ProgressEvent::Assistant { text } => {
                self.thinking_since = None;
                self.merge_step_text(text, false);
            }
            ProgressEvent::ToolCall { name, summary } => {
                self.thinking_since = None;
                self.tool_since = Some((name.clone(), Instant::now()));
                self.items.push(Item::Tool { name, summary })
            }
            ProgressEvent::ToolResult {
                ok, preview, diff, ..
            } => {
                self.tool_since = None;
                // Approval notes land between a call and its result; keep the result on its card.
                let card = self
                    .items
                    .iter()
                    .rposition(|item| matches!(item, Item::Tool { .. }))
                    .filter(|&t| {
                        self.items[t + 1..]
                            .iter()
                            .all(|item| matches!(item, Item::Info(_) | Item::Note(_)))
                    });
                let result = Item::ToolResult { ok, preview, diff };
                match card {
                    Some(t) => self.items.insert(t + 1, result),
                    None => self.items.push(result),
                }
            }
            ProgressEvent::Steered { text } => {
                if let Some(at) = self.queued.iter().position(|q| *q == text) {
                    self.queued.remove(at);
                }
                self.items.push(Item::User(text));
            }
            ProgressEvent::Retry { attempt, max, .. } => {
                // The failed attempt's streamed text is void; the retry streams the step again.
                if let Some(start) = [self.step_reasoning, self.step_text]
                    .into_iter()
                    .flatten()
                    .min()
                {
                    self.items.truncate(start.min(self.items.len()));
                }
                self.step_text = None;
                self.step_reasoning = None;
                self.thinking_since = None;
                self.items.push(Item::Info(format!(
                    "connection lost; retrying ({attempt}/{max})"
                )));
            }
            ProgressEvent::Usage { input, output } => {
                // Usage is cumulative per run; the step's own input is the context it sent.
                self.context_used = input.saturating_sub(self.run_in);
                self.run_in = input;
                self.run_out = output;
            }
        }
    }

    fn on_done(&mut self, result: LoopResult) {
        self.status = Status::Idle;
        self.branch = git_branch();
        self.tool_since = None;
        self.thinking_since = None;
        self.quit_armed = false;
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
            let v = v.trim_start_matches("Verification Summary:").trim();
            if v.starts_with("UNVERIFIED. No test command") {
                self.items.push(Item::Note(
                    "untested: no test command (set verification.test_command in .devcode.json)"
                        .into(),
                ));
            } else {
                for line in v.lines().filter(|l| !l.trim().is_empty()) {
                    self.items.push(Item::Note(line.trim().to_string()));
                }
            }
        }
        if let Some(s) = &result.summary {
            let mut parts = vec![
                plural(s.steps, "step", "steps"),
                format!(
                    "{} in / {} out tokens",
                    tokens(s.tokens_in),
                    tokens(s.tokens_out)
                ),
            ];
            let counts = [
                (s.files_edited, "file edited", "files edited"),
                (s.tests_changed, "test file changed", "test files changed"),
                (
                    s.files_changed_by_commands,
                    "file changed by commands",
                    "files changed by commands",
                ),
                (s.files_outside_scope, "outside scope", "outside scope"),
                (
                    s.dependencies_added,
                    "dependency added",
                    "dependencies added",
                ),
            ];
            parts.extend(
                counts
                    .iter()
                    .filter(|(n, ..)| *n > 0)
                    .map(|&(n, one, many)| plural(n, one, many)),
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
            ExitReason::Cancelled => self.items.push(Item::Info(
                "cancelled (completed edits stay; /rollback undoes them)".into(),
            )),
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
                // One insertion keeps large pastes linear. Tabs stay tabs (Makefiles, Go); the
                // input box draws them as four columns.
                let text: String = clean
                    .chars()
                    .filter(|c| matches!(c, '\n' | '\t') || !c.is_control())
                    .collect();
                let at = self.byte_at(self.cursor);
                self.input.insert_str(at, &text);
                self.cursor += text.chars().count();
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Action> {
        self.selection = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return self.on_ctrl_c();
        }
        self.quit_armed = false;
        if self.modal.is_some() {
            return self.on_modal_key(key);
        }
        if let Some(actions) = self.suggestion_key(key) {
            return actions;
        }
        let before = self.input.len();
        let actions = self.input_key(key, ctrl);
        if self.input.len() != before {
            self.suggest = 0;
        }
        actions
    }

    /// Slash commands matching what is typed, as Claude Code and Codex list them under the
    /// input: prefix matches first, then names that contain the text.
    pub fn suggestions(&self) -> Vec<(&'static str, &'static str)> {
        let typed = self.input.as_str();
        if self.modal.is_some() || !typed.starts_with('/') || typed.contains(char::is_whitespace) {
            return Vec::new();
        }
        let names = slash_command_items();
        let prefix = names.iter().filter(|n| n.starts_with(typed));
        let inner = names
            .iter()
            .filter(|n| !n.starts_with(typed) && n.contains(&typed[1..]));
        prefix
            .chain(inner)
            .map(|n| (*n, command_about(n)))
            .collect()
    }

    /// Up/Down pick a suggestion, Tab completes it, Enter runs it. None: not a suggestion key.
    fn suggestion_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let list = self.suggestions();
        if list.is_empty() || !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
            return None;
        }
        let pick = self.suggest.min(list.len() - 1);
        match key.code {
            KeyCode::Up => self.suggest = pick.checked_sub(1).unwrap_or(list.len() - 1),
            KeyCode::Down => self.suggest = (pick + 1) % list.len(),
            KeyCode::Tab => {
                let name = list[pick].0;
                self.input = if takes_argument(name) {
                    format!("{name} ")
                } else {
                    name.to_string()
                };
                self.cursor = self.input.chars().count();
                self.suggest = 0;
            }
            KeyCode::Enter => {
                let name = list[pick].0;
                if self.input != name && takes_argument(name) {
                    // Commands with arguments complete first so the argument can be typed.
                    self.input = format!("{name} ");
                    self.cursor = self.input.chars().count();
                    self.suggest = 0;
                    return Some(Vec::new());
                }
                self.input = name.to_string();
                self.suggest = 0;
                return Some(self.submit());
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    fn input_key(&mut self, key: KeyEvent, ctrl: bool) -> Vec<Action> {
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
            KeyCode::Esc if self.input.is_empty() && !self.queued.is_empty() => {
                // Pull the last queued message back for editing.
                if let Some(last) = self.queued.pop() {
                    self.cursor = last.chars().count();
                    self.input = last.clone();
                    return vec![Action::Unsteer(last)];
                }
            }
            KeyCode::Esc => {
                // Recoverable with Up: the cleared text goes into history.
                let cleared = self.input.trim().to_string();
                if !cleared.is_empty() && self.history.last() != Some(&cleared) {
                    self.history.push(cleared);
                }
                self.history_pos = None;
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
        self.flash_profile();
        vec![Action::Send(Cmd::SetProfile(next))]
    }

    /// The info row badge shows the profile; a short notice says what it allows.
    fn flash_profile(&mut self) {
        let note = match self.profile {
            Profile::Ask => "ask: read-only answers",
            Profile::Plan => "plan: read-only planning",
            Profile::Build => "build: edits and commands, with approval",
        };
        self.flash = Some((note.into(), Instant::now()));
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
        if self.quit_armed {
            return vec![Action::Quit];
        }
        if !self.is_working() {
            // Like Claude Code and Codex: Ctrl-C clears a draft, and quitting takes a second press.
            let draft = self.input.trim().to_string();
            if !draft.is_empty() {
                if self.history.last() != Some(&draft) {
                    self.history.push(draft);
                }
                self.input.clear();
                self.cursor = 0;
                return Vec::new();
            }
            self.quit_armed = true;
            self.flash = Some(("press Ctrl-C again to quit".into(), Instant::now()));
            return Vec::new();
        }
        self.quit_armed = true;
        self.items.push(Item::Info(
            "cancelling run… (press Ctrl-C again to quit)".into(),
        ));
        vec![Action::Cancel]
    }

    fn on_modal_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Some(modal) = self.modal.take() else {
            return Vec::new();
        };
        match modal {
            Modal::Approve { text, scroll } => {
                self.approval_key(Modal::Approve { text, scroll }, key.code, EDIT_CHOICES)
            }
            Modal::Confirm { text, scroll } => {
                let choices = if text.contains("outside declared scope") {
                    SCOPE_CHOICES
                } else {
                    CONFIRM_CHOICES
                };
                self.approval_key(Modal::Confirm { text, scroll }, key.code, choices)
            }
            Modal::Text { text, mut input } => match key.code {
                KeyCode::Enter => {
                    let line = input.trim().to_string();
                    if line.is_empty() {
                        // Do not silently reject on accidental empty Enter; keep the prompt open
                        // so only Esc/Ctrl-C cancels or a non-empty justification submits.
                        self.modal = Some(Modal::Text { text, input });
                        Vec::new()
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
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.clear();
                    self.modal = Some(Modal::Text { text, input });
                    Vec::new()
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
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
                            PickKind::Conversation => {
                                let id = self
                                    .conversations
                                    .iter()
                                    .find(|(label, _)| *label == value)
                                    .map(|(_, id)| id.clone())
                                    .unwrap_or_default();
                                vec![Action::Send(Cmd::Resume(id))]
                            }
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

    /// Keys in the approval panel: ↑↓ pick an option, Enter or a number confirms it, y/a/n
    /// answer directly, Esc rejects, PgUp/PgDn scroll the diff.
    fn approval_key(
        &mut self,
        modal: Modal,
        code: KeyCode,
        choices: &[ApprovalChoice],
    ) -> Vec<Action> {
        let by_answer = |answer: &str| choices.iter().position(|c| c.answer == answer);
        let pick = match code {
            KeyCode::Enter => Some(self.approval_choice.min(choices.len() - 1)),
            KeyCode::Char(c @ '1'..='9') => {
                Some(c as usize - '1' as usize).filter(|&i| i < choices.len())
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => by_answer("y"),
            KeyCode::Char('a') | KeyCode::Char('A') => by_answer("a"),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => by_answer("n"),
            _ => None,
        };
        if let Some(i) = pick {
            let choice = &choices[i];
            return self.answer(choice.answer, choice.note);
        }
        match code {
            KeyCode::Up => self.approval_choice = self.approval_choice.saturating_sub(1),
            KeyCode::Down => {
                self.approval_choice = (self.approval_choice + 1).min(choices.len() - 1)
            }
            _ => {}
        }
        self.modal = Some(match modal {
            Modal::Approve { text, scroll }
                if matches!(code, KeyCode::PageUp | KeyCode::PageDown) =>
            {
                Modal::Approve {
                    text,
                    scroll: scrolled(scroll, code),
                }
            }
            Modal::Confirm { text, scroll }
                if matches!(code, KeyCode::PageUp | KeyCode::PageDown) =>
            {
                Modal::Confirm {
                    text,
                    scroll: scrolled(scroll, code),
                }
            }
            other => other,
        });
        Vec::new()
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

    /// What the open picker's kind is set to now, read from the provider label
    /// (`connector · model · effort LEVEL`).
    pub fn picker_current(&self) -> Option<String> {
        let Some(Modal::Picker { kind, .. }) = &self.modal else {
            return None;
        };
        let mut parts = self.provider.split(" · ");
        let provider = parts.next()?;
        let model = parts.next();
        let effort = parts.find_map(|p| p.strip_prefix("effort "));
        match kind {
            PickKind::Provider => Some(provider.to_string()),
            PickKind::Model => model.map(str::to_string),
            PickKind::Effort => Some(effort.unwrap_or("default").to_string()),
            PickKind::Command | PickKind::Conversation => None,
        }
    }

    /// Opens a picker on the current choice, as Codex and Claude Code do.
    fn focus_current(&mut self) {
        let Some(current) = self.picker_current() else {
            return;
        };
        let at = self
            .modal
            .as_ref()
            .and_then(|m| m.visible().iter().position(|v| *v == current));
        if let (
            Some(at),
            Some(Modal::Picker {
                selected, filter, ..
            }),
        ) = (at, &mut self.modal)
        {
            if filter.is_empty() {
                *selected = at;
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
            self.items.push(Item::Command(line.clone()));
            return self.slash(rest.trim());
        }
        // A message typed during a run reaches the model at its next step (as Codex steers).
        if self.is_working() {
            self.queued.push(line.clone());
            return vec![Action::Steer(line)];
        }
        vec![Action::Send(self.start_run(line))]
    }

    fn start_run(&mut self, line: String) -> Cmd {
        self.items.push(Item::User(line.clone()));
        self.status = Status::Working { step: 0 };
        self.started = Some(Instant::now());
        self.run_in = 0;
        self.run_out = 0;
        Cmd::Prompt(line)
    }

    /// Queued messages, joined into one prompt, once the run and any dialog have finished.
    pub fn next_queued(&mut self) -> Option<Cmd> {
        if self.is_working() || self.modal.is_some() || self.queued.is_empty() {
            return None;
        }
        let line = std::mem::take(&mut self.queued).join("\n\n");
        Some(self.start_run(line))
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
                self.flash_profile();
                vec![Action::Send(Cmd::SetProfile(profile))]
            }
            "connect" => {
                if busy {
                    return busy_block(&mut self.items);
                }
                if self.provider == "no provider" {
                    self.items.push(Item::Error(
                        "Run /provider to choose a provider, then /connect.".into(),
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
                let actions = self.pick(name, arg);
                self.focus_current();
                actions
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
                if busy {
                    return busy_block(&mut self.items);
                }
                self.items.clear();
                self.expanded_tool = None;
                self.total_in = 0;
                self.total_out = 0;
                self.context_used = 0;
                self.items.push(Item::Note(
                    "New conversation. Earlier messages are forgotten.".into(),
                ));
                vec![Action::Send(Cmd::ClearHistory)]
            }
            "compact" => {
                if busy {
                    return busy_block(&mut self.items);
                }
                vec![Action::Send(Cmd::Compact)]
            }
            "export" => {
                if busy {
                    return busy_block(&mut self.items);
                }
                let path = rest
                    .strip_prefix("export")
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                match path {
                    Some(p) => vec![Action::Send(Cmd::Export(p.to_string()))],
                    None => {
                        self.items.push(Item::Info("usage: /export PATH".into()));
                        Vec::new()
                    }
                }
            }
            "resume" | "continue" => {
                if busy {
                    return busy_block(&mut self.items);
                }
                match arg {
                    Some(id) => vec![Action::Send(Cmd::Resume(id.to_string()))],
                    None => self.open_resume(),
                }
            }
            "help" | "?" => {
                self.modal = Some(Modal::Help);
                Vec::new()
            }
            "ps" | "processes" => vec![Action::Processes],
            "status" => {
                for line in self.status_lines() {
                    self.items.push(Item::Info(line));
                }
                Vec::new()
            }
            "statusline" => self.statusline_command(arg),
            "stop" => match arg {
                Some(id) => vec![Action::StopProcess(id.to_string())],
                None => {
                    self.items
                        .push(Item::Info("usage: /stop ID (see /ps for ids)".into()));
                    Vec::new()
                }
            },
            "thinking" => {
                self.show_thinking = !self.show_thinking;
                self.items.push(Item::Info(if self.show_thinking {
                    "model thinking is shown; /thinking hides it".into()
                } else {
                    "model thinking is hidden; /thinking shows it".into()
                }));
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
                        "No provider yet. Run /provider to choose one.".into(),
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
                        "No provider yet. Run /provider to choose one.".into(),
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
/// One-line description shown next to a slash command suggestion.
pub fn command_about(name: &str) -> &'static str {
    match name {
        "/ask" => "read-only answers",
        "/plan" => "read-only planning",
        "/build" => "edit files and run commands, with approval",
        "/model" => "pick or set the model",
        "/provider" => "switch provider",
        "/effort" => "reasoning effort for this model",
        "/connect" => "store an API key",
        "/connections" => "list accounts and keys",
        "/init" => "create .devcode.json for this project",
        "/rollback" => "undo the latest run (or step N and later)",
        "/thinking" => "show or hide model thinking",
        "/ps" => "list background processes",
        "/stop" => "stop a background process",
        "/status" => "model, context, tokens, branch and folder",
        "/statusline" => "choose what the status line shows",
        "/clear" => "start a new conversation",
        "/compact" => "compact older tool outputs to free context",
        "/export" => "export the conversation as markdown",
        "/resume" => "continue a saved conversation",
        "/help" => "keys and commands",
        "/exit" => "quit",
        _ => "",
    }
}

/// Commands that read an argument; completing them leaves room to type it.
fn takes_argument(name: &str) -> bool {
    matches!(
        name,
        "/stop" | "/rollback" | "/init" | "/statusline" | "/export"
    )
}

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
        "/thinking",
        "/ps",
        "/stop",
        "/status",
        "/statusline",
        "/clear",
        "/compact",
        "/export",
        "/resume",
        "/help",
        "/exit",
    ]
}

/// Status line items (name, what it shows), in their default order. When the row is too
/// narrow the last ones are dropped first.
pub const STATUSLINE_ITEMS: &[(&str, &str)] = &[
    ("processes", "background processes"),
    ("context", "context left"),
    ("tokens", "session tokens in / out"),
    ("branch", "git branch"),
    ("folder", "working folder"),
];

/// Current git branch (or short commit when detached), read from `.git/HEAD` without git.
pub fn git_branch() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let dot_git = cwd
        .ancestors()
        .map(|d| d.join(".git"))
        .find(|p| p.exists())?;
    // Worktrees and submodules point to the real git dir from a `.git` file.
    let git_dir = if dot_git.is_file() {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let dir = text.trim().strip_prefix("gitdir:")?.trim().to_string();
        dot_git.parent()?.join(dir)
    } else {
        dot_git
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: refs/heads/") {
        Some(branch) => branch.to_string(),
        None => head.chars().take(7).collect(),
    })
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Token counts as people read them: 504, 40.5k, 1.2M.
pub(crate) fn tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
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
        format!("{message}. Run /provider to choose a provider, then /connect.")
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
    let context_window =
        crate::config::model_context_window(model).unwrap_or(crate::config::DEFAULT_CONTEXT_WINDOW);
    let mut value = serde_json::json!({
        "provider": {
            "kind": preset.kind.as_str(),
            "base_url": base_url,
            "connector": connector,
            "api_key_env": preset.api_key_env,
            "model": model,
            "context_window": context_window
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
        assert!(
            matches!(&a.on_key(key(KeyCode::Enter))[..], [Action::Steer(t)] if t == "two"),
            "a second prompt steers the run instead of starting another"
        );
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
        assert_eq!(a.approval_choice, 1, "risky command defaults to No");
        let actions = a.on_key(key(KeyCode::Esc));
        assert!(matches!(&actions[..], [Action::Answer(s)] if s == "n"));

        // Out-of-scope file edits default to Yes so Enter confirms instead of rejecting.
        a.on_event(Ev::PromptText(
            "--- a/package.json\n+++ b/package.json\n@@ -1 +1 @@\n-a\n+b\nEdit to 'package.json' is outside declared scope. Approve? [y]es / [n]o: ".into(),
        ));
        a.on_event(Ev::NeedInput);
        assert_eq!(a.approval_choice, 0, "scope confirm defaults to Yes");
    }

    #[test]
    fn dependency_prompt_takes_text_and_ignores_empty_enter() {
        let mut a = app();
        a.on_event(Ev::PromptText(
            "Dependency addition detected in 'Cargo.toml'. Enter justification to approve: ".into(),
        ));
        a.on_event(Ev::NeedInput);
        assert!(matches!(a.modal, Some(Modal::Text { .. })));
        // Empty Enter keeps the prompt open instead of silently rejecting.
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
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
        assert!(
            a.on_key(ctrl('c')).is_empty(),
            "first idle press only arms quit"
        );
        assert!(matches!(&a.on_key(ctrl('c'))[..], [Action::Quit]));
    }

    #[test]
    fn idle_ctrl_c_clears_draft_before_arming_quit() {
        let mut a = app();
        type_str(&mut a, "half a thought");
        assert!(a.on_key(ctrl('c')).is_empty());
        assert!(a.input.is_empty());
        assert!(!a.quit_armed);
    }

    #[test]
    fn ctrl_c_cancels_first_then_quits_on_second_press_while_working() {
        let mut a = app();
        type_str(&mut a, "go");
        a.on_key(key(KeyCode::Enter));
        assert!(matches!(&a.on_key(ctrl('c'))[..], [Action::Cancel]));
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
            last_input_tokens: 10,
            messages: Vec::new(),
            tool_outcomes: Default::default(),
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
        assert!(a.items.contains(&Item::Note("ok".into())));
        assert!(a.items.contains(&Item::Note(
            "3 steps · 10 in / 4 out tokens · 1 file edited".into()
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
    fn resume_picker_lists_conversations_and_replays_the_chosen_one() {
        use crate::agent::{Message, ToolCall};
        let mut a = app();
        let actions = a.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        drop(actions);
        a.input = "/resume".into();
        a.cursor = a.input.len();
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(
            &actions[..],
            [Action::Send(Cmd::ListConversations)]
        ));
        a.on_event(Ev::Conversations(vec![
            ("fix the bug  · 2m ago · 1 prompt".into(), "id-new".into()),
            ("old work  · 3d ago · 4 prompts".into(), "id-old".into()),
        ]));
        type_str(&mut a, "old");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&actions[..], [Action::Send(Cmd::Resume(id))] if id == "id-old"));

        a.on_event(Ev::Resumed(Box::new(
            crate::conversation::LoadedConversation {
                id: "id-old".into(),
                title: "old work".into(),
                messages: vec![
                    Message::user("old work"),
                    Message::assistant(
                        "reading",
                        vec![
                            ToolCall {
                                id: "1".into(),
                                thought_signature: None,
                                name: "read_file".into(),
                                arguments: r#"{"path":"a.rs"}"#.into(),
                            },
                            ToolCall {
                                id: "2".into(),
                                thought_signature: None,
                                name: "read_file".into(),
                                arguments: r#"{"path":"b.rs"}"#.into(),
                            },
                        ],
                    ),
                    Message::tool("fn main() {}", "1"),
                    Message::tool("pub fn b() {}", "2"),
                    Message::assistant("done", Vec::new()),
                ],
                tool_outcomes: Default::default(),
                total_in: 420,
                total_out: 80,
                context_used: 300,
            },
        )));
        assert_eq!((a.total_in, a.total_out, a.context_used), (420, 80, 300));
        assert!(matches!(a.items.first(), Some(Item::Welcome)));
        assert!(a
            .items
            .iter()
            .any(|i| matches!(i, Item::User(t) if t == "old work")));
        // Tool calls and results are interleaved so view.rs groups consecutive calls like a live run.
        let tool_idx = a
            .items
            .iter()
            .position(|i| matches!(i, Item::Tool { name, summary } if name == "read_file" && summary == "a.rs"))
            .expect("first tool");
        assert!(matches!(
            a.items.get(tool_idx + 1),
            Some(Item::ToolResult { ok: true, .. })
        ));
        assert!(
            matches!(a.items.get(tool_idx + 2), Some(Item::Tool { name, summary }) if name == "read_file" && summary == "b.rs")
        );
        assert!(matches!(
            a.items.get(tool_idx + 3),
            Some(Item::ToolResult { ok: true, .. })
        ));
        assert!(a
            .items
            .iter()
            .any(|i| matches!(i, Item::Assistant(t) if t == "done")));
        assert!(matches!(
            a.items.last(),
            Some(Item::Note(t)) if t.contains("1 prompt") && !t.contains("1 1 prompt")
        ));
    }

    #[test]
    fn resume_picker_disambiguates_duplicate_labels() {
        let mut a = app();
        let _ = a.open_resume();
        a.on_event(Ev::Conversations(vec![
            ("same title  · just now · 1 prompt".into(), "id-1".into()),
            ("same title  · just now · 1 prompt".into(), "id-2".into()),
        ]));
        a.on_key(key(KeyCode::Down));
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&actions[..], [Action::Send(Cmd::Resume(id))] if id == "id-2"));
    }

    #[test]
    fn resume_with_nothing_saved_says_so() {
        let mut a = app();
        let _ = a.open_resume();
        a.on_event(Ev::Conversations(Vec::new()));
        assert!(a.modal.is_none());
        assert!(matches!(a.items.last(), Some(Item::Info(t)) if t.contains("no saved")));
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
        a.on_event(Ev::Provider(
            "gemini · gemini-3-flash-preview".into(),
            Some(1_000_000),
        ));
        assert!(a.models.is_empty(), "provider switch clears catalog");
        a.on_event(Ev::Models(Ok(vec![
            "gemini-3-flash-preview".into(),
            "gemini-flash-lite-latest".into(),
        ])));
        assert_eq!(a.models.len(), 2);
        assert!(a.modal.is_none());
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
            diff: None,
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
            last_input_tokens: 1,
            messages: Vec::new(),
            tool_outcomes: Default::default(),
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
            diff: None,
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
        assert!(matches!(
            &a.on_key(key(KeyCode::Enter))[..],
            [Action::Send(Cmd::ClearHistory)]
        ));
        assert!(a.modal.is_none());
        assert!(matches!(&a.items[..], [Item::Note(_)]));
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
    fn startup_setup_hint_replaces_ready_message() {
        let a = App::new(
            Profile::Build,
            "nim · model".into(),
            Some("No API key for nim. Run /connect or /provider.".into()),
        );
        assert!(
            matches!(a.items.as_slice(), [Item::Welcome, Item::Info(t)] if t.contains("/connect"))
        );
    }

    #[test]
    fn startup_without_nim_label_skips_no_provider_init_hint() {
        let a = App::new(Profile::Build, "nim · model".into(), None);
        assert!(!a.items.iter().any(|i| matches!(
            i,
            Item::Info(t) if t.contains("/init nim")
        )));
    }

    #[test]
    fn reasoning_progress_merges_and_done_cancelled_clears_busy() {
        use crate::agent::{LoopResult, ProgressEvent};
        let mut a = app();
        a.status = Status::Working { step: 1 };
        a.on_event(Ev::Progress(ProgressEvent::Thinking { step: 1 }));
        a.on_event(Ev::Progress(ProgressEvent::Reasoning {
            text: "first ".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Reasoning {
            text: "first step".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "answer".into(),
        }));
        assert!(a
            .items
            .iter()
            .any(|i| matches!(i, Item::Reasoning(t) if t == "first step")));
        assert_eq!(assistant_bubble_texts(&a.items), vec!["answer"]);

        a.on_event(Ev::Done(Box::new(LoopResult {
            exit_reason: ExitReason::Cancelled,
            steps: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            last_input_tokens: 0,
            messages: Vec::new(),
            tool_outcomes: Default::default(),
            summary: None,
            verification: None,
            no_progress: None,
        })));
        assert!(!a.is_working());
        assert!(a
            .items
            .iter()
            .any(|i| matches!(i, Item::Info(t) if t.contains("cancelled"))));
    }

    #[test]
    fn follow_up_typed_while_working_is_queued_and_sent_after_the_run() {
        let mut a = app();
        a.status = Status::Working { step: 1 };
        type_str(&mut a, "can you close the server?");
        assert!(matches!(
            &a.on_key(key(KeyCode::Enter))[..],
            [Action::Steer(_)]
        ));
        assert!(a.input.is_empty());
        assert_eq!(a.queued, vec!["can you close the server?".to_string()]);
        assert!(a.next_queued().is_none(), "nothing is sent while working");
        type_str(&mut a, "and the port?");
        a.on_key(key(KeyCode::Enter));
        a.status = Status::Idle;
        let sent = a.next_queued();
        assert!(
            matches!(&sent, Some(Cmd::Prompt(p)) if p == "can you close the server?\n\nand the port?")
        );
        assert!(a.queued.is_empty());
        assert!(a.is_working());
    }

    #[test]
    fn esc_on_empty_input_pulls_back_the_last_queued_message() {
        let mut a = app();
        a.status = Status::Working { step: 1 };
        type_str(&mut a, "oops");
        a.on_key(key(KeyCode::Enter));
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.input, "oops");
        assert!(a.queued.is_empty());
    }

    #[test]
    fn tool_timer_and_background_count_follow_events() {
        let mut a = app();
        a.on_event(Ev::Progress(ProgressEvent::ToolCall {
            name: "start_process".into(),
            summary: "npm run dev".into(),
        }));
        assert_eq!(
            a.tool_since.as_ref().map(|(n, _)| n.as_str()),
            Some("start_process")
        );
        a.on_event(Ev::Progress(ProgressEvent::ToolResult {
            ok: true,
            preview: "started proc-1".into(),
            name: "start_process".into(),
            diff: None,
        }));
        assert!(a.tool_since.is_none());
        a.on_event(Ev::Processes(1));
        assert_eq!(a.bg_processes, 1);
    }

    #[test]
    fn ps_and_stop_work_while_a_run_is_going() {
        let mut a = app();
        a.status = Status::Working { step: 2 };
        type_str(&mut a, "/ps");
        assert!(matches!(
            &a.on_key(key(KeyCode::Enter))[..],
            [Action::Processes]
        ));
        type_str(&mut a, "/stop proc-1");
        assert!(matches!(
            &a.on_key(key(KeyCode::Enter))[..],
            [Action::StopProcess(id)] if id == "proc-1"
        ));
    }

    #[test]
    fn paste_keeps_indentation_and_places_cursor_after_text() {
        let mut a = app();
        type_str(&mut a, "ab");
        a.on_key(key(KeyCode::Left));
        a.on_paste("fn x() {\r\n\treturn;\n}");
        assert_eq!(a.input, "afn x() {\n\treturn;\n}b");
        assert_eq!(a.cursor, a.input.chars().count() - 1);
    }

    #[test]
    fn esc_clears_input_but_up_restores_it() {
        let mut a = app();
        type_str(&mut a, "long draft");
        a.on_key(key(KeyCode::Esc));
        assert!(a.input.is_empty());
        a.on_key(key(KeyCode::Up));
        assert_eq!(a.input, "long draft");
    }

    #[test]
    fn late_reasoning_tail_updates_in_place_instead_of_duplicating() {
        let mut a = app();
        a.items.clear();
        a.on_event(Ev::Progress(ProgressEvent::Thinking { step: 1 }));
        a.on_event(Ev::Progress(ProgressEvent::Reasoning {
            text: "plan the ans".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "Hello wor".into(),
        }));
        // Redaction holds back the reasoning tail until the stream ends.
        a.on_event(Ev::Progress(ProgressEvent::Reasoning {
            text: "plan the answer".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "Hello world".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "Hello world".into(),
        }));
        assert_eq!(
            a.items,
            vec![
                Item::Reasoning("plan the answer".into()),
                Item::Assistant("Hello world".into()),
            ]
        );
    }

    #[test]
    fn thinking_command_toggles_reasoning_visibility() {
        let mut a = app();
        assert!(!a.show_thinking);
        type_str(&mut a, "/thinking");
        a.on_key(key(KeyCode::Enter));
        assert!(a.show_thinking);
        type_str(&mut a, "/thinking");
        a.on_key(key(KeyCode::Enter));
        assert!(!a.show_thinking);
    }

    #[test]
    fn approval_panel_arrows_pick_and_enter_confirms() {
        let mut a = app();
        a.on_event(Ev::PromptText(
            "--- a/x\n+++ b/x\nApprove edit to 'x'? [y]es / [a]lways / [n]o: ".into(),
        ));
        a.on_event(Ev::NeedInput);
        a.on_key(key(KeyCode::Down));
        assert_eq!(a.approval_choice, 1);
        assert!(matches!(&a.on_key(key(KeyCode::Enter))[..], [Action::Answer(s)] if s == "a"));
        a.on_event(Ev::PromptText(
            "Destructive command detected: 'npm run dev' (starts a managed long-running process). Approve? [y]es / [n]o: ".into(),
        ));
        a.on_event(Ev::NeedInput);
        assert_eq!(
            a.approval_choice, 0,
            "each question starts on the first option"
        );
        assert!(matches!(&a.on_key(key(KeyCode::Char('2')))[..], [Action::Answer(s)] if s == "n"));
    }

    #[test]
    fn approval_note_between_call_and_result_keeps_result_on_its_card() {
        let mut a = app();
        a.items.clear();
        a.on_event(Ev::Progress(ProgressEvent::ToolCall {
            name: "write_file".into(),
            summary: "src/index.css".into(),
        }));
        a.items.push(Item::Info("edit approved".into()));
        a.on_event(Ev::Progress(ProgressEvent::ToolResult {
            name: "write_file".into(),
            ok: true,
            preview: "Updated src/index.css".into(),
            diff: None,
        }));
        assert!(matches!(a.items[1], Item::ToolResult { .. }));
        assert!(matches!(a.items[2], Item::Info(_)));
    }

    #[test]
    fn retry_drops_the_failed_attempts_partial_text() {
        let mut a = app();
        a.items.clear();
        a.on_event(Ev::Progress(ProgressEvent::Thinking { step: 1 }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "Half an ans".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Retry {
            attempt: 1,
            max: 3,
            reason: "Connection reset by peer".into(),
        }));
        a.on_event(Ev::Progress(ProgressEvent::Assistant {
            text: "Full answer".into(),
        }));
        assert_eq!(
            a.items,
            vec![
                Item::Info("connection lost; retrying (1/3)".into()),
                Item::Assistant("Full answer".into()),
            ]
        );
    }

    #[test]
    fn typing_slash_lists_matching_commands_live() {
        let mut a = app();
        type_str(&mut a, "/c");
        let names: Vec<&str> = a.suggestions().iter().map(|(n, _)| *n).collect();
        assert_eq!(&names[..3], ["/connect", "/connections", "/clear"]);
        type_str(&mut a, "l");
        assert_eq!(a.suggestions()[0].0, "/clear");
        type_str(&mut a, " ");
        assert!(a.suggestions().is_empty(), "an argument hides the list");
    }

    #[test]
    fn suggestion_keys_pick_complete_and_run() {
        let mut a = app();
        type_str(&mut a, "/th");
        assert!(matches!(&a.on_key(key(KeyCode::Enter))[..], []));
        assert!(a.show_thinking, "Enter ran /thinking");
        assert!(a.input.is_empty());
        type_str(&mut a, "/s");
        a.on_key(key(KeyCode::Down));
        let second = a.suggestions()[1].0;
        a.on_key(key(KeyCode::Tab));
        assert!(a.input.starts_with(second));
        a.input.clear();
        a.cursor = 0;
        type_str(&mut a, "/sto");
        a.on_key(key(KeyCode::Enter));
        assert_eq!(
            a.input, "/stop ",
            "commands with an argument complete first"
        );
    }

    #[test]
    fn steered_message_moves_from_queue_into_transcript() {
        let mut a = app();
        a.status = Status::Working { step: 1 };
        type_str(&mut a, "use port 3000");
        a.on_key(key(KeyCode::Enter));
        a.on_event(Ev::Progress(ProgressEvent::Steered {
            text: "use port 3000".into(),
        }));
        assert!(a.queued.is_empty());
        assert_eq!(a.items.last(), Some(&Item::User("use port 3000".into())));
        type_str(&mut a, "oops");
        a.on_key(key(KeyCode::Enter));
        assert!(
            matches!(&a.on_key(key(KeyCode::Esc))[..], [Action::Unsteer(t)] if t == "oops"),
            "Esc takes an unread message back"
        );
        assert_eq!(a.input, "oops");
    }

    #[test]
    fn context_left_follows_the_latest_request() {
        let mut a = app();
        a.context_window = Some(100_000);
        assert_eq!(a.context_left(), None, "nothing sent yet");
        a.on_event(Ev::Progress(ProgressEvent::Usage {
            input: 30_000,
            output: 500,
        }));
        assert_eq!(a.context_left(), Some(70));
        // Usage is cumulative within a run; the second request sent 40k.
        a.on_event(Ev::Progress(ProgressEvent::Usage {
            input: 70_000,
            output: 900,
        }));
        assert_eq!(a.context_left(), Some(60));
        type_str(&mut a, "/clear");
        a.on_key(key(KeyCode::Enter));
        assert_eq!(a.context_left(), None);
    }

    #[test]
    fn statusline_command_toggles_and_saves_items() {
        let mut a = app();
        assert!(a.statusline.iter().any(|s| s == "branch"));
        type_str(&mut a, "/statusline branch");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(!a.statusline.iter().any(|s| s == "branch"));
        assert!(
            matches!(&actions[..], [Action::Send(Cmd::SaveStatusline(items))] if !items.contains(&"branch".to_string()))
        );
        type_str(&mut a, "/statusline branch");
        a.on_key(key(KeyCode::Enter));
        let names: Vec<&str> = STATUSLINE_ITEMS.iter().map(|(n, _)| *n).collect();
        assert_eq!(a.statusline, names, "shown again in its default place");
        type_str(&mut a, "/statusline nope");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        assert!(matches!(a.items.last(), Some(Item::Error(e)) if e.contains("unknown item")));
    }

    #[test]
    fn status_command_lists_session_details() {
        let mut a = app();
        a.context_window = Some(128_000);
        a.on_event(Ev::ContextSource(Some("model")));
        type_str(&mut a, "/status");
        a.on_key(key(KeyCode::Enter));
        let text = format!("{:?}", a.items);
        for part in [
            "model",
            "context     128.0k window (model)",
            "tokens",
            "branch",
            "folder",
        ] {
            assert!(text.contains(part), "{part}: {text}");
        }
    }

    #[test]
    fn git_branch_reads_this_repository() {
        // Tests run from the crate root, which is a git checkout.
        assert!(git_branch().is_some_and(|b| !b.is_empty()));
    }

    #[test]
    fn model_list_arriving_selects_the_current_model() {
        let mut a = App::new(Profile::Build, "nim · b/two".into(), None);
        type_str(&mut a, "/model");
        a.on_key(key(KeyCode::Enter));
        a.on_event(Ev::Models(Ok(vec!["a/one".into(), "b/two".into()])));
        assert!(matches!(a.modal, Some(Modal::Picker { selected: 1, .. })));
    }

    #[test]
    fn model_picker_hides_non_chat_models_but_keeps_typed_id_usable() {
        let mut a = app();
        a.on_event(Ev::Models(Ok(vec![
            "nvidia/nemotron-3.5-super-vl-120b-a12b".into(),
            "nvidia/nv-embedqa-e5-v5".into(),
            "nvidia/llama-3.2-nv-rerankqa-1b-v2".into(),
            "meta/llama-guard-3-8b".into(),
            "hive/ai-generated-detector".into(),
            "openai/tts-1".into(),
            "openai/whisper-large-v3".into(),
            "nvidia/nemotron-content-safety-reasoning-4b".into(),
            "gpt-4o".into(),
        ])));
        assert_eq!(
            a.models,
            vec!["nvidia/nemotron-3.5-super-vl-120b-a12b", "gpt-4o"]
        );

        // Opening /model shows only the chat models, and typing a filtered/custom id still submits it.
        type_str(&mut a, "/model");
        a.on_key(key(KeyCode::Enter));
        assert_eq!(
            picker(&a).visible(),
            vec!["nvidia/nemotron-3.5-super-vl-120b-a12b", "gpt-4o"]
        );
        type_str(&mut a, "openai/whisper-large-v3");
        assert!(picker(&a).visible().is_empty());
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(
            &actions[..],
            [Action::Send(Cmd::SetModel(id))] if id == "openai/whisper-large-v3"
        ));

        // Direct `/model <id>` also works for non-chat or unlisted ids.
        a.input.clear();
        a.cursor = 0;
        type_str(&mut a, "/model openai/tts-1");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(
            &actions[..],
            [Action::Send(Cmd::SetModel(id))] if id == "openai/tts-1"
        ));
    }

    #[test]
    fn compact_and_export_slash_commands_dispatch_and_handle_events() {
        let mut a = app();
        a.context_used = 5_000;
        type_str(&mut a, "/compact");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&actions[..], [Action::Send(Cmd::Compact)]));
        a.on_event(Ev::Compacted(
            "Compacted conversation (8 -> 3 messages).".into(),
        ));
        assert_eq!(a.context_used, 0);
        assert!(matches!(
            a.items.last(),
            Some(Item::Note(t)) if t.contains("Compacted conversation")
        ));

        a.input.clear();
        a.cursor = 0;
        type_str(&mut a, "/export");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        assert!(matches!(
            a.items.last(),
            Some(Item::Info(t)) if t.contains("usage: /export PATH")
        ));

        a.input.clear();
        a.cursor = 0;
        type_str(&mut a, "/export docs/chat log.md");
        let actions = a.on_key(key(KeyCode::Enter));
        assert!(matches!(
            &actions[..],
            [Action::Send(Cmd::Export(p))] if p == "docs/chat log.md"
        ));
    }
}
