//! The command palette (Ctrl+Shift+P): every menu command, every component
//! to place and every circuit, found by typing part of their name.

use iced::widget::{Space, button, column, container, mouse_area, opaque, row, text, text_input};
use iced::{Alignment, Element, Fill, Padding};
use lb_core::component::Kind;

use crate::app::{Align, App, Message};
use crate::settings::{Scheme, TEXT_SIZES};
use crate::theme;

pub const INPUT_ID: &str = "command-palette";

/// Entries shown at once; the list follows the highlighted one.
pub const ROWS: usize = 12;

/// The open palette: what is typed and which match is highlighted.
#[derive(Debug, Default)]
pub struct CommandPalette {
    pub query: String,
    /// Index of the highlighted entry among the matches.
    pub selected: usize,
    /// First match shown.
    pub first: usize,
}

impl CommandPalette {
    /// Moves the highlight by `delta` among `count` matches, wrapping around,
    /// and scrolls the list to keep it shown.
    pub fn step(&mut self, delta: i32, count: usize) {
        if count == 0 {
            return;
        }
        self.selected = (self.selected as i64 + delta as i64).rem_euclid(count as i64) as usize;
        self.reveal();
    }

    /// Scrolls the list so that the highlighted entry is shown.
    fn reveal(&mut self) {
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + ROWS {
            self.first = self.selected + 1 - ROWS;
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// "Group: name", e.g. "Edit: Rotate".
    pub label: String,
    /// The keyboard shortcut, if any.
    pub key: &'static str,
    pub msg: Message,
    pub enabled: bool,
}

fn entry(group: &str, name: &str, key: &'static str, msg: Message, enabled: bool) -> Entry {
    Entry { label: format!("{group}: {name}"), key, msg, enabled }
}

/// Every command the palette offers, in menu order, then the components and
/// the circuits.
pub fn entries(app: &App) -> Vec<Entry> {
    let on = app.enabled();
    let mut v = vec![
        entry("File", "New", "Ctrl+N", Message::New, true),
        entry("File", "Open…", "Ctrl+O", Message::Open, true),
        entry("File", "Save", "Ctrl+S", Message::Save, true),
        entry("File", "Save as…", "Ctrl+Shift+S", Message::SaveAs, true),
        entry("File", "New circuit", "", Message::NewCircuit, true),
        entry("File", "Quit", "", Message::Quit, true),
        entry(
            "Edit",
            format!("Undo {}", app.history.undo_label().unwrap_or("")).trim_end(),
            "Ctrl+Z",
            Message::Undo,
            app.history.can_undo(),
        ),
        entry(
            "Edit",
            format!("Redo {}", app.history.redo_label().unwrap_or("")).trim_end(),
            "Ctrl+Shift+Z",
            Message::Redo,
            app.history.can_redo(),
        ),
        entry("Edit", "Cut", "Ctrl+X", Message::Cut, on.sel),
        entry("Edit", "Copy", "Ctrl+C", Message::Copy, on.sel),
        entry("Edit", "Paste", "Ctrl+V", Message::Paste, on.paste),
        entry("Edit", "Duplicate", "Ctrl+D", Message::Duplicate, on.sel),
        entry("Edit", "Delete", "Del", Message::Delete, on.delete),
        entry("Edit", "Select all", "Ctrl+A", Message::SelectAll, true),
        entry("Edit", "Rotate", "R", Message::Rotate, on.transform),
        entry("Edit", "Flip", "F", Message::Mirror, on.transform),
        entry("Edit", "Align left", "", Message::Align(Align::Left), on.align),
        entry("Edit", "Align top", "", Message::Align(Align::Top), on.align),
        entry("Edit", "Center horizontally", "", Message::Align(Align::CenterX), on.align),
        entry("Edit", "Center vertically", "", Message::Align(Align::CenterY), on.align),
        entry("Simulation", if app.running { "Pause" } else { "Run" }, "Space", Message::ToggleRun, on.sim),
        entry("Simulation", "Reset", "", Message::Reset, on.sim),
        entry("Simulation", &format!("Single step ({})", app.step_kind), "S", Message::Step, on.sim),
        entry(
            "Simulation",
            if app.zero_delay { "Turn zero-delay mode off" } else { "Turn zero-delay mode on" },
            "",
            Message::SetZeroDelay(!app.zero_delay),
            true,
        ),
        entry("View", "Zoom to fit", "Ctrl+0", Message::FitView, true),
        entry("View", "Zoom in", "Ctrl++", Message::ZoomIn, true),
        entry("View", "Zoom out", "Ctrl+-", Message::ZoomOut, true),
        entry("View", if app.show_grid { "Hide grid" } else { "Show grid" }, "", Message::ToggleGrid, true),
        entry(
            "View",
            if app.wave.visible { "Hide waveform" } else { "Show waveform" },
            "",
            Message::ToggleWaveform,
            true,
        ),
    ];
    if !app.hdl_shown() {
        let help = app.help_kind();
        let name = help.map_or("Component documentation".into(), |k| format!("Documentation of {}", k.name()));
        v.push(entry("Help", &name, "F1", help.map_or(Message::Noop, Message::ShowHelp), help.is_some()));
    }
    #[cfg(feature = "hdl")]
    if app.settings.hdl {
        use crate::hdl::Msg as H;
        use lb_hdl::lang::Lang;
        let idle = app.hdl.running().is_none();
        let sources = !app.project.hdl.sources.is_empty();
        v.extend([
            entry(
                "HDL",
                if app.hdl.panel { "Hide HDL view" } else { "Show HDL view" },
                "",
                Message::Hdl(H::TogglePanel),
                true,
            ),
            entry("HDL", "Open files…", "", Message::Hdl(H::AddFiles), true),
            entry("HDL", "New VHDL file…", "", Message::Hdl(H::NewFile(Lang::Vhdl)), true),
            entry("HDL", "New Verilog file…", "", Message::Hdl(H::NewFile(Lang::Verilog)), true),
            entry("HDL", "Compile", "", Message::Hdl(H::Compile), idle && sources),
            entry("HDL", "Synthesize", "", Message::Hdl(H::Synthesize), idle && sources),
            entry("HDL", "Cancel", "", Message::Hdl(H::Cancel), !idle),
            entry("HDL", "HDL tools…", "", Message::Hdl(H::ShowTools), true),
        ]);
    }
    for scheme in Scheme::ALL {
        let name = format!("Color scheme {}", scheme.label());
        v.push(entry("Settings", &name, "", Message::SetScheme(scheme), app.settings.scheme != scheme));
    }
    for n in TEXT_SIZES {
        let name = format!("Text size {n}%");
        v.push(entry("Settings", &name, "", Message::SetTextSize(n), app.settings.text_size != n));
    }
    if cfg!(feature = "hdl") {
        let s = &app.settings;
        let name = if s.hdl { "Disable HDL" } else { "Enable HDL" };
        v.push(entry("Settings", name, "", Message::SetHdlEnabled(!s.hdl), true));
        if s.hdl {
            let name = if s.hdl_auto {
                "Do not synthesize when a source is saved"
            } else {
                "Synthesize when a source is saved"
            };
            v.push(entry("Settings", name, "", Message::SetHdlAuto(!s.hdl_auto), true));
        }
    }
    // Components and circuits belong to the circuit view.
    if app.hdl_shown() {
        return v;
    }
    for &k in Kind::ALL.iter().filter(|k| **k != Kind::Subcircuit) {
        v.push(entry("Place", k.name(), "", Message::PickTool(k), true));
    }
    let here = &app.circuit().name;
    for c in &app.project.circuits {
        let name = &c.name;
        v.push(entry("Open circuit", name, "", Message::OpenCircuit(name.clone()), name != here));
        if app.placement_refusal(name).is_none() {
            v.push(entry("Place subcircuit", name, "", Message::PickSubcircuit(name.clone()), true));
        }
    }
    v
}

/// The entries matching `query`: every word of it must occur in the label,
/// ignoring case. Names starting with the query come first, then those where
/// every word starts a word of the label, then the others, each group in
/// the original order.
pub fn filter(entries: Vec<Entry>, query: &str) -> Vec<Entry> {
    let query = query.trim().to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return entries;
    }
    let mut ranked: Vec<(u8, Entry)> = entries
        .into_iter()
        .filter_map(|e| {
            let label = e.label.to_lowercase();
            if !words.iter().all(|w| label.contains(w)) {
                return None;
            }
            let name = label.split_once(": ").map_or(label.as_str(), |(_, n)| n);
            let starts_word = |w: &str| {
                label.match_indices(w).any(|(i, _)| i == 0 || !label.as_bytes()[i - 1].is_ascii_alphanumeric())
            };
            let rank = if name.starts_with(&query) || label.starts_with(&query) {
                0
            } else if words.iter().all(|w| starts_word(w)) {
                1
            } else {
                2
            };
            Some((rank, e))
        })
        .collect();
    // Stable: equal ranks keep the original order.
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, e)| e).collect()
}

