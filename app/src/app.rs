//! Application state, messages, update and view.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::keyboard::{self, Key, key::Named};
use iced::widget::{
    Space, button, canvas, column, container, mouse_area, opaque, pick_list, row, stack, text, text_editor, text_input,
};
use iced::{Alignment, Element, Fill, Padding, Point, Size, Subscription, Task, Vector, window};

use lb_core::circuit::{Circuit, CompGeom, CompId, Component, Geometry, PinRef, Project, Wire};
use lb_core::component::Kind;
use lb_core::history::{Clip, Command, History, Op};
use lb_core::io;
use lb_core::netlist::{NetId, compile};
use lb_core::sim::{SimConfig, Simulator};
use lb_core::value::Signal;

use crate::autoroute;
use crate::canvas::{CircuitCache, Editor, EditorEvent, Selection, Tool, orient_wire};
use crate::commands::{self, CommandPalette};
use crate::icons::{self, Emblem, Icon};
use crate::routing;
use crate::settings::{Scheme, Settings, TEXT_SIZE_RANGE, TEXT_SIZES};
use crate::shapes::View;
use crate::theme;
use crate::waveform::{self, WaveMsg, WaveView, Waveform};
use crate::{palette, props};

pub const DEFAULT_SCALE: f32 = 16.0;

/// What the editor shows: a circuit on its own, or one instance of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub circuit: String,
    /// Circuit simulated as root while this tab is shown.
    pub root: String,
    /// Instance path within the root (empty: the root itself).
    pub inst: Vec<CompId>,
    pub view: View,
    /// The user panned or zoomed: stop fitting automatically on resize.
    pub view_touched: bool,
}

impl Location {
    fn new(circuit: &str) -> Self {
        Location {
            circuit: circuit.into(),
            root: circuit.into(),
            inst: Vec::new(),
            view: View { scale: DEFAULT_SCALE, offset: Vector::new(80.0, 80.0) },
            view_touched: false,
        }
    }
}

struct SimSlot {
    sim: Simulator,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Ticks(u32),
    Max,
}

impl Speed {
    pub const ALL: [Speed; 14] = [
        Speed::Ticks(1),
        Speed::Ticks(2),
        Speed::Ticks(5),
        Speed::Ticks(10),
        Speed::Ticks(20),
        Speed::Ticks(50),
        Speed::Ticks(100),
        Speed::Ticks(200),
        Speed::Ticks(500),
        Speed::Ticks(1_000),
        Speed::Ticks(10_000),
        Speed::Ticks(100_000),
        Speed::Ticks(1_000_000),
        Speed::Max,
    ];
}

impl Speed {
    /// Simulated time shown by the waveform panel: ten seconds at this
    /// speed. `None` at maximum speed, which has no fixed rate.
    pub fn wave_span(self) -> Option<f64> {
        match self {
            Speed::Ticks(n) => Some(n as f64 * WAVE_SECONDS),
            Speed::Max => None,
        }
    }
}

/// Seconds of simulation visible in the waveform panel.
const WAVE_SECONDS: f64 = 10.0;

impl std::fmt::Display for Speed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Speed::Ticks(n) if *n >= 1_000_000 && n % 1_000_000 == 0 => {
                write!(f, "{} M tick/s", n / 1_000_000)
            }
            Speed::Ticks(n) if *n >= 1_000 && n % 1_000 == 0 => write!(f, "{} k tick/s", n / 1_000),
            Speed::Ticks(n) => write!(f, "{n} tick/s"),
            Speed::Max => f.write_str("Max"),
        }
    }
}

/// Entry of the speed menu: a preset, or a typed number of ticks per second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedChoice {
    Preset(Speed),
    Custom,
}

impl std::fmt::Display for SpeedChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpeedChoice::Preset(s) => s.fmt(f),
            SpeedChoice::Custom => f.write_str("Custom…"),
        }
    }
}

/// Highest custom speed, in ticks per second.
pub const MAX_CUSTOM_SPEED: u32 = 100_000_000;
const CUSTOM_SPEED_ID: &str = "custom-speed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Event,
    Delta,
    Tick,
    Clock,
}

impl StepKind {
    pub const ALL: [StepKind; 4] = [StepKind::Event, StepKind::Delta, StepKind::Tick, StepKind::Clock];
}

impl std::fmt::Display for StepKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StepKind::Event => "One event",
            StepKind::Delta => "One delta",
            StepKind::Tick => "One tick",
            StepKind::Clock => "One clock edge",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    File,
    Edit,
    Simulation,
    View,
    Hdl,
    Settings,
}

/// Which commands of the menus and the command palette apply now.
pub struct Enabled {
    /// Components are selected.
    pub sel: bool,
    /// Something is selected (components or wires).
    pub delete: bool,
    pub paste: bool,
    /// Rotate and flip: a selection or a component being placed.
    pub transform: bool,
    pub align: bool,
    /// The shown circuit is simulated.
    pub sim: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    CenterX,
    Right,
    Top,
    CenterY,
    Bottom,
}

/// What to do after the user resolves unsaved changes.
#[derive(Clone, Debug, PartialEq)]
pub enum After {
    New,
    Open,
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Modal {
    Unsaved(After),
    Message(String),
    /// The documentation of a component (its items in [`App::help`]).
    Help(Kind),
    /// Where GHDL and Yosys are.
    #[cfg(feature = "hdl")]
    HdlTools,
}

#[derive(Debug, Clone)]
pub enum Message {
    Noop,
    Menu(Option<Menu>),
    // File
    New,
    Open,
    OpenPicked(Option<PathBuf>),
    Save,
    SaveAs,
    SavePicked(Option<PathBuf>),
    ModalSave,
    ModalDiscard,
    ModalCancel,
    /// Show the documentation of a component.
    ShowHelp(Kind),
    CloseRequested,
    Quit,
    // Edit
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    Duplicate,
    Delete,
    SelectAll,
    Rotate,
    Mirror,
    Align(Align),
    // Simulation
    ToggleRun,
    Reset,
    Step,
    SetStepKind(StepKind),
    SetSpeed(Speed),
    /// Show the field to type a custom speed.
    EditCustomSpeed,
    CustomSpeed(String),
    CustomSpeedDone,
    SetZeroDelay(bool),
    SetGateDelay(String),
    SetDeltaLimit(String),
    SetTraceCapacity(String),
    Frame(Instant),
    // View
    FitView,
    ZoomIn,
    ZoomOut,
    ToggleGrid,
    SetScheme(Scheme),
    /// Sets the text size, in percent of the default.
    SetTextSize(u32),
    /// Empties the buffer of a keyboard component.
    ClearKeyboard(CompId),
    /// The desktop switched between light and dark.
    SystemTheme(iced::theme::Mode),
    ToggleWaveform,
    // Command palette
    OpenCommands,
    CloseCommands,
    CommandQuery(String),
    /// Move the highlight up (-1) or down (+1).
    CommandMove(i32),
    /// Run the match with this index.
    CommandRun(usize),
    // Palette, circuits
    Search(String),
    /// Collapse or expand a section of the component library.
    ToggleSection(String),
    PickTool(Kind),
    PickSubcircuit(String),
    NewCircuit,
    OpenCircuit(String),
    /// Go to a level of the current instance path (0: the root circuit).
    Breadcrumb(usize),
    RenameCircuit(String),
    DeleteCircuit,
    MakeMain,
    /// Turn a circuit synthesized from HDL into an ordinary, editable one.
    ConvertCircuit(String),
    // HDL
    /// Show or hide the HDL features.
    SetHdlEnabled(bool),
    SetHdlAuto(bool),
    #[cfg(feature = "hdl")]
    Hdl(crate::hdl::Msg),
    // Properties
    Prop(props::Edit),
    TraceComponent(CompId),
    TraceWire(Wire),
    SetWirePath(Wire, Vec<(i32, i32)>),
    ImportHex,
    HexEdit(text_editor::Action),
    HexPicked(Option<PathBuf>),
    ExportHex,
    HexSavePicked(Option<PathBuf>),
    // Canvas and waveform
    Editor(EditorEvent),
    Wave(WaveMsg),
    WaveExportPicked(bool, Option<PathBuf>),
    Key(keyboard::Event),
    StartResize(Split),
    ResizeMove(Point),
    ResizeEnd,
    // Agents
    #[cfg(feature = "mcp")]
    Mcp(crate::mcp::Event),
    /// A screenshot an agent asked for (JSON-RPC id, where to answer).
    #[cfg(feature = "mcp")]
    McpScreenshot(serde_json::Value, crate::mcp::Reply, Option<window::Screenshot>),
}

pub struct App {
    pub project: Project,
    pub path: Option<PathBuf>,
    pub history: History,
    generation: u64,
    pub loc: Location,
    /// Last view of each circuit, restored when it is shown again.
    views: HashMap<String, (View, bool)>,
    pub cache: CircuitCache,
    pub selection: Selection,
    pub tool: Tool,
    clipboard: Clip,
    paste_count: i32,
    sims: HashMap<String, SimSlot>,
    pub sim_cfg: SimConfig,
    pub zero_delay: bool,
    pub running: bool,
    pub speed: Speed,
    pub step_kind: StepKind,
    last_frame: Option<Instant>,
    tick_debt: f64,
    pending_settle: bool,
    pub error: Option<String>,
    space_down: bool,
    space_panned: bool,
    pub menu: Option<Menu>,
    pub search: String,
    /// Text of the custom speed field, while it is shown.
    pub custom_speed: Option<String>,
    /// Collapsed sections of the component library.
    pub collapsed: HashSet<String>,
    pub wave: WaveView,
    pub canvas_size: Size,
    pub modal: Option<Modal>,
    /// The documentation shown by [`Modal::Help`], parsed.
    help: Vec<iced::widget::markdown::Item>,
    /// The command palette, while it is open.
    pub commands: Option<CommandPalette>,
    after_save: Option<After>,
    pub show_grid: bool,
    /// Text being typed in property fields, by key.
    pub drafts: HashMap<String, String>,
    pub hex_editor: props::HexEditor,
    pub trace_capacity: usize,
    route_memo: HashMap<String, autoroute::Memo>,
    pub left_w: f32,
    pub right_w: f32,
    pub wave_h: f32,
    /// Panel boundary being dragged, with the last cursor position.
    resizing: Option<(Split, Option<Point>)>,
    render_version: u64,
    evals_per_sec: f64,
    fps: f64,
    frames: u64,
    perf_window: (Instant, u64),
    pub settings: Settings,
    /// Where the settings are saved; `None` keeps them for this session.
    settings_path: Option<PathBuf>,
    system_dark: bool,
    #[cfg(feature = "hdl")]
    pub hdl: crate::hdl::Hdl,
    /// The server agents connect to, when started with `--mcp`.
    #[cfg(feature = "mcp")]
    pub mcp: Option<crate::mcp::Server>,
}

impl App {
    /// Creates the application, optionally opening `file` and starting the
    /// simulation right away (`run`), with the settings saved in
    /// `settings_path`.
    pub fn new(file: Option<PathBuf>, run: bool, settings_path: Option<PathBuf>) -> (Self, Task<Message>) {
        let mut project = Project::new(Circuit::new("Main"));
        let mut path = None;
        let mut load_error = None;
        if let Some(p) = file {
            match io::load(&p) {
                Ok(proj) => {
                    project = proj;
                    path = Some(p);
                }
                Err(e) => load_error = Some(e.to_string()),
            }
        }
        let main = project.main().name.clone();
        let mut app = App {
            project,
            path,
            history: History::new(),
            generation: 0,
            loc: Location::new(&main),
            views: HashMap::new(),
            cache: CircuitCache::default(),
            selection: Selection::default(),
            tool: Tool::Select,
            clipboard: Clip::default(),
            paste_count: 0,
            sims: HashMap::new(),
            sim_cfg: SimConfig::default(),
            zero_delay: false,
            running: false,
            speed: Speed::Ticks(200),
            step_kind: StepKind::Clock,
            last_frame: None,
            tick_debt: 0.0,
            pending_settle: true,
            error: None,
            space_down: false,
            space_panned: false,
            menu: None,
            search: String::new(),
            custom_speed: None,
            collapsed: HashSet::new(),
            wave: WaveView::default(),
            canvas_size: Size::new(800.0, 600.0),
            modal: None,
            help: Vec::new(),
            commands: None,
            after_save: None,
            show_grid: true,
            drafts: HashMap::new(),
            hex_editor: props::HexEditor::default(),
            trace_capacity: lb_core::trace::DEFAULT_CAPACITY,
            route_memo: HashMap::new(),
            left_w: 230.0,
            right_w: 300.0,
            wave_h: 230.0,
            resizing: None,
            render_version: 0,
            evals_per_sec: 0.0,
            fps: 0.0,
            frames: 0,
            perf_window: (Instant::now(), 0),
            settings: settings_path.as_deref().map(Settings::load).unwrap_or_default(),
            settings_path,
            system_dark: false,
            #[cfg(feature = "hdl")]
            hdl: crate::hdl::Hdl::default(),
            #[cfg(feature = "mcp")]
            mcp: None,
        };
        app.apply_settings();
        app.refresh();
        app.fit_view();
        app.set_speed(app.speed);
        app.running = run && app.sim().is_some();
        if let Some(e) = load_error {
            app.warn(e);
        }
        (app, iced::system::theme().map(Message::SystemTheme))
    }

