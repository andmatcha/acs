use super::common::{
    PortSpec, default_baud_rate, default_log_dir, next_value, parse_port_spec, parse_u32_arg,
    resolve_requested_port_spec,
};
use super::config;
use super::help::{is_help_flag, print_send_help};
use super::signal;
use crate::output::OutputFormat;
use crate::output::formats::DummyPayloadGenerator;
use crate::port_display::{PortDisplayConfig, parse_display_assignment};
use crate::serial;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const SEND_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Default)]
struct SendCliOptions {
    port: Option<PortSpec>,
    outputs: Vec<SendOutputBinding>,
    baud: Option<u32>,
    format: Option<String>,
    display: PortDisplayConfig,
    monitor_ports: Vec<SendMonitorBinding>,
    config_path: Option<PathBuf>,
    log_dir: Option<PathBuf>,
    interactive: bool,
}

#[derive(Debug, Clone)]
struct SendOutputBinding {
    id: String,
    port: String,
    baud: Option<u32>,
    format: Option<String>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
    line_break_mode: Option<crate::port_display::LineBreakMode>,
}

#[derive(Debug, Clone)]
struct SendMonitorBinding {
    port: String,
    baud: Option<u32>,
    format: Option<String>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
    line_break_mode: Option<crate::port_display::LineBreakMode>,
}

#[derive(Debug, Clone)]
struct SendOutputSettings {
    session: SessionOutputSpec,
    format: OutputFormat,
}

#[derive(Debug, Clone)]
struct SendInputPacketFormat {
    port: String,
    format: OutputFormat,
}

struct SendSettings {
    inputs: Vec<SessionInputSpec>,
    outputs: Vec<SendOutputSettings>,
    input_packet_formats: Vec<SendInputPacketFormat>,
    log_dir: PathBuf,
    interactive: bool,
}

struct SendOutputRunResult {
    id: String,
    port: String,
    baud_rate: u32,
    format: OutputFormat,
    sent_count: u64,
    payload_len: usize,
}

struct SendRunResult {
    interactive: bool,
    message_count: u64,
    outputs: Vec<SendOutputRunResult>,
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
            if result.interactive {
                if result.outputs.len() == 1 {
                    let output = &result.outputs[0];
                    println!(
                        "sent {} messages to {} @ {} baud",
                        result.message_count, output.port, output.baud_rate
                    );
                } else {
                    println!(
                        "sent {} messages to {} outputs",
                        result.message_count,
                        result.outputs.len()
                    );
                    for output in &result.outputs {
                        println!(
                            "  {}: {} writes to {} @ {} baud",
                            output.id, output.sent_count, output.port, output.baud_rate
                        );
                    }
                }
            } else if result.outputs.len() == 1 {
                let output = &result.outputs[0];
                println!(
                    "sent {} packets ({} bytes each) to {} @ {} baud, format={}",
                    output.sent_count,
                    output.payload_len,
                    output.port,
                    output.baud_rate,
                    output.format.as_str()
                );
            } else {
                println!("sent dummy packets to {} outputs", result.outputs.len());
                for output in &result.outputs {
                    println!(
                        "  {}: {} packets ({} bytes each) to {} @ {} baud, format={}",
                        output.id,
                        output.sent_count,
                        output.payload_len,
                        output.port,
                        output.baud_rate,
                        output.format.as_str()
                    );
                }
            }
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
    let output_specs = settings.outputs.clone();
    let input_packet_formats = settings.input_packet_formats.clone();

    let mut payload_lengths = BTreeMap::new();
    let mut generators = BTreeMap::<String, Box<dyn DummyPayloadGenerator>>::new();
    if !settings.interactive {
        for output in &output_specs {
            payload_lengths.insert(
                output.session.id.clone(),
                output.format.encode_dummy_payload()?.len(),
            );
            generators.insert(
                output.session.id.clone(),
                output.format.create_dummy_generator()?,
            );
        }
    }

    let mut session = SessionRuntime::new(SessionSpec {
        title: String::from("acs send"),
        command_name: String::from("send"),
        log_dir: settings.log_dir.clone(),
        logging_enabled: true,
        inputs: settings.inputs.clone(),
        outputs: output_specs
            .iter()
            .map(|output| output.session.clone())
            .collect(),
    })?;
    for output in &output_specs {
        session.set_output_packet_rate_enabled(&output.session.port, true);
    }
    for input_packet_format in &input_packet_formats {
        session.set_input_packet_framing(
            &input_packet_format.port,
            input_packet_format.format.packet_len(),
        );
    }
    let log_path = session.log_path().to_path_buf();
    let log_path_display = log_path.display().to_string();

