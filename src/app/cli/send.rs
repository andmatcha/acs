use super::common::{
    PortSpec, default_baud_rate, default_log_dir, next_value, parse_port_spec, parse_u32_arg,
    resolve_requested_port_spec,
};
use super::config;
use super::help::{is_help_flag, print_send_help};
use super::signal;
use crate::output::OutputFormat;
use crate::port_display::{PortDisplayConfig, parse_display_assignment};
use crate::serial;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const SEND_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Default)]
struct SendCliOptions {
    port: Option<PortSpec>,
    baud: Option<u32>,
    format: Option<String>,
    display: PortDisplayConfig,
    monitor_ports: Vec<PortSpec>,
    config_path: Option<PathBuf>,
    log_dir: Option<PathBuf>,
}

struct SendSettings {
    inputs: Vec<SessionInputSpec>,
    output: SessionOutputSpec,
    format: OutputFormat,
    log_dir: PathBuf,
}

struct SendRunResult {
    settings: SendSettings,
    payload_len: usize,
    sent_count: u64,
    log_path: PathBuf,
}

pub(crate) fn run(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_send_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let cli_options = match parse_send_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_send_help(bin_name);
            return ExitCode::from(2);
        }
    };

    match run_with_options(cli_options) {
        Ok(result) => {
            println!(
                "sent {} packets ({} bytes each) to {} @ {} baud, format={}",
                result.sent_count,
                result.payload_len,
                result.settings.output.port,
                result.settings.output.baud_rate,
                result.settings.format.as_str()
            );
            println!("log saved to {}", result.log_path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run_with_options(cli_options: SendCliOptions) -> Result<SendRunResult, String> {
    let loaded_config = config::load_config_or_default(cli_options.config_path.as_deref())?;
    let settings = build_settings(cli_options, loaded_config.config, &loaded_config.lookup)?;
    let payload = settings.format.encode_dummy_payload()?;
    let output_id = settings.output.id.clone();
    let output_port = settings.output.port.clone();
    let output_baud = settings.output.baud_rate;
    let extra_monitor_ports = settings
        .inputs
        .iter()
        .filter(|input| input.port != output_port)
        .map(|input| format!("{}@{}", input.port, input.baud_rate))
        .collect::<Vec<_>>();
    let mut session = SessionRuntime::new(SessionSpec {
        title: String::from("acs send"),
        command_name: String::from("send"),
        raw_input: false,
        log_dir: settings.log_dir.clone(),
        inputs: settings.inputs.clone(),
        outputs: vec![settings.output.clone()],
    })?;
    let log_path = session.log_path().to_path_buf();
    let log_path_display = log_path.display().to_string();
    let mut sent_count = 0u64;
    let mut last_error = None;
    let mut output_has_error = false;

    let mut header_lines = vec![
        format!(
            "output: {} @ {} baud, format={}",
            output_port,
            output_baud,
            settings.format.as_str()
        ),
        format!("payload: {} bytes", payload.len()),
    ];
    if !extra_monitor_ports.is_empty() {
        header_lines.push(format!(
            "extra monitors: {}",
            extra_monitor_ports.join(", ")
        ));
    }
    header_lines.push(format!("log: {log_path_display}"));
    header_lines.push(String::from("Space で表示を一時停止/再開  Ctrl-C で終了"));
    session.set_header_lines(header_lines);
    signal::install_handler();
    session.run_loop_with_tick(
        SEND_INTERVAL,
        signal::is_stop_requested,
        |_, _| Ok(()),
        |session| {
            match session.write_output(&output_id, &payload) {
                Ok(()) => {
                    if output_has_error {
                        session.clear_output_error(&output_id)?;
                        output_has_error = false;
                    }
                    sent_count = sent_count.saturating_add(1);
                    last_error = None;
                }
                Err(error) => {
                    session.set_output_error(&output_id, &error)?;
                    output_has_error = true;
                    last_error = Some(error);
                }
            }
            Ok(())
        },
    )?;

    if sent_count == 0 {
        if let Some(error) = last_error {
            return Err(error);
        }
    }

    Ok(SendRunResult {
        settings,
        payload_len: payload.len(),
        sent_count,
        log_path,
    })
}

fn build_settings(
    cli_options: SendCliOptions,
    file_config: config::AppConfig,
    config_lookup: &super::paths::ConfigLookup,
) -> Result<SendSettings, String> {
    let config = file_config.send;
    let using_cli_port = cli_options.port.is_some();
    let using_cli_monitor_ports = !cli_options.monitor_ports.is_empty();
    let selected_port = resolve_requested_port_spec(cli_options.port, config.port);
    let default_baud = cli_options
        .baud
        .or(config.baud)
        .unwrap_or_else(default_baud_rate);
    let format_name = cli_options
        .format
        .or(config.format)
        .unwrap_or_else(|| String::from("packetacv6"));
    let format = OutputFormat::parse(&format_name)?;
    let mut display = config.display;
    if !using_cli_port
        && let Some(port_spec) = selected_port.as_ref()
        && let Some(mode) = port_spec.display_mode
    {
        display.set_output(port_spec.port.clone(), mode);
    }
    let monitor_port_specs = if using_cli_monitor_ports {
        cli_options.monitor_ports
    } else {
        config.monitor_ports
    };
    if !using_cli_monitor_ports {
        for port_spec in &monitor_port_specs {
            if let Some(mode) = port_spec.display_mode {
                display.set_input(port_spec.port.clone(), mode);
            }
        }
    }
    display.merge_from(cli_options.display);
    let port = match &selected_port {
        Some(port_spec) => {
            serial::resolve_port(Some(&port_spec.port)).map_err(|error| error.to_string())?
        }
        None => serial::resolve_port(None).map_err(|error| error.to_string())?,
    };
    let output_baud = selected_port
        .as_ref()
        .and_then(|port_spec| port_spec.baud)
        .unwrap_or(default_baud);
    let output_display_mode = if using_cli_port {
        selected_port
            .as_ref()
            .and_then(|port_spec| port_spec.display_mode)
            .unwrap_or(display.resolve_output(&port))
    } else {
        display.resolve_output(&port)
    };
    let log_dir = cli_options
        .log_dir
        .or(config.log_dir)
        .or(file_config.log_dir)
        .unwrap_or_else(|| default_log_dir(config_lookup));
    let mut inputs = vec![SessionInputSpec {
        id: port.clone(),
        port: port.clone(),
        baud_rate: output_baud,
        display_mode: display.resolve_input(&port),
    }];
    for port_spec in monitor_port_specs {
        let monitor_port =
            serial::resolve_port(Some(&port_spec.port)).map_err(|error| error.to_string())?;
        if inputs
            .iter()
            .any(|input: &SessionInputSpec| input.port == monitor_port)
        {
            continue;
        }
        inputs.push(SessionInputSpec {
            id: monitor_port.clone(),
            port: monitor_port.clone(),
            baud_rate: port_spec.baud.unwrap_or(default_baud),
            display_mode: if using_cli_monitor_ports {
                port_spec
                    .display_mode
                    .unwrap_or(display.resolve_input(&monitor_port))
            } else {
                display.resolve_input(&monitor_port)
            },
        });
    }

    Ok(SendSettings {
        inputs,
        output: SessionOutputSpec {
            id: String::from("main"),
            port,
            baud_rate: output_baud,
            format_name: format.as_str().to_owned(),
            display_mode: output_display_mode,
        },
        format,
        log_dir,
    })
}

fn parse_send_args(args: Vec<String>) -> Result<SendCliOptions, String> {
    let mut options = SendCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--port" | "-p" => {
                options.port = Some(parse_port_spec(
                    "--port",
                    &next_value(&mut iter, "--port")?,
                )?)
            }
            "--baud" | "-b" => {
                let value = next_value(&mut iter, "--baud")?;
                options.baud = Some(parse_u32_arg("--baud", &value)?);
            }
            "--format" | "-f" => options.format = Some(next_value(&mut iter, "--format")?),
            "--display" => {
                let value = next_value(&mut iter, "--display")?;
                let assignment = parse_display_assignment(&value)?;
                options.display.set_for_stream(
                    assignment.stream,
                    assignment.target,
                    assignment.mode,
                );
            }
            "--monitor" | "-m" => options.monitor_ports.push(parse_port_spec(
                "--monitor",
                &next_value(&mut iter, "--monitor")?,
            )?),
            "--config" => {
                options.config_path = Some(PathBuf::from(next_value(&mut iter, "--config")?))
            }
            "--log-dir" => {
                options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            other => return Err(format!("unknown option for send: {other}")),
        }
    }

    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::{parse_send_args, resolve_requested_port_spec};
    use crate::app::cli::common::PortSpec;
    use crate::port_display::PortDisplayMode;
    use std::path::PathBuf;

    #[test]
    fn parse_send_args_accepts_port_baud_and_format() {
        let options = parse_send_args(vec![
            String::from("--port"),
            String::from("/dev/ttyUSB0"),
            String::from("--baud"),
            String::from("921600"),
            String::from("--format"),
            String::from("PacketACv6"),
            String::from("--monitor"),
            String::from("/dev/ttyUSB1"),
        ])
        .expect("should parse");

        assert_eq!(
            options.port.as_ref().map(|port| port.port.as_str()),
            Some("/dev/ttyUSB0")
        );
        assert_eq!(options.baud, Some(921_600));
        assert_eq!(options.format.as_deref(), Some("PacketACv6"));
        assert_eq!(
            options.monitor_ports,
            vec![PortSpec {
                port: String::from("/dev/ttyUSB1"),
                baud: None,
                display_mode: None,
            }]
        );
    }

    #[test]
    fn parse_send_args_accepts_display_and_log_dir() {
        let options = parse_send_args(vec![
            String::from("--display"),
            String::from("output:default=hex"),
            String::from("--config"),
            String::from("config"),
            String::from("--log-dir"),
            String::from("tmp/send-logs"),
        ])
        .expect("should parse");

        assert_eq!(
            options.display.resolve_output("/dev/ttyUSB0"),
            PortDisplayMode::Hex
        );
        assert_eq!(options.config_path, Some(PathBuf::from("config")));
        assert_eq!(options.log_dir, Some(PathBuf::from("tmp/send-logs")));
    }

    #[test]
    fn requested_port_falls_back_to_config_when_cli_port_is_empty() {
        assert_eq!(
            resolve_requested_port_spec(
                Some(PortSpec {
                    port: String::from(""),
                    baud: None,
                    display_mode: None,
                }),
                Some(PortSpec {
                    port: String::from("/dev/ttyUSB0"),
                    baud: Some(115_200),
                    display_mode: Some(PortDisplayMode::Hex),
                }),
            ),
            Some(PortSpec {
                port: String::from("/dev/ttyUSB0"),
                baud: Some(115_200),
                display_mode: Some(PortDisplayMode::Hex),
            })
        );
        assert_eq!(
            resolve_requested_port_spec(
                None,
                Some(PortSpec {
                    port: String::from("   "),
                    baud: None,
                    display_mode: None,
                })
            ),
            None
        );
    }
}
