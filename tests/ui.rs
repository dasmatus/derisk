use derisk::{
    action::Action,
    geom::rect,
    overview::{OverviewLayout, fit, grid, row},
    shell::Shell,
    ui::{ShellUi, to_rect},
};
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

/// Drags the first exposé window onto workspace strip slot `slot` (0-based).
fn drag_to_slot(shell: &Shell, slot: usize, slots: usize) -> Vec<Action> {
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
    let mut actions = Vec::new();
    let steps = [
        vec![egui::Event::PointerMoved(from)],
        vec![button(from, true)],
        vec![egui::Event::PointerMoved(from + egui::vec2(30.0, -30.0))],
        vec![egui::Event::PointerMoved(to)],
        vec![button(to, false)],
    ];
    for (i, events) in steps.into_iter().enumerate() {
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
    let actions = drag_to_slot(&shell, 1, 2);
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
    let actions = drag_to_slot(&shell, 1, 3);
    assert_eq!(
        actions,
        vec![Action::MoveToWorkspace {
            window: Some(a),
            workspace: 2
        }]
    );
    // Dropping back on the current workspace does nothing.
    assert!(drag_to_slot(&shell, 0, 3).is_empty());
}
