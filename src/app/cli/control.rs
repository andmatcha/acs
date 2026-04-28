use super::common::{
    PortSpec, default_baud_rate, default_log_dir, next_value, parse_key_value_args,
    parse_port_spec, parse_u32_arg,
};
use super::help::{is_help_flag, print_control_help};
use super::io::{
    ObservedInput, format_input_display_value, format_io_input_binding, parse_output_format_list,
    prompt_display_mode, prompt_input_format, prompt_output_format, prompt_serial_port,
    prompt_u32_choice, resolve_display_mode, resolve_input_line_break_mode, shell_quote_arg,
};
use super::signal;
use crate::ingress::IngressFrame;
use crate::input::ds4_hid::{Ds4Controller, Ds4DeviceInfo};
use crate::output::OutputFormat;
use crate::pipeline::{
    ClassifyModuleConfig, FilterModuleConfig, PipelineDefinition, PipelineEngine, PipelineSpec,
    RouterModuleConfig, TransformChainConfig, TransformModuleConfig,
};
use crate::port_display::{
    LineBreakMode, PortDisplayConfig, PortDisplayMode, parse_display_assignment,
};
use crate::serial;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const LOOP_INTERVAL: Duration = Duration::from_millis(20);
const CONTROLLER_POLL_MILLIS: i32 = 0;
const STATUS_INTERVAL: Duration = Duration::from_millis(200);
const DISPLAY_FLUSH_SLICE: usize = 32;
const DISPLAY_FLUSH_PACKET_BUDGET: usize = 256;

#[derive(Debug, Default)]
struct ControlCliOptions {
    port: Option<ControlPortArg>,
    baud: Option<u32>,
    controller: Option<String>,
    format: Option<String>,
    display: PortDisplayConfig,
    monitor_ports: Vec<ControlMonitorArg>,
    log_dir: Option<PathBuf>,
    no_log: bool,
    s3b: bool,
}

