use derisk::{
    action::Action,
    geom::rect,
    overview::{OverviewLayout, fit, grid, row},
    shell::Shell,
    ui::{ShellUi, to_rect},
};
use derisk_settings::LowPower;
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

/// Opens workspace 2 with one window on it (workspaces are dynamic).
fn open_second_workspace(shell: &mut Shell) {
    let (w, _) = shell.map_window("foot", "sh");
    shell
        .apply(Action::MoveToWorkspace {
            window: Some(w.get()),
            workspace: 2,
        })
        .unwrap();
}

#[test]
fn palette_runs_the_selected_entry_and_confirms_destructive_ones() {
    let (w, h) = (1280.0, 800.0);
    let mut shell = Shell::new(rect(0, 0, 1280, 800), false);
    open_second_workspace(&mut shell);
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
        vec![text("workspace 2")],
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
            Action::SwitchWorkspace { workspace: 2 },
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
        vec![text("workspace 2")],
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
            workspace: 2
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

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }
}

#[test]
fn palette_hands_requests_to_the_assistant_and_shows_the_conversation() {
    use derisk::{conversation::Source, ui::Ask};

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
    // The palette stays open: the request runs in the conversation.
    let actions = frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![key(egui::Key::Enter)],
        5000,
    );
    assert!(actions.is_empty());
    let asks = ui.take_asks();
    assert_eq!(
        asks,
        [Ask {
            text: "open kitty then go to workspace 2".into(),
            confirmed: false
        }]
    );
    shell.ask(&asks[0].text, Source::User, false).unwrap();
    frame(&ctx, &mut ui, &shell, (w, h), vec![], 5000);

    // Follow-ups go straight to the assistant, and shutting down needs a
    // second Enter.
    frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![egui::Event::Text("shut down".into())],
        5000,
    );
    frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![key(egui::Key::Enter)],
        5000,
    );
    assert!(ui.take_asks().is_empty());
    frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![key(egui::Key::Enter)],
        5000,
    );
    assert_eq!(
        ui.take_asks(),
        [Ask {
            text: "shut down".into(),
            confirmed: true
        }]
    );

    // Backspace on an empty field goes back to searching.
    frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![key(egui::Key::Backspace)],
        5000,
    );
    frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![egui::Event::Text("lock screen".into())],
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
    assert!(matches!(actions[0], Action::Session { .. }), "{actions:?}");
}

#[test]
fn question_mark_opens_the_conversation() {
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
        vec![egui::Event::Text("?lock".into())],
        5000,
    );
    frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![key(egui::Key::Enter)],
        5000,
    );
    assert_eq!(ui.take_asks()[0].text, "lock");
}

#[test]
fn typing_on_the_overview_opens_the_palette() {
    let (w, h) = (1280.0, 800.0);
    let mut shell = Shell::new(rect(0, 0, 1280, 800), false);
    shell
        .apply(Action::Overview {
            visible: Some(true),
        })
        .unwrap();
    let mut ui = ShellUi::new(&shell, true);
    let ctx = egui::Context::default();
    frame(&ctx, &mut ui, &shell, (w, h), vec![], 5000);
    let actions = frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![egui::Event::Text("o".into())],
        5000,
    );
    assert_eq!(
        actions,
        [Action::Palette {
            visible: Some(true)
        }]
    );
    shell.run(actions).unwrap();
    frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![egui::Event::Text("pen kitty and snap it left".into())],
        5000,
    );
    frame(
        &ctx,
        &mut ui,
        &shell,
        (w, h),
        vec![key(egui::Key::Enter)],
        5000,
    );
    assert_eq!(ui.take_asks()[0].text, "open kitty and snap it left");
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

#[test]
fn translucent_panels_report_blur_areas_unless_low_power() {
    let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
    shell.map_window("editor", "notes.md");
    shell
        .apply(Action::Overview {
            visible: Some(true),
        })
        .unwrap();
    let mut ui = ShellUi::new(&shell, false);
    let ctx = egui::Context::default();
    frame(&ctx, &mut ui, &shell, (1920.0, 1080.0), vec![], 5000);
    let bar = shell.profile().top_bar;
    let areas: Vec<_> = ui.blur_regions().iter().map(|b| b.area).collect();
    assert!(areas.contains(&rect(0, 0, 1920, bar)), "{areas:?}");
    assert!(areas.contains(&shell.work_area()), "{areas:?}");
    assert!(ui.blur_regions().iter().all(|b| b.strength > 0));

    shell.effects.low_power = LowPower::On;
    frame(&ctx, &mut ui, &shell, (1920.0, 1080.0), vec![], 5100);
    assert!(ui.blur_regions().is_empty());

    shell.effects.low_power = LowPower::Off;
    shell.effects.blur = 0;
    frame(&ctx, &mut ui, &shell, (1920.0, 1080.0), vec![], 5200);
    assert!(ui.blur_regions().is_empty());
}
/// Drags the first exposé window onto workspace strip slot `slot` (0-based).
fn drag_to_slot(shell: &mut Shell, slot: usize, slots: usize) -> Vec<Action> {
    drag_to_slot_with(shell, slot, slots, |_| {}, vec![])
}