    let output_ports = output_specs
        .iter()
        .map(|output| output.session.port.clone())
        .collect::<BTreeSet<_>>();
    let extra_monitor_ports = settings
        .inputs
        .iter()
        .filter(|input| !output_ports.contains(&input.port))
        .map(|input| format!("{}@{}", input.port, input.baud_rate))
        .collect::<Vec<_>>();

    let mut header_lines = output_specs
        .iter()
        .map(|output| {
            if settings.interactive {
                format!(
                    "output[{}]: {} @ {} baud",
                    output.session.id, output.session.port, output.session.baud_rate
                )
            } else {
                format!(
                    "output[{}]: {} @ {} baud, format={}, payload={} bytes",
                    output.session.id,
                    output.session.port,
                    output.session.baud_rate,
                    output.format.as_str(),
                    payload_lengths
                        .get(&output.session.id)
                        .copied()
                        .unwrap_or_default()
                )
            }
        })
        .collect::<Vec<_>>();
    if settings.interactive {
        header_lines.push(String::from(
            "Enter で全出力ポートへ送信 (\\r\\n を末尾に付加)",
        ));
    }
    if !extra_monitor_ports.is_empty() {
        header_lines.push(format!(
            "extra monitors: {}",
            extra_monitor_ports.join(", ")
        ));
    }
    header_lines.push(format!("log: {log_path_display}"));
    if settings.interactive {
        header_lines.push(String::from("Ctrl-C で終了"));
    } else {
        header_lines.push(String::from("Space で表示を一時停止/再開  Ctrl-C で終了"));
    }
    session.set_header_lines(header_lines);
    signal::install_handler();

    let mut sent_counts = output_specs
        .iter()
        .map(|output| (output.session.id.clone(), 0u64))
        .collect::<BTreeMap<_, _>>();
    let mut output_has_error = output_specs
        .iter()
        .map(|output| (output.session.id.clone(), false))
        .collect::<BTreeMap<_, _>>();
    let mut last_errors = BTreeMap::<String, String>::new();
    let mut message_count = 0u64;

    if settings.interactive {
        session.set_interactive_input(true);
        session.run_loop_with_tick(
            SEND_INTERVAL,
            signal::is_stop_requested,
            |_, _| Ok(()),
            |session| {
                if let Some(input) = session.take_user_input() {
                    message_count = message_count.saturating_add(1);
                    let mut bytes = input.into_bytes();
                    bytes.extend_from_slice(b"\r\n");

                    for output in &output_specs {
                        let output_id = &output.session.id;
                        match session.write_output(output_id, &bytes) {
                            Ok(()) => {
                                if output_has_error.get(output_id).copied().unwrap_or(false) {
                                    session.clear_output_error(output_id)?;
                                    output_has_error.insert(output_id.clone(), false);
                                }
                                *sent_counts.entry(output_id.clone()).or_insert(0) += 1;
                                last_errors.remove(output_id);
                            }
                            Err(error) => {
                                session.set_output_error(output_id, &error)?;
                                output_has_error.insert(output_id.clone(), true);
                                last_errors.insert(output_id.clone(), error);
                            }
                        }
                    }
                }
                Ok(())
            },
        )?;
    } else {
        session.run_loop_with_tick(
            SEND_INTERVAL,
            signal::is_stop_requested,
            |_, _| Ok(()),
            |session| {
                for output in &output_specs {
                    let output_id = &output.session.id;
                    let payload = generators
                        .get_mut(output_id)
                        .ok_or_else(|| format!("missing dummy generator for output `{output_id}`"))?
                        .next_payload()?;
                    match session.write_output(output_id, &payload) {
                        Ok(()) => {
                            if output_has_error.get(output_id).copied().unwrap_or(false) {
                                session.clear_output_error(output_id)?;
                                output_has_error.insert(output_id.clone(), false);
                            }
                            *sent_counts.entry(output_id.clone()).or_insert(0) += 1;
                            last_errors.remove(output_id);
                        }
                        Err(error) => {
                            session.set_output_error(output_id, &error)?;
                            output_has_error.insert(output_id.clone(), true);
                            last_errors.insert(output_id.clone(), error);
                        }
                    }
                }
                Ok(())
            },
        )?;

        if sent_counts.values().all(|count| *count == 0)
            && let Some(error) = last_errors.into_values().next()
        {
            return Err(error);
        }
    }

    Ok(SendRunResult {
        interactive: settings.interactive,
        message_count,
        outputs: output_specs
            .into_iter()
            .map(|output| SendOutputRunResult {
                id: output.session.id.clone(),
                port: output.session.port,
                baud_rate: output.session.baud_rate,
                format: output.format,
                sent_count: sent_counts.remove(&output.session.id).unwrap_or_default(),
                payload_len: payload_lengths
                    .remove(&output.session.id)
                    .unwrap_or_default(),
            })
            .collect(),
        log_path,
    })
}

