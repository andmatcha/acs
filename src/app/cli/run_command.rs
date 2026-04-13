use super::help::{is_help_flag, print_run_help};
use super::run_logger::RunLogger;
use super::runtime::{
    RunStateGuard, RuntimeRegistration, install_live_monitor_sigint_handler,
    request_live_monitor_stop, should_background_run, should_stop_running, spawn_background_run,
};
use crate::input::compact::{self, CompactReport};
use crate::input::ds4_hid::{self, InputReportEvent};
use crate::output::serial::{PortRxCallback, SerialConfig, SerialOutput};
use crate::output::{OutputDriver, OutputFormat};
use crate::ui::live_monitor::{DisplayMode, MonitorAction, MonitorFrame, MonitorUi};
use std::cell::RefCell;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

const MONITOR_IDLE_REFRESH_MILLIS: i32 = 50;

#[derive(Debug, Clone)]
struct RunCommandConfig {
    display_mode: DisplayMode,
    output_format: Option<OutputFormat>,
    serial: Option<SerialConfig>,
    log_file: Option<PathBuf>,
}

impl RunCommandConfig {
    fn effective_output_format(&self) -> Option<OutputFormat> {
        self.output_format
            .or(self.serial.as_ref().map(|_| OutputFormat::Arm9))
    }

    fn runtime_registration(&self) -> RuntimeRegistration {
        RuntimeRegistration {
            display_mode: self.display_mode,
            output_format: self.effective_output_format(),
            port: self.serial.as_ref().map(|serial| serial.port.clone()),
            baud_rate: self.serial.as_ref().map(|serial| serial.baud_rate),
            log_file: self
                .log_file
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }
}

struct RunOutput {
    format: OutputFormat,
    driver: Box<dyn OutputDriver>,
    serial: Option<SerialOutput>,
}

struct ProcessedOutput {
    bytes: Vec<u8>,
    status: &'static str,
}

impl RunOutput {
    fn open(
        config: &RunCommandConfig,
        port_rx_callback: Option<PortRxCallback>,
    ) -> Result<Option<Self>, String> {
        let Some(format) = config.effective_output_format() else {
            return Ok(None);
        };

        let serial = match config.serial.as_ref() {
            Some(serial_config) => Some(
                SerialOutput::open(serial_config, port_rx_callback).map_err(|error| {
                    format!(
                        "failed to open serial port {} at {} baud: {}",
                        serial_config.port, serial_config.baud_rate, error
                    )
                })?,
            ),
            None => None,
        };

        Ok(Some(Self {
            format,
            driver: format.create_driver(),
            serial,
        }))
    }

    fn process_compact_report(
        &mut self,
        compact_report: &CompactReport,
    ) -> Result<ProcessedOutput, String> {
        let bytes = self.driver.encode(compact_report)?;
        let status = if self.serial.is_some() {
            "sent"
        } else {
            "preview"
        };

        if let Some(serial) = self.serial.as_mut() {
            serial.write_bytes(&bytes).map_err(|error| {
                format!(
                    "failed to write {} output: {}",
                    self.driver.format_name(),
                    error
                )
            })?;
        }

        Ok(ProcessedOutput { bytes, status })
    }

