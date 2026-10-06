use derisk::{
    action::{Action, LayoutKind},
    keys::{Key, Mods, SuperTap, binding},
    snap::Direction,
};

const SUPER: Mods = Mods {
    logo: true,
    shift: false,
    ctrl: false,
    alt: false,
};

#[test]
fn super_chords_map_to_actions() {
    assert_eq!(
        binding(SUPER, Key::Arrow(Direction::Left)),
        Some(Action::Nudge {
            window: None,
            direction: Direction::Left
        })
    );
    assert_eq!(
        binding(SUPER, Key::Digit(3)),
        Some(Action::SwitchWorkspace { workspace: 3 })
    );
    let shifted = Mods {
        shift: true,
        ..SUPER
    };
    assert_eq!(
        binding(shifted, Key::Digit(2)),
        Some(Action::MoveToWorkspace {
            window: None,
            workspace: 2
        })
    );
    assert_eq!(
        binding(SUPER, Key::Letter('m')),
        Some(Action::SetLayout {
            layout: LayoutKind::Monocle
        })
    );
    assert_eq!(
        binding(shifted, Key::Letter('m')),
        Some(Action::SetLayout {
            layout: LayoutKind::Tall
        })
    );
    assert_eq!(binding(SUPER, Key::Digit(0)), None);
}

#[test]
fn plain_keys_reach_clients() {
    assert_eq!(binding(Mods::default(), Key::Letter('q')), None);
    assert_eq!(binding(Mods::default(), Key::Arrow(Direction::Up)), None);
    let ctrl_super = Mods {
        ctrl: true,
        ..SUPER
    };
    assert_eq!(binding(ctrl_super, Key::Letter('q')), None);
}

#[test]
fn alt_tab_cycles_focus() {
    let alt = Mods {
        alt: true,
        ..Mods::default()
    };
    assert_eq!(binding(alt, Key::Tab), Some(Action::FocusNext));
    let alt_shift = Mods { shift: true, ..alt };
    assert_eq!(binding(alt_shift, Key::Tab), Some(Action::FocusPrevious));
}

#[test]
fn super_tap_only_without_other_keys() {
    let mut tap = SuperTap::default();
    assert!(!tap.key(true, true));
    assert!(tap.key(true, false));

    assert!(!tap.key(true, true));
    assert!(!tap.key(false, true));
    assert!(!tap.key(true, false));

    assert!(!tap.key(true, true));
    tap.cancel();
    assert!(!tap.key(true, false));
}

#[test]
fn rebound_shortcuts_move_and_free_their_chord() {
    use derisk::keys::binding_with;
    use derisk_settings::{Chord, Shortcut, Shortcuts};
    let mut shortcuts = Shortcuts::default();
    shortcuts.set(Shortcut::Close, Chord::parse("Ctrl+Alt+W"));
    shortcuts.set(Shortcut::Float, None);
    let ctrl_alt = Mods {
        ctrl: true,
        alt: true,
        ..Mods::default()
    };
    assert_eq!(
        binding_with(&shortcuts, ctrl_alt, Key::Letter('w')),
        Some(Action::Close { window: None })
    );
    assert_eq!(binding_with(&shortcuts, SUPER, Key::Letter('q')), None);
    assert_eq!(binding_with(&shortcuts, SUPER, Key::Letter('f')), None);
    // The fixed families still work.
    assert_eq!(
        binding_with(&shortcuts, SUPER, Key::Digit(4)),
        Some(Action::SwitchWorkspace { workspace: 4 })
    );
}
