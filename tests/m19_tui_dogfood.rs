//! Scripted TUI dogfood for RES-013 checklist rows D5–D12.
//! Live provider/model/effort and build runs are recorded in
//! `docs/research/tui-dogfood-and-harder-task.md`.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use devcode::tui::app::{Action, App, Item, Modal, PickKind};
use devcode::tui::worker::{Cmd, Ev};
use devcode::Profile;

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

#[test]
fn dogfood_checklist_d5_through_d12() {
    let mut app = App::new(Profile::Ask, "gemini · gemini-3.8-flash".into(), None);
    // D1: provider label present at startup.
    assert!(
        app.provider.contains("gemini"),
        "D1: startup provider label"
    );

    // D5: scroll pin + Ctrl-End.
    app.scroll = 25;
    app.on_event(Ev::Progress(devcode::agent::ProgressEvent::Assistant {
        text: "more".into(),
    }));
    assert_eq!(app.scroll, 25, "D5: progress must not unpin scroll");
    app.on_key(ctrl_key(KeyCode::End));
    assert_eq!(app.scroll, 0, "D5: Ctrl-End jumps to latest");

    // D6: tool card toggle.
    app.items.push(Item::Tool {
        name: "grep".into(),
        summary: "pattern".into(),
    });
    app.on_key(ctrl('t'));
    assert_eq!(
        app.expanded_tool,
        Some(app.items.len() - 1),
        "D6: Ctrl-T expands"
    );
    app.on_key(ctrl('t'));
    assert_eq!(app.expanded_tool, None, "D6: Ctrl-T collapses");

    // D7: approve modal scroll clamps on draw (U4); PageDown after clamp is a no-op.
    use devcode::tui::view::draw;
    use ratatui::{backend::TestBackend, Terminal};
    let mut text = String::from("--- a/x\n+++ b/x\n");
    for i in 0..80 {
        text.push_str(&format!("+line-{i:04}\n"));
    }
    text.push_str("Approve edit to 'x'? [y]es / [a]lways / [n]o: ");
    app.on_event(Ev::PromptText(text));
    app.on_event(Ev::NeedInput);
    for _ in 0..200 {
        app.on_key(key(KeyCode::PageDown));
    }
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| draw(f, &mut app)).unwrap();
    let before = match app.modal.clone().expect("D7 modal") {
        Modal::Approve { scroll, .. } => scroll,
        other => panic!("D7: expected approve modal, got {other:?}"),
    };
    assert!(before < 200, "D7: draw clamp reduces huge scroll");
    app.on_key(key(KeyCode::PageDown));
    terminal.draw(|f| draw(f, &mut app)).unwrap();
    let after = match app.modal.clone().expect("D7 modal") {
        Modal::Approve { scroll, .. } => scroll,
        other => panic!("D7: modal lost, got {other:?}"),
    };
    assert_eq!(after, before, "D7: PageDown at bottom is stable");
    app.modal = None;

    // D8: command palette.
    app.on_key(ctrl('k'));
    match &app.modal {
        Some(Modal::Picker {
            kind: PickKind::Command,
            ..
        }) => {}
        other => panic!("D8: expected command palette, got {other:?}"),
    }
    app.on_key(key(KeyCode::Esc));

    // D9: model picker filter collects characters.
    app.modal = Some(Modal::Picker {
        kind: PickKind::Model,
        items: vec![
            "gemini-3.8-flash".into(),
            "gemini-3-pro-preview".into(),
            "nvidia/nemotron-70b".into(),
        ],
        filter: String::new(),
        selected: 0,
        loading: false,
    });
    type_str(&mut app, "nem");
    match &app.modal {
        Some(Modal::Picker { filter, .. }) => assert_eq!(filter, "nem", "D9: filter"),
        other => panic!("D9: picker closed, got {other:?}"),
    }
    app.modal = None;

    // D10: working status flips with Thinking progress.
    assert!(!app.is_working(), "D10: idle");
    app.on_event(Ev::Progress(devcode::agent::ProgressEvent::Thinking {
        step: 1,
    }));
    assert!(app.is_working(), "D10: working after Thinking");

    // D11: Shift+Enter inserts newline.
    app.input = "hello".into();
    app.cursor = app.input.len();
    app.on_key(shift_key(KeyCode::Enter));
    assert!(app.input.contains('\n'), "D11: Shift+Enter newline");

    // D12: empty Up recalls history; nonempty Up does not.
    app.history.push("prior".into());
    app.input.clear();
    app.cursor = 0;
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.input, "prior", "D12: empty Up recalls history");
    app.input = "a\nb".into();
    app.cursor = app.input.len();
    app.on_key(key(KeyCode::Up));
    assert_ne!(app.input, "prior", "D12: nonempty Up keeps edit buffer");

    // D2 surface: /provider dispatches SetProvider (blocked while Working — reset first).
    app.status = devcode::tui::app::Status::Idle;
    app.input.clear();
    app.cursor = 0;
    type_str(&mut app, "/provider gemini");
    let actions = app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(&actions[..], [Action::Send(Cmd::SetProvider(name))] if name == "gemini"),
        "D2: /provider gemini did not dispatch SetProvider"
    );

    // Problem note (RES-013): while Working, /provider is intentionally deferred.
    app.status = devcode::tui::app::Status::Working { step: 1 };
    app.input.clear();
    app.cursor = 0;
    type_str(&mut app, "/provider nim");
    let blocked = app.on_key(key(KeyCode::Enter));
    assert!(
        blocked.is_empty(),
        "D2 busy: /provider must not switch mid-run"
    );
    assert!(
        app.items
            .iter()
            .any(|i| matches!(i, Item::Info(s) if s.contains("wait for the current run"))),
        "D2 busy: user must see wait hint"
    );
}