    fn sync_port_rx_into_frame(
        &self,
        frame: &mut MonitorFrame,
        rx_text_pending: &mut String,
    ) -> bool {
        let Some(serial) = self.serial.as_ref() else {
            return false;
        };

        let received_chunks = serial.take_port_rx_chunks();
        let snapshot = serial.port_rx_snapshot();
        let mut changed = !received_chunks.is_empty()
            || frame.port_rx_state.as_deref() != Some(snapshot.status.as_str())
            || frame.port_rx_bytes != snapshot.bytes;
        for chunk in received_chunks {
            for line in consume_received_text(&chunk, rx_text_pending) {
                frame.push_rx_history(line);
                changed = true;
            }
        }
        frame.port_rx_state = Some(snapshot.status);
        frame.port_rx_bytes = snapshot.bytes;
        changed
    }
}

struct LiveMonitorSession {
    ui: MonitorUi,
    last_frame: MonitorFrame,
    output: Option<RunOutput>,
    logger: Option<Arc<RunLogger>>,
    paused: bool,
    rx_text_pending: String,
    render_error: Option<String>,
    effective_output_format: Option<OutputFormat>,
    serial_enabled: bool,
}

impl LiveMonitorSession {
    fn handle_input_report(&mut self, event: &InputReportEvent, compact_report: CompactReport) {
        self.poll_ui_action();
        if self.render_error.is_some() {
            return;
        }

        let hid_history = self.last_frame.hid_history.clone();
        let tx_history = self.last_frame.tx_history.clone();
        let rx_history = self.last_frame.rx_history.clone();
        self.last_frame = monitor_frame_from_event(event, compact_report);
        self.last_frame.hid_history = hid_history;
        self.last_frame.tx_history = tx_history;
        self.last_frame.rx_history = rx_history;
        self.last_frame
            .push_hid_history(self.last_frame.raw_report.clone());
        self.last_frame.output_format = self
            .effective_output_format
            .map(|format| String::from(format.as_str()));

        let output_status = match self.output.as_mut() {
            Some(output) => match output.process_compact_report(&compact_report) {
                Ok(processed) => {
                    self.last_frame.output_format = Some(String::from(output.format.as_str()));
                    self.last_frame.output_bytes = processed.bytes;
                    self.last_frame.output_state = Some(String::from(processed.status));
                    self.last_frame
                        .push_tx_history(self.last_frame.output_bytes.clone());
                    if let Some(logger) = self.logger.as_ref() {
                        let _ = logger.log_output_bytes(
                            event.sequence,
                            output.format.as_str(),
                            processed.status,
                            &self.last_frame.output_bytes,
                        );
                    }
                    None
                }
                Err(_) => {
                    self.last_frame.output_state = Some(String::from("error"));
                    if self.serial_enabled {
                        self.last_frame.port_rx_state = Some(String::from("error"));
                    }
                    Some(String::from("output error"))
                }
            },
            None => None,
        };

        if let Some(output) = self.output.as_ref() {
            output.sync_port_rx_into_frame(&mut self.last_frame, &mut self.rx_text_pending);
        }

        if self.paused {
            return;
        }

        if let Err(error) = self.ui.render(&self.last_frame, output_status.as_deref()) {
            self.render_error = Some(error.to_string());
        }
    }

    fn handle_idle_refresh(&mut self) {
        self.poll_ui_action();
        if self.render_error.is_some() {
            return;
        }

        let should_render = self.output.as_ref().is_some_and(|output| {
            output.sync_port_rx_into_frame(&mut self.last_frame, &mut self.rx_text_pending)
        });

        if self.paused {
            return;
        }

        if should_render && let Err(error) = self.ui.render(&self.last_frame, None) {
            self.render_error = Some(error.to_string());
        }
    }

