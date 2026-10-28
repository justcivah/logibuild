//! Headless tests of the editor logic: they drive `App::update` with the same
//! messages the canvas and the panels produce, without opening a window.

use std::time::Instant;

use lb_core::circuit::PinRef;
use lb_core::component::Kind;
use lb_core::value::Signal;

use crate::app::{App, Message};
use crate::canvas::EditorEvent;
use crate::props::Edit;

fn app() -> App {
    App::new(None, false, None).0
}

fn ev(app: &mut App, e: EditorEvent) {
    let _ = app.update(Message::Editor(e));
}

fn place(app: &mut App, kind: Kind, pos: (i32, i32)) -> u32 {
    let _ = app.update(Message::PickTool(kind));
    ev(app, EditorEvent::Place { pos });
    escape(app);
    *app.selection.comps.iter().next().expect("placed component is selected")
}

fn escape(app: &mut App) {
    press_named(app, iced::keyboard::key::Named::Escape);
}

fn press_named(app: &mut App, key: iced::keyboard::key::Named) {
    let _ = app.update(Message::Key(iced::keyboard::Event::KeyPressed {
        key: iced::keyboard::Key::Named(key),
        modified_key: iced::keyboard::Key::Named(key),
        physical_key: iced::keyboard::key::Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
        location: iced::keyboard::Location::Standard,
        modifiers: iced::keyboard::Modifiers::empty(),
        text: None,
        repeat: false,
    }));
}

fn connect(app: &mut App, a: (u32, &str), b: (u32, &str)) {
    ev(app, EditorEvent::Connect { from: PinRef::new(a.0, a.1), to: PinRef::new(b.0, b.1) });
}

fn settle(app: &mut App) {
    for _ in 0..50 {
        let _ = app.update(Message::Frame(Instant::now()));
    }
}

fn pin_value(app: &App, comp: u32, pin: usize) -> Option<Signal> {
    let sim = app.sim()?;
    let net = sim.netlist().pin_net(&app.loc().inst, comp, pin)?;
    Some(sim.net_value(net))
}

/// A NOT gate fed by a switch, read by an LED.
fn inverter(app: &mut App) -> (u32, u32, u32) {
    let i = place(app, Kind::Input, (0, 0));
    let n = place(app, Kind::Not, (5, 0));
    let o = place(app, Kind::Led, (11, 0));
    connect(app, (i, "out"), (n, "in"));
    // Drawn backwards: the editor orients it from the output.
    connect(app, (o, "in"), (n, "out"));
    (i, n, o)
}

#[test]
fn build_and_simulate_an_inverter() {
    let mut app = app();
    let (i, n, _) = inverter(&mut app);
    assert_eq!(app.circuit().wires.len(), 2);
    assert!(app.circuit().wires.iter().any(|w| w.from == PinRef::new(n, "out")), "wire oriented from the output");
    settle(&mut app);
    assert_eq!(pin_value(&app, n, 1), Some(Signal::bool(true)));
    ev(&mut app, EditorEvent::Poke { id: i });
    settle(&mut app);
    assert_eq!(pin_value(&app, n, 1), Some(Signal::bool(false)));
}

#[test]
fn undo_redo_and_dirty_state() {
    let mut app = app();
    assert!(!app.is_dirty());
    inverter(&mut app);
    assert!(app.is_dirty());
    let n = app.circuit().components.len() + app.circuit().wires.len();
    for _ in 0..n {
        let _ = app.update(Message::Undo);
    }
    assert!(app.circuit().components.is_empty());
    assert!(!app.is_dirty());
    let _ = app.update(Message::Redo);
    assert_eq!(app.circuit().components.len(), 1);
}

#[test]
fn move_rotate_delete_selection() {
    let mut app = app();
    let (i, n, o) = inverter(&mut app);
    ev(&mut app, EditorEvent::SelectRect { from: (-1.0, -1.0), to: (30.0, 10.0), additive: false });
    assert_eq!(app.selection.comps.len(), 3);
    assert_eq!(app.selection.wires.len(), 2);
    ev(&mut app, EditorEvent::MoveSelection { dx: 3, dy: 4 });
    assert_eq!(app.circuit().component(n).unwrap().pos, (8, 4));
    let _ = app.update(Message::Rotate);
    assert!(app.circuit().components.iter().all(|c| c.rot == 1));
    ev(&mut app, EditorEvent::Select { id: o, additive: false });
    let _ = app.update(Message::Delete);
    assert!(app.circuit().component(o).is_none());
    assert_eq!(app.circuit().wires.len(), 1, "the LED wire went with it");
    let _ = app.update(Message::Undo);
    assert_eq!(app.circuit().wires.len(), 2);
    assert!(app.circuit().component(i).is_some());
}

#[test]
fn copy_paste_and_duplicate() {
    let mut app = app();
    inverter(&mut app);
    let _ = app.update(Message::SelectAll);
    let _ = app.update(Message::Copy);
    let _ = app.update(Message::Paste);
    assert_eq!(app.circuit().components.len(), 6);
    assert_eq!(app.circuit().wires.len(), 4, "internal wires are copied");
    let _ = app.update(Message::Duplicate);
    assert_eq!(app.circuit().components.len(), 9);
    let _ = app.update(Message::Undo);
    assert_eq!(app.circuit().components.len(), 6);
}

#[test]
fn property_edits_merge_into_one_undo_step() {
    let mut app = app();
    let i = place(&mut app, Kind::Input, (0, 0));
    ev(&mut app, EditorEvent::Select { id: i, additive: false });
    let before = app.history.len();
    for s in ["c", "cl", "clk"] {
        let _ = app.update(Message::Prop(Edit::Text(lb_core::component::PropKey::Label, s.into())));
    }
    assert_eq!(app.circuit().component(i).unwrap().props.label, "clk");
    assert_eq!(app.history.len(), before + 1);
    let _ = app.update(Message::Prop(Edit::Width(8)));
    assert_eq!(app.circuit().component(i).unwrap().props.width, 8);
    let _ = app.update(Message::Prop(Edit::LiveValue("0x2a".into())));
    settle(&mut app);
    assert_eq!(pin_value(&app, i, 0), Some(Signal::new(8, 0x2a)));
}

