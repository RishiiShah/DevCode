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

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let [header, body, input, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(3),
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

fn transcript_lines(items: &[Item]) -> Vec<Line<'static>> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for item in items {
        match item {
            Item::User(text) => {
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![
                    Span::styled("› ", Style::new().bold().fg(Color::Cyan)),
                    Span::styled(text.clone(), Style::new().bold()),
                ]));
            }
            Item::Assistant(text) => {
                lines.push(Line::raw(""));
                lines.extend(super::markdown::render(text));
            }
            Item::Tool { name, summary } => lines.push(Line::from(vec![
                Span::styled("  ⚙ ", dim),
                Span::styled(name.clone(), Style::new().fg(Color::Blue)),
                Span::styled(format!("  {summary}"), dim),
            ])),
            Item::ToolResult { ok, preview } => {
                let (mark, color) = if *ok {
                    ("✓", Color::Green)
                } else {
                    ("✗", Color::Red)
                };
                for (i, text) in preview.lines().enumerate() {
                    let lead = if i == 0 {
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
    }
    lines
}

fn draw_transcript(frame: &mut Frame, area: Rect, app: &mut App) {
    let paragraph = Paragraph::new(transcript_lines(&app.items)).wrap(Wrap { trim: false });
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
    let inner_width = area.width.saturating_sub(2) as usize;
    let chars: Vec<char> = app.input.chars().collect();
    // Keep the cursor visible: show the window of the line that ends at the cursor.
    let start = (app.cursor + 1)
        .saturating_sub(inner_width)
        .min(chars.len());
    let end = (start + inner_width).min(chars.len());
    let visible: String = chars[start..end].iter().collect();
    frame.render_widget(Paragraph::new(visible).block(block), area);
    if app.modal.is_none() {
        let before: String = chars[start..app.cursor.clamp(start, chars.len())]
            .iter()
            .collect();
        let x = (Line::raw(before).width() as u16).min(area.width.saturating_sub(2));
        frame.set_cursor_position(Position::new(area.x + 1 + x, area.y + 1));
    }
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let left = match app.status {
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
    };
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

fn draw_modal(frame: &mut Frame, screen: Rect, app: &App) {
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
    frame.render_widget(
        Paragraph::new(body)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
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

    #[test]
    fn idle_screen_shows_header_input_and_hint() {
        let mut app = App::new(Profile::Build, "nim · model-x".into());
        let screen = render(&mut app, 80, 20);
        assert!(screen.contains("DevCode"));
        assert!(screen.contains("build"));
        assert!(screen.contains("nim · model-x"));
        assert!(screen.contains("message"));
        assert!(screen.contains("/help"));
    }

    #[test]
    fn approve_modal_shows_diff() {
        let mut app = App::new(Profile::Build, "p".into());
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
        let mut app = App::new(Profile::Build, "p".into());
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
        let mut app = App::new(Profile::Build, "gemini · m".into());
        app.modal = Some(Modal::Picker {
            kind: PickKind::Model,
            items: (0..40).map(|i| format!("model-{i}")).collect(),
            filter: "model-3".into(),
            selected: 2,
            loading: false,
        });
        let screen = render(&mut app, 80, 24);
        assert!(screen.contains("Select model"));
        assert!(screen.contains("model-3"));
        assert!(screen.contains("3 of 11 models"));
        assert!(!screen.contains("model-4 "));
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let mut app = App::new(Profile::Build, "p".into());
        app.input = "héllo 世界 wide".into();
        app.cursor = app.input.chars().count();
        for (w, h) in [(1, 1), (2, 2), (5, 3), (20, 6), (40, 8)] {
            render(&mut app, w, h);
            app.modal = Some(Modal::Help);
            render(&mut app, w, h);
            app.modal = None;
        }
    }

    #[test]
    fn long_transcript_follows_bottom_and_scrolls() {
        let mut app = App::new(Profile::Ask, "p".into());
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
}
