mod app;
mod common;
mod input;
mod output;
mod pipeline;
mod port_display;
mod serial;
mod session;
mod ui;

use std::process::ExitCode;

fn main() -> ExitCode {
    app::cli::run()
}