fn build_settings(
    cli_options: SendCliOptions,
    file_config: config::AppConfig,
    config_lookup: &super::paths::ConfigLookup,
) -> Result<SendSettings, String> {
    let config = file_config.send;
    let using_cli_outputs = !cli_options.outputs.is_empty();
    let using_cli_port = cli_options.port.is_some();
    if using_cli_outputs && using_cli_port {
        return Err(String::from(
            "cannot combine --port with --output-port; use one style or the other",
        ));
    }
    if !config.outputs.is_empty() && using_cli_port {
        return Err(String::from(
            "cannot use --port when `send.outputs` is configured; use --output-port instead",
        ));
    }

    let using_cli_monitor_ports = !cli_options.monitor_ports.is_empty();
    let default_baud = cli_options
        .baud
        .or(config.baud)
        .unwrap_or_else(default_baud_rate);
    let default_format_name = cli_options
        .format
        .clone()
        .or(config.format.clone())
        .unwrap_or_else(|| String::from("packetacv6"));
    let default_format = OutputFormat::parse(&default_format_name)?;
    let output_bindings =
        resolve_output_bindings(&cli_options, &config, default_baud, default_format)?;
    let monitor_port_specs =
        resolve_monitor_bindings(&cli_options, &config, using_cli_monitor_ports)?;

    let mut display = config.display;
    if !using_cli_monitor_ports {
        for port_spec in &monitor_port_specs {
            if let Some(mode) = port_spec.display_mode {
                display.set_input(port_spec.port.clone(), mode);
            }
            if let Some(mode) = port_spec.line_break_mode {
                display.set_line_break_for_stream(
                    Some(crate::port_display::PortDisplayStream::Input),
                    port_spec.port.clone(),
                    mode,
                );
            }
        }
    }
    display.merge_from(cli_options.display);
    let log_dir = cli_options
        .log_dir
        .or(config.log_dir)
        .or(file_config.log_dir)
        .unwrap_or_else(|| default_log_dir(config_lookup));

    let mut outputs = Vec::new();
    let mut inputs = Vec::new();
    let mut input_packet_formats = Vec::new();
    let mut seen_output_ids = BTreeSet::new();
    let mut seen_output_ports = BTreeSet::new();

    for binding in output_bindings {
        if !seen_output_ids.insert(binding.id.clone()) {
            return Err(format!("duplicate send output id: {}", binding.id));
        }

        let port = serial::resolve_port(Some(&binding.port)).map_err(|error| error.to_string())?;
        if !seen_output_ports.insert(port.clone()) {
            return Err(format!("duplicate send output port: {port}"));
        }

        let baud_rate = binding.baud.unwrap_or(default_baud);
        let format = binding.format.unwrap_or(default_format);
        let output_display_mode = binding
            .display_mode
            .unwrap_or(display.resolve_output(&port));
        let input_display_mode = binding.display_mode.unwrap_or(display.resolve_input(&port));
        let line_break_mode = binding
            .line_break_mode
            .unwrap_or(display.resolve_line_break_input(&port));

        outputs.push(SendOutputSettings {
            session: SessionOutputSpec {
                id: binding.id.clone(),
                port: port.clone(),
                baud_rate,
                format_name: format.as_str().to_owned(),
                display_mode: output_display_mode,
            },
            format,
        });
        inputs.push(SessionInputSpec {
            id: port.clone(),
            port: port.clone(),
            baud_rate,
            display_mode: input_display_mode,
            line_break_mode,
        });
        if !cli_options.interactive {
            input_packet_formats.push(SendInputPacketFormat { port, format });
        }
    }

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
            display_mode: port_spec
                .display_mode
                .unwrap_or(display.resolve_input(&monitor_port)),
            line_break_mode: port_spec
                .line_break_mode
                .unwrap_or(display.resolve_line_break_input(&monitor_port)),
        });
        if let Some(format) = port_spec.format {
            input_packet_formats.push(SendInputPacketFormat {
                port: monitor_port,
                format,
            });
        }
    }

    Ok(SendSettings {
        inputs,
        outputs,
        input_packet_formats,
        log_dir,
        interactive: cli_options.interactive,
    })
}

