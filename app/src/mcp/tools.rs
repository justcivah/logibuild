//! The tools an agent can call. They use the same messages and history
//! commands as the user interface, never open dialogs, and edit the shown
//! circuit unless told otherwise.

use serde_json::{Map, Value, json};

use lb_core::circuit::{CompGeom, CompId, Component, PinRef, Wire};
use lb_core::component::layout::layout;
use lb_core::component::{Kind, PinDir, PropKey, Props};
use lb_core::history::Command;
use lb_core::value::Signal;

use crate::app::{App, Message, Modal, StepKind};
use crate::canvas::{EditorEvent, Tool};
use crate::commands;

pub type Result<T> = std::result::Result<T, String>;

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
    })
}

fn pin_schema() -> Value {
    json!({
        "type": "object",
        "description": "A pin: component id and pin name (see get_circuit).",
        "properties": { "id": { "type": "integer" }, "pin": { "type": "string" } },
        "required": ["id", "pin"],
    })
}

/// Every tool, as `tools/list` describes them.
pub fn list() -> Vec<Value> {
    let circuit = json!({ "type": "string", "description": "Circuit to show first (default: the shown one)." });
    let props = json!({
        "type": "object",
        "description": "Properties to set, by the names list_kinds gives, e.g. {\"width\": 8, \"label\": \"a\"}.",
    });
    vec![
        tool("get_state", "File, circuits, shown circuit, selection and simulation state.", json!({}), &[]),
        tool(
            "list_kinds",
            "Every component kind: its name for add_component, category, pins and properties.",
            json!({}),
            &[],
        ),
        tool(
            "get_circuit",
            "Components (id, kind, position, rotation, properties, pins with grid positions) and wires.",
            json!({ "circuit": circuit }),
            &[],
        ),
        tool(
            "add_component",
            "Places a component with its top-left corner at (x, y), in grid units; returns its id.",
            json!({
                "kind": { "type": "string", "description": "Kind from list_kinds, e.g. \"And\"." },
                "x": { "type": "integer" },
                "y": { "type": "integer" },
                "rot": { "type": "integer", "description": "Clockwise quarter turns, 0 to 3." },
                "mirror": { "type": "boolean" },
                "props": props,
                "circuit": circuit,
            }),
            &["kind", "x", "y"],
        ),
        tool(
            "update_component",
            "Moves, turns, flips or changes properties of a component.",
            json!({
                "id": { "type": "integer" },
                "x": { "type": "integer" },
                "y": { "type": "integer" },
                "rot": { "type": "integer" },
                "mirror": { "type": "boolean" },
                "props": props,
            }),
            &["id"],
        ),
        tool(
            "remove",
            "Removes components (with their wires) and wires.",
            json!({
                "components": { "type": "array", "items": { "type": "integer" } },
                "wires": {
                    "type": "array",
                    "items": { "type": "object", "properties": { "from": pin_schema(), "to": pin_schema() } },
                },
            }),
            &[],
        ),
        tool(
            "connect",
            "Connects two pins with a wire; it is routed automatically.",
            json!({ "from": pin_schema(), "to": pin_schema() }),
            &["from", "to"],
        ),
        tool(
            "new_circuit",
            "Adds a circuit to the project and shows it.",
            json!({ "name": { "type": "string" } }),
            &[],
        ),
        tool("open_circuit", "Shows a circuit.", json!({ "name": { "type": "string" } }), &["name"]),
        tool("rename_circuit", "Renames the shown circuit.", json!({ "name": { "type": "string" } }), &["name"]),
        tool("undo", "Undoes the last edit.", json!({}), &[]),
        tool("redo", "Redoes the last undone edit.", json!({}), &[]),
        tool(
            "save",
            "Saves the project to its file, or to `path` (.lbc).",
            json!({ "path": { "type": "string" } }),
            &[],
        ),
        tool(
            "set_input",
            "Sets the value of an Input component and settles the circuit. Values: 0, 1, X, Z, \
             binary digits, or 0x/0b prefixed numbers.",
            json!({ "id": { "type": "integer" }, "value": { "type": "string" } }),
            &["id", "value"],
        ),
        tool(
            "simulate",
            "run or pause the simulation in real time, reset it, step it `count` times (by `kind`: \
             event, delta, tick or clock), or run_for `count` ticks at once.",
            json!({
                "action": { "type": "string", "enum": ["run", "pause", "reset", "step", "run_for"] },
                "count": { "type": "integer", "minimum": 1 },
                "kind": { "type": "string", "enum": ["event", "delta", "tick", "clock"] },
            }),
            &["action"],
        ),
        tool(
            "read",
            "Values on the pins of components (default: every Input, Output and Probe), with the \
             simulation time.",
            json!({ "ids": { "type": "array", "items": { "type": "integer" } } }),
            &[],
        ),
        tool(
            "list_commands",
            "Command palette entries (menu commands, components, circuits) matching `query`.",
            json!({ "query": { "type": "string" } }),
            &[],
        ),
        tool(
            "run_command",
            "Runs a command palette entry by its label, or by a query matching only one entry.",
            json!({ "label": { "type": "string" } }),
            &["label"],
        ),
        tool("screenshot", "A PNG image of the window.", json!({}), &[]),
    ]
}

