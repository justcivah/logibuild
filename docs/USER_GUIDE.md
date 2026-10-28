# LogiBuild user guide

LogiBuild is an editor and simulator for digital logic circuits. This guide covers the
whole user interface; see [COMPONENTS.md](COMPONENTS.md) for every component and its pins,
and [FILE_FORMAT.md](FILE_FORMAT.md) for the `.lbc` files.

## Starting

```bash
logibuild                         # empty project
logibuild examples/game_of_life.lbc     # open a file
logibuild --run examples/stopwatch.lbc  # open and start simulating
```

The window has five areas:

| Area | Contents |
| --- | --- |
| Top | menu bar (File, Edit, Simulation, View, Settings) and toolbar |
| Left | component library with search, and the circuits of the project |
| Centre | the editor canvas, with the shown circuit (or instance path) in the bar above it, and the waveform panel |
| Right | properties of the selection, or of the circuit when nothing is selected |
| Bottom | status line: simulation time and state, frame rate, zoom |

### Command palette

**Ctrl+Shift+P** (or **View → Command palette…**) opens a search field over the window. Type
part of a name to find any menu command, any component to place ("place and") or any circuit
to open or place as a subcircuit. Matches whose name starts with the text come first. **↑**
and **↓** move the highlight, **Enter** runs it, **Esc** or a click outside closes the
palette. Commands that do not apply now are greyed out, with their shortcut on the right. The
palette also opens the documentation of the component being placed or selected (like **F1**).

## Building circuits

### Placing components

Click a component in the library, then click on the canvas: a translucent preview follows
the mouse. The tool stays active so that you can place several copies; press **Esc** or
right-click to go back to selection. **R** rotates and **F** flips the preview (or the
selected components).

Type in the search box to filter the library by name, file name (`Xor`) or category.

### Wiring

Drag from a pin to another pin. Pins are the small circles on the edge of the components;
the cursor becomes a crosshair over them and the target pin is highlighted while you drag.
Releasing on an existing wire joins that wire's net. A wire always connects two pins; the
editor orients it from the driving output.

Wires are **routed automatically**: they leave each pin straight, avoid component bodies,
never run along or turn on a wire of another net (so that unrelated nets never look
connected) and prefer few bends. Wires of the same net share segments, and a junction dot
appears wherever three or more of them meet. Routing is incremental: editing one part of a
large circuit only reroutes the wires that are affected.

To shape a wire by hand, select it and:

* **Alt+click** on the wire to add a bend point (waypoint);
* drag the square handles to move waypoints, double-click a handle to remove it;
* use **Freeze route** in the properties panel to turn the automatic route into
  waypoints, or **Automatic route** to remove all of them.

Waypoints are kept only where the wire bends: after freezing, dragging or removing a
waypoint, and after moving, rotating or aligning the components a wire is attached to, a
waypoint left on a straight run is removed, and so is one where the wire would turn back
on itself (a useless spur). When no bend is left the wire is routed automatically again.
A waypoint just added with Alt+click stays until you drag it.

### Selecting and editing

* Click a component or a wire to select it; **Shift+click** or **Ctrl+click** adds or removes
  it from the selection; drag on an empty area for a rectangular selection; **Ctrl+A**
  selects everything.
* Drag a selected component to move the whole selection (wires follow).
* **Delete** removes the selection, together with the wires attached to removed components.
* **Ctrl+C / Ctrl+X / Ctrl+V / Ctrl+D**: copy, cut, paste, duplicate (wires between copied
  components are copied too).
* With several components selected, the properties panel offers alignment (left, centre,
  right, top, middle, bottom).
* **Ctrl+Z / Ctrl+Shift+Z** (or **Ctrl+Y**): unlimited undo and redo. Every operation is a
  single step, however many elements it touches; typing in a property field is merged into
  one step.

### Properties

The right panel shows the properties of the selected component (label, width, number of
inputs, clock period, edge, memory contents, …). Changes apply immediately and can be
undone. Each component has an optional **Delay** that overrides the default gate delay.
The **?** next to the name of the component shows its documentation: what it does, its pins
and its properties, from [COMPONENTS.md](COMPONENTS.md).

