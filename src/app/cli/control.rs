use super::common::{dedup_strings, default_baud_rate, default_log_dir};
use super::config;
use super::help::{is_help_flag, print_control_help};
use super::logger::CommandLogger;
use super::signal;
use crate::input::compact;
use crate::input::ds4_hid::Ds4Controller;
use crate::output::OutputFormat;
use crate::port_display::{PortDisplayConfig, parse_display_assignment};
use crate::serial::{
    self, SerialCallback, SerialConfig, SerialConnection, SerialEvent, SerialLineBuffer,
    SerialMonitor,
};
use crate::ui::text_dashboard::{TextDashboard, TextDashboardAction};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const CONTROLLER_POLL_MILLIS: i32 = 20;
const RENDER_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Default)]
struct ControlCliOptions {
    port: Option<String>,
    baud: Option<u32>,
    controller: Option<String>,
    format: Option<String>,
    raw: bool,
    display: PortDisplayConfig,
    monitor_ports: Vec<String>,
    config_path: Option<PathBuf>,
    log_dir: Option<PathBuf>,
}

struct ControlSettings {
    port: String,
    baud: u32,
    controller: Option<String>,
    format: OutputFormat,
    raw: bool,
    display: PortDisplayConfig,
    monitor_ports: Vec<String>,
    log_dir: PathBuf,
}