    fn poll_ui_action(&mut self) {
        let action = match self.ui.poll_action() {
            Ok(action) => action,
            Err(error) => {
                self.render_error = Some(error.to_string());
                return;
            }
        };

        match action {
            Some(MonitorAction::TogglePause) => {
                self.paused = !self.paused;
                let status = if self.paused {
                    Some("paused (space: resume, q: quit)")
                } else {
                    Some("resumed")
                };
                if let Err(error) = self.ui.render(&self.last_frame, status) {
                    self.render_error = Some(error.to_string());
                }
            }
            Some(MonitorAction::Quit) => {
                request_live_monitor_stop();
            }
            None => {}
        }
    }
}

pub(crate) fn run_live_monitor(args: Vec<String>, bin_name: &str) -> ExitCode {
    let original_args = args.clone();
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_run_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let config = match parse_run_args(args) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            print_run_help(bin_name);
            return ExitCode::from(2);
        }
    };

    if should_background_run(config.display_mode) {
        return spawn_background_run(&original_args);
    }

    install_live_monitor_sigint_handler();
    let logger = match config
        .log_file
        .as_ref()
        .map(|path| RunLogger::open(path).map(Arc::new))
        .transpose()
    {
        Ok(logger) => logger,
        Err(error) => {
            eprintln!("failed to open log file: {error}");
            return ExitCode::from(1);
        }
    };
    let _run_state_guard = match RunStateGuard::acquire(&config.runtime_registration()) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("failed to prepare runtime state: {error}");
            return ExitCode::from(1);
        }
    };

    let output = match RunOutput::open(&config, logger_port_rx_callback(logger.as_ref())) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };

    if config.display_mode == DisplayMode::None {
        return run_output_only(output, logger);
    }

    if let Err(error) = ds4_hid::ensure_device_ready() {
        eprintln!("failed to run DS4 monitor: {error}");
        return ExitCode::from(1);
    }

    let (monitor_result, render_error) = {
        let mut ui = match MonitorUi::new(config.display_mode) {
            Ok(ui) => ui,
            Err(error) => {
                eprintln!("failed to initialize live monitor UI: {error}");
                return ExitCode::from(1);
            }
        };
        let mut last_frame = MonitorFrame::idle();
        last_frame.output_format = config
            .effective_output_format()
            .map(|format| String::from(format.as_str()));
        last_frame.output_state = match config.effective_output_format() {
            Some(_) if config.serial.is_some() => Some(String::from("idle")),
            Some(_) => Some(String::from("preview")),
            None => None,
        };
        last_frame.port_rx_state = config.serial.as_ref().map(|_| String::from("waiting"));

        if let Err(error) = ui.render(&last_frame, Some("waiting")) {
            eprintln!("failed to render live monitor UI: {error}");
            return ExitCode::from(1);
        }

        let session = RefCell::new(LiveMonitorSession {
            ui,
            last_frame,
            output,
            logger,
            paused: false,
            rx_text_pending: String::new(),
            render_error: None,
            effective_output_format: config.effective_output_format(),
            serial_enabled: config.serial.is_some(),
        });

        let monitor_result = ds4_hid::monitor_input_reports_with_idle_until(
            |event| {
                if let Some(logger) = session.borrow().logger.as_ref() {
                    let _ = logger.log_input_report(&event);
                }
                match compact::convert_input_report(&event.report) {
                    Ok(compact_report) => {
                        session
                            .borrow_mut()
                            .handle_input_report(&event, compact_report);
                    }
                    Err(_) => {
                        let mut session = session.borrow_mut();
                        let frame = session.last_frame.clone();
                        if let Err(render_issue) =
                            session.ui.render(&frame, Some("unsupported report"))
                        {
                            session.render_error = Some(render_issue.to_string());
                        }
                    }
                }
            },
            || {
                session.borrow_mut().handle_idle_refresh();
            },
            should_stop_running,
            MONITOR_IDLE_REFRESH_MILLIS,
        );

        let session = session.into_inner();
        (monitor_result, session.render_error)
    };

    if let Some(error) = render_error {
        eprintln!("failed to update live monitor UI: {error}");
        return ExitCode::from(1);
    }

    match monitor_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("failed to run DS4 monitor: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_output_only(mut output: Option<RunOutput>, logger: Option<Arc<RunLogger>>) -> ExitCode {
    let monitor_result = ds4_hid::monitor_input_reports_until(
        |event| {
            if let Some(logger) = logger.as_ref() {
                let _ = logger.log_input_report(&event);
            }
            match compact::convert_input_report(&event.report) {
                Ok(compact_report) => {
                    if let Some(output) = output.as_mut() {
                        match output.process_compact_report(&compact_report) {
                            Ok(processed) => {
                                if let Some(logger) = logger.as_ref() {
                                    let _ = logger.log_output_bytes(
                                        event.sequence,
                                        output.format.as_str(),
                                        processed.status,
                                        &processed.bytes,
                                    );
                                }
                            }
                            Err(error) => {
                                eprintln!("output error: {error}");
                            }
                        }
                    }
                }
                Err(_) => {}
            }
        },
        should_stop_running,
    );

    match monitor_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("failed to run DS4 output: {error}");
            ExitCode::from(1)
        }
    }
}

