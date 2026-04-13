use serialport::{
    ClearBuffer, Error as SerialPortLibError, SerialPort, SerialPortInfo, SerialPortType, new,
};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const READ_BUFFER_SIZE: usize = 256;
const READ_TIMEOUT_MILLIS: u64 = 50;
const WRITE_TIMEOUT_MILLIS: u64 = 1_000;

pub type SerialCallback = Arc<dyn Fn(SerialEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct SerialConfig {
    pub port: String,
    pub baud_rate: u32,
}

#[derive(Debug, Clone)]
pub enum SerialEvent {
    Data { port: String, bytes: Vec<u8> },
    Error { port: String, message: String },
}

#[derive(Debug, Clone)]
pub struct SerialInputLine {
    pub port: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct SerialLineBuffer {
    pending: BTreeMap<String, Vec<u8>>,
}

impl SerialLineBuffer {
    pub fn push_chunk(&mut self, port: &str, bytes: &[u8]) -> Vec<Vec<u8>> {
        let pending = self.pending.entry(String::from(port)).or_default();
        pending.extend_from_slice(bytes);
        take_complete_lines(pending)
    }

    pub fn drain_pending_lines(&mut self) -> Vec<SerialInputLine> {
        let mut lines = Vec::new();

        for (port, bytes) in self.pending.iter_mut() {
            if bytes.is_empty() {
                continue;
            }

            lines.push(SerialInputLine {
                port: port.clone(),
                bytes: std::mem::take(bytes),
            });
        }

        lines
    }
}

#[derive(Debug)]
pub enum SerialError {
    List(SerialPortLibError),
    Open {
        port: String,
        source: SerialPortLibError,
    },
    Configure {
        port: String,
        source: SerialPortLibError,
    },
    Clone {
        port: String,
        source: SerialPortLibError,
    },
    Write {
        port: String,
        source: io::Error,
    },
    NoSerialPortFound,
    MultiplePortsFound(usize),
    PortNotFound(String),
}

impl fmt::Display for SerialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::List(error) => write!(f, "{error}"),
            Self::Open { port, source } => write!(f, "failed to open {port}: {source}"),
            Self::Configure { port, source } => {
                write!(f, "failed to configure {port}: {source}")
            }
            Self::Clone { port, source } => {
                write!(f, "failed to clone handle for {port}: {source}")
            }
            Self::Write { port, source } => write!(f, "failed to write to {port}: {source}"),
            Self::NoSerialPortFound => {
                write!(f, "no serial port found; specify one with --port")
            }
            Self::MultiplePortsFound(count) => write!(
                f,
                "{count} serial ports found; specify one explicitly with --port"
            ),
            Self::PortNotFound(port) => write!(
                f,
                "serial port `{port}` was not found; use `acs ports` to inspect candidates"
            ),
        }
    }
}

pub struct SerialWriter {
    port_name: String,
    writer: Box<dyn SerialPort>,
}

impl SerialWriter {
    pub fn open(config: &SerialConfig) -> Result<Self, SerialError> {
        let writer = open_port(config)?;
        Ok(Self::from_port(config.port.clone(), writer))
    }

    fn from_port(port_name: String, writer: Box<dyn SerialPort>) -> Self {
        Self { port_name, writer }
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), SerialError> {
        self.writer
            .write_all(bytes)
            .map_err(|source| SerialError::Write {
                port: self.port_name.clone(),
                source,
            })
    }
}

pub struct SerialMonitor {
    stop_requested: Arc<AtomicBool>,
    reader_thread: Option<JoinHandle<()>>,
}

impl SerialMonitor {
    pub fn open(config: &SerialConfig, callback: SerialCallback) -> Result<Self, SerialError> {
        let reader = open_port(config)?;
        Self::from_reader(reader, config.port.clone(), callback)
    }

    fn from_reader(
        mut reader: Box<dyn SerialPort>,
        port_name: String,
        callback: SerialCallback,
    ) -> Result<Self, SerialError> {
        reader
            .clear(ClearBuffer::Input)
            .map_err(|source| SerialError::Configure {
                port: port_name.clone(),
                source,
            })?;
        reader
            .set_timeout(Duration::from_millis(READ_TIMEOUT_MILLIS))
            .map_err(|source| SerialError::Configure {
                port: port_name.clone(),
                source,
            })?;

        let stop_requested = Arc::new(AtomicBool::new(false));
        let reader_thread = Some(spawn_reader_thread(
            port_name.clone(),
            reader,
            Arc::clone(&stop_requested),
            callback,
        ));

        Ok(Self {
            stop_requested,
            reader_thread,
        })
    }
}

impl Drop for SerialMonitor {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::SeqCst);
        if let Some(reader_thread) = self.reader_thread.take() {
            let _ = reader_thread.join();
        }
    }
}

