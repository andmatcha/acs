mod commands;
pub(crate) mod common;
mod control;
mod help;
mod monitor;
mod paths;
mod route;
mod send;
mod signal;
mod version;
mod xbee_mock;
mod xbee_rtt;
mod xbee_test;

use std::env;
use std::process::ExitCode;

pub fn run() -> ExitCode {
    let mut args = env::args();
    let bin_name = args.next().unwrap_or_else(|| String::from("acs"));

    match args.next().as_deref() {
        Some("--help") | Some("-h") => {
            help::print_help(&bin_name);
            ExitCode::SUCCESS
        }
        Some("--version") | Some("-V") => version::print_version(),
        Some("help") => help::print_help_topic(&bin_name, args.next().as_deref()),
        Some("version") => version::print_version(),
        Some("controllers") => commands::list_controllers(),
        Some("ports") => commands::list_ports(),
        Some("control") => control::run(args.collect(), &bin_name),
        Some("monitor") => monitor::run(args.collect(), &bin_name),
        Some("route") => route::run(args.collect(), &bin_name),
        Some("send") => send::run(args.collect(), &bin_name),
        Some("xbee-mock") => xbee_mock::run(args.collect(), &bin_name),
        Some("xbee-rtt") => xbee_rtt::run(args.collect(), &bin_name),
        Some("xbee-test") => xbee_test::run(args.collect(), &bin_name),
        Some(command) => {
            eprintln!("不明なサブコマンドです: {command}");
            help::print_usage(&bin_name);
            ExitCode::from(2)
        }
        None => {
            help::print_help(&bin_name);
            ExitCode::SUCCESS
        }
    }
}
