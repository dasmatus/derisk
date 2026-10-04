use std::{
    io::Write,
    process::{Command, Stdio},
};

use derisk::{
    action::{Action, Effect},
    assistant,
    geom::rect,
    ipc,
    shell::Shell,
    snap::SnapZone,
    systemd::SessionOp,
};
use serde_json::{Value, json};

#[test]
fn assistant_understands_compound_requests() {
    assert_eq!(
        assistant::interpret("open firefox and snap it to the left").unwrap(),
        vec![
            Action::Launch {
                app: "firefox".into()
            },
            Action::Snap {
                window: None,
                zone: SnapZone::Left
            },
        ]
    );
    assert_eq!(
        assistant::interpret("Go to workspace 3").unwrap(),
        vec![Action::SwitchWorkspace { workspace: 3 }]
    );
    assert_eq!(
        assistant::interpret("show the overview").unwrap(),
        vec![Action::Overview {
            visible: Some(true)
        }]
    );
    assert!(assistant::interpret("make me a sandwich").is_err());
}

#[test]
fn assistant_never_confirms_destructive_ops_itself() {
    assert_eq!(
        assistant::interpret("shut down").unwrap(),
        vec![Action::Session {
            op: SessionOp::PowerOff,
            confirmed: false
        }]
    );
}