fn resolve_output_bindings(
    cli_options: &SendCliOptions,
    config: &config::SendConfig,
    default_baud: u32,
    default_format: OutputFormat,
) -> Result<Vec<ResolvedSendOutputBinding>, String> {
    if !cli_options.outputs.is_empty() {
        return cli_options
            .outputs
            .iter()
            .cloned()
            .map(|binding| resolve_send_output_binding(binding, default_baud, default_format))
            .collect();
    }

    if !config.outputs.is_empty() {
        return config
            .outputs
            .iter()
            .map(|output| {
                resolve_send_output_binding(
                    SendOutputBinding {
                        id: output.id.clone(),
                        port: output.port.clone(),
                        baud: output.baud,
                        format: output.format.clone(),
                        display_mode: output.display_mode,
                        line_break_mode: output.line_break_mode,
                    },
                    default_baud,
                    default_format,
                )
            })
            .collect();
    }

    let selected_port = resolve_requested_port_spec(cli_options.port.clone(), config.port.clone());
    let port = match &selected_port {
        Some(port_spec) => {
            serial::resolve_port(Some(&port_spec.port)).map_err(|error| error.to_string())?
        }
        None => serial::resolve_port(None).map_err(|error| error.to_string())?,
    };

    resolve_send_output_binding(
        SendOutputBinding {
            id: String::from("main"),
            port,
            baud: selected_port.as_ref().and_then(|port_spec| port_spec.baud),
            format: None,
            display_mode: selected_port
                .as_ref()
                .and_then(|port_spec| port_spec.display_mode),
            line_break_mode: selected_port
                .as_ref()
                .and_then(|port_spec| port_spec.line_break_mode),
        },
        default_baud,
        default_format,
    )
    .map(|binding| vec![binding])
}

#[derive(Debug, Clone)]
struct ResolvedSendOutputBinding {
    id: String,
    port: String,
    baud: Option<u32>,
    format: Option<OutputFormat>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
    line_break_mode: Option<crate::port_display::LineBreakMode>,
}

fn resolve_send_output_binding(
    binding: SendOutputBinding,
    _default_baud: u32,
    default_format: OutputFormat,
) -> Result<ResolvedSendOutputBinding, String> {
    Ok(ResolvedSendOutputBinding {
        id: binding.id,
        port: binding.port,
        baud: binding.baud,
        format: Some(match binding.format {
            Some(format) => OutputFormat::parse(&format)?,
            None => default_format,
        }),
        display_mode: binding.display_mode,
        line_break_mode: binding.line_break_mode,
    })
}

#[derive(Debug, Clone)]
struct ResolvedSendMonitorBinding {
    port: String,
    baud: Option<u32>,
    format: Option<OutputFormat>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
    line_break_mode: Option<crate::port_display::LineBreakMode>,
}

fn resolve_monitor_bindings(
    cli_options: &SendCliOptions,
    config: &config::SendConfig,
    using_cli_monitor_ports: bool,
) -> Result<Vec<ResolvedSendMonitorBinding>, String> {
    if using_cli_monitor_ports {
        return cli_options
            .monitor_ports
            .iter()
            .cloned()
            .map(resolve_send_monitor_binding)
            .collect();
    }

    config
        .monitor_ports
        .iter()
        .map(|binding| {
            resolve_send_monitor_binding(SendMonitorBinding {
                port: binding.port.clone(),
                baud: binding.baud,
                format: binding.format.clone(),
                display_mode: binding.display_mode,
                line_break_mode: binding.line_break_mode,
            })
        })
        .collect()
}

