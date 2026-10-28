# Architecture

LogiBuild is a Cargo workspace with three crates:

```text
core/  lb_core     library, no GUI dependency: everything that is not drawing
hdl/   lb_hdl      VHDL/Verilog: GHDL and Yosys, import of synthesized netlists
app/   logibuild  the iced 0.14 application
```

`app` depends on the libraries, never the other way round, and `lb_hdl` on `lb_core`. The core
can be used headless (tests, benchmarks, scripts); see the example in `core/src/lib.rs`. HDL
support is optional: `cargo build --no-default-features` leaves out `lb_hdl` and the HDL code of
the application.

## `lb_core`

| Module | Responsibility |
| --- | --- |
| `value` | `Signal`: 1–64 bit four-valued values (`0 1 X Z`) as three bit masks (`val`, `unk`, `hiz`), so bus operations are O(1); gate logic, wire resolution, formatting and parsing. |
| `component` | `Kind`, `Props`, names and categories; `layout` (pin geometry, transforms); `eval` (`Prim::eval`, the behaviour of every primitive). |
| `circuit` | `Circuit`, `Component`, `Wire`, `Project`; hierarchy checks (missing circuits, recursion); world geometry of pins. |
| `netlist` | Compilation of a project into a flat netlist. |
| `sim` | The event-driven simulator. |
| `trace` | Recording of signal changes; CSV and VCD export. |
| `io` | `.lbc` reading/writing, libraries, `.hex` images, atomic save. |
| `history` | Invertible editing operations, commands, undo/redo, clipboard. |

### Compilation

`netlist::compile(project, root)` checks the hierarchy, then expands the root circuit
recursively. Every pin becomes a node of a union-find; wires merge nodes, and so do tunnels
with the same label in the same instance (they have no primitive, only their pin node). A subcircuit
instance allocates one node per terminal, and inside the instance its `Input`/`Output`
components are not instantiated: their pins *are* the terminal nodes. The resulting sets
are numbered as nets. Primitive components get contiguous ranges of input and output slots.
Each net records its drivers and readers, its width (a width mismatch is a warning and the
net reads `X`) and a hierarchical name (`alu/adder0/carry`), taken from the shallowest
label of a pin, probe or LED on it.

The netlist also keeps, for every instance path, the map from `(component id, pin index)`
to net, which the editor uses to colour wires and to show live values inside instances.
Full compilation of 10 000 components takes about 19 ms.

### Simulation

The simulator keeps the value driven by every output, the projected (last scheduled) value
of every output, the resolved value of every net and the state of every component.

* Events are output changes in a binary heap ordered by `(time, delta, sequence)`. The
  sequence number makes runs fully deterministic.
* A batch is all events with the same `(time, delta)`: their outputs are applied, the
  touched nets are resolved (Z yields to anything, different values give X), and every
  component reading a net that changed is evaluated once, in index order.
* A changed output is scheduled `delay` ticks later (component override or global gate
  delay, 1 by default), or at `delta + 1` of the same instant with zero delay. Sources
  (inputs, clocks) have no delay.
* Clocks are not queued: the simulator knows their next toggle and fires them before any
  later event.
* Oscillation: delta cycles are counted from the last external stimulus; past the limit
  (1000) the simulator stops and reports the nets that changed in the last cycles.
* Initialisation sets every output to 0 and runs a short sequential relaxation
  (components evaluated one at a time with immediate updates) before the event loop. This
  breaks the symmetry of cross-coupled latches, which would otherwise oscillate forever,
  while a true oscillator still never converges and is caught by the event loop.
* After an edit the circuit is recompiled and the new simulator inherits time, trace and
  the state and known output values of every component that still exists, so editing does
  not glitch the running circuit.

Throughput is about 9 million gate evaluations per second on one core
(`cargo bench -p lb_core`).

### Undo/redo

A `Command` is a list of `Op`s (`AddComp`, `RemoveComp`, `UpdateComp{before, after}`,
`AddWire`, `RemoveWire`, circuit add/remove/rename), each with an exact inverse: a
differential snapshot of only what changed. Commands with the same `merge` key are
coalesced (typing in a field), the history is unbounded and tracks the saved state.

## `lb_hdl`

| Module | Responsibility |
| --- | --- |
| `toolchain` | Finding GHDL and Yosys, the Yosys script, running the tools in a work folder (cancellable). |
| `json` | The netlist written by Yosys' `write_json`. |
| `import` | Yosys modules to LogiBuild circuits. |
| `place` | Automatic layout of imported circuits. |
| `lang` | File types, the units a source declares, tokens for highlighting. |
| `log` | Messages of the tools, with their source location. |

**Compile** runs `ghdl -i` on the VHDL files and `ghdl -m` on the tops (GHDL finds the order
of analysis), and Yosys `read_verilog` on the Verilog files. **Synthesize** then runs Yosys
once per top: `ghdl` (plugin) or `read_verilog`, `hierarchy`, `proc`, `opt`, `memory -nomap`,
`pmuxtree`, then `techmap` only on the operators without a component (`$mul`, `$div`…), and
`write_json`. The hierarchy is not flattened. `opt` runs with `-mux_undef`, which removes the
`we ? addr : x` multiplexers `proc` puts in front of memory write ports, so that a RAM whose
reads and writes use the same address stays one RAM. GHDL names an instantiated entity
`<entity>_B<architecture>`; the circuits are renamed after the entity.