    /// Applies and saves new settings.
    pub(crate) fn change_settings(&mut self, settings: Settings) {
        self.settings = settings;
        self.apply_settings();
        if let Some(path) = &self.settings_path
            && let Err(e) = self.settings.save(path)
        {
            self.warn(format!("Could not save the settings to {}: {e}", path.display()));
        }
    }

    /// Sets the colour scheme and the text size used for drawing.
    fn apply_settings(&mut self) {
        theme::set_dark(match self.settings.scheme {
            Scheme::System => self.system_dark,
            Scheme::Light => false,
            Scheme::Dark => true,
        });
        theme::set_text_scale(self.settings.text_size);
        // The cached canvas layers are drawn with the old colours.
        self.render_version += 1;
    }

    /// Changes the simulation speed and zooms the waveform to match it.
    fn set_speed(&mut self, speed: Speed) {
        self.speed = speed;
        if let Some(span) = speed.wave_span() {
            self.wave.set_span(span);
            let now = self.sim().map_or(0, |s| s.time());
            self.wave.follow_to(now);
        }
    }

    // ---- accessors -----------------------------------------------------------------

    pub fn loc(&self) -> &Location {
        &self.loc
    }

    /// The circuits along the current instance path, starting from the root,
    /// with the label of each instance.
    pub fn breadcrumbs(&self) -> Vec<(String, String)> {
        let mut out = vec![(self.loc.root.clone(), String::new())];
        let mut circuit = self.project.circuit(&self.loc.root);
        for id in &self.loc.inst {
            let Some(c) = circuit.and_then(|c| c.component(*id)) else { break };
            out.push((c.props.circuit.clone(), c.props.label.clone()));
            circuit = self.project.circuit(&c.props.circuit);
        }
        out
    }

    pub fn circuit(&self) -> &Circuit {
        self.project.circuit(&self.loc().circuit).unwrap_or(self.project.main())
    }

    pub fn sim(&self) -> Option<&Simulator> {
        self.sims.get(&self.loc().root).map(|s| &s.sim)
    }

    pub(crate) fn sim_mut(&mut self) -> Option<&mut Simulator> {
        let root = self.loc().root.clone();
        self.sims.get_mut(&root).map(|s| &mut s.sim)
    }

    pub fn is_dirty(&self) -> bool {
        #[cfg(feature = "hdl")]
        if self.hdl.has_unsaved() {
            return true;
        }
        self.history.is_dirty()
    }

    pub fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        format!("{}{name} — LogiBuild", if self.is_dirty() { "• " } else { "" })
    }

    /// Why an instance of `child` cannot be placed in the shown circuit, if
    /// it cannot: the main circuit is never a subcircuit, and a circuit cannot
    /// contain itself.
    pub fn placement_refusal(&self, child: &str) -> Option<String> {
        let here = &self.circuit().name;
        if self.project.main().name == child {
            Some("The main circuit cannot be used as a subcircuit.".into())
        } else if self.project.would_recurse(here, child) {
            Some(format!("“{child}” cannot be placed in “{here}”: recursive reference."))
        } else {
            None
        }
    }

    pub fn selected_component(&self) -> Option<&Component> {
        if self.selection.comps.len() == 1 && self.selection.wires.is_empty() {
            self.circuit().component(*self.selection.comps.first()?)
        } else {
            None
        }
    }

    pub fn oscillating(&self) -> HashSet<NetId> {
        self.sim().and_then(|s| s.oscillation()).map(|o| o.nets.iter().copied().collect()).unwrap_or_default()
    }

    // ---- state maintenance -----------------------------------------------------------

    /// Recomputes the geometry cache of the active circuit and brings the
    /// simulation of the active root up to date.
    fn refresh(&mut self) {
        #[cfg(feature = "hdl")]
        crate::hdl::sync(self);
        if self.project.circuit(&self.loc.circuit).is_none() || self.project.circuit(&self.loc.root).is_none() {
            let main = self.project.main().name.clone();
            self.switch_to(&main, &main, Vec::new());
        }
        let circuit = self.circuit().clone();
        let geom = Geometry::new(&self.project, &circuit);
        let memo = self.route_memo.entry(circuit.name.clone()).or_default();
        let routes = autoroute::route_all(&geom, &circuit, memo);
        self.cache = CircuitCache { geom, routes };
        self.render_version += 1;
        self.selection.comps.retain(|id| circuit.component(*id).is_some());
        self.selection.wires.retain(|w| circuit.wires.contains(w));

        let root = self.loc().root.clone();
        let stale = self.sims.get(&root).is_none_or(|s| s.generation != self.generation);
        if stale {
            match compile(&self.project, &root) {
                Ok(nl) => {
                    let sim = match self.sims.get(&root) {
                        Some(old) => Simulator::new_from(nl, self.config(), &old.sim),
                        None => {
                            let mut s = Simulator::new(nl, self.config());
                            s.trace.set_capacity(self.trace_capacity);
                            s
                        }
                    };
                    self.sims.insert(root, SimSlot { sim, generation: self.generation });
                    self.error = None;
                    self.pending_settle = true;
                }
                Err(e) => {
                    self.sims.remove(&root);
                    self.error = Some(e.to_string());
                    self.running = false;
                }
            }
        }
        // An instance path may disappear after an edit.
        let inst = self.loc().inst.clone();
        if !inst.is_empty() && self.sim().is_some_and(|s| s.netlist().instance(&inst).is_none()) {
            self.loc.inst.clear();
            self.loc.root = self.loc.circuit.clone();
            self.refresh();
        }
    }

    fn config(&self) -> SimConfig {
        let mut c = self.sim_cfg;
        if self.zero_delay {
            c.gate_delay = 0;
        }
        c
    }

    /// Runs an edit; returns whether it changed the project.
    pub(crate) fn exec(&mut self, mut cmd: Command) -> bool {
        // Circuits synthesized from HDL change only with their sources.
        let edits = cmd.ops.iter().any(|op| {
            matches!(
                op,
                Op::AddComp(_) | Op::RemoveComp(_) | Op::UpdateComp { .. } | Op::AddWire(_) | Op::RemoveWire(_)
            )
        });
        if edits && self.project.is_hdl(&cmd.circuit) {
            return false;
        }
        self.tidy_wires(&mut cmd);
        let changed = self.history.execute(&mut self.project, cmd);
        if changed {
            self.generation += 1;
            self.refresh();
        }
        changed
    }

    /// Adds to `cmd` the tidying of the waypoints of the wires attached to
    /// the components it moves, turns or resizes: a bend point left on a
    /// straight run or at the tip of a spur is removed, and a corner that a
    /// pin moving alone adds gets one, as does the stub it leaves the pin
    /// with (see [`routing::with_stubs`]).
    fn tidy_wires(&mut self, cmd: &mut Command) {
        let Some(circuit) = self.project.circuit(&cmd.circuit) else { return };
        let mut edited: HashMap<CompId, CompGeom> = HashMap::new();
        // Wires as the command leaves them (`None`: removed), by ends.
        let mut wires: HashMap<(&PinRef, &PinRef), Option<&Wire>> = HashMap::new();
        for op in &cmd.ops {
            match op {
                Op::UpdateComp { after, .. } => {
                    edited.insert(after.id, CompGeom::new(&self.project, after));
                }
                Op::AddWire(w) => _ = wires.insert((&w.from, &w.to), Some(w)),
                Op::RemoveWire(w) => _ = wires.insert((&w.from, &w.to), None),
                _ => {}
            }
        }
        if edited.is_empty() {
            return;
        }
        // `(position, outward direction)` of a pin.
        let pin = |p: &PinRef| {
            let at = |g: &CompGeom| g.layout.pin(&p.1).map(|(i, _)| (g.pins[i], routing::pin_direction(g, i)));
            match edited.get(&p.0) {
                Some(g) => at(g),
                None => at(&CompGeom::new(&self.project, circuit.component(p.0)?)),
            }
        };
        let mut tidied = Vec::new();
        for w in &circuit.wires {
            if !edited.contains_key(&w.from.0) && !edited.contains_key(&w.to.0) {
                continue;
            }
            let Some(w) = wires.get(&(&w.from, &w.to)).copied().unwrap_or(Some(w)) else { continue };
            // Routed automatically: nothing to tidy, and it must stay so.
            if w.path.is_empty() {
                continue;
            }
            let (Some(a), Some(b)) = (pin(&w.from), pin(&w.to)) else { continue };
            // With both ends edited (moved together) the shape is unchanged.
            let path = if edited.contains_key(&w.from.0) && edited.contains_key(&w.to.0) {
                routing::tidy_path(a.0, &w.path, b.0)
            } else {
                routing::with_stubs(a, &w.path, b)
            };
            if path != w.path {
                tidied.push((w.clone(), Wire { path, ..w.clone() }));
            }
        }
        for (old, new) in tidied {
            for s in self.selection.wires.iter_mut().filter(|s| **s == old) {
                *s = new.clone();
            }
            cmd.push(Op::RemoveWire(old));
            cmd.push(Op::AddWire(new));
        }
    }

    fn view_mut(&mut self) -> &mut View {
        &mut self.loc.view
    }

    pub fn fit_view(&mut self) {
        let items: Vec<(i32, i32, i32, i32)> = self.cache.geom.items.values().map(CompGeom::bounds).collect();
        let size = self.canvas_size;
        let view = self.view_mut();
        if items.is_empty() {
            *view = View { scale: DEFAULT_SCALE, offset: Vector::new(size.width / 2.0, size.height / 2.0) };
            return;
        }
        let x0 = items.iter().map(|b| b.0).min().unwrap() as f32 - 2.0;
        let y0 = items.iter().map(|b| b.1).min().unwrap() as f32 - 2.0;
        let x1 = items.iter().map(|b| b.2).max().unwrap() as f32 + 2.0;
        let y1 = items.iter().map(|b| b.3).max().unwrap() as f32 + 2.0;
        let scale = (size.width / (x1 - x0)).min(size.height / (y1 - y0)).clamp(1.0, 40.0);
        let cx = (x0 + x1) / 2.0;
        let cy = (y0 + y1) / 2.0;
        *view = View { scale, offset: Vector::new(size.width / 2.0 - cx * scale, size.height / 2.0 - cy * scale) };
    }

    fn zoom(&mut self, factor: f32, anchor: Point) {
        let view = self.view_mut();
        let new = (view.scale * factor).clamp(1.0, 80.0);
        let f = new / view.scale;
        view.offset = Vector::new(anchor.x - (anchor.x - view.offset.x) * f, anchor.y - (anchor.y - view.offset.y) * f);
        view.scale = new;
    }

    /// Tells why an action could not be done, in a dialog.
    pub(crate) fn warn(&mut self, msg: impl Into<String>) {
        self.modal = Some(Modal::Message(msg.into()));
    }

    /// Redraws the cached layers of the canvas.
    #[cfg_attr(not(feature = "hdl"), allow(dead_code))]
    pub(crate) fn redraw(&mut self) {
        self.render_version += 1;
    }

    /// Makes `circuit` the shown circuit (the instance `inst` of it within
    /// `root` when `inst` is not empty), keeping the view of the previous one
    /// and restoring the last view of the new one.
    fn switch_to(&mut self, circuit: &str, root: &str, inst: Vec<CompId>) {
        self.views.insert(self.loc.circuit.clone(), (self.loc.view, self.loc.view_touched));
        let mut loc = Location::new(circuit);
        loc.root = root.into();
        loc.inst = inst;
        if let Some(&(view, touched)) = self.views.get(circuit) {
            loc.view = view;
            loc.view_touched = touched;
        }
        self.loc = loc;
    }

    /// Shows another circuit or instance, fitting the view the first time.
    fn navigate(&mut self, circuit: &str, root: &str, inst: Vec<CompId>) {
        self.switch_to(circuit, root, inst);
        self.selection.clear();
        self.tool = Tool::Select;
        self.drafts.clear();
        self.refresh();
        if !self.loc.view_touched {
            self.fit_view();
        }
    }

    fn load_project(&mut self, project: Project, path: Option<PathBuf>) {
        self.project = project;
        self.path = path;
        self.history = History::new();
        self.generation += 1;
        self.sims.clear();
        let main = self.project.main().name.clone();
        self.loc = Location::new(&main);
        self.views.clear();
        self.selection.clear();
        self.tool = Tool::Select;
        self.running = false;
        self.drafts.clear();
        let old = std::mem::take(&mut self.wave);
        self.wave.visible = old.visible;
        self.wave.resize(old.size);
        self.refresh();
        self.fit_view();
        self.set_speed(self.speed);
        #[cfg(feature = "hdl")]
        crate::hdl::project_loaded(self);
    }

    pub(crate) fn save_to(&mut self, path: PathBuf) -> bool {
        let path = if path.extension().is_none() { path.with_extension(io::EXTENSION) } else { path };
        #[cfg(feature = "hdl")]
        {
            // Sources stay the same files when the project moves.
            crate::hdl::rebase(&mut self.project, self.path.as_deref(), &path);
            crate::hdl::save_all(self);
        }
        match io::save(&self.project, &path) {
            Ok(()) => {
                self.history.mark_saved();
                self.path = Some(path);
                true
            }
            Err(e) => {
                self.modal = Some(Modal::Message(format!("Cannot save: {e}")));
                false
            }
        }
    }

    fn continue_after(&mut self, after: After) -> Task<Message> {
        match after {
            After::New => {
                self.load_project(Project::new(Circuit::new("Main")), None);
                Task::none()
            }
            After::Open => pick_file_task(),
            After::Quit => iced::exit(),
        }
    }

