//! Rendering. Reads `App`; the only state it writes back is the scroll clamp.
use super::app::{help_text, App, Item, Modal, PickKind, Status};
use crate::Profile;
use ratatui::{
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const INPUT_MIN_HEIGHT: u16 = 3;
const INPUT_MAX_HEIGHT: u16 = 8;

fn wrap_line_count(line: &str, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    let len = line.chars().count();
    if len == 0 {
        1
    } else {
        len.div_ceil(width)
    }
}

/// Visual line count for the input pane (soft-wrap at `width` minus borders).
pub(crate) fn wrapped_input_line_count(input: &str, area_width: u16) -> usize {
    let inner = area_width.saturating_sub(2).max(1) as usize;
    if input.is_empty() {
        return 1;
    }
    input
        .split('\n')
        .map(|line| wrap_line_count(line, inner))
        .sum()
}

/// Total input widget height (border + text lines), capped for layout.
pub(crate) fn input_area_height(input: &str, area_width: u16) -> u16 {
    let lines = wrapped_input_line_count(input, area_width);
    (lines as u16 + 2).clamp(INPUT_MIN_HEIGHT, INPUT_MAX_HEIGHT)
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let [header, body, input, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(input_area_height(&app.input, area.width)),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(frame, header, app);
    draw_transcript(frame, body, app);
    draw_input(frame, input, app);
    draw_footer(frame, footer, app);
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

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let cwd = short_path(&std::env::current_dir().unwrap_or_default());
    let line = Line::from(vec![
        Span::styled(
            " DevCode ",
            Style::new().bold().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(" "),
        Span::styled(
            format!(" {} ", app.profile.name()),
            Style::new()
                .bold()
                .fg(Color::Black)
                .bg(profile_color(app.profile)),
        ),
        Span::raw("  "),
        Span::styled(app.provider.clone(), Style::new().fg(Color::Gray)),
        Span::raw("  "),
        Span::styled(cwd, Style::new().add_modifier(Modifier::DIM)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn push_tool_result_lines(
    lines: &mut Vec<Line<'static>>,
    ok: bool,
    preview: &str,
    expanded: bool,
    border: Style,
    dim: Style,
) {
    let (mark, color) = if ok {
        ("✓", Color::Green)
    } else {
        ("✗", Color::Red)
    };
    let preview_lines: Vec<&str> = preview.lines().collect();
    if preview_lines.is_empty() {
        lines.push(Line::from(vec![Span::styled("  └", border)]));
        return;
    }
    if expanded {
        for (j, text) in preview_lines.iter().enumerate() {
            let lead = if j == 0 {
                Span::styled(format!("  │ {mark} "), Style::new().fg(color))
            } else {
                Span::raw("  │     ")
            };
            lines.push(Line::from(vec![
                lead,
                Span::styled((*text).to_string(), dim),
            ]));
        }
        lines.push(Line::from(vec![Span::styled("  └", border)]));
        return;
    }
    let suffix = if preview_lines.len() > 1 { " …" } else { "" };
    lines.push(Line::from(vec![
        Span::styled("  │ ", border),
        Span::styled(format!("{mark} "), Style::new().fg(color)),
        Span::styled(format!("{}{suffix}", preview_lines[0]), dim),
    ]));
    lines.push(Line::from(vec![Span::styled("  └", border)]));
}

fn transcript_lines(items: &[Item], expanded_tool: Option<usize>) -> Vec<Line<'static>> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let border = Style::new().fg(Color::DarkGray);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            Item::User(text) => {
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![
                    Span::styled("› ", Style::new().bold().fg(Color::Cyan)),
                    Span::styled(text.clone(), Style::new().bold()),
                ]));
            }
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
            Item::Tool { name, summary } => {
                let expanded = expanded_tool == Some(i);
                let toggle = if expanded { "  ▾" } else { "  ▸" };
                lines.push(Line::from(vec![
                    Span::styled("  ┌ ", border),
                    Span::styled("⚙ ", dim),
                    Span::styled(name.clone(), Style::new().fg(Color::Blue)),
                    Span::styled(format!("  {summary}"), dim),
                    Span::styled(toggle, dim),
                ]));
                if let Some(Item::ToolResult { ok, preview }) = items.get(i + 1) {
                    push_tool_result_lines(&mut lines, *ok, preview, expanded, border, dim);
                    i += 2;
                    continue;
                }
                lines.push(Line::from(vec![Span::styled("  └", border)]));
            }
            Item::ToolResult { ok, preview } => {
                let (mark, color) = if *ok {
                    ("✓", Color::Green)
                } else {
                    ("✗", Color::Red)
                };
                for (j, text) in preview.lines().enumerate() {
                    let lead = if j == 0 {
                        Span::styled(format!("    {mark} "), Style::new().fg(color))
                    } else {
                        Span::raw("      ")
                    };
                    lines.push(Line::from(vec![lead, Span::styled(text.to_string(), dim)]));
                }
            }
            Item::Info(text) => lines.push(Line::styled(
                format!("  {text}"),
                Style::new().fg(Color::Yellow),
            )),
            Item::Error(text) => lines.push(Line::styled(
                format!("  ✗ {text}"),
                Style::new().bold().fg(Color::Red),
            )),
            Item::Note(text) => lines.push(Line::styled(format!("  {text}"), dim)),
        }
        i += 1;
    }
    lines
}

fn draw_transcript(frame: &mut Frame, area: Rect, app: &mut App) {
    let paragraph =
        Paragraph::new(transcript_lines(&app.items, app.expanded_tool)).wrap(Wrap { trim: false });
    let total = paragraph.line_count(area.width);
    let max_scroll = total.saturating_sub(area.height as usize);
    app.scroll = app.scroll.min(max_scroll);
    let top = (max_scroll - app.scroll).min(u16::MAX as usize) as u16;
    frame.render_widget(paragraph.scroll((top, 0)), area);
}

fn draw_input(frame: &mut Frame, area: Rect, app: &App) {
    let busy = app.is_working();
    let title = if busy { " working… " } else { " message " };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(if busy { Color::DarkGray } else { Color::Cyan }))
        .title(title);
    let inner_width = area.width.saturating_sub(2).max(1) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;
    let display_lines = wrapped_display_lines(&app.input, inner_width);
    let (cursor_display_line, cursor_display_col) =
        wrapped_cursor_pos(&app.input, app.cursor, inner_width);

    let scroll_line = cursor_display_line.saturating_sub(inner_height.saturating_sub(1));
    let visible: Vec<Line> = display_lines
        .iter()
        .skip(scroll_line)
        .take(inner_height)
        .map(|s| Line::raw(s.as_str()))
        .collect();

    frame.render_widget(
        Paragraph::new(visible)
            .wrap(Wrap { trim: false })
            .block(block),
        area,
    );
    if app.modal.is_none() {
        let on_screen_line = cursor_display_line.saturating_sub(scroll_line);
        let x = (cursor_display_col as u16).min(area.width.saturating_sub(2));
        let y = area.y + 1 + on_screen_line as u16;
        frame.set_cursor_position(Position::new(area.x + 1 + x, y));
    }
}

