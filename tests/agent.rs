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
