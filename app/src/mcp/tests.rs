//! Tests of the MCP server: the JSON-RPC messages are answered headless by
//! `App::update`, like the other app tests; one test goes through a socket.

use std::sync::mpsc;

use serde_json::{Value, json};

use super::{Call, Event, Reply};
use crate::app::{App, Message};

fn app() -> App {
    App::new(None, false, None).0
}

/// Sends one JSON-RPC message and returns the answer, if any.
fn send(app: &mut App, request: Value) -> Option<Value> {
    let (tx, rx) = mpsc::channel();
    let _ = app.update(Message::Mcp(Event::Call(Call { line: request.to_string(), reply: Reply::new(tx) })));
    rx.try_recv().expect("answered at once")
}

/// Calls a tool; returns its JSON value, or the error text.
fn tool(app: &mut App, name: &str, args: Value) -> Result<Value, String> {
    let answer = send(
        app,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": args } }),
    )
    .expect("a request is answered");
    let result = &answer["result"];
    let text = result["content"][0]["text"].as_str().expect("text content").to_string();
    if result["isError"] == json!(true) {
        Err(text)
    } else {
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

fn add(app: &mut App, kind: &str, x: i32, y: i32) -> u64 {
    let r = tool(app, "add_component", json!({ "kind": kind, "x": x, "y": y })).unwrap();
    r["id"].as_u64().unwrap()
}

#[test]
fn protocol_handshake_and_errors() {
    let mut app = app();
    let init = send(
        &mut app,
        json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": { "protocolVersion": "2025-03-26", "capabilities": {} } }),
    )
    .unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    assert_eq!(send(&mut app, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })), None);

    let list = send(&mut app, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).unwrap();
    let names: Vec<&str> =
        list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for name in
        ["get_circuit", "add_component", "connect", "set_input", "simulate", "read", "run_command", "screenshot"]
    {
        assert!(names.contains(&name), "{name} in {names:?}");
    }

    let unknown = send(&mut app, json!({ "jsonrpc": "2.0", "id": 2, "method": "nope" })).unwrap();
    assert_eq!(unknown["error"]["code"], -32601);
    let bad =
        send(&mut app, json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "nope" } }));
    assert_eq!(bad.unwrap()["error"]["code"], -32602);
    let (tx, rx) = mpsc::channel();
    let _ = app.update(Message::Mcp(Event::Call(Call { line: "{oops".into(), reply: Reply::new(tx) })));
    assert_eq!(rx.try_recv().unwrap().unwrap()["error"]["code"], -32700);
}

#[test]
fn build_simulate_and_undo() {
    let mut app = app();
    let a = add(&mut app, "Input", 0, 0);
    let b = add(&mut app, "Input", 0, 6);
    let and = add(&mut app, "and", 8, 2);
    let out = add(&mut app, "Output", 16, 3);
    for (from, to) in [((a, "out"), (and, "in0")), ((b, "out"), (and, "in1")), ((and, "out"), (out, "in"))] {
        let r = tool(
            &mut app,
            "connect",
            json!({ "from": { "id": from.0, "pin": from.1 }, "to": { "id": to.0, "pin": to.1 } }),
        );
        r.unwrap();
    }
    let circuit = tool(&mut app, "get_circuit", json!({})).unwrap();
    assert_eq!(circuit["components"].as_array().unwrap().len(), 4);
    assert_eq!(circuit["wires"].as_array().unwrap().len(), 3);

    let bad =
        tool(&mut app, "connect", json!({ "from": { "id": a, "pin": "nope" }, "to": { "id": out, "pin": "in" } }));
    assert!(bad.unwrap_err().contains("no pin"));

    let value = |app: &mut App| {
        let r = tool(app, "read", json!({ "ids": [out] })).unwrap();
        r["components"][0]["pins"]["in"].as_str().unwrap().to_string()
    };
    tool(&mut app, "set_input", json!({ "id": a, "value": "1" })).unwrap();
    assert_eq!(value(&mut app), "0");
    tool(&mut app, "set_input", json!({ "id": b, "value": "1" })).unwrap();
    assert_eq!(value(&mut app), "1");
    assert!(tool(&mut app, "set_input", json!({ "id": and, "value": "1" })).is_err());

    let before = tool(&mut app, "simulate", json!({ "action": "run_for", "count": 10 })).unwrap();
    assert!(before["time"].as_u64().unwrap() >= 10);

    tool(&mut app, "undo", json!({})).unwrap();
    assert_eq!(app.circuit().wires.len(), 2);
    tool(&mut app, "remove", json!({ "components": [and] })).unwrap();
    assert_eq!(app.circuit().components.len(), 3);
    assert!(app.circuit().wires.is_empty());
}

