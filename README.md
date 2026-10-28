# LogiBuild

A digital logic circuit editor and simulator inspired by Logisim, written in Rust with an
[iced](https://iced.rs) user interface.

* Four-valued signals (`0 1 X Z`) on buses up to 64 bits; 47 components: gates, plexers,
  arithmetic, flip-flops, registers, counters, ROM/RAM, subcircuits, a keyboard, and displays (numbers,
  7-segment digits, LED matrix, text terminal, pixel screen).
* Event-driven, deterministic simulation with gate delays or zero-delay mode, oscillation
  detection, single stepping by event, delta, tick or clock edge (~9 M gate evaluations/s).
* Automatic orthogonal wire routing around components, with editable waypoints.
* Integrated waveform viewer with CSV/VCD export (GTKWave).
* Unlimited undo/redo, copy/paste, alignment; text project files that diff well in git.
* Command palette (Ctrl+Shift+P) to find any command, component or circuit by name.
* Optional MCP server (`--features mcp`): AI agents such as Claude Code can build, simulate and
  inspect circuits in an open window, over a local socket.
* Optional VHDL and Verilog support: an editor for the sources, compilation and synthesis with
  GHDL and Yosys into readable circuits (operators stay whole, hierarchy becomes subcircuits).

## Building

LogiBuild needs Rust 1.97 or later (edition 2024), installed with [rustup](https://rustup.rs).
The same commands then build and run it on every platform:

Empty project:

```bash
cargo run --release -p logibuild
```

Open a file:

```bash
cargo run --release -p logibuild -- examples/game_of_life.lbc
```

Open and run:

```bash
cargo run --release -p logibuild -- --run examples/game_of_life.lbc
```

The executable is `target/release/logibuild` (`logibuild.exe` on Windows); it embeds its fonts
and needs no other files. Each platform needs a few system packages first.

### Linux

A C toolchain, `pkg-config` and the headers for keyboard input, Wayland and X11, plus a Vulkan
driver.

On Debian and Ubuntu:

```bash
sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev libx11-dev \
    libvulkan1 mesa-vulkan-drivers
```

On Fedora:

```bash
sudo dnf install gcc pkgconf-pkg-config libxkbcommon-devel wayland-devel libX11-devel \
    vulkan-loader mesa-vulkan-drivers
```

On Arch Linux:

```bash
sudo pacman -S base-devel libxkbcommon wayland libx11 vulkan-icd-loader
```

plus the Vulkan driver for your GPU (`vulkan-radeon`, `vulkan-intel`, `nvidia-utils`...).
Without a Vulkan driver iced falls back to its software renderer. The Open and Save dialogs
use the desktop portal (`xdg-desktop-portal`), which desktop environments normally provide.

### Windows

Install the [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
with the *Desktop development with C++* workload, then rustup with its default
`x86_64-pc-windows-msvc` toolchain. Build from PowerShell or the command prompt with the
commands above (paths such as `examples\stopwatch.lbc` work too). Rendering uses Direct3D 12 or
Vulkan, whichever the GPU driver offers.

### macOS

Install the Xcode command line tools, then rustup:

```bash
xcode-select --install
```

Rendering uses Metal; nothing else is needed.

## Examples

| File | Shows |
| --- | --- |
| `electronic_dice.lbc` | hold ROLL to throw: a counter spinning while a button is down, gates lighting the pips, an adder showing the number |
| `stopwatch.lbc` | real-time stopwatch: four chained decade counters, a T flip-flop for start/stop, ROM decoders, a 4-digit 7-segment display |
| `rot13_terminal.lbc` | keyboard echoed on two text terminals, letters encrypted with comparators, an adder and a subtractor |
| `munching_squares.lbc` | the classic display hack, animated on a 32 x 32 colour pixel screen |
| `led_marquee.lbc` | scrolling text on an LED matrix: 64-bit constants, rotators, tunnels |
| `array_multiplier.lbc` | 4 x 4 multiplier in three levels of subcircuits (`adder_row`, `full_adder`, `half_adder`) |
| `game_of_life.lbc` | Conway's Game of Life on an 8 x 8 torus, computed a row at a time with bit-sliced carry-save adders |
| `computer.lbc` | 8-bit computer (control ROM, register file, ALU, RAM, keyboard, terminal) running a calculator program |

The first six are small enough to read at a glance; the last two are larger designs built from
several subcircuits.

`examples/hdl/hdl_demo.lbc` comes with VHDL and Verilog sources (a counter on a 7-segment
digit, an ALU, a traffic light, an LFSR, a RAM): enable HDL in the settings, then
**HDL → Synthesize** (GHDL and Yosys are needed, see the [user guide](docs/USER_GUIDE.md)).


## Documentation

* [User guide](docs/USER_GUIDE.md) — editing, simulation, waveforms, subcircuits, HDL, shortcuts
* [Component reference](docs/COMPONENTS.md) — every component with its pins and properties
* [File format](docs/FILE_FORMAT.md) — `.lbc` projects and `.hex` memory images
* [Architecture](docs/ARCHITECTURE.md) — crates, simulation engine, router, rendering, tests

API documentation: `cargo doc --no-deps --open`.

## Development

```bash
cargo test --workspace                       # all tests (headless)
cargo test --release -p logibuild            # also the 5 000-component routing stress test
cargo bench -p lb_core                       # criterion benchmarks
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
```

```text
core/      lb_core: signals, components, netlist, simulator, trace, file I/O, undo/redo
hdl/       lb_hdl: GHDL/Yosys toolchain, import and layout of synthesized netlists
app/       logibuild: iced editor (canvas, router, panels, waveform, theme, HDL panel)
examples/  example circuits
docs/      documentation
```

## License

Copyright 2026 Giovanni Novati. Licensed under either of

* [Apache License, Version 2.0](LICENSE-APACHE)
* [MIT License](LICENSE-MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed
as above, without any additional terms or conditions.