fn wrapped_display_lines(input: &str, width: usize) -> Vec<String> {
    if input.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    for physical in input.split('\n') {
        let chars: Vec<char> = physical.chars().collect();
        if chars.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut i = 0;
        while i < chars.len() {
            let end = (i + width).min(chars.len());
            out.push(chars[i..end].iter().collect());
            i = end;
        }
    }
    out
}

fn wrapped_cursor_pos(input: &str, cursor: usize, width: usize) -> (usize, usize) {
    if input.is_empty() {
        return (0, 0);
    }
    let mut display_line = 0usize;
    let mut char_idx = 0usize;
    for physical in input.split('\n') {
        let line_len = physical.chars().count();
        let line_end = char_idx + line_len;
        if cursor <= line_end {
            let col_on_line = cursor.saturating_sub(char_idx);
            if width == 0 {
                return (display_line, col_on_line);
            }
            if cursor == line_end && col_on_line > 0 && col_on_line.is_multiple_of(width) {
                return (display_line + col_on_line / width - 1, width - 1);
            }
            display_line += col_on_line / width;
            return (display_line, col_on_line % width);
        }
        let segments = wrap_line_count(physical, width);
        display_line += segments;
        char_idx = line_end + 1;
    }
    let total_display = wrapped_display_lines(input, width).len().saturating_sub(1);
    (total_display, 0)
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

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let left = footer_status_left(app);
    let right = if app.is_working() {
        "Ctrl-C twice: quit "
    } else {
        "/help · Ctrl-C: quit "
    };
    let pad = (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!("{left}{}{right}", " ".repeat(pad)),
            dim,
        )),
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