fn monitor_frame_from_event(
    event: &InputReportEvent,
    compact_report: CompactReport,
) -> MonitorFrame {
    MonitorFrame {
        sequence: event.sequence,
        transport: event.device.transport,
        report_len: event.report.len(),
        device_name: event
            .device
            .product_name
            .clone()
            .unwrap_or_else(|| String::from("unknown")),
        vendor_id: event.device.vendor_id,
        product_id: event.device.product_id,
        interface_number: event.device.interface_number,
        raw_report: event.report.clone(),
        compact: compact_report,
        output_format: None,
        output_state: None,
        output_bytes: Vec::new(),
        port_rx_state: None,
        port_rx_bytes: Vec::new(),
        hid_history: Vec::new(),
        tx_history: Vec::new(),
        rx_history: Vec::new(),
    }
}

fn normalize_received_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter_map(|ch| match ch {
            '\r' => None,
            '\n' | '\t' => Some(ch),
            _ if ch.is_control() => Some(' '),
            _ => Some(ch),
        })
        .collect()
}

fn consume_received_text(bytes: &[u8], pending: &mut String) -> Vec<String> {
    let mut lines = Vec::new();

    for ch in normalize_received_text(bytes).chars() {
        if ch == '\n' {
            let trimmed = pending.trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_owned());
            }
            pending.clear();
            continue;
        }

        pending.push(ch);
    }

    lines
}

fn parse_run_args(args: Vec<String>) -> Result<RunCommandConfig, String> {
    let mut display_mode = DisplayMode::Full;
    let mut output_format = None;
    let mut port = None;
    let mut baud_rate = None;
    let mut log_file = None;
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--monitor" | "-m" => {
                let value = iter
                    .next()
                    .ok_or_else(|| String::from("missing value for --monitor"))?;
                display_mode = DisplayMode::parse(&value)?;
            }
            "--format" | "-f" => {
                let value = iter
                    .next()
                    .ok_or_else(|| String::from("missing value for --format"))?;
                output_format = Some(OutputFormat::parse(&value)?);
            }
            "--port" | "-p" => {
                let value = iter
                    .next()
                    .ok_or_else(|| String::from("missing value for --port"))?;
                port = Some(value);
            }
            "--baud" | "-b" => {
                let value = iter
                    .next()
                    .ok_or_else(|| String::from("missing value for --baud"))?;
                let parsed = value
                    .parse::<u32>()
                    .map_err(|_| format!("invalid baud rate: {value}"))?;
                baud_rate = Some(parsed);
            }
            "--log-file" => {
                let value = iter
                    .next()
                    .ok_or_else(|| String::from("missing value for --log-file"))?;
                log_file = Some(PathBuf::from(value));
            }
            other => return Err(format!("unknown run option: {other}")),
        }
    }

    let serial = match (port, baud_rate) {
        (Some(port), Some(baud_rate)) => Some(SerialConfig { port, baud_rate }),
        (Some(_), None) => {
            return Err(String::from(
                "missing required option for serial output: --baud",
            ));
        }
        (None, Some(_)) => {
            return Err(String::from(
                "missing required option for serial output: --port",
            ));
        }
        (None, None) => None,
    };

    if display_mode == DisplayMode::None && serial.is_none() {
        return Err(String::from(
            "--monitor none requires serial output via --port and --baud",
        ));
    }

    Ok(RunCommandConfig {
        display_mode,
        output_format,
        serial,
        log_file,
    })
}

fn logger_port_rx_callback(logger: Option<&Arc<RunLogger>>) -> Option<PortRxCallback> {
    logger.map(|logger| {
        let logger = Arc::clone(logger);
        Arc::new(move |status: String, bytes: Vec<u8>| {
            let _ = logger.log_serial_rx(&status, &bytes);
        }) as PortRxCallback
    })
}

#[cfg(test)]
mod tests {
    use super::{consume_received_text, normalize_received_text, parse_run_args};
    use crate::output::OutputFormat;
    use crate::ui::live_monitor::DisplayMode;

    #[test]
    fn parse_run_args_defaults_to_monitor_only() {
        let config = parse_run_args(vec![]).expect("run config should parse");
        assert_eq!(config.display_mode, DisplayMode::Full);
        assert!(config.serial.is_none());
        assert_eq!(config.output_format, None);
        assert!(config.log_file.is_none());
    }