#[test]
fn subcircuit_instance_and_recursion_guard() {
    let mut app = app();
    inverter(&mut app);
    // Turn the LED into an output pin so that the circuit has terminals.
    let _ = app.update(Message::NewCircuit);
    let child = app.circuit().name.clone();
    let a = place(&mut app, Kind::Input, (0, 0));
    let n = place(&mut app, Kind::Not, (5, 0));
    let y = place(&mut app, Kind::Output, (10, 0));
    connect(&mut app, (a, "out"), (n, "in"));
    connect(&mut app, (n, "out"), (y, "in"));
    // Back to main and use the child there.
    let _ = app.update(Message::OpenCircuit("Main".into()));
    let _ = app.update(Message::PickSubcircuit(child.clone()));
    ev(&mut app, EditorEvent::Place { pos: (0, 10) });
    let sub = *app.selection.comps.iter().next().unwrap();
    assert_eq!(app.circuit().component(sub).unwrap().kind, Kind::Subcircuit);
    settle(&mut app);
    // Unconnected input: the inner NOT sees Z, its output is X.
    let out = app.cache.geom.items[&sub].layout.pin(&format!("out#{y}")).unwrap().0;
    assert_eq!(pin_value(&app, sub, out), Some(Signal::x(1)));
    // Navigating into the instance shows its live values.
    ev(&mut app, EditorEvent::OpenInstance { id: sub });
    assert_eq!(app.loc().inst, vec![sub]);
    assert!(pin_value(&app, n, 1).is_some());
    // The child cannot contain main (which contains the child).
    let _ = app.update(Message::OpenCircuit(child.clone()));
    let _ = app.update(Message::PickSubcircuit("Main".into()));
    assert!(matches!(app.tool, crate::canvas::Tool::Select), "recursive placement refused");
}

#[test]
fn one_circuit_at_a_time_with_breadcrumbs_and_remembered_views() {
    let mut app = app();
    let _ = app.update(Message::NewCircuit);
    let child = app.circuit().name.clone();
    let a = place(&mut app, Kind::Input, (0, 0));
    let y = place(&mut app, Kind::Output, (6, 0));
    connect(&mut app, (a, "out"), (y, "in"));
    ev(&mut app, EditorEvent::Pan(iced::Vector::new(37.0, 0.0)));
    let child_view = app.loc().view;
    assert_eq!(app.breadcrumbs(), vec![(child.clone(), String::new())]);

    let _ = app.update(Message::OpenCircuit("Main".into()));
    assert_eq!(app.circuit().name, "Main");
    let _ = app.update(Message::PickSubcircuit(child.clone()));
    ev(&mut app, EditorEvent::Place { pos: (0, 0) });
    let sub = *app.selection.comps.iter().next().unwrap();
    let _ = app.update(Message::Prop(Edit::Text(lb_core::component::PropKey::Label, "u1".into())));
    escape(&mut app);

    // Each circuit gets its own view back.
    let _ = app.update(Message::OpenCircuit(child.clone()));
    assert_eq!(app.loc().view, child_view);

    // Inside an instance the breadcrumb shows the path from the root.
    let _ = app.update(Message::OpenCircuit("Main".into()));
    ev(&mut app, EditorEvent::OpenInstance { id: sub });
    assert_eq!(app.circuit().name, child);
    assert_eq!(app.loc().root, "Main");
    assert_eq!(app.breadcrumbs(), vec![("Main".into(), String::new()), (child.clone(), "u1".into())]);
    // Esc with nothing to cancel goes back up.
    escape(&mut app);
    assert_eq!(app.circuit().name, "Main");
    assert!(app.loc().inst.is_empty());
    ev(&mut app, EditorEvent::OpenInstance { id: sub });
    let _ = app.update(Message::Breadcrumb(0));
    assert_eq!(app.circuit().name, "Main");
    assert!(app.loc().inst.is_empty());
}

#[test]
fn trace_and_export() {
    let mut app = app();
    let (i, n, _) = inverter(&mut app);
    settle(&mut app);
    ev(&mut app, EditorEvent::Trace { comp: n, pin: 1 });
    ev(&mut app, EditorEvent::Trace { comp: i, pin: 0 });
    assert_eq!(app.sim().unwrap().trace.signals().len(), 2);
    ev(&mut app, EditorEvent::Poke { id: i });
    settle(&mut app);
    let csv = app.sim().unwrap().trace.export_csv();
    assert!(csv.lines().count() >= 3, "{csv}");
    // Tracing survives an edit (recompilation).
    place(&mut app, Kind::Led, (0, 8));
    assert_eq!(app.sim().unwrap().trace.signals().len(), 2);
}

#[test]
fn oscillation_stops_the_simulation() {
    let mut app = app();
    let _ = app.update(Message::SetZeroDelay(true));
    let n = place(&mut app, Kind::Not, (0, 0));
    connect(&mut app, (n, "out"), (n, "in"));
    settle(&mut app);
    assert!(app.sim().unwrap().oscillation().is_some());
    assert!(!app.oscillating().is_empty());
    let _ = app.update(Message::Reset);
    assert!(app.sim().unwrap().oscillation().is_none());
}

