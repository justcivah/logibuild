//! Model Context Protocol server: lets an AI agent drive the running
//! application (optional `mcp` feature).
//!
//! The application listens on a local socket (a Unix domain socket, or a
//! named pipe on Windows) when started with `--mcp`. MCP clients start their
//! servers as processes speaking over stdio, so `logibuild --mcp-bridge`
//! relays its stdin and stdout to that socket, starting the application when
//! none is listening. Messages are newline-delimited JSON-RPC 2.0; they are
//! answered by [`App::update`] like any other message, so every edit goes
//! through the undo history.

pub mod tools;

use std::io::{self, BufRead, BufReader, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use iced::futures::channel::mpsc as channel;
use iced::{Subscription, Task, window};
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{ListenerOptions, Name};
use serde_json::{Value, json};

use crate::app::{App, Message};

/// Socket name used when `--mcp` and `--mcp-bridge` are given none.
pub const DEFAULT_NAME: &str = "logibuild";

const PROTOCOL_VERSION: &str = "2025-06-18";

const INSTRUCTIONS: &str = "LogiBuild is a digital logic circuit editor and simulator. \
Coordinates are grid units. Call list_kinds for the component types and their pins, get_circuit \
for what the shown circuit contains, then add_component and connect to build. Edits are undoable. \
Use set_input, simulate and read to test a circuit, and screenshot to see the window.";

/// What the listener reports to the application.
#[derive(Debug, Clone)]
pub enum Event {
    /// The socket is open.
    Listening,
    Failed(String),
    /// A client connected (`true`) or left (`false`).
    Connected(bool),
    Call(Call),
    /// The window was drawn, while screenshots wait.
    Frame,
}

/// One line received from the client, with the way to answer it.
#[derive(Debug, Clone)]
pub struct Call {
    pub line: String,
    pub reply: Reply,
}

/// Sends the answer to a call back to the listener; `None` for
/// notifications, which get none. Only the first answer is sent.
#[derive(Clone)]
pub struct Reply(Arc<Mutex<Option<mpsc::Sender<Option<Value>>>>>);

impl Reply {
    pub fn new(tx: mpsc::Sender<Option<Value>>) -> Self {
        Reply(Arc::new(Mutex::new(Some(tx))))
    }

    pub fn send(&self, answer: Option<Value>) {
        if let Some(tx) = self.0.lock().ok().and_then(|mut tx| tx.take()) {
            let _ = tx.send(answer);
        }
    }
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reply")
    }
}

/// State of the server while the application listens.
#[derive(Debug, Default)]
pub struct Server {
    pub name: String,
    pub connected: bool,
    /// Screenshots asked for, with the frames to wait before taking them,
    /// so that the window shows the last edits.
    pub shots: Vec<(Value, Reply, u8)>,
}

// ---- socket ---------------------------------------------------------------------------

/// The socket for `name`: a file in the runtime directory on Unix, so that
/// only the user can connect, and a named pipe on Windows.
fn socket_name(name: &str) -> io::Result<Name<'static>> {
    #[cfg(unix)]
    {
        use interprocess::local_socket::GenericFilePath;
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        dir.join(format!("{name}.sock")).to_fs_name::<GenericFilePath>()
    }
    #[cfg(not(unix))]
    {
        use interprocess::local_socket::GenericNamespaced;
        name.to_string().to_ns_name::<GenericNamespaced>()
    }
}

/// Where the socket for `name` is, for messages.
pub fn socket_path(name: &str) -> String {
    #[cfg(unix)]
    {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        dir.join(format!("{name}.sock")).display().to_string()
    }
    #[cfg(not(unix))]
    {
        format!(r"\\.\pipe\{name}")
    }
}

/// Listens on `name`. A socket file left by a crashed instance is replaced;
/// one that an instance still listens on is not.
fn bind(name: &str) -> io::Result<LocalSocketListener> {
    let options = || -> io::Result<ListenerOptions<'static>> {
        let options = ListenerOptions::new().name(socket_name(name)?);
        #[cfg(unix)]
        let options = {
            use interprocess::os::unix::local_socket::ListenerOptionsExt;
            options.mode(0o600)
        };
        Ok(options)
    };
    match options()?.create_sync() {
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            if LocalSocketStream::connect(socket_name(name)?).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another LogiBuild already listens on {}", socket_path(name)),
                ));
            }
            options()?.try_overwrite(true).create_sync()
        }
        r => r,
    }
}

/// The listener as a subscription: answers come back from [`App::update`].
pub fn subscription(server: &Server) -> Subscription<Message> {
    let listener = Subscription::run_with(server.name.clone(), |name| {
        let (tx, rx) = channel::unbounded();
        let name = name.clone();
        std::thread::spawn(move || listen(&name, &tx));
        rx
    });
    if server.shots.is_empty() {
        listener
    } else {
        Subscription::batch([listener, window::frames().map(|_| Message::Mcp(Event::Frame))])
    }
}

/// Counts a drawn frame; takes the screenshots that waited long enough.
pub fn frame(server: &mut Server) -> Task<Message> {
    let mut tasks = Vec::new();
    server.shots.retain_mut(|(id, reply, wait)| {
        *wait = wait.saturating_sub(1);
        if *wait == 0 {
            tasks.push(screenshot(id.clone(), reply.clone()));
        }
        *wait > 0
    });
    Task::batch(tasks)
}