The importer converts each module into a circuit, children first. Each cell becomes one or a
few components (`$add` an adder, `$dff`/`$adff` a register, with XOR gates for a non-zero
reset value, `$mem_v2` a ROM or a RAM, an instance a subcircuit…) reading and driving vectors
of bits. Once every cell is converted, each vector read is traced back to the output pins
driving its bits: one whole pin is a wire, a slice goes through one splitter per pin (cut
where all its slices start and end), several pieces or constants through a merger (shared
between identical reads). Nets with six readers or more are drawn with tunnels. A memory that
fits no RAM component (several ports, separate addresses) makes the importer ask for another
run with `memory` mapping it to registers.

Layout: columns by longest path from the inputs (feedback edges, found by a depth-first
search, are ignored), barycentre ordering within columns, rows aligned with the pins
feeding each component; inputs first and outputs last in port order, which is also the pin
order of the circuit as a subcircuit. Constants and tunnels with one connection sit beside
their pin. Wires are left to the router.

Synthesized circuits carry `HdlInfo` (digest of the sources). The application keeps them
read-only (commands editing their components are refused), compares the digest with the
sources on disk to show them out of date, and applies a synthesis as one command replacing
the circuits.

## `logibuild` (app)

| Module | Responsibility |
| --- | --- |
| `app` | State, messages, `update`, `view`, subscriptions, file dialogs, simulation loop. |
| `canvas` | The editor `canvas::Program`: hit testing, drags, drawing layers. |
| `shapes` | Drawing of each component kind. |
| `routing` | Pin directions, simple L/Z routes (drag previews and fallbacks), waypoints, junction dots. |
| `autoroute` | The automatic router. |
| `waveform` | The timing diagram canvas. |
| `palette`, `props` | Left and right panels. |
| `commands` | Command palette: the commands of the menus, components and circuits, filtered by what is typed. |
| `theme` | Light and dark colour schemes (from KDE Breeze), widget styles (KDE Plasma look), embedded Noto Sans, signal colours, text scaling (`fs`). |
| `settings` | Colour scheme, text size and HDL options, saved in the user configuration directory. |
| `hdl` | (feature `hdl`) HDL panel: sources, editor with highlighting, jobs run on a thread, file polling, applying a synthesis. |
| `icons` | Symbolic 16 px toolbar and menu icons (inline SVG, tinted per state). |
| `mcp` | (feature `mcp`) Model Context Protocol server for agents: local socket listener, stdio bridge, JSON-RPC, tools. |

The application owns the project, the history, the selection and one simulator per
simulation root. The canvas keeps only transient interaction state and publishes
`EditorEvent`s; all edits go through `history` commands. After each edit the geometry
and routes of the visible circuit are recomputed and the simulator is refreshed.

### Agents (MCP)

With `--mcp`, a subscription runs a thread listening on a local socket (`interprocess`: a Unix
domain socket file with mode 0600, or a named pipe on Windows). Each newline-delimited JSON-RPC
message becomes a `Message::Mcp` handled by `App::update`, which sends the answer back through
a channel; the thread answers one message at a time. Tools reuse the messages of the user
interface and `App::exec`, so agent edits are undoable and the window shows them at once. When
an action fails, the tool takes the message of the dialog `App::warn` opened and returns it to
the agent, so agent errors do not interrupt the user. The
screenshot tool answers later, from the task that captures the window. MCP clients start
servers on stdio, so `--mcp-bridge` only copies bytes between stdio and the socket.

### Automatic routing

Each wire is routed by A* on the grid from the stub of one pin to the stub of the other,
on a dense board covering the circuit. Component bodies are obstacles; running along a
segment of another net, turning on it or touching its corners is forbidden; crossings and
bends have a cost; segments of the same net are cheap to reuse, so fan-outs share trunks
and get junction dots. The point in front of every pin is reserved for its net: other
wires may cross it but not end, turn or run there. Short wires are routed first. A wire
without a legal route falls back to a plain L/Z shape (and does not constrain the others).

When the result has crossings or fallbacks, the wires involved and those around them
(grown until the area covers whole bundles, at most 64 wires) are ripped up and routed
again shortest or longest first, preferring to bend near the start, near the goal or
anywhere; the layout with the fewest fallbacks, then crossings, bends and length is kept.
This turns bundles of parallel wires, e.g. into or out of a bus, into staircases, also
when they are drawn one at a time.

Routes are memoized by their endpoints and reused while still valid, so an edit only
reroutes what it affects: 5 300 components and 5 250 wires are rerouted in ~17 ms after
an edit (~1.4 s from scratch).

### Rendering

The canvas draws three layers: the grid (cached per view), a static layer with every
component whose look does not depend on simulation values (cached until the circuit or
the view changes), and a dynamic layer redrawn every frame with the wires batched into one
path per colour, junction dots, the live components (inputs, outputs, LEDs, displays,
registers…), the selection and the overlays. Everything is culled to the visible area; at
far zoom components are drawn as coloured blocks. A 4 900-component circuit fully visible
and simulating renders at 60 fps.

## Tests

* `core` unit tests: signal truth tables, every primitive, layouts, netlist, simulator,
  trace, file format (golden file and round trips), history.
* `core/tests/circuits.rs`: SR latch, master-slave D flip-flop, 4-bit counter, 8-bit
  ripple-carry adder, shift registers, ALU, RAM, oscillation, driver conflicts, recursion.
* `app` tests drive `App::update` headless with the same messages the widgets produce, and
  test the router (obstacles, net separation, trunks, bus staircases, memo; a 5 000-component
  stress test
  runs with `cargo test --release`).
* `cargo bench -p lb_core`: simulation and compilation benchmarks (criterion).

Local CI: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
