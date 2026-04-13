use crate::port_display::PortDisplayMode;
use crate::serial::SerialLineBuffer;
use crate::session::logger::CommandLogger;
use crate::ui::text_dashboard::{TextDashboard, TextDashboardAction};
use std::path::Path;

pub(crate) struct SessionDashboard {
    dashboard: TextDashboard,
    logger: CommandLogger,
    line_buffer: SerialLineBuffer,
    raw_input: bool,
    paused: bool,
    status: String,
}

impl SessionDashboard {
    pub(crate) fn new(
        title: impl Into<String>,
        raw_input: bool,
        command_name: &str,
        log_dir: &Path,
    ) -> Result<Self, String> {
        let logger = CommandLogger::create(command_name, log_dir)
            .map_err(|error| format!("failed to create log file: {error}"))?;
        let dashboard = TextDashboard::new(title)
            .map_err(|error| format!("failed to initialize dashboard: {error}"))?;

        Ok(Self {
            dashboard,
            logger,
            line_buffer: SerialLineBuffer::default(),
            raw_input,
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
    ) {
        self.dashboard
            .set_input_status(port, format!("baud={baud_rate}"));
        self.dashboard.set_input_baud_rate(port, baud_rate);
        self.dashboard.set_input_display_mode(port, display_mode);
    }

    pub(crate) fn configure_output_port(
        &mut self,
        port: &str,
        baud_rate: u32,
        format_name: &str,
        display_mode: PortDisplayMode,
    ) {
        self.dashboard
            .set_output_status(port, format!("baud={baud_rate} format={format_name}"));
        self.dashboard.set_output_baud_rate(port, baud_rate);
        self.dashboard.set_output_display_mode(port, display_mode);
    }

    pub(crate) fn set_output_status(&mut self, port: &str, status: impl Into<String>) {
        self.dashboard.set_output_status(port, status);
    }

    pub(crate) fn set_input_status(&mut self, port: &str, status: impl Into<String>) {
        self.dashboard.set_input_status(port, status);
    }

    pub(crate) fn handle_dashboard_action(&mut self) -> Result<(), String> {
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
            None => {}
        }

        Ok(())
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.paused
    }

    pub(crate) fn record_output(&mut self, port: &str, bytes: &[u8]) -> Result<(), String> {
        self.dashboard.record_output_bytes(port, bytes);
        self.dashboard.add_output(port, bytes);
        self.logger
            .log_output(port, bytes)
            .map_err(|error| format!("failed to write log: {error}"))
    }

    pub(crate) fn record_input(&mut self, port: &str, bytes: &[u8]) -> Result<bool, String> {
        self.dashboard.record_input_bytes(port, bytes);

        if self.raw_input {
            self.dashboard.add_input(port, bytes);
            self.logger
                .log_input(port, bytes)
                .map_err(|error| format!("failed to write log: {error}"))?;
            return Ok(true);
        }

        let mut changed = false;
        for line in self.line_buffer.push_chunk(port, bytes) {
            self.dashboard.add_input(port, &line);
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

    pub(crate) fn flush_pending_input_lines(&mut self) -> Result<bool, String> {
        if self.raw_input {
            return Ok(false);
        }

        let mut changed = false;
        for line in self.line_buffer.drain_pending_lines() {
            self.dashboard.add_input(&line.port, &line.bytes);
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
