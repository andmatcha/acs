use super::logger::CommandLogger;
use crate::port_display::PortDisplayMode;
use crate::serial::{SerialCallback, SerialConfig, SerialEvent, SerialLineBuffer, SerialMonitor};
use crate::ui::text_dashboard::{TextDashboard, TextDashboardAction};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

pub(crate) struct SerialDashboard {
    dashboard: TextDashboard,
    logger: CommandLogger,
    line_buffer: SerialLineBuffer,
    raw_input: bool,
    paused: bool,
}

impl SerialDashboard {
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

    pub(crate) fn handle_dashboard_action(&mut self) -> Result<(), String> {
        let action = self
            .dashboard
            .poll_action()
            .map_err(|error| format!("failed to read keyboard input: {error}"))?;

        match action {
            Some(TextDashboardAction::TogglePause) => {
                self.paused = !self.paused;
                let status = if self.paused {
                    "paused (space: resume)"
                } else {
                    "resumed"
                };
                self.render(Some(status))?;
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

    pub(crate) fn drain_serial_events(
        &mut self,
        event_rx: &Receiver<SerialEvent>,
    ) -> Result<bool, String> {
        let mut changed = false;

        while let Ok(event) = event_rx.try_recv() {
            if self.handle_event(event)? {
                changed = true;
            }
        }

        Ok(changed)
    }

    pub(crate) fn wait_for_serial_events(
        &mut self,
        event_rx: &Receiver<SerialEvent>,
        wait_interval: Duration,
    ) -> Result<bool, String> {
        let mut changed = false;

        match event_rx.recv_timeout(wait_interval) {
            Ok(event) => {
                if self.handle_event(event)? {
                    changed = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(String::from("serial monitor channel disconnected"));
            }
        }

        if self.drain_serial_events(event_rx)? {
            changed = true;
        }

        Ok(changed)
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

    pub(crate) fn render(&mut self, status: Option<&str>) -> Result<(), String> {
        self.dashboard
            .render(status)
            .map_err(|error| format!("failed to render dashboard: {error}"))
    }

    fn handle_event(&mut self, event: SerialEvent) -> Result<bool, String> {
        match event {
            SerialEvent::Data { port, bytes } => self.handle_input_bytes(&port, &bytes),
            SerialEvent::Error { port, message } => {
                self.dashboard
                    .set_input_status(&port, format!("error: {message}"));
                self.logger
                    .log_status(&port, &message)
                    .map_err(|error| format!("failed to write log: {error}"))?;
                Ok(true)
            }
        }
    }

    fn handle_input_bytes(&mut self, port: &str, bytes: &[u8]) -> Result<bool, String> {
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
}

pub(crate) fn make_serial_callback(event_tx: mpsc::Sender<SerialEvent>) -> SerialCallback {
    Arc::new(move |event| {
        let _ = event_tx.send(event);
    })
}

pub(crate) fn open_serial_monitors(
    ports: &[String],
    baud_rate: u32,
    callback: SerialCallback,
) -> Result<Vec<SerialMonitor>, String> {
    ports
        .iter()
        .map(|port| {
            SerialMonitor::open(
                &SerialConfig {
                    port: port.clone(),
                    baud_rate,
                },
                Arc::clone(&callback),
            )
            .map_err(|error| error.to_string())
        })
        .collect()
}
