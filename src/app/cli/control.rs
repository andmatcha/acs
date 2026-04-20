use super::common::{
    PortSpec, default_baud_rate, default_log_dir, next_value, parse_port_spec, parse_u32_arg,
    resolve_requested_port_spec,
};
use super::config;
use super::help::{is_help_flag, print_control_help};
use super::signal;
use crate::input::ds4_hid::Ds4Controller;
use crate::output::OutputFormat;
use crate::pipeline::{
    ClassifyModuleConfig, FilterModuleConfig, PipelineDefinition, PipelineEngine, PipelineSpec,
    RouterModuleConfig, TransformChainConfig, TransformModuleConfig,
};
use crate::port_display::{PortDisplayConfig, parse_display_assignment};
use crate::serial;
use crate::session::event::IngressFrame;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const LOOP_INTERVAL: Duration = Duration::from_millis(20);
const CONTROLLER_POLL_MILLIS: i32 = 0;

#[derive(Debug, Default)]
struct ControlCliOptions {
    port: Option<PortSpec>,
    baud: Option<u32>,
    controller: Option<String>,
    format: Option<String>,
    display: PortDisplayConfig,
    monitor_ports: Vec<PortSpec>,
    config_path: Option<PathBuf>,
    log_dir: Option<PathBuf>,
}

struct ControlSettings {
    inputs: Vec<SessionInputSpec>,
    output: SessionOutputSpec,
    controller: Option<String>,
    format: OutputFormat,
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
    let loaded_config = config::load_config_or_default(cli_options.config_path.as_deref())?;
    let settings = build_settings(cli_options, loaded_config.config, &loaded_config.lookup)?;

    let mut controller = Ds4Controller::open(settings.controller.as_deref())
        .map_err(|error| format!("failed to open controller: {error}"))?;
    let controller_info = controller.info().clone();
    let controller_input_id = String::from("ds4_main");
    let mut engine = PipelineEngine::new(&build_control_pipeline_spec(
        &controller_input_id,
        settings.format,
    ))?;
    let output_port = settings.output.port.clone();
    let output_baud = settings.output.baud_rate;
    let extra_monitor_ports = settings
        .inputs
        .iter()
        .filter(|input| input.port != output_port)
        .map(|input| format!("{}@{}", input.port, input.baud_rate))
        .collect::<Vec<_>>();
    let mut session = SessionRuntime::new(SessionSpec {
        title: String::from("acs control"),
        command_name: String::from("control"),
        log_dir: settings.log_dir.clone(),
        logging_enabled: true,
        inputs: settings.inputs.clone(),
        outputs: vec![settings.output.clone()],
    })?;
    let log_path = session.log_path().to_path_buf();
    let log_path_display = log_path.display().to_string();

    signal::install_handler();
    let mut header_lines = vec![
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
            output_port,
            output_baud,
            settings.format.as_str()
        ),
    ];
    if !extra_monitor_ports.is_empty() {
        header_lines.push(format!(
            "extra monitors: {}",
            extra_monitor_ports.join(", ")
        ));
    }
    header_lines.extend([
        format!("log: {log_path_display}"),
        String::from("Space で表示を一時停止/再開  Ctrl-C で終了"),
    ]);
    session.set_header_lines(header_lines);
    session.run_loop_with_tick(
        LOOP_INTERVAL,
        signal::is_stop_requested,
        |_, _| Ok(()),
        |session| {
            let maybe_report = match controller.read_next_report(CONTROLLER_POLL_MILLIS) {
                Ok(report) => report,
                Err(error) => {
                    session.set_status(format!("controller read error: {error}"));
                    return Ok(());
                }
            };

            if let Some(report) = maybe_report {
                let frame = IngressFrame {
                    input_id: controller_input_id.clone(),
                    bytes: report,
                };

                match engine.process_frame(&frame) {
                    Ok(dispatches) => {
                        for dispatch in dispatches {
                            match session.write_output(&dispatch.output_id, &dispatch.bytes) {
                                Ok(()) => {
                                    session.clear_output_error(&dispatch.output_id)?;
                                }
                                Err(error) => {
                                    session.set_output_error(&dispatch.output_id, &error)?;
                                }
                            }
                        }
                    }
                    Err(error) if error.starts_with("failed to convert DS4 report:") => {}
                    Err(error) => {
                        session.set_status(format!("pipeline error: {error}"));
                    }
                }
            }
            Ok(())
        },
    )?;

    Ok(log_path)
}

fn build_control_pipeline_spec(controller_input_id: &str, format: OutputFormat) -> PipelineSpec {
    PipelineSpec {
        pipelines: vec![PipelineDefinition {
            id: String::from("control_main"),
            inputs: vec![controller_input_id.to_owned()],
            filter: FilterModuleConfig::AllowAll,
            transform: TransformChainConfig {
                modules: control_transform_modules(format),
            },
            classify: ClassifyModuleConfig::None,
            router: RouterModuleConfig::Broadcast {
                outputs: vec![String::from("main")],
            },
        }],
    }
}

fn control_transform_modules(format: OutputFormat) -> Vec<TransformModuleConfig> {
    vec![
        TransformModuleConfig::Ds4ToCompact,
        TransformModuleConfig::OutputEncode { format },
    ]
}

fn build_settings(
    cli_options: ControlCliOptions,
    file_config: config::AppConfig,
    config_lookup: &super::paths::ConfigLookup,
) -> Result<ControlSettings, String> {
    let config = file_config.control;
    let using_cli_port = cli_options.port.is_some();
    let using_cli_monitor_ports = !cli_options.monitor_ports.is_empty();
    let selected_port = resolve_requested_port_spec(cli_options.port, config.port);
    let default_baud = cli_options
        .baud
        .or(config.baud)
        .unwrap_or_else(default_baud_rate);
    let controller = cli_options.controller.or(config.controller);
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
    if !using_cli_port
        && let Some(port_spec) = selected_port.as_ref()
        && let Some(mode) = port_spec.line_break_mode
    {
        display.set_line_break_for_stream(
            Some(crate::port_display::PortDisplayStream::Input),
            port_spec.port.clone(),
            mode,
        );
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
        display_mode: selected_port
            .as_ref()
            .and_then(|port_spec| port_spec.display_mode)
            .unwrap_or(display.resolve_input(&port)),
        line_break_mode: selected_port
            .as_ref()
            .and_then(|port_spec| port_spec.line_break_mode)
            .unwrap_or(display.resolve_line_break_input(&port)),
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
            line_break_mode: port_spec
                .line_break_mode
                .unwrap_or(display.resolve_line_break_input(&monitor_port)),
        });
    }

    Ok(ControlSettings {
        inputs,
        output: SessionOutputSpec {
            id: String::from("main"),
            port,
            baud_rate: output_baud,
            format_name: format.as_str().to_owned(),
            display_mode: output_display_mode,
        },
        controller,
        format,
        log_dir,
    })
}

fn parse_control_args(args: Vec<String>) -> Result<ControlCliOptions, String> {
    let mut options = ControlCliOptions::default();
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
            "--controller" | "-c" => {
                options.controller = Some(next_value(&mut iter, "--controller")?);
            }
            "--format" | "-f" => options.format = Some(next_value(&mut iter, "--format")?),
            "--display" => {
                let value = next_value(&mut iter, "--display")?;
                let assignment = parse_display_assignment(&value)?;
                assignment.apply_to(&mut options.display);
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
            other => return Err(format!("unknown option for control: {other}")),
        }
    }

    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::resolve_requested_port_spec;
    use crate::app::cli::common::PortSpec;
    use crate::port_display::{LineBreakMode, PortDisplayMode};

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
}