    #[test]
    fn parse_run_args_supports_arm9_output() {
        let config = parse_run_args(vec![
            String::from("-f"),
            String::from("arm9"),
            String::from("-p"),
            String::from("/dev/ttyUSB0"),
            String::from("-b"),
            String::from("115200"),
        ])
        .expect("run config should parse");

        assert_eq!(config.display_mode, DisplayMode::Full);
        assert_eq!(config.output_format, Some(OutputFormat::Arm9));
        let serial = config.serial.expect("serial config should exist");
        assert_eq!(serial.port, "/dev/ttyUSB0");
        assert_eq!(serial.baud_rate, 115200);
    }

    #[test]
    fn parse_run_args_supports_format_without_serial_output() {
        let config = parse_run_args(vec![String::from("--format"), String::from("arm9")])
            .expect("parse should succeed");
        assert_eq!(config.output_format, Some(OutputFormat::Arm9));
        assert!(config.serial.is_none());
    }

    #[test]
    fn parse_run_args_rejects_baud_without_port() {
        let error = parse_run_args(vec![String::from("--baud"), String::from("115200")])
            .expect_err("parse should fail");
        assert!(error.contains("--port"));
    }

    #[test]
    fn parse_run_args_supports_raw_display_mode() {
        let config = parse_run_args(vec![String::from("-m"), String::from("raw")])
            .expect("run config should parse");
        assert_eq!(config.display_mode, DisplayMode::Raw);
    }

    #[test]
    fn parse_run_args_supports_compact_display_mode() {
        let config = parse_run_args(vec![String::from("--monitor"), String::from("compact")])
            .expect("run config should parse");
        assert_eq!(config.display_mode, DisplayMode::Compact);
    }

    #[test]
    fn parse_run_args_supports_none_display_mode_with_serial_output() {
        let config = parse_run_args(vec![
            String::from("--monitor"),
            String::from("none"),
            String::from("--format"),
            String::from("arm9"),
            String::from("--port"),
            String::from("/dev/ttyUSB0"),
            String::from("--baud"),
            String::from("115200"),
        ])
        .expect("run config should parse");
        assert_eq!(config.display_mode, DisplayMode::None);
        assert!(config.serial.is_some());
    }

    #[test]
    fn parse_run_args_rejects_none_display_mode_without_serial_output() {
        let error = parse_run_args(vec![String::from("--monitor"), String::from("none")])
            .expect_err("parse should fail");
        assert!(error.contains("--monitor none requires serial output"));
    }

    #[test]
    fn parse_run_args_supports_log_file() {
        let config = parse_run_args(vec![
            String::from("--log-file"),
            String::from("logs/ds4.log"),
        ])
        .expect("run config should parse");
        assert_eq!(
            config.log_file,
            Some(std::path::PathBuf::from("logs/ds4.log"))
        );
    }

    #[test]
    fn display_mode_parse_supports_graphic() {
        assert_eq!(
            DisplayMode::parse("graphic").expect("display mode should parse"),
            DisplayMode::Full
        );
    }

    #[test]
    fn help_flags_are_detected() {
        assert!(super::is_help_flag("--help"));
        assert!(super::is_help_flag("-h"));
        assert!(!super::is_help_flag("--monitor"));
    }

    #[test]
    fn normalize_received_text_renders_plain_text() {
        assert_eq!(normalize_received_text(b"OK\r\nREADY"), "OK\nREADY");
    }

    #[test]
    fn normalize_received_text_replaces_non_text_control_chars() {
        assert_eq!(normalize_received_text(b"A\x01B"), "A B");
    }

    #[test]
    fn consume_received_text_keeps_partial_line_until_next_chunk() {
        let mut pending = String::new();
        assert!(consume_received_text(b"RE", &mut pending).is_empty());
        assert_eq!(pending, "RE");

        let lines = consume_received_text(b"ADY\nOK\n", &mut pending);
        assert_eq!(lines, vec![String::from("READY"), String::from("OK")]);
        assert!(pending.is_empty());
    }

    #[test]
    fn consume_received_text_skips_empty_lines() {
        let mut pending = String::new();
        let lines = consume_received_text(b"\nA\n\n", &mut pending);
        assert_eq!(lines, vec![String::from("A")]);
        assert!(pending.is_empty());
    }
}