    fn guard(&mut self, after: After) -> Task<Message> {
        if self.is_dirty() {
            self.modal = Some(Modal::Unsaved(after));
            Task::none()
        } else {
            self.continue_after(after)
        }
    }

    // ---- editing helpers ---------------------------------------------------------------

    fn delete_selection(&mut self) {
        let ids: Vec<CompId> = self.selection.comps.iter().copied().collect();
        let cmd = Command::delete(self.circuit(), &ids, &self.selection.wires);
        self.selection.clear();
        self.exec(cmd);
    }

    fn transform_selection(&mut self, f: impl Fn(&mut Component)) {
        let edited: Vec<Component> = self
            .selection
            .comps
            .iter()
            .filter_map(|id| self.circuit().component(*id))
            .map(|c| {
                let mut c = c.clone();
                f(&mut c);
                c
            })
            .collect();
        let mut cmd = Command::update(&self.project, self.circuit(), edited);
        cmd.label = "Transform".into();
        self.exec(cmd);
    }

    fn paste(&mut self, clip: Clip, offset: (i32, i32)) {
        if clip.is_empty() {
            return;
        }
        // Refuse the main circuit and subcircuits that would create recursion.
        if let Some(why) = clip
            .components
            .iter()
            .filter(|c| c.kind == Kind::Subcircuit)
            .find_map(|c| self.placement_refusal(&c.props.circuit))
        {
            self.warn(format!("Cannot paste: {why}"));
            return;
        }
        let (cmd, ids) = clip.paste(self.circuit(), offset);
        self.exec(cmd);
        self.selection.clear();
        self.selection.comps.extend(ids);
    }

    fn align(&mut self, how: Align) {
        let geoms: Vec<(CompId, (i32, i32, i32, i32))> = self
            .selection
            .comps
            .iter()
            .filter_map(|id| self.cache.geom.items.get(id).map(|g| (*id, g.bounds())))
            .collect();
        if geoms.len() < 2 {
            return;
        }
        let x0 = geoms.iter().map(|g| g.1.0).min().unwrap();
        let y0 = geoms.iter().map(|g| g.1.1).min().unwrap();
        let x1 = geoms.iter().map(|g| g.1.2).max().unwrap();
        let y1 = geoms.iter().map(|g| g.1.3).max().unwrap();
        let edited: Vec<Component> = geoms
            .iter()
            .filter_map(|(id, b)| {
                let mut c = self.circuit().component(*id)?.clone();
                let (w, h) = (b.2 - b.0, b.3 - b.1);
                match how {
                    Align::Left => c.pos.0 = x0,
                    Align::Right => c.pos.0 = x1 - w,
                    Align::CenterX => c.pos.0 = (x0 + x1) / 2 - w / 2,
                    Align::Top => c.pos.1 = y0,
                    Align::Bottom => c.pos.1 = y1 - h,
                    Align::CenterY => c.pos.1 = (y0 + y1) / 2 - h / 2,
                }
                Some(c)
            })
            .collect();
        let mut cmd = Command::update(&self.project, self.circuit(), edited);
        cmd.label = "Align".into();
        self.exec(cmd);
    }

    fn add_trace(&mut self, comp: CompId, pin: usize) {
        let inst = self.loc().inst.clone();
        let name_hint = self.circuit().component(comp).map(|c| (c.props.label.clone(), c.kind));
        let Some(sim) = self.sim_mut() else { return };
        let Some(net) = sim.netlist().pin_net(&inst, comp, pin) else {
            return;
        };
        let mut name = sim.netlist().nets[net as usize].name.clone();
        if let Some((label, kind)) = name_hint
            && !label.is_empty()
            && matches!(kind, Kind::Probe | Kind::Output | Kind::Input)
            && !name.ends_with(&label)
        {
            name = label;
        }
        let value = sim.net_value(net);
        let time = sim.time();
        sim.trace.add(name.clone(), net, value, time, Some((inst.into(), comp, pin)));
        self.wave.visible = true;
    }

    /// Settles the simulation now rather than over the next frames.
    #[cfg_attr(not(feature = "mcp"), allow(dead_code))]
    pub(crate) fn settle(&mut self) {
        let Some(sim) = self.sim_mut() else { return };
        match sim.settle(1_000_000) {
            Ok(settled) => self.pending_settle = !settled,
            Err(_) => self.report_osc(),
        }
    }

    #[cfg(feature = "mcp")]
    fn mcp_event(&mut self, event: crate::mcp::Event) -> Task<Message> {
        use crate::mcp::Event;
        match event {
            Event::Listening => {}
            Event::Failed(e) => {
                self.warn(format!("Cannot listen for agents: {e}"));
                self.mcp = None;
            }
            Event::Connected(on) => {
                if let Some(server) = &mut self.mcp {
                    server.connected = on;
                }
            }
            Event::Call(call) => return crate::mcp::handle(self, call),
            Event::Frame => return self.mcp.as_mut().map_or(Task::none(), crate::mcp::frame),
        }
        Task::none()
    }

    fn report_osc(&mut self) {
        self.running = false;
        self.pending_settle = false;
    }

