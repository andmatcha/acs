mod app;
mod common;
mod input;
mod output;
mod port_display;
mod ui;
mod serial;

use std::process::ExitCode;

fn main() -> ExitCode {
    app::cli::run()
}
