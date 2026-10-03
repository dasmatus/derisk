//! Agent-first control: a JSON-lines protocol for AI agents and scripts.
//!
//! Each request is one JSON object per line; each response is one line:
//!
//! ```text
//! {"method":"state"}
//! {"method":"tools"}
//! {"method":"dispatch","actions":[{"action":"snap","zone":"left"}]}
//! {"method":"ask","text":"open firefox and snap it left"}
//! {"method":"register_menu","window":1,"menus":[...]}
//! ```
//!
//! Responses are `{"ok":true,"result":...}` or `{"ok":false,"error":"..."}`.
//! Effects in results (`launch`, `close`, `menu_activated`, `session`, ...)
//! must be carried out by the host that owns the shell; see
//! [`crate::systemd::effect_argv`].

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    action::{Action, Effect},
    adaptive::FormFactor,
    assistant,
    geom::Rect,
    menu::Menu,
    overview::Battery,
    shell::{Mode, Shell},
    time::Clock,
};

/// A request from an agent.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Request {
    /// Describe the desktop.
    State,
    /// List tools in an MCP-compatible shape.
    Tools,
    /// Apply actions.
    Dispatch {
        /// Actions, applied in order.
        actions: Vec<Action>,
    },
    /// Interpret natural language with the built-in assistant, then apply it.
    Ask {
        /// Request text.
        text: String,
    },
    /// Register global menus for a window.
    RegisterMenu {
        /// Window.
        window: u64,
        /// Menus.
        menus: Vec<Menu>,
    },
}

/// One managed window in a [`State`] snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct WindowState {
    /// ID.
    pub id: u64,
    /// App ID.
    pub app_id: String,
    /// Title.
    pub title: String,
    /// Workspace number.
    pub workspace: u64,
    /// Placement mode.
    #[serde(flatten)]
    pub mode: Mode,
    /// Frame on screen, if visible.
    pub frame: Option<Rect>,
    /// Keyboard focus.
    pub focused: bool,
    /// Minimized.
    pub minimized: bool,
}

/// A snapshot of the desktop for agents.
#[derive(Clone, Debug, Serialize)]
pub struct State {
    /// Output bounds.
    pub output: Rect,
    /// Detected form factor.
    pub form_factor: FormFactor,
    /// Active workspace number.
    pub active_workspace: u64,
    /// Workspace numbers.
    pub workspaces: Vec<u64>,
    /// All managed windows.
    pub windows: Vec<WindowState>,
    /// Overview visibility.
    pub overview: bool,
    /// Global menus of the focused window.
    pub menus: Vec<Menu>,
    /// Tray item titles.
    pub tray: Vec<String>,
    /// Suggested apps for this hour.
    pub suggestions: Vec<String>,
    /// Clock.
    pub clock: Clock,
    /// Battery.
    pub battery: Option<Battery>,
    /// Failed systemd user units.
    pub failed_units: Vec<String>,
}

/// Builds a [`State`] snapshot.
pub fn state(shell: &Shell) -> State {
    let placements = shell.placements();
    let focused = shell.focused();
    let windows = shell
        .desktop()
        .workspaces()
        .flat_map(|ws| ws.windows().map(move |w| (ws.id(), w)))
        .map(|(ws, w)| {
            let (app_id, title) = shell.window_label(w).unwrap_or_default();
            WindowState {
                id: w.get(),
                app_id: app_id.to_owned(),
                title: title.to_owned(),
                workspace: ws.get(),
                mode: shell.mode(w).unwrap_or(Mode::Tiled),
                frame: placements
                    .iter()
                    .find(|p| p.window == w)
                    .map(|p| p.frame.into()),
                focused: focused == Some(w),
                minimized: shell.is_minimized(w),
            }
        })
        .collect();
    State {
        output: shell.output().into(),
        form_factor: shell.profile().form_factor,
        active_workspace: shell.desktop().active().id().get(),
        workspaces: shell.desktop().workspaces().map(|w| w.id().get()).collect(),
        windows,
        overview: shell.overview_visible(),
        menus: shell.menus.bar(focused.map(|w| w.get())),
        tray: shell.tray.items().map(|i| i.title.clone()).collect(),
        suggestions: shell.habits.suggestions(shell.clock.hour, 5),
        clock: shell.clock,
        battery: shell.battery,
        failed_units: shell.failed_units.clone(),
    }
}

