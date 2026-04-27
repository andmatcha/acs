use super::common::{
    PortSpec, default_baud_rate, default_log_dir, next_value, parse_key_value_args,
    parse_port_spec, parse_u32_arg,
};
use super::help::{is_help_flag, print_xbee_talk_help};
use super::signal;
use crate::port_display::{PortDisplayConfig, PortDisplayMode, parse_display_assignment};
use crate::serial;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const TALK_LOOP_INTERVAL: Duration = Duration::from_millis(1);
const TALK_OUTPUT_ID: &str = "main";
const TALK_FORMAT_NAME: &str = "xbee-talk";

#[derive(Debug, Default)]
struct XbeeTalkCliOptions {
    port: Option<PortSpec>,
    inputs: Vec<PortSpec>,
    baud: Option<u32>,
    display: PortDisplayConfig,
    log_dir: Option<PathBuf>,
    no_log: bool,
    s3b: bool,
}

struct XbeeTalkSettings {
    output: SessionOutputSpec,
    inputs: Vec<SessionInputSpec>,
    log_dir: PathBuf,
    logging_enabled: bool,
    s3b: bool,
}

struct XbeeTalkRunResult {
    message_count: u64,
    output_port: String,
    baud_rate: u32,
    logging_enabled: bool,
    log_path: PathBuf,
}

pub(crate) fn run(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_xbee_talk_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let cli_options = match parse_xbee_talk_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_xbee_talk_help(bin_name);
            return ExitCode::from(2);
        }
    };

    match run_with_options(cli_options) {
        Ok(result) => {
            println!(
                "sent {} messages to {} @ {} baud",
                result.message_count, result.output_port, result.baud_rate
            );
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

fn run_with_options(cli_options: XbeeTalkCliOptions) -> Result<XbeeTalkRunResult, String> {
    let settings = build_settings(cli_options)?;
    let output = settings.output.clone();
    let mut session = SessionRuntime::new(SessionSpec {
        title: String::from("acs xbee-talk"),
        command_name: String::from("xbee-talk"),
        log_dir: settings.log_dir.clone(),
        logging_enabled: settings.logging_enabled,
        xbee_s3b_recovery: settings.s3b,
        inputs: settings.inputs.clone(),
        outputs: vec![output.clone()],
    })?;
    let log_path = session.log_path().to_path_buf();
    let log_path_display = log_path.display().to_string();
    session.set_interactive_input(true);
    session.set_header_lines(build_header_lines(&settings, &log_path_display));

    signal::install_handler();
    let mut message_count = 0u64;
    let mut output_has_error = false;
    let mut last_error = None::<String>;

    session.run_loop_with_tick(
        TALK_LOOP_INTERVAL,
        signal::is_stop_requested,
        |_, _| Ok(()),
        |session| {
            if let Some(input) = session.take_user_input() {
                message_count = message_count.saturating_add(1);
                let mut bytes = input.into_bytes();
                bytes.extend_from_slice(b"\r\n");

                match session.write_output(TALK_OUTPUT_ID, &bytes) {
                    Ok(()) => {
                        if output_has_error {
                            session.clear_output_error(TALK_OUTPUT_ID)?;
                            output_has_error = false;
                        }
                        last_error = None;
                    }
                    Err(error) => {
                        session.set_output_error(TALK_OUTPUT_ID, &error)?;
                        output_has_error = true;
                        last_error = Some(error);
                    }
                }
            }
            Ok(())
        },
    )?;

    if message_count == 0
        && let Some(error) = last_error
    {
        return Err(error);
    }

    Ok(XbeeTalkRunResult {
        message_count,
        output_port: output.port,
        baud_rate: output.baud_rate,
        logging_enabled: settings.logging_enabled,
        log_path,
    })
}

fn build_settings(cli_options: XbeeTalkCliOptions) -> Result<XbeeTalkSettings, String> {
    let default_baud = cli_options.baud.unwrap_or_else(default_baud_rate);
    let selected_port = cli_options.port.clone().and_then(PortSpec::normalized);
    let output_port = match &selected_port {
        Some(port_spec) => {
            serial::resolve_port(Some(&port_spec.port)).map_err(|error| error.to_string())?
        }
        None => serial::resolve_port(None).map_err(|error| error.to_string())?,
    };
    let output_baud = selected_port
        .as_ref()
        .and_then(|port_spec| port_spec.baud)
        .unwrap_or(default_baud);
    let output_display_mode = selected_port
        .as_ref()
        .and_then(|port_spec| port_spec.display_mode)
        .or_else(|| cli_options.display.resolve_output_override(&output_port))
        .unwrap_or(PortDisplayMode::HexUtf8);

    let output = SessionOutputSpec {
        id: String::from(TALK_OUTPUT_ID),
        port: output_port.clone(),
        baud_rate: output_baud,
        format_name: String::from(TALK_FORMAT_NAME),
        display_mode: output_display_mode,
    };

    let mut inputs = Vec::new();
    add_unique_input(
        &mut inputs,
        SessionInputSpec {
            id: output_port.clone(),
            port: output_port.clone(),
            baud_rate: output_baud,
            display_mode: selected_port
                .as_ref()
                .and_then(|port_spec| port_spec.display_mode)
                .or_else(|| cli_options.display.resolve_input_override(&output_port))
                .unwrap_or(PortDisplayMode::HexUtf8),
            line_break_mode: selected_port
                .as_ref()
                .and_then(|port_spec| port_spec.line_break_mode)
                .unwrap_or_else(|| cli_options.display.resolve_line_break_input(&output_port)),
        },
    )?;

    for input in cli_options.inputs {
        let input = input
            .normalized()
            .ok_or_else(|| String::from("xbee-talk input port must not be empty"))?;
        let port = serial::resolve_port(Some(&input.port)).map_err(|error| error.to_string())?;
        let baud_rate = input.baud.unwrap_or(default_baud);
        add_unique_input(
            &mut inputs,
            SessionInputSpec {
                id: port.clone(),
                port: port.clone(),
                baud_rate,
                display_mode: input
                    .display_mode
                    .or_else(|| cli_options.display.resolve_input_override(&port))
                    .unwrap_or(PortDisplayMode::HexUtf8),
                line_break_mode: input
                    .line_break_mode
                    .unwrap_or_else(|| cli_options.display.resolve_line_break_input(&port)),
            },
        )?;
    }

    Ok(XbeeTalkSettings {
        output,
        inputs,
        log_dir: cli_options.log_dir.unwrap_or_else(default_log_dir),
        logging_enabled: !cli_options.no_log,
        s3b: cli_options.s3b,
    })
}

fn add_unique_input(
    inputs: &mut Vec<SessionInputSpec>,
    input: SessionInputSpec,
) -> Result<(), String> {
    if inputs
        .iter()
        .any(|existing| existing.port == input.port && existing.baud_rate != input.baud_rate)
    {
        return Err(format!(
            "xbee-talk input port `{}` cannot use multiple baud rates",
            input.port
        ));
    }
    if inputs
        .iter()
        .any(|existing| existing.port == input.port && existing.baud_rate == input.baud_rate)
    {
        return Ok(());
    }
    inputs.push(input);
    Ok(())
}

fn build_header_lines(settings: &XbeeTalkSettings, log_path: &str) -> Vec<String> {
    let input_summary = settings
        .inputs
        .iter()
        .map(|input| format!("{}@{}", input.port, input.baud_rate))
        .collect::<Vec<_>>()
        .join(", ");
    let mut lines = vec![
        format!(
            "output: {} @ {} baud",
            settings.output.port, settings.output.baud_rate
        ),
        format!("input: {input_summary}"),
    ];
    if settings.logging_enabled {
        lines.push(format!("log: {log_path}"));
    } else {
        lines.push(String::from("log: disabled (--no-log)"));
    }
    lines.push(String::from(
        "Enter で送信 (\\r\\n を末尾に付加)  Ctrl-C で終了",
    ));
    lines
}

fn parse_xbee_talk_args(args: Vec<String>) -> Result<XbeeTalkCliOptions, String> {
    let mut options = XbeeTalkCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--port" | "-p" => {
                options.port = Some(parse_port_spec(
                    "--port",
                    &next_value(&mut iter, "--port")?,
                )?)
            }
            "--input" | "--monitor" | "-m" => options.inputs.push(parse_port_spec(
                "--input",
                &next_value(&mut iter, "--input")?,
            )?),
            "--interactive" | "-i" => {}
            "--config" => {
                apply_xbee_talk_config_args(&mut options, &next_value(&mut iter, "--config")?)?
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
            "--s3b" => options.s3b = true,
            other => return Err(format!("unknown option for xbee-talk: {other}")),
        }
    }

    Ok(options)
}

