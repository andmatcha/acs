use crate::port_display::{LineBreakMode, PortDisplayMode};
use crate::serial::{
    SerialCallback, SerialConfig, SerialEvent, SerialMonitor, SerialWriter, open_monitor_and_writer,
};
use crate::session::dashboard::SessionDashboard;
use crate::session::event::{IngressFrame, SessionEvent};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const RENDER_INTERVAL: Duration = Duration::from_millis(100);
const RENDER_RATE_WINDOW: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub(crate) struct SessionInputSpec {
    pub id: String,
    pub port: String,
    pub baud_rate: u32,
    pub display_mode: PortDisplayMode,
    pub line_break_mode: LineBreakMode,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionOutputSpec {
    pub id: String,
    pub port: String,
    pub baud_rate: u32,
    pub format_name: String,
    pub display_mode: PortDisplayMode,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionSpec {
    pub title: String,
    pub command_name: String,
    pub log_dir: PathBuf,
    pub logging_enabled: bool,
    pub inputs: Vec<SessionInputSpec>,
    pub outputs: Vec<SessionOutputSpec>,
}

struct SessionOutputHandle {
    port: String,
    status_label: String,
    connection: SerialWriter,
}

pub(crate) struct SessionRuntime {
    dashboard: SessionDashboard,
    event_rx: Receiver<SessionEvent>,
    _input_monitors: Vec<SerialMonitor>,
    outputs: BTreeMap<String, SessionOutputHandle>,
    manual_input_recording: BTreeSet<String>,
    manual_output_recording: BTreeSet<String>,
    render_samples: VecDeque<Instant>,
    dirty: bool,
    inputs_disconnected: bool,
    last_render: Instant,
    pending_user_input: Option<String>,
}

impl SessionRuntime {
    pub(crate) fn new(spec: SessionSpec) -> Result<Self, String> {
        let mut dashboard = SessionDashboard::new(
            &spec.title,
            &spec.command_name,
            &spec.log_dir,
            spec.logging_enabled,
        )?;

        for input in &spec.inputs {
            dashboard.configure_input_port(
                &input.port,
                input.baud_rate,
                input.display_mode,
                input.line_break_mode,
            );
        }
        for output in &spec.outputs {
            dashboard.configure_output_port(
                &output.port,
                output.baud_rate,
                &output.format_name,
                output.display_mode,
            );
        }

        let (event_tx, event_rx) = mpsc::channel::<SessionEvent>();
        let mut input_monitors = Vec::new();
        let mut shared_input_indexes = vec![false; spec.inputs.len()];
        let mut outputs = BTreeMap::new();
        for output in &spec.outputs {
            let status_label = format!("baud={} format={}", output.baud_rate, output.format_name);
            let connection = if let Some((input_index, input)) =
                spec.inputs.iter().enumerate().find(|(index, input)| {
                    !shared_input_indexes[*index]
                        && input.port == output.port
                        && input.baud_rate == output.baud_rate
                }) {
                let (monitor, writer) = open_monitor_and_writer(
                    &SerialConfig {
                        port: output.port.clone(),
                        baud_rate: output.baud_rate,
                    },
                    make_session_callback(event_tx.clone(), &input.id),
                )
                .map_err(|error| error.to_string())?;
                input_monitors.push(monitor);
                shared_input_indexes[input_index] = true;
                writer
            } else {
                SerialWriter::open(&SerialConfig {
                    port: output.port.clone(),
                    baud_rate: output.baud_rate,
                })
                .map_err(|error| error.to_string())?
            };
            outputs.insert(
                output.id.clone(),
                SessionOutputHandle {
                    port: output.port.clone(),
                    status_label,
                    connection,
                },
            );
        }

        for (index, input) in spec.inputs.iter().enumerate() {
            if shared_input_indexes[index] {
                continue;
            }

            let monitor = SerialMonitor::open(
                &SerialConfig {
                    port: input.port.clone(),
                    baud_rate: input.baud_rate,
                },
                make_session_callback(event_tx.clone(), &input.id),
            )
            .map_err(|error| error.to_string())?;
            input_monitors.push(monitor);
        }

        Ok(Self {
            dashboard,
            event_rx,
            _input_monitors: input_monitors,
            outputs,
            manual_input_recording: BTreeSet::new(),
            manual_output_recording: BTreeSet::new(),
            render_samples: VecDeque::new(),
            dirty: false,
            inputs_disconnected: false,
            last_render: Instant::now(),
            pending_user_input: None,
        })
    }

    pub(crate) fn log_path(&self) -> &Path {
        self.dashboard.log_path()
    }

    pub(crate) fn set_interactive_input(&mut self, enabled: bool) {
        self.dashboard.set_interactive_mode(enabled);
    }

    pub(crate) fn set_input_packet_framing(&mut self, port: &str, packet_len: usize) {
        self.dashboard.set_input_packet_framing(port, packet_len);
        self.dirty = true;
    }

    pub(crate) fn set_input_packet_rate_enabled(&mut self, port: &str, enabled: bool) {
        self.dashboard.set_input_packet_rate_enabled(port, enabled);
        self.dirty = true;
    }

    pub(crate) fn set_output_packet_rate_enabled(&mut self, port: &str, enabled: bool) {
        self.dashboard.set_output_packet_rate_enabled(port, enabled);
        self.dirty = true;
    }

    pub(crate) fn set_manual_input_recording(&mut self, input_id: &str, enabled: bool) {
        if enabled {
            self.manual_input_recording.insert(input_id.to_owned());
        } else {
            self.manual_input_recording.remove(input_id);
        }
    }

    pub(crate) fn set_manual_output_recording(&mut self, output_id: &str, enabled: bool) {
        if enabled {
            self.manual_output_recording.insert(output_id.to_owned());
        } else {
            self.manual_output_recording.remove(output_id);
        }
    }

    pub(crate) fn take_user_input(&mut self) -> Option<String> {
        self.pending_user_input.take()
    }

    pub(crate) fn set_header_lines(&mut self, lines: Vec<String>) {
        self.dashboard.set_header_lines(lines);
        self.dirty = true;
    }

    pub(crate) fn set_status(&mut self, status: impl Into<String>) {
        self.dashboard.set_status(status);
        self.dirty = true;
    }

    pub(crate) fn record_input_sample(&mut self, port: &str, byte_len: usize, packet_count: usize) {
        self.dashboard
            .record_input_sample(port, byte_len, packet_count);
        self.dirty = true;
    }

    pub(crate) fn record_output_sample(
        &mut self,
        port: &str,
        byte_len: usize,
        packet_count: usize,
    ) {
        self.dashboard
            .record_output_sample(port, byte_len, packet_count);
        self.dirty = true;
    }

    pub(crate) fn add_input_entry(&mut self, port: &str, bytes: &[u8]) -> Result<(), String> {
        self.dashboard.add_input_entry(port, bytes)?;
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn add_output_entry(&mut self, port: &str, bytes: &[u8]) -> Result<(), String> {
        self.dashboard.add_output_entry(port, bytes)?;
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn write_output(&mut self, output_id: &str, bytes: &[u8]) -> Result<(), String> {
        let output = self
            .outputs
            .get_mut(output_id)
            .ok_or_else(|| format!("unknown output id: {output_id}"))?;
        output
            .connection
            .write_bytes(bytes)
            .map_err(|error| error.to_string())?;
        if !self.manual_output_recording.contains(output_id) {
            self.dashboard.record_output(&output.port, bytes)?;
            self.dirty = true;
        }
        Ok(())
    }

    pub(crate) fn current_render_fps(&mut self) -> f64 {
        self.prune_render_samples(Instant::now());
        self.render_samples.len() as f64 / RENDER_RATE_WINDOW.as_secs_f64()
    }

    pub(crate) fn set_output_error(
        &mut self,
        output_id: &str,
        message: &str,
    ) -> Result<(), String> {
        let output = self
            .outputs
            .get(output_id)
            .ok_or_else(|| format!("unknown output id: {output_id}"))?;
        self.dashboard.set_output_status(
            &output.port,
            format!("{} error={message}", output.status_label),
        );
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn clear_output_error(&mut self, output_id: &str) -> Result<(), String> {
        let output = self
            .outputs
            .get(output_id)
            .ok_or_else(|| format!("unknown output id: {output_id}"))?;
        self.dashboard
            .set_output_status(&output.port, output.status_label.clone());
        self.dirty = true;
        Ok(())
    }

    pub(crate) fn run_loop<F, G>(
        &mut self,
        wait_interval: Duration,
        stop_requested: G,
        on_frame: F,
    ) -> Result<PathBuf, String>
    where
        F: FnMut(&IngressFrame, &mut SessionRuntime) -> Result<(), String>,
        G: FnMut() -> bool,
    {
        self.run_loop_with_tick(wait_interval, stop_requested, on_frame, |_| Ok(()))
    }

    pub(crate) fn run_loop_with_tick<F, G, H>(
        &mut self,
        wait_interval: Duration,
        mut stop_requested: G,
        mut on_frame: F,
        mut on_tick: H,
    ) -> Result<PathBuf, String>
    where
        F: FnMut(&IngressFrame, &mut SessionRuntime) -> Result<(), String>,
        G: FnMut() -> bool,
        H: FnMut(&mut SessionRuntime) -> Result<(), String>,
    {
        self.dashboard.render()?;
        self.note_render(Instant::now());

        while !stop_requested() {
            let user_input = self.dashboard.handle_dashboard_action()?;
            if let Some(input) = user_input {
                self.pending_user_input = Some(input);
            }
            self.wait_for_events(wait_interval, &mut on_frame)?;
            on_tick(self)?;
            self.render_if_needed()?;
        }

        self.drain_pending_events(&mut on_frame)?;
        if self.dashboard.flush_pending_input_lines()? {
            self.dirty = true;
        }
        self.dashboard.set_status("stopped");
        self.dashboard.render()?;
        self.note_render(Instant::now());

        Ok(self.dashboard.log_path().to_path_buf())
    }

    fn wait_for_events<F>(
        &mut self,
        wait_interval: Duration,
        on_frame: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(&IngressFrame, &mut SessionRuntime) -> Result<(), String>,
    {
        if self._input_monitors.is_empty() {
            thread::sleep(wait_interval);
            return Ok(());
        }

        match self.event_rx.recv_timeout(wait_interval) {
            Ok(event) => self.handle_event(event, on_frame)?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !self.inputs_disconnected {
                    self.dashboard.set_status("input disconnected");
                    self.dirty = true;
                    self.inputs_disconnected = true;
                }
                thread::sleep(wait_interval);
                return Ok(());
            }
        }

        self.drain_pending_events(on_frame)
    }

    fn drain_pending_events<F>(&mut self, on_frame: &mut F) -> Result<(), String>
    where
        F: FnMut(&IngressFrame, &mut SessionRuntime) -> Result<(), String>,
    {
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_event(event, on_frame)?;
        }
        Ok(())
    }

    fn handle_event<F>(&mut self, event: SessionEvent, on_frame: &mut F) -> Result<(), String>
    where
        F: FnMut(&IngressFrame, &mut SessionRuntime) -> Result<(), String>,
    {
        match event {
            SessionEvent::InputData {
                input_id,
                port,
                bytes,
            } => {
                let should_record_input = !self.manual_input_recording.contains(&input_id);
                if should_record_input && self.dashboard.record_input(&port, &bytes)? {
                    self.dirty = true;
                }

                let frame = IngressFrame { input_id, bytes };
                on_frame(&frame, self)?;
            }
            SessionEvent::InputError {
                input_id,
                port,
                message,
            } => {
                let _ = input_id;
                if self.dashboard.record_input_error(&port, &message)? {
                    self.dirty = true;
                }
            }
        }

        Ok(())
    }

    fn render_if_needed(&mut self) -> Result<(), String> {
        if !self.dashboard.is_paused()
            && self.dirty
            && self.last_render.elapsed() >= RENDER_INTERVAL
        {
            self.dashboard.render()?;
            self.dirty = false;
            self.note_render(Instant::now());
        }
        Ok(())
    }

    fn note_render(&mut self, now: Instant) {
        self.last_render = now;
        self.render_samples.push_back(now);
        self.prune_render_samples(now);
    }

    fn prune_render_samples(&mut self, now: Instant) {
        while let Some(sample) = self.render_samples.front() {
            if now.duration_since(*sample) <= RENDER_RATE_WINDOW {
                break;
            }
            self.render_samples.pop_front();
        }
    }
}

fn make_session_callback(event_tx: mpsc::Sender<SessionEvent>, input_id: &str) -> SerialCallback {
    let input_id = input_id.to_owned();
    Arc::new(move |event| match event {
        SerialEvent::Data { port, bytes } => {
            let _ = event_tx.send(SessionEvent::InputData {
                input_id: input_id.clone(),
                port,
                bytes,
            });
        }
        SerialEvent::Error { port, message } => {
            let _ = event_tx.send(SessionEvent::InputError {
                input_id: input_id.clone(),
                port,
                message,
            });
        }
    })
}
