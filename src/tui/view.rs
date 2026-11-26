//! Rendering. Reads `App`; writes back only what drawing decides: the scroll clamps, the
//! transcript area, and the mouse selection's text.
use super::app::{
    help_text, App, ApprovalChoice, Item, Modal, PickKind, Status, CONFIRM_CHOICES, EDIT_CHOICES,
};
use crate::Profile;
use ratatui::{
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const INPUT_MIN_HEIGHT: u16 = 3;
const INPUT_MAX_HEIGHT: u16 = 8;

/// Visual line count for the input pane (soft-wrap at `width` minus borders).
/// Columns the `› ` prompt takes before the typed text.
const PROMPT_WIDTH: u16 = 2;

pub(crate) fn wrapped_input_line_count(input: &str, area_width: u16) -> usize {
    // Border and one column of padding on each side, then the prompt.
    let inner = area_width.saturating_sub(4 + PROMPT_WIDTH).max(1) as usize;
    layout_input(input, 0, inner).0.len()
}

/// Total input widget height (border + text lines), capped for layout.
pub(crate) fn input_area_height(input: &str, area_width: u16) -> u16 {
    let lines = wrapped_input_line_count(input, area_width);
    (lines as u16 + 2).clamp(INPUT_MIN_HEIGHT, INPUT_MAX_HEIGHT)
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // As in Codex and Gemini CLI: activity above the input while it matters, session details
    // (profile, model, folder) on one line under it, no permanent header.
    let status_rows = u16::from(app.is_working() || app.modal.is_some() || flash(app).is_some());
    let [body, status, input, info] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(status_rows),
        Constraint::Length(input_area_height(&app.input, area.width)),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_transcript(frame, body, app);
    draw_status(frame, status, app);
    draw_input(frame, input, app);
    draw_info(frame, info, app);
    draw_suggestions(frame, body, app);
    if app.modal.is_some() {
        draw_modal(frame, area, app);
    }
}

fn profile_color(profile: Profile) -> Color {
    match profile {
        Profile::Ask => Color::Green,
        Profile::Plan => Color::Yellow,
        Profile::Build => Color::Magenta,
    }
}

/// `~` for the home directory, and only the last two components when the path is long.
fn short_path(path: &std::path::Path) -> String {
    let shown = match std::env::var_os("HOME").map(std::path::PathBuf::from) {
        Some(home) if path.starts_with(&home) => {
            format!("~/{}", path.strip_prefix(&home).unwrap_or(path).display())
        }
        _ => path.display().to_string(),
    };
    let parts: Vec<&str> = shown.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() > 3 {
        format!("…/{}", parts[parts.len() - 2..].join("/"))
    } else {
        shown
    }
}

/// Startup card, as Claude Code and Codex open: name and version, then provider, model,
/// profile and folder in an aligned column, each with its key hint.
fn welcome_card(app: &App) -> Vec<Line<'static>> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let border = Style::new().fg(Color::DarkGray);
    let cwd = std::env::current_dir().unwrap_or_default();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let folder = match home {
        Some(h) if cwd.starts_with(&h) => {
            format!("~/{}", cwd.strip_prefix(&h).unwrap_or(&cwd).display())
        }
        _ => cwd.display().to_string(),
    };
    let (provider, model, effort) = label_parts(&app.provider);
    let model = match effort {
        Some(e) => format!("{model}  ({e} effort)"),
        None => model.to_string(),
    };
    let profile = app.profile.name().to_string();
    let value_width = [
        provider.width(),
        model.width(),
        profile.width(),
        folder.width(),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    // Hints line up in their own column after the widest value.
    let row = |label: &str, value: String, value_style: Style, hint: &str| {
        let pad = value_width.saturating_sub(value.width()) + 3;
        let hint = if hint.is_empty() {
            String::new()
        } else {
            format!("{}{hint}", " ".repeat(pad))
        };
        vec![
            Span::styled(format!("{label:<10}"), dim),
            Span::styled(value, value_style),
            Span::styled(hint, dim),
        ]
    };
    let rows: Vec<Vec<Span<'static>>> = vec![
        vec![
            Span::styled(">_ ", Style::new().fg(Color::Cyan).bold()),
            Span::styled("DevCode", Style::new().bold()),
            Span::styled(format!("  v{}", env!("CARGO_PKG_VERSION")), dim),
        ],
        Vec::new(),
        row("provider", provider.to_string(), Style::new(), "/provider"),
        row("model", model, Style::new().bold(), "/model"),
        row(
            "profile",
            profile.clone(),
            Style::new().fg(profile_color(app.profile)),
            "Shift+Tab",
        ),
        row("folder", folder, Style::new(), ""),
    ];
    let inner = rows
        .iter()
        .map(|r| r.iter().map(|s| s.content.width()).sum::<usize>())
        .max()
        .unwrap_or(0)
        + 3;
    let mut out = vec![Line::styled(format!("╭{}╮", "─".repeat(inner)), border)];
    for row in rows {
        let used: usize = row.iter().map(|s| s.content.width()).sum();
        let mut spans = vec![Span::styled("│ ", border)];
        spans.extend(row);
        spans.push(Span::raw(" ".repeat(inner - 1 - used)));
        spans.push(Span::styled("│", border));
        out.push(Line::from(spans));
    }
    out.push(Line::styled(format!("╰{}╯", "─".repeat(inner)), border));
    out
}

/// Tools whose consecutive calls collapse into one summary line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolGroup {
    Explore,
    Run,
    Edit,
}

fn tool_group(name: &str) -> Option<ToolGroup> {
    match name {
        "read_file" | "glob" | "grep" | "list_processes" | "process_status" => {
            Some(ToolGroup::Explore)
        }
        "run_command" => Some(ToolGroup::Run),
        "edit_file" | "write_file" => Some(ToolGroup::Edit),
        _ => None,
    }
}

/// A run of same-group tool calls starting at `start`: the Tool item indexes, the index just
/// past the run, and per call (name, summary, result: None while running, Some(ok)).
struct ToolRun<'a> {
    kind: ToolGroup,
    calls: Vec<usize>,
    end: usize,
    entries: Vec<(&'a str, &'a str, Option<bool>)>,
    /// Lines added and removed across the run's edit diffs.
    added: usize,
    removed: usize,
}

fn tool_run(items: &[Item], start: usize) -> ToolRun<'_> {
    let Item::Tool { name, .. } = &items[start] else {
        unreachable!("tool_run starts on a Tool item")
    };
    let kind = tool_group(name).expect("grouped tool");
    let mut run = ToolRun {
        kind,
        calls: Vec::new(),
        end: start,
        entries: Vec::new(),
        added: 0,
        removed: 0,
    };
    let mut j = start;
    while let Some(Item::Tool { name, summary }) = items.get(j) {
        if tool_group(name) != Some(kind) {
            break;
        }
        let result = match items.get(j + 1) {
            Some(Item::ToolResult { ok, diff, .. }) => {
                if let Some(diff) = diff {
                    let (_, added, removed) = numbered_diff(diff);
                    run.added += added;
                    run.removed += removed;
                }
                Some(*ok)
            }
            _ => None,
        };
        run.calls.push(j);
        run.entries.push((name.as_str(), summary.as_str(), result));
        j += if result.is_some() { 2 } else { 1 };
    }
    run.end = j;
    run
}