    // ---- update ----------------------------------------------------------------------

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        let selected = self.selected_component();
        if self.hex_editor.stale(selected) {
            let selected = selected.cloned();
            self.hex_editor.sync(selected.as_ref());
        }
        task
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        // Messages of the user close the open menu; those from timers and
        // background jobs do not.
        if !self.is_background(&message)
            && !matches!(
                message,
                Message::Frame(_)
                    | Message::Menu(_)
                    | Message::Editor(_)
                    | Message::Wave(_)
                    | Message::Key(_)
                    | Message::ResizeMove(_)
                    | Message::StartResize(_)
                    | Message::ResizeEnd
                    | Message::SystemTheme(_)
                    | Message::SetTextSize(_)
            )
        {
            self.menu = None;
        }
        match message {
            Message::Noop => {}
            Message::Menu(m) => self.menu = if self.menu == m { None } else { m },

            // ---- file
            Message::New => return self.guard(After::New),
            Message::Open => return self.guard(After::Open),
            Message::OpenPicked(Some(p)) => match io::load(&p) {
                Ok(project) => {
                    self.load_project(project, Some(p.clone()));
                }
                Err(e) => self.modal = Some(Modal::Message(e.to_string())),
            },
            Message::OpenPicked(None) => {}
            Message::Save => match self.path.clone() {
                Some(p) => {
                    self.save_to(p);
                }
                None => return save_file_task("circuit.lbc", "LogiBuild", &["lbc"], Message::SavePicked),
            },
            Message::SaveAs => {
                let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
                return save_file_task(
                    &name.unwrap_or_else(|| "circuit.lbc".into()),
                    "LogiBuild",
                    &["lbc"],
                    Message::SavePicked,
                );
            }
            Message::SavePicked(Some(p)) => {
                if self.save_to(p)
                    && let Some(after) = self.after_save.take()
                {
                    return self.continue_after(after);
                }
            }
            Message::SavePicked(None) => self.after_save = None,
            Message::ModalSave => {
                if let Some(Modal::Unsaved(after)) = self.modal.take() {
                    match self.path.clone() {
                        Some(p) => {
                            if self.save_to(p) {
                                return self.continue_after(after);
                            }
                        }
                        None => {
                            self.after_save = Some(after);
                            return save_file_task("circuit.lbc", "LogiBuild", &["lbc"], Message::SavePicked);
                        }
                    }
                }
            }
            Message::ModalDiscard => {
                if let Some(Modal::Unsaved(after)) = self.modal.take() {
                    return self.continue_after(after);
                }
            }
            Message::ModalCancel => self.modal = None,
            Message::ShowHelp(kind) => {
                if let Some(doc) = crate::help::section(kind) {
                    self.help = iced::widget::markdown::parse(doc).collect();
                    self.modal = Some(Modal::Help(kind));
                }
            }
            Message::CloseRequested | Message::Quit => return self.guard(After::Quit),

            // ---- edit
            Message::Undo => {
                if let Some(cmd) = self.history.undo(&mut self.project) {
                    let circuit = cmd.circuit.clone();
                    self.after_history(&circuit);
                }
            }
            Message::Redo => {
                if let Some(cmd) = self.history.redo(&mut self.project) {
                    let circuit = cmd.circuit_after().to_string();
                    self.after_history(&circuit);
                }
            }
            Message::Copy | Message::Cut => {
                let ids: Vec<CompId> = self.selection.comps.iter().copied().collect();
                if !ids.is_empty() {
                    self.clipboard = Clip::copy(self.circuit(), &ids);
                    self.paste_count = 0;
                    if matches!(message, Message::Cut) {
                        self.delete_selection();
                    }
                }
            }
            Message::Paste => {
                self.paste_count += 1;
                let d = 2 * self.paste_count;
                self.paste(self.clipboard.clone(), (d, d));
            }
            Message::Duplicate => {
                let ids: Vec<CompId> = self.selection.comps.iter().copied().collect();
                let clip = Clip::copy(self.circuit(), &ids);
                self.paste(clip, (2, 2));
            }
            Message::Delete => self.delete_selection(),
            Message::SelectAll => {
                self.selection.comps = self.circuit().components.iter().map(|c| c.id).collect();
                self.selection.wires.clear();
            }
            Message::Rotate => match &mut self.tool {
                Tool::Place { rot, .. } => *rot = (*rot + 1) % 4,
                Tool::Select => self.transform_selection(|c| c.rot = (c.rot + 1) % 4),
            },
            Message::Mirror => match &mut self.tool {
                Tool::Place { mirror, .. } => *mirror = !*mirror,
                Tool::Select => self.transform_selection(|c| c.mirror = !c.mirror),
            },
            Message::Align(a) => self.align(a),

            // ---- simulation
            Message::ToggleRun => {
                if self.sim().is_some_and(|s| s.oscillation().is_none()) {
                    self.running = !self.running;
                    self.last_frame = None;
                }
            }
            Message::Reset => {
                if let Some(sim) = self.sim_mut() {
                    sim.reset();
                }
                self.pending_settle = true;
                self.wave.start = 0.0;
            }
            Message::Step => {
                self.running = false;
                let kind = self.step_kind;
                if let Some(sim) = self.sim_mut() {
                    let r = match kind {
                        StepKind::Event => sim.step_event().map(|_| ()),
                        StepKind::Delta => sim.step_delta().map(|_| ()),
                        StepKind::Tick => sim.step_tick(),
                        StepKind::Clock => sim.step_clock(),
                    };
                    if r.is_err() {
                        self.report_osc();
                    }
                    self.pending_settle = false;
                }
                let now = self.sim().map_or(0, |s| s.time());
                self.wave.follow_to(now);
            }
            Message::SetStepKind(k) => self.step_kind = k,
            Message::SetSpeed(s) => {
                self.set_speed(s);
                self.custom_speed = None;
            }
            Message::EditCustomSpeed => {
                let n = match self.speed {
                    Speed::Ticks(n) => n,
                    Speed::Max => 1_000,
                };
                self.custom_speed = Some(n.to_string());
                return Task::batch([
                    iced::widget::operation::focus(CUSTOM_SPEED_ID),
                    iced::widget::operation::select_all(CUSTOM_SPEED_ID),
                ]);
            }
            Message::CustomSpeed(t) => {
                let digits: String = t.chars().filter(|c| !matches!(c, ' ' | '_' | '\'' | ',' | '.')).collect();
                if let Ok(n) = digits.parse::<u32>()
                    && (1..=MAX_CUSTOM_SPEED).contains(&n)
                {
                    self.set_speed(Speed::Ticks(n));
                }
                self.custom_speed = Some(t);
            }
            Message::CustomSpeedDone => self.custom_speed = None,
            Message::SetZeroDelay(z) => {
                self.zero_delay = z;
                self.apply_config();
            }
            Message::SetGateDelay(s) => {
                if let Ok(v) = s.trim().parse::<u32>() {
                    self.sim_cfg.gate_delay = v.clamp(1, 1_000_000);
                    self.apply_config();
                }
                self.drafts.insert("cfg.delay".into(), s);
            }
            Message::SetDeltaLimit(s) => {
                if let Ok(v) = s.trim().parse::<u32>() {
                    self.sim_cfg.delta_limit = v.clamp(10, 10_000_000);
                    self.apply_config();
                }
                self.drafts.insert("cfg.delta".into(), s);
            }
            Message::SetTraceCapacity(s) => {
                if let Ok(v) = s.trim().parse::<usize>() {
                    self.trace_capacity = v.clamp(100, 100_000_000);
                    let cap = self.trace_capacity;
                    for slot in self.sims.values_mut() {
                        slot.sim.trace.set_capacity(cap);
                    }
                }
                self.drafts.insert("cfg.trace".into(), s);
            }
            Message::Frame(now) => self.frame(now),
            #[cfg(feature = "mcp")]
            Message::Mcp(event) => return self.mcp_event(event),
            #[cfg(feature = "mcp")]
            Message::McpScreenshot(id, reply, shot) => crate::mcp::screenshot_taken(id, reply, shot),

            // ---- view
            Message::FitView => self.fit_view(),
            Message::ZoomIn => {
                let c = Point::new(self.canvas_size.width / 2.0, self.canvas_size.height / 2.0);
                self.zoom(1.25, c);
            }
            Message::ZoomOut => {
                let c = Point::new(self.canvas_size.width / 2.0, self.canvas_size.height / 2.0);
                self.zoom(0.8, c);
            }
            Message::ToggleGrid => self.show_grid = !self.show_grid,
            Message::ToggleWaveform => self.wave.visible = !self.wave.visible,

            // ---- command palette
            Message::OpenCommands => {
                if self.modal.is_some() {
                    return Task::none();
                }
                self.commands = Some(CommandPalette::default());
                return Task::batch([
                    iced::widget::operation::focus(commands::INPUT_ID),
                    iced::widget::operation::select_all(commands::INPUT_ID),
                ]);
            }
            Message::CloseCommands => self.commands = None,
            Message::CommandQuery(q) => {
                if let Some(p) = &mut self.commands {
                    *p = CommandPalette { query: q, ..CommandPalette::default() };
                }
            }
            Message::CommandMove(delta) => {
                if let Some(p) = &self.commands {
                    let n = commands::matches(self, p).len();
                    if let Some(p) = &mut self.commands {
                        p.step(delta, n);
                    }
                }
            }
            Message::CommandRun(i) => {
                let Some(p) = &self.commands else { return Task::none() };
                if let Some(e) = commands::matches(self, p).into_iter().nth(i).filter(|e| e.enabled) {
                    self.commands = None;
                    return self.update(e.msg);
                }
            }
            Message::SetScheme(scheme) => self.change_settings(Settings { scheme, ..self.settings.clone() }),
            Message::SetTextSize(n) => {
                let text_size = n.clamp(*TEXT_SIZE_RANGE.start(), *TEXT_SIZE_RANGE.end());
                self.change_settings(Settings { text_size, ..self.settings.clone() });
            }
            Message::SystemTheme(mode) => {
                self.system_dark = mode == iced::theme::Mode::Dark;
                self.apply_settings();
            }

            // ---- palette, circuits
            Message::Search(s) => self.search = s,
            Message::ToggleSection(name) => {
                if !self.collapsed.remove(&name) {
                    self.collapsed.insert(name);
                }
            }
            Message::PickTool(kind) => {
                self.tool = Tool::Place { kind, props: Box::new(kind.default_props()), rot: 0, mirror: false };
            }
            Message::PickSubcircuit(name) => {
                if self.placement_refusal(&name).is_none() {
                    let mut props = Kind::Subcircuit.default_props();
                    props.circuit = name.clone();
                    self.tool = Tool::Place { kind: Kind::Subcircuit, props: Box::new(props), rot: 0, mirror: false };
                }
            }
            Message::NewCircuit => {
                let name = self.project.unique_name("Circuit");
                let index = self.project.circuits.len();
                let mut cmd = Command::new("New circuit", &name);
                cmd.push(Op::AddCircuit(index, Box::new(Circuit::new(&name))));
                self.exec(cmd);
                self.navigate(&name, &name, Vec::new());
            }
            Message::OpenCircuit(name) => {
                if self.project.circuit(&name).is_some() {
                    self.navigate(&name, &name, Vec::new());
                }
            }
            Message::Breadcrumb(depth) => {
                let crumbs = self.breadcrumbs();
                if depth + 1 < crumbs.len() {
                    let inst = self.loc.inst[..depth].to_vec();
                    let root = self.loc.root.clone();
                    self.navigate(&crumbs[depth].0, &root, inst);
                }
            }
            Message::RenameCircuit(_) if self.project.is_hdl(&self.circuit().name) => {
                self.warn("A circuit synthesized from HDL is named after its entity: rename it in the sources.");
            }
            Message::RenameCircuit(new) => {
                let old = self.circuit().name.clone();
                let new_trim = new.trim().to_string();
                self.drafts.insert("circuit.name".into(), new.clone());
                if !new_trim.is_empty() && new_trim != old && self.project.circuit(&new_trim).is_none() {
                    let mut cmd = Command::new("Rename circuit", &old);
                    cmd.push(Op::RenameCircuit { from: old.clone(), to: new_trim.clone() });
                    cmd.merge = Some("rename-circuit".into());
                    if self.history.execute(&mut self.project, cmd) {
                        self.generation += 1;
                        if self.loc.circuit == old {
                            self.loc.circuit = new_trim.clone();
                        }
                        if self.loc.root == old {
                            self.loc.root = new_trim.clone();
                        }
                        if let Some(view) = self.views.remove(&old) {
                            self.views.insert(new_trim.clone(), view);
                        }
                        if let Some(slot) = self.sims.remove(&old) {
                            self.sims.insert(new_trim.clone(), slot);
                        }
                        self.refresh();
                    }
                }
            }
            Message::DeleteCircuit => {
                let name = self.circuit().name.clone();
                let used = self.project.circuits.iter().any(|c| c.subcircuit_refs().any(|r| r == name));
                let index = self.project.circuit_index(&name).unwrap_or(0);
                if index == 0 {
                    self.warn("The main circuit cannot be deleted.");
                } else if used {
                    self.warn(format!("“{name}” is used as a subcircuit: remove its instances first."));
                } else {
                    let mut cmd = Command::new("Delete circuit", &name);
                    cmd.push(Op::RemoveCircuit(index, Box::new(self.circuit().clone())));
                    // Not synthesized again.
                    if self.project.hdl.tops.contains(&name) {
                        let mut after = self.project.hdl.clone();
                        after.tops.retain(|t| *t != name);
                        cmd.push(Op::SetHdl { before: Box::new(self.project.hdl.clone()), after: Box::new(after) });
                    }
                    let main = self.project.main().name.clone();
                    self.navigate(&main, &main, Vec::new());
                    self.views.remove(&name);
                    self.exec(cmd);
                }
            }
            Message::MakeMain => {
                let name = self.circuit().name.clone();
                let used = self.project.circuits.iter().any(|c| c.subcircuit_refs().any(|r| r == name));
                if self.project.is_hdl(&name) {
                    self.warn("A circuit synthesized from HDL is always a subcircuit: convert it first.");
                } else if used {
                    self.warn(format!(
                        "“{name}” is used as a subcircuit: remove its instances before making it the main circuit."
                    ));
                } else if let Some(i) = self.project.circuit_index(&name)
                    && i > 0
                {
                    let c = self.project.circuits[i].clone();
                    let mut cmd = Command::new("Set as main", &name);
                    cmd.push(Op::RemoveCircuit(i, Box::new(c.clone())));
                    cmd.push(Op::AddCircuit(0, Box::new(c)));
                    self.exec(cmd);
                }
            }

            Message::ConvertCircuit(name) => {
                if let Some(i) = self.project.circuit_index(&name)
                    && self.project.circuits[i].hdl.is_some()
                {
                    let old = self.project.circuits[i].clone();
                    let new = lb_core::circuit::Circuit { hdl: None, ..old.clone() };
                    let mut cmd = Command::new("Convert to circuit", &name);
                    cmd.push(Op::RemoveCircuit(i, Box::new(old)));
                    cmd.push(Op::AddCircuit(i, Box::new(new)));
                    if self.project.hdl.tops.contains(&name) {
                        let mut after = self.project.hdl.clone();
                        after.tops.retain(|t| *t != name);
                        cmd.push(Op::SetHdl { before: Box::new(self.project.hdl.clone()), after: Box::new(after) });
                    }
                    self.exec(cmd);
                }
            }

            // ---- HDL
            Message::SetHdlEnabled(on) => {
                self.change_settings(Settings { hdl: on, ..self.settings.clone() });
                self.refresh();
            }
            Message::SetHdlAuto(on) => self.change_settings(Settings { hdl_auto: on, ..self.settings.clone() }),
            #[cfg(feature = "hdl")]
            Message::Hdl(m) => return crate::hdl::update(self, m),

            // ---- properties
            Message::ClearKeyboard(id) => {
                let inst = self.loc().inst.clone();
                if let Some(sim) = self.sim_mut()
                    && let Some(i) = sim.netlist().comp_index(&inst, id)
                {
                    sim.clear_keys(i);
                    self.pending_settle = true;
                }
            }
            Message::Prop(edit) => {
                if let Some(cmd) = props::apply(self, edit) {
                    self.exec(cmd);
                }
            }
            Message::TraceComponent(id) => {
                let n = self.cache.geom.items.get(&id).map_or(0, |g| g.pins.len());
                let outputs: Vec<usize> = self
                    .cache
                    .geom
                    .items
                    .get(&id)
                    .map(|g| {
                        g.layout
                            .pins
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| p.dir == lb_core::component::PinDir::Out)
                            .map(|(i, _)| i)
                            .collect()
                    })
                    .unwrap_or_default();
                if outputs.is_empty() {
                    for i in 0..n.min(1) {
                        self.add_trace(id, i);
                    }
                } else {
                    for i in outputs {
                        self.add_trace(id, i);
                    }
                }
            }
            Message::SetWirePath(w, path) => self.set_wire_path(w, path),
            Message::TraceWire(w) => {
                if let Some(g) = self.cache.geom.items.get(&w.from.0)
                    && let Some((i, _)) = g.layout.pin(&w.from.1)
                {
                    self.add_trace(w.from.0, i);
                }
            }
            Message::HexEdit(action) => {
                if let Some(cmd) = props::edit_hex(self, action) {
                    self.exec(cmd);
                }
            }
            Message::ImportHex => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .add_filter("Hex image", &["hex", "txt"])
                            .pick_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::HexPicked,
                );
            }
            Message::HexPicked(Some(p)) => {
                match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| io::parse_hex(&t)) {
                    Ok(words) => {
                        if let Some(mut c) = self.selected_component().cloned() {
                            c.props.data = io::format_hex(&words, false);
                            let cmd = Command::update(&self.project, self.circuit(), vec![c]);
                            self.exec(cmd);
                        }
                    }
                    Err(e) => self.modal = Some(Modal::Message(format!("{}: {e}", p.display()))),
                }
            }
            Message::HexPicked(None) => {}
            Message::ExportHex => {
                return save_file_task("memory.hex", "Hex image", &["hex"], Message::HexSavePicked);
            }
            Message::HexSavePicked(Some(p)) => {
                if let Some(c) = self.selected_component() {
                    // Export the live contents if simulating, else the properties.
                    let inst = self.loc().inst.clone();
                    let live = self
                        .sim()
                        .and_then(|s| s.netlist().comp_index(&inst, c.id).map(|i| s.state(i).mem.clone()))
                        .filter(|m| !m.is_empty());
                    let words = live.unwrap_or_else(|| io::parse_hex(&c.props.data).unwrap_or_default());
                    match std::fs::write(&p, io::format_hex(&words, true)) {
                        Ok(()) => {}
                        Err(e) => self.modal = Some(Modal::Message(e.to_string())),
                    }
                }
            }
            Message::HexSavePicked(None) => {}

            // ---- canvas
            Message::Editor(ev) => return self.editor_event(ev),
            Message::Wave(msg) => return self.wave_msg(msg),
            Message::WaveExportPicked(vcd, Some(p)) => {
                if let Some(sim) = self.sim() {
                    let content = if vcd { sim.trace.export_vcd("1ns", sim.time()) } else { sim.trace.export_csv() };
                    match std::fs::write(&p, content) {
                        Ok(()) => {}
                        Err(e) => self.modal = Some(Modal::Message(e.to_string())),
                    }
                }
            }
            Message::WaveExportPicked(_, None) => {}
            Message::Key(ev) => return self.key(ev),
            Message::StartResize(which) => self.resizing = Some((which, None)),
            Message::ResizeMove(p) => {
                if let Some((which, last)) = &mut self.resizing {
                    if let Some(l) = *last {
                        let (dx, dy) = (p.x - l.x, p.y - l.y);
                        match which {
                            Split::Left => self.left_w = (self.left_w + dx).clamp(160.0, 520.0),
                            Split::Right => self.right_w = (self.right_w - dx).clamp(220.0, 640.0),
                            Split::Wave => self.wave_h = (self.wave_h - dy).clamp(90.0, 700.0),
                            #[cfg(feature = "hdl")]
                            Split::HdlSources => {
                                self.hdl.sources_w = (self.hdl.sources_w + dx).clamp(160.0, 700.0);
                            }
                            #[cfg(feature = "hdl")]
                            Split::HdlLog => self.hdl.log_w = (self.hdl.log_w - dx).clamp(160.0, 900.0),
                        }
                    }
                    *last = Some(p);
                }
            }
            Message::ResizeEnd => self.resizing = None,
        }
        Task::none()
    }

    /// A message not caused by the user.
    fn is_background(&self, message: &Message) -> bool {
        #[cfg(feature = "hdl")]
        if let Message::Hdl(m) = message {
            return matches!(m, crate::hdl::Msg::Poll | crate::hdl::Msg::Done(..));
        }
        #[cfg(feature = "mcp")]
        if matches!(message, Message::Mcp(_) | Message::McpScreenshot(..)) {
            return true;
        }
        let _ = message;
        false
    }

    fn after_history(&mut self, circuit: &str) {
        self.generation += 1;
        // Undoing an edit of another circuit shows that circuit.
        let switched = self.project.circuit(circuit).is_some() && self.circuit().name != circuit;
        if switched {
            self.switch_to(circuit, circuit, Vec::new());
        }
        self.drafts.clear();
        self.refresh();
        if switched && !self.loc.view_touched {
            self.fit_view();
        }
    }

    fn apply_config(&mut self) {
        let cfg = self.config();
        for slot in self.sims.values_mut() {
            slot.sim.set_config(cfg);
        }
    }

    fn frame(&mut self, now: Instant) {
        self.frames += 1;
        let dt = self.last_frame.map_or(0.0, |t| (now - t).as_secs_f64()).min(0.1);
        self.last_frame = Some(now);
        let running = self.running;
        let speed = self.speed;
        let pending = self.pending_settle;
        let root = self.loc.root.clone();
        let Some(slot) = self.sims.get_mut(&root) else { return };
        let sim = &mut slot.sim;
        let result = if running {
            match speed {
                Speed::Ticks(n) => {
                    self.tick_debt += n as f64 * dt;
                    let ticks = self.tick_debt.floor();
                    self.tick_debt -= ticks;
                    let target = sim.time() + ticks as u64;
                    match sim.run_until_limited(target, 3_000_000) {
                        Ok(false) => {
                            self.tick_debt = 0.0;
                            Ok(())
                        }
                        r => r.map(|_| ()),
                    }
                }
                Speed::Max => {
                    let start = Instant::now();
                    let mut r = Ok(());
                    while start.elapsed() < Duration::from_millis(14) {
                        let t = sim.time() + 1_000_000;
                        if let Err(e) = sim.run_until_limited(t, 50_000) {
                            r = Err(e);
                            break;
                        }
                        if sim.is_stable() && !sim.has_clocks() {
                            let t = sim.time();
                            let _ = sim.run_until(t + 1);
                            break;
                        }
                    }
                    r
                }
            }
        } else if pending {
            match sim.settle(20_000) {
                Ok(true) => {
                    self.pending_settle = false;
                    Ok(())
                }
                Ok(false) => Ok(()),
                Err(e) => Err(e),
            }
        } else {
            Ok(())
        };
        let evals = sim.stats.evaluations;
        let t = sim.time();
        if result.is_err() {
            self.report_osc();
        }
        let (t0, e0) = self.perf_window;
        let el = (now - t0).as_secs_f64();
        if el >= 1.0 {
            self.evals_per_sec = (evals.saturating_sub(e0)) as f64 / el;
            self.fps = self.frames as f64 / el;
            self.frames = 0;
            self.perf_window = (now, evals);
        }
        self.wave.follow_to(t);
    }

    fn editor_event(&mut self, ev: EditorEvent) -> Task<Message> {
        match ev {
            EditorEvent::Many(list) => {
                let tasks: Vec<Task<Message>> = list.into_iter().map(|e| self.editor_event(e)).collect();
                return Task::batch(tasks);
            }
            EditorEvent::Resized(size) => {
                self.canvas_size = size;
                if !self.loc().view_touched {
                    self.fit_view();
                }
            }
            EditorEvent::Pan(d) => {
                self.loc.view_touched = true;
                let v = self.view_mut();
                v.offset += d;
            }
            EditorEvent::Zoom { factor, anchor } => {
                self.loc.view_touched = true;
                self.zoom(factor, anchor);
            }
            EditorEvent::Select { id, additive } => {
                if additive {
                    if !self.selection.comps.remove(&id) {
                        self.selection.comps.insert(id);
                    }
                } else {
                    self.selection.clear();
                    self.selection.comps.insert(id);
                }
                self.drafts.clear();
            }
            EditorEvent::SelectWire { wire, additive } => {
                if !additive {
                    self.selection.clear();
                }
                if let Some(i) = self.selection.wires.iter().position(|w| *w == wire) {
                    self.selection.wires.remove(i);
                } else {
                    self.selection.wires.push(wire);
                }
                self.drafts.clear();
            }
            EditorEvent::SelectRect { from, to, additive } => {
                if !additive {
                    self.selection.clear();
                }
                let (x0, x1) = (from.0.min(to.0), from.0.max(to.0));
                let (y0, y1) = (from.1.min(to.1), from.1.max(to.1));
                for (id, g) in &self.cache.geom.items {
                    let b = g.bounds();
                    if b.0 as f32 >= x0 && b.2 as f32 <= x1 && b.1 as f32 >= y0 && b.3 as f32 <= y1 {
                        self.selection.comps.insert(*id);
                    }
                }
                let ids = self.selection.comps.clone();
                let inner: Vec<Wire> = self
                    .circuit()
                    .wires
                    .iter()
                    .filter(|w| ids.contains(&w.from.0) && ids.contains(&w.to.0) && !self.selection.wires.contains(w))
                    .cloned()
                    .collect();
                self.selection.wires.extend(inner);
                self.drafts.clear();
            }
            EditorEvent::ClearSelection => {
                self.selection.clear();
                self.drafts.clear();
            }
            EditorEvent::MoveSelection { dx, dy } => {
                let ids: Vec<CompId> = self.selection.comps.iter().copied().collect();
                let cmd = Command::translate(self.circuit(), &ids, dx, dy);
                // Wires between moved components keep their selection even
                // though their waypoints move with them.
                for w in &mut self.selection.wires {
                    if !w.path.is_empty()
                        && self.selection.comps.contains(&w.from.0)
                        && self.selection.comps.contains(&w.to.0)
                    {
                        for p in &mut w.path {
                            *p = (p.0 + dx, p.1 + dy);
                        }
                    }
                }
                self.exec(cmd);
            }
            EditorEvent::Place { pos } => {
                if let Some(mut c) = self.tool.ghost(pos) {
                    if c.kind == Kind::Subcircuit && self.placement_refusal(&c.props.circuit).is_some() {
                        return Task::none();
                    }
                    c.id = self.circuit().next_id();
                    let id = c.id;
                    let cmd = Command::add(self.circuit(), vec![c], vec![]);
                    self.exec(cmd);
                    self.selection.clear();
                    self.selection.comps.insert(id);
                }
            }
            EditorEvent::Connect { from, to } => self.connect(from, to),
            EditorEvent::ConnectToWire { from, wire } => {
                // Join the net of the wire: attach to its driving end.
                let target = wire.from.clone();
                if target != from {
                    self.connect(from, target);
                }
            }
            EditorEvent::Poke { id } => self.poke(id),
            EditorEvent::Button { id, pressed } => {
                let inst = self.loc().inst.clone();
                if let Some(sim) = self.sim_mut()
                    && let Some(i) = sim.netlist().comp_index(&inst, id)
                {
                    sim.set_input(i, Signal::bool(pressed));
                    self.pending_settle = true;
                }
            }
            EditorEvent::OpenInstance { id } => {
                let Some(c) = self.circuit().component(id) else { return Task::none() };
                let name = c.props.circuit.clone();
                if self.project.circuit(&name).is_none() {
                    return Task::none();
                }
                let mut inst = self.loc.inst.clone();
                inst.push(id);
                let root = self.loc.root.clone();
                self.navigate(&name, &root, inst);
            }
            EditorEvent::Trace { comp, pin } => self.add_trace(comp, pin),
            EditorEvent::CancelTool => {
                self.tool = Tool::Select;
            }
            EditorEvent::SpacePan => self.space_panned = true,
            EditorEvent::SetWirePath { wire, path } => self.set_wire_path(wire, path),
        }
        Task::none()
    }

    /// Replaces the waypoints of a wire (empty: automatic routing).
    pub fn set_wire_path(&mut self, wire: Wire, path: Vec<(i32, i32)>) {
        if wire.path == path || !self.circuit().wires.contains(&wire) {
            return;
        }
        let new = Wire { path, ..wire.clone() };
        let mut cmd = Command::new("Edit wire", &self.circuit().name);
        cmd.push(Op::RemoveWire(wire.clone()));
        cmd.push(Op::AddWire(new.clone()));
        for w in &mut self.selection.wires {
            if *w == wire {
                *w = new.clone();
            }
        }
        self.exec(cmd);
    }

    /// The current route of a wire, as computed by the router.
    pub fn wire_route(&self, wire: &Wire) -> Option<&[(i32, i32)]> {
        let i = self.circuit().wires.iter().position(|w| w == wire)?;
        self.cache.routes.wires.get(i).map(|r| r.points.as_slice())
    }

    fn connect(&mut self, a: PinRef, b: PinRef) {
        let wire = orient_wire(&self.project, self.circuit(), a, b);
        if self.circuit().wires.iter().any(|w| w.same_ends(&wire)) {
            return;
        }
        let mut cmd = Command::add(self.circuit(), vec![], vec![wire]);
        cmd.label = "Connect".into();
        self.exec(cmd);
    }

    fn poke(&mut self, id: CompId) {
        let inst = self.loc().inst.clone();
        let Some(c) = self.circuit().component(id) else { return };
        if c.kind != Kind::Input || c.props.width() != 1 {
            return;
        }
        if let Some(sim) = self.sim_mut()
            && let Some(i) = sim.netlist().comp_index(&inst, id)
        {
            let v = sim.input_value(i);
            sim.set_input(i, Signal::bool(!v.is_one()));
            self.pending_settle = true;
        }
    }

    /// The keyboard component that receives the typed keys: the only
    /// selected component, when it is a keyboard, the circuit simulates and
    /// no component is being placed.
    pub fn typing_target(&self) -> Option<CompId> {
        let sel = &self.selection;
        if sel.comps.len() != 1 || !sel.wires.is_empty() || !matches!(self.tool, Tool::Select) {
            return None;
        }
        let &id = sel.comps.iter().next()?;
        let c = self.circuit().component(id)?;
        let sim = self.sim()?;
        (c.kind == Kind::Keyboard && sim.netlist().comp_index(&self.loc().inst, id).is_some()).then_some(id)
    }

    /// Characters waiting in the buffer of a keyboard component, when the
    /// circuit simulates.
    pub fn keyboard_keys(&self, id: CompId) -> Option<usize> {
        let sim = self.sim()?;
        let i = sim.netlist().comp_index(&self.loc().inst, id)?;
        Some(lb_core::component::eval::keyboard_buffer(&sim.state(i).mem).len())
    }

    fn type_key(&mut self, id: CompId, c: u8) {
        let inst = self.loc().inst.clone();
        if let Some(sim) = self.sim_mut()
            && let Some(i) = sim.netlist().comp_index(&inst, id)
        {
            sim.type_keys(i, &[c]);
            self.pending_settle = true;
        }
    }

    /// Sets the live value of an input (from the properties panel).
    pub fn set_live_input(&mut self, id: CompId, value: Signal) {
        let inst = self.loc().inst.clone();
        if let Some(sim) = self.sim_mut()
            && let Some(i) = sim.netlist().comp_index(&inst, id)
        {
            sim.set_input(i, value);
            self.pending_settle = true;
        }
    }

    pub(crate) fn editor_program(&self) -> EditorProgram<'_> {
        EditorProgram { oscillating: self.oscillating(), app: self }
    }

    pub fn live_input(&self, id: CompId) -> Option<Signal> {
        let sim = self.sim()?;
        let i = sim.netlist().comp_index(&self.loc().inst, id)?;
        Some(sim.input_value(i))
    }

    fn wave_msg(&mut self, msg: WaveMsg) -> Task<Message> {
        let now = self.sim().map_or(0, |s| s.time());
        match msg {
            WaveMsg::Resized(size) => self.wave.resize(size),
            WaveMsg::NamesWidth(w) => self.wave.set_names_width(w),
            WaveMsg::Select(i) => self.wave.selected = Some(i),
            WaveMsg::CycleRadix(i) => {
                if let Some(sim) = self.sim_mut()
                    && let Some(s) = sim.trace.signals_mut().get_mut(i)
                {
                    let all = lb_core::value::Radix::ALL;
                    let k = all.iter().position(|r| *r == s.radix).unwrap_or(0);
                    s.radix = all[(k + 1) % all.len()];
                }
                self.wave.selected = Some(i);
            }
            WaveMsg::Remove(i) => {
                if let Some(sim) = self.sim_mut() {
                    sim.trace.remove(i);
                }
                self.wave.selected = None;
            }
            WaveMsg::Scroll(start) => {
                self.wave.start = start;
                self.wave.follow = false;
            }
            WaveMsg::Zoom { factor, anchor } => self.wave.zoom_at(factor, anchor),
            WaveMsg::ToggleFollow => {
                self.wave.follow = !self.wave.follow;
                self.wave.follow_to(now);
            }
            WaveMsg::Fit => {
                let start = self.sim().map_or(0, |s| s.trace.start_time());
                self.wave.fit(start, now.max(start + 10));
                self.wave.follow = false;
            }
            WaveMsg::ExportCsv => {
                return save_file_task("signals.csv", "CSV", &["csv"], |p| Message::WaveExportPicked(false, p));
            }
            WaveMsg::ExportVcd => {
                return save_file_task("signals.vcd", "Value Change Dump", &["vcd"], |p| {
                    Message::WaveExportPicked(true, p)
                });
            }
            WaveMsg::Hide => self.wave.visible = false,
        }
        Task::none()
    }

    /// The component whose documentation F1 shows: the one being placed or selected.
    pub(crate) fn help_kind(&self) -> Option<Kind> {
        match &self.tool {
            Tool::Place { kind, .. } => Some(*kind),
            _ if self.selection.comps.len() == 1 => {
                self.selection.comps.first().and_then(|&id| self.circuit().component(id)).map(|c| c.kind)
            }
            _ => None,
        }
    }

    fn key(&mut self, ev: keyboard::Event) -> Task<Message> {
        // F1: the documentation of the component being placed or selected.
        if let keyboard::Event::KeyPressed { key: Key::Named(Named::F1), .. } = &ev
            && self.modal.is_none()
            && !self.hdl_shown()
        {
            return self.help_kind().map_or(Task::none(), |k| self.update(Message::ShowHelp(k)));
        }
        // A selected keyboard component takes the keys, except shortcuts
        // with Ctrl or Alt.
        if let keyboard::Event::KeyPressed { key, modifiers, text, .. } = &ev
            && !modifiers.command()
            && !modifiers.alt()
            && self.modal.is_none()
            && self.commands.is_none()
            && let Some(id) = self.typing_target()
        {
            let code = match key.as_ref() {
                Key::Named(Named::Escape) => {
                    self.selection.clear();
                    return Task::none();
                }
                Key::Named(Named::Delete) => return self.update(Message::Delete),
                Key::Named(Named::Enter) => Some(b'\n'),
                Key::Named(Named::Backspace) => Some(8),
                Key::Named(Named::Space) => Some(b' '),
                _ => text.as_deref().and_then(|t| match t.as_bytes() {
                    &[c] if (32..127).contains(&c) => Some(c),
                    _ => None,
                }),
            };
            if let Some(c) = code {
                self.type_key(id, c);
            }
            return Task::none();
        }
        // The open command palette handles its own keys (see `subscription`).
        if self.commands.is_some() {
            return Task::none();
        }
        match ev {
            keyboard::Event::KeyPressed { key, modifiers, repeat, .. } => {
                let ctrl = modifiers.command();
                let shift = modifiers.shift();
                let msg = match key.as_ref() {
                    // The circuit is hidden in the HDL view: only the file shortcuts.
                    Key::Character(c) if self.hdl_shown() => match (ctrl, shift, c.to_lowercase().as_str()) {
                        (true, false, "s") => Some(Message::Save),
                        (true, true, "s") => Some(Message::SaveAs),
                        (true, _, "o") => Some(Message::Open),
                        (true, _, "n") => Some(Message::New),
                        (true, true, "p") => Some(Message::OpenCommands),
                        _ => None,
                    },
                    k if self.hdl_shown() && k != Key::Named(Named::Escape) => None,
                    Key::Named(Named::Space) => {
                        if !repeat {
                            self.space_down = true;
                            self.space_panned = false;
                        }
                        None
                    }
                    Key::Named(Named::Escape) => {
                        self.menu = None;
                        if self.modal.is_some() {
                            self.modal = None;
                            None
                        } else if matches!(self.tool, Tool::Place { .. }) {
                            self.tool = Tool::Select;
                            None
                        } else if self.selection.is_empty() && !self.loc.inst.is_empty() {
                            // Nothing to cancel inside an instance: go up one level.
                            Some(Message::Breadcrumb(self.loc.inst.len() - 1))
                        } else {
                            self.selection.clear();
                            None
                        }
                    }
                    Key::Named(Named::Delete) | Key::Named(Named::Backspace) => Some(Message::Delete),
                    Key::Character(c) => {
                        let c = c.to_lowercase();
                        match (ctrl, shift, c.as_str()) {
                            (true, false, "s") => Some(Message::Save),
                            (true, true, "s") => Some(Message::SaveAs),
                            (true, _, "o") => Some(Message::Open),
                            (true, _, "n") => Some(Message::New),
                            (true, true, "p") => Some(Message::OpenCommands),
                            (true, false, "z") => Some(Message::Undo),
                            (true, true, "z") | (true, _, "y") => Some(Message::Redo),
                            (true, _, "c") => Some(Message::Copy),
                            (true, _, "x") => Some(Message::Cut),
                            (true, _, "v") => Some(Message::Paste),
                            (true, _, "d") => Some(Message::Duplicate),
                            (true, _, "a") => Some(Message::SelectAll),
                            (true, _, "0") => Some(Message::FitView),
                            (true, _, "+") | (true, _, "=") => Some(Message::ZoomIn),
                            (true, _, "-") => Some(Message::ZoomOut),
                            (false, _, "r") => Some(Message::Rotate),
                            (false, _, "f") => Some(Message::Mirror),
                            (false, _, "s") => Some(Message::Step),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if let Some(m) = msg {
                    return self.update(m);
                }
            }
            keyboard::Event::KeyReleased { key: Key::Named(Named::Space), .. } if !self.hdl_shown() => {
                let toggle = self.space_down && !self.space_panned;
                self.space_down = false;
                if toggle {
                    return self.update(Message::ToggleRun);
                }
            }
            _ => {}
        }
        Task::none()
    }

    // ---- subscription ------------------------------------------------------------------

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            keyboard::listen().map(Message::Key),
            iced::system::theme_changes().map(Message::SystemTheme),
            window::close_requests().map(|_| Message::CloseRequested),
        ];
        if self.running || self.pending_settle {
            subs.push(window::frames().map(Message::Frame));
        }
        #[cfg(feature = "hdl")]
        subs.push(crate::hdl::subscription(self));
        #[cfg(feature = "mcp")]
        if let Some(server) = &self.mcp {
            subs.push(crate::mcp::subscription(server));
        }
        if self.commands.is_some() {
            // Also the keys the focused search field captures: Escape would
            // only unfocus it.
            subs.push(iced::event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. }) => match key.as_ref() {
                    Key::Named(Named::Escape) => Some(Message::CloseCommands),
                    Key::Named(Named::ArrowUp) => Some(Message::CommandMove(-1)),
                    Key::Named(Named::ArrowDown) => Some(Message::CommandMove(1)),
                    _ => None,
                },
                _ => None,
            }));
        }
        if self.resizing.is_some() {
            subs.push(iced::event::listen_with(|event, _status, _window| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => Some(Message::ResizeMove(position)),
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(_)) => Some(Message::ResizeEnd),
                _ => None,
            }));
        }
        Subscription::batch(subs)
    }

    // ---- view ------------------------------------------------------------------------

    pub fn view(&self) -> Element<'_, Message> {
        let body: Element<Message> = if self.hdl_shown() {
            self.hdl_view()
        } else {
            row![
                container(palette::view(self)).width(self.left_w).height(Fill).style(theme::window),
                splitter(Split::Left, self.is_resizing(Split::Left)),
                self.center(),
                splitter(Split::Right, self.is_resizing(Split::Right)),
                container(props::view(self)).width(self.right_w).height(Fill).style(theme::window),
            ]
            .height(Fill)
            .into()
        };

        let base = column![self.menubar(), self.toolbar(), theme::hline(), body, theme::hline(), self.statusbar()]
            .height(Fill);
        let mut layers: Vec<Element<Message>> = vec![base.into()];
        if let Some(m) = self.menu {
            layers.push(self.menu_overlay(m));
        }
        if let Some(p) = &self.commands {
            layers.push(commands::view(self, p));
        }
        if let Some(modal) = &self.modal {
            layers.push(self.modal_overlay(modal));
        }
        stack(layers).into()
    }

    /// The panel boundary being dragged is `which`.
    pub(crate) fn is_resizing(&self, which: Split) -> bool {
        self.resizing.is_some_and(|r| r.0 == which)
    }

    /// The HDL view (sources, editor, messages) replaces the circuit view.
    pub(crate) fn hdl_shown(&self) -> bool {
        #[cfg(feature = "hdl")]
        return self.settings.hdl && self.hdl.panel;
        #[cfg(not(feature = "hdl"))]
        false
    }

    fn hdl_view(&self) -> Element<'_, Message> {
        #[cfg(feature = "hdl")]
        return crate::hdl::view::workspace(self);
        #[cfg(not(feature = "hdl"))]
        Space::new().into()
    }

    fn center(&self) -> Element<'_, Message> {
        // Where the editor is: the circuit, or the path to the instance shown.
        let crumbs = self.breadcrumbs();
        let last = crumbs.len() - 1;
        let mut trail = row![].spacing(2).align_y(Alignment::Center);
        for (i, (circuit, label)) in crumbs.into_iter().enumerate() {
            if i > 0 {
                trail = trail.push(icons::icon(Icon::ChevronRight, 12.0, theme::colors().muted));
            }
            let font = if i == last { theme::FONT_MEDIUM } else { theme::FONT };
            let mut name =
                row![text(circuit).size(theme::fs(theme::BODY)).font(font)].spacing(5).align_y(Alignment::Center);
            if !label.is_empty() {
                name = name.push(text(label).size(theme::fs(theme::SMALL)).color(theme::colors().muted));
            }
            trail = trail.push(if i == last {
                button(name).padding([3, 8]).style(theme::bare(theme::colors().text, theme::colors().text))
            } else {
                button(name).on_press(Message::Breadcrumb(i)).padding([3, 8]).style(theme::tool)
            });
        }
        // Mirror of “‹ Circuit” in the HDL view.
        #[cfg_attr(not(feature = "hdl"), allow(unused_mut))]
        let mut location_row = row![trail, Space::new().width(Fill)].align_y(Alignment::Center);
        #[cfg(feature = "hdl")]
        if self.settings.hdl {
            location_row = location_row.push(
                button(
                    row![
                        text("HDL").size(theme::fs(theme::BODY)),
                        icons::icon(Icon::ChevronRight, 16.0, theme::colors().text)
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .on_press(Message::Hdl(crate::hdl::Msg::TogglePanel))
                .padding([4, 8])
                .style(theme::tool),
            );
        }
        let location = container(location_row)
            .padding([0, 6])
            .height(theme::fs(32.0))
            .width(Fill)
            .align_y(Alignment::Center)
            .style(theme::window);

        // Messages sit at the top of the view, above the canvas.
        let banner: Option<Element<Message>> = if let Some(err) = &self.error {
            Some(
                theme::message(Emblem::Error, text(format!("Compilation error: {err}")).size(theme::fs(theme::BODY)))
                    .into(),
            )
        } else {
            self.sim().and_then(|s| s.oscillation()).map(|osc| {
                theme::message(
                    Emblem::Warning,
                    row![
                        text(format!("{osc}. The nets involved are highlighted in orange."))
                            .size(theme::fs(theme::BODY))
                            .width(Fill),
                        theme::push_button("Reset").on_press(Message::Reset),
                    ]
                    .align_y(Alignment::Center),
                )
                .padding(Padding { top: 4.0, right: 4.0, bottom: 4.0, left: 10.0 })
                .into()
            })
        };

        let canvas_el: Element<EditorEvent> = canvas(self.editor_program()).width(Fill).height(Fill).into();
        let mut content = column![location, theme::hline()];
        if let Some(banner) = banner {
            content = content.push(container(banner).padding(6).width(Fill).style(theme::view));
        }
        if let Some(banner) = self.hdl_banner() {
            content = content.push(container(banner).padding(6).width(Fill).style(theme::view));
        }
        content = content.push(canvas_el.map(Message::Editor));
        if self.wave.visible {
            content = content
                .push(splitter(Split::Wave, self.resizing.is_some_and(|r| r.0 == Split::Wave)))
                .push(self.waveform_panel());
        }
        content.width(Fill).height(Fill).into()
    }

    /// Above a circuit synthesized from HDL: it is read-only, and whether it
    /// is out of date, with the source it comes from.
    fn hdl_banner(&self) -> Option<Element<'_, Message>> {
        let c = self.circuit();
        let h = c.hdl.as_ref()?;
        let name = c.name.clone();
        let push = |label: &str, msg: Message, default: bool| {
            theme::push_button(label).on_press(msg).style(if default { theme::default_push } else { theme::push })
        };
        // The file and line declaring the entity, when the HDL view is on.
        #[cfg(feature = "hdl")]
        let source = self.settings.hdl.then(|| {
            self.hdl.files.iter().find_map(|f| {
                f.units.iter().find(|u| u.name.eq_ignore_ascii_case(&name)).map(|u| (f.path.clone(), u.line))
            })
        });
        #[cfg(feature = "hdl")]
        let source = source.flatten();
        let (emblem, title, detail) = if h.stale {
            (Emblem::Warning, "Out of date", "The sources changed since the last synthesis.")
        } else {
            (Emblem::Info, "", "Read only: this circuit is generated from HDL source code.")
        };
        let mut line = row![].spacing(8).align_y(Alignment::Center);
        if !title.is_empty() {
            line = line.push(text(title).size(theme::fs(theme::BODY)).font(theme::FONT_MEDIUM));
        }
        line = line.push(text(detail).size(theme::fs(theme::BODY)).width(Fill));
        #[cfg(feature = "hdl")]
        if h.stale && self.settings.hdl && self.hdl.running().is_none() {
            line = line.push(push("Synthesize", Message::Hdl(crate::hdl::Msg::Synthesize), true));
        }
        #[cfg(feature = "hdl")]
        if let Some((path, at)) = source {
            line = line.push(push("Edit source", Message::Hdl(crate::hdl::Msg::OpenAt(path, at, 1)), !h.stale));
        }
        line = line.push(push("Convert to circuit", Message::ConvertCircuit(name), false));
        Some(theme::message(emblem, line).padding(Padding { top: 4.0, right: 4.0, bottom: 4.0, left: 10.0 }).into())
    }

    /// The menus shown in the menu bar.
    /// Which commands apply to the current selection, tool and simulation.
    pub fn enabled(&self) -> Enabled {
        let sel = !self.selection.comps.is_empty();
        Enabled {
            sel,
            delete: !self.selection.is_empty(),
            paste: !self.clipboard.is_empty(),
            transform: sel || matches!(self.tool, Tool::Place { .. }),
            align: self.selection.comps.len() > 1,
            sim: self.sim().is_some(),
        }
    }

    fn menus(&self) -> Vec<(&'static str, Menu)> {
        let mut m =
            vec![("File", Menu::File), ("Edit", Menu::Edit), ("Simulation", Menu::Simulation), ("View", Menu::View)];
        if cfg!(feature = "hdl") && self.settings.hdl {
            m.push(("HDL", Menu::Hdl));
        }
        m.push(("Settings", Menu::Settings));
        m
    }

    fn waveform_panel(&self) -> Element<'_, Message> {
        let trace = self.sim().map(|s| &s.trace);
        let n = trace.map_or(0, |t| t.signals().len());
        let now = self.sim().map_or(0, |s| s.time());
        let wbtn = |content: Element<'static, Message>, msg: WaveMsg| {
            button(content).on_press(Message::Wave(msg)).padding([4, 6]).style(theme::tool)
        };
        let wicon = |i: Icon, label: &str, msg: WaveMsg| theme::tip(wbtn(theme::icon_label(i, "", true), msg), label);
        let header = row![
            text("Waveform").size(theme::fs(theme::BODY)).font(theme::FONT_MEDIUM),
            Space::new().width(6),
            text(format!("t = {now}")).size(theme::fs(theme::SMALL)).color(theme::colors().muted),
            Space::new().width(Fill),
            button(text("Follow").size(theme::fs(theme::BODY)))
                .on_press(Message::Wave(WaveMsg::ToggleFollow))
                .padding([4, 8])
                .style(theme::toggle(self.wave.follow)),
            wicon(Icon::Fit, "Fit to window", WaveMsg::Fit),
            theme::tool_separator(),
            theme::tip(wbtn(theme::icon_label(Icon::Export, "CSV", true), WaveMsg::ExportCsv), "Export as CSV"),
            theme::tip(
                wbtn(theme::icon_label(Icon::Export, "VCD", true), WaveMsg::ExportVcd),
                "Export as VCD (GTKWave)"
            ),
            theme::tool_separator(),
            wicon(Icon::Close, "Close", WaveMsg::Hide),
        ]
        .spacing(2)
        .align_y(Alignment::Center);
        let wave = Waveform { trace, now, view: &self.wave };
        // At least as tall as the panel, so that the time grid reaches its bottom.
        let header_h = theme::fs(34.0);
        let height = waveform::content_height(n).max(self.wave_h - header_h - 1.0);
        let canvas_el: Element<WaveMsg> = canvas(wave).width(Fill).height(height).into();
        column![
            container(header)
                .padding(Padding { top: 3.0, right: 6.0, bottom: 3.0, left: 10.0 })
                .height(header_h)
                .align_y(Alignment::Center)
                .style(theme::window),
            theme::hline(),
            container(theme::scroll(canvas_el.map(Message::Wave)).height(Fill)).height(Fill).style(theme::view),
        ]
        .width(Fill)
        .height(self.wave_h)
        .into()
    }

    fn menubar(&self) -> Element<'_, Message> {
        self.menubar_row(None)
    }

    /// The menu bar. With `dropdown`, the same row is laid out invisibly and
    /// the dropdown is placed under its menu, so it lines up exactly with the
    /// visible bar whatever the width of the labels.
    fn menubar_row<'a>(&'a self, dropdown: Option<(Menu, Element<'a, Message>)>) -> Element<'a, Message> {
        let ghost = dropdown.is_some();
        let mut dropdown = dropdown;
        let bar_h = theme::fs(26.0);
        let mut items = row![].spacing(1).align_y(Alignment::Start);
        for (label, m) in self.menus() {
            let color = if ghost { iced::Color::TRANSPARENT } else { theme::colors().text };
            let b =
                button(text(label).size(theme::fs(theme::BODY)).color(color).height(Fill).align_y(Alignment::Center))
                    .height(bar_h)
                    .padding([0, 9]);
            let b: Element<Message> = if ghost {
                b.style(|_, _| button::Style::default()).into()
            } else {
                b.on_press(Message::Menu(Some(m))).style(theme::item(self.menu == Some(m))).into()
            };
            match dropdown.take_if(|(dm, _)| *dm == m) {
                Some((_, list)) => items = items.push(column![b, Space::new().height(2), list]),
                None => items = items.push(b),
            }
        }
        let bar = container(items).padding([2, 4]).width(Fill);
        if ghost { bar.into() } else { bar.style(theme::header).into() }
    }

    fn menu_overlay(&self, m: Menu) -> Element<'_, Message> {
        // An entry with an icon of the given colour (the text colour when `None`).
        let tinted = |icon: Option<Icon>,
                      tint: Option<iced::Color>,
                      label: &str,
                      key: &str,
                      msg: Message,
                      enabled: bool|
         -> Element<Message> {
            let color = if enabled { tint.unwrap_or(theme::colors().text) } else { theme::colors().disabled };
            let icon: Element<Message> = match icon {
                Some(i) => icons::icon(i, 16.0, color).into(),
                None => Space::new().width(16).into(),
            };
            button(
                row![
                    icon,
                    text(label.to_string()).size(theme::fs(theme::BODY)).width(Fill),
                    text(key.to_string()).size(theme::fs(theme::SMALL)).color(if enabled {
                        theme::colors().muted
                    } else {
                        theme::colors().disabled
                    })
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .on_press_maybe(enabled.then_some(msg))
            .padding([4, 8])
            .width(Fill)
            .style(theme::item(false))
            .into()
        };
        let entry = |icon: Option<Icon>, label: &str, key: &str, msg: Message, enabled: bool| {
            tinted(icon, None, label, key, msg, enabled)
        };
        // Checkable entries show a check box, choices among several a radio
        // button, in the icon column.
        let mark = |on: bool, off: Icon, checked: Icon| {
            if on { (checked, theme::colors().accent) } else { (off, theme::colors().muted) }
        };
        let toggle = |label: &str, on: bool, msg: Message| {
            let (i, c) = mark(on, Icon::CheckOff, Icon::CheckOn);
            tinted(Some(i), Some(c), label, "", msg, true)
        };
        let choice = |label: &str, on: bool, msg: Message| {
            let (i, c) = mark(on, Icon::RadioOff, Icon::RadioOn);
            tinted(Some(i), Some(c), label, "", msg, true)
        };
        let sep = || -> Element<Message> { container(theme::hline()).padding([4, 6]).into() };
        let Enabled { sel, delete, paste, transform, align, sim: has_sim } = self.enabled();
        let items: Vec<Element<Message>> = match m {
            Menu::File => vec![
                entry(Some(Icon::New), "New", "Ctrl+N", Message::New, true),
                entry(Some(Icon::Open), "Open…", "Ctrl+O", Message::Open, true),
                sep(),
                entry(Some(Icon::Save), "Save", "Ctrl+S", Message::Save, true),
                entry(None, "Save as…", "Ctrl+Shift+S", Message::SaveAs, true),
                sep(),
                entry(None, "New circuit", "", Message::NewCircuit, true),
                sep(),
                entry(Some(Icon::Close), "Quit", "", Message::Quit, true),
            ],
            Menu::Edit => vec![
                entry(
                    Some(Icon::Undo),
                    &format!("Undo {}", self.history.undo_label().unwrap_or("")),
                    "Ctrl+Z",
                    Message::Undo,
                    self.history.can_undo(),
                ),
                entry(
                    Some(Icon::Redo),
                    &format!("Redo {}", self.history.redo_label().unwrap_or("")),
                    "Ctrl+Shift+Z",
                    Message::Redo,
                    self.history.can_redo(),
                ),
                sep(),
                entry(None, "Cut", "Ctrl+X", Message::Cut, sel),
                entry(None, "Copy", "Ctrl+C", Message::Copy, sel),
                entry(None, "Paste", "Ctrl+V", Message::Paste, paste),
                entry(None, "Duplicate", "Ctrl+D", Message::Duplicate, sel),
                entry(Some(Icon::Delete), "Delete", "Del", Message::Delete, delete),
                entry(None, "Select all", "Ctrl+A", Message::SelectAll, true),
                sep(),
                entry(Some(Icon::Rotate), "Rotate", "R", Message::Rotate, transform),
                entry(Some(Icon::Flip), "Flip", "F", Message::Mirror, transform),
                sep(),
                entry(None, "Align left", "", Message::Align(Align::Left), align),
                entry(None, "Align top", "", Message::Align(Align::Top), align),
                entry(None, "Center horizontally", "", Message::Align(Align::CenterX), align),
                entry(None, "Center vertically", "", Message::Align(Align::CenterY), align),
            ],
            Menu::Simulation => vec![
                entry(
                    Some(if self.running { Icon::Pause } else { Icon::Run }),
                    if self.running { "Pause" } else { "Run" },
                    "Space",
                    Message::ToggleRun,
                    has_sim,
                ),
                entry(Some(Icon::Reset), "Reset", "", Message::Reset, has_sim),
                sep(),
                entry(Some(Icon::Step), &format!("Single step ({})", self.step_kind), "S", Message::Step, has_sim),
                sep(),
                toggle("Zero-delay mode", self.zero_delay, Message::SetZeroDelay(!self.zero_delay)),
            ],
            #[cfg(feature = "hdl")]
            Menu::Hdl => {
                use crate::hdl::Msg as H;
                use lb_hdl::lang::Lang;
                let idle = self.hdl.running().is_none();
                let sources = !self.project.hdl.sources.is_empty();
                vec![
                    toggle("HDL view", self.hdl.panel, Message::Hdl(H::TogglePanel)),
                    sep(),
                    entry(Some(Icon::Open), "Open files…", "", Message::Hdl(H::AddFiles), true),
                    entry(Some(Icon::New), "New VHDL file…", "", Message::Hdl(H::NewFile(Lang::Vhdl)), true),
                    entry(Some(Icon::New), "New Verilog file…", "", Message::Hdl(H::NewFile(Lang::Verilog)), true),
                    sep(),
                    entry(Some(Icon::Compile), "Compile", "", Message::Hdl(H::Compile), idle && sources),
                    entry(Some(Icon::Chip), "Synthesize", "", Message::Hdl(H::Synthesize), idle && sources),
                    entry(Some(Icon::Stop), "Cancel", "", Message::Hdl(H::Cancel), !idle),
                    sep(),
                    entry(None, "HDL tools…", "", Message::Hdl(H::ShowTools), true),
                ]
            }
            #[cfg(not(feature = "hdl"))]
            Menu::Hdl => Vec::new(),
            Menu::View => vec![
                entry(Some(Icon::Fit), "Zoom to fit", "Ctrl+0", Message::FitView, true),
                entry(None, "Zoom in", "Ctrl++", Message::ZoomIn, true),
                entry(None, "Zoom out", "Ctrl+-", Message::ZoomOut, true),
                sep(),
                toggle("Grid", self.show_grid, Message::ToggleGrid),
                toggle("Waveform", self.wave.visible, Message::ToggleWaveform),
                sep(),
                entry(Some(Icon::Search), "Command palette…", "Ctrl+Shift+P", Message::OpenCommands, true),
            ],
            Menu::Settings => {
                let heading = |label: &str| -> Element<Message> {
                    container(text(label.to_string()).size(theme::fs(theme::SMALL)).color(theme::colors().muted))
                        .padding([4, 8])
                        .into()
                };
                let mut items = vec![heading("Color scheme")];
                for scheme in Scheme::ALL {
                    items.push(choice(scheme.label(), self.settings.scheme == scheme, Message::SetScheme(scheme)));
                }
                items.push(sep());
                items.push(heading("Text size"));
                let size = self.settings.text_size;
                let mut sizes = TEXT_SIZES.to_vec();
                // A size set otherwise (older settings) is listed too.
                if !sizes.contains(&size) {
                    sizes.push(size);
                    sizes.sort();
                }
                for n in sizes {
                    let label = if n == 100 { "100% (default)".to_string() } else { format!("{n}%") };
                    items.push(choice(&label, n == size, Message::SetTextSize(n)));
                }
                if cfg!(feature = "hdl") {
                    let s = &self.settings;
                    items.push(sep());
                    items.push(heading("Features"));
                    items.push(toggle("Enable HDL", s.hdl, Message::SetHdlEnabled(!s.hdl)));
                    if s.hdl {
                        items.push(toggle(
                            "Synthesize when a source is saved",
                            s.hdl_auto,
                            Message::SetHdlAuto(!s.hdl_auto),
                        ));
                        #[cfg(feature = "hdl")]
                        items.push(entry(None, "HDL tools…", "", Message::Hdl(crate::hdl::Msg::ShowTools), true));
                    }
                }
                items
            }
        };
        let list = container(column(items)).padding(4).width(theme::fs(280.0)).style(theme::menu);
        let positioned = container(self.menubar_row(Some((m, opaque(list))))).width(Fill).height(Fill);
        mouse_area(positioned).on_press(Message::Menu(None)).into()
    }

    /// The documentation of [`Modal::Help`], in the colours of the app.
    fn help_view(&self) -> Element<'_, Message> {
        use iced::widget::markdown;
        let c = theme::colors();
        let style = markdown::Style {
            font: theme::FONT,
            inline_code_highlight: markdown::Highlight {
                background: theme::alpha(c.text, 0.07).into(),
                border: iced::border::rounded(theme::ITEM_RADIUS),
            },
            inline_code_padding: Padding { left: 2.0, right: 2.0, ..Padding::ZERO },
            inline_code_color: c.text,
            inline_code_font: iced::Font::MONOSPACE,
            code_block_font: iced::Font::MONOSPACE,
            link_color: c.link,
        };
        markdown::view(&self.help, markdown::Settings::with_text_size(theme::fs(theme::BODY), style))
            .map(|_| Message::Noop)
    }

    fn modal_overlay(&self, modal: &Modal) -> Element<'_, Message> {
        let dialog_button = |label: &str, msg: Message, default: bool| {
            button(text(label.to_string()).size(theme::fs(theme::BODY)).width(Fill).align_x(Alignment::Center))
                .on_press(msg)
                .width(theme::fs(96.0))
                .padding(theme::PUSH_PADDING)
                .style(if default { theme::default_push } else { theme::push })
        };
        // KMessageBox layout: emblem on the left of the title and the text.
        let body = |title: &str, msg: String| -> Element<Message> {
            row![
                icons::emblem(Emblem::Warning, 32.0),
                column![
                    text(title.to_string()).size(theme::fs(theme::TITLE)).font(theme::FONT_MEDIUM),
                    text(msg).size(theme::fs(theme::BODY))
                ]
                .spacing(6),
            ]
            .spacing(14)
            .into()
        };
        let (content, buttons): (Element<Message>, Vec<Element<Message>>) = match modal {
            Modal::Unsaved(after) => {
                let what = match after {
                    After::New => "creating a new project",
                    After::Open => "opening another project",
                    After::Quit => "quitting",
                };
                (
                    body("Unsaved changes", format!("The project has unsaved changes. Save them before {what}?")),
                    vec![
                        dialog_button("Save", Message::ModalSave, true).into(),
                        dialog_button("Don’t save", Message::ModalDiscard, false).into(),
                        dialog_button("Cancel", Message::ModalCancel, false).into(),
                    ],
                )
            }
            Modal::Message(msg) => {
                (body("Warning", msg.clone()), vec![dialog_button("OK", Message::ModalCancel, true).into()])
            }
            Modal::Help(kind) => (
                column![
                    text(kind.name()).size(theme::fs(theme::TITLE)).font(theme::FONT_MEDIUM),
                    container(theme::scroll(
                        container(self.help_view()).padding(Padding { right: 12.0, ..Padding::ZERO })
                    ))
                    .max_height(theme::fs(460.0)),
                ]
                .spacing(12)
                .into(),
                vec![dialog_button("Close", Message::ModalCancel, true).into()],
            ),
            #[cfg(feature = "hdl")]
            Modal::HdlTools => {
                (crate::hdl::view::tools(self), vec![dialog_button("Close", Message::ModalCancel, true).into()])
            }
        };
        // The content, then the buttons on the right.
        let card =
            column![content, row![Space::new().width(Fill)].extend(buttons).spacing(6)].spacing(20).padding([18, 20]);
        let width = if matches!(modal, Modal::Help(_)) { 600.0 } else { 460.0 };
        let dim = container(iced::widget::center(opaque(
            container(card).width(theme::fs(width)).clip(true).style(theme::dialog),
        )))
        .width(Fill)
        .height(Fill)
        .style(theme::backdrop);
        opaque(mouse_area(dim).on_press(Message::Noop))
    }

    fn toolbar(&self) -> Element<'_, Message> {
        // Icon-only actions, with the shortcut in the tooltip.
        let tb = |i: Icon, tip: &str, msg: Message, enabled: bool| -> Element<Message> {
            theme::tip(
                button(theme::icon_label(i, "", enabled))
                    .on_press_maybe(enabled.then_some(msg))
                    .padding(6)
                    .style(theme::tool),
                tip,
            )
        };
        // Main actions: icon and text.
        let tbl = |i: Icon, label: &str, msg: Message, enabled: bool| {
            button(theme::icon_label(i, label, enabled)).on_press_maybe(enabled.then_some(msg)).padding([5, 8])
        };
        // The HDL view has no circuit to edit or simulate.
        if self.hdl_shown() {
            return container(row(self.hdl_toolbar()).spacing(3).height(theme::fs(32.0)).align_y(Alignment::Center))
                .padding([2, 6])
                .width(Fill)
                .style(theme::header)
                .into();
        }
        let has_sim = self.sim().is_some();
        let transform = !self.selection.comps.is_empty() || matches!(self.tool, Tool::Place { .. });
        let run = if self.running {
            tbl(Icon::Pause, "Pause", Message::ToggleRun, has_sim)
        } else {
            tbl(Icon::Run, "Run", Message::ToggleRun, has_sim)
        }
        .style(theme::toggle(self.running));
        let speeds: Vec<SpeedChoice> =
            Speed::ALL.iter().map(|&s| SpeedChoice::Preset(s)).chain([SpeedChoice::Custom]).collect();
        let custom: Element<Message> = match &self.custom_speed {
            Some(t) => row![
                text_input("Ticks", t)
                    .id(CUSTOM_SPEED_ID)
                    .on_input(Message::CustomSpeed)
                    .on_submit(Message::CustomSpeedDone)
                    .width(theme::fs(90.0))
                    .size(theme::fs(theme::BODY))
                    .padding([4, 8])
                    .style(theme::input),
                text("tick/s").size(theme::fs(theme::BODY)).color(theme::colors().muted),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .into(),
            None => Space::new().into(),
        };
        container(
            row![
                tb(Icon::New, "New (Ctrl+N)", Message::New, true),
                tb(Icon::Open, "Open (Ctrl+O)", Message::Open, true),
                tb(Icon::Save, "Save (Ctrl+S)", Message::Save, true),
                theme::tool_separator(),
                tb(Icon::Undo, "Undo (Ctrl+Z)", Message::Undo, self.history.can_undo()),
                tb(Icon::Redo, "Redo (Ctrl+Shift+Z)", Message::Redo, self.history.can_redo()),
                theme::tool_separator(),
                tb(Icon::Rotate, "Rotate (R)", Message::Rotate, transform),
                tb(Icon::Flip, "Flip (F)", Message::Mirror, transform),
                tb(Icon::Delete, "Delete (Del)", Message::Delete, !self.selection.is_empty()),
                theme::tool_separator(),
                run,
                tbl(Icon::Reset, "Reset", Message::Reset, has_sim).style(theme::tool),
                tbl(Icon::Step, "Step", Message::Step, has_sim).style(theme::tool),
                pick_list(&StepKind::ALL[..], Some(self.step_kind), Message::SetStepKind)
                    .text_size(theme::fs(theme::BODY))
                    .padding([4, 10])
                    .style(theme::dropdown)
                    .menu_style(theme::dropdown_menu)
                    .handle(theme::dropdown_handle()),
                Space::new().width(6),
                text("Speed:").size(theme::fs(theme::BODY)),
                pick_list(speeds, Some(SpeedChoice::Preset(self.speed)), |c| match c {
                    SpeedChoice::Preset(s) => Message::SetSpeed(s),
                    SpeedChoice::Custom => Message::EditCustomSpeed,
                })
                .text_size(theme::fs(theme::BODY))
                .padding([4, 10])
                .style(theme::dropdown)
                .menu_style(theme::dropdown_menu)
                .handle(theme::dropdown_handle()),
            ]
            .push(custom)
            .push(theme::tool_separator())
            .push(tb(Icon::Fit, "Zoom to fit (Ctrl+0)", Message::FitView, true))
            .push(tbl(Icon::Wave, "Waveform", Message::ToggleWaveform, true).style(theme::toggle(self.wave.visible)))
            .spacing(3)
            .height(theme::fs(32.0))
            .align_y(Alignment::Center),
        )
        .padding([2, 6])
        .width(Fill)
        .style(theme::header)
        .into()
    }

    fn hdl_toolbar(&self) -> Vec<Element<'_, Message>> {
        #[cfg(feature = "hdl")]
        return crate::hdl::view::toolbar(self);
        #[cfg(not(feature = "hdl"))]
        Vec::new()
    }

    /// "Agent" in the status bar while an agent is connected.
    fn agent_info(&self) -> &'static str {
        #[cfg(feature = "mcp")]
        if self.mcp.as_ref().is_some_and(|s| s.connected) {
            return "Agent";
        }
        ""
    }

    fn statusbar(&self) -> Element<'_, Message> {
        let sim_info = match self.sim() {
            Some(s) => {
                let mode = if self.zero_delay {
                    "Zero-delay".to_string()
                } else {
                    format!("Gate delay {}", self.sim_cfg.gate_delay)
                };
                let state = if self.running { format!("Running, {:.0} fps", self.fps) } else { "Paused".into() };
                format!(
                    "t = {} · {state} · {mode} · {} components, {} nets · {:.2} M evaluations/s",
                    s.time(),
                    s.netlist().comps.len(),
                    s.netlist().nets.len(),
                    self.evals_per_sec / 1e6
                )
            }
            None => "Simulation unavailable".into(),
        };
        let zoom = format!("Zoom {:.0}%", self.loc().view.scale / DEFAULT_SCALE * 100.0);
        container(
            row![
                Space::new().width(Fill),
                text(sim_info).size(theme::fs(theme::SMALL)).color(theme::colors().muted),
                theme::vline(),
                text(zoom).size(theme::fs(theme::SMALL)).color(theme::colors().muted),
                text(self.agent_info()).size(theme::fs(theme::SMALL)).color(theme::colors().link),
            ]
            .spacing(12)
            .height(theme::fs(14.0))
            .align_y(Alignment::Center),
        )
        .padding([4, 10])
        .width(Fill)
        .style(theme::window)
        .into()
    }
}