fn resolve_send_monitor_binding(
    binding: SendMonitorBinding,
) -> Result<ResolvedSendMonitorBinding, String> {
    Ok(ResolvedSendMonitorBinding {
        port: binding.port,
        baud: binding.baud,
        format: binding
            .format
            .map(|format| OutputFormat::parse(&format))
            .transpose()?,
        display_mode: binding.display_mode,
        line_break_mode: binding.line_break_mode,
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
            "--output-port" | "-o" => options.outputs.push(parse_send_output_binding(
                &next_value(&mut iter, "--output-port")?,
            )?),
            "--baud" | "-b" => {
                let value = next_value(&mut iter, "--baud")?;
                options.baud = Some(parse_u32_arg("--baud", &value)?);
            }
            "--format" | "-f" => options.format = Some(next_value(&mut iter, "--format")?),
            "--display" => {
                let value = next_value(&mut iter, "--display")?;
                let assignment = parse_display_assignment(&value)?;
                assignment.apply_to(&mut options.display);
            }
            "--monitor" | "-m" => {
                options
                    .monitor_ports
                    .push(parse_send_monitor_binding(&next_value(
                        &mut iter,
                        "--monitor",
                    )?)?)
            }
            "--interactive" | "-i" => options.interactive = true,
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

fn parse_send_monitor_binding(value: &str) -> Result<SendMonitorBinding, String> {
    if value.is_empty() {
        return Err(String::from("send monitor binding must not be empty"));
    }

    let (port_text, format) = if let Some((port_text, format_name)) = value.rsplit_once(',') {
        if OutputFormat::parse(format_name).is_ok() {
            (port_text, Some(format_name.to_owned()))
        } else {
            (value, None)
        }
    } else {
        (value, None)
    };

    let port_spec = parse_port_spec("--monitor", port_text)?;
    Ok(SendMonitorBinding {
        port: port_spec.port,
        baud: port_spec.baud,
        format,
        display_mode: port_spec.display_mode,
        line_break_mode: port_spec.line_break_mode,
    })
}

fn parse_send_output_binding(value: &str) -> Result<SendOutputBinding, String> {
    if value.is_empty() {
        return Err(String::from("send output binding must not be empty"));
    }

    let (binding_text, format) = if let Some((binding_text, format_name)) = value.rsplit_once(',') {
        if OutputFormat::parse(format_name).is_ok() {
            (binding_text, Some(format_name.to_owned()))
        } else {
            (value, None)
        }
    } else {
        (value, None)
    };

    let (id, port_text) = if let Some((id, port_text)) = binding_text.split_once('=') {
        if id.is_empty() || port_text.is_empty() {
            return Err(format!("invalid send output binding: {value}"));
        }
        (Some(id.to_owned()), port_text)
    } else {
        (None, binding_text)
    };

    let port_spec = parse_port_spec("send output binding", port_text)?;
    Ok(SendOutputBinding {
        id: id.unwrap_or_else(|| port_spec.port.clone()),
        port: port_spec.port,
        baud: port_spec.baud,
        format,
        display_mode: port_spec.display_mode,
        line_break_mode: port_spec.line_break_mode,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        parse_send_args, parse_send_monitor_binding, parse_send_output_binding,
        resolve_requested_port_spec,
    };
    use crate::app::cli::common::PortSpec;
    use crate::port_display::{LineBreakMode, PortDisplayMode};
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
        assert_eq!(options.monitor_ports.len(), 1);
        assert_eq!(options.monitor_ports[0].port, "/dev/ttyUSB1");
        assert_eq!(options.monitor_ports[0].baud, None);
        assert_eq!(options.monitor_ports[0].format, None);
    }

    #[test]
    fn parse_send_monitor_binding_accepts_format_and_packet_mode() {
        let binding =
            parse_send_monitor_binding("/dev/ttyUSB1@115200,utf8+line,packetjfv1").unwrap();

        assert_eq!(binding.port, "/dev/ttyUSB1");
        assert_eq!(binding.baud, Some(115_200));
        assert_eq!(binding.format.as_deref(), Some("packetjfv1"));
        assert_eq!(binding.display_mode, Some(PortDisplayMode::Utf8));
        assert_eq!(binding.line_break_mode, Some(LineBreakMode::Line));
    }

    #[test]
    fn parse_send_output_binding_accepts_id_format_and_packet_mode() {
        let binding =
            parse_send_output_binding("main=/dev/ttyUSB0@921600,hex+packet,packetacv6").unwrap();

        assert_eq!(binding.id, "main");
        assert_eq!(binding.port, "/dev/ttyUSB0");
        assert_eq!(binding.baud, Some(921_600));
        assert_eq!(binding.format.as_deref(), Some("packetacv6"));
        assert_eq!(binding.display_mode, Some(PortDisplayMode::Hex));
        assert_eq!(binding.line_break_mode, Some(LineBreakMode::Packet));
    }

    #[test]
    fn requested_port_falls_back_to_config_when_cli_port_is_empty() {
        assert_eq!(
            resolve_requested_port_spec(
                Some(PortSpec {
                    port: String::from(""),
                    baud: None,
                    display_mode: None,
                    line_break_mode: None,
                }),
                Some(PortSpec {
                    port: String::from("/dev/ttyUSB0"),
                    baud: Some(115_200),
                    display_mode: Some(PortDisplayMode::Hex),
                    line_break_mode: Some(LineBreakMode::Packet),
                }),
            ),
            Some(PortSpec {
                port: String::from("/dev/ttyUSB0"),
                baud: Some(115_200),
                display_mode: Some(PortDisplayMode::Hex),
                line_break_mode: Some(LineBreakMode::Packet),
            })
        );
        assert_eq!(
            resolve_requested_port_spec(
                None,
                Some(PortSpec {
                    port: String::from("   "),
                    baud: None,
                    display_mode: None,
                    line_break_mode: None,
                })
            ),
            None
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
}