pub fn open_monitor_and_writer(
    config: &SerialConfig,
    callback: SerialCallback,
) -> Result<(SerialMonitor, SerialWriter), SerialError> {
    let reader = open_port(config)?;
    let writer = reader.try_clone().map_err(|source| SerialError::Clone {
        port: config.port.clone(),
        source,
    })?;
    let monitor = SerialMonitor::from_reader(reader, config.port.clone(), callback)?;

    Ok((
        monitor,
        SerialWriter::from_port(config.port.clone(), writer),
    ))
}

pub fn available_ports() -> Result<Vec<SerialPortInfo>, SerialError> {
    serialport::available_ports().map_err(SerialError::List)
}

pub fn resolve_port(port_name: Option<&str>) -> Result<String, SerialError> {
    let ports = available_ports()?;
    match port_name {
        Some(port_name) => ports
            .iter()
            .find(|port| port.port_name == port_name)
            .map(|port| port.port_name.clone())
            .ok_or_else(|| SerialError::PortNotFound(port_name.to_owned())),
        None => auto_select_port(&ports),
    }
}

fn auto_select_port(ports: &[SerialPortInfo]) -> Result<String, SerialError> {
    let usb_ports = ports
        .iter()
        .filter(|port| matches!(port.port_type, SerialPortType::UsbPort(_)))
        .collect::<Vec<_>>();

    match usb_ports.as_slice() {
        [port] => Ok(port.port_name.clone()),
        [] => match ports {
            [] => Err(SerialError::NoSerialPortFound),
            [port] => Ok(port.port_name.clone()),
            many => Err(SerialError::MultiplePortsFound(many.len())),
        },
        many => Err(SerialError::MultiplePortsFound(many.len())),
    }
}

fn open_port(config: &SerialConfig) -> Result<Box<dyn SerialPort>, SerialError> {
    new(&config.port, config.baud_rate)
        .timeout(Duration::from_millis(WRITE_TIMEOUT_MILLIS))
        .open()
        .map_err(|source| SerialError::Open {
            port: config.port.clone(),
            source,
        })
}

fn spawn_reader_thread(
    port_name: String,
    mut reader: Box<dyn SerialPort>,
    stop_requested: Arc<AtomicBool>,
    callback: SerialCallback,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0u8; READ_BUFFER_SIZE];

        while !stop_requested.load(Ordering::SeqCst) {
            match reader.read(&mut buffer) {
                Ok(0) => {}
                Ok(count) => {
                    let mut bytes = Vec::from(&buffer[..count]);
                    // OS から分割されて届いた連続データは、可能な限り 1 イベントにまとめる。
                    if let Err(error) = drain_available_bytes(&mut *reader, &mut bytes, &mut buffer)
                    {
                        callback(SerialEvent::Error {
                            port: port_name.clone(),
                            message: error.to_string(),
                        });
                        break;
                    }
                    callback(SerialEvent::Data {
                        port: port_name.clone(),
                        bytes,
                    });
                }
                Err(error) if is_transient_read_error(&error) => {}
                Err(error) => {
                    callback(SerialEvent::Error {
                        port: port_name.clone(),
                        message: error.to_string(),
                    });
                    break;
                }
            }
        }
    })
}

fn drain_available_bytes(
    reader: &mut dyn SerialPort,
    received: &mut Vec<u8>,
    buffer: &mut [u8; READ_BUFFER_SIZE],
) -> Result<(), SerialPortLibError> {
    loop {
        let available = reader.bytes_to_read()? as usize;
        if available == 0 {
            return Ok(());
        }

        let chunk_len = available.min(buffer.len());
        match reader.read(&mut buffer[..chunk_len]) {
            Ok(0) => return Ok(()),
            Ok(count) => received.extend_from_slice(&buffer[..count]),
            Err(error) if is_transient_read_error(&error) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
}

fn is_transient_read_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

fn take_complete_lines(pending: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();

    while let Some(position) = pending.iter().position(|byte| *byte == b'\n') {
        lines.push(pending.drain(..=position).collect());
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::SerialLineBuffer;

    #[test]
    fn line_buffer_reassembles_text_split_across_chunks() {
        let mut buffer = SerialLineBuffer::default();

        assert_eq!(
            buffer.push_chunk("/dev/ttyUSB0", b"0 00 00\r\nCA"),
            vec![b"0 00 00\r\n".to_vec()]
        );
        assert!(
            buffer
                .push_chunk("/dev/ttyUSB0", b"N TX 0x1FF: ")
                .is_empty()
        );
        assert!(buffer.push_chunk("/dev/ttyUSB0", b"00 00 ").is_empty());
        assert!(
            buffer
                .push_chunk("/dev/ttyUSB0", b"00 00 00 00 00 00")
                .is_empty()
        );
        assert_eq!(
            buffer.push_chunk("/dev/ttyUSB0", b"\r\nCAN TX 0x2"),
            vec![b"CAN TX 0x1FF: 00 00 00 00 00 00 00 00\r\n".to_vec()]
        );

        let pending = buffer.drain_pending_lines();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].port, "/dev/ttyUSB0");
        assert_eq!(pending[0].bytes, b"CAN TX 0x2".to_vec());
    }
}
