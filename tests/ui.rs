use derisk::{action::Action, geom::rect, shell::Shell, ui::ShellUi};
use mcsapi::toolkit::egui;

fn frame(
    ctx: &egui::Context,
    ui: &mut ShellUi,
    shell: &Shell,
    size: (f32, f32),
    events: Vec<egui::Event>,
    ms: u32,
) -> Vec<Action> {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(size.0, size.1),
        )),
        events,
        ..Default::default()
    };
    let mut actions = Vec::new();
    let mut out = ctx.run_ui(input, |root| {
        ui.paint_decorations(root.painter(), shell);
        actions = ui.show(root, shell, ms);
    });
    assert!(!out.shapes.is_empty());
    out.textures_delta.clear();
    actions
}

#[test]
fn renders_every_form_factor_with_and_without_overview() {
    for (w, h, touch) in [(1920, 1080, false), (1280, 800, true), (400, 800, true)] {
        let mut shell = Shell::new(rect(0, 0, w, h), touch);
        shell.map_window("editor", "notes.md");
        shell.map_window("terminal", "sh");
        shell.failed_units = vec!["foo.service".into()];
        let mut ui = ShellUi::new(&shell, false);
        let ctx = egui::Context::default();
        for ms in [0, 900, 5000] {
            frame(&ctx, &mut ui, &shell, (w as f32, h as f32), vec![], ms);
        }
        shell
            .apply(Action::Overview {
                visible: Some(true),
            })
            .unwrap();
        frame(&ctx, &mut ui, &shell, (w as f32, h as f32), vec![], 5000);
    }
}

#[test]
fn startup_cover_swallows_clicks() {
    let shell = Shell::new(rect(0, 0, 1920, 1080), false);
    let mut ui = ShellUi::new(&shell, false);
    let ctx = egui::Context::default();
    let click = |pressed| egui::Event::PointerButton {
        pos: egui::pos2(14.0, 14.0),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(&ctx, &mut ui, &shell, (1920.0, 1080.0), vec![], 0);
    let actions = frame(
        &ctx,
        &mut ui,
        &shell,
        (1920.0, 1080.0),
        vec![click(true), click(false)],
        10,
    );
    assert!(actions.is_empty());
}

#[test]
fn overview_button_in_the_top_bar_toggles_the_overview() {
    let shell = Shell::new(rect(0, 0, 1920, 1080), false);
    let mut ui = ShellUi::new(&shell, false);
    let ctx = egui::Context::default();
    let click = |pressed| egui::Event::PointerButton {
        pos: egui::pos2(16.0, 14.0),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let size = (1920.0, 1080.0);
    frame(
        &ctx,
        &mut ui,
        &shell,
        size,
        vec![egui::Event::PointerMoved(egui::pos2(16.0, 14.0))],
        5000,
    );
    frame(&ctx, &mut ui, &shell, size, vec![click(true)], 5010);
    let actions = frame(&ctx, &mut ui, &shell, size, vec![click(false)], 5020);
    assert_eq!(actions, vec![Action::Overview { visible: None }]);
}

#[test]
fn palette_runs_the_selected_entry_and_confirms_destructive_ones() {
    let (w, h) = (1280.0, 800.0);
    let mut shell = Shell::new(rect(0, 0, 1280, 800), false);
    shell.map_window("editor", "notes.md");
    shell.apply(Action::Palette { visible: None }).unwrap();
    let mut ui = ShellUi::new(&shell, true);
    let ctx = egui::Context::default();
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    };
    let text = |t: &str| egui::Event::Text(t.to_owned());

    // Opening focuses the search field, so typing goes straight in.
    frame(&ctx, &mut ui, &shell, (w, h), vec![], 5000);
    frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![text("workspace 3")],
        5000,
    );
    let actions = frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![key(egui::Key::Enter)],
        5000,
    );
    assert_eq!(
        actions,
        [
            Action::SwitchWorkspace { workspace: 3 },
            Action::Palette {
                visible: Some(false)
            }
        ]
    );

    // Arrow down picks the next match.
    let mut ui = ShellUi::new(&shell, true);
    let ctx = egui::Context::default();
    frame(&ctx, &mut ui, &shell, (w, h), vec![], 5000);
    frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![text("workspace 3")],
        5000,
    );
    let actions = frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![key(egui::Key::ArrowDown), key(egui::Key::Enter)],
        5000,
    );
    assert_eq!(
        actions[0],
        Action::MoveToWorkspace {
            window: None,
            workspace: 3
        }
    );

    // Shutting down takes a second Enter.
    let mut ui = ShellUi::new(&shell, true);
    let ctx = egui::Context::default();
    frame(&ctx, &mut ui, &shell, (w, h), vec![], 5000);
    frame(&ctx, &mut ui, &shell, (w, h), vec![text("shut down")], 5000);
    let first = frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![key(egui::Key::Enter)],
        5000,
    );
    assert!(first.is_empty());
    let second = frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![key(egui::Key::Enter)],
        5000,
    );
    assert_eq!(
        second[0],
        Action::Session {
            op: derisk::systemd::SessionOp::PowerOff,
            confirmed: true
        }
    );

    // Escape closes without running anything.
    let actions = frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![key(egui::Key::Escape)],
        5000,
    );
    assert_eq!(
        actions,
        [Action::Palette {
            visible: Some(false)
        }]
    );
}

#[test]
fn palette_hands_requests_to_the_assistant() {
    let (w, h) = (1280.0, 800.0);
    let mut shell = Shell::new(rect(0, 0, 1280, 800), false);
    shell.apply(Action::Palette { visible: None }).unwrap();
    let mut ui = ShellUi::new(&shell, true);
    let ctx = egui::Context::default();
    frame(&ctx, &mut ui, &shell, (w, h), vec![], 5000);
    frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![egui::Event::Text(
            "open kitty then go to workspace 2".into(),
        )],
        5000,
    );
    let actions = frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
        5000,
    );
    assert_eq!(
        actions,
        [
            Action::Launch {
                app: "kitty".into()
            },
            Action::SwitchWorkspace { workspace: 2 },
            Action::Palette {
                visible: Some(false)
            }
        ]
    );
}

#[test]
fn failed_units_open_the_palette_on_their_actions() {
    let (w, h) = (1280.0, 800.0);
    let mut shell = Shell::new(rect(0, 0, 1280, 800), false);
    shell.failed_units = vec!["foo.service".into()];
    let mut ui = ShellUi::new(&shell, true);
    ui.palette.preset = Some("> failed".into());
    shell.apply(Action::Palette { visible: None }).unwrap();
    let ctx = egui::Context::default();
    frame(&ctx, &mut ui, &shell, (w, h), vec![], 5000);
    let actions = frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
        5000,
    );
    assert_eq!(
        actions[0],
        Action::RestartUnit {
            unit: "foo.service".into()
        }
    );
}