#[test]
fn ipc_dispatch_state_and_tools() {
    let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
    let (resp, effects) = ipc::handle_line(
        &mut shell,
        r#"{"method":"dispatch","actions":[{"action":"launch","app":"foot"}]}"#,
    );
    assert_eq!(effects, vec![Effect::Launch { app: "foot".into() }]);
    assert_eq!(serde_json::from_str::<Value>(&resp).unwrap()["ok"], true);

    shell.map_window("foot", "foot");
    let (resp, _) = ipc::handle_line(&mut shell, r#"{"method":"state"}"#);
    let v: Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["result"]["windows"][0]["app_id"], "foot");
    assert_eq!(v["result"]["form_factor"], "desktop");

    let (resp, _) = ipc::handle_line(&mut shell, r#"{"method":"tools"}"#);
    let v: Value = serde_json::from_str(&resp).unwrap();
    let names: Vec<_> = v["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(names.contains(&"dispatch".to_owned()));

    let (resp, effects) = ipc::handle_line(&mut shell, "{nope");
    assert!(effects.is_empty());
    assert_eq!(serde_json::from_str::<Value>(&resp).unwrap()["ok"], false);
}

#[test]
fn ipc_menus_feed_the_global_menu() {
    let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
    let (w, _) = shell.map_window("app", "App");
    let req = json!({
        "method": "register_menu",
        "window": w.get(),
        "menus": [{"title": "File", "entries": [{"kind": "item", "id": "open", "label": "Open"}]}],
    });
    let (resp, _) = ipc::handle_line(&mut shell, &req.to_string());
    assert_eq!(
        serde_json::from_str::<Value>(&resp).unwrap()["ok"],
        true,
        "{resp}"
    );
    let bar = shell.menus.bar(Some(w.get()));
    assert_eq!(bar[0].title, "File");
    assert_eq!(bar.last().unwrap().title, "Window");
}

#[test]
fn agent_binary_speaks_json_lines_on_stdio() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_derisk"))
        .arg("agent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"method\":\"ask\",\"text\":\"open kitty and snap it right\"}\n{\"method\":\"state\"}\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let lines: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    let window = &lines[1]["result"]["windows"][0];
    assert_eq!(window["app_id"], "kitty");
    assert_eq!(window["zone"], "right");
}

#[test]
fn requests_are_recorded_with_step_progress() {
    use derisk::conversation::{Source, StepStatus};

    let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
    shell
        .ask("open kitty and snap it left", Source::User, false)
        .into_result()
        .unwrap();
    let turn = shell.conversation.turns().last().unwrap().clone();
    assert_eq!(turn.source, Source::User);
    assert_eq!(turn.steps[0].label, "Open kitty");
    assert_eq!(turn.steps[0].status, StepStatus::Done);
    assert_eq!(turn.steps[1].label, "Snap the window left");
    assert_eq!(turn.steps[1].status, StepStatus::Waiting);
    assert!(!turn.is_settled());
    assert!(turn.reply().starts_with("Waiting"));

    // The step finishes when kitty's window maps.
    shell.map_window("kitty", "~");
    let turn = shell.conversation.turns().last().unwrap();
    assert_eq!(turn.steps[1].status, StepStatus::Done);
    assert_eq!(turn.reply(), "Done.");

    // A failing step stops the rest.
    assert!(
        shell
            .ask("go to workspace 42 and close it", Source::User, false)
            .into_result()
            .is_err()
    );
    let turn = shell.conversation.turns().last().unwrap();
    assert!(matches!(turn.steps[0].status, StepStatus::Failed(_)));
    assert_eq!(turn.steps[1].status, StepStatus::Skipped);
    assert!(
        turn.reply()
            .starts_with("Stopped at \"Go to workspace 42\"")
    );

    // Not understood.
    assert!(
        shell
            .ask("make me a sandwich", Source::User, false)
            .into_result()
            .is_err()
    );
    assert!(shell.conversation.turns().last().unwrap().error.is_some());

    // Destructive operations need the person's confirmation.
    assert!(matches!(
        shell.ask("reboot", Source::User, false).into_result(),
        Err(derisk::shell::Error::NeedsConfirmation(SessionOp::Reboot))
    ));
    assert!(
        shell
            .ask("reboot", Source::User, true)
            .into_result()
            .is_ok()
    );
    assert!(
        shell
            .ask("lock the screen", Source::User, false)
            .into_result()
            .is_ok()
    );
}

#[test]
fn agent_requests_show_in_the_conversation() {
    use derisk::conversation::Source;

    let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
    ipc::handle_line(&mut shell, r#"{"method":"ask","text":"go to workspace 2"}"#);
    ipc::handle_line(
        &mut shell,
        r#"{"method":"dispatch","actions":[{"action":"switch_workspace","workspace":3}]}"#,
    );
    let (_, effects) = ipc::handle_line(&mut shell, r#"{"method":"ask","text":"shut down"}"#);
    assert!(
        effects.is_empty(),
        "agents still cannot power off unconfirmed"
    );
    let turns: Vec<_> = shell.conversation.turns().collect();
    assert_eq!(turns.len(), 3);
    assert!(turns.iter().all(|t| t.source == Source::Agent));
    assert_eq!(turns[0].request, "go to workspace 2");
    assert_eq!(turns[1].request, "1 action");
    assert_eq!(turns[1].steps[0].label, "Go to workspace 3");
}

#[test]
fn a_failed_step_keeps_the_effects_of_the_steps_before_it() {
    use derisk::conversation::{Source, StepStatus};

    let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
    // Launching kitty succeeds, then the workspace does not exist: the
    // launch must still happen.
    let outcome = shell.ask("open kitty then go to workspace 42", Source::User, false);
    assert!(outcome.result.is_err());
    assert_eq!(
        outcome.effects,
        [Effect::Launch {
            app: "kitty".into()
        }]
    );
    let turn = shell.conversation.turns().last().unwrap();
    assert_eq!(turn.steps[0].status, StepStatus::Done);
    assert!(matches!(turn.steps[1].status, StepStatus::Failed(_)));

    // Over the agent protocol, the failed request still carries its effects.
    let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
    let request = json!({
        "method": "dispatch",
        "actions": [
            {"action": "launch", "app": "kitty"},
            {"action": "switch_workspace", "workspace": 42}
        ]
    });
    let (reply, effects) = ipc::handle_line(&mut shell, &request.to_string());
    assert!(reply.contains("\"ok\":false"), "{reply}");
    assert_eq!(
        effects,
        [Effect::Launch {
            app: "kitty".into()
        }]
    );
}

#[test]
fn deferred_steps_stop_at_the_first_failure() {
    use derisk::conversation::{Source, StepStatus};

    let mut shell = Shell::new(rect(0, 0, 1920, 1080), false);
    let outcome = shell.run_recorded(
        "open kitty, move it to workspace 42 and close it",
        Source::User,
        vec![
            Action::Launch {
                app: "kitty".into(),
            },
            Action::MoveToWorkspace {
                window: None,
                workspace: 42,
            },
            Action::Close { window: None },
        ],
    );
    assert!(outcome.result.is_ok());
    let (kitty, effects) = shell.map_window("kitty", "~");
    let turn = shell.conversation.turns().last().unwrap();
    assert!(matches!(turn.steps[1].status, StepStatus::Failed(_)));
    assert_eq!(turn.steps[2].status, StepStatus::Skipped);
    assert!(effects.is_empty(), "the close was skipped: {effects:?}");
    assert!(shell.window_label(kitty).is_some());
}