With nothing selected, the panel shows the circuit: its name (editable), its terminals
when used as a subcircuit, compilation warnings (pins of different widths on one net,
several active drivers, wires to missing pins, lone tunnels) and the simulation settings.

### Annotations

**Text** shows its label on the canvas; **Box** draws a dashed frame (size in the
properties) to group parts of a circuit. A **Probe** shows the value of the net it is
attached to and gives it a name.

### Tunnels

A **Tunnel** (Inputs & outputs) connects a pin to every other tunnel with the same label
in the same circuit, without drawing a wire: place one where a signal starts and one
wherever it is needed (`clk`, `rst`, a data bus…). Its width must match the pins it joins.
The label also names the net in the waveform and in warnings.

* Hover over a tunnel, or select it, to outline the tunnels with the same label and link
  them to it with dashed lines.
* The properties panel says how many tunnels it is connected to and lists the labels already
  used in the circuit: click one to use it.
* Renaming a tunnel moves only that tunnel to the other net; copies keep their label, so
  they connect to the same net.
* Tunnels stay inside their circuit: each instance of a subcircuit has its own, and a
  subcircuit exchanges signals only through its Input and Output pins.
* A tunnel without a label, or the only one with its label, is listed under the circuit's
  warnings.

## Simulating

The simulation is always active: every change you make is compiled and propagated
immediately, keeping the state of flip-flops, memories and inputs that still exist.

* **Click a 1-bit input** on the canvas to toggle it. Multi-bit inputs are set in the
  properties panel (**Current value**, in decimal, `0x…` hex or `0b…` binary).
  **Buttons** are momentary: they read 1 while the mouse is pressed.
* **Select a keyboard** and type: while it is the only selected component, the keys go to
  its buffer instead of triggering the shortcuts (Enter sends a line feed, Backspace a
  backspace; Ctrl shortcuts still work, **Del** deletes the keyboard); **Esc** stops typing.
  **Clear buffer** in the properties panel discards the characters waiting. The circuit
  reads them from `data` and `avail` (see `examples/rot13_terminal.lbc`).
* **Run / Pause** (toolbar, Simulation menu or **Space**) advances time at the chosen
  **Speed** (ticks per second, or *Max*; **Custom…** lets you type any rate up to 100 million
  ticks per second). Clocks only tick while running.
* **Step** (**S**) advances by the amount chosen next to it: one event, one delta cycle,
  one tick, or one clock edge (it then lets the circuit settle until just before the next
  edge).
* **Reset** restarts from time 0 with the initial values.

Signal values colour the wires: dark green 0, bright green 1, red `X` (unknown or
conflict), grey `Z` (floating), dark blue for buses, orange for nets whose pins have
different widths.

### Timing model

By default each component has a delay of 1 tick, so you can see glitches and races in the
waveform. **Zero-delay** mode (Simulation menu or the circuit panel) keeps only delta
cycles: every change settles within the same instant. Use long enough clock periods for
deep logic in the default mode (the default clock period is 100 ticks).

If a net keeps changing within one instant for more than the **delta limit** (1000 by
default), the simulation stops with an oscillation warning and the nets involved are
highlighted in orange; press **Reset** after fixing the circuit. (With non-zero delays a
NOT gate fed back on itself is a ring oscillator, which is a legitimate clock.)

## Waveform panel

Add signals to the timing diagram in any of these ways:

* double-click a wire, a probe, an output or a LED;
* drag a wire and drop it on the waveform panel below the canvas;
* use **To waveform** / **Add to waveform** in the properties panel.

In the panel:

| Action | Effect |
| --- | --- |
| Drag, Shift+wheel | scroll in time |
| Ctrl+wheel | zoom around the mouse |
| Fit | show the whole recorded history |
| Follow | keep the current time in view while running |
| Drag the line after the names | widen or narrow the column of names |
| Click a name | select the signal |
| Click the radix chip | cycle bin / hex / dec / signed |
| × next to a name | remove the signal |
| CSV, VCD | export (VCD opens in GTKWave: `sudo apt install gtkwave`) |

The history of each signal is a ring buffer (100 000 changes by default, configurable as
**Trace samples**). Signals stay in the diagram when you edit the circuit.