fn apply_xbee_talk_config_args(
    options: &mut XbeeTalkCliOptions,
    value: &str,
) -> Result<(), String> {
    for assignment in parse_key_value_args("--config", value)? {
        let key = assignment.key.to_ascii_uppercase();
        match key.as_str() {
            "BAUD" => options.baud = Some(parse_u32_arg("BAUD", &assignment.value)?),
            "DISPLAY" => {
                let display = parse_display_assignment(&assignment.value)?;
                display.apply_to(&mut options.display);
            }
            "LOG_DIR" => options.log_dir = Some(PathBuf::from(assignment.value)),
            other => return Err(format!("unknown xbee-talk config key: {other}")),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_xbee_talk_args;
    use crate::port_display::PortDisplayMode;
    use std::path::PathBuf;

    #[test]
    fn parse_xbee_talk_args_accepts_legacy_interactive_flag() {
        let options = parse_xbee_talk_args(vec![
            String::from("-i"),
            String::from("--port"),
            String::from("/dev/ttyUSB0@921600,utf8"),
            String::from("--monitor"),
            String::from("/dev/ttyUSB1@115200,hex+packet"),
            String::from("--no-log"),
        ])
        .expect("should parse");

        let port = options.port.expect("port");
        assert_eq!(port.port, "/dev/ttyUSB0");
        assert_eq!(port.baud, Some(921_600));
        assert_eq!(port.display_mode, Some(PortDisplayMode::Utf8));
        assert_eq!(options.inputs.len(), 1);
        assert_eq!(options.inputs[0].port, "/dev/ttyUSB1");
        assert!(options.no_log);
    }

    #[test]
    fn parse_xbee_talk_config_accepts_display_and_log_dir() {
        let options = parse_xbee_talk_args(vec![
            String::from("--config"),
            String::from("BAUD=921600,DISPLAY=input:default=utf8+line,LOG_DIR=tmp/talk"),
        ])
        .expect("should parse");

        assert_eq!(options.baud, Some(921_600));
        assert_eq!(
            options.display.resolve_input("/dev/ttyUSB0"),
            PortDisplayMode::Utf8
        );
        assert_eq!(options.log_dir, Some(PathBuf::from("tmp/talk")));
    }
}
