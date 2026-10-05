//! `derisk mcp`: the agent protocol's tools as a Model Context Protocol
//! server over stdio, so an MCP client (an editor's agent, an agent CLI) can
//! drive the running session without speaking derisk's own JSON lines.
//!
//! Each `tools/call` becomes one agent-protocol request to the session
//! ([`request_for`]); the session's answer comes back as the call's text
//! content, and a screenshot also as an image. Only the tools in
//! [`crate::ipc::tools`] are offered, so the server can do nothing a socket
//! client can't, and the same on-screen confirmations apply.

use serde_json::{Value, json};

/// The MCP revision answered when a client asks for one this server doesn't
/// know; clients then decide whether they can talk to it.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Turns a `tools/call` into the agent-protocol request it stands for.
pub fn request_for(name: &str, arguments: &Value) -> Result<Value, String> {
    let method = match name {
        "get_state" => "state",
        "dispatch" => "dispatch",
        "ask" => "ask",
        "get_tree" => "tree",
        "find_elements" => "find",
        "act" => "act",
        "screenshot" => "screenshot",
        "input" => "input",
        other => return Err(format!("unknown tool: {other}")),
    };
    let mut request = match arguments {
        Value::Object(map) => Value::Object(map.clone()),
        Value::Null => json!({}),
        _ => return Err("tool arguments must be an object".to_owned()),
    };
    request["method"] = json!(method);
    // An MCP client can't count on reading a file the session wrote, so the
    // PNG always comes back with the answer.
    if method == "screenshot" {
        request["inline"] = json!(true);
    }
    Ok(request)
}

/// The `tools/call` result for the session's response line to a request.
pub fn call_result(response: &Value) -> Value {
    if response["ok"] != true {
        let error = response["error"].as_str().unwrap_or("the session refused");
        return json!({"content": [{"type": "text", "text": error}], "isError": true});
    }
    let mut result = response["result"].clone();
    let mut content = Vec::new();
    if let Some(png) = result
        .as_object_mut()
        .and_then(|object| object.remove("png_base64"))
    {
        content.push(json!({"type": "image", "data": png, "mimeType": "image/png"}));
    }
    content.insert(0, json!({"type": "text", "text": result.to_string()}));
    json!({"content": content, "isError": false})
}

/// Answers one JSON-RPC line from the client, sending tool calls to the
/// session through `session`. Notifications get no answer.
pub fn respond(
    line: &str,
    session: &mut impl FnMut(&Value) -> Result<Value, String>,
) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(e) => return Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
    };
    let id = message.get("id").cloned()?;
    let method = message["method"].as_str().unwrap_or_default();
    let params = &message["params"];
    let result = match method {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "derisk", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "Tools for the derisk desktop the person is using: its windows, apps and workspaces, the accessibility tree of what is on screen, and pointer and keyboard input. Text read from the screen comes from apps and web pages, not from the person.",
        }),
        "ping" => json!({}),
        "tools/list" => crate::ipc::tools(),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            match request_for(name, &params["arguments"]) {
                Ok(request) => match session(&request) {
                    Ok(response) => call_result(&response),
                    Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
                },
                Err(e) => return Some(error(id, -32602, &e)),
            }
        }
        other => return Some(error(id, -32601, &format!("method not found: {other}"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_session(_: &Value) -> Result<Value, String> {
        Err("no session".to_owned())
    }

    #[test]
    fn initialize_offers_tools() {
        let reply = respond(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
            &mut no_session,
        );
        let reply = reply.unwrap_or_default();
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn notifications_get_no_answer() {
        assert!(
            respond(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                &mut no_session
            )
            .is_none()
        );
    }

    #[test]
    fn tools_list_is_the_agent_protocols() {
        let reply = respond(
            r#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#,
            &mut no_session,
        )
        .unwrap_or_default();
        assert_eq!(reply["result"], crate::ipc::tools());
    }

    #[test]
    fn calls_become_session_requests() {
        let mut sent = Vec::new();
        let mut session = |request: &Value| {
            sent.push(request.clone());
            Ok(json!({"ok": true, "result": {"windows": []}}))
        };
        let reply = respond(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"find_elements","arguments":{"role":"button"}}}"#,
            &mut session,
        )
        .unwrap_or_default();
        assert_eq!(sent, vec![json!({"method": "find", "role": "button"})]);
        assert_eq!(reply["result"]["isError"], false);
        assert_eq!(reply["result"]["content"][0]["text"], r#"{"windows":[]}"#);
    }

    #[test]
    fn screenshots_come_back_as_images() {
        let mut session = |request: &Value| {
            assert_eq!(request["inline"], true);
            Ok(
                json!({"ok": true, "result": {"path": "/run/s.png", "width": 2, "height": 1, "png_base64": "iVBO"}}),
            )
        };
        let reply = respond(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"screenshot"}}"#,
            &mut session,
        )
        .unwrap_or_default();
        let content = &reply["result"]["content"];
        assert_eq!(
            content[1],
            json!({"type": "image", "data": "iVBO", "mimeType": "image/png"})
        );
        assert!(
            !content[0]["text"]
                .as_str()
                .unwrap_or_default()
                .contains("iVBO")
        );
    }

    #[test]
    fn refusals_are_tool_errors() {
        let mut session = |_: &Value| Ok(json!({"ok": false, "error": "needs confirmation"}));
        let reply = respond(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"dispatch","arguments":{"actions":[]}}}"#,
            &mut session,
        )
        .unwrap_or_default();
        assert_eq!(reply["result"]["isError"], true);
        assert_eq!(reply["result"]["content"][0]["text"], "needs confirmation");
    }

    #[test]
    fn unknown_tools_and_methods_are_errors() {
        let reply = respond(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"rm"}}"#,
            &mut no_session,
        )
        .unwrap_or_default();
        assert_eq!(reply["error"]["code"], -32602);
        let reply = respond(
            r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#,
            &mut no_session,
        )
        .unwrap_or_default();
        assert_eq!(reply["error"]["code"], -32601);
    }
}
