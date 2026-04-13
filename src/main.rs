mod app;
mod common;
mod input;
mod output;
mod port_display;
mod serial;
mod ui;

use std::process::ExitCode;

fn main() -> ExitCode {
    app::cli::run()
}