## Subcircuits

Any circuit of the project can be used inside another one:

1. **+ New** next to *Project circuits* (or File → New circuit) creates and opens an empty circuit.
2. Build it with **Input** and **Output** pins: they become the terminals of the
   subcircuit, ordered top to bottom (inputs on the left, outputs on the right), named after
   their labels.
3. In another circuit, click the subcircuit in **Project circuits** and place it.

Editing the subcircuit updates every instance. A circuit cannot contain itself, directly or
indirectly, and the main circuit cannot be used as a subcircuit: such placements are
refused (hover a greyed circuit to see why).

One circuit is shown at a time; it is highlighted in **Project circuits**. **Open** next to
a circuit edits it on its own (it then becomes the simulation root while it is shown). Each
circuit remembers its zoom and position. **Double-click** an instance to look inside it with
the live values of that instance: the top left of the canvas shows the path from the root
circuit (`Main › full_adder adder0`); click a level to go back to it, or press **Esc** to go
up one level.

The circuit marked ★ is the main one (saved first in the file); **Make main**, rename and
**Delete circuit** are in the circuit panel. A circuit used as a subcircuit cannot be made
the main one.

## HDL: VHDL and Verilog

Entities written in VHDL, or modules in Verilog, can become circuits of the project. The
sources are compiled by two external tools: **GHDL** (VHDL) and **Yosys** (synthesis, and
Verilog). The simplest way to get both, with the GHDL plugin for Yosys, is the
[OSS CAD Suite](https://github.com/YosysHQ/oss-cad-suite-build/releases) (Linux, macOS,
Windows).

HDL support is off by default: turn it on with **Settings → Features → Enable HDL**.
This adds the **HDL** menu and the **HDL view**: **HDL ›**, at the top right of the canvas
(or **HDL → HDL view**), shows it in place of the canvas, the side panels and the waveform,
with **Compile** and **Synthesize** at its top; **‹ Circuit** goes back. Turning it off again
only hides them: circuits synthesized from HDL stay in the project and still simulate.

### Sources

The left column of the HDL view lists the source files of the project with the entities
or modules each declares. **Open files** (in the toolbar) adds existing files, **New VHDL** and **New
Verilog** create one from a template; **Remove**, above the editor, removes
the file shown (the file itself is kept). Sources are stored relative to the project file. The order of the files
does not matter: GHDL works out the order of analysis. The VHDL revision (VHDL-93 or
VHDL-2008) is chosen in the toolbar; the Synopsys packages (`std_logic_unsigned`…) are
available.

**Settings → HDL tools…** (also **Tools…** in the HDL view) says where GHDL and Yosys were
found (on the `PATH`, or in `~/oss-cad-suite/bin` where the suite is usually unpacked), or
lets you type the path of each program or of the folder holding it (such as
`~/oss-cad-suite/bin`). When only one of the two is found, the other is looked for in the same
folder.

The three columns of the HDL view (sources, editor, messages) are resized by dragging the lines
between them.

### Editor

Click a file (or an entity, to go to its declaration) to open it in the editor, with keywords
highlighted. Each open file has a tab with its name. **Ctrl+S** in the editor saves the file
(saving the project saves the edited sources too), **Open externally** opens it in your usual
editor. Files edited elsewhere are reloaded automatically; if you were editing it here too, a
message lets you reload it or keep your version.

### Compile and synthesize

* **Compile** checks every source: GHDL analyzes and elaborates the VHDL, Yosys reads the
  Verilog. It needs no entity to be added to the project.
* **Add** next to an entity synthesizes it into a circuit of the same name, together with the
  entities it instantiates (each one becomes a subcircuit). Place it from **Project
  circuits**; **View** next to the entity shows the synthesized circuit.
* **Synthesize** synthesizes again every entity added to the project; when none was added
  yet, it adds and synthesizes all the entities and modules of the sources. With **Synthesize when
  a source is saved** (Settings), saving a source does it automatically.

Messages of the tools appear on the right of the HDL view; click a message with a location to
open the file at that line. **Cancel** stops a long run.