fn group_line(run: &ToolRun, dim: Style) -> Line<'static> {
    let running = run.entries.iter().any(|(.., r)| r.is_none());
    let failed = run
        .entries
        .iter()
        .filter(|(.., r)| *r == Some(false))
        .count();
    let dot = if running {
        Color::Yellow
    } else if failed > 0 {
        Color::Red
    } else {
        Color::Green
    };
    let count = |pred: &dyn Fn(&str) -> bool| run.entries.iter().filter(|(n, ..)| pred(n)).count();
    let (verb, counts, detail) = match run.kind {
        ToolGroup::Explore => {
            let files = count(&|n| n == "read_file");
            let searches = count(&|n| n == "glob" || n == "grep");
            let other = run.entries.len() - files - searches;
            let mut parts = Vec::new();
            if files > 0 {
                parts.push(format!("{files} file{}", if files == 1 { "" } else { "s" }));
            }
            if searches > 0 {
                parts.push(format!(
                    "{searches} search{}",
                    if searches == 1 { "" } else { "es" }
                ));
            }
            if other > 0 {
                parts.push(format!(
                    "{other} check{}",
                    if other == 1 { "" } else { "s" }
                ));
            }
            let names: Vec<String> = run
                .entries
                .iter()
                .map(|(n, s, _)| {
                    if *n == "read_file" {
                        s.rsplit('/').next().unwrap_or(s).to_string()
                    } else {
                        s.to_string()
                    }
                })
                .collect();
            let verb = if running { "Exploring" } else { "Explored" };
            (verb, parts.join(", "), names.join(", "))
        }
        ToolGroup::Edit => {
            let mut files: Vec<&str> = Vec::new();
            for (_, s, _) in &run.entries {
                let name = s.rsplit('/').next().unwrap_or(s);
                if !files.contains(&name) {
                    files.push(name);
                }
            }
            let n = files.len();
            let verb = if running { "Editing" } else { "Edited" };
            (
                verb,
                format!("{n} file{}", if n == 1 { "" } else { "s" }),
                files.join(", "),
            )
        }
        ToolGroup::Run => {
            let n = run.entries.len();
            let verb = if running { "Running" } else { "Ran" };
            let cmds: Vec<String> = run
                .entries
                .iter()
                .map(|(_, s, _)| {
                    let s: String = s.chars().take(32).collect();
                    s
                })
                .collect();
            (
                verb,
                format!("{n} command{}", if n == 1 { "" } else { "s" }),
                cmds.join(" · "),
            )
        }
    };
    let mut detail: String = detail;
    if detail.chars().count() > 60 {
        detail = detail.chars().take(59).collect::<String>() + "…";
    }
    let mut spans = vec![
        Span::styled("● ", Style::new().fg(dot)),
        Span::styled(format!("{verb} "), Style::new().bold()),
        Span::raw(counts),
        Span::styled(format!(" ({detail})"), dim),
    ];
    if run.added + run.removed > 0 {
        spans.push(Span::styled(
            format!("  +{}", run.added),
            Style::new().fg(Color::Green),
        ));
        spans.push(Span::styled(
            format!(" −{}", run.removed),
            Style::new().fg(Color::Red),
        ));
    }
    if failed > 0 {
        spans.push(Span::styled(
            format!("  {failed} failed"),
            Style::new().fg(Color::Red),
        ));
    }
    Line::from(spans)
}

/// One tool card with its full result, for an expanded group.
fn push_tool_card(
    lines: &mut Vec<Line<'static>>,
    items: &[Item],
    i: usize,
    name: &str,
    summary: &str,
    expanded: bool,
    dim: Style,
) {
    let result = match items.get(i + 1) {
        Some(Item::ToolResult { ok, preview, diff }) => Some((*ok, preview, diff)),
        _ => None,
    };
    let dot = match result {
        None => Color::Yellow,
        Some((true, ..)) => Color::Green,
        Some((false, ..)) => Color::Red,
    };
    lines.push(Line::from(vec![
        Span::styled("● ", Style::new().fg(dot)),
        Span::styled(name.to_string(), Style::new().bold()),
        Span::styled(format!("  {summary}"), dim),
    ]));
    match result {
        Some((true, preview, Some(diff))) => push_diff_lines(lines, preview, diff, expanded, dim),
        Some((ok, preview, _)) => push_tool_result_lines(lines, ok, preview, expanded, dim),
        None => lines.push(Line::styled("  ⎿  running…", dim)),
    }
}

/// Edit result as Claude Code shows it: a count line, then the numbered, tinted diff. Long
/// diffs are cut to the first lines until the card is expanded (Ctrl-T).
fn push_diff_lines(
    lines: &mut Vec<Line<'static>>,
    preview: &str,
    diff: &str,
    expanded: bool,
    dim: Style,
) {
    const COLLAPSED: usize = 16;
    let (body, added, removed) = numbered_diff(diff);
    let what = preview.lines().next().unwrap_or("Updated");
    lines.push(Line::from(vec![
        Span::styled("  ⎿  ", dim),
        Span::raw(format!(
            "{what} with {} and {}",
            plural_word(added, "addition", "additions"),
            plural_word(removed, "removal", "removals")
        )),
    ]));
    let shown = if expanded {
        body.len()
    } else {
        body.len().min(COLLAPSED)
    };
    let hidden = body.len() - shown;
    for line in body.into_iter().take(shown) {
        let mut spans = vec![Span::raw("     ")];
        spans.extend(line.spans);
        lines.push(Line::from(spans).style(line.style));
    }
    if hidden > 0 {
        lines.push(Line::styled(
            format!("     … +{hidden} lines (Ctrl-T to expand)"),
            dim,
        ));
    }
}

fn plural_word(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Result lines under a tool card: `⎿` then the first preview line, or every line when the
/// card is expanded.
fn push_tool_result_lines(
    lines: &mut Vec<Line<'static>>,
    ok: bool,
    preview: &str,
    expanded: bool,
    dim: Style,
) {
    let preview_lines: Vec<&str> = preview.lines().collect();
    let color = if ok { Color::Reset } else { Color::Red };
    let style = if ok { dim } else { Style::new().fg(color) };
    let Some(first) = preview_lines.first() else {
        lines.push(Line::from(vec![
            Span::styled("  ⎿  ", dim),
            Span::styled(if ok { "done" } else { "failed" }, style),
        ]));
        return;
    };
    if expanded {
        for (j, text) in preview_lines.iter().enumerate() {
            let lead = if j == 0 { "  ⎿  " } else { "     " };
            lines.push(Line::from(vec![
                Span::styled(lead, dim),
                Span::styled((*text).to_string(), style),
            ]));
        }
        return;
    }
    let more = preview_lines.len() - 1;
    let suffix = match more {
        0 => String::new(),
        1 => "  (+1 line)".to_string(),
        n => format!("  (+{n} lines)"),
    };
    lines.push(Line::from(vec![
        Span::styled("  ⎿  ", dim),
        Span::styled((*first).to_string(), style),
        Span::styled(suffix, dim),
    ]));
}

fn transcript_lines(app: &App) -> Vec<Line<'static>> {
    let (items, expanded_tool, show_thinking) =
        (&app.items[..], app.expanded_tool, app.show_thinking);
    let dim = Style::new().add_modifier(Modifier::DIM);
    let border = Style::new().fg(Color::DarkGray);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            Item::Welcome => lines.extend(welcome_card(app)),
            Item::User(text) => {
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![
                    Span::styled("› ", Style::new().bold().fg(Color::Cyan)),
                    Span::styled(text.clone(), Style::new().bold()),
                ]));
            }
            Item::Reasoning(_) if !show_thinking => {}
            Item::Reasoning(text) => {
                lines.push(Line::raw(""));
                let reasoning_style = dim.add_modifier(Modifier::ITALIC);
                for (j, l) in text.lines().enumerate() {
                    let prefix = if j == 0 { "  · " } else { "    " };
                    lines.push(Line::styled(format!("{prefix}{l}"), reasoning_style));
                }
            }
            Item::Assistant(text) => {
                lines.push(Line::raw(""));
                lines.extend(super::markdown::render(text));
            }
            Item::Tool { name, .. } if tool_group(name).is_some() => {
                // Runs of reads, searches or commands collapse into one line, as Claude Code
                // shows them ("Explored 2 files (a.rs, b.rs)"). Ctrl-T on the run expands it.
                let group = tool_run(items, i);
                if !group.calls.iter().any(|&c| expanded_tool == Some(c)) {
                    if i == 0
                        || !matches!(items[i - 1], Item::ToolResult { .. } | Item::Tool { .. })
                    {
                        lines.push(Line::raw(""));
                    }
                    lines.push(group_line(&group, dim));
                    i = group.end;
                    continue;
                }
                // Expanded: one card per call, the expanded one with its full output.
                lines.push(Line::raw(""));
                for &c in &group.calls {
                    if let Item::Tool { name, summary } = &items[c] {
                        let open = expanded_tool == Some(c);
                        push_tool_card(&mut lines, items, c, name, summary, open, dim);
                    }
                }
                i = group.end;
                continue;
            }
            Item::Tool { name, summary } => {
                let expanded = expanded_tool == Some(i);
                let result = match items.get(i + 1) {
                    Some(Item::ToolResult { ok, preview, diff }) => Some((*ok, preview, diff)),
                    _ => None,
                };
                let dot = match result {
                    None => Style::new().fg(Color::Yellow),
                    Some((true, ..)) => Style::new().fg(Color::Green),
                    Some((false, ..)) => Style::new().fg(Color::Red),
                };
                if i == 0 || !matches!(items[i - 1], Item::ToolResult { .. } | Item::Tool { .. }) {
                    lines.push(Line::raw(""));
                }
                lines.push(Line::from(vec![
                    Span::styled("● ", dot),
                    Span::styled(name.clone(), Style::new().bold()),
                    Span::styled(format!("  {summary}"), dim),
                ]));
                match result {
                    Some((true, preview, Some(diff))) => {
                        push_diff_lines(&mut lines, preview, diff, expanded, dim);
                        i += 2;
                        continue;
                    }
                    Some((ok, preview, _)) => {
                        push_tool_result_lines(&mut lines, ok, preview, expanded, dim);
                        i += 2;
                        continue;
                    }
                    None => lines.push(Line::from(vec![Span::styled("  ⎿  running…", dim)])),
                }
            }
            Item::ToolResult { ok, preview, .. } => {
                push_tool_result_lines(&mut lines, *ok, preview, false, dim);
            }
            Item::Command(text) => {
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![
                    Span::styled("› ", Style::new().fg(Color::Cyan)),
                    Span::styled(text.clone(), Style::new().bold()),
                ]));
            }
            Item::Info(text) => {
                // A slash command's output nests under it, as Claude Code shows tool output.
                let run_start = items[..i]
                    .iter()
                    .rposition(|it| !matches!(it, Item::Info(_)));
                let under_command = run_start.is_some_and(|s| matches!(items[s], Item::Command(_)));
                let (prefix, style) = match (under_command, run_start == i.checked_sub(1)) {
                    (true, true) => ("  ⎿  ", Style::new()),
                    (true, false) => ("     ", Style::new()),
                    _ => ("  · ", Style::new().fg(Color::Gray)),
                };
                lines.push(Line::from(vec![
                    Span::styled(prefix, dim),
                    Span::styled(text.clone(), style),
                ]));
            }
            Item::Error(text) => {
                let under_command = i > 0 && matches!(items[i - 1], Item::Command(_));
                lines.push(Line::styled(
                    format!("{}{text}", if under_command { "  ⎿  " } else { "  ✗ " }),
                    Style::new().fg(Color::Red),
                ));
            }
            Item::Note(text) => lines.push(Line::styled(format!("  {text}"), dim)),
        }
        i += 1;
    }
    // Messages typed during the run, shown where they will appear once sent.
    if !app.queued.is_empty() {
        lines.push(Line::raw(""));
    }
    for text in &app.queued {
        lines.push(Line::from(vec![
            Span::styled("  ↳ queued  ", border),
            Span::styled(text.clone(), dim),
        ]));
    }
    lines
}

