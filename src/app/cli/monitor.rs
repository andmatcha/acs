use super::common::{dedup_strings, default_baud_rate, default_log_dir, next_value, parse_u32_arg};
use super::config;
use super::help::{is_help_flag, print_monitor_help};
use super::serial_dashboard::{SerialDashboard, make_serial_callback, open_serial_monitors};
use super::signal;
use crate::port_display::{PortDisplayConfig, parse_display_assignment};
use crate::serial::{self, SerialEvent};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const RENDER_INTERVAL: Duration = Duration::from_millis(100);
const WAIT_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
struct MonitorCliOptions {
    ports: Vec<String>,
    baud: Option<u32>,
    raw: bool,
    display: PortDisplayConfig,
    config_path: Option<PathBuf>,
    log_dir: Option<PathBuf>,
}

struct MonitorSettings {
    ports: Vec<String>,
    baud: u32,
    raw: bool,
    display: PortDisplayConfig,
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
    let file_config = config::load_config_or_default(cli_options.config_path.as_deref())?;
    let settings = build_settings(cli_options, file_config)?;
    let mut dashboard =
        SerialDashboard::new("acs monitor", settings.raw, "monitor", &settings.log_dir)?;
    let log_path = dashboard.log_path().to_path_buf();
    let log_path_display = log_path.display().to_string();

    let (event_tx, event_rx) = mpsc::channel::<SerialEvent>();
    let callback = make_serial_callback(event_tx);
    let _monitors = open_serial_monitors(&settings.ports, settings.baud, callback)?;

    signal::install_handler();

    dashboard.set_header_lines(vec![
        format!("ports: {}", settings.ports.join(", ")),
        format!("baud: {}", settings.baud),
        format!("input mode: {}", if settings.raw { "raw" } else { "line" }),
        format!("log: {log_path_display}"),
        String::from("Space で表示を一時停止/再開  Ctrl-C で終了"),
    ]);
    for port in &settings.ports {
        dashboard.configure_input_port(port, settings.baud, settings.display.resolve_input(port));
    }

    dashboard.render(None)?;
    let mut dirty = false;
    let mut last_render = Instant::now();

    while !signal::is_stop_requested() {
        dashboard.handle_dashboard_action()?;
        if dashboard.wait_for_serial_events(&event_rx, WAIT_INTERVAL)? {
            dirty = true;
        }
        if !dashboard.is_paused() && dirty && last_render.elapsed() >= RENDER_INTERVAL {
            dashboard.render(None)?;
            dirty = false;
            last_render = Instant::now();
        }
    }

    let _ = dashboard.drain_serial_events(&event_rx)?;
    let _ = dashboard.flush_pending_input_lines()?;
    dashboard.render(Some("stopped"))?;

    Ok(log_path)
}

fn build_settings(
    cli_options: MonitorCliOptions,
    file_config: config::AppConfig,
) -> Result<MonitorSettings, String> {
    let requested_ports = if cli_options.ports.is_empty() {
        file_config.monitor.ports
    } else {
        cli_options.ports
    };
    let ports = if requested_ports.is_empty() {
        vec![serial::resolve_port(None).map_err(|error| error.to_string())?]
    } else {
        requested_ports
            .into_iter()
            .map(|port| serial::resolve_port(Some(&port)).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?
    };
    let ports = dedup_strings(ports);
    let baud = cli_options
        .baud
        .or(file_config.monitor.baud)
        .unwrap_or_else(default_baud_rate);
    let raw = cli_options.raw || file_config.monitor.raw.unwrap_or(false);
    let mut display = file_config.monitor.display;
    display.merge_from(cli_options.display);
    let log_dir = cli_options
        .log_dir
        .or(file_config.monitor.log_dir)
        .or(file_config.log_dir)
        .unwrap_or_else(default_log_dir);

    Ok(MonitorSettings {
        ports,
        baud,
        raw,
        display,
        log_dir,
    })
}

fn parse_monitor_args(args: Vec<String>) -> Result<MonitorCliOptions, String> {
    let mut options = MonitorCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--port" | "-p" => options.ports.push(next_value(&mut iter, "--port")?),
            "--baud" | "-b" => {
                let value = next_value(&mut iter, "--baud")?;
                options.baud = Some(parse_u32_arg("--baud", &value)?);
            }
            "--raw" => options.raw = true,
            "--display" => {
                let value = next_value(&mut iter, "--display")?;
                let assignment = parse_display_assignment(&value)?;
                options.display.set_for_stream(
                    assignment.stream,
                    assignment.target,
                    assignment.mode,
                );
            }
            "--config" => {
                options.config_path = Some(PathBuf::from(next_value(&mut iter, "--config")?))
            }
            "--log-dir" => {
                options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            other => return Err(format!("unknown option for monitor: {other}")),
        }
    }

    Ok(options)
}
