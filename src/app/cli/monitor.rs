use super::common::{dedup_strings, default_baud_rate, default_log_dir};
use super::config;
use super::help::{is_help_flag, print_monitor_help};
use super::logger::CommandLogger;
use super::signal;
use crate::serial::{self, SerialCallback, SerialConfig, SerialEvent, SerialMonitor};
use crate::ui::text_dashboard::{TextDashboard, TextDashboardAction};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const RENDER_INTERVAL: Duration = Duration::from_millis(100);
const WAIT_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
struct MonitorCliOptions {
    ports: Vec<String>,
    baud: Option<u32>,
    config_path: Option<PathBuf>,
    log_dir: Option<PathBuf>,
}

struct MonitorSettings {
    ports: Vec<String>,
    baud: u32,
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
    let file_config = match cli_options.config_path.as_deref() {
        Some(path) => config::load_config(path)?,
        None => config::AppConfig::default(),
    };
    let settings = build_settings(cli_options, file_config)?;
    let mut logger = CommandLogger::create("monitor", &settings.log_dir)
        .map_err(|error| format!("failed to create log file: {error}"))?;

    let (event_tx, event_rx) = mpsc::channel::<SerialEvent>();
    let callback = make_serial_callback(event_tx);
    let _monitors = open_monitors(&settings.ports, settings.baud, callback)?;

    signal::install_handler();

    let mut dashboard = TextDashboard::new("acs monitor")
        .map_err(|error| format!("failed to initialize dashboard: {error}"))?;
    dashboard.set_header_lines(vec![
        format!("ports: {}", settings.ports.join(", ")),
        format!("baud: {}", settings.baud),
        format!("log: {}", logger.path().display()),
        String::from("Space で表示を一時停止/再開  Ctrl-C で終了"),
    ]);
    for port in &settings.ports {
        dashboard.set_input_status(port, format!("baud={}", settings.baud));
    }

    dashboard
        .render(None)
        .map_err(|error| format!("failed to render dashboard: {error}"))?;
    let mut dirty = false;
    let mut last_render = Instant::now();
    let mut paused = false;

    while !signal::is_stop_requested() {
        if handle_dashboard_action(&mut dashboard, &mut paused)? {
            dirty = true;
        }
        if wait_for_serial_events(&event_rx, &mut dashboard, &mut logger)? {
            dirty = true;
        }
        if !paused && dirty && last_render.elapsed() >= RENDER_INTERVAL {
            dashboard
                .render(None)
                .map_err(|error| format!("failed to render dashboard: {error}"))?;
            dirty = false;
            last_render = Instant::now();
        }
    }

    let _ = drain_serial_events(&event_rx, &mut dashboard, &mut logger)?;
    dashboard
        .render(Some("stopped"))
        .map_err(|error| format!("failed to render dashboard: {error}"))?;

    Ok(logger.path().to_path_buf())
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
    let log_dir = cli_options
        .log_dir
        .or(file_config.monitor.log_dir)
        .or(file_config.log_dir)
        .unwrap_or_else(default_log_dir);

    Ok(MonitorSettings { ports, baud, log_dir })
}

fn make_serial_callback(
    event_tx: mpsc::Sender<SerialEvent>,
) -> SerialCallback {
    Arc::new(move |event| {
        let _ = event_tx.send(event);
    })
}

fn open_monitors(
    ports: &[String],
    baud: u32,
    callback: SerialCallback,
) -> Result<Vec<SerialMonitor>, String> {
    ports
        .iter()
        .map(|port| {
            SerialMonitor::open(
                &SerialConfig {
                    port: port.clone(),
                    baud_rate: baud,
                },
                Arc::clone(&callback),
            )
            .map_err(|error| error.to_string())
        })
        .collect()
}

fn wait_for_serial_events(
    event_rx: &Receiver<SerialEvent>,
    dashboard: &mut TextDashboard,
    logger: &mut CommandLogger,
) -> Result<bool, String> {
    let mut changed = false;

    match event_rx.recv_timeout(WAIT_INTERVAL) {
        Ok(event) => {
            if handle_event(event, dashboard, logger)? {
                changed = true;
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err(String::from("serial monitor channel disconnected"));
        }
    }

    if drain_serial_events(event_rx, dashboard, logger)? {
        changed = true;
    }

    Ok(changed)
}

fn drain_serial_events(
    event_rx: &Receiver<SerialEvent>,
    dashboard: &mut TextDashboard,
    logger: &mut CommandLogger,
) -> Result<bool, String> {
    let mut changed = false;

    while let Ok(event) = event_rx.try_recv() {
        if handle_event(event, dashboard, logger)? {
            changed = true;
        }
    }
    Ok(changed)
}

fn handle_event(
    event: SerialEvent,
    dashboard: &mut TextDashboard,
    logger: &mut CommandLogger,
) -> Result<bool, String> {
    match event {
        SerialEvent::Data { port, bytes } => {
            dashboard.add_input(&port, &bytes);
            logger
                .log_input(&port, &bytes)
                .map_err(|error| format!("failed to write log: {error}"))?;
            Ok(true)
        }
        SerialEvent::Error { port, message } => {
            dashboard.set_input_status(&port, format!("error: {message}"));
            logger
                .log_status(&port, &message)
                .map_err(|error| format!("failed to write log: {error}"))?;
            Ok(true)
        }
    }
}

fn handle_dashboard_action(
    dashboard: &mut TextDashboard,
    paused: &mut bool,
) -> Result<bool, String> {
    let action = dashboard
        .poll_action()
        .map_err(|error| format!("failed to read keyboard input: {error}"))?;

    match action {
        Some(TextDashboardAction::TogglePause) => {
            *paused = !*paused;
            let status = if *paused {
                "paused (space: resume)"
            } else {
                "resumed"
            };
            dashboard
                .render(Some(status))
                .map_err(|error| format!("failed to render dashboard: {error}"))?;
            Ok(false)
        }
        None => Ok(false),
    }
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
            "--config" => options.config_path = Some(PathBuf::from(next_value(&mut iter, "--config")?)),
            "--log-dir" => options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?)),
            other => return Err(format!("unknown option for monitor: {other}")),
        }
    }

    Ok(options)
}

fn next_value(iter: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("missing value for {option}"))
}

fn parse_u32_arg(option: &str, value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("invalid value for {option}: {value}"))
}
