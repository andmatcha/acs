use super::common::{default_baud_rate, default_log_dir, next_value, parse_u32_arg};
use super::config;
use super::help::{is_help_flag, print_send_help};
use super::signal;
use crate::output::OutputFormat;
use crate::port_display::{PortDisplayConfig, parse_display_assignment};
use crate::serial;
use crate::session::runtime::{SessionOutputSpec, SessionRuntime, SessionSpec};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const SEND_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Default)]
struct SendCliOptions {
    port: Option<String>,
    baud: Option<u32>,
    format: Option<String>,
    display: PortDisplayConfig,
    config_path: Option<PathBuf>,
    log_dir: Option<PathBuf>,
}

struct SendSettings {
    port: String,
    baud: u32,
    format: OutputFormat,
    display: PortDisplayConfig,
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
                result.settings.port,
                result.settings.baud,
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
    let file_config = config::load_config_or_default(cli_options.config_path.as_deref())?;
    let settings = build_settings(cli_options, file_config)?;
    let payload = settings.format.encode_dummy_payload()?;
    let output_id = String::from("main");
    let mut session = SessionRuntime::new(SessionSpec {
        title: String::from("acs send"),
        command_name: String::from("send"),
        raw_input: false,
        log_dir: settings.log_dir.clone(),
        inputs: Vec::new(),
        outputs: vec![SessionOutputSpec {
            id: output_id.clone(),
            port: settings.port.clone(),
            baud_rate: settings.baud,
            format_name: settings.format.as_str().to_owned(),
            display_mode: settings.display.resolve_output(&settings.port),
        }],
    })?;
    let log_path = session.log_path().to_path_buf();
    let log_path_display = log_path.display().to_string();
    let mut sent_count = 0u64;
    let mut last_error = None;
    let mut output_has_error = false;

    session.set_header_lines(vec![
        format!(
            "output: {} @ {} baud, format={}",
            settings.port,
            settings.baud,
            settings.format.as_str()
        ),
        format!("payload: {} bytes", payload.len()),
        format!("log: {log_path_display}"),
        String::from("Space で表示を一時停止/再開  Ctrl-C で終了"),
    ]);
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
) -> Result<SendSettings, String> {
    let raw_port = resolve_requested_port(cli_options.port, file_config.send.port);
    let port = serial::resolve_port(raw_port.as_deref()).map_err(|error| error.to_string())?;
    let baud = cli_options
        .baud
        .or(file_config.send.baud)
        .unwrap_or_else(default_baud_rate);
    let format_name = cli_options
        .format
        .or(file_config.send.format)
        .unwrap_or_else(|| String::from("packetacv6"));
    let format = OutputFormat::parse(&format_name)?;
    let mut display = file_config.send.display;
    display.merge_from(cli_options.display);
    let log_dir = cli_options
        .log_dir
        .or(file_config.send.log_dir)
        .or(file_config.log_dir)
        .unwrap_or_else(default_log_dir);

    Ok(SendSettings {
        port,
        baud,
        format,
        display,
        log_dir,
    })
}

fn parse_send_args(args: Vec<String>) -> Result<SendCliOptions, String> {
    let mut options = SendCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--port" | "-p" => options.port = Some(next_value(&mut iter, "--port")?),
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

fn resolve_requested_port(cli_port: Option<String>, config_port: Option<String>) -> Option<String> {
    normalize_requested_port(cli_port).or_else(|| normalize_requested_port(config_port))
}

fn normalize_requested_port(port: Option<String>) -> Option<String> {
    port.and_then(|port| {
        let trimmed = port.trim();
        (!trimmed.is_empty()).then_some(trimmed.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_send_args, resolve_requested_port};
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
        ])
        .expect("should parse");

        assert_eq!(options.port.as_deref(), Some("/dev/ttyUSB0"));
        assert_eq!(options.baud, Some(921_600));
        assert_eq!(options.format.as_deref(), Some("PacketACv6"));
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
            resolve_requested_port(Some(String::from("")), Some(String::from("/dev/ttyUSB0"))),
            Some(String::from("/dev/ttyUSB0"))
        );
        assert_eq!(
            resolve_requested_port(
                Some(String::from("   ")),
                Some(String::from("/dev/ttyUSB0"))
            ),
            Some(String::from("/dev/ttyUSB0"))
        );
    }
}