/// Width of the grab area of a splitter, beside its hairline.
const GRIP: f32 = 4.0;

/// Resizable boundaries between panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    Left,
    Right,
    Wave,
    /// Between the sources and the editor of the HDL view.
    #[cfg(feature = "hdl")]
    HdlSources,
    /// Between the editor and the messages of the HDL view.
    #[cfg(feature = "hdl")]
    HdlLog,
}

/// A drag handle between two panels: a hairline between the view and a
/// strip of the panel colour, so that it reads as a plain separator.
pub(crate) fn splitter<'a>(which: Split, active: bool) -> Element<'a, Message> {
    let color = if active { theme::colors().accent } else { theme::colors().separator };
    let line = move |_: &iced::Theme| container::Style {
        background: Some(iced::Background::Color(color)),
        snap: true,
        ..Default::default()
    };
    let left_panel = || -> Element<Message> {
        row![
            container(Space::new()).width(GRIP).height(Fill).style(theme::window),
            container(Space::new()).width(1).height(Fill).style(line),
        ]
        .into()
    };
    let right_panel = || -> Element<Message> {
        row![
            container(Space::new()).width(1).height(Fill).style(line),
            container(Space::new()).width(GRIP).height(Fill).style(theme::window),
        ]
        .into()
    };
    let area: Element<Message> = match which {
        Split::Left => left_panel(),
        Split::Right => right_panel(),
        #[cfg(feature = "hdl")]
        Split::HdlSources => left_panel(),
        #[cfg(feature = "hdl")]
        Split::HdlLog => right_panel(),
        // Panels under the canvas.
        Split::Wave => column![
            container(Space::new()).width(Fill).height(1).style(line),
            container(Space::new()).width(Fill).height(GRIP).style(theme::window),
        ]
        .into(),
    };
    mouse_area(area)
        .on_press(Message::StartResize(which))
        .interaction(if which == Split::Wave {
            iced::mouse::Interaction::ResizingVertically
        } else {
            iced::mouse::Interaction::ResizingHorizontally
        })
        .into()
}