#[test]
fn properties_and_circuits() {
    let mut app = app();
    let id = add(&mut app, "And", 0, 0);
    tool(&mut app, "update_component", json!({ "id": id, "x": 4, "props": { "inputs": 3, "width": 8 } })).unwrap();
    let c = app.circuit().component(id as u32).unwrap();
    assert_eq!((c.pos, c.props.inputs, c.props.width), ((4, 0), 3, 8));
    let err = tool(&mut app, "update_component", json!({ "id": id, "props": { "colour": 1 } })).unwrap_err();
    assert!(err.contains("no property"), "{err}");

    tool(&mut app, "new_circuit", json!({ "name": "Adder" })).unwrap();
    assert_eq!(app.circuit().name, "Adder");
    tool(&mut app, "open_circuit", json!({ "name": "Main" })).unwrap();
    let sub = tool(
        &mut app,
        "add_component",
        json!({ "kind": "Subcircuit", "x": 0, "y": 10,
        "props": { "circuit": "Adder" } }),
    );
    sub.unwrap();
    let main = tool(
        &mut app,
        "add_component",
        json!({ "kind": "Subcircuit", "x": 0, "y": 20,
        "props": { "circuit": "Main" } }),
    );
    assert!(main.is_err());
}

#[test]
fn palette_commands() {
    let mut app = app();
    add(&mut app, "And", 0, 0);
    let found = tool(&mut app, "list_commands", json!({ "query": "select all" })).unwrap();
    assert_eq!(found[0]["label"], "Edit: Select all");
    tool(&mut app, "run_command", json!({ "label": "Edit: Select all" })).unwrap();
    assert_eq!(app.selection.comps.len(), 1);
    assert!(tool(&mut app, "run_command", json!({ "label": "Edit: Redo" })).unwrap_err().contains("not available"));
    assert!(tool(&mut app, "run_command", json!({ "label": "File: Quit" })).is_err());
    assert!(tool(&mut app, "run_command", json!({ "label": "align" })).unwrap_err().contains("several"));
}

#[test]
fn socket_round_trip() {
    use interprocess::local_socket::prelude::*;
    use std::io::{BufRead, BufReader, Write};

    let name = format!("logibuild-test-{}", std::process::id());
    let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
    let listener_name = name.clone();
    std::thread::spawn(move || super::listen(&listener_name, &tx));
    let next = |rx: &mut iced::futures::channel::mpsc::UnboundedReceiver<Message>| {
        for _ in 0..500 {
            if let Ok(Message::Mcp(e)) = rx.try_recv() {
                return e;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("no event from the listener");
    };
    assert!(matches!(next(&mut rx), Event::Listening));

    let conn = LocalSocketStream::connect(super::socket_name(&name).unwrap()).unwrap();
    (&conn).write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n").unwrap();
    assert!(matches!(next(&mut rx), Event::Connected(true)));
    let Event::Call(call) = next(&mut rx) else { panic!("expected a call") };
    let mut app = app();
    let _ = super::handle(&mut app, call);
    let mut line = String::new();
    BufReader::new(&conn).read_line(&mut line).unwrap();
    let answer: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(answer, json!({ "jsonrpc": "2.0", "id": 7, "result": {} }));

    // A second instance does not take over the socket.
    assert!(super::bind(&name).is_err());
}

#[test]
fn screenshot_waits_for_the_window_to_be_drawn() {
    let mut app = app();
    app.mcp = Some(super::Server { name: "unused".into(), ..Default::default() });
    let (tx, rx) = mpsc::channel();
    let request = json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "screenshot" } });
    let _ = app.update(Message::Mcp(Event::Call(Call { line: request.to_string(), reply: Reply::new(tx) })));
    assert!(rx.try_recv().is_err(), "not answered before the window is drawn");
    let _ = app.update(Message::Mcp(Event::Frame));
    assert_eq!(app.mcp.as_ref().unwrap().shots.len(), 1);
    let _ = app.update(Message::Mcp(Event::Frame));
    assert!(app.mcp.as_ref().unwrap().shots.is_empty(), "taken after two frames");
}