#[test]
fn save_and_reopen() {
    let dir = std::env::temp_dir().join(format!("lb_app_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("inv.lbc");
    let mut app = app();
    inverter(&mut app);
    let _ = app.update(Message::SavePicked(Some(path.clone())));
    assert!(!app.is_dirty());
    let (reopened, _) = App::new(Some(path.clone()), false, None);
    assert_eq!(reopened.project, app.project);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn waypoints_can_be_added_moved_and_cleared() {
    let mut app = app();
    let (i, n, _) = inverter(&mut app);
    let wire = app.circuit().wires.iter().find(|w| w.from.0 == i && w.to.0 == n).unwrap().clone();
    ev(&mut app, EditorEvent::SelectWire { wire: wire.clone(), additive: false });
    // Freeze, then edit a waypoint, then go back to automatic routing.
    ev(&mut app, EditorEvent::SetWirePath { wire: wire.clone(), path: vec![(3, 5)] });
    let w2 = app.circuit().wires.iter().find(|w| w.from.0 == i).unwrap().clone();
    assert_eq!(w2.path, vec![(3, 5)]);
    assert_eq!(app.selection.wires, vec![w2.clone()], "selection follows the edited wire");
    let route = app.wire_route(&w2).unwrap().to_vec();
    assert!(route.contains(&(3, 5)), "{route:?}");
    let _ = app.update(Message::SetWirePath(w2, Vec::new()));
    assert!(app.circuit().wires.iter().all(|w| w.path.is_empty()));
    let _ = app.update(Message::Undo);
    assert!(app.circuit().wires.iter().any(|w| w.path == vec![(3, 5)]));
}

#[test]
fn buttons_can_be_moved() {
    let mut app = app();
    let b = place(&mut app, Kind::Button, (0, 0));
    // The canvas reports a press, then a move: the button is released and moved.
    ev(&mut app, EditorEvent::Button { id: b, pressed: true });
    ev(&mut app, EditorEvent::Button { id: b, pressed: false });
    ev(&mut app, EditorEvent::MoveSelection { dx: 4, dy: 2 });
    assert_eq!(app.circuit().component(b).unwrap().pos, (4, 2));
}

#[test]
fn panels_resize_and_menus_toggle() {
    use crate::app::Split;
    let mut app = app();
    let (l, r, w) = (app.left_w, app.right_w, app.wave_h);
    for (split, dx, dy) in [(Split::Left, 40.0, 0.0), (Split::Right, -30.0, 0.0), (Split::Wave, 0.0, -50.0)] {
        let _ = app.update(Message::StartResize(split));
        let _ = app.update(Message::ResizeMove(iced::Point::new(500.0, 500.0)));
        let _ = app.update(Message::ResizeMove(iced::Point::new(500.0 + dx, 500.0 + dy)));
        let _ = app.update(Message::ResizeEnd);
    }
    assert_eq!((app.left_w, app.right_w, app.wave_h), (l + 40.0, r + 30.0, w + 50.0));
    // Clamped to sensible sizes.
    let _ = app.update(Message::StartResize(Split::Left));
    let _ = app.update(Message::ResizeMove(iced::Point::new(0.0, 0.0)));
    let _ = app.update(Message::ResizeMove(iced::Point::new(-5000.0, 0.0)));
    assert_eq!(app.left_w, 160.0);
    let _ = app.update(Message::Menu(Some(crate::app::Menu::Edit)));
    assert_eq!(app.menu, Some(crate::app::Menu::Edit));
    let _ = app.update(Message::Menu(Some(crate::app::Menu::Edit)));
    assert_eq!(app.menu, None);
}

/// Feeds mouse events to the real canvas program and applies what it
/// publishes, like the iced runtime does.
fn canvas_mouse(app: &mut App, state: &mut crate::canvas::EditorState, event: iced::mouse::Event, at: iced::Point) {
    use iced::widget::canvas::Program;
    let bounds = iced::Rectangle::new(iced::Point::ORIGIN, app.canvas_size);
    let action = {
        let program = app.editor_program();
        program.update(state, &iced::Event::Mouse(event), bounds, iced::mouse::Cursor::Available(at))
    };
    if let Some(action) = action
        && let (Some(msg), _, _) = action.into_inner()
    {
        let _ = app.update(Message::Editor(msg));
    }
}

#[test]
fn dragging_a_button_moves_it_and_clicking_presses_it() {
    use iced::mouse::{Button as B, Event as E};
    let mut app = app();
    let b = place(&mut app, Kind::Button, (2, 2));
    let view = app.loc().view;
    let center = view.to_screen((3.0, 3.0));
    let far = view.to_screen((9.0 + 8.0 / view.scale, 7.0));
    let mut st = crate::canvas::EditorState::default();
    let pressed = |app: &App| app.live_input(b).is_some_and(|v| v.is_one());

    // Click: pressed while the mouse is down, released after.
    canvas_mouse(&mut app, &mut st, E::CursorMoved { position: center }, center);
    canvas_mouse(&mut app, &mut st, E::ButtonPressed(B::Left), center);
    assert!(pressed(&app));
    canvas_mouse(&mut app, &mut st, E::ButtonReleased(B::Left), center);
    assert!(!pressed(&app));
    assert_eq!(app.circuit().component(b).unwrap().pos, (2, 2));

    // Drag: the button is released and the component moves. (Press a few
    // pixels away so that it is not taken as a double click.)
    let grab = iced::Point::new(center.x + 8.0, center.y);
    canvas_mouse(&mut app, &mut st, E::CursorMoved { position: grab }, grab);
    canvas_mouse(&mut app, &mut st, E::ButtonPressed(B::Left), grab);
    canvas_mouse(&mut app, &mut st, E::CursorMoved { position: far }, far);
    assert!(!pressed(&app));
    canvas_mouse(&mut app, &mut st, E::ButtonReleased(B::Left), far);
    assert_eq!(app.circuit().component(b).unwrap().pos, (8, 6));
}

#[test]
fn selected_wires_stay_selected_after_a_move() {
    let mut app = app();
    let (i, n, _) = inverter(&mut app);
    let wire = app.circuit().wires.iter().find(|w| w.from.0 == i && w.to.0 == n).unwrap().clone();
    ev(&mut app, EditorEvent::SetWirePath { wire, path: vec![(3, 6)] });
    ev(&mut app, EditorEvent::SelectRect { from: (-1.0, -1.0), to: (30.0, 10.0), additive: false });
    assert_eq!(app.selection.wires.len(), 2);
    ev(&mut app, EditorEvent::MoveSelection { dx: 2, dy: 3 });
    assert_eq!(app.selection.wires.len(), 2, "both wires still selected");
    assert!(app.selection.wires.iter().any(|w| w.path == vec![(5, 9)]));
}

#[test]
fn library_sections_collapse() {
    let mut app = app();
    let section = "Gates".to_string();
    let _ = app.update(Message::ToggleSection(section.clone()));
    assert!(app.collapsed.contains(&section));
    let _ = app.update(Message::ToggleSection(section.clone()));
    assert!(!app.collapsed.contains(&section));
    assert_eq!(lb_core::component::Category::Annotations.name(), "Text & annotations");
}

#[test]
fn touchpad_scrolling_pans_and_mouse_wheel_zooms() {
    use iced::mouse::{Event as E, ScrollDelta};
    let mut app = app();
    let mut st = crate::canvas::EditorState::default();
    let at = iced::Point::new(200.0, 200.0);
    let before = app.loc().view;
    canvas_mouse(&mut app, &mut st, E::WheelScrolled { delta: ScrollDelta::Pixels { x: 12.0, y: -30.0 } }, at);
    let view = app.loc().view;
    assert_eq!(view.scale, before.scale);
    assert_eq!(view.offset, before.offset + iced::Vector::new(12.0, -30.0));
    // Fractions of a line come from a touchpad too (X11).
    canvas_mouse(&mut app, &mut st, E::WheelScrolled { delta: ScrollDelta::Lines { x: 0.0, y: 0.25 } }, at);
    assert_eq!(app.loc().view.scale, before.scale);
    // A notched wheel zooms.
    canvas_mouse(&mut app, &mut st, E::WheelScrolled { delta: ScrollDelta::Lines { x: 0.0, y: 1.0 } }, at);
    assert!(app.loc().view.scale > before.scale);
}

#[test]
fn custom_speed() {
    use crate::app::Speed;
    let mut app = app();
    let _ = app.update(Message::EditCustomSpeed);
    assert_eq!(app.custom_speed.as_deref(), Some("200"));
    let _ = app.update(Message::CustomSpeed("1 500".into()));
    assert_eq!(app.speed, Speed::Ticks(1500));
    assert_eq!(app.speed.to_string(), "1500 tick/s");
    // Invalid text keeps the last valid speed.
    let _ = app.update(Message::CustomSpeed("15x".into()));
    let _ = app.update(Message::CustomSpeed("0".into()));
    assert_eq!(app.speed, Speed::Ticks(1500));
    let _ = app.update(Message::CustomSpeedDone);
    assert!(app.custom_speed.is_none());
    assert_eq!(Speed::Ticks(2_000).to_string(), "2 k tick/s");
}

#[test]
fn the_main_circuit_is_never_a_subcircuit() {
    use crate::canvas::Tool;
    let mut app = app();
    let _ = app.update(Message::NewCircuit);
    let child = app.circuit().name.clone();
    assert_eq!(child, "Circuit 1");
    let _ = app.update(Message::NewCircuit);
    assert_eq!(app.circuit().name, "Circuit 2");
    let _ = app.update(Message::OpenCircuit(child.clone()));
    // Main does not contain the child, so only the main rule forbids this.
    assert!(!app.project.would_recurse(&child, "Main"));
    let _ = app.update(Message::PickSubcircuit("Main".into()));
    assert!(matches!(app.tool, Tool::Select));
    assert!(app.placement_refusal("Main").is_some());

    // A circuit used as a subcircuit cannot become the main one.
    let _ = app.update(Message::OpenCircuit("Main".into()));
    let _ = app.update(Message::PickSubcircuit(child.clone()));
    ev(&mut app, EditorEvent::Place { pos: (0, 0) });
    escape(&mut app);
    let _ = app.update(Message::OpenCircuit(child.clone()));
    let _ = app.update(Message::MakeMain);
    assert_eq!(app.project.main().name, "Main");
}

#[test]
fn the_column_of_signal_names_can_be_resized() {
    use crate::waveform::{NAME_W, WaveMsg};
    let mut app = app();
    let _ = app.update(Message::Wave(WaveMsg::Resized(iced::Size::new(1000.0, 200.0))));
    let before = app.wave.visible_ticks();
    let _ = app.update(Message::Wave(WaveMsg::NamesWidth(300.0)));
    assert_eq!(app.wave.names_w, 300.0);
    assert!(app.wave.visible_ticks() < before, "less room for the waves");
    assert_eq!(app.wave.x_of(app.wave.start), 300.0, "waves start at the edge");
    // Kept between a minimum and most of the panel.
    let _ = app.update(Message::Wave(WaveMsg::NamesWidth(10.0)));
    assert!(app.wave.names_w > 100.0 && app.wave.names_w < NAME_W);
    let _ = app.update(Message::Wave(WaveMsg::NamesWidth(5000.0)));
    assert_eq!(app.wave.names_w, 700.0);
}

#[test]
fn the_waveform_shows_ten_seconds_at_the_current_speed() {
    use crate::app::Speed;
    use crate::waveform::{NAME_W, WaveMsg};
    let mut app = app();
    // The initial speed applies once the panel has a size.
    let _ = app.update(Message::Wave(WaveMsg::Resized(iced::Size::new(NAME_W + 800.0, 200.0))));
    assert!((app.wave.visible_ticks() - 2_000.0).abs() < 1e-6);
    let _ = app.update(Message::SetSpeed(Speed::Ticks(50)));
    assert!((app.wave.visible_ticks() - 500.0).abs() < 1e-6);
    let _ = app.update(Message::CustomSpeed("1234".into()));
    assert!((app.wave.visible_ticks() - 12_340.0).abs() < 1e-6);
    // Maximum speed has no rate: the zoom is kept.
    let _ = app.update(Message::SetSpeed(Speed::Max));
    assert!((app.wave.visible_ticks() - 12_340.0).abs() < 1e-6);
}

#[test]
fn settings_round_trip_and_ignore_unknown_values() {
    use crate::settings::{Scheme, Settings};
    let s = Settings {
        scheme: Scheme::Dark,
        text_size: 125,
        hdl: true,
        hdl_auto: false,
        ghdl: "/opt/oss cad/bin/ghdl".into(),
        yosys: String::new(),
    };
    assert_eq!(Settings::parse(&s.to_text()), s);
    let odd = Settings::parse("scheme = purple\ntext_size = 300\nother = 1\nnot a setting");
    assert_eq!(odd, Settings::default());
}

#[test]
fn the_settings_menu_changes_scheme_and_text_size() {
    use crate::settings::Scheme;
    use crate::theme;
    let mut app = app();
    let _ = app.update(Message::SetTextSize(150));
    assert_eq!(theme::fs(12.0), 18.0);
    let _ = app.update(Message::SetScheme(Scheme::Dark));
    assert!(theme::is_dark());
    // Following the system uses the mode reported by the desktop.
    let _ = app.update(Message::SetScheme(Scheme::System));
    assert!(!theme::is_dark());
    let _ = app.update(Message::SystemTheme(iced::theme::Mode::Dark));
    assert!(theme::is_dark());
    let _ = app.update(Message::SetScheme(Scheme::Light));
    assert!(!theme::is_dark());
    // Sizes are kept in range.
    let _ = app.update(Message::SetTextSize(1000));
    assert_eq!(app.settings.text_size, 200);
    let _ = app.update(Message::SetTextSize(100));
    assert_eq!(theme::fs(12.0), 12.0);
}

fn press(app: &mut App, key: iced::keyboard::Key, text: Option<&str>, ctrl: bool) {
    let modifiers = if ctrl { iced::keyboard::Modifiers::CTRL } else { iced::keyboard::Modifiers::empty() };
    let _ = app.update(Message::Key(iced::keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: iced::keyboard::key::Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
        location: iced::keyboard::Location::Standard,
        modifiers,
        text: text.map(Into::into),
        repeat: false,
    }));
}

#[test]
fn a_selected_keyboard_takes_the_typed_keys() {
    use iced::keyboard::{Key, key::Named};
    let mut app = app();
    let kb = place(&mut app, Kind::Keyboard, (0, 0));
    let _ = app.update(Message::Editor(EditorEvent::Select { id: kb, additive: false }));
    assert_eq!(app.typing_target(), Some(kb));
    let rot = app.circuit().component(kb).unwrap().rot;
    for c in ["r", "F"] {
        press(&mut app, Key::Character(c.to_lowercase().into()), Some(c), false);
    }
    press(&mut app, Key::Named(Named::Space), Some(" "), false);
    press(&mut app, Key::Named(Named::Enter), Some("\r"), false);
    press(&mut app, Key::Named(Named::Backspace), None, false);
    assert_eq!(app.circuit().component(kb).unwrap().rot, rot, "R typed, not rotated");
    let sim = app.sim().unwrap();
    let i = sim.netlist().comp_index(&[], kb).unwrap();
    let keys = lb_core::component::eval::keyboard_buffer(&sim.state(i).mem).to_vec();
    assert_eq!(keys, [b'r' as u64, b'F' as u64, 32, 10, 8]);
    // Ctrl shortcuts still work; Esc stops typing.
    press(&mut app, Key::Character("z".into()), Some("z"), true);
    assert!(app.circuit().component(kb).is_none(), "Ctrl+Z undid the placement");
    let kb = place(&mut app, Kind::Keyboard, (0, 0));
    let _ = app.update(Message::Editor(EditorEvent::Select { id: kb, additive: false }));
    press(&mut app, Key::Named(Named::Escape), None, false);
    assert_eq!(app.typing_target(), None);
}

#[test]
fn keyboard_buffer_size_clear_and_delete() {
    use iced::keyboard::{Key, key::Named};
    use lb_core::component::PropKey;
    let mut app = app();
    let kb = place(&mut app, Kind::Keyboard, (0, 0));
    let _ = app.update(Message::Editor(EditorEvent::Select { id: kb, additive: false }));
    // Typed in the field one key at a time; out of range values are clamped.
    for (typed, size) in [("3", 3), ("", 3), ("5", 5), ("51", 51), ("512", 512), ("99999", 4096)] {
        let _ = app.update(Message::Prop(Edit::Text(PropKey::Buffer, typed.into())));
        assert_eq!(app.circuit().component(kb).unwrap().props.key_buffer(), size, "after {typed:?}");
    }
    let sim = app.sim().unwrap();
    let i = sim.netlist().comp_index(&[], kb).unwrap();
    assert_eq!(sim.state(i).mem.len(), 4097, "the simulator uses the new size");

    for c in ["a", "b"] {
        press(&mut app, Key::Character(c.into()), Some(c), false);
    }
    assert_eq!(app.keyboard_keys(kb), Some(2));
    let _ = app.update(Message::ClearKeyboard(kb));
    assert_eq!(app.keyboard_keys(kb), Some(0), "cleared");

    // Del deletes the selected keyboard; Backspace is typed.
    press(&mut app, Key::Named(Named::Backspace), None, false);
    assert_eq!(app.keyboard_keys(kb), Some(1));
    press(&mut app, Key::Named(Named::Delete), None, false);
    assert!(app.circuit().component(kb).is_none());
}

#[test]
fn tunnels_carry_the_signal_and_link_their_group() {
    use lb_core::component::PropKey;
    let mut app = app();
    let tunnel = |app: &mut App, pos, label: &str| {
        let t = place(app, Kind::Tunnel, pos);
        let _ = app.update(Message::Prop(Edit::Text(PropKey::Label, label.into())));
        t
    };
    let i = place(&mut app, Kind::Input, (0, 0));
    let a = tunnel(&mut app, (4, 0), "clk");
    let b = tunnel(&mut app, (20, 10), "clk");
    let other = tunnel(&mut app, (20, 20), "rst");
    let led = place(&mut app, Kind::Led, (28, 10));
    connect(&mut app, (i, "out"), (a, "in"));
    connect(&mut app, (b, "in"), (led, "in"));
    ev(&mut app, EditorEvent::Poke { id: i });
    settle(&mut app);
    assert_eq!(pin_value(&app, led, 0), Some(Signal::ones(1)));

    let circuit = app.circuit();
    let (links, group) = crate::canvas::tunnel_links(circuit, circuit.component(a).into_iter());
    assert_eq!(links.into_iter().collect::<Vec<_>>(), [(a.min(b), a.max(b))]);
    assert!(group.contains(&b) && !group.contains(&other));
    let (links, _) = crate::canvas::tunnel_links(circuit, circuit.component(other).into_iter());
    assert!(links.is_empty(), "no other tunnel is labelled rst");
}

#[test]
fn moving_a_component_adds_a_waypoint_at_the_new_corner() {
    let mut app = app();
    let (i, n, _) = inverter(&mut app);
    let wire = app.circuit().wires.iter().find(|w| w.from.0 == i && w.to.0 == n).unwrap().clone();
    // From (2, 1) down and back up into the NOT input at (5, 1).
    ev(&mut app, EditorEvent::SetWirePath { wire, path: vec![(3, 1), (3, 4), (4, 4), (4, 1)] });
    ev(&mut app, EditorEvent::Select { id: n, additive: false });
    // Moving the NOT up by 3 puts its input at (5, -2): the wire goes up at
    // x = 4 and into the input from its left, the side it faces.
    ev(&mut app, EditorEvent::MoveSelection { dx: 0, dy: -3 });
    let wire = app.circuit().wires.iter().find(|w| w.from.0 == i && w.to.0 == n).unwrap();
    assert_eq!(wire.path, [(3, 1), (3, 4), (4, 4), (4, -2)]);
}

#[test]
fn moving_a_component_keeps_its_wires_automatic() {
    let mut app = app();
    let (_, n, _) = inverter(&mut app);
    ev(&mut app, EditorEvent::Select { id: n, additive: false });
    ev(&mut app, EditorEvent::MoveSelection { dx: 6, dy: 5 });
    assert!(app.circuit().wires.iter().all(|w| w.path.is_empty()));
}

#[test]
fn moving_a_component_removes_the_spurs_of_its_wires() {
    let mut app = app();
    let (i, n, _) = inverter(&mut app);
    let wire = app.circuit().wires.iter().find(|w| w.from.0 == i && w.to.0 == n).unwrap().clone();
    // From (2, 1) around to (7, 1), straight on to the NOT input at (5, 1).
    let path = vec![(3, 1), (3, 4), (10, 4), (10, 1), (7, 1)];
    ev(&mut app, EditorEvent::SetWirePath { wire, path: path.clone() });
    ev(&mut app, EditorEvent::Select { id: n, additive: false });
    // Now the input is at (8, 1): the wire would reach (7, 1) and turn back.
    ev(&mut app, EditorEvent::MoveSelection { dx: 3, dy: 0 });
    let wire = app.circuit().wires.iter().find(|w| w.from.0 == i && w.to.0 == n).unwrap();
    assert_eq!(wire.path, [(3, 1), (3, 4), (10, 4), (10, 1)]);
    let _ = app.update(Message::Undo);
    assert_eq!(app.circuit().component(n).unwrap().pos, (5, 0));
    assert!(app.circuit().wires.iter().any(|w| w.path == path), "one undo step");
}

#[test]
fn a_waypoint_dropped_on_a_straight_run_is_removed() {
    use iced::mouse::{Button as B, Event as E};
    let mut app = app();
    let (i, n, _) = inverter(&mut app);
    let wire = |app: &App| app.circuit().wires.iter().find(|w| w.from.0 == i && w.to.0 == n).unwrap().clone();
    let set = |app: &mut App, path: Vec<(i32, i32)>| {
        let w = wire(app);
        ev(app, EditorEvent::SelectWire { wire: w.clone(), additive: false });
        ev(app, EditorEvent::SetWirePath { wire: w, path });
    };
    let mut st = crate::canvas::EditorState::default();
    let view = app.loc().view;
    let mut drag = |app: &mut App, from: (f32, f32), to: (f32, f32)| {
        let (a, b) = (view.to_screen(from), view.to_screen(to));
        canvas_mouse(app, &mut st, E::CursorMoved { position: a }, a);
        canvas_mouse(app, &mut st, E::ButtonPressed(B::Left), a);
        canvas_mouse(app, &mut st, E::CursorMoved { position: b }, b);
        canvas_mouse(app, &mut st, E::ButtonReleased(B::Left), b);
    };

    // A detour below the straight line from (2, 1) to (5, 1). Deepening it
    // by one corner leaves (4, 4) on the straight run (4, 6)-(4, 1), and the
    // new corner (4, 6) gets a waypoint.
    set(&mut app, vec![(3, 1), (3, 4), (4, 4), (4, 1)]);
    drag(&mut app, (3.0, 4.0), (3.0, 6.0));
    assert_eq!(wire(&app).path, [(3, 1), (3, 6), (4, 6), (4, 1)]);

    // Pulling either bottom corner deeper takes the bottom run along.
    drag(&mut app, (4.0, 6.0), (4.0, 9.0));
    assert_eq!(wire(&app).path, [(3, 1), (3, 9), (4, 9), (4, 1)]);

    // Flattening it leaves no bend: the wire is routed automatically again.
    drag(&mut app, (3.0, 9.0), (3.0, 1.0));
    assert!(wire(&app).path.is_empty());
}

/// Presses a character key with any modifiers.
fn press_with(app: &mut App, c: &str, modifiers: iced::keyboard::Modifiers) {
    use iced::keyboard::{Key, Location, key};
    let _ = app.update(Message::Key(iced::keyboard::Event::KeyPressed {
        key: Key::Character(c.into()),
        modified_key: Key::Character(c.into()),
        physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
        location: Location::Standard,
        modifiers,
        text: Some(c.into()),
        repeat: false,
    }));
}

fn open_commands(app: &mut App) {
    use iced::keyboard::Modifiers;
    press_with(app, "P", Modifiers::CTRL | Modifiers::SHIFT);
    assert!(app.commands.is_some(), "Ctrl+Shift+P opens the command palette");
}

/// Types `query` in the palette and runs the first match.
fn run_command(app: &mut App, query: &str) {
    let _ = app.update(Message::CommandQuery(query.into()));
    let _ = app.update(Message::CommandRun(0));
}

#[test]
fn command_palette_opens_and_closes() {
    let mut app = app();
    open_commands(&mut app);
    let _ = app.update(Message::CloseCommands);
    assert!(app.commands.is_none());
    // Shortcuts are off while it is open: S does not step the simulation.
    open_commands(&mut app);
    let t = app.sim().map(|s| s.time());
    press_with(&mut app, "s", iced::keyboard::Modifiers::empty());
    assert_eq!(app.sim().map(|s| s.time()), t);
    assert!(app.commands.is_some());
}

#[test]
fn command_palette_ranks_and_filters() {
    let app = app();
    let all = crate::commands::entries(&app);
    let labels =
        |q: &str| -> Vec<String> { crate::commands::filter(all.clone(), q).into_iter().map(|e| e.label).collect() };
    assert_eq!(labels("").len(), all.len());
    assert_eq!(labels("rot")[0], "Edit: Rotate");
    assert_eq!(labels("ZOOM IN"), ["View: Zoom in"]);
    assert_eq!(labels("place and")[0], "Place: AND");
    assert!(labels("no such command").is_empty());
}

#[test]
fn command_palette_runs_commands() {
    use crate::canvas::Tool;
    let mut app = app();
    open_commands(&mut app);
    run_command(&mut app, "place not");
    assert!(app.commands.is_none(), "running a command closes the palette");
    assert!(matches!(app.tool, Tool::Place { kind: Kind::Not, .. }));

    // A disabled command does nothing and leaves the palette open.
    let mut app = self::app();
    open_commands(&mut app);
    run_command(&mut app, "edit undo");
    assert!(app.commands.is_some());

    let grid = app.show_grid;
    run_command(&mut app, "grid");
    assert_eq!(app.show_grid, !grid);
}

#[test]
fn command_palette_highlight_wraps_and_scrolls() {
    use crate::commands::ROWS;
    let mut app = app();
    open_commands(&mut app);
    let n = crate::commands::matches(&app, app.commands.as_ref().unwrap()).len();
    assert!(n > ROWS);
    let _ = app.update(Message::CommandMove(-1));
    let p = app.commands.as_ref().unwrap();
    assert_eq!((p.selected, p.first), (n - 1, n - ROWS));
    let _ = app.update(Message::CommandMove(1));
    let p = app.commands.as_ref().unwrap();
    assert_eq!((p.selected, p.first), (0, 0));
}

// TEMP-PROFILE-BEGIN
#[test]
#[ignore]
fn tmp_profile_edit_cycle() {
    use lb_core::circuit::{Circuit, Geometry, Project};
    use std::time::Instant;
    let mut c = Circuit::new("stress");
    for row in 0..50 {
        let y = row * 6;
        let mut prev = (c.add(Kind::Input, (0, y)), "out");
        for col in 0..99 {
            let kind = if col % 2 == 0 { Kind::Not } else { Kind::Buffer };
            let g = c.add(kind, (5 + col * 6, y));
            c.connect(prev, (g, "in"));
            prev = (g, "out");
        }
    }
    let mut seed = 12345u32;
    for k in 0..300 {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let col = (seed >> 8) % 98;
        let row = (seed >> 20) % 49;
        let led = c.add(Kind::Led, (8 + col as i32 * 6, row as i32 * 6 + 3));
        let src = 1 + (k % 50) * 100;
        c.connect((src, "out"), (led, "in"));
    }
    let dir = std::env::temp_dir().join(format!("lb-prof-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("stress.lbc");
    lb_core::io::save(&Project::new(c), &path).unwrap();

    let t = Instant::now();
    let (mut app, _) = App::new(Some(path.clone()), false, None);
    eprintln!("load: {:?}", t.elapsed());

    let p = app.project.clone();
    let circ = app.circuit().clone();
    let t = Instant::now();
    for _ in 0..10 {
        let _ = circ.clone();
    }
    eprintln!("clone circuit: {:?}", t.elapsed() / 10);
    let t = Instant::now();
    for _ in 0..10 {
        let _ = Geometry::new(&p, &circ);
    }
    eprintln!("geometry: {:?}", t.elapsed() / 10);
    let g = Geometry::new(&p, &circ);
    let mut memo = Default::default();
    let t = Instant::now();
    crate::autoroute::route_all(&g, &circ, &mut memo);
    eprintln!("route_all fresh: {:?}", t.elapsed());
    let t = Instant::now();
    for _ in 0..10 {
        crate::autoroute::route_all(&g, &circ, &mut memo);
    }
    eprintln!("route_all memo: {:?}", t.elapsed() / 10);
    let t = Instant::now();
    for _ in 0..10 {
        let _ = lb_core::netlist::compile(&p, "stress").unwrap();
    }
    eprintln!("compile: {:?}", t.elapsed() / 10);

    ev(&mut app, EditorEvent::Select { id: 500, additive: false });
    let t = Instant::now();
    for i in 0..10 {
        ev(&mut app, EditorEvent::MoveSelection { dx: 0, dy: if i % 2 == 0 { 1 } else { -1 } });
    }
    eprintln!("move one component (full update): {:?}", t.elapsed() / 10);
    let t = Instant::now();
    for _ in 0..10 {
        let _ = app.update(Message::Undo);
    }
    eprintln!("undo: {:?}", t.elapsed() / 10);
    let t = Instant::now();
    for _ in 0..100 {
        let _ = app.update(Message::Frame(Instant::now()));
    }
    eprintln!("idle frame: {:?}", t.elapsed() / 100);
    let t = Instant::now();
    for _ in 0..100 {
        let _ = app.view();
    }
    eprintln!("view(): {:?}", t.elapsed() / 100);
    ev(&mut app, EditorEvent::SelectRect { from: (-10.0, -10.0), to: (700.0, 400.0), additive: false });
    let t = Instant::now();
    ev(&mut app, EditorEvent::MoveSelection { dx: 2, dy: 0 });
    eprintln!("move all: {:?}", t.elapsed());
    let _ = std::fs::remove_dir_all(&dir);
}
// TEMP-PROFILE-END

#[test]
fn circuits_synthesized_from_hdl_are_read_only_until_converted() {
    use lb_core::circuit::{Circuit, HdlInfo, Project};
    let mut main = Circuit::new("Main");
    let sub = main.add(Kind::Subcircuit, (0, 0));
    main.component_mut(sub).unwrap().props.circuit = "alu".into();
    let mut alu = Circuit::new("alu");
    alu.add(Kind::Input, (0, 0));
    alu.hdl = Some(HdlInfo { digest: "d".into(), stale: false });
    let mut p = Project::new(main);
    p.circuits.push(alu);
    p.hdl.tops.push("alu".into());
    let dir = std::env::temp_dir().join(format!("lb_app_ro_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("p.lbc");
    lb_core::io::save(&p, &path).unwrap();

    let mut app = app();
    let _ = app.update(Message::OpenPicked(Some(path)));
    let _ = app.update(Message::OpenCircuit("alu".into()));
    place_refused(&mut app);
    let _ = app.update(Message::RenameCircuit("other".into()));
    assert!(app.project.circuit("alu").is_some(), "named after its entity");
    let _ = app.update(Message::MakeMain);
    assert_eq!(app.project.main().name, "Main");

    let _ = app.update(Message::ConvertCircuit("alu".into()));
    assert!(app.project.circuit("alu").unwrap().hdl.is_none());
    assert!(app.project.hdl.tops.is_empty(), "no longer synthesized");
    let n = app.circuit().components.len();
    let _ = app.update(Message::PickTool(Kind::And));
    ev(&mut app, EditorEvent::Place { pos: (10, 10) });
    assert_eq!(app.circuit().components.len(), n + 1, "editable once converted");
    // Undo brings back the synthesized circuit in one step.
    let _ = app.update(Message::Undo);
    let _ = app.update(Message::Undo);
    assert!(app.project.circuit("alu").unwrap().hdl.is_some());
    assert_eq!(app.project.hdl.tops, ["alu"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Placing a component in the shown (read-only) circuit does nothing.
fn place_refused(app: &mut App) {
    let n = app.circuit().components.len();
    let _ = app.update(Message::PickTool(Kind::And));
    ev(app, EditorEvent::Place { pos: (10, 10) });
    assert_eq!(app.circuit().components.len(), n);
    escape(app);
}

#[test]
fn frozen_route_can_go_back_to_automatic() {
    let mut app = app();
    let i = place(&mut app, Kind::Input, (0, 0));
    let n = place(&mut app, Kind::Not, (6, 4));
    connect(&mut app, (i, "out"), (n, "in"));
    let wire = app.circuit().wires[0].clone();
    ev(&mut app, EditorEvent::SelectWire { wire: wire.clone(), additive: false });
    // What the Freeze route button sends.
    let pts = app.wire_route(&wire).unwrap().to_vec();
    assert!(pts.len() > 2, "{pts:?}");
    let frozen = crate::routing::tidy_path(pts[0], &pts[1..pts.len() - 1], pts[pts.len() - 1]);
    let _ = app.update(Message::SetWirePath(wire, frozen));
    let sel = app.selection.wires[0].clone();
    assert!(app.circuit().wires.contains(&sel), "selection follows the frozen wire");
    let _ = app.update(Message::SetWirePath(sel, Vec::new()));
    assert!(app.circuit().wires[0].path.is_empty(), "{:?}", app.circuit().wires);
}

#[test]
fn f1_shows_the_documentation_of_the_selected_component() {
    use crate::app::Modal;
    use iced::keyboard::key::Named;
    let mut app = app();
    let _ = place(&mut app, Kind::Counter, (0, 0));
    press_named(&mut app, Named::F1);
    assert_eq!(app.modal, Some(Modal::Help(Kind::Counter)));
    escape(&mut app);
    assert_eq!(app.modal, None);
    // While placing, the component being placed.
    let _ = app.update(Message::PickTool(Kind::Ram));
    press_named(&mut app, Named::F1);
    assert_eq!(app.modal, Some(Modal::Help(Kind::Ram)));
}