fn draw_modal(frame: &mut Frame, screen: Rect, app: &mut App) {
    let Some(modal) = &app.modal else { return };
    let mut area = centered(screen, 90, 75);
    if let Modal::Picker { items, .. } = modal {
        // Fit the list: header rows plus items, never taller than the default size.
        let rows = modal.visible().len().min(items.len()).max(1) as u16;
        area.height = (rows + 6).clamp(8, area.height.max(8)).min(screen.height);
        area.y = screen.y + (screen.height.saturating_sub(area.height)) / 2;
    }
    frame.render_widget(Clear, area);
    let (title, body, hint, scroll): (&str, Vec<Line>, &str, u16) = match modal {
        Modal::Approve { text, scroll } => (
            " Approve edit ",
            diff_lines(text),
            " y: approve   a: approve all this session   n/Esc: reject   ↑↓: scroll ",
            *scroll,
        ),
        Modal::Confirm { text, scroll } => (
            " Confirm ",
            diff_lines(text),
            " y: yes   n/Esc: no ",
            *scroll,
        ),
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
        Modal::Picker {
            kind,
            filter,
            selected,
            loading,
            ..
        } => {
            let (title, noun) = match kind {
                PickKind::Model => (" Select model ", "models"),
                PickKind::Provider => (" Select provider ", "providers"),
                PickKind::Effort => (" Reasoning effort ", "levels"),
                PickKind::Command => (" Command palette ", "commands"),
            };
            let shown = modal.visible();
            let mut lines = vec![Line::from(vec![
                Span::styled("filter: ", Style::new().bold()),
                Span::raw(filter.clone()),
                Span::styled("▏", Style::new().fg(Color::Cyan)),
            ])];
            lines.push(Line::raw(""));
            if *loading {
                lines.push(Line::styled(
                    "loading models from the provider…",
                    Style::new().add_modifier(Modifier::DIM),
                ));
            } else if shown.is_empty() {
                let note = match kind {
                    PickKind::Model if !filter.trim().is_empty() => {
                        format!("no match; Enter uses '{}' as the model id", filter.trim())
                    }
                    PickKind::Command => "no commands match".to_string(),
                    _ => format!("no {noun} match"),
                };
                lines.push(Line::styled(note, Style::new().fg(Color::Yellow)));
            } else {
                // Window of rows that keeps the selection visible.
                let rows = (area.height as usize).saturating_sub(6).max(1);
                let start = selected
                    .saturating_sub(rows / 2)
                    .min(shown.len().saturating_sub(rows));
                for (i, item) in shown.iter().enumerate().skip(start).take(rows) {
                    let style = if i == *selected {
                        Style::new().bold().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::new()
                    };
                    lines.push(Line::styled(format!(" {item} "), style));
                }
                lines.push(Line::styled(
                    format!("{} of {} {noun}", selected + 1, shown.len()),
                    Style::new().add_modifier(Modifier::DIM),
                ));
            }
            (
                title,
                lines,
                " type: filter   ↑↓: move   Enter: select   Esc: cancel ",
                0,
            )
        }
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
        .border_style(Style::new().fg(Color::Cyan))
        .title(title)
        .title_bottom(Line::styled(hint, Style::new().add_modifier(Modifier::DIM)));
    let inner = block.inner(area);
    let paragraph = Paragraph::new(body).block(block).wrap(Wrap { trim: false });
    let mut scroll = scroll;
    if matches!(
        app.modal,
        Some(Modal::Approve { .. }) | Some(Modal::Confirm { .. })
    ) {
        let total = paragraph.line_count(inner.width);
        let max_scroll = total
            .saturating_sub(inner.height as usize)
            .min(u16::MAX as usize) as u16;
        scroll = scroll.min(max_scroll);
        app.modal = match app.modal.take() {
            Some(Modal::Approve { text, .. }) => Some(Modal::Approve { text, scroll }),
            Some(Modal::Confirm { text, .. }) => Some(Modal::Confirm { text, scroll }),
            other => other,
        };
    }
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
        assert!(footer_row(&screen).contains("session"));
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
    fn idle_screen_shows_header_input_and_hint() {
        let mut app = App::new(Profile::Build, "nim · model-x".into(), None);
        let screen = render(&mut app, 80, 20);
        assert!(screen.contains("DevCode"));
        assert!(screen.contains("build"));
        assert!(screen.contains("nim · model-x"));
        assert!(screen.contains("message"));
        assert!(screen.contains("/help"));
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
        assert!(screen.contains("line-0079") || screen.contains("[y]es"));
    }

    #[test]
    fn approve_modal_shows_diff() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.on_event(Ev::PromptText(
            "--- a/calc.py\n+++ b/calc.py\n-return a - b\n+return a + b\nApprove edit to 'calc.py'? [y]es / [a]lways / [n]o: ".into(),
        ));
        app.on_event(Ev::NeedInput);
        let screen = render(&mut app, 100, 30);
        assert!(screen.contains("Approve edit"));
        assert!(screen.contains("+return a + b"));
        assert!(screen.contains("n/Esc: reject"));
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
        assert!(screen.contains(&format!("3 of {total} models")));
        assert!(!screen.contains("model-4 "));
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
            },
        ]
    }

    #[test]
    fn tool_card_collapsed_hides_extra_preview_lines() {
        let mut app = App::new(Profile::Build, "p".into(), None);
        app.items.extend(sample_tool_items());
        app.expanded_tool = None;
        let screen = render(&mut app, 80, 20);
        assert!(screen.contains("read_file"));
        assert!(screen.contains("line-one-visible"));
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
        assert_eq!(wrapped_cursor_pos(&app.input, app.cursor, 8), (0, 7));
        assert!(render(&mut app, 10, 10).contains("abcdefgh"));
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
        let screen = render(&mut app, 80, 20);
        assert!(screen.contains("· thinking line"));
    }
}
