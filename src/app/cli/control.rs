use super::common::{dedup_strings, default_baud_rate, default_log_dir, next_value, parse_u32_arg};
use super::config;
use super::help::{is_help_flag, print_control_help};
use super::signal;
use crate::input::compact;
use crate::input::ds4_hid::Ds4Controller;
use crate::output::OutputFormat;
use crate::port_display::{PortDisplayConfig, parse_display_assignment};
use crate::serial;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const LOOP_INTERVAL: Duration = Duration::from_millis(20);
const CONTROLLER_POLL_MILLIS: i32 = 0;

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
    let extra_monitor_ports = settings
        .monitor_ports
        .into_iter()
        .filter(|port| port != &settings.port)
        .collect::<Vec<_>>();
    let mut inputs = vec![SessionInputSpec {
        id: settings.port.clone(),
        port: settings.port.clone(),
        baud_rate: settings.baud,
        display_mode: settings.display.resolve_input(&settings.port),
    }];
    inputs.extend(extra_monitor_ports.iter().map(|port| SessionInputSpec {
        id: port.clone(),
        port: port.clone(),
        baud_rate: settings.baud,
        display_mode: settings.display.resolve_input(port),
    }));
    let mut session = SessionRuntime::new(SessionSpec {
        title: String::from("acs control"),
        command_name: String::from("control"),
        raw_input: settings.raw,
        log_dir: settings.log_dir.clone(),
        header_lines: Vec::new(),
        inputs,
        outputs: vec![SessionOutputSpec {
            id: String::from("main"),
            port: settings.port.clone(),
            baud_rate: settings.baud,
            format_name: driver.format_name().to_owned(),
            display_mode: settings.display.resolve_output(&settings.port),
        }],
    })?;
    let log_path = session.log_path().to_path_buf();
    let log_path_display = log_path.display().to_string();

    signal::install_handler();
    session.set_header_lines(vec![
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
        format!("input mode: {}", if settings.raw { "raw" } else { "line" }),
        format!("log: {log_path_display}"),
        String::from("Space で表示を一時停止/再開  Ctrl-C で終了"),
    ]);
    session.run_loop_with_tick(
        LOOP_INTERVAL,
        signal::is_stop_requested,
        |_, _| Ok(()),
        |session| {
            if let Some(report) = controller
                .read_next_report(CONTROLLER_POLL_MILLIS)
                .map_err(|error| format!("failed to read controller input: {error}"))?
                && let Ok(compact_report) = compact::convert_input_report(&report)
            {
                let bytes = driver.encode(&compact_report)?;
                session.write_output("main", &bytes)?;
            }
            Ok(())
        },
    )?;

    Ok(log_path)
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
                options.display.set_for_stream(
                    assignment.stream,
                    assignment.target,
                    assignment.mode,
                );
            }
            "--monitor" | "-m" => options
                .monitor_ports
                .push(next_value(&mut iter, "--monitor")?),
            "--config" => {
                options.config_path = Some(PathBuf::from(next_value(&mut iter, "--config")?))
            }
            "--log-dir" => {
                options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            other => return Err(format!("unknown option for control: {other}")),
        }
    }

    Ok(options)
}