/// Runs a tool; `None` when there is no tool with this name.
pub fn call(app: &mut App, name: &str, args: &Value) -> Option<Result<Value>> {
    let edits = !matches!(name, "get_state" | "list_kinds" | "get_circuit" | "read" | "list_commands");
    if edits && app.modal.is_some() {
        return Some(Err("A dialog is open in LogiBuild: the user must close it first.".into()));
    }
    Some(match name {
        "get_state" => Ok(state(app)),
        "list_kinds" => Ok(kinds()),
        "get_circuit" => show(app, args).map(|()| circuit(app)),
        "add_component" => show(app, args).and_then(|()| add(app, args)),
        "update_component" => update(app, args),
        "remove" => remove(app, args),
        "connect" => connect(app, args),
        "new_circuit" => new_circuit(app, args),
        "open_circuit" => str_arg(args, "name")
            .and_then(|name| show(app, &json!({ "circuit": name })).map(|()| json!(format!("Showing “{name}”")))),
        "rename_circuit" => rename(app, args),
        "undo" => {
            let done = app.history.undo_label().map(|l| format!("Undone: {l}"));
            history(app, Message::Undo, done)
        }
        "redo" => {
            let done = app.history.redo_label().map(|l| format!("Redone: {l}"));
            history(app, Message::Redo, done)
        }
        "save" => save(app, args),
        "set_input" => set_input(app, args),
        "simulate" => simulate(app, args),
        "read" => read(app, args),
        "list_commands" => Ok(list_commands(app, args)),
        "run_command" => run_command(app, args),
        _ => return None,
    })
}

// ---- arguments ------------------------------------------------------------------------

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("Missing string argument “{key}”."))
}

fn int_arg(args: &Value, key: &str) -> Result<Option<i64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| format!("Argument “{key}” must be an integer.")),
    }
}

fn id_arg(args: &Value, key: &str) -> Result<CompId> {
    int_arg(args, key)?.and_then(|i| CompId::try_from(i).ok()).ok_or_else(|| format!("Missing component id “{key}”."))
}

fn pin_arg(app: &App, args: &Value, key: &str) -> Result<PinRef> {
    let p = args.get(key).ok_or_else(|| format!("Missing pin “{key}”."))?;
    let id = id_arg(p, "id")?;
    let pin = str_arg(p, "pin")?;
    let c = app.circuit().component(id).ok_or_else(|| format!("No component {id} in “{}”.", app.circuit().name))?;
    let l = app.project.layout(c);
    if l.pin(pin).is_none() {
        let names: Vec<&str> = l.pins.iter().map(|p| p.name.as_str()).collect();
        return Err(format!("Component {id} has no pin “{pin}”; its pins: {}.", names.join(", ")));
    }
    Ok(PinRef::new(id, pin))
}

fn component(app: &App, id: CompId) -> Result<&Component> {
    app.circuit().component(id).ok_or_else(|| format!("No component {id} in “{}”.", app.circuit().name))
}

