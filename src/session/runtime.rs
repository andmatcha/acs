use crate::port_display::PortDisplayMode;
use crate::serial::{SerialCallback, SerialConfig, SerialEvent, SerialMonitor, SerialWriter};
use crate::session::dashboard::SessionDashboard;
use crate::session::event::{IngressFrame, SessionEvent};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const RENDER_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub(crate) struct SessionInputSpec {
    pub id: String,
    pub port: String,
    pub baud_rate: u32,
    pub display_mode: PortDisplayMode,
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
    pub raw_input: bool,
    pub log_dir: PathBuf,
    pub header_lines: Vec<String>,
    pub inputs: Vec<SessionInputSpec>,
    pub outputs: Vec<SessionOutputSpec>,
}

struct SessionOutputHandle {
    port: String,
    connection: SerialWriter,
}

pub(crate) struct SessionRuntime {
    dashboard: SessionDashboard,
    event_rx: Receiver<SessionEvent>,
    _input_monitors: Vec<SerialMonitor>,
    outputs: BTreeMap<String, SessionOutputHandle>,
    next_sequence: u64,
    dirty: bool,
    last_render: Instant,
}

impl SessionRuntime {
    pub(crate) fn new(spec: SessionSpec) -> Result<Self, String> {
        let mut dashboard = SessionDashboard::new(
            &spec.title,
            spec.raw_input,
            &spec.command_name,
            &spec.log_dir,
        )?;
        dashboard.set_header_lines(spec.header_lines);

        let (event_tx, event_rx) = mpsc::channel::<SessionEvent>();
        let input_monitors = spec
            .inputs
            .iter()
            .map(|input| {
                dashboard.configure_input_port(&input.port, input.baud_rate, input.display_mode);
                SerialMonitor::open(
                    &SerialConfig {
                        port: input.port.clone(),
                        baud_rate: input.baud_rate,
                    },
                    make_session_callback(event_tx.clone(), &input.id, &input.port),
                )
                .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut outputs = BTreeMap::new();
        for output in &spec.outputs {
            dashboard.configure_output_port(
                &output.port,
                output.baud_rate,
                &output.format_name,
                output.display_mode,
            );
            let connection = SerialWriter::open(&SerialConfig {
                port: output.port.clone(),
                baud_rate: output.baud_rate,
            })
            .map_err(|error| error.to_string())?;
            outputs.insert(
                output.id.clone(),
                SessionOutputHandle {
                    port: output.port.clone(),
                    connection,
                },
            );
        }

        Ok(Self {
            dashboard,
            event_rx,
            _input_monitors: input_monitors,
            outputs,
            next_sequence: 0,
            dirty: false,
            last_render: Instant::now(),
        })
    }

    pub(crate) fn log_path(&self) -> &Path {
        self.dashboard.log_path()
    }

    pub(crate) fn set_header_lines(&mut self, lines: Vec<String>) {
        self.dashboard.set_header_lines(lines);
    }

    pub(crate) fn write_output(&mut self, output_id: &str, bytes: &[u8]) -> Result<(), String> {
        let output = self
            .outputs
            .get_mut(output_id)
            .ok_or_else(|| format!("unknown output id: {output_id}"))?;
        output.connection.write_bytes(bytes).map_err(|error| error.to_string())?;
        self.dashboard.record_output(&output.port, bytes)?;
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
        self.dashboard.render(None)?;

        while !stop_requested() {
            self.dashboard.handle_dashboard_action()?;
            self.wait_for_events(wait_interval, &mut on_frame)?;
            on_tick(self)?;
            self.render_if_needed(None)?;
        }

        self.drain_pending_events(&mut on_frame)?;
        if self.dashboard.flush_pending_input_lines()? {
            self.dirty = true;
        }
        self.dashboard.render(Some("stopped"))?;

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
        match self.event_rx.recv_timeout(wait_interval) {
            Ok(event) => self.handle_event(event, on_frame)?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(String::from("session input channel disconnected"));
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
                if self.dashboard.record_input(&port, &bytes)? {
                    self.dirty = true;
                }

                let frame = IngressFrame {
                    input_id,
                    port,
                    bytes,
                    sequence: self.next_sequence,
                };
                self.next_sequence = self.next_sequence.wrapping_add(1);
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

    fn render_if_needed(&mut self, status: Option<&str>) -> Result<(), String> {
        if !self.dashboard.is_paused() && self.dirty && self.last_render.elapsed() >= RENDER_INTERVAL
        {
            self.dashboard.render(status)?;
            self.dirty = false;
            self.last_render = Instant::now();
        }
        Ok(())
    }
}

fn make_session_callback(event_tx: mpsc::Sender<SessionEvent>, input_id: &str, _: &str) -> SerialCallback {
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
