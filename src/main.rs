mod app;
mod common;
mod ingress;
mod input;
mod network;
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
