use crate::port_display::{LineBreakMode, PortDisplayMode};
use crate::serial::SerialLineBuffer;
use crate::session::logger::CommandLogger;
use crate::ui::text_dashboard::{TextDashboard, TextDashboardAction};
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) struct SessionDashboard {
    dashboard: TextDashboard,
    logger: CommandLogger,
    line_buffer: SerialLineBuffer,
    input_line_break_modes: BTreeMap<String, LineBreakMode>,
    paused: bool,
    status: String,
}

impl SessionDashboard {
    pub(crate) fn new(
        title: impl Into<String>,
        command_name: &str,
        log_dir: &Path,
        logging_enabled: bool,
    ) -> Result<Self, String> {
        let logger = if logging_enabled {
            CommandLogger::create(command_name, log_dir)
                .map_err(|error| format!("failed to create log file: {error}"))?
        } else {
            CommandLogger::disabled(command_name)
        };
        let dashboard = TextDashboard::new(title)
            .map_err(|error| format!("failed to initialize dashboard: {error}"))?;

        Ok(Self {
            dashboard,
            logger,
            line_buffer: SerialLineBuffer::default(),
            input_line_break_modes: BTreeMap::new(),
            paused: false,
            status: String::from("running"),
        })
    }

    pub(crate) fn log_path(&self) -> &Path {
        self.logger.path()
    }

    pub(crate) fn set_header_lines(&mut self, lines: Vec<String>) {
        self.dashboard.set_header_lines(lines);
    }

    pub(crate) fn configure_input_port(
        &mut self,
        port: &str,
        baud_rate: u32,
        display_mode: PortDisplayMode,
        line_break_mode: LineBreakMode,
    ) {
        self.dashboard
            .set_input_status(port, format!("baud={baud_rate}"));
        self.dashboard.set_input_baud_rate(port, baud_rate);
        self.dashboard.set_input_display_mode(port, display_mode);
        self.input_line_break_modes
            .insert(port.to_string(), line_break_mode);
    }

    pub(crate) fn configure_output_port(
        &mut self,
        port: &str,
        baud_rate: u32,
        display_mode: PortDisplayMode,
    ) {
        self.dashboard
            .set_output_status(port, format!("baud={baud_rate}"));
        self.dashboard.set_output_baud_rate(port, baud_rate);
        self.dashboard.set_output_display_mode(port, display_mode);
    }

    pub(crate) fn configure_udp_output(
        &mut self,
        target: &str,
        destination: &str,
        display_mode: PortDisplayMode,
    ) {
        self.dashboard
            .set_output_status(target, format!("udp destination={destination}"));
        self.dashboard.set_output_display_mode(target, display_mode);
    }

    pub(crate) fn set_output_known_formats(&mut self, port: &str, formats: Vec<String>) {
        self.dashboard.set_output_known_formats(port, formats);
    }

    pub(crate) fn set_input_known_formats(&mut self, port: &str, formats: Vec<String>) {
        self.dashboard.set_input_known_formats(port, formats);
    }

    pub(crate) fn set_output_packet_rate_enabled(&mut self, port: &str, enabled: bool) {
        self.dashboard.set_output_packet_rate_enabled(port, enabled);
    }

    pub(crate) fn set_input_packet_rate_enabled(&mut self, port: &str, enabled: bool) {
        self.dashboard.set_input_packet_rate_enabled(port, enabled);
    }

    pub(crate) fn set_output_status(&mut self, port: &str, status: impl Into<String>) {
        self.dashboard.set_output_status(port, status);
    }

    pub(crate) fn set_input_status(&mut self, port: &str, status: impl Into<String>) {
        self.dashboard.set_input_status(port, status);
    }

    pub(crate) fn set_interactive_mode(&mut self, enabled: bool) {
        self.dashboard.set_interactive_mode(enabled);
    }

    pub(crate) fn handle_dashboard_action(&mut self) -> Result<Option<String>, String> {
        let action = self
            .dashboard
            .poll_action()
            .map_err(|error| format!("failed to read keyboard input: {error}"))?;

        match action {
            Some(TextDashboardAction::TogglePause) => {
                self.paused = !self.paused;
                self.status = String::from(if self.paused {
                    "paused (space: resume)"
                } else {
                    "running"
                });
                self.render()?;
            }
            Some(TextDashboardAction::InputChanged) => {
                self.render()?;
            }
            Some(TextDashboardAction::Submit(text)) => {
                self.render()?;
                return Ok(Some(text));
            }
            None => {}
        }

        Ok(None)
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused
    }

    pub(crate) fn record_output_with_options(
        &mut self,
        port: &str,
        bytes: &[u8],
        display_mode: Option<PortDisplayMode>,
        preserve_line_breaks: bool,
    ) -> Result<(), String> {
        self.dashboard.record_output_sample(port, bytes.len(), 1);
        self.dashboard
            .add_output_with_options(port, bytes, display_mode, preserve_line_breaks);
        self.logger
            .log_output(port, bytes)
            .map_err(|error| format!("failed to write log: {error}"))
    }

