use derisk::{action::Action, geom::rect, shell::Shell, ui::ShellUi};
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
