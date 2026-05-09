use super::common::{
    PortSpec, default_baud_rate, default_log_dir, next_value, parse_key_value_args,
    parse_port_spec, parse_u32_arg,
};
use super::help::{is_help_flag, print_control_help};
use super::io::{
    IO_SEND_RATE_CHOICES, ObservedInput, choose_from_menu_with_preview, format_command_preview,
    format_input_display_value, format_io_input_binding, parse_output_format_list,
    prompt_display_mode_with_preview, prompt_input_format_with_preview,
    prompt_serial_port_with_preview, prompt_u32_choice_with_preview, resolve_display_mode,
    resolve_input_line_break_mode, shell_quote_arg,
};
use super::signal;
use crate::ingress::IngressFrame;
use crate::input::ds4_hid::{Ds4Controller, Ds4DeviceInfo, list_devices};
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

const SEND_LOOP_INTERVAL: Duration = Duration::from_millis(1);
const CONTROLLER_POLL_MILLIS: i32 = 0;
const STATUS_INTERVAL: Duration = Duration::from_millis(200);
const READ_USB_PULSE_DURATION: Duration = Duration::from_millis(250);
const DISPLAY_FLUSH_SLICE: usize = 32;
const DISPLAY_FLUSH_PACKET_BUDGET: usize = 256;