fn draw_transcript(frame: &mut Frame, area: Rect, app: &mut App) {
    let paragraph = Paragraph::new(transcript_lines(app)).wrap(Wrap { trim: false });
    let total = paragraph.line_count(area.width);
    let max_scroll = total.saturating_sub(area.height as usize);
    app.scroll = app.scroll.min(max_scroll);
    let top = (max_scroll - app.scroll).min(u16::MAX as usize) as u16;
    frame.render_widget(paragraph.clone().scroll((top, 0)), area);
    app.transcript_area = area;
    app.transcript_top = top as usize;
    draw_selection(frame, area, app, &paragraph, total);
}

/// Highlights the visible part of the mouse selection. After a release it copies the whole
/// selection, rows scrolled off screen included, from an off-screen render of the transcript,
/// so the copy matches the wrapped text the user saw.
fn draw_selection(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    paragraph: &Paragraph,
    total: usize,
) {
    let Some((anchor, head)) = app.selection else {
        return;
    };
    if area.is_empty() || total == 0 {
        return;
    }
    let last_row = total - 1;
    let clamp = |(x, row): (u16, usize)| {
        (
            x.clamp(area.x, area.right() - 1) - area.x,
            row.min(last_row),
        )
    };
    let (anchor, head) = (clamp(anchor), clamp(head));
    let (start, end) = if (anchor.1, anchor.0) <= (head.1, head.0) {
        (anchor, head)
    } else {
        (head, anchor)
    };
    let span = |row: usize, width: u16| {
        let first = if row == start.1 { start.0 } else { 0 };
        let last = if row == end.1 { end.0 } else { width - 1 };
        first..=last
    };
    let top = app.transcript_top;
    let buffer = frame.buffer_mut();
    for row in start.1.max(top)..=end.1.min(top + area.height as usize - 1) {
        let y = area.y + (row - top) as u16;
        for x in span(row, area.width) {
            buffer[(area.x + x, y)].set_style(Style::new().add_modifier(Modifier::REVERSED));
        }
    }
    if !std::mem::take(&mut app.copy_requested) {
        return;
    }
    // Render rows start..=end alone, then read the selected cells.
    let rows = (end.1 - start.1 + 1).min(u16::MAX as usize) as u16;
    let off = Rect::new(0, 0, area.width, rows);
    let mut copy = ratatui::buffer::Buffer::empty(off);
    let scroll = start.1.min(u16::MAX as usize) as u16;
    ratatui::widgets::Widget::render(paragraph.clone().scroll((scroll, 0)), off, &mut copy);
    let mut text = String::new();
    for (i, row) in (start.1..=end.1).enumerate().take(rows as usize) {
        let mut line = String::new();
        let mut skip = 0usize;
        for x in span(row, area.width) {
            // A wide character fills the next cell too; copy it once.
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let symbol = copy[(x, i as u16)].symbol();
            skip = symbol.width().saturating_sub(1);
            line.push_str(symbol);
        }
        if i > 0 {
            text.push('\n');
        }
        text.push_str(line.trim_end());
    }
    app.copied = Some(text);
}

