//! Small markdown renderer for assistant text: headings, bullets, quotes, fenced code, tables,
//! **bold** and `code`. No dependencies; unknown syntax is shown as written.
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

fn code_style() -> Style {
    Style::new().fg(Color::Yellow)
}

/// Inline spans: `code` and **bold**. An unclosed marker is kept as text.
fn inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut bold = false;
    let style = |bold: bool| {
        if bold {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        }
    };
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '`' {
            if let Some(len) = chars[i + 1..].iter().position(|c| *c == '`') {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), style(bold)));
                }
                let code: String = chars[i + 1..i + 1 + len].iter().collect();
                spans.push(Span::styled(code, code_style()));
                i += len + 2;
                continue;
            }
        }
        if chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            let closes = chars[i + 2..].windows(2).any(|w| w == ['*', '*']);
            if bold || closes {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), style(bold)));
                }
                bold = !bold;
                i += 2;
                continue;
            }
        }
        buf.push(chars[i]);
        i += 1;
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, style(bold)));
    }
    spans
}

fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Renders consecutive table lines with aligned columns; the first row is the header.
fn table(rows: &[&str]) -> Vec<Line<'static>> {
    let parsed: Vec<Vec<Vec<Span<'static>>>> = rows
        .iter()
        .filter(|r| !is_table_separator(r))
        .enumerate()
        .map(|(n, row)| {
            let base = if n == 0 {
                Style::new().add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            row.trim()
                .trim_matches('|')
                .split('|')
                .map(|cell| inline(cell.trim(), base))
                .collect()
        })
        .collect();
    let columns = parsed.iter().map(Vec::len).max().unwrap_or(0);
    let width = |cell: &[Span]| Line::from(cell.to_vec()).width();
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            parsed
                .iter()
                .filter_map(|r| r.get(c))
                .map(|cell| width(cell))
                .max()
                .unwrap_or(0)
        })
        .collect();
    parsed
        .into_iter()
        .map(|row| {
            let mut spans = Vec::new();
            for (c, cell) in row.into_iter().enumerate() {
                if c > 0 {
                    spans.push(Span::styled("  │  ", dim()));
                }
                let pad = widths[c].saturating_sub(width(&cell));
                spans.extend(cell);
                if c + 1 < columns {
                    spans.push(Span::raw(" ".repeat(pad)));
                }
            }
            Line::from(spans)
        })
        .collect()
}

pub fn render(text: &str) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut in_code = false;
    let all: Vec<&str> = text.lines().collect();
    let mut index = 0;
    while index < all.len() {
        let raw = all[index];
        index += 1;
        let trimmed = raw.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if in_code {
                out.push(Line::styled("  ╰─", dim()));
            } else {
                let lang = rest.trim();
                out.push(Line::styled(
                    if lang.is_empty() {
                        "  ╭─".to_string()
                    } else {
                        format!("  ╭─ {lang}")
                    },
                    dim(),
                ));
            }
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push(Line::from(vec![
                Span::styled("  │ ", dim()),
                Span::styled(raw.replace('\t', "    "), Style::new().fg(Color::Gray)),
            ]));
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            let title = trimmed[hashes..].trim();
            let style = Style::new().bold().fg(Color::Cyan);
            out.push(Line::from(inline(title, style)));
            continue;
        }
        if trimmed.len() >= 3 && trimmed.chars().all(|c| matches!(c, '-' | '*' | '_')) {
            out.push(Line::styled("  ────────────────", dim()));
            continue;
        }
        if let Some(quote) = trimmed.strip_prefix('>') {
            let mut spans = vec![Span::styled("▎ ", dim())];
            spans.extend(inline(quote.trim_start(), dim()));
            out.push(Line::from(spans));
            continue;
        }
        if trimmed.starts_with('|') {
            let start = index - 1;
            while index < all.len() && all[index].trim_start().starts_with('|') {
                index += 1;
            }
            out.extend(table(&all[start..index]));
            continue;
        }
        let indent = raw.len() - trimmed.len();
        if let Some(item) = ["- ", "* ", "+ "]
            .iter()
            .find_map(|marker| trimmed.strip_prefix(marker))
        {
            let mut spans = vec![Span::raw(format!("{}• ", " ".repeat(indent)))];
            spans.extend(inline(item, Style::new()));
            out.push(Line::from(spans));
            continue;
        }
        out.push(Line::from(inline(raw, Style::new())));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn headings_lose_hashes_and_bullets_get_dots() {
        let out = plain(&render("## `calc.py`\n- one\n* two\n  - nested"));
        assert_eq!(out, vec!["calc.py", "• one", "• two", "  • nested"]);
    }

    #[test]
    fn fenced_code_is_boxed_and_not_parsed() {
        let out = plain(&render(
            "```python\ndef f(): # **x**\n    return 1\n```\nafter",
        ));
        assert_eq!(
            out,
            vec![
                "  ╭─ python",
                "  │ def f(): # **x**",
                "  │     return 1",
                "  ╰─",
                "after"
            ]
        );
    }

    #[test]
    fn inline_code_and_bold_lose_their_markers() {
        let lines = render("it **really** uses `a - b` here");
        assert_eq!(plain(&lines), vec!["it really uses a - b here"]);
        let bold = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "really")
            .unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let code = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "a - b")
            .unwrap();
        assert_eq!(code.style.fg, Some(Color::Yellow));
    }

    #[test]
    fn unclosed_markers_stay_literal() {
        assert_eq!(plain(&render("a ** b and `c")), vec!["a ** b and `c"]);
    }

    #[test]
    fn tables_drop_separator_rows() {
        let out = plain(&render(
            "| file | n |\n|---|---|\n| calc.py | 2 |\n| a | 10 |",
        ));
        assert_eq!(
            out,
            vec!["file     │  n", "calc.py  │  2", "a        │  10"]
        );
    }

    #[test]
    fn numbered_and_plain_lines_pass_through() {
        assert_eq!(
            plain(&render("1. first\nplain text")),
            vec!["1. first", "plain text"]
        );
    }
}