#[derive(Debug, Default)]
struct ControlCliOptions {
    port: Option<ControlPortArg>,
    baud: Option<u32>,
    controller: Option<String>,
    format: Option<ControlFormatArg>,
    rate_hz: Option<u32>,
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
    rate_hz: Option<u32>,
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
enum ControlFormatArg {
    Provided(String),
    Prompt,
}

#[derive(Debug, Clone)]
struct ControlPromptCommand {
    output: Option<String>,
    format: Option<String>,
    rate_hz: Option<u32>,
    controller: Option<String>,
    monitors: Vec<String>,
    log_dir: Option<PathBuf>,
    no_log: bool,
    s3b: bool,
}

#[derive(Debug, Clone, Copy)]
enum ControlPromptCandidate<'a> {
    Output(&'a str),
    Format(&'a str),
    Rate(&'a str),
    Monitor(&'a str),
}

impl ControlPromptCommand {
    fn new(options: &ControlRuntimeOptions) -> Self {
        Self {
            output: options
                .port
                .as_ref()
                .map(|port| format_control_port_spec_preview(port)),
            format: options.format.clone(),
            rate_hz: options.rate_hz,
            controller: options.controller.clone(),
            monitors: options
                .monitor_ports
                .iter()
                .map(format_control_monitor_binding_preview)
                .collect(),
            log_dir: options.log_dir.clone(),
            no_log: options.no_log,
            s3b: options.s3b,
        }
    }

    fn set_output(&mut self, output: String) {
        self.output = Some(output);
    }

    fn set_format(&mut self, format: String) {
        self.format = Some(format);
    }

    fn set_rate_hz(&mut self, rate_hz: u32) {
        self.rate_hz = Some(rate_hz);
    }

    fn push_monitor(&mut self, monitor: String) {
        self.monitors.push(monitor);
    }

    fn ensure_default_format(&mut self) {
        if self.format.is_none() {
            self.format = Some(String::from("packetacv6"));
        }
    }

    fn render(&self, candidate: Option<ControlPromptCandidate<'_>>) -> String {
        let mut args = vec![String::from("acs"), String::from("control")];

        if let Some(output) = candidate
            .and_then(|candidate| match candidate {
                ControlPromptCandidate::Output(output) => Some(output),
                _ => None,
            })
            .map(str::to_owned)
            .or_else(|| self.output.clone())
        {
            args.push(String::from("-p"));
            args.push(output);
        }

        if let Some(format) = candidate
            .and_then(|candidate| match candidate {
                ControlPromptCandidate::Format(format) => Some(format),
                _ => None,
            })
            .map(str::to_owned)
            .or_else(|| self.format.clone())
        {
            args.push(String::from("--format"));
            args.push(format);
        }

        if let Some(rate_hz) = candidate
            .and_then(|candidate| match candidate {
                ControlPromptCandidate::Rate(rate_hz) => Some(rate_hz),
                _ => None,
            })
            .map(str::to_owned)
            .or_else(|| self.rate_hz.map(|rate_hz| rate_hz.to_string()))
        {
            args.push(String::from("--rate"));
            args.push(rate_hz);
        }

        if let Some(controller) = self.controller.clone() {
            args.push(String::from("--controller"));
            args.push(controller);
        }

        for monitor in &self.monitors {
            args.push(String::from("-m"));
            args.push(monitor.clone());
        }
        if let Some(ControlPromptCandidate::Monitor(monitor)) = candidate {
            args.push(String::from("-m"));
            args.push(monitor.to_owned());
        }

        append_control_prompt_common_args(&mut args, self.log_dir.as_ref(), self.no_log, self.s3b);
        format_command_preview(&args)
    }
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
    rate_hz: u32,
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
    preserve_line_breaks: bool,
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
    read_usb_enabled: bool,
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
                        spec.preserve_line_breaks,
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
                "output: {} @ {} baud, format={}, tx_target={} Hz",
                settings.output.port,
                settings.output.baud_rate,
                settings.format.as_str(),
                settings.rate_hz
            ),
            read_usb_enabled: settings.format == OutputFormat::PacketAcV6,
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
        let controls = if self.read_usb_enabled {
            "Space で表示を一時停止/再開  R で READ USB  Ctrl-C で終了"
        } else {
            "Space で表示を一時停止/再開  Ctrl-C で終了"
        };
        lines.push(String::from(controls));
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

fn default_control_rate_hz(format: OutputFormat) -> u32 {
    match format {
        OutputFormat::PacketAcV6 | OutputFormat::PacketAcV6Usb | OutputFormat::PacketMv1 => 100,
        _ => 20,
    }
}

fn build_control_executed_command(
    output: &SessionOutputSpec,
    format: OutputFormat,
    rate_hz: u32,
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
    args.push(String::from("--rate"));
    args.push(rate_hz.to_string());

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

fn format_control_port_spec_preview(port: &PortSpec) -> String {
    format_prompt_control_port_binding(
        &port.port,
        port.baud.as_ref().map(ToString::to_string).as_deref(),
        port.display_mode.map(super::io::display_mode_value),
    )
}

fn format_prompt_control_port_binding(
    port: &str,
    baud: Option<&str>,
    display: Option<&str>,
) -> String {
    let mut value = port.to_owned();
    if let Some(baud) = baud {
        value.push('@');
        value.push_str(baud);
    }
    if let Some(display) = display {
        value.push(',');
        value.push_str(display);
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

fn format_control_monitor_binding_preview(monitor: &ControlMonitorBinding) -> String {
    let baud = monitor.baud.as_ref().map(ToString::to_string);
    let display = monitor.display_mode.map(super::io::display_mode_value);
    let input_display = match (display, monitor.line_break_mode) {
        (Some(display), Some(line_break)) => Some(format!(
            "{display}+{}",
            super::io::line_break_mode_value(line_break)
        )),
        (Some(display), None) => Some(display.to_owned()),
        (None, Some(line_break)) => Some(super::io::line_break_mode_value(line_break).to_owned()),
        (None, None) => None,
    };
    format_prompt_control_monitor_binding(
        &monitor.port,
        baud.as_deref(),
        input_display.as_deref(),
        (!monitor.formats.is_empty())
            .then(|| monitor.formats.join("+"))
            .as_deref(),
    )
}

fn format_prompt_control_monitor_binding(
    port: &str,
    baud: Option<&str>,
    display: Option<&str>,
    format: Option<&str>,
) -> String {
    let mut value = format_prompt_control_port_binding(port, baud, display);
    if let Some(format) = format {
        value.push(',');
        value.push_str(format);
    }
    value
}

fn append_control_prompt_common_args(
    args: &mut Vec<String>,
    log_dir: Option<&PathBuf>,
    no_log: bool,
    s3b: bool,
) {
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
                println!("Command:");
                println!("{command}");
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
        title: settings
            .executed_command
            .clone()
            .unwrap_or_else(|| String::from("acs control")),
        command_name: String::from("control"),
        log_dir: settings.log_dir.clone(),
        logging_enabled: settings.logging_enabled,
        xbee_s3b_recovery: settings.s3b,
        inputs: settings.inputs.clone(),
        outputs: vec![settings.output.clone()],
    })?;
    session.set_output_packet_rate_enabled(&settings.output.port, true);
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
    let send_period = Duration::from_secs_f64(1.0 / settings.rate_hz as f64);
    let mut next_send_at = started_at;
    let mut latest_controller_report = None::<Vec<u8>>;
    let mut read_usb_until = None::<Instant>;
    let mut read_usb_pending_packet = false;
    session.run_loop_with_tick(
        SEND_LOOP_INTERVAL,
        signal::is_stop_requested,
        |frame, session| runtime_state.borrow_mut().handle_input(frame, session),
        |session| {
            let now = Instant::now();
            if session.take_read_usb_request() {
                read_usb_until = Some(now + READ_USB_PULSE_DURATION);
                read_usb_pending_packet = true;
            }
            let read_usb_window_active = read_usb_until.is_some_and(|deadline| now < deadline);
            let read_usb_requested = read_usb_pending_packet || read_usb_window_active;

            let maybe_report = match controller.read_next_report(CONTROLLER_POLL_MILLIS) {
                Ok(report) => report,
                Err(error) => {
                    session.set_status(format!("controller read error: {error}"));
                    return Ok(());
                }
            };

            if let Some(report) = maybe_report {
                latest_controller_report = Some(report);
            }

            if now >= next_send_at {
                realign_control_send_schedule(&mut next_send_at, send_period, now);
                if let Some(report) = latest_controller_report.as_ref() {
                    dispatch_control_report(
                        &mut engine,
                        session,
                        &controller_input_id,
                        report,
                        settings.format,
                        read_usb_requested,
                    )?;
                    read_usb_pending_packet = false;
                }
                next_send_at += send_period;
            }
            if !read_usb_pending_packet && !read_usb_window_active {
                read_usb_until = None;
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

fn dispatch_control_report(
    engine: &mut PipelineEngine,
    session: &mut SessionRuntime,
    controller_input_id: &str,
    report: &[u8],
    format: OutputFormat,
    read_usb_requested: bool,
) -> Result<(), String> {
    let frame = IngressFrame {
        input_id: controller_input_id.to_owned(),
        bytes: report.to_vec(),
    };

    match engine.process_frame(&frame) {
        Ok(dispatches) => {
            for mut dispatch in dispatches {
                if read_usb_requested && format == OutputFormat::PacketAcV6 {
                    format.set_usb_read_flag(&mut dispatch.bytes)?;
                }
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

    Ok(())
}

fn realign_control_send_schedule(next_send_at: &mut Instant, period: Duration, now: Instant) {
    if now <= *next_send_at || period.is_zero() {
        return;
    }

    let overdue = now.duration_since(*next_send_at);
    if overdue < period {
        return;
    }

    let skipped_periods = (overdue.as_secs_f64() / period.as_secs_f64()).floor() as u32;
    if skipped_periods > 0 {
        if let Some(advance) = period.checked_mul(skipped_periods) {
            *next_send_at += advance;
        } else {
            *next_send_at = now;
        }
    }
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
    let requested_controller = cli_options.controller;
    let format_name = cli_options
        .format
        .unwrap_or_else(|| String::from("packetacv6"));
    let format = OutputFormat::parse(&format_name)?;
    let rate_hz = cli_options
        .rate_hz
        .unwrap_or_else(|| default_control_rate_hz(format));
    if rate_hz == 0 {
        return Err(String::from("control send rate must be greater than 0"));
    }
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
            display.resolve_line_break_input_override(&monitor_port),
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
            let preserve_line_breaks = inputs
                .iter()
                .find(|input| input.port == port)
                .map(|input| input.line_break_mode.preserve_entry_line_breaks())
                .unwrap_or(false);
            Some(ControlObservedInputSpec {
                input_id,
                port,
                formats,
                per_format_display_modes,
                preserve_line_breaks,
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
    let controller = resolve_control_controller(requested_controller, |controller| {
        build_control_executed_command(
            &output,
            format,
            rate_hz,
            &command_monitors,
            Some(controller),
            explicit_log_dir.as_ref(),
            cli_options.no_log,
            cli_options.s3b,
        )
    })?;
    let executed_command = Some(build_control_executed_command(
        &output,
        format,
        rate_hz,
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
        rate_hz,
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
        if let Some(value) = strip_control_value(&arg, &["-f", "--format"]) {
            options.format = Some(ControlFormatArg::Provided(value));
            continue;
        }
        if let Some(value) = strip_control_value(&arg, &["-r", "--rate"]) {
            options.rate_hz = Some(parse_u32_arg("--rate", &value)?);
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
            "--rate" | "-r" => {
                let value = next_value(&mut iter, "--rate")?;
                options.rate_hz = Some(parse_u32_arg("--rate", &value)?);
            }
            "--controller" | "-c" => {
                options.controller = Some(next_value(&mut iter, "--controller")?);
            }
            "--format" | "-f" => {
                options.format = Some(
                    next_optional_control_binding(&mut iter)
                        .map_or(ControlFormatArg::Prompt, ControlFormatArg::Provided),
                );
            }
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
        rate_hz: cli_options.rate_hz,
        display: cli_options.display,
        log_dir: cli_options.log_dir,
        no_log: cli_options.no_log,
        s3b: cli_options.s3b,
        ..ControlRuntimeOptions::default()
    };
    let mut prompt_command = ControlPromptCommand::new(&runtime_options);
    let mut prompt_rate = false;

    if let Some(format) = cli_options.format {
        let format = match format {
            ControlFormatArg::Provided(value) => value,
            ControlFormatArg::Prompt => prompt_control_output_format(&prompt_command)?,
        };
        prompt_command.set_format(format.clone());
        runtime_options.format = Some(format);
    }

    match cli_options.port {
        Some(ControlPortArg::Provided(value)) => {
            runtime_options.port = Some(parse_port_spec("--port", &value)?);
            prompt_command.set_output(value);
        }
        Some(ControlPortArg::Prompt) | None => {
            prompt_rate = true;
            let (value, format) =
                prompt_control_output_binding(&prompt_command, runtime_options.format.is_none())?;
            runtime_options.port = Some(parse_port_spec("--port", &value)?);
            prompt_command.set_output(value.clone());
            if runtime_options.format.is_none() {
                if let Some(format) = format {
                    prompt_command.set_format(format.clone());
                    runtime_options.format = Some(format);
                }
            }
        }
    }

    prompt_command.ensure_default_format();
    if runtime_options.rate_hz.is_none() && prompt_rate {
        let format = OutputFormat::parse(
            runtime_options
                .format
                .as_deref()
                .unwrap_or_else(|| prompt_command.format.as_deref().unwrap_or("packetacv6")),
        )?;
        let rate_hz = prompt_control_output_rate(&prompt_command, format)?;
        prompt_command.set_rate_hz(rate_hz);
        runtime_options.rate_hz = Some(rate_hz);
    }
    for monitor in cli_options.monitor_ports {
        let value = match monitor {
            ControlMonitorArg::Provided(value) => value,
            ControlMonitorArg::Prompt => prompt_control_monitor_binding(&prompt_command)?,
        };
        runtime_options
            .monitor_ports
            .push(parse_control_monitor_binding(&value)?);
        prompt_command.push_monitor(value);
    }

    Ok(runtime_options)
}

fn prompt_control_output_binding(
    command: &ControlPromptCommand,
    prompt_format: bool,
) -> Result<(String, Option<String>), String> {
    let port = prompt_serial_port_with_preview("送信ポートを選択", |port| {
        Some(command.render(Some(ControlPromptCandidate::Output(
            &format_prompt_control_port_binding(port, None, None),
        ))))
    })?;
    let baud = prompt_u32_choice_with_preview(
        "送信ボーレート",
        default_baud_rate(),
        &[115_200, 921_600, 460_800, 230_400, 57_600, 38_400, 9_600],
        |baud| {
            Some(command.render(Some(ControlPromptCandidate::Output(
                &format_prompt_control_port_binding(&port, Some(baud), None),
            ))))
        },
    )?;
    let baud_text = baud.to_string();
    let display = prompt_display_mode_with_preview("送信表示形式", false, |display| {
        Some(command.render(Some(ControlPromptCandidate::Output(
            &format_prompt_control_port_binding(&port, Some(&baud_text), Some(display)),
        ))))
    })?;
    let format = if prompt_format {
        let output_binding =
            format_prompt_control_port_binding(&port, Some(&baud_text), display.as_deref());
        Some(prompt_control_output_format_with_output(
            command,
            &output_binding,
        )?)
    } else {
        None
    };

    Ok((
        format_control_port_binding(&port, baud, display_mode_from_prompt(display.as_deref())?),
        format,
    ))
}

fn prompt_control_monitor_binding(command: &ControlPromptCommand) -> Result<String, String> {
    let port = prompt_serial_port_with_preview("受信ポートを選択", |port| {
        Some(command.render(Some(ControlPromptCandidate::Monitor(
            &format_prompt_control_monitor_binding(port, None, None, None),
        ))))
    })?;
    let baud = prompt_u32_choice_with_preview(
        "受信ボーレート",
        default_baud_rate(),
        &[115_200, 921_600, 460_800, 230_400, 57_600, 38_400, 9_600],
        |baud| {
            Some(command.render(Some(ControlPromptCandidate::Monitor(
                &format_prompt_control_monitor_binding(&port, Some(baud), None, None),
            ))))
        },
    )?;
    let baud_text = baud.to_string();
    let format = prompt_input_format_with_preview(|format| {
        Some(command.render(Some(ControlPromptCandidate::Monitor(
            &format_prompt_control_monitor_binding(&port, Some(&baud_text), None, format),
        ))))
    })?;
    let display = prompt_display_mode_with_preview("受信表示形式", true, |display| {
        Some(command.render(Some(ControlPromptCandidate::Monitor(
            &format_prompt_control_monitor_binding(
                &port,
                Some(&baud_text),
                Some(display),
                format.as_deref(),
            ),
        ))))
    })?;

    Ok(format_io_input_binding(
        &port,
        baud,
        display.as_deref(),
        format.as_deref(),
    ))
}

fn prompt_control_output_format(command: &ControlPromptCommand) -> Result<String, String> {
    prompt_control_output_format_with_preview(command, None)
}

fn prompt_control_output_format_with_output(
    command: &ControlPromptCommand,
    output_binding: &str,
) -> Result<String, String> {
    prompt_control_output_format_with_preview(command, Some(output_binding))
}

fn prompt_control_output_format_with_preview(
    command: &ControlPromptCommand,
    output_binding: Option<&str>,
) -> Result<String, String> {
    let formats = vec![
        OutputFormat::PacketAcV6,
        OutputFormat::PacketMv1,
        OutputFormat::PacketGcV1,
    ];
    let labels = formats
        .iter()
        .map(|format| format.display_name().to_owned())
        .collect::<Vec<_>>();
    let selected = choose_from_menu_with_preview("送信フォーマット", &labels, 0, |index| {
        let format = formats[index].as_str();
        let mut command = command.clone();
        if let Some(output_binding) = output_binding {
            command.set_output(output_binding.to_owned());
        }
        Some(command.render(Some(ControlPromptCandidate::Format(format))))
    })?;
    Ok(formats[selected].as_str().to_owned())
}

fn prompt_control_output_rate(
    command: &ControlPromptCommand,
    format: OutputFormat,
) -> Result<u32, String> {
    prompt_u32_choice_with_preview(
        "送信レート (Hz)",
        default_control_rate_hz(format),
        IO_SEND_RATE_CHOICES,
        |rate_hz| Some(command.render(Some(ControlPromptCandidate::Rate(rate_hz)))),
    )
}

fn resolve_control_controller<F>(
    controller: Option<String>,
    preview: F,
) -> Result<Option<String>, String>
where
    F: Fn(&str) -> String,
{
    if controller.is_some() {
        return Ok(controller);
    }

    let devices = list_devices().map_err(|error| format!("failed to list controllers: {error}"))?;
    match devices.as_slice() {
        [] | [_] => Ok(None),
        many => {
            let labels = many
                .iter()
                .enumerate()
                .map(|(index, device)| format_controller_choice(index, device))
                .collect::<Vec<_>>();
            let selected = choose_from_menu_with_preview(
                "コントローラーを選択",
                &labels,
                0,
                |index| Some(preview(&index.to_string())),
            )?;
            Ok(Some(selected.to_string()))
        }
    }
}

fn format_controller_choice(index: usize, device: &Ds4DeviceInfo) -> String {
    format!(
        "[{index}] transport={} vid=0x{:04x} pid=0x{:04x} interface={} product={} path={}",
        device.transport,
        device.vendor_id,
        device.product_id,
        device.interface_number,
        device.product_name.as_deref().unwrap_or("unknown"),
        device.path
    )
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
            "FORMAT" => options.format = Some(ControlFormatArg::Provided(assignment.value)),
            "RATE" => options.rate_hz = Some(parse_u32_arg("RATE", &assignment.value)?),
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
        ControlFormatArg, ControlMonitorArg, ControlPortArg, ControlPromptCandidate,
        ControlPromptCommand, ControlRuntimeOptions, build_control_pipeline_spec,
        default_control_rate_hz, parse_control_args, parse_control_monitor_binding,
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
            String::from("FORMAT=PacketACv6,RATE=100,CONTROLLER=0,DISPLAY=input:default=utf8+packet,LOG_DIR=tmp/control-logs"),
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
        assert_eq!(
            options.format,
            Some(ControlFormatArg::Provided(String::from("PacketACv6")))
        );
        assert_eq!(options.rate_hz, Some(100));
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
    fn parse_control_args_accepts_bare_format_for_prompting() {
        let options = parse_control_args(vec![String::from("-f")]).expect("should parse");

        assert_eq!(options.format, Some(ControlFormatArg::Prompt));
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
    fn parse_control_monitor_binding_accepts_ascii_crlf_mode() {
        let binding =
            parse_control_monitor_binding("/dev/ttyUSB1@115200,ascii+crlf,roverupgeneral").unwrap();

        assert_eq!(binding.port, "/dev/ttyUSB1");
        assert_eq!(binding.baud, Some(115_200));
        assert_eq!(binding.formats, vec![String::from("roverupgeneral")]);
        assert_eq!(binding.display_mode, Some(PortDisplayMode::Ascii));
        assert_eq!(binding.line_break_mode, Some(LineBreakMode::Crlf));
    }

    #[test]
    fn parse_control_args_accepts_packetmv1_format() {
        let options = parse_control_args(vec![
            String::from("--config"),
            String::from("FORMAT=PacketMv1"),
        ])
        .expect("should parse");

        assert_eq!(
            options.format,
            Some(ControlFormatArg::Provided(String::from("PacketMv1")))
        );
    }

    #[test]
    fn parse_control_args_accepts_rate_option() {
        let options = parse_control_args(vec![String::from("--rate"), String::from("20")])
            .expect("should parse");

        assert_eq!(options.rate_hz, Some(20));
    }

    #[test]
    fn default_control_rates_match_formats() {
        assert_eq!(default_control_rate_hz(OutputFormat::PacketAcV6), 100);
        assert_eq!(default_control_rate_hz(OutputFormat::PacketMv1), 100);
        assert_eq!(default_control_rate_hz(OutputFormat::PacketGcV1), 20);
    }

    #[test]
    fn control_prompt_command_renders_current_candidate() {
        let options = ControlRuntimeOptions {
            controller: Some(String::from("0")),
            log_dir: Some(PathBuf::from("tmp/control logs")),
            no_log: true,
            s3b: true,
            ..ControlRuntimeOptions::default()
        };
        let mut command = ControlPromptCommand::new(&options);
        command.set_output(String::from("/dev/ttyUSB0@921600,hex"));
        command.set_format(String::from("packetacv6"));

        assert_eq!(
            command.render(Some(ControlPromptCandidate::Monitor(
                "/dev/ttyUSB1@115200,utf8+packet,packetjfv1"
            ))),
            "acs control -p /dev/ttyUSB0@921600,hex --format packetacv6 --controller 0 -m /dev/ttyUSB1@115200,utf8+packet,packetjfv1 --log-dir 'tmp/control logs' --no-log --s3b"
        );
    }

    #[test]
    fn control_pipeline_accepts_packetmv1_format() {
        let spec = build_control_pipeline_spec("ds4_main", OutputFormat::PacketMv1);

        PipelineEngine::new(&spec).expect("PacketMv1 should build a control pipeline");
    }

    #[test]
    fn control_pipeline_accepts_packetgcv1_format() {
        let spec = build_control_pipeline_spec("ds4_main", OutputFormat::PacketGcV1);

        PipelineEngine::new(&spec).expect("PacketGCv1 should build a control pipeline");
    }
}
