use super::common::{
    PortSpec, default_baud_rate, default_log_dir, next_value, parse_port_spec, parse_u32_arg,
};
use super::help::{is_help_flag, print_monitor_help};
use super::signal;
use crate::port_display::{PortDisplayConfig, parse_display_assignment};
use crate::serial;
use crate::session::runtime::{SessionInputSpec, SessionRuntime, SessionSpec};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const WAIT_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
struct MonitorCliOptions {
    ports: Vec<PortSpec>,
    baud: Option<u32>,
    display: PortDisplayConfig,
    log_dir: Option<PathBuf>,
}

struct MonitorSettings {
    inputs: Vec<SessionInputSpec>,
    log_dir: PathBuf,
}

pub(crate) fn run(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_monitor_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let cli_options = match parse_monitor_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_monitor_help(bin_name);
            return ExitCode::from(2);
        }
    };

    match run_with_options(cli_options) {
        Ok(log_path) => {
            println!("log saved to {}", log_path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run_with_options(cli_options: MonitorCliOptions) -> Result<PathBuf, String> {
    let settings = build_settings(cli_options)?;
    let mut session = SessionRuntime::new(SessionSpec {
        title: String::from("acs monitor"),
        command_name: String::from("monitor"),
        log_dir: settings.log_dir.clone(),
        logging_enabled: true,
        inputs: settings.inputs.clone(),
        outputs: Vec::new(),
    })?;
    let log_path = session.log_path().to_path_buf();
    let log_path_display = log_path.display().to_string();
    let port_summary = settings
        .inputs
        .iter()
        .map(|input| format!("{}@{}", input.port, input.baud_rate))
        .collect::<Vec<_>>()
        .join(", ");

    signal::install_handler();
    session.set_header_lines(vec![
        format!("ports: {port_summary}"),
        format!("log: {log_path_display}"),
        String::from("Space で表示を一時停止/再開  Ctrl-C で終了"),
    ]);
    session.run_loop(WAIT_INTERVAL, signal::is_stop_requested, |_, _| Ok(()))
}

fn build_settings(cli_options: MonitorCliOptions) -> Result<MonitorSettings, String> {
    let requested_ports = cli_options.ports;
    let default_baud = cli_options.baud.unwrap_or_else(default_baud_rate);
    let display = cli_options.display;
    let inputs = if requested_ports.is_empty() {
        let port = serial::resolve_port(None).map_err(|error| error.to_string())?;
        vec![SessionInputSpec {
            id: port.clone(),
            display_mode: display.resolve_input(&port),
            line_break_mode: display.resolve_line_break_input(&port),
            port,
            baud_rate: default_baud,
        }]
    } else {
        let mut inputs = Vec::new();
        for port_spec in requested_ports {
            let port =
                serial::resolve_port(Some(&port_spec.port)).map_err(|error| error.to_string())?;
            if inputs
                .iter()
                .any(|input: &SessionInputSpec| input.port == port)
            {
                continue;
            }
            inputs.push(SessionInputSpec {
                id: port.clone(),
                display_mode: port_spec
                    .display_mode
                    .unwrap_or(display.resolve_input(&port)),
                line_break_mode: port_spec
                    .line_break_mode
                    .unwrap_or(display.resolve_line_break_input(&port)),
                baud_rate: port_spec.baud.unwrap_or(default_baud),
                port,
            });
        }
        inputs
    };
    let log_dir = cli_options.log_dir.unwrap_or_else(default_log_dir);

    Ok(MonitorSettings { inputs, log_dir })
}

fn parse_monitor_args(args: Vec<String>) -> Result<MonitorCliOptions, String> {
    let mut options = MonitorCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--port" | "-p" => options.ports.push(parse_port_spec(
                "--port",
                &next_value(&mut iter, "--port")?,
            )?),
            "--baud" | "-b" => {
                let value = next_value(&mut iter, "--baud")?;
                options.baud = Some(parse_u32_arg("--baud", &value)?);
            }
            "--display" => {
                let value = next_value(&mut iter, "--display")?;
                let assignment = parse_display_assignment(&value)?;
                assignment.apply_to(&mut options.display);
            }
            "--log-dir" => {
                options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            other => return Err(format!("unknown option for monitor: {other}")),
        }
    }

    Ok(options)
}