/// Slash command suggestions docked at the bottom of the transcript, right above the input.
fn draw_suggestions(frame: &mut Frame, body: Rect, app: &App) {
    let list = app.suggestions();
    if list.is_empty() {
        return;
    }
    const SHOWN: usize = 8;
    let selected = app.suggest.min(list.len() - 1);
    let first = selected.saturating_sub(SHOWN - 1);
    let rows = list.len().min(SHOWN) as u16;
    let height = rows.min(body.height);
    let area = Rect::new(body.x, body.bottom() - height, body.width, height);
    let name_width = list.iter().map(|(n, _)| n.width()).max().unwrap_or(0) + 3;
    let dim = Style::new().add_modifier(Modifier::DIM);
    let lines: Vec<Line> = list
        .iter()
        .enumerate()
        .skip(first)
        .take(height as usize)
        .map(|(i, (name, about))| {
            let style = if i == selected {
                Style::new().fg(Color::Cyan).bold()
            } else {
                Style::new()
            };
            Line::from(vec![
                Span::styled(if i == selected { " › " } else { "   " }, style),
                Span::styled(format!("{name:<name_width$}"), style),
                Span::styled(about.to_string(), if i == selected { style } else { dim }),
            ])
        })
        .collect();
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_input(frame: &mut Frame, area: Rect, app: &App) {
    let busy = app.is_working();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
        .padding(ratatui::widgets::Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text_width = inner.width.saturating_sub(PROMPT_WIDTH).max(1) as usize;
    let inner_height = inner.height as usize;
    let (display_lines, (cursor_line, cursor_col)) =
        layout_input(&app.input, app.cursor, text_width);
    let scroll_line = cursor_line.saturating_sub(inner_height.saturating_sub(1));
    let prompt = Style::new()
        .fg(if busy { Color::DarkGray } else { Color::Cyan })
        .bold();
    let dim = Style::new().add_modifier(Modifier::DIM);
    let visible: Vec<Line> = if app.input.is_empty() {
        let hint = if busy {
            "Type to steer the run · Esc takes back the last message"
        } else {
            "Ask anything · / for commands · Shift+Tab switches profile"
        };
        vec![Line::from(vec![
            Span::styled("› ", prompt),
            Span::styled(hint, dim),
        ])]
    } else {
        display_lines
            .iter()
            .enumerate()
            .skip(scroll_line)
            .take(inner_height)
            .map(|(i, s)| {
                let lead = if i == 0 { "› " } else { "  " };
                Line::from(vec![Span::styled(lead, prompt), Span::raw(s.clone())])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(visible), inner);
    if app.modal.is_none() {
        let y = inner.y + cursor_line.saturating_sub(scroll_line) as u16;
        let x = (cursor_col as u16 + PROMPT_WIDTH).min(inner.width.saturating_sub(1));
        frame.set_cursor_position(Position::new(inner.x + x, y));
    }
}

/// Splits the input into rows at most `width` terminal columns wide (wide CJK/emoji
/// characters count as two) and returns the rows plus the cursor's (row, column).
fn layout_input(input: &str, cursor: usize, width: usize) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let mut rows = vec![String::new()];
    let mut col = 0usize;
    let mut pos = None;
    for (index, c) in input.chars().enumerate() {
        if c == '\n' {
            if index == cursor {
                pos = Some((rows.len() - 1, col));
            }
            rows.push(String::new());
            col = 0;
            continue;
        }
        let w = if c == '\t' { 4 } else { c.width().unwrap_or(0) };
        if col > 0 && col + w > width {
            rows.push(String::new());
            col = 0;
        }
        if index == cursor {
            pos = Some((rows.len() - 1, col));
        }
        if let Some(row) = rows.last_mut() {
            if c == '\t' {
                row.push_str("    ");
            } else {
                row.push(c);
            }
        }
        col += w;
    }
    let (row, col) = pos.unwrap_or((rows.len() - 1, col));
    (rows, (row, col.min(width - 1)))
}

fn modal_blocked_label(modal: &Modal) -> &'static str {
    match modal {
        Modal::Approve { .. } => "awaiting approval",
        Modal::Confirm { .. } => "awaiting confirmation",
        Modal::Text { .. } => "awaiting input",
        Modal::Secret { .. } => "awaiting secret",
        Modal::ConfirmRollback { .. } => "awaiting confirmation",
        Modal::Picker { .. } => "modal · picker",
        Modal::Help => "modal · help",
    }
}

/// Left segment of the footer status strip.
pub(crate) fn footer_status_left(app: &App) -> String {
    if let Some(modal) = &app.modal {
        return format!(" blocked · {}", modal_blocked_label(modal));
    }
    match app.status {
        Status::Working { step } if app.tool_since.is_some() => {
            let (name, since) = app
                .tool_since
                .as_ref()
                .map_or(("tool", 0), |(n, s)| (n.as_str(), s.elapsed().as_secs()));
            format!(
                " {} {name} {since}s · step {} · {} in / {} out",
                SPINNER[app.tick % SPINNER.len()],
                step.max(1),
                app.run_in,
                app.run_out
            )
        }
        Status::Working { step } if !app.show_thinking && app.thinking_since.is_some() => {
            let secs = app.thinking_since.map_or(0, |s| s.elapsed().as_secs());
            format!(
                " {} thinking {}s · step {} · {} in / {} out",
                SPINNER[app.tick % SPINNER.len()],
                secs,
                step.max(1),
                app.run_in,
                app.run_out
            )
        }
        Status::Working { step } => {
            let secs = app.started.map_or(0, |s| s.elapsed().as_secs());
            format!(
                " {} step {} · {}s · {} in / {} out",
                SPINNER[app.tick % SPINNER.len()],
                step.max(1),
                secs,
                app.run_in,
                app.run_out
            )
        }
        Status::Idle => format!(
            " ready · session {} in / {} out",
            app.total_in, app.total_out
        ),
    }
}

fn flash(app: &App) -> Option<&str> {
    app.flash
        .as_ref()
        .filter(|(_, at)| at.elapsed() < std::time::Duration::from_secs(2))
        .map(|(text, _)| text.as_str())
}

/// Left and right text in one row, the left side cut with `…` when both do not fit.
fn split_row(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let right_width: usize = right.iter().map(|s| s.content.width()).sum();
    let room = (width as usize).saturating_sub(right_width + 1);
    let mut used = 0usize;
    let mut spans = Vec::new();
    for span in left {
        let w = span.content.width();
        if used + w <= room {
            used += w;
            spans.push(span);
            continue;
        }
        let mut cut = String::new();
        for c in span.content.chars() {
            let cw = c.width().unwrap_or(0);
            if used + cw + 1 > room {
                break;
            }
            used += cw;
            cut.push(c);
        }
        cut.push('…');
        used += 1;
        spans.push(Span::styled(cut, span.style));
        break;
    }
    spans.push(Span::raw(
        " ".repeat((width as usize).saturating_sub(used + right_width)),
    ));
    spans.extend(right);
    Line::from(spans)
}

/// Activity row above the input: spinner, current step or tool, and how to interrupt.
fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 {
        return;
    }
    let dim = Style::new().add_modifier(Modifier::DIM);
    let left = if app.is_working() || app.modal.is_some() {
        footer_status_left(app)
    } else {
        String::new()
    };
    let right = match flash(app) {
        Some(text) => format!("{text} "),
        None if app.modal.is_some() => String::new(),
        None => "Ctrl-C to cancel ".into(),
    };
    let left = vec![Span::styled(left, Style::new().fg(Color::Cyan))];
    let mut line = split_row(left, vec![Span::styled(right, dim)], area.width);
    if !app.is_working() {
        line = line.style(dim);
    }
    frame.render_widget(Paragraph::new(line), area);
}

/// Session row under the input: profile, provider and model on the left; background processes,
/// session tokens and the folder on the right.
/// `connector · model · effort LEVEL` split into its parts.
fn label_parts(label: &str) -> (&str, &str, Option<&str>) {
    let mut parts = label.split(" · ");
    let provider = parts.next().unwrap_or(label);
    let model = parts.next().unwrap_or("");
    let effort = parts.find_map(|p| p.strip_prefix("effort "));
    (provider, model, effort)
}

/// Remaining context as a small meter, green → yellow → red as it fills.
/// "96% context left" as Codex shows it: dim until it runs low, no glyph meter (block glyphs
/// fall back to odd shapes in many terminal fonts).
fn context_meter(left: u64) -> Vec<Span<'static>> {
    let style = match left {
        25.. => Style::new().add_modifier(Modifier::DIM),
        10..25 => Style::new().fg(Color::Yellow),
        _ => Style::new().fg(Color::Red),
    };
    vec![Span::styled(format!("{left}% context left"), style)]
}

