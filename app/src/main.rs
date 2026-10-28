//! LogiBuild: digital logic circuit editor and simulator.

mod app;
mod autoroute;
mod canvas;
mod commands;
mod elide;
#[cfg(feature = "hdl")]
mod hdl;
mod help;
mod icons;
#[cfg(feature = "mcp")]
mod mcp;
mod palette;
mod props;
mod routing;
mod settings;
mod shapes;
mod theme;
mod waveform;

fn main() -> iced::Result {
    let mut file = None;
    let mut run = false;
    #[cfg(feature = "mcp")]
    let mut mcp = None;
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--run") => run = true,
            Some("--help" | "-h") => {
                println!("usage: logibuild [--run] [FILE.lbc]\n  --run  start the simulation immediately");
                #[cfg(feature = "mcp")]
                println!(
                    "  --mcp[=NAME]         let AI agents drive this window (Model Context Protocol)\n  \
                     --mcp-bridge[=NAME]  MCP server over stdio for agents: connects to the window, starting one"
                );
                return Ok(());
            }
            #[cfg(feature = "mcp")]
            Some(a) if a == "--mcp" || a.starts_with("--mcp=") => {
                mcp = Some(a.strip_prefix("--mcp=").unwrap_or(mcp::DEFAULT_NAME).to_string());
            }
            #[cfg(feature = "mcp")]
            Some(a) if a == "--mcp-bridge" || a.starts_with("--mcp-bridge=") => {
                let name = a.strip_prefix("--mcp-bridge=").unwrap_or(mcp::DEFAULT_NAME);
                if let Err(e) = mcp::bridge(name) {
                    eprintln!("logibuild: cannot reach LogiBuild on {}: {e}", mcp::socket_path(name));
                    std::process::exit(1);
                }
                return Ok(());
            }
            _ => file = Some(std::path::PathBuf::from(arg)),
        }
    }
    iced::application(
        move || {
            #[allow(unused_mut)]
            let (mut app, task) = app::App::new(file.clone(), run, settings::Settings::user_path());
            #[cfg(feature = "mcp")]
            {
                app.mcp = mcp.clone().map(|name| mcp::Server { name, ..Default::default() });
            }
            (app, task)
        },
        app::App::update,
        app::App::view,
    )
    .font(theme::FONT_DATA[0])
    .font(theme::FONT_DATA[1])
    .title(app::App::title)
    .subscription(app::App::subscription)
    .theme(|_: &app::App| theme::theme())
    .default_font(theme::FONT)
    .window_size((1440.0, 900.0))
    .exit_on_close_request(false)
    .antialiasing(true)
    .run()
}

#[cfg(test)]
mod tests;