#[derive(Debug, Default)]
struct ControlRuntimeOptions {
    port: Option<PortSpec>,
    baud: Option<u32>,
    controller: Option<String>,
    format: Option<String>,
    display: PortDisplayConfig,
    monitor_ports: Vec<ControlMonitorBinding>,
    log_dir: Option<PathBuf>,
    no_log: bool,
    s3b: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ControlPortArg {
    Provided(String),
    Prompt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ControlMonitorArg {
    Provided(String),
    Prompt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ControlMonitorBinding {
    port: String,
    baud: Option<u32>,
    formats: Vec<String>,
    display_mode: Option<PortDisplayMode>,
    line_break_mode: Option<LineBreakMode>,
}

struct ControlSettings {
    inputs: Vec<SessionInputSpec>,
    output: SessionOutputSpec,
    observed_inputs: Vec<ControlObservedInputSpec>,
    header_monitors: Vec<ControlHeaderMonitor>,
    controller: Option<String>,
    format: OutputFormat,
    log_dir: PathBuf,
    logging_enabled: bool,
    s3b: bool,
    executed_command: Option<String>,
}

struct ControlRunResult {
    logging_enabled: bool,
    log_path: PathBuf,
    executed_command: Option<String>,
}

#[derive(Debug, Clone)]
struct ControlObservedInputSpec {
    input_id: String,
    port: String,
    formats: Vec<OutputFormat>,
    per_format_display_modes: Option<BTreeMap<OutputFormat, PortDisplayMode>>,
}

#[derive(Debug, Clone)]
struct ControlHeaderMonitor {
    port: String,
    baud_rate: u32,
    formats: Vec<OutputFormat>,
}

struct ControlCommandMonitor {
    port: String,
    baud_rate: u32,
    display_mode: PortDisplayMode,
    line_break_mode: LineBreakMode,
    formats: Vec<OutputFormat>,
}

struct ControlRuntimeState {
    logging_enabled: bool,
    log_path_display: String,
    controller_line: String,
    output_line: String,
    monitors: Vec<ControlHeaderMonitor>,
    observed_inputs: BTreeMap<String, ObservedInput>,
    last_status_update: Instant,
    header_lines: Vec<String>,
}

impl ControlRuntimeState {
    fn new(
        settings: &ControlSettings,
        controller_info: &Ds4DeviceInfo,
        log_path_display: String,
        started_at: Instant,
    ) -> Self {
        let observed_inputs = settings
            .observed_inputs
            .iter()
            .cloned()
            .map(|spec| {
                (
                    spec.input_id.clone(),
                    ObservedInput::new(
                        spec.input_id,
                        spec.port,
                        spec.formats,
                        spec.per_format_display_modes,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();

        Self {
            logging_enabled: settings.logging_enabled,
            log_path_display,
            controller_line: format!(
                "controller: {} ({})",
                controller_info
                    .product_name
                    .as_deref()
                    .unwrap_or("unknown controller"),
                controller_info.path
            ),
            output_line: format!(
                "output: {} @ {} baud, format={}",
                settings.output.port,
                settings.output.baud_rate,
                settings.format.as_str()
            ),
            monitors: settings.header_monitors.clone(),
            observed_inputs,
            last_status_update: started_at
                .checked_sub(STATUS_INTERVAL)
                .unwrap_or(started_at),
            header_lines: Vec::new(),
        }
    }

    fn configure_session(&self, session: &mut SessionRuntime) {
        for input in self.observed_inputs.values() {
            session.set_manual_input_recording(&input.input_id, true);
            session.set_input_packet_rate_enabled(&input.port, true);
            session.set_input_known_formats(
                &input.port,
                input
                    .known_formats()
                    .iter()
                    .map(|format| format.display_name().to_owned())
                    .collect(),
            );
        }
    }

    fn handle_input(
        &mut self,
        frame: &IngressFrame,
        session: &mut SessionRuntime,
    ) -> Result<(), String> {
        let Some(input) = self.observed_inputs.get_mut(&frame.input_id) else {
            return Ok(());
        };

        let batch = input.observe(&frame.bytes, Instant::now());
        if batch.valid_packet_count > 0 {
            session.record_input_sample(
                &input.port,
                batch.valid_byte_len,
                batch.valid_packet_count,
            );
            for (format, (byte_len, packet_count)) in batch.per_format_totals {
                session.record_input_format_sample(
                    &input.port,
                    format.display_name(),
                    byte_len,
                    packet_count,
                );
            }
        }
        Ok(())
    }

    fn on_tick(&mut self, session: &mut SessionRuntime) -> Result<(), String> {
        self.flush_display_queues(session)?;

        let now = Instant::now();
        if now.duration_since(self.last_status_update) >= STATUS_INTERVAL {
            self.last_status_update = now;
            let header_lines = self.build_header_lines(now);
            if header_lines != self.header_lines {
                self.header_lines = header_lines.clone();
                session.set_header_lines(header_lines);
            }
        }

        Ok(())
    }

    fn flush_display_queues(&mut self, session: &mut SessionRuntime) -> Result<(), String> {
        let mut remaining_budget = DISPLAY_FLUSH_PACKET_BUDGET;

        while remaining_budget > 0 {
            let mut flushed_total = 0usize;
            for input in self.observed_inputs.values_mut() {
                if remaining_budget == 0 {
                    break;
                }
                let slice = remaining_budget.min(DISPLAY_FLUSH_SLICE);
                let flushed = input.flush_display_batch(session, slice)?;
                remaining_budget = remaining_budget.saturating_sub(flushed);
                flushed_total += flushed;
            }

            if flushed_total == 0 {
                break;
            }
        }

        Ok(())
    }

    fn build_header_lines(&mut self, now: Instant) -> Vec<String> {
        let mut lines = vec![self.controller_line.clone(), self.output_line.clone()];
        for monitor in self.monitors.clone() {
            let formats = format_monitor_formats(&monitor.formats);
            let mut line = format!(
                "monitor: {} @ {} baud, rx_format={formats}",
                monitor.port, monitor.baud_rate
            );
            let rx_rates = monitor
                .formats
                .iter()
                .filter_map(|format| {
                    self.rx_rate_hz(&monitor.port, *format, now)
                        .map(|rate_hz| format!("{}={rate_hz:.1} Hz", format.as_str()))
                })
                .collect::<Vec<_>>();
            if !rx_rates.is_empty() {
                line.push_str(", ");
                line.push_str(&rx_rates.join(", "));
            }
            lines.push(line);
        }

        if self.logging_enabled {
            lines.push(format!("log: {}", self.log_path_display));
        } else {
            lines.push(String::from("log: disabled (--no-log)"));
        }
        lines.push(String::from("Space で表示を一時停止/再開  Ctrl-C で終了"));
        lines
    }

    fn rx_rate_hz(&mut self, port: &str, format: OutputFormat, now: Instant) -> Option<f64> {
        self.observed_inputs
            .values_mut()
            .find(|input| input.port == port)
            .and_then(|input| input.packet_rate_hz(format, now))
    }
}

fn format_monitor_formats(formats: &[OutputFormat]) -> String {
    if formats.is_empty() {
        return String::from("raw");
    }

    formats
        .iter()
        .map(|format| format.as_str())
        .collect::<Vec<_>>()
        .join("+")
}

fn extend_unique_formats(target: &mut Vec<OutputFormat>, values: &[OutputFormat]) {
    for value in values {
        if !target.contains(value) {
            target.push(*value);
        }
    }
}

fn build_control_executed_command(
    output: &SessionOutputSpec,
    format: OutputFormat,
    monitors: &[ControlCommandMonitor],
    controller: Option<&str>,
    log_dir: Option<&PathBuf>,
    no_log: bool,
    s3b: bool,
) -> String {
    let mut args = vec![String::from("acs"), String::from("control")];

    args.push(String::from("-p"));
    args.push(format_control_port_binding(
        &output.port,
        output.baud_rate,
        Some(output.display_mode),
    ));
    args.push(String::from("--format"));
    args.push(format.as_str().to_owned());

    if let Some(controller) = controller {
        args.push(String::from("--controller"));
        args.push(controller.to_owned());
    }

    for monitor in monitors {
        args.push(String::from("-m"));
        args.push(format_io_input_binding(
            &monitor.port,
            monitor.baud_rate,
            Some(&format_input_display_value(
                monitor.display_mode,
                monitor.line_break_mode,
            )),
            format_monitor_format_arg(&monitor.formats).as_deref(),
        ));
    }

    if let Some(log_dir) = log_dir {
        args.push(String::from("--log-dir"));
        args.push(log_dir.display().to_string());
    }
    if no_log {
        args.push(String::from("--no-log"));
    }
    if s3b {
        args.push(String::from("--s3b"));
    }

    args.iter()
        .map(|arg| shell_quote_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_control_port_binding(
    port: &str,
    baud: u32,
    display_mode: Option<PortDisplayMode>,
) -> String {
    let mut value = format!("{port}@{baud}");
    if let Some(display_mode) = display_mode {
        value.push(',');
        value.push_str(super::io::display_mode_value(display_mode));
    }
    value
}

fn format_monitor_format_arg(formats: &[OutputFormat]) -> Option<String> {
    (!formats.is_empty()).then(|| {
        formats
            .iter()
            .map(|format| format.as_str())
            .collect::<Vec<_>>()
            .join("+")
    })
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

    let runtime_options = match resolve_control_options(cli_options) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    match run_with_options(runtime_options) {
        Ok(result) => {
            if result.logging_enabled {
                println!("log saved to {}", result.log_path.display());
            }
            if let Some(command) = &result.executed_command {
                println!("実行コマンド: {command}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run_with_options(cli_options: ControlRuntimeOptions) -> Result<ControlRunResult, String> {
    let settings = build_settings(cli_options)?;

    let mut controller = Ds4Controller::open(settings.controller.as_deref())
        .map_err(|error| format!("failed to open controller: {error}"))?;
    let controller_info = controller.info().clone();
    let controller_input_id = String::from("ds4_main");
    let mut engine = PipelineEngine::new(&build_control_pipeline_spec(
        &controller_input_id,
        settings.format,
    ))?;
    let started_at = Instant::now();
    let mut session = SessionRuntime::new(SessionSpec {
        title: String::from("acs control"),
        command_name: String::from("control"),
        log_dir: settings.log_dir.clone(),
        logging_enabled: settings.logging_enabled,
        xbee_s3b_recovery: settings.s3b,
        inputs: settings.inputs.clone(),
        outputs: vec![settings.output.clone()],
    })?;
    let log_path = session.log_path().to_path_buf();
    let log_path_display = log_path.display().to_string();
    let runtime_state = RefCell::new(ControlRuntimeState::new(
        &settings,
        &controller_info,
        log_path_display,
        started_at,
    ));
    runtime_state.borrow().configure_session(&mut session);
    let initial_header_lines = runtime_state.borrow_mut().build_header_lines(started_at);
    session.set_header_lines(initial_header_lines);

    signal::install_handler();
    session.run_loop_with_tick(
        LOOP_INTERVAL,
        signal::is_stop_requested,
        |frame, session| runtime_state.borrow_mut().handle_input(frame, session),
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
            runtime_state.borrow_mut().on_tick(session)?;
            Ok(())
        },
    )?;

    Ok(ControlRunResult {
        logging_enabled: settings.logging_enabled,
        log_path,
        executed_command: settings.executed_command,
    })
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

fn build_settings(cli_options: ControlRuntimeOptions) -> Result<ControlSettings, String> {
    let selected_port = cli_options.port.and_then(PortSpec::normalized);
    let default_baud = cli_options.baud.unwrap_or_else(default_baud_rate);
    let controller = cli_options.controller;
    let format_name = cli_options
        .format
        .unwrap_or_else(|| String::from("packetacv6"));
    let format = OutputFormat::parse(&format_name)?;
    let display = cli_options.display;
    let monitor_port_specs = cli_options.monitor_ports;
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
    let output_display_mode = selected_port
        .as_ref()
        .and_then(|port_spec| port_spec.display_mode)
        .or(display.resolve_output_override(&port))
        .unwrap_or(format.default_display_mode());
    let explicit_log_dir = cli_options.log_dir.clone();
    let log_dir = cli_options.log_dir.unwrap_or_else(default_log_dir);
    let mut inputs = Vec::<SessionInputSpec>::new();
    let mut header_monitors = Vec::<ControlHeaderMonitor>::new();
    let mut command_monitors = Vec::<ControlCommandMonitor>::new();
    let mut input_packet_format_candidates = BTreeMap::<String, Vec<OutputFormat>>::new();
    let mut input_display_is_explicit = BTreeMap::<String, bool>::new();

    for port_spec in monitor_port_specs {
        let monitor_port =
            serial::resolve_port(Some(&port_spec.port)).map_err(|error| error.to_string())?;
        let monitor_formats = port_spec
            .formats
            .iter()
            .map(|format| OutputFormat::parse(format))
            .collect::<Result<Vec<_>, _>>()?;
        let monitor_baud = port_spec.baud.unwrap_or(default_baud);
        if monitor_port == port && monitor_baud != output_baud {
            return Err(format!(
                "control monitor `{monitor_port}` must use the output baud rate {output_baud} when sharing the control output port"
            ));
        }

        let input_has_formats = !monitor_formats.is_empty();
        let display_mode = resolve_display_mode(
            port_spec.display_mode,
            display.resolve_input_override(&monitor_port),
            monitor_formats
                .first()
                .copied()
                .map(OutputFormat::default_display_mode),
        );
        let line_break_mode = resolve_input_line_break_mode(
            port_spec.line_break_mode,
            input_has_formats,
            display.resolve_line_break_input(&monitor_port),
        );

        if let Some(existing_input) = inputs.iter().find(|input| input.port == monitor_port) {
            if existing_input.baud_rate != monitor_baud {
                return Err(format!(
                    "control monitor `{monitor_port}` cannot use multiple baud rates ({} and {monitor_baud})",
                    existing_input.baud_rate
                ));
            }
        } else {
            inputs.push(SessionInputSpec {
                id: monitor_port.clone(),
                port: monitor_port.clone(),
                baud_rate: monitor_baud,
                display_mode,
                line_break_mode,
            });
            header_monitors.push(ControlHeaderMonitor {
                port: monitor_port.clone(),
                baud_rate: monitor_baud,
                formats: Vec::new(),
            });
            command_monitors.push(ControlCommandMonitor {
                port: monitor_port.clone(),
                baud_rate: monitor_baud,
                display_mode,
                line_break_mode,
                formats: Vec::new(),
            });
        }

        if let Some(header) = header_monitors
            .iter_mut()
            .find(|header| header.port == monitor_port)
        {
            extend_unique_formats(&mut header.formats, &monitor_formats);
        }
        if let Some(command) = command_monitors
            .iter_mut()
            .find(|command| command.port == monitor_port)
        {
            extend_unique_formats(&mut command.formats, &monitor_formats);
        }

        if input_has_formats {
            let input_display_override = port_spec
                .display_mode
                .or(display.resolve_input_override(&monitor_port));
            input_display_is_explicit
                .entry(monitor_port.clone())
                .and_modify(|explicit| *explicit |= input_display_override.is_some())
                .or_insert(input_display_override.is_some());
            let formats = input_packet_format_candidates
                .entry(monitor_port)
                .or_default();
            extend_unique_formats(formats, &monitor_formats);
        }
    }

    let observed_inputs = input_packet_format_candidates
        .into_iter()
        .filter_map(|(port, mut formats)| {
            if formats.is_empty() {
                return None;
            }
            formats.sort_by_key(|format| format.as_str());
            let explicit_input_display = input_display_is_explicit
                .get(&port)
                .copied()
                .unwrap_or(false);
            let per_format_display_modes = if explicit_input_display || formats.len() <= 1 {
                None
            } else {
                Some(
                    formats
                        .iter()
                        .map(|format| (*format, format.default_display_mode()))
                        .collect(),
                )
            };
            let input_id = inputs
                .iter()
                .find(|input| input.port == port)
                .map(|input| input.id.clone())?;
            Some(ControlObservedInputSpec {
                input_id,
                port,
                formats,
                per_format_display_modes,
            })
        })
        .collect();

    let output = SessionOutputSpec {
        id: String::from("main"),
        port: port.clone(),
        baud_rate: output_baud,
        format_name: format.as_str().to_owned(),
        display_mode: output_display_mode,
    };
    let executed_command = Some(build_control_executed_command(
        &output,
        format,
        &command_monitors,
        controller.as_deref(),
        explicit_log_dir.as_ref(),
        cli_options.no_log,
        cli_options.s3b,
    ));

    Ok(ControlSettings {
        inputs,
        output,
        observed_inputs,
        header_monitors,
        controller,
        format,
        log_dir,
        logging_enabled: !cli_options.no_log,
        s3b: cli_options.s3b,
        executed_command,
    })
}

fn parse_control_args(args: Vec<String>) -> Result<ControlCliOptions, String> {
    let mut options = ControlCliOptions::default();
    let mut iter = args.into_iter().peekable();

    while let Some(arg) = iter.next() {
        if let Some(value) = strip_control_value(&arg, &["-p", "--port"]) {
            options.port = Some(ControlPortArg::Provided(value));
            continue;
        }
        if let Some(value) = strip_control_value(&arg, &["-m", "--monitor"]) {
            options
                .monitor_ports
                .push(ControlMonitorArg::Provided(value));
            continue;
        }

        match arg.as_str() {
            "--port" | "-p" => {
                options.port = Some(
                    next_optional_control_binding(&mut iter)
                        .map_or(ControlPortArg::Prompt, ControlPortArg::Provided),
                );
            }
            "--config" => {
                apply_control_config_args(&mut options, &next_value(&mut iter, "--config")?)?
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
            "--monitor" | "-m" => options.monitor_ports.push(
                next_optional_control_binding(&mut iter)
                    .map_or(ControlMonitorArg::Prompt, ControlMonitorArg::Provided),
            ),
            "--log-dir" => {
                options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            "--no-log" => options.no_log = true,
            "--s3b" => options.s3b = true,
            other => return Err(format!("unknown option for control: {other}")),
        }
    }

    Ok(options)
}

fn strip_control_value(arg: &str, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| arg.strip_prefix(&format!("{name}=")))
        .map(str::to_owned)
}

fn next_optional_control_binding(
    iter: &mut std::iter::Peekable<impl Iterator<Item = String>>,
) -> Option<String> {
    match iter.peek() {
        Some(next) if !next.starts_with('-') => iter.next(),
        _ => None,
    }
}

fn resolve_control_options(
    cli_options: ControlCliOptions,
) -> Result<ControlRuntimeOptions, String> {
    let mut runtime_options = ControlRuntimeOptions {
        baud: cli_options.baud,
        controller: cli_options.controller,
        format: cli_options.format,
        display: cli_options.display,
        log_dir: cli_options.log_dir,
        no_log: cli_options.no_log,
        s3b: cli_options.s3b,
        ..ControlRuntimeOptions::default()
    };

    if let Some(port) = cli_options.port {
        match port {
            ControlPortArg::Provided(value) => {
                runtime_options.port = Some(parse_port_spec("--port", &value)?);
            }
            ControlPortArg::Prompt => {
                let (value, format) =
                    prompt_control_output_binding(runtime_options.format.is_none())?;
                runtime_options.port = Some(parse_port_spec("--port", &value)?);
                if runtime_options.format.is_none() {
                    runtime_options.format = format;
                }
            }
        }
    }

    for monitor in cli_options.monitor_ports {
        let value = match monitor {
            ControlMonitorArg::Provided(value) => value,
            ControlMonitorArg::Prompt => prompt_control_monitor_binding()?,
        };
        runtime_options
            .monitor_ports
            .push(parse_control_monitor_binding(&value)?);
    }

    Ok(runtime_options)
}

fn prompt_control_output_binding(prompt_format: bool) -> Result<(String, Option<String>), String> {
    let port = prompt_serial_port("送信ポートを選択")?;
    let baud = prompt_u32_choice(
        "送信ボーレート",
        default_baud_rate(),
        &[115_200, 921_600, 460_800, 230_400, 57_600, 38_400, 9_600],
    )?;
    let display = prompt_display_mode("送信表示形式", false)?;
    let format = if prompt_format {
        Some(prompt_output_format()?)
    } else {
        None
    };

    Ok((
        format_control_port_binding(&port, baud, display_mode_from_prompt(display.as_deref())?),
        format,
    ))
}

fn prompt_control_monitor_binding() -> Result<String, String> {
    let port = prompt_serial_port("受信ポートを選択")?;
    let baud = prompt_u32_choice(
        "受信ボーレート",
        default_baud_rate(),
        &[115_200, 921_600, 460_800, 230_400, 57_600, 38_400, 9_600],
    )?;
    let format = prompt_input_format()?;
    let display = prompt_display_mode("受信表示形式", true)?;

    Ok(format_io_input_binding(
        &port,
        baud,
        display.as_deref(),
        format.as_deref(),
    ))
}

fn display_mode_from_prompt(value: Option<&str>) -> Result<Option<PortDisplayMode>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let (display_mode, _) = crate::port_display::parse_display_value(value)?;
    Ok(display_mode)
}

fn parse_control_monitor_binding(value: &str) -> Result<ControlMonitorBinding, String> {
    if value.is_empty() {
        return Err(String::from("control monitor binding must not be empty"));
    }

    let (port_text, formats) = if let Some((port_text, format_names)) = value.rsplit_once(',') {
        if let Some(formats) = parse_output_format_list(format_names) {
            (port_text, formats)
        } else {
            (value, Vec::new())
        }
    } else {
        (value, Vec::new())
    };

    let port_spec = parse_port_spec("control monitor binding", port_text)?;
    Ok(ControlMonitorBinding {
        port: port_spec.port,
        baud: port_spec.baud,
        formats,
        display_mode: port_spec.display_mode,
        line_break_mode: port_spec.line_break_mode,
    })
}

fn apply_control_config_args(options: &mut ControlCliOptions, value: &str) -> Result<(), String> {
    for assignment in parse_key_value_args("--config", value)? {
        let key = assignment.key.to_ascii_uppercase();
        match key.as_str() {
            "CONTROLLER" => options.controller = Some(assignment.value),
            "FORMAT" => options.format = Some(assignment.value),
            "DISPLAY" => {
                let display = parse_display_assignment(&assignment.value)?;
                display.apply_to(&mut options.display);
            }
            "LOG_DIR" => options.log_dir = Some(PathBuf::from(assignment.value)),
            other => return Err(format!("unknown control config key: {other}")),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ControlMonitorArg, ControlPortArg, build_control_pipeline_spec, parse_control_args,
        parse_control_monitor_binding,
    };
    use crate::output::OutputFormat;
    use crate::pipeline::PipelineEngine;
    use crate::port_display::{LineBreakMode, PortDisplayMode};
    use std::path::PathBuf;

    #[test]
    fn parse_control_args_accepts_config_and_no_log() {
        let options = parse_control_args(vec![
            String::from("--port"),
            String::from("/dev/ttyUSB0@921600"),
            String::from("--config"),
            String::from("FORMAT=PacketACv6,CONTROLLER=0,DISPLAY=input:default=utf8+packet,LOG_DIR=tmp/control-logs"),
            String::from("--monitor"),
            String::from("/dev/ttyUSB1@115200,hex"),
            String::from("--no-log"),
        ])
        .expect("should parse");

        assert_eq!(
            options.port,
            Some(ControlPortArg::Provided(String::from(
                "/dev/ttyUSB0@921600"
            )))
        );
        assert_eq!(options.controller.as_deref(), Some("0"));
        assert_eq!(options.format.as_deref(), Some("PacketACv6"));
        assert_eq!(options.monitor_ports.len(), 1);
        assert_eq!(
            options.monitor_ports[0],
            ControlMonitorArg::Provided(String::from("/dev/ttyUSB1@115200,hex"))
        );
        assert_eq!(options.log_dir, Some(PathBuf::from("tmp/control-logs")));
        assert!(options.no_log);
    }

    #[test]
    fn parse_control_args_accepts_bare_port_and_monitor_for_prompting() {
        let options =
            parse_control_args(vec![String::from("-p"), String::from("-m")]).expect("should parse");

        assert_eq!(options.port, Some(ControlPortArg::Prompt));
        assert_eq!(options.monitor_ports, vec![ControlMonitorArg::Prompt]);
    }

    #[test]
    fn parse_control_monitor_binding_accepts_format_and_packet_mode() {
        let binding =
            parse_control_monitor_binding("/dev/ttyUSB1@115200,utf8+packet,packetjfv1").unwrap();

        assert_eq!(binding.port, "/dev/ttyUSB1");
        assert_eq!(binding.baud, Some(115_200));
        assert_eq!(binding.formats, vec![String::from("packetjfv1")]);
        assert_eq!(binding.display_mode, Some(PortDisplayMode::Utf8));
        assert_eq!(binding.line_break_mode, Some(LineBreakMode::Packet));
    }

    #[test]
    fn parse_control_args_accepts_packetmv1_format() {
        let options = parse_control_args(vec![
            String::from("--config"),
            String::from("FORMAT=PacketMv1"),
        ])
        .expect("should parse");

        assert_eq!(options.format.as_deref(), Some("PacketMv1"));
    }

    #[test]
    fn control_pipeline_accepts_packetmv1_format() {
        let spec = build_control_pipeline_spec("ds4_main", OutputFormat::PacketMv1);

        PipelineEngine::new(&spec).expect("PacketMv1 should build a control pipeline");
    }
}