fn listen(name: &str, tx: &channel::UnboundedSender<Message>) {
    let send = |e: Event| tx.unbounded_send(Message::Mcp(e)).is_ok();
    let listener = match bind(name) {
        Ok(l) => l,
        Err(e) => {
            send(Event::Failed(e.to_string()));
            return;
        }
    };
    send(Event::Listening);
    // One client at a time: the next waits until the previous one leaves.
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        if !send(Event::Connected(true)) {
            return;
        }
        let _ = serve(conn, tx);
        if !send(Event::Connected(false)) {
            return;
        }
    }
}

/// Passes the lines of a client to the application, one at a time, and
/// writes the answers back.
fn serve(conn: LocalSocketStream, tx: &channel::UnboundedSender<Message>) -> io::Result<()> {
    let mut send = &conn;
    for line in BufReader::new(&conn).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let (rtx, rrx) = mpsc::channel();
        if tx.unbounded_send(Message::Mcp(Event::Call(Call { line, reply: Reply::new(rtx) }))).is_err() {
            return Ok(());
        }
        match rrx.recv() {
            Ok(Some(answer)) => {
                send.write_all(format!("{answer}\n").as_bytes())?;
                send.flush()?;
            }
            Ok(None) => {}
            Err(_) => return Ok(()),
        }
    }
    Ok(())
}

// ---- bridge ---------------------------------------------------------------------------

/// `logibuild --mcp-bridge`: relays stdin and stdout to the application
/// listening on `name`, starting it when none is.
pub fn bridge(name: &str) -> io::Result<()> {
    let conn = match LocalSocketStream::connect(socket_name(name)?) {
        Ok(c) => c,
        Err(_) => {
            std::process::Command::new(std::env::current_exe()?)
                .arg(format!("--mcp={name}"))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            let mut tries = 0;
            loop {
                std::thread::sleep(Duration::from_millis(200));
                match LocalSocketStream::connect(socket_name(name)?) {
                    Ok(c) => break c,
                    Err(e) if tries >= 75 => return Err(e),
                    Err(_) => tries += 1,
                }
            }
        }
    };
    let conn = std::sync::Arc::new(conn);
    let recv = conn.clone();
    std::thread::spawn(move || {
        let _ = io::copy(&mut &*recv, &mut io::stdout());
        // The application closed: nothing more to relay.
        std::process::exit(0);
    });
    io::copy(&mut io::stdin().lock(), &mut &*conn)?;
    Ok(())
}

// ---- protocol -------------------------------------------------------------------------

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// The result of a tool call: its value as JSON text, or the error.
pub fn tool_result(r: tools::Result<Value>) -> Value {
    match r {
        Ok(v) => {
            let text = if let Value::String(s) = v { s } else { serde_json::to_string_pretty(&v).unwrap_or_default() };
            json!({ "content": [{ "type": "text", "text": text }], "isError": false })
        }
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

/// Answers one JSON-RPC message; the screenshot is answered later, by
/// [`screenshot_taken`].
pub fn handle(app: &mut App, call: Call) -> Task<Message> {
    let request: Value = match serde_json::from_str(&call.line) {
        Ok(v) => v,
        Err(e) => {
            call.reply.send(Some(error(Value::Null, -32700, format!("Parse error: {e}"))));
            return Task::none();
        }
    };
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    // Notifications, and answers to requests this server never sends.
    let Some(id) = id else {
        call.reply.send(None);
        return Task::none();
    };
    let answer = match method {
        "initialize" => {
            let version = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
            result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "logibuild", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({ "tools": tools::list() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if name == "screenshot" {
                match &mut app.mcp {
                    // Taken once the window shows the last edits.
                    Some(server) => server.shots.push((id, call.reply, 2)),
                    None => call.reply.send(Some(result(id, tool_result(Err("Not listening.".into()))))),
                }
                return Task::none();
            }
            match tools::call(app, name, &args) {
                Some(r) => result(id, tool_result(r)),
                None => error(id, -32602, format!("Unknown tool: {name}")),
            }
        }
        _ => error(id, -32601, format!("Method not found: {method}")),
    };
    call.reply.send(Some(answer));
    Task::none()
}

fn screenshot(id: Value, reply: Reply) -> Task<Message> {
    window::latest()
        .then(|w| match w {
            Some(w) => window::screenshot(w).map(Some),
            None => Task::done(None),
        })
        .map(move |shot| Message::McpScreenshot(id.clone(), reply.clone(), shot))
}

/// Answers a screenshot request with the window as a PNG image.
pub fn screenshot_taken(id: Value, reply: Reply, shot: Option<window::Screenshot>) {
    let encoded = shot.ok_or_else(|| "No window to take a screenshot of.".to_string()).and_then(|s| png(&s));
    let answer = match encoded {
        Ok(data) => {
            json!({ "content": [{ "type": "image", "data": data, "mimeType": "image/png" }], "isError": false })
        }
        Err(e) => tool_result(Err(e)),
    };
    reply.send(Some(result(id, answer)));
}

/// The screenshot as a base64-encoded PNG.
fn png(shot: &window::Screenshot) -> tools::Result<String> {
    use base64::Engine;
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, shot.size.width, shot.size.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(&shot.rgba))
        .map_err(|e| format!("Cannot encode the screenshot: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests;