/// Canvas program built in `view`: it owns the data computed for the frame
/// and lends it, with the application state, to an [`Editor`].
pub(crate) struct EditorProgram<'a> {
    oscillating: HashSet<NetId>,
    app: &'a App,
}

impl EditorProgram<'_> {
    fn editor(&self) -> Editor<'_> {
        let app = self.app;
        let loc = app.loc();
        Editor {
            project: &app.project,
            circuit: app.circuit(),
            cache: &app.cache,
            sim: app.sim(),
            inst: &loc.inst,
            selection: &app.selection,
            tool: &app.tool,
            view: loc.view,
            size: app.canvas_size,
            space_down: app.space_down,
            oscillating: &self.oscillating,
            show_grid: app.show_grid,
            version: app.render_version,
        }
    }
}

impl canvas::Program<EditorEvent> for EditorProgram<'_> {
    type State = crate::canvas::EditorState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &canvas::Event,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> Option<canvas::Action<EditorEvent>> {
        self.editor().update(state, event, bounds, cursor)
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &iced::Renderer,
        theme: &iced::Theme,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        self.editor().draw(state, renderer, theme, bounds, cursor)
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        self.editor().mouse_interaction(state, bounds, cursor)
    }
}

fn pick_file_task() -> Task<Message> {
    Task::perform(
        async {
            rfd::AsyncFileDialog::new()
                .add_filter("LogiBuild circuit", &["lbc"])
                .pick_file()
                .await
                .map(|h| h.path().to_path_buf())
        },
        Message::OpenPicked,
    )
}

fn save_file_task(
    name: &str,
    filter: &str,
    ext: &[&str],
    msg: impl Fn(Option<PathBuf>) -> Message + Send + 'static,
) -> Task<Message> {
    let name = name.to_string();
    let filter = filter.to_string();
    let ext: Vec<String> = ext.iter().map(|s| s.to_string()).collect();
    Task::perform(
        async move {
            rfd::AsyncFileDialog::new()
                .set_file_name(name)
                .add_filter(filter, &ext)
                .save_file()
                .await
                .map(|h| h.path().to_path_buf())
        },
        msg,
    )
}