Synthesized circuits keep the structure of the source: operators stay whole (an addition is
one adder, a comparison one comparator, a `case` a multiplexer), registers are named after
their signal, constant tables become ROMs and simple single-port memories RAMs. Signals read
in many places (clocks, resets) are drawn with tunnels. Multiplication and division have no
component and become gates; memories with several ports become registers and multiplexers.

### Synthesized circuits

A synthesized circuit is marked **HDL** in Project circuits and on its instances. It is
**read-only**: change it by editing its sources. When the sources change, it is out of
date until it is synthesized again: its **HDL** mark turns orange, and the HDL view says
**old** instead of **built**. If the synthesis fails, the last working version stays in
place, still out of date. A synthesis replaces the circuits in one
step, which **Undo** reverts.

**Convert to circuit** (in the banner over the circuit or in the circuit panel) makes it an
ordinary circuit that you can edit by hand; the sources no longer update it.

## Memories

ROM and RAM contents are typed as hexadecimal words separated by spaces (`N*value` repeats
a value). **Import .hex…** loads a file, **Export .hex…** saves the contents — the live
contents of a RAM while simulating. Set **External .hex file** to keep the contents in a
separate file next to the project instead of inline. Images are compatible with Logisim's
`v2.0 raw` format.

## Files

Projects are `.lbc` text files designed for version control (stable order, one component or
wire per line). Saving is atomic and keeps the previous version as `<name>.lbc.bak`. A file
can embed its subcircuits, or reference folders of `.lbc` files with the `libraries` field.

## Appearance

The **Settings** menu chooses the colour scheme — **Follow system** (the default: light or
dark as the desktop is, updated when it changes), **Light** or **Dark** — and the
text size, from 90% to 175% (100% is the default). The choices are kept in
`$XDG_CONFIG_HOME/logibuild/settings` (usually `~/.config/logibuild/settings`).

## Agents (MCP)

A build with the optional `mcp` feature lets an AI agent, such as Claude Code, drive an open
window through the [Model Context Protocol](https://modelcontextprotocol.io): you ask the agent
for a circuit and watch it being placed, wired and simulated.

```bash
cargo build --release -p logibuild --features mcp
logibuild --mcp examples/stopwatch.lbc     # this window accepts agents
claude mcp add logibuild -- /path/to/logibuild --mcp-bridge
```

`--mcp` makes the window listen on a local socket: `$XDG_RUNTIME_DIR/logibuild.sock` (or the
temporary folder) on Linux and macOS, readable by your user only, and the named pipe
`\\.\pipe\logibuild` on Windows. Nothing listens without the option. `--mcp-bridge` is the MCP
server the agent starts: it relays the agent's messages to that window, and opens one when
none is listening. `--mcp=NAME` and `--mcp-bridge=NAME` choose another socket name, to drive
several windows. The status line shows **Agent** while an agent is connected.

The agent can read the project and the shown circuit, list the component kinds with their
pins, place, move, change and remove components, connect pins, create, open and rename
circuits, undo, redo and save; set inputs, run, pause, reset, step or advance the simulation
and read the values on pins; find and run any command palette entry; and take a screenshot of
the window. Its edits go through the undo history like yours. It cannot open file dialogs or
quit, and it waits while a dialog is open.

## Keyboard shortcuts

| Action | Keys |
| --- | --- |
| Command palette | Ctrl+Shift+P |
| New / Open / Save / Save as | Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S |
| Undo / Redo | Ctrl+Z / Ctrl+Shift+Z or Ctrl+Y |
| Copy / Cut / Paste / Duplicate | Ctrl+C / Ctrl+X / Ctrl+V / Ctrl+D |
| Select all | Ctrl+A |
| Delete | Del or Backspace |
| Rotate / Flip | R / F |
| Run or pause | Space |
| Single step | S |
| Save the source being edited (HDL editor) | Ctrl+S |
| Cancel tool, clear selection, leave an instance | Esc |
| Zoom to fit / in / out | Ctrl+0 / Ctrl++ / Ctrl+- |
| Pan | touchpad two-finger scroll, middle or right drag, Space+drag, Shift+wheel |
| Zoom | mouse wheel, Ctrl+touchpad scroll |
| Add a bend to a wire | Alt+click |
