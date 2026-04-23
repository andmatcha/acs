use super::common::{
    PortSpec, default_baud_rate, default_log_dir, next_value, parse_key_value_args,
    parse_port_spec, parse_u32_arg,
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
    no_log: bool,
}

struct MonitorSettings {
    inputs: Vec<SessionInputSpec>,
    log_dir: PathBuf,
    logging_enabled: bool,
}

struct MonitorRunResult {
    logging_enabled: bool,
    log_path: PathBuf,
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
        Ok(result) => {
            if result.logging_enabled {
                println!("log saved to {}", result.log_path.display());
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run_with_options(cli_options: MonitorCliOptions) -> Result<MonitorRunResult, String> {
    let settings = build_settings(cli_options)?;
    let mut session = SessionRuntime::new(SessionSpec {
        title: String::from("acs monitor"),
        command_name: String::from("monitor"),
        log_dir: settings.log_dir.clone(),
        logging_enabled: settings.logging_enabled,
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
    let mut header_lines = vec![format!("ports: {port_summary}")];
    if settings.logging_enabled {
        header_lines.push(format!("log: {log_path_display}"));
    } else {
        header_lines.push(String::from("log: disabled (--no-log)"));
    }
    header_lines.push(String::from("Space で表示を一時停止/再開  Ctrl-C で終了"));
    session.set_header_lines(header_lines);
    session.run_loop(WAIT_INTERVAL, signal::is_stop_requested, |_, _| Ok(()))?;
    Ok(MonitorRunResult {
        logging_enabled: settings.logging_enabled,
        log_path,
    })
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

    Ok(MonitorSettings {
        inputs,
        log_dir,
        logging_enabled: !cli_options.no_log,
    })
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
            "--config" => {
                apply_monitor_config_args(&mut options, &next_value(&mut iter, "--config")?)?
            }
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
            "--no-log" => options.no_log = true,
            other => return Err(format!("unknown option for monitor: {other}")),
        }
    }

    Ok(options)
}

fn apply_monitor_config_args(options: &mut MonitorCliOptions, value: &str) -> Result<(), String> {
    for assignment in parse_key_value_args("--config", value)? {
        let key = assignment.key.to_ascii_uppercase();
        match key.as_str() {
            "DISPLAY" => {
                let display = parse_display_assignment(&assignment.value)?;
                display.apply_to(&mut options.display);
            }
            "LOG_DIR" => options.log_dir = Some(PathBuf::from(assignment.value)),
            other => return Err(format!("unknown monitor config key: {other}")),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_monitor_args;
    use std::path::PathBuf;

    #[test]
    fn parse_monitor_args_accepts_config_and_no_log() {
        let options = parse_monitor_args(vec![
            String::from("--port"),
            String::from("/dev/ttyUSB0@921600,utf8"),
            String::from("--config"),
            String::from("DISPLAY=input:default=hex+packet,LOG_DIR=tmp/monitor-logs"),
            String::from("--no-log"),
        ])
        .expect("should parse");

        assert_eq!(options.ports.len(), 1);
        assert_eq!(options.ports[0].port, "/dev/ttyUSB0");
        assert_eq!(options.ports[0].baud, Some(921_600));
        assert_eq!(options.log_dir, Some(PathBuf::from("tmp/monitor-logs")));
        assert!(options.no_log);
    }
}