/// Tool descriptions for LLM agents, in MCP `tools/list` shape.
pub fn tools() -> Value {
    let window =
        json!({"type": "integer", "description": "Window ID; omit for the focused window"});
    let zone = json!({"enum": ["left", "right", "top_left", "top_right", "bottom_left", "bottom_right", "maximize"]});
    let action = json!({
        "oneOf": [
            {"type": "object", "required": ["action", "app"], "properties": {"action": {"const": "launch"}, "app": {"type": "string"}}},
            {"type": "object", "required": ["action"], "properties": {"action": {"enum": ["close", "tile", "float", "toggle_maximize", "minimize"]}, "window": window}},
            {"type": "object", "required": ["action", "window"], "properties": {"action": {"enum": ["focus", "restore"]}, "window": {"type": "integer"}}},
            {"type": "object", "required": ["action"], "properties": {"action": {"enum": ["focus_next", "focus_previous", "promote"]}}},
            {"type": "object", "required": ["action", "zone"], "properties": {"action": {"const": "snap"}, "window": window, "zone": zone}},
            {"type": "object", "required": ["action", "direction"], "properties": {"action": {"const": "nudge"}, "window": window, "direction": {"enum": ["left", "right", "up", "down"]}}},
            {"type": "object", "required": ["action", "workspace"], "properties": {"action": {"const": "switch_workspace"}, "workspace": {"type": "integer", "minimum": 1}}},
            {"type": "object", "required": ["action", "workspace"], "properties": {"action": {"const": "move_to_workspace"}, "window": window, "workspace": {"type": "integer", "minimum": 1}}},
            {"type": "object", "required": ["action", "layout"], "properties": {"action": {"const": "set_layout"}, "layout": {"enum": ["tall", "monocle"]}}},
            {"type": "object", "required": ["action"], "properties": {"action": {"const": "overview"}, "visible": {"type": "boolean"}}},
            {"type": "object", "required": ["action", "item"], "properties": {"action": {"const": "activate_menu"}, "window": window, "item": {"type": "string"}}},
            {"type": "object", "required": ["action", "op"], "properties": {"action": {"const": "session"}, "op": {"enum": ["lock", "suspend", "hibernate", "logout", "reboot", "power_off"]}, "confirmed": {"type": "boolean", "description": "Required for logout/reboot/power_off; only set after the user explicitly agreed"}}},
            {"type": "object", "required": ["action", "id"], "properties": {"action": {"const": "activate_tray"}, "id": {"type": "string"}, "item": {"type": "string"}}},
            {"type": "object", "required": ["action", "unit"], "properties": {"action": {"enum": ["restart_unit", "reset_failed"]}, "unit": {"type": "string", "description": "A unit listed in failed_units"}}}
        ]
    });
    json!({"tools": [
        {
            "name": "get_state",
            "description": "Describe the desktop: workspaces, windows (id, app, title, mode, frame, focus), overview, focused window's global menus, tray, app suggestions, failed systemd user units, clock and battery.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "dispatch",
            "description": "Apply desktop actions in order. Window actions without a window ID that follow a launch wait for that app's window.",
            "inputSchema": {"type": "object", "required": ["actions"], "properties": {"actions": {"type": "array", "items": action}}}
        },
        {
            "name": "ask",
            "description": "Let the built-in assistant interpret a short request such as 'open firefox and snap it left'.",
            "inputSchema": {"type": "object", "required": ["text"], "properties": {"text": {"type": "string"}}}
        }
    ]})
}

#[derive(Serialize)]
struct Applied {
    actions: Vec<Action>,
    effects: Vec<Effect>,
}

/// Handles one request line; returns the response line and host effects.
pub fn handle_line(shell: &mut Shell, line: &str) -> (String, Vec<Effect>) {
    let reply = |result: Result<Value, String>| match result {
        Ok(result) => json!({"ok": true, "result": result}).to_string(),
        Err(error) => json!({"ok": false, "error": error}).to_string(),
    };
    let request = match serde_json::from_str::<Request>(line) {
        Ok(r) => r,
        Err(e) => return (reply(Err(format!("bad request: {e}"))), Vec::new()),
    };
    let (result, effects) = match request {
        Request::State => (Ok(to_value(&state(shell))), Vec::new()),
        Request::Tools => (Ok(tools()), Vec::new()),
        Request::Dispatch { actions } => match shell.run(actions.clone()) {
            Ok(effects) => (
                Ok(to_value(&Applied {
                    actions,
                    effects: effects.clone(),
                })),
                effects,
            ),
            Err(e) => (Err(e.to_string()), Vec::new()),
        },
        Request::Ask { text } => match assistant::interpret(&text) {
            Ok(actions) => match shell.run(actions.clone()) {
                Ok(effects) => (
                    Ok(to_value(&Applied {
                        actions,
                        effects: effects.clone(),
                    })),
                    effects,
                ),
                Err(e) => (Err(e.to_string()), Vec::new()),
            },
            Err(e) => (Err(e.to_string()), Vec::new()),
        },
        Request::RegisterMenu { window, menus } => {
            shell.menus.register(window, menus);
            (Ok(Value::Null), Vec::new())
        }
    };
    (reply(result), effects)
}

fn to_value(value: &impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