/// Session row under the input. Left: profile, provider, model, effort. Right: the
/// `/statusline` items; when space runs out the last ones go first, then the model is cut.
fn draw_info(frame: &mut Frame, area: Rect, app: &App) {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let mut right: Vec<Vec<Span<'static>>> = Vec::new();
    for item in &app.statusline {
        let spans = match item.as_str() {
            "processes" if app.bg_processes > 0 => vec![Span::styled(
                format!("{} background", app.bg_processes),
                dim,
            )],
            "context" => match app.context_left() {
                Some(left) => context_meter(left),
                None => continue,
            },
            "tokens" if app.total_in + app.total_out > 0 => vec![Span::styled(
                format!(
                    "{} in · {} out",
                    super::app::tokens(app.total_in),
                    super::app::tokens(app.total_out)
                ),
                dim,
            )],
            "branch" => match &app.branch {
                Some(branch) => vec![Span::styled(branch.clone(), dim)],
                None => continue,
            },
            "folder" => vec![Span::styled(
                short_path(&std::env::current_dir().unwrap_or_default()),
                dim,
            )],
            _ => continue,
        };
        right.push(spans);
    }
    let (provider, model, effort) = label_parts(&app.provider);
    let mut left = vec![
        Span::raw(" "),
        Span::styled(
            app.profile.name().to_string(),
            Style::new().bold().fg(profile_color(app.profile)),
        ),
        Span::styled(format!(" · {provider} · "), dim),
        Span::styled(model.to_string(), dim),
    ];
    if let Some(effort) = effort {
        left.push(Span::styled(format!(" · {effort}"), dim));
    }
    let width_of = |spans: &[Span]| spans.iter().map(|s| s.content.width()).sum::<usize>();
    let left_width = width_of(&left);
    let gap = 3;
    let joined = |right: &[Vec<Span<'static>>]| -> Vec<Span<'static>> {
        let mut out = Vec::new();
        for (i, item) in right.iter().enumerate() {
            if i > 0 {
                out.push(Span::styled(" · ", dim));
            }
            out.extend(item.iter().cloned());
        }
        out.push(Span::raw(" "));
        out
    };
    while right.len() > 1 && left_width + width_of(&joined(&right)) + gap > area.width as usize {
        right.pop();
    }
    frame.render_widget(
        Paragraph::new(split_row(left, joined(&right), area.width)),
        area,
    );
}

fn centered(area: Rect, pct_w: u16, pct_h: u16) -> Rect {
    let w = (area.width * pct_w / 100).clamp(30.min(area.width), area.width);
    let h = (area.height * pct_h / 100).clamp(8.min(area.height), area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn diff_lines(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .map(|l| {
            let style = if l.starts_with("+++") || l.starts_with("---") {
                Style::new().bold()
            } else if l.starts_with('+') {
                Style::new().fg(Color::Green)
            } else if l.starts_with('-') {
                Style::new().fg(Color::Red)
            } else if l.starts_with("@@") {
                Style::new().fg(Color::Cyan)
            } else if l.contains("[y]es") || l.contains("justification") {
                Style::new().bold().fg(Color::Yellow)
            } else {
                Style::new()
            };
            Line::styled(l.to_string(), style)
        })
        .collect()
}

/// What an approval question is about, read from the approval code's prompt text.
struct ApprovalView {
    title: Vec<Span<'static>>,
    body: Vec<Line<'static>>,
    choices: &'static [ApprovalChoice],
}

fn approval_view(modal: &Modal) -> Option<ApprovalView> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    match modal {
        Modal::Approve { text, .. } => {
            let path = text
                .lines()
                .rev()
                .find_map(|l| l.split_once("Approve edit to '")?.1.split_once("'?"))
                .map_or("file", |(path, _)| path)
                .to_string();
            let (body, added, removed) = numbered_diff(text);
            Some(ApprovalView {
                title: vec![
                    Span::styled(format!("Edit {path}"), Style::new().bold()),
                    Span::styled(format!("  +{added}"), Style::new().fg(Color::Green)),
                    Span::styled(format!(" −{removed}"), Style::new().fg(Color::Red)),
                ],
                body,
                choices: EDIT_CHOICES,
            })
        }
        Modal::Confirm { text, .. } => {
            let question = text.trim_end().trim_end_matches("[y]es / [n]o:").trim_end();
            let command = question
                .split_once("detected: '")
                .and_then(|(_, rest)| rest.rsplit_once("' ("))
                .map(|(cmd, rest)| (cmd.to_string(), rest.trim_end_matches(". Approve?")));
            let (title, body) = match command {
                Some((cmd, reason)) => {
                    let reason = reason.trim_end_matches(')').to_string();
                    let title = if reason.contains("long-running") {
                        "Start a background process?"
                    } else {
                        "Run a risky command?"
                    };
                    (
                        title.to_string(),
                        vec![
                            Line::from(vec![
                                Span::styled("$ ", dim),
                                Span::styled(cmd, Style::new().bold()),
                            ]),
                            Line::styled(reason, dim),
                        ],
                    )
                }
                None => (
                    "Confirm".to_string(),
                    question.lines().map(|l| Line::raw(l.to_string())).collect(),
                ),
            };
            Some(ApprovalView {
                title: vec![Span::styled(title, Style::new().bold())],
                body,
                choices: CONFIRM_CHOICES,
            })
        }
        _ => None,
    }
}

/// Unified diff as numbered lines (new-file numbers; old numbers for removals), without the
/// `---`/`+++` headers or the question line. Returns the lines and the added/removed counts.
fn numbered_diff(text: &str) -> (Vec<Line<'static>>, usize, usize) {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let (mut old, mut new, mut added, mut removed) = (0usize, 0usize, 0usize, 0usize);
    let mut lines = Vec::new();
    for l in text.lines() {
        if l.starts_with("---")
            || l.starts_with("+++")
            || l.starts_with('\\')
            || l.starts_with("Approve edit to '")
        {
            continue;
        }
        if let Some(hunk) = l.strip_prefix("@@ ") {
            let start = |prefix: char| {
                hunk.split_whitespace()
                    .find_map(|part| part.strip_prefix(prefix))
                    .and_then(|range| range.split(',').next()?.parse::<usize>().ok())
                    .unwrap_or(1)
            };
            old = start('-');
            new = start('+');
            if !lines.is_empty() {
                lines.push(Line::styled("     ⋮", dim));
            }
            continue;
        }
        let (sign, rest) = match l.chars().next() {
            Some(c @ ('+' | '-' | ' ')) => (c, &l[1..]),
            _ => (' ', l),
        };
        // Tinted rows as in Claude Code: dark green for additions, dark red for removals.
        let (number, sign_style, row) = match sign {
            '+' => {
                added += 1;
                new += 1;
                (
                    new - 1,
                    Style::new().fg(Color::Green),
                    Style::new().bg(Color::Rgb(18, 52, 30)),
                )
            }
            '-' => {
                removed += 1;
                old += 1;
                (
                    old - 1,
                    Style::new().fg(Color::Red),
                    Style::new().bg(Color::Rgb(66, 22, 24)),
                )
            }
            _ => {
                old += 1;
                new += 1;
                (new - 1, Style::new(), Style::new())
            }
        };
        lines.push(
            Line::from(vec![
                Span::styled(format!("{number:>5} "), dim),
                Span::styled(format!("{sign} "), sign_style),
                Span::raw(rest.to_string()),
            ])
            .style(row),
        );
    }
    (lines, added, removed)
}

/// Approval panel docked above the footer (as in Codex): title, scrollable detail, options.
fn draw_approval(frame: &mut Frame, screen: Rect, app: &mut App) {
    let Some(view) = app.modal.as_ref().and_then(approval_view) else {
        return;
    };
    let dim = Style::new().add_modifier(Modifier::DIM);
    let choices = view.choices.len() as u16;
    // Border, title, gap, body, gap, options, hint.
    let wanted = view.body.len() as u16 + choices + 5;
    let max = (screen.height.saturating_sub(2) * 3 / 4).max(10);
    // Options and hint stay visible in a tiny terminal; the detail shrinks first.
    let floor = choices + 3;
    let height = wanted
        .clamp(floor, max.max(floor))
        .min(screen.height.saturating_sub(1));
    let area = Rect::new(
        screen.x,
        screen.bottom().saturating_sub(height + 1),
        screen.width,
        height,
    );
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [title, _, body, _, options, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(choices),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(Line::from(view.title)), title);

    let detail = Paragraph::new(view.body).wrap(Wrap { trim: false });
    let max_scroll = detail
        .line_count(body.width)
        .saturating_sub(body.height as usize)
        .min(u16::MAX as usize) as u16;
    let scroll = match &mut app.modal {
        Some(Modal::Approve { scroll, .. }) | Some(Modal::Confirm { scroll, .. }) => {
            *scroll = (*scroll).min(max_scroll);
            *scroll
        }
        _ => 0,
    };
    frame.render_widget(detail.scroll((scroll, 0)), body);

    let selected = app.approval_choice.min(view.choices.len() - 1);
    let rows: Vec<Line> = view
        .choices
        .iter()
        .enumerate()
        .map(|(i, choice)| {
            let chosen = i == selected;
            let style = if chosen {
                Style::new().bold().fg(Color::Cyan)
            } else {
                Style::new()
            };
            Line::from(vec![
                Span::styled(if chosen { "› " } else { "  " }, style),
                Span::styled(format!("{}. {}", i + 1, choice.label), style),
                Span::styled(format!(" ({})", choice.key), dim),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), options);
    let more = if max_scroll > 0 {
        " · PgUp/PgDn scroll"
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!("↑↓ select · Enter confirm · Esc reject{more}"),
            dim,
        )),
        hint,
    );
}

/// Picker docked at the bottom like the approval panel (Codex style): title with the current
/// value, filter line, a window of options with the current one ticked, and key hints.
fn draw_picker(frame: &mut Frame, screen: Rect, app: &App) {
    let Some(
        modal @ Modal::Picker {
            kind,
            filter,
            selected,
            loading,
            ..
        },
    ) = &app.modal
    else {
        return;
    };
    let dim = Style::new().add_modifier(Modifier::DIM);
    let (title, noun) = match kind {
        PickKind::Model => ("Select model", "models"),
        PickKind::Provider => ("Select provider", "providers"),
        PickKind::Effort => ("Reasoning effort", "levels"),
        PickKind::Command => ("Commands", "commands"),
        PickKind::Conversation => ("Resume conversation", "conversations"),
    };
    let current = app.picker_current();
    let shown = modal.visible();
    const MAX_ROWS: usize = 12;
    let list_rows = shown.len().clamp(1, MAX_ROWS) as u16;
    // Border, title, filter, gap, list, gap, hint.
    let height = (list_rows + 6).min(screen.height.saturating_sub(1));
    let area = Rect::new(
        screen.x,
        screen.bottom().saturating_sub(height + 1),
        screen.width,
        height,
    );
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(Color::Cyan))
        .padding(ratatui::widgets::Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [head, filter_row, _, list, _, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let mut title_spans = vec![Span::styled(title, Style::new().bold())];
    if let Some(current) = &current {
        title_spans.push(Span::styled(format!("   current: {current}"), dim));
    }
    frame.render_widget(Paragraph::new(Line::from(title_spans)), head);
    let filter_line = if filter.is_empty() {
        Line::from(vec![
            Span::styled("⌕ ", Style::new().fg(Color::Cyan)),
            Span::styled(format!("Type to filter {noun}"), dim),
        ])
    } else {
        Line::from(vec![
            Span::styled("⌕ ", Style::new().fg(Color::Cyan)),
            Span::raw(filter.clone()),
            Span::styled("▏", Style::new().fg(Color::Cyan)),
        ])
    };
    frame.render_widget(Paragraph::new(filter_line), filter_row);
    let rows: Vec<Line> = if *loading {
        vec![Line::styled(
            format!("Loading {noun} from the provider…"),
            dim,
        )]
    } else if shown.is_empty() {
        vec![Line::styled(
            match kind {
                PickKind::Model if !filter.trim().is_empty() => {
                    format!("No match. Enter uses '{}' as the model id.", filter.trim())
                }
                _ => format!("No {noun} match."),
            },
            Style::new().fg(Color::Yellow),
        )]
    } else {
        let height = list.height as usize;
        let start = selected
            .saturating_sub(height / 2)
            .min(shown.len().saturating_sub(height));
        shown
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(i, item)| {
                let chosen = i == *selected;
                let style = if chosen {
                    Style::new().fg(Color::Cyan).bold()
                } else {
                    Style::new()
                };
                let mut spans = vec![
                    Span::styled(if chosen { "› " } else { "  " }, style),
                    Span::styled(item.to_string(), style),
                ];
                if current.as_deref() == Some(*item) {
                    spans.push(Span::styled("  ✓ current", Style::new().fg(Color::Green)));
                }
                if kind == &PickKind::Command {
                    spans.push(Span::styled(
                        format!("  {}", super::app::command_about(item)),
                        dim,
                    ));
                }
                Line::from(spans)
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(rows), list);
    let position = if shown.is_empty() {
        String::new()
    } else {
        format!("{}/{} · ", selected + 1, shown.len())
    };
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!("{position}↑↓ move · Enter select · Esc cancel"),
            dim,
        )),
        hint,
    );
}

fn draw_modal(frame: &mut Frame, screen: Rect, app: &mut App) {
    let Some(modal) = &app.modal else { return };
    if matches!(modal, Modal::Approve { .. } | Modal::Confirm { .. }) {
        draw_approval(frame, screen, app);
        return;
    }
    if matches!(modal, Modal::Picker { .. }) {
        draw_picker(frame, screen, app);
        return;
    }
    let area = centered(screen, 90, 75);
    frame.render_widget(Clear, area);
    let (title, body, hint, scroll): (&str, Vec<Line>, &str, u16) = match modal {
        Modal::Approve { .. } | Modal::Confirm { .. } => return,
        Modal::Text { text, input } => {
            let mut lines = diff_lines(text);
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled("> ", Style::new().bold().fg(Color::Cyan)),
                Span::raw(input.clone()),
                Span::styled("▏", Style::new().fg(Color::Cyan)),
            ]));
            (" Input needed ", lines, " Enter: submit   Esc: reject ", 0)
        }
        Modal::Secret { name, input } => {
            let label = name.as_deref().unwrap_or("the configured provider");
            let lines = vec![
                Line::raw(format!("Paste or type the API key for {label}.")),
                Line::raw(
                    "It is hidden here and stored in your user config directory, not in the repo.",
                ),
                Line::raw(""),
                Line::from(vec![
                    Span::styled("key: ", Style::new().bold()),
                    Span::raw("•".repeat(input.chars().count())),
                    Span::styled("▏", Style::new().fg(Color::Cyan)),
                ]),
            ];
            (" Connect ", lines, " Enter: store   Esc: cancel ", 0)
        }
        Modal::ConfirmRollback { step } => {
            let what = match step {
                Some(n) => format!("Restore files to the state before step {n}?"),
                None => "Restore all files changed in the latest run?".to_string(),
            };
            (
                " Rollback ",
                vec![Line::raw(what)],
                " y: restore   n/Esc: cancel ",
                0,
            )
        }
        Modal::Picker { .. } => return,
        Modal::Help => (
            " Help ",
            help_text()
                .lines()
                .map(|l| Line::raw(l.to_string()))
                .collect(),
            " any key: close ",
            0,
        ),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
        .padding(ratatui::widgets::Padding::new(2, 2, 1, 0))
        .title(Line::styled(title, Style::new().bold().fg(Color::Cyan)))
        .title_bottom(Line::styled(hint, Style::new().add_modifier(Modifier::DIM)));
    let paragraph = Paragraph::new(body).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph.scroll((scroll, 0)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::worker::Ev;
    use ratatui::{backend::TestBackend, Terminal};

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn long_approve_diff(body_lines: usize) -> String {
        let mut text = String::from("--- a/big.rs\n+++ b/big.rs\n");
        for i in 0..body_lines {
            text.push_str(&format!("+line-{i:04}\n"));
        }
        text.push_str("Approve edit to 'big.rs'? [y]es / [a]lways / [n]o: ");
        text
    }

    fn footer_row(screen: &str) -> &str {
        screen.lines().last().expect("screen has rows")
    }

    #[test]
    fn status_strip_idle_shows_session_tokens() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.total_in = 12_345;
        app.total_out = 678;
        let left = footer_status_left(&app);
        assert!(left.contains("ready"));
        assert!(left.contains("session"));
        assert!(left.contains("12345"));
        assert!(left.contains("678"));
        assert!(!left.contains("step"));
        assert!(!left.contains("blocked"));
        let screen = render(&mut app, 80, 20);
        assert!(
            footer_row(&screen).contains("12.3k in · 678 out"),
            "{screen}"
        );
    }

    #[test]
    fn status_strip_working_shows_step_and_run_tokens() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.status = Status::Working { step: 4 };
        app.run_in = 100;
        app.run_out = 50;
        app.tick = 0;
        let left = footer_status_left(&app);
        assert!(left.contains("step 4"));
        assert!(left.contains("100 in"));
        assert!(left.contains("50 out"));
        assert!(left.contains(SPINNER[0]));
        assert!(!left.contains("ready"));
        assert!(!left.contains("blocked"));
    }

    #[test]
    fn status_strip_blocked_when_modal_open_even_if_working() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.status = Status::Working { step: 2 };
        app.run_in = 999;
        app.modal = Some(Modal::Approve {
            text: "Approve?".into(),
            scroll: 0,
        });
        let left = footer_status_left(&app);
        assert!(left.contains("blocked"));
        assert!(left.contains("awaiting approval"));
        assert!(!left.contains("step 2"));
        assert!(!left.contains("999"));
    }

    #[test]
    fn status_strip_labels_differ_across_idle_working_blocked() {
        let idle = footer_status_left(&App::new(Profile::Build, "p".into(), None));
        let mut working = App::new(Profile::Build, "p".into(), None);
        working.status = Status::Working { step: 1 };
        working.tick = 0;
        let working = footer_status_left(&working);
        let mut blocked = App::new(Profile::Build, "p".into(), None);
        blocked.modal = Some(Modal::Confirm {
            text: "ok?".into(),
            scroll: 0,
        });
        let blocked = footer_status_left(&blocked);
        assert_ne!(idle, working);
        assert_ne!(working, blocked);
        assert_ne!(idle, blocked);
        assert!(idle.contains("ready"));
        assert!(working.contains("step"));
        assert!(blocked.contains("blocked"));
    }

    #[test]
    fn idle_screen_shows_model_under_the_input() {
        let mut app = App::new(Profile::Build, "nim · model-x".into(), None);
        let screen = render(&mut app, 80, 20);
        let info = footer_row(&screen);
        assert!(info.contains("build · nim · model-x"), "{screen}");
        assert!(screen.contains("model     model-x"), "{screen}");
        assert!(screen.contains("provider  nim"), "{screen}");
        assert!(
            screen.contains("› Ask anything · / for commands"),
            "{screen}"
        );
        assert!(
            !screen.contains("Ctrl-C to cancel"),
            "no status row while idle"
        );
    }

    #[test]
    fn diff_lines_keep_distinct_hunk_styles() {
        let lines = diff_lines("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-removed\n+added\n context");
        let style = |i: usize| ratatui::style::Styled::style(&lines[i]);
        assert!(style(0).add_modifier.contains(Modifier::BOLD));
        assert!(style(1).add_modifier.contains(Modifier::BOLD));
        assert_eq!(style(2).fg, Some(Color::Cyan));
        assert_eq!(style(3).fg, Some(Color::Red));
        assert_eq!(style(4).fg, Some(Color::Green));
        assert_eq!(style(5).fg, None);
    }

    #[test]
    fn approve_modal_long_diff_viewports() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.modal = Some(Modal::Approve {
            text: long_approve_diff(80),
            scroll: 0,
        });
        let top = render(&mut app, 80, 24);
        assert!(top.contains("line-0000"));
        assert!(!top.contains("line-0079"));

        if let Some(Modal::Approve { scroll: s, .. }) = &app.modal {
            assert_eq!(*s, 0);
        } else {
            panic!("expected approve modal");
        }

        app.modal = Some(Modal::Approve {
            text: long_approve_diff(80),
            scroll: 60,
        });
        let mid = render(&mut app, 80, 24);
        assert!(!mid.contains("line-0000"));
        assert!(
            mid.contains("line-0060") || mid.contains("line-0059") || mid.contains("line-0061")
        );
    }

    #[test]
    fn approve_modal_scroll_keys_shift_visible_lines() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.on_event(Ev::PromptText(long_approve_diff(80)));
        app.on_event(Ev::NeedInput);
        let before = render(&mut app, 80, 24);
        assert!(before.contains("line-0000"));
        app.on_key(KeyEvent::new_with_kind(
            KeyCode::PageDown,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        ));
        let after = render(&mut app, 80, 24);
        assert!(!after.contains("line-0000"));
        assert!(after.contains("line-00"));
    }

    #[test]
    fn approve_modal_scroll_clamped_on_draw() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.modal = Some(Modal::Approve {
            text: long_approve_diff(80),
            scroll: u16::MAX,
        });
        render(&mut app, 80, 24);
        let Modal::Approve { scroll, .. } = app.modal.clone().expect("modal") else {
            panic!("expected approve modal");
        };
        assert!(scroll < u16::MAX);
        let screen = render(&mut app, 80, 24);
        assert!(screen.contains("line-0079"), "{screen}");
    }

    #[test]
    fn approve_modal_shows_diff() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.on_event(Ev::PromptText(
            "--- a/calc.py\n+++ b/calc.py\n-return a - b\n+return a + b\nApprove edit to 'calc.py'? [y]es / [a]lways / [n]o: ".into(),
        ));
        app.on_event(Ev::NeedInput);
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("Edit calc.py  +1 −1"), "{screen}");
        assert!(screen.contains("+ return a + b"), "{screen}");
        assert!(!screen.contains("[y]es"), "{screen}");
        assert!(screen.contains("› 1. Yes, apply this edit (y)"), "{screen}");
        assert!(screen.contains("3. No, and tell DevCode what to do instead (esc)"));
    }

    #[test]
    fn secret_modal_never_renders_the_key() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.modal = Some(Modal::Secret {
            name: Some("gemini".into()),
            input: "AIzaSECRETKEY".into(),
        });
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("gemini"));
        assert!(!screen.contains("AIzaSECRETKEY"));
        assert!(screen.contains("•••••••••••••"));
    }

    #[test]
    fn model_picker_renders_list_and_selection() {
        let mut app = App::new(Profile::Build, "gemini · m".into(), None);
        let modal = Modal::Picker {
            kind: PickKind::Model,
            items: (0..40).map(|i| format!("model-{i}")).collect(),
            filter: "model-3".into(),
            selected: 2,
            loading: false,
        };
        let total = modal.visible().len();
        app.modal = Some(modal);
        let screen = render(&mut app, 80, 24);
        assert!(screen.contains("Select model"));
        assert!(screen.contains("model-3"));
        assert!(screen.contains(&format!("3/{total} · ↑↓ move")), "{screen}");
        assert!(!screen.contains("model-4 "));
    }

    #[test]
    fn model_picker_opens_on_the_current_model_and_ticks_it() {
        let mut app = App::new(Profile::Build, "nim · b/two · effort high".into(), None);
        app.modal = Some(Modal::Picker {
            kind: PickKind::Model,
            items: vec!["a/one".into(), "b/two".into(), "c/three".into()],
            filter: String::new(),
            selected: 0,
            loading: false,
        });
        assert_eq!(app.picker_current().as_deref(), Some("b/two"));
        let screen = render(&mut app, 80, 24);
        assert!(screen.contains("current: b/two"), "{screen}");
        assert!(screen.contains("b/two  ✓ current"), "{screen}");
        assert!(screen.contains("⌕ Type to filter models"), "{screen}");
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.input = "héllo 世界 wide".into();
        app.cursor = app.input.chars().count();
        for (w, h) in [(1, 1), (2, 2), (5, 3), (20, 6), (40, 8)] {
            render(&mut app, w, h);
            app.modal = Some(Modal::Help);
            render(&mut app, w, h);
            app.modal = None;
        }
    }

    fn sample_tool_items() -> Vec<Item> {
        vec![
            Item::Tool {
                name: "read_file".into(),
                summary: "src/main.rs".into(),
            },
            Item::ToolResult {
                ok: true,
                preview: "line-one-visible\nline-two-hidden\nline-three-hidden".into(),
                diff: None,
            },
        ]
    }

    #[test]
    fn tool_card_collapsed_hides_extra_preview_lines() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.items.extend(sample_tool_items());
        app.expanded_tool = None;
        let screen = render(&mut app, 80, 20);
        assert!(screen.contains("● Explored 1 file (main.rs)"), "{screen}");
        assert!(!screen.contains("line-one-visible"));
        assert!(!screen.contains("line-two-hidden"));
        assert!(!screen.contains("line-three-hidden"));
    }

    #[test]
    fn tool_card_expanded_shows_multiline_preview() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.items.extend(sample_tool_items());
        app.expanded_tool = Some(1);
        let screen = render(&mut app, 80, 24);
        assert!(screen.contains("line-one-visible"));
        assert!(screen.contains("line-two-hidden"));
        assert!(screen.contains("line-three-hidden"));
    }

    #[test]
    fn long_transcript_follows_bottom_and_scrolls() {
        let mut app = App::new(Profile::Ask, "p".into(), None);
        for i in 0..100 {
            app.items.push(Item::Info(format!("line number {i}")));
        }
        let bottom = render(&mut app, 80, 20);
        assert!(bottom.contains("line number 99"));
        app.scroll = 30;
        let up = render(&mut app, 80, 20);
        assert!(!up.contains("line number 99"));
        assert!(up.contains("line number 6"));
    }

    #[test]
    fn input_area_height_grows_with_line_count() {
        let w = 80;
        assert_eq!(input_area_height("", w), 3);
        assert_eq!(input_area_height("one", w), 3);
        assert_eq!(input_area_height("a\nb", w), 4);
        assert_eq!(input_area_height("a\nb\nc", w), 5);
    }

    #[test]
    fn long_single_line_expands_input_height_when_wrapped() {
        let long = "a".repeat(200);
        assert!(input_area_height(&long, 80) > 3);
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.input = long;
        app.cursor = 200;
        render(&mut app, 80, 24);
    }

    #[test]
    fn exact_width_input_stays_visible() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.input = "abcdefgh".into();
        app.cursor = 8;
        assert_eq!(layout_input(&app.input, app.cursor, 8).1, (0, 7));
        assert!(render(&mut app, 14, 10).contains("› abcdefgh"));
    }

    #[test]
    fn input_area_height_capped_at_eight_rows() {
        let many = (0..12)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(input_area_height(&many, 80), 8);
    }

    #[test]
    fn multiline_input_renders_without_panic() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.input = (0..6)
            .map(|i| format!("row{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.cursor = app.input.chars().count();
        render(&mut app, 80, 24);
    }

    #[test]
    fn reasoning_item_renders_distinct_prefix() {
        let mut app = App::new(Profile::Ask, "p".into(), None);
        app.items.push(Item::Reasoning("thinking line".into()));
        assert!(!render(&mut app, 80, 20).contains("thinking line"));
        app.show_thinking = true;
        let screen = render(&mut app, 80, 20);
        assert!(screen.contains("· thinking line"));
    }

    #[test]
    fn input_layout_counts_wide_characters_as_two_columns() {
        let (rows, pos) = layout_input("日本語x", 4, 4);
        assert_eq!(rows, vec!["日本".to_string(), "語x".to_string()]);
        assert_eq!(pos, (1, 3));
        let (_, mid) = layout_input("日本語x", 1, 4);
        assert_eq!(mid, (0, 2));
        // Border, padding and the `› ` prompt take six of the ten columns.
        assert_eq!(wrapped_input_line_count("日本語x", 10), 2);
    }

    #[test]
    fn footer_shows_thinking_label_while_reasoning_is_hidden() {
        let mut app = App::new(Profile::Ask, "p".into(), None);
        app.status = Status::Working { step: 2 };
        app.thinking_since = Some(std::time::Instant::now());
        assert!(footer_status_left(&app).contains("thinking 0s · step 2"));
        app.show_thinking = true;
        assert!(!footer_status_left(&app).contains("thinking"));
    }

    #[test]
    fn mouse_drag_selects_and_copies_drawn_text() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = App::new(Profile::Ask, "p".into(), None);
        app.items.clear();
        app.items.push(Item::Assistant("hello 世界 world".into()));
        let screen = render(&mut app, 40, 12);
        let (row, line) = screen
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains("hello"))
            .unwrap();
        let col = line.find("hello").unwrap() as u16;
        let row = row as u16;
        let mouse = |kind, column| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), col));
        // "hello " is 6 cells, the two CJK characters 4 more, then " wor".
        app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), col + 13));
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), col + 13));
        render(&mut app, 40, 12);
        assert_eq!(app.copied.as_deref(), Some("hello 世界 wor"));
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), col));
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), col));
        assert!(app.selection.is_none());
    }

    #[test]
    fn long_model_name_is_cut_before_the_right_side() {
        let mut app = App::new(
            Profile::Build,
            "nim · nvidia/nemotron-3.5-super-vl-120b-a12b · effort high".into(),
            None,
        );
        app.total_in = 129_417;
        app.total_out = 2_140;
        let screen = render(&mut app, 60, 12);
        let info = footer_row(&screen);
        assert!(info.contains("129.4k in · 2.1k out"), "{info}");
        assert!(info.contains('…'), "{info}");
        assert_eq!(info.width(), 60, "{info}");
    }

    #[test]
    fn edit_card_shows_numbered_diff_with_counts() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.items.push(Item::Tool {
            name: "edit_file".into(),
            summary: "calc.py".into(),
        });
        app.items.push(Item::ToolResult {
            ok: true,
            preview: "Updated calc.py".into(),
            diff: Some("--- a/calc.py\n+++ b/calc.py\n@@ -3,3 +3,3 @@\n def add(a, b):\n-    return a - b\n+    return a + b\n \n".into()),
        });
        let collapsed = render(&mut app, 80, 24);
        assert!(
            collapsed.contains("● Edited 1 file (calc.py)  +1 −1"),
            "{collapsed}"
        );
        assert!(!collapsed.contains("return a - b"), "diff waits for Ctrl-T");
        app.expanded_tool = Some(1);
        let screen = render(&mut app, 80, 24);
        assert!(
            screen.contains("⎿  Updated calc.py with 1 addition and 1 removal"),
            "{screen}"
        );
        assert!(screen.contains("4 -     return a - b"), "{screen}");
        assert!(screen.contains("4 +     return a + b"), "{screen}");
        assert!(!screen.contains("+++"), "{screen}");
    }

    #[test]
    fn drag_past_the_top_edge_scrolls_and_copies_rows_off_screen() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = App::new(Profile::Ask, "p".into(), None);
        app.items.clear();
        let text: Vec<String> = (0..40).map(|i| format!("row-{i:02}")).collect();
        app.items.push(Item::Assistant(text.join("\n\n")));
        let screen = render(&mut app, 40, 14);
        let (row, line) = screen
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains("row-"))
            .last()
            .unwrap();
        let last_visible = line.trim().to_string();
        let col = line.find("row-").unwrap() as u16 + 6;
        let mouse = |kind, row| MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), row as u16));
        // Hold the pointer on the top edge: every frame scrolls one row.
        app.on_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 0));
        for _ in 0..6 {
            app.on_tick();
            render(&mut app, 40, 14);
        }
        assert!(app.scroll > 0, "the transcript scrolled up");
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 0));
        render(&mut app, 40, 14);
        let copied = app.copied.clone().expect("copied");
        assert!(copied.ends_with(&last_visible), "{copied}");
        assert!(
            copied.lines().count() > 12,
            "rows that scrolled away are copied: {copied}"
        );
    }

    #[test]
    fn info_row_shows_context_and_branch_and_respects_statusline() {
        let mut app = App::new(Profile::Build, "nim · m".into(), None);
        app.context_window = Some(100_000);
        app.context_used = 25_000;
        app.branch = Some("feat/tui".into());
        let info = footer_row(&render(&mut app, 120, 12)).to_string();
        assert!(info.contains("75% context left"), "{info}");
        assert!(info.contains("feat/tui"), "{info}");
        app.statusline.retain(|s| s != "branch");
        let info = footer_row(&render(&mut app, 120, 12)).to_string();
        assert!(!info.contains("feat/tui"), "{info}");
    }

    #[test]
    fn slash_command_output_nests_under_the_command() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.items.clear();
        app.items.push(Item::Command("/connections".into()));
        app.items.push(Item::Info("● nim  stored  · active".into()));
        app.items.push(Item::Info("○ gemini  stored".into()));
        app.items.push(Item::Info("edit approved".into()));
        app.items.insert(3, Item::Note("x".into()));
        let screen = render(&mut app, 60, 12);
        assert!(screen.contains("› /connections"), "{screen}");
        assert!(screen.contains("  ⎿  ● nim  stored  · active"), "{screen}");
        assert!(screen.contains("     ○ gemini  stored"), "{screen}");
        assert!(screen.contains("  · edit approved"), "{screen}");
    }

    #[test]
    fn consecutive_reads_and_commands_collapse_into_summary_lines() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.items.clear();
        let call = |name: &str, summary: &str| Item::Tool {
            name: name.into(),
            summary: summary.into(),
        };
        let done = |ok| Item::ToolResult {
            ok,
            preview: "x".into(),
            diff: None,
        };
        app.items.extend([
            call("read_file", "src/tui/app.rs"),
            done(true),
            call("grep", "fn draw"),
            done(true),
            call("read_file", "src/tui/view.rs"),
            done(true),
            call("run_command", "ls -la docs"),
            done(true),
            call("run_command", "git status"),
            done(false),
            call("read_file", "TODO.md"),
        ]);
        let screen = render(&mut app, 100, 20);
        assert!(
            screen.contains("● Explored 2 files, 1 search (app.rs, fn draw, view.rs)"),
            "{screen}"
        );
        assert!(
            screen.contains("● Ran 2 commands (ls -la docs · git status)  1 failed"),
            "{screen}"
        );
        assert!(screen.contains("● Exploring 1 file (TODO.md)"), "{screen}");
        app.expanded_tool = Some(6);
        let open = render(&mut app, 100, 30);
        assert!(open.contains("run_command  ls -la docs"), "{open}");
    }
}