/// The entries matching what is typed in the open palette.
pub fn matches(app: &App, palette: &CommandPalette) -> Vec<Entry> {
    filter(entries(app), &palette.query)
}

pub fn view<'a>(app: &'a App, palette: &'a CommandPalette) -> Element<'a, Message> {
    let input = text_input("Type a command, component or circuit…", &palette.query)
        .id(INPUT_ID)
        .on_input(Message::CommandQuery)
        .on_submit(Message::CommandRun(palette.selected))
        .size(theme::fs(14.0))
        .padding([7, 10])
        .style(theme::input);
    let found = matches(app, palette);
    let mut list = column![].spacing(1);
    for (i, e) in found.iter().enumerate().skip(palette.first).take(ROWS) {
        let color = if e.enabled { theme::colors().muted } else { theme::colors().disabled };
        list = list.push(
            button(
                row![
                    text(e.label.clone()).size(theme::fs(theme::BODY)).width(Fill),
                    text(e.key).size(theme::fs(theme::SMALL)).color(color),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .on_press_maybe(e.enabled.then_some(Message::CommandRun(i)))
            .padding([4, 8])
            .width(Fill)
            .style(theme::item(i == palette.selected)),
        );
    }
    let footer: Element<Message> = if found.is_empty() {
        container(text("No matching commands").size(theme::fs(theme::SMALL)).color(theme::colors().muted))
            .padding(8)
            .into()
    } else if found.len() > ROWS {
        let shown = format!("{}–{} of {}", palette.first + 1, (palette.first + ROWS).min(found.len()), found.len());
        container(text(shown).size(theme::fs(theme::TINY)).color(theme::colors().muted)).padding([2, 8]).into()
    } else {
        Space::new().into()
    };
    let card =
        container(column![input, list, footer].spacing(6)).padding(6).width(theme::fs(520.0)).style(theme::dialog);
    // Below the menu bar and toolbar, centred horizontally.
    let dim = container(opaque(card))
        .width(Fill)
        .height(Fill)
        .align_x(Alignment::Center)
        .padding(Padding { top: theme::fs(64.0), ..Padding::ZERO })
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color { a: 0.2, ..iced::Color::BLACK })),
            ..Default::default()
        });
    opaque(mouse_area(dim).on_press(Message::CloseCommands))
}