/// The JSON name of a property: `PropKey::OutWidth` is `out_width`.
fn prop_name(key: PropKey) -> String {
    let mut out = String::new();
    for (i, ch) in format!("{key:?}").chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// `props` with the fields of `changes` set; only the properties of `kind`.
fn merge_props(kind: Kind, props: &Props, changes: Option<&Value>) -> Result<Props> {
    let Some(changes) = changes.filter(|v| !v.is_null()) else { return Ok(props.clone()) };
    let changes = changes.as_object().ok_or("“props” must be an object.")?;
    let mut allowed: Vec<String> = kind.prop_keys().iter().map(|k| prop_name(*k)).collect();
    if kind == Kind::Subcircuit {
        allowed.push("circuit".into());
    }
    let mut value = serde_json::to_value(props).map_err(|e| e.to_string())?;
    let fields = value.as_object_mut().ok_or("Unexpected properties.")?;
    for (k, v) in changes {
        if !allowed.contains(k) {
            return Err(format!("{} has no property “{k}”; its properties: {}.", kind.name(), allowed.join(", ")));
        }
        fields.insert(k.clone(), v.clone());
    }
    serde_json::from_value(value).map_err(|e| format!("Invalid properties: {e}"))
}

fn parse_kind(name: &str) -> Result<Kind> {
    serde_json::from_value(json!(name))
        .ok()
        .or_else(|| Kind::ALL.iter().copied().find(|k| k.name().eq_ignore_ascii_case(name)))
        .ok_or_else(|| format!("Unknown component kind “{name}”: see list_kinds."))
}

// ---- inspection -----------------------------------------------------------------------

fn state(app: &App) -> Value {
    let circuits: Vec<Value> = app
        .project
        .circuits
        .iter()
        .enumerate()
        .map(|(i, c)| {
            json!({
                "name": c.name,
                "main": i == 0,
                "read_only": c.hdl.is_some(),
                "components": c.components.len(),
                "wires": c.wires.len(),
            })
        })
        .collect();
    let tool = match &app.tool {
        Tool::Select => json!("select"),
        Tool::Place { kind, .. } => json!(format!("place {kind:?}")),
    };
    json!({
        "file": app.path.as_ref().map(|p| p.display().to_string()),
        "unsaved_changes": app.is_dirty(),
        "circuits": circuits,
        "shown_circuit": app.circuit().name,
        "selection": { "components": app.selection.comps, "wires": app.selection.wires.len() },
        "tool": tool,
        "simulation": app.sim().map(|s| json!({
            "running": app.running,
            "time": s.time(),
            "oscillating": s.oscillation().is_some(),
        })),
        "error": app.error,
    })
}

fn pins(geom: &CompGeom) -> Vec<Value> {
    geom.layout
        .pins
        .iter()
        .zip(&geom.pins)
        .map(|(p, pos)| {
            json!({
                "name": p.name,
                "dir": if p.dir == PinDir::In { "in" } else { "out" },
                "width": p.width,
                "x": pos.0,
                "y": pos.1,
            })
        })
        .collect()
}

fn kinds() -> Value {
    let kinds: Vec<Value> = Kind::ALL
        .iter()
        .copied()
        .filter(|k| *k != Kind::Subcircuit)
        .map(|k| {
            let props = k.default_props();
            let l = layout(k, &props, None);
            let pins: Vec<Value> = l
                .pins
                .iter()
                .map(|p| json!({ "name": p.name, "dir": if p.dir == PinDir::In { "in" } else { "out" }, "width": p.width }))
                .collect();
            let props: Vec<Value> =
                k.prop_keys().iter().map(|key| json!({ "name": prop_name(*key), "label": key.name() })).collect();
            json!({
                "kind": k,
                "name": k.name(),
                "category": k.category().name(),
                "size": [l.w, l.h],
                "pins": pins,
                "props": props,
            })
        })
        .collect();
    json!({
        "kinds": kinds,
        "subcircuits": "Place a circuit of the project with kind \"Subcircuit\" and props {\"circuit\": NAME}; \
                        its pins are its Input and Output components, named by their labels.",
    })
}

fn circuit(app: &App) -> Value {
    let c = app.circuit();
    let comps: Vec<Value> = c
        .components
        .iter()
        .map(|comp| {
            let geom = CompGeom::new(&app.project, comp);
            json!({
                "id": comp.id,
                "kind": comp.kind,
                "x": comp.pos.0,
                "y": comp.pos.1,
                "rot": comp.rot,
                "mirror": comp.mirror,
                "props": comp.props,
                "pins": pins(&geom),
            })
        })
        .collect();
    let wires: Vec<Value> = c
        .wires
        .iter()
        .map(|w| {
            json!({
                "from": { "id": w.from.0, "pin": w.from.1 },
                "to": { "id": w.to.0, "pin": w.to.1 },
            })
        })
        .collect();
    json!({ "name": c.name, "read_only": c.hdl.is_some(), "components": comps, "wires": wires })
}

/// Values on the pins of components; the default is every Input, Output
/// and Probe.
fn read(app: &App, args: &Value) -> Result<Value> {
    let ids: Vec<CompId> = match args.get("ids").and_then(Value::as_array) {
        Some(ids) => ids.iter().filter_map(|v| v.as_u64()).filter_map(|v| CompId::try_from(v).ok()).collect(),
        None => app
            .circuit()
            .components
            .iter()
            .filter(|c| matches!(c.kind, Kind::Input | Kind::Output | Kind::Probe))
            .map(|c| c.id)
            .collect(),
    };
    let sim = app.sim().ok_or_else(|| format!("No simulation: {}", app.error.clone().unwrap_or_default()))?;
    let inst = &app.loc().inst;
    let mut out = Vec::new();
    for id in ids {
        let c = component(app, id)?;
        let mut values = Map::new();
        for (i, p) in app.project.layout(c).pins.iter().enumerate() {
            let v = sim.netlist().pin_net(inst, id, i).map(|n| sim.net_value(n).to_string());
            values.insert(p.name.clone(), json!(v));
        }
        out.push(json!({ "id": id, "kind": c.kind, "label": c.props.label, "pins": values }));
    }
    Ok(json!({
        "time": sim.time(),
        "oscillation": sim.oscillation().map(|o| o.to_string()),
        "components": out,
    }))
}

// ---- editing --------------------------------------------------------------------------

/// Shows `args.circuit`, if given.
fn show(app: &mut App, args: &Value) -> Result<()> {
    let Some(name) = args.get("circuit").and_then(Value::as_str) else { return Ok(()) };
    if app.project.circuit(name).is_none() {
        return Err(format!("No circuit “{name}”."));
    }
    if app.circuit().name != name {
        let _ = app.update(Message::OpenCircuit(name.to_string()));
    }
    Ok(())
}

/// Takes the message of the dialog an action opened to tell why it failed,
/// closing it: the agent gets the reason, the user is not interrupted.
fn dialog(app: &mut App) -> Option<String> {
    match app.modal.take() {
        Some(Modal::Message(m)) => Some(m),
        other => {
            app.modal = other;
            None
        }
    }
}

/// Runs `cmd`; the error is the reason it was refused.
fn exec(app: &mut App, cmd: Command) -> Result<()> {
    let circuit = cmd.circuit.clone();
    if app.exec(cmd) {
        Ok(())
    } else if app.project.is_hdl(&circuit) {
        Err(format!(
            "“{circuit}” is synthesized from HDL and read-only: edit its sources, or convert it to an ordinary circuit."
        ))
    } else {
        Err(dialog(app).unwrap_or_else(|| "Nothing changed.".into()))
    }
}

fn add(app: &mut App, args: &Value) -> Result<Value> {
    let kind = parse_kind(str_arg(args, "kind")?)?;
    let x = int_arg(args, "x")?.ok_or("Missing “x”.")? as i32;
    let y = int_arg(args, "y")?.ok_or("Missing “y”.")? as i32;
    let props = merge_props(kind, &kind.default_props(), args.get("props"))?;
    if kind == Kind::Subcircuit {
        if app.project.circuit(&props.circuit).is_none() {
            return Err(format!("No circuit “{}” to place.", props.circuit));
        }
        if let Some(why) = app.placement_refusal(&props.circuit) {
            return Err(why);
        }
    }
    let c = Component {
        id: app.circuit().next_id(),
        kind,
        pos: (x, y),
        rot: (int_arg(args, "rot")?.unwrap_or(0).rem_euclid(4)) as u8,
        mirror: args.get("mirror").and_then(Value::as_bool).unwrap_or(false),
        props,
    };
    let id = c.id;
    exec(app, Command::add(app.circuit(), vec![c], vec![]))?;
    let geom = CompGeom::new(&app.project, component(app, id)?);
    Ok(json!({ "id": id, "pins": pins(&geom) }))
}

fn update(app: &mut App, args: &Value) -> Result<Value> {
    let id = id_arg(args, "id")?;
    let mut c = component(app, id)?.clone();
    if let Some(x) = int_arg(args, "x")? {
        c.pos.0 = x as i32;
    }
    if let Some(y) = int_arg(args, "y")? {
        c.pos.1 = y as i32;
    }
    if let Some(r) = int_arg(args, "rot")? {
        c.rot = r.rem_euclid(4) as u8;
    }
    if let Some(m) = args.get("mirror").and_then(Value::as_bool) {
        c.mirror = m;
    }
    c.props = merge_props(c.kind, &c.props, args.get("props"))?;
    let cmd = Command::update(&app.project, app.circuit(), vec![c]);
    exec(app, cmd)?;
    let geom = CompGeom::new(&app.project, component(app, id)?);
    Ok(json!({ "id": id, "pins": pins(&geom) }))
}

fn remove(app: &mut App, args: &Value) -> Result<Value> {
    let ids: Vec<CompId> = match args.get("components").and_then(Value::as_array) {
        Some(ids) => ids
            .iter()
            .map(|v| v.as_u64().and_then(|v| CompId::try_from(v).ok()).ok_or("Component ids must be integers."))
            .collect::<std::result::Result<_, _>>()?,
        None => Vec::new(),
    };
    for &id in &ids {
        component(app, id)?;
    }
    let mut wires = Vec::new();
    for w in args.get("wires").and_then(Value::as_array).into_iter().flatten() {
        let wire = Wire::new(pin_arg(app, w, "from")?, pin_arg(app, w, "to")?);
        let found = app.circuit().wires.iter().find(|x| x.same_ends(&wire)).cloned();
        wires.push(found.ok_or_else(|| format!("No wire between {:?} and {:?}.", wire.from, wire.to))?);
    }
    if ids.is_empty() && wires.is_empty() {
        return Err("Nothing to remove.".into());
    }
    app.selection.clear();
    exec(app, Command::delete(app.circuit(), &ids, &wires))?;
    Ok(json!(format!("Removed {} components and {} wires", ids.len(), wires.len())))
}

fn connect(app: &mut App, args: &Value) -> Result<Value> {
    let from = pin_arg(app, args, "from")?;
    let to = pin_arg(app, args, "to")?;
    if from == to {
        return Err("A pin cannot be connected to itself.".into());
    }
    let wires = app.circuit().wires.len();
    let _ = app.update(Message::Editor(EditorEvent::Connect { from, to }));
    if app.circuit().wires.len() == wires {
        return Err(dialog(app).map_or("Not connected.".into(), |m| format!("Not connected: {m}")));
    }
    Ok(json!("Connected"))
}

fn new_circuit(app: &mut App, args: &Value) -> Result<Value> {
    let _ = app.update(Message::NewCircuit);
    if let Some(name) = args.get("name").and_then(Value::as_str) {
        rename(app, &json!({ "name": name }))?;
    }
    Ok(json!(format!("Showing the new circuit “{}”", app.circuit().name)))
}

fn rename(app: &mut App, args: &Value) -> Result<Value> {
    let name = str_arg(args, "name")?.trim().to_string();
    if app.project.circuit(&name).is_some() {
        return Err(format!("There is already a circuit “{name}”."));
    }
    let _ = app.update(Message::RenameCircuit(name.clone()));
    app.drafts.remove("circuit.name");
    if app.circuit().name != name {
        return Err(dialog(app).map_or("Not renamed.".into(), |m| format!("Not renamed: {m}")));
    }
    Ok(json!(format!("Renamed to “{name}”")))
}

fn history(app: &mut App, msg: Message, done: Option<String>) -> Result<Value> {
    let done = done.ok_or("Nothing to do.")?;
    let _ = app.update(msg);
    Ok(json!(done))
}

fn save(app: &mut App, args: &Value) -> Result<Value> {
    let path = match args.get("path").and_then(Value::as_str) {
        Some(p) => std::path::PathBuf::from(p),
        None => app.path.clone().ok_or("The project has no file yet: give a “path”.")?,
    };
    if app.save_to(path.clone()) {
        Ok(json!(format!("Saved {}", path.display())))
    } else {
        Err(dialog(app).unwrap_or_else(|| "Cannot save.".into()))
    }
}

// ---- simulation -----------------------------------------------------------------------

fn set_input(app: &mut App, args: &Value) -> Result<Value> {
    let id = id_arg(args, "id")?;
    let value = str_arg(args, "value")?;
    let c = component(app, id)?;
    if c.kind != Kind::Input {
        return Err(format!("Component {id} is a {}, not an Input.", c.kind.name()));
    }
    let width = c.props.width();
    let signal = Signal::parse(width, value).ok_or_else(|| format!("“{value}” is not a value of {width} bits."))?;
    let inst = app.loc().inst.clone();
    let sim = app.sim_mut().ok_or("No simulation.")?;
    let i = sim.netlist().comp_index(&inst, id).ok_or("The input is not simulated.")?;
    let shown = signal.to_string();
    sim.set_input(i, signal);
    app.settle();
    Ok(json!(format!("Input {id} = {shown}")))
}

fn simulate(app: &mut App, args: &Value) -> Result<Value> {
    if app.sim().is_none() {
        return Err(format!("No simulation: {}", app.error.clone().unwrap_or_default()));
    }
    let count = int_arg(args, "count")?.unwrap_or(1).max(1);
    match str_arg(args, "action")? {
        "run" if !app.running => {
            let _ = app.update(Message::ToggleRun);
        }
        "pause" if app.running => {
            let _ = app.update(Message::ToggleRun);
        }
        "run" | "pause" => {}
        "reset" => {
            let _ = app.update(Message::Reset);
            app.settle();
        }
        "step" => {
            let kind = match args.get("kind").and_then(Value::as_str) {
                None => app.step_kind,
                Some("event") => StepKind::Event,
                Some("delta") => StepKind::Delta,
                Some("tick") => StepKind::Tick,
                Some("clock") => StepKind::Clock,
                Some(k) => return Err(format!("Unknown step kind “{k}”.")),
            };
            let previous = std::mem::replace(&mut app.step_kind, kind);
            for _ in 0..count.min(1_000_000) {
                let _ = app.update(Message::Step);
                if app.sim().is_some_and(|s| s.oscillation().is_some()) {
                    break;
                }
            }
            app.step_kind = previous;
        }
        "run_for" => {
            let sim = app.sim_mut().ok_or("No simulation.")?;
            let t = sim.time() + count as u64;
            if let Err(e) = sim.run_until(t) {
                let e = e.to_string();
                app.settle();
                return Err(format!("Oscillation: {e}"));
            }
        }
        a => return Err(format!("Unknown action “{a}”.")),
    }
    Ok(json!({
        "running": app.running,
        "time": app.sim().map(|s| s.time()),
        "oscillating": app.sim().is_some_and(|s| s.oscillation().is_some()),
    }))
}

// ---- command palette ------------------------------------------------------------------

fn list_commands(app: &App, args: &Value) -> Value {
    let query = args.get("query").and_then(Value::as_str).unwrap_or_default();
    let found: Vec<Value> = commands::filter(commands::entries(app), query)
        .into_iter()
        .map(|e| json!({ "label": e.label, "key": e.key, "enabled": e.enabled }))
        .collect();
    json!(found)
}

fn run_command(app: &mut App, args: &Value) -> Result<Value> {
    let label = str_arg(args, "label")?;
    let entries = commands::entries(app);
    let entry = match entries.iter().find(|e| e.label.eq_ignore_ascii_case(label)) {
        Some(e) => e.clone(),
        None => {
            let found = commands::filter(entries, label);
            match found.as_slice() {
                [e] => e.clone(),
                [] => return Err(format!("No command matches “{label}”.")),
                many => {
                    let labels: Vec<&str> = many.iter().take(10).map(|e| e.label.as_str()).collect();
                    return Err(format!("“{label}” matches several commands: {}.", labels.join("; ")));
                }
            }
        }
    };
    if !entry.enabled {
        return Err(format!("“{}” is not available now.", entry.label));
    }
    // Commands that would open a file dialog or quit belong to the user.
    let picker = matches!(entry.msg, Message::Save) && app.path.is_none();
    if picker || matches!(entry.msg, Message::Open | Message::SaveAs | Message::Quit | Message::New) {
        return Err(format!("“{}” needs the user: use the save tool, or ask the user.", entry.label));
    }
    let _ = app.update(entry.msg);
    match dialog(app) {
        Some(why) => Err(format!("“{}” failed: {why}", entry.label)),
        None => Ok(json!(format!("Ran “{}”", entry.label))),
    }
}