pub(crate) fn run(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_control_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let cli_options = match parse_control_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_control_help(bin_name);
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

fn run_with_options(cli_options: ControlCliOptions) -> Result<PathBuf, String> {
    let file_config = config::load_config_or_default(cli_options.config_path.as_deref())?;
    let settings = build_settings(cli_options, file_config)?;

    let mut controller = Ds4Controller::open(settings.controller.as_deref())
        .map_err(|error| format!("failed to open controller: {error}"))?;
    let controller_info = controller.info().clone();
    let mut driver = settings.format.create_driver();
    let mut logger = CommandLogger::create("control", &settings.log_dir)
        .map_err(|error| format!("failed to create log file: {error}"))?;

    let (event_tx, event_rx) = mpsc::channel::<SerialEvent>();
    let callback = make_serial_callback(event_tx);
    let serial_config = SerialConfig {
        port: settings.port.clone(),
        baud_rate: settings.baud,
    };
    let mut output = SerialConnection::open(&serial_config, Arc::clone(&callback))
        .map_err(|error| error.to_string())?;

    let extra_monitor_ports = settings
        .monitor_ports
        .into_iter()
        .filter(|port| port != output.port_name())
        .collect::<Vec<_>>();
    let _extra_monitors = open_monitors(&extra_monitor_ports, settings.baud, callback)?;

    signal::install_handler();

    let mut dashboard = TextDashboard::new("acs control")
        .map_err(|error| format!("failed to initialize dashboard: {error}"))?;
    dashboard.set_header_lines(vec![
        format!(
            "controller: {} ({})",
            controller_info
                .product_name
                .as_deref()
                .unwrap_or("unknown controller"),
            controller_info.path
        ),
        format!(
            "output: {} @ {} baud, format={}",
            settings.port,
            settings.baud,
            driver.format_name()
        ),
        format!(
            "input mode: {}",
            if settings.raw { "raw" } else { "line" }
        ),
        format!("log: {}", logger.path().display()),
        String::from("Space で表示を一時停止/再開  Ctrl-C で終了"),
    ]);
    dashboard.set_output_status(
        &settings.port,
        format!("baud={} format={}", settings.baud, driver.format_name()),
    );
    dashboard.set_output_baud_rate(&settings.port, settings.baud);
    dashboard.set_output_display_mode(&settings.port, settings.display.resolve_output(&settings.port));
    dashboard.set_input_status(&settings.port, format!("baud={}", settings.baud));
    dashboard.set_input_baud_rate(&settings.port, settings.baud);
    dashboard.set_input_display_mode(&settings.port, settings.display.resolve_input(&settings.port));
    for port in &extra_monitor_ports {
        dashboard.set_input_status(port, format!("baud={}", settings.baud));
        dashboard.set_input_baud_rate(port, settings.baud);
        dashboard.set_input_display_mode(port, settings.display.resolve_input(port));
    }

    dashboard
        .render(None)
        .map_err(|error| format!("failed to render dashboard: {error}"))?;
    let mut dirty = false;
    let mut last_render = Instant::now();
    let mut paused = false;
    let mut line_buffer = SerialLineBuffer::default();

    loop {
        if handle_dashboard_action(&mut dashboard, &mut paused)? {
            dirty = true;
        }

        if drain_serial_events(
            &event_rx,
            &mut dashboard,
            &mut logger,
            &mut line_buffer,
            settings.raw,
        )? {
            dirty = true;
        }

        if signal::is_stop_requested() {
            break;
        }

        if let Some(report) = controller
            .read_next_report(CONTROLLER_POLL_MILLIS)
            .map_err(|error| format!("failed to read controller input: {error}"))?
        {
            if let Ok(compact_report) = compact::convert_input_report(&report) {
                let bytes = driver.encode(&compact_report)?;
                output.write_bytes(&bytes).map_err(|error| error.to_string())?;
                dashboard.record_output_bytes(output.port_name(), &bytes);
                dashboard.add_output(output.port_name(), &bytes);
                logger
                    .log_output(output.port_name(), &bytes)
                    .map_err(|error| format!("failed to write log: {error}"))?;
                dirty = true;
            }
        }

        if !paused && dirty && last_render.elapsed() >= RENDER_INTERVAL {
            dashboard
                .render(None)
                .map_err(|error| format!("failed to render dashboard: {error}"))?;
            dirty = false;
            last_render = Instant::now();
        } else if dirty {
            continue;
        }
    }

    let _ = drain_serial_events(
        &event_rx,
        &mut dashboard,
        &mut logger,
        &mut line_buffer,
        settings.raw,
    )?;
    if !settings.raw {
        let _ = flush_pending_input_lines(&mut dashboard, &mut logger, &mut line_buffer)?;
    }
    dashboard
        .render(Some("stopped"))
        .map_err(|error| format!("failed to render dashboard: {error}"))?;

    Ok(logger.path().to_path_buf())
}
fn build_settings(
    cli_options: ControlCliOptions,
    file_config: config::AppConfig,
) -> Result<ControlSettings, String> {
    let raw_port = cli_options.port.or(file_config.control.port);
    let port = serial::resolve_port(raw_port.as_deref()).map_err(|error| error.to_string())?;
    let baud = cli_options
        .baud
        .or(file_config.control.baud)
        .unwrap_or_else(default_baud_rate);
    let controller = cli_options.controller.or(file_config.control.controller);
    let format_name = cli_options
        .format
        .or(file_config.control.format)
        .unwrap_or_else(|| String::from("arm9"));
    let format = OutputFormat::parse(&format_name)?;
    let raw = cli_options.raw || file_config.control.raw.unwrap_or(false);
    let mut display = file_config.control.display;
    display.merge_from(cli_options.display);
    let monitor_ports = if cli_options.monitor_ports.is_empty() {
        file_config.control.monitor_ports
    } else {
        cli_options.monitor_ports
    };
    let monitor_ports = dedup_strings(
        monitor_ports
            .into_iter()
            .map(|port| serial::resolve_port(Some(&port)).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let log_dir = cli_options
        .log_dir
        .or(file_config.control.log_dir)
        .or(file_config.log_dir)
        .unwrap_or_else(default_log_dir);

    Ok(ControlSettings {
        port,
        baud,
        controller,
        format,
        raw,
        display,
        monitor_ports,
        log_dir,
    })
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

fn drain_serial_events(
    event_rx: &Receiver<SerialEvent>,
    dashboard: &mut TextDashboard,
    logger: &mut CommandLogger,
    line_buffer: &mut SerialLineBuffer,
    raw_input: bool,
) -> Result<bool, String> {
    let mut changed = false;

    while let Ok(event) = event_rx.try_recv() {
        match event {
            SerialEvent::Data { port, bytes } => {
                if handle_input_bytes(&port, &bytes, dashboard, logger, line_buffer, raw_input)? {
                    changed = true;
                }
            }
            SerialEvent::Error { port, message } => {
                dashboard.set_input_status(&port, format!("error: {message}"));
                logger
                    .log_status(&port, &message)
                    .map_err(|error| format!("failed to write log: {error}"))?;
                changed = true;
            }
        }
    }

    Ok(changed)
}

fn handle_input_bytes(
    port: &str,
    bytes: &[u8],
    dashboard: &mut TextDashboard,
    logger: &mut CommandLogger,
    line_buffer: &mut SerialLineBuffer,
    raw_input: bool,
) -> Result<bool, String> {
    if raw_input {
        dashboard.record_input_bytes(port, bytes);
        dashboard.add_input(port, bytes);
        logger
            .log_input(port, bytes)
            .map_err(|error| format!("failed to write log: {error}"))?;
        return Ok(true);
    }

    dashboard.record_input_bytes(port, bytes);
    let mut changed = false;
    for line in line_buffer.push_chunk(port, bytes) {
        dashboard.add_input(port, &line);
        logger
            .log_input(port, &line)
            .map_err(|error| format!("failed to write log: {error}"))?;
        changed = true;
    }

    Ok(changed)
}

fn flush_pending_input_lines(
    dashboard: &mut TextDashboard,
    logger: &mut CommandLogger,
    line_buffer: &mut SerialLineBuffer,
) -> Result<bool, String> {
    let mut changed = false;

    for line in line_buffer.drain_pending_lines() {
        dashboard.add_input(&line.port, &line.bytes);
        logger
            .log_input(&line.port, &line.bytes)
            .map_err(|error| format!("failed to write log: {error}"))?;
        changed = true;
    }

    Ok(changed)
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

fn parse_control_args(args: Vec<String>) -> Result<ControlCliOptions, String> {
    let mut options = ControlCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--port" | "-p" => options.port = Some(next_value(&mut iter, "--port")?),
            "--baud" | "-b" => {
                let value = next_value(&mut iter, "--baud")?;
                options.baud = Some(parse_u32_arg("--baud", &value)?);
            }
            "--controller" | "-c" => {
                options.controller = Some(next_value(&mut iter, "--controller")?);
            }
            "--format" | "-f" => options.format = Some(next_value(&mut iter, "--format")?),
            "--raw" => options.raw = true,
            "--display" => {
                let value = next_value(&mut iter, "--display")?;
                let assignment = parse_display_assignment(&value)?;
                options
                    .display
                    .set_for_stream(assignment.stream, assignment.target, assignment.mode);
            }
            "--monitor" | "-m" => {
                options
                    .monitor_ports
                    .push(next_value(&mut iter, "--monitor")?)
            }
            "--config" => options.config_path = Some(PathBuf::from(next_value(&mut iter, "--config")?)),
            "--log-dir" => options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?)),
            other => return Err(format!("unknown option for control: {other}")),
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