/// Like [`drag_to_slot`], but once the drag is under way `mid_drag` changes
/// the shell and `extra` events are sent with the move onto the slot.
fn drag_to_slot_with(
    shell: &mut Shell,
    slot: usize,
    slots: usize,
    mid_drag: impl FnOnce(&mut Shell),
    extra: Vec<egui::Event>,
) -> Vec<Action> {
    let size = (1920.0, 1080.0);
    let mut ui = ShellUi::new(shell, false);
    let ctx = egui::Context::default();
    let layout = OverviewLayout::new(shell.work_area(), shell.profile().form_factor);
    let active = shell.workspaces()[shell.active_workspace() as usize - 1];
    let windows = shell.windows_on(active);
    let cell = grid(windows.len(), layout.windows, 24)[0];
    let frame0 = shell
        .placements()
        .into_iter()
        .find(|p| p.window == windows[0])
        .unwrap()
        .frame;
    let from = to_rect(fit(frame0, cell)).center();
    let to = to_rect(row(slots, layout.workspaces, 10)[slot]).center();
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let mut steps = [
        vec![egui::Event::PointerMoved(from)],
        vec![button(from, true)],
        vec![egui::Event::PointerMoved(from + egui::vec2(30.0, -30.0))],
        vec![egui::Event::PointerMoved(to)],
        vec![button(to, false)],
    ];
    steps[3].extend(extra);
    let mut mid_drag = Some(mid_drag);
    let mut actions = Vec::new();
    for (i, events) in steps.into_iter().enumerate() {
        if i == 3 {
            (mid_drag.take().expect("runs once"))(shell);
        }
        actions.extend(frame(
            &ctx,
            &mut ui,
            shell,
            size,
            events,
            5000 + i as u32 * 16,
        ));
    }
    actions
}

fn overview_with_two_windows() -> (Shell, u64) {
    let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
    let (a, _) = shell.map_window("editor", "notes.md");
    shell.map_window("terminal", "sh");
    shell
        .apply(Action::Overview {
            visible: Some(true),
        })
        .unwrap();
    (shell, a.get())
}

#[test]
fn dragging_a_window_onto_plus_opens_a_new_workspace() {
    let (mut shell, a) = overview_with_two_windows();
    // Strip: workspace 1, then "+".
    let actions = drag_to_slot(&mut shell, 1, 2);
    assert_eq!(
        actions,
        vec![Action::MoveToWorkspace {
            window: Some(a),
            workspace: 2
        }]
    );
    shell.run(actions).unwrap();
    assert_eq!(shell.workspaces().len(), 2);
    assert!(shell.overview_visible());
}

#[test]
fn dragging_a_window_onto_a_workspace_moves_it_there() {
    let (mut shell, a) = overview_with_two_windows();
    let (c, _) = shell.map_window("browser", "web");
    shell
        .apply(Action::MoveToWorkspace {
            window: Some(c.get()),
            workspace: 2,
        })
        .unwrap();
    // Strip: workspaces 1 and 2, then "+".
    let actions = drag_to_slot(&mut shell, 1, 3);
    assert_eq!(
        actions,
        vec![Action::MoveToWorkspace {
            window: Some(a),
            workspace: 2
        }]
    );
    // Dropping back on the current workspace does nothing.
    assert!(drag_to_slot(&mut shell, 0, 3).is_empty());
}

#[test]
fn other_buttons_do_not_drop_a_dragged_window() {
    let (mut shell, a) = overview_with_two_windows();
    let secondary = |pressed| egui::Event::PointerButton {
        pos: egui::pos2(500.0, 60.0),
        button: egui::PointerButton::Secondary,
        pressed,
        modifiers: Default::default(),
    };
    // A right click mid-drag leaves the drag alive; the primary release drops it.
    let actions = drag_to_slot_with(
        &mut shell,
        1,
        2,
        |_| {},
        vec![secondary(true), secondary(false)],
    );
    assert_eq!(
        actions,
        vec![Action::MoveToWorkspace {
            window: Some(a),
            workspace: 2
        }]
    );
}

#[test]
fn switching_workspaces_mid_drag_cancels_it() {
    let (mut shell, _) = overview_with_two_windows();
    let (c, _) = shell.map_window("browser", "web");
    shell
        .apply(Action::MoveToWorkspace {
            window: Some(c.get()),
            workspace: 2,
        })
        .unwrap();
    // Super+2 during the drag: the dragged window is no longer on screen,
    // so releasing over the "+" slot must not move it.
    let actions = drag_to_slot_with(
        &mut shell,
        2,
        3,
        |shell| {
            shell
                .apply(Action::SwitchWorkspace { workspace: 2 })
                .unwrap();
        },
        vec![],
    );
    assert!(actions.is_empty(), "{actions:?}");
}