    pub(crate) fn record_input(&mut self, port: &str, bytes: &[u8]) -> Result<bool, String> {
        self.dashboard.record_input_bytes(port, bytes);

        let line_break = self
            .input_line_break_modes
            .get(port)
            .copied()
            .unwrap_or_default();

        match line_break {
            LineBreakMode::Packet => {
                self.dashboard.add_input(port, bytes);
                self.logger
                    .log_input(port, bytes)
                    .map_err(|error| format!("failed to write log: {error}"))?;
                return Ok(true);
            }
            LineBreakMode::Wrap => {
                self.dashboard.add_input_stream_wrapped(port, bytes, None);
                self.logger
                    .log_input(port, bytes)
                    .map_err(|error| format!("failed to write log: {error}"))?;
                return Ok(true);
            }
            LineBreakMode::Line | LineBreakMode::Crlf => {}
        }

        let mut changed = false;
        for line in self.line_buffer.push_chunk(port, bytes) {
            self.dashboard.add_input_with_options(
                port,
                &line,
                None,
                line_break.preserve_entry_line_breaks(),
            );
            self.logger
                .log_input(port, &line)
                .map_err(|error| format!("failed to write log: {error}"))?;
            changed = true;
        }

        Ok(changed)
    }

    pub(crate) fn record_input_error(&mut self, port: &str, message: &str) -> Result<bool, String> {
        self.set_input_status(port, format!("error: {message}"));
        self.logger
            .log_status(port, message)
            .map_err(|error| format!("failed to write log: {error}"))?;
        Ok(true)
    }

    pub(crate) fn record_input_sample(&mut self, port: &str, byte_len: usize, packet_count: usize) {
        self.dashboard
            .record_input_sample(port, byte_len, packet_count);
    }

    pub(crate) fn record_input_format_sample(
        &mut self,
        port: &str,
        format_name: &str,
        byte_len: usize,
        packet_count: usize,
    ) {
        self.dashboard
            .record_input_format_sample(port, format_name, byte_len, packet_count);
    }

    pub(crate) fn record_output_sample(
        &mut self,
        port: &str,
        byte_len: usize,
        packet_count: usize,
    ) {
        self.dashboard
            .record_output_sample(port, byte_len, packet_count);
    }

    pub(crate) fn record_output_format_sample(
        &mut self,
        port: &str,
        format_name: &str,
        byte_len: usize,
        packet_count: usize,
    ) {
        self.dashboard
            .record_output_format_sample(port, format_name, byte_len, packet_count);
    }

    pub(crate) fn add_input_entry(&mut self, port: &str, bytes: &[u8]) -> Result<(), String> {
        self.add_input_entry_with_options(port, bytes, None, false)
    }

    pub(crate) fn add_input_entry_with_options(
        &mut self,
        port: &str,
        bytes: &[u8],
        display_mode: Option<PortDisplayMode>,
        preserve_line_breaks: bool,
    ) -> Result<(), String> {
        self.dashboard
            .add_input_with_options(port, bytes, display_mode, preserve_line_breaks);
        self.logger
            .log_input(port, bytes)
            .map_err(|error| format!("failed to write log: {error}"))
    }

    pub(crate) fn add_output_entry(&mut self, port: &str, bytes: &[u8]) -> Result<(), String> {
        self.add_output_entry_with_options(port, bytes, None, false)
    }

    pub(crate) fn add_output_entry_with_options(
        &mut self,
        port: &str,
        bytes: &[u8],
        display_mode: Option<PortDisplayMode>,
        preserve_line_breaks: bool,
    ) -> Result<(), String> {
        self.dashboard
            .add_output_with_options(port, bytes, display_mode, preserve_line_breaks);
        self.logger
            .log_output(port, bytes)
            .map_err(|error| format!("failed to write log: {error}"))
    }

    pub(crate) fn flush_pending_input_lines(&mut self) -> Result<bool, String> {
        let mut changed = false;
        for line in self.line_buffer.drain_pending_lines() {
            let line_break = self
                .input_line_break_modes
                .get(&line.port)
                .copied()
                .unwrap_or_default();
            if matches!(line_break, LineBreakMode::Packet | LineBreakMode::Wrap) {
                continue;
            }
            self.dashboard.add_input_with_options(
                &line.port,
                &line.bytes,
                None,
                line_break.preserve_entry_line_breaks(),
            );
            self.logger
                .log_input(&line.port, &line.bytes)
                .map_err(|error| format!("failed to write log: {error}"))?;
            changed = true;
        }

        Ok(changed)
    }

    pub(crate) fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    pub(crate) fn render(&mut self) -> Result<(), String> {
        self.dashboard
            .render(Some(&self.status))
            .map_err(|error| format!("failed to render dashboard: {error}"))
    }
}
