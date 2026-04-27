use serialport::{
    ClearBuffer, Error as SerialPortLibError, SerialPort, SerialPortInfo, SerialPortType, new,
};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const READ_BUFFER_SIZE: usize = 256;
const READ_TIMEOUT_MILLIS: u64 = 50;
const WRITE_TIMEOUT_MILLIS: u64 = 1_000;
const WRITE_RETRY_INTERVAL: Duration = Duration::from_millis(5);
const XBEE_S3B_BOOTLOADER_SCAN_MILLIS: u64 = 500;
const XBEE_S3B_BOOTLOADER_READ_TIMEOUT_MILLIS: u64 = 20;
const XBEE_S3B_BOOTLOADER_RECOVERY_SETTLE_MILLIS: u64 = 500;
const XBEE_S3B_BOOTLOADER_RECOVERY_COMMAND: &[u8] = b"B";
const XBEE_S3B_BOOTLOADER_MENU_MARKERS: [&[u8]; 5] = [
    b"R-Reset",
    b"A-App Ver.",
    b"V-BL Ver.",
    b"T-Timeout",
    b"F-Update App",
];

pub type SerialCallback = Arc<dyn Fn(SerialEvent) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct SerialConfig {
    pub port: String,
    pub baud_rate: u32,
    pub xbee_s3b_recovery: bool,
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
    PortIndexOutOfRange {
        index: usize,
        count: usize,
    },
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
            Self::PortIndexOutOfRange { index, count } => write!(
                f,
                "serial port index `{index}` is out of range for {count} available ports; use `acs ports` to inspect candidates"
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
        let deadline = Instant::now() + Duration::from_millis(WRITE_TIMEOUT_MILLIS);
        let mut written = 0usize;

        while written < bytes.len() {
            match self.writer.write(&bytes[written..]) {
                Ok(0) => {
                    if Instant::now() >= deadline {
                        return Err(SerialError::Write {
                            port: self.port_name.clone(),
                            source: io::Error::new(
                                io::ErrorKind::WriteZero,
                                "serial port accepted 0 bytes",
                            ),
                        });
                    }
                    thread::sleep(WRITE_RETRY_INTERVAL);
                }
                Ok(count) => written += count,
                Err(error) if is_transient_write_error(&error) && Instant::now() < deadline => {
                    thread::sleep(WRITE_RETRY_INTERVAL);
                }
                Err(source) => {
                    return Err(SerialError::Write {
                        port: self.port_name.clone(),
                        source,
                    });
                }
            }
        }

        Ok(())
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
    resolve_port_from_ports(port_name, &ports)
}

fn resolve_port_from_ports(
    port_name: Option<&str>,
    ports: &[SerialPortInfo],
) -> Result<String, SerialError> {
    match port_name {
        Some(port_name) => resolve_requested_port(port_name, ports),
        None => auto_select_port(&ports),
    }
}

fn resolve_requested_port(
    port_name: &str,
    ports: &[SerialPortInfo],
) -> Result<String, SerialError> {
    if let Some(port) = ports.iter().find(|port| port.port_name == port_name) {
        return Ok(port.port_name.clone());
    }

    if let Ok(index) = port_name.parse::<usize>() {
        return ports.get(index).map(|port| port.port_name.clone()).ok_or(
            SerialError::PortIndexOutOfRange {
                index,
                count: ports.len(),
            },
        );
    }

    Err(SerialError::PortNotFound(port_name.to_owned()))
}

fn auto_select_port(ports: &[SerialPortInfo]) -> Result<String, SerialError> {
    let preferred_ports = grouped_ports(
        ports
            .iter()
            .filter(|port| is_preferred_auto_select_port(port)),
    );
    match preferred_ports.as_slice() {
        [group] => Ok(select_group_port(group)),
        [] => fallback_auto_select_port(ports),
        many => Err(SerialError::MultiplePortsFound(many.len())),
    }
}

fn fallback_auto_select_port(ports: &[SerialPortInfo]) -> Result<String, SerialError> {
    let usb_ports = grouped_ports(
        ports
            .iter()
            .filter(|port| matches!(port.port_type, SerialPortType::UsbPort(_))),
    );

    match usb_ports.as_slice() {
        [group] => Ok(select_group_port(group)),
        [] => {
            let all_ports = grouped_ports(ports.iter());
            match all_ports.as_slice() {
                [] => Err(SerialError::NoSerialPortFound),
                [group] => Ok(select_group_port(group)),
                many => Err(SerialError::MultiplePortsFound(many.len())),
            }
        }
        many => Err(SerialError::MultiplePortsFound(many.len())),
    }
}

fn grouped_ports<'a>(
    ports: impl IntoIterator<Item = &'a SerialPortInfo>,
) -> Vec<Vec<&'a SerialPortInfo>> {
    let mut groups = BTreeMap::<String, Vec<&'a SerialPortInfo>>::new();

    for port in ports {
        groups
            .entry(auto_select_group_key(&port.port_name))
            .or_default()
            .push(port);
    }

    groups.into_values().collect()
}

fn auto_select_group_key(port_name: &str) -> String {
    if let Some(suffix) = port_name.strip_prefix("/dev/tty.") {
        return format!("/dev/serial.{suffix}");
    }
    if let Some(suffix) = port_name.strip_prefix("/dev/cu.") {
        return format!("/dev/serial.{suffix}");
    }
    port_name.to_owned()
}

fn select_group_port(group: &[&SerialPortInfo]) -> String {
    group
        .iter()
        .copied()
        .filter(|port| is_dialout_port_name(&port.port_name))
        .min_by_key(|port| port.port_name.as_str())
        .or_else(|| {
            group
                .iter()
                .copied()
                .min_by_key(|port| port.port_name.as_str())
        })
        .expect("group must not be empty")
        .port_name
        .clone()
}

fn is_dialout_port_name(port_name: &str) -> bool {
    port_name.starts_with("/dev/cu.")
}

fn is_preferred_auto_select_port(port: &SerialPortInfo) -> bool {
    matches!(port.port_type, SerialPortType::UsbPort(_))
        || port_name_looks_like_usb_serial(&port.port_name)
}

fn port_name_looks_like_usb_serial(port_name: &str) -> bool {
    let port_name = port_name.to_ascii_lowercase();
    port_name.contains("ttyusb")
        || port_name.contains("ttyacm")
        || port_name.contains("usbserial")
        || port_name.contains("usbmodem")
        || port_name.contains("/dev/tty.usb")
        || port_name.contains("/dev/cu.usb")
        || port_name.contains("st-link")
        || port_name.contains("stlink")
}

fn open_port(config: &SerialConfig) -> Result<Box<dyn SerialPort>, SerialError> {
    let mut port = new(&config.port, config.baud_rate)
        .timeout(Duration::from_millis(WRITE_TIMEOUT_MILLIS))
        .open()
        .map_err(|source| SerialError::Open {
            port: config.port.clone(),
            source,
        })?;
    clear_break_after_open(&mut *port);
    if config.xbee_s3b_recovery {
        recover_xbee_s3b_bootloader(&mut *port, &config.port)?;
    }
    Ok(port)
}

fn clear_break_after_open(port: &dyn SerialPort) {
    // XBee bootloaders can use a serial break during their entry sequence.  Clearing it here is
    // best-effort because some adapters do not expose break control, and normal UART traffic does
    // not require a fatal error if the line cannot be changed.
    let _ = port.clear_break();
}

fn recover_xbee_s3b_bootloader(
    port: &mut dyn SerialPort,
    port_name: &str,
) -> Result<(), SerialError> {
    let original_timeout = port.timeout();
    port.set_timeout(Duration::from_millis(
        XBEE_S3B_BOOTLOADER_READ_TIMEOUT_MILLIS,
    ))
    .map_err(|source| SerialError::Configure {
        port: port_name.to_owned(),
        source,
    })?;

    let result = scan_and_recover_xbee_s3b_bootloader(port, port_name);
    let restore_result =
        port.set_timeout(original_timeout)
            .map_err(|source| SerialError::Configure {
                port: port_name.to_owned(),
                source,
            });

    result.and(restore_result)
}

fn scan_and_recover_xbee_s3b_bootloader(
    port: &mut dyn SerialPort,
    port_name: &str,
) -> Result<(), SerialError> {
    let deadline = Instant::now() + Duration::from_millis(XBEE_S3B_BOOTLOADER_SCAN_MILLIS);
    let mut received = Vec::new();
    let mut buffer = [0u8; READ_BUFFER_SIZE];

    while Instant::now() < deadline {
        match port.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => {
                received.extend_from_slice(&buffer[..count]);
                if looks_like_xbee_s3b_bootloader_menu(&received) {
                    port.write_all(XBEE_S3B_BOOTLOADER_RECOVERY_COMMAND)
                        .map_err(|source| SerialError::Write {
                            port: port_name.to_owned(),
                            source,
                        })?;
                    thread::sleep(Duration::from_millis(
                        XBEE_S3B_BOOTLOADER_RECOVERY_SETTLE_MILLIS,
                    ));
                    return Ok(());
                }
            }
            Err(error) if is_transient_read_error(&error) => {}
            Err(source) => {
                return Err(SerialError::Write {
                    port: port_name.to_owned(),
                    source,
                });
            }
        }
    }

    Ok(())
}

pub(crate) fn looks_like_xbee_s3b_bootloader_menu(bytes: &[u8]) -> bool {
    let marker_count = XBEE_S3B_BOOTLOADER_MENU_MARKERS
        .iter()
        .filter(|marker| contains_ascii_case_insensitive(bytes, marker))
        .count();

    marker_count >= 2
        || contains_ascii_case_insensitive(bytes, b"A-App Ver.")
        || contains_ascii_case_insensitive(bytes, b"V-BL Ver.")
        || contains_ascii_case_insensitive(bytes, b"F-Update App")
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .windows(needle.len())
        .any(|window| ascii_case_insensitive_eq(window, needle))
}

fn ascii_case_insensitive_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
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

fn is_transient_write_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
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
    use super::{SerialError, SerialLineBuffer, auto_select_port, resolve_port_from_ports};
    use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};

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

    #[test]
    fn auto_select_port_prefers_single_usb_serial_device_even_with_tty_cu_aliases() {
        let ports = vec![
            port(
                "/dev/tty.Bluetooth-Incoming-Port",
                SerialPortType::BluetoothPort,
            ),
            port("/dev/tty.usbmodem1103", SerialPortType::Unknown),
            port("/dev/cu.usbmodem1103", SerialPortType::Unknown),
        ];

        assert_eq!(auto_select_port(&ports).unwrap(), "/dev/cu.usbmodem1103");
    }

    #[test]
    fn auto_select_port_supports_single_st_link_named_port() {
        let ports = vec![
            port(
                "/dev/tty.Bluetooth-Incoming-Port",
                SerialPortType::BluetoothPort,
            ),
            port("/dev/cu.ST-LINK", SerialPortType::Unknown),
        ];

        assert_eq!(auto_select_port(&ports).unwrap(), "/dev/cu.ST-LINK");
    }

    #[test]
    fn auto_select_port_prefers_single_usb_port_when_other_ports_exist() {
        let ports = vec![
            port("/dev/ttyS0", SerialPortType::PciPort),
            port("/dev/ttyUSB0", usb_port_type("USB Serial")),
        ];

        assert_eq!(auto_select_port(&ports).unwrap(), "/dev/ttyUSB0");
    }

    #[test]
    fn resolve_port_from_ports_accepts_acs_ports_index() {
        let ports = vec![
            port("/dev/ttyUSB0", usb_port_type("USB Serial 0")),
            port("/dev/ttyUSB1", usb_port_type("USB Serial 1")),
        ];

        assert_eq!(
            resolve_port_from_ports(Some("1"), &ports).unwrap(),
            "/dev/ttyUSB1"
        );
    }

    #[test]
    fn resolve_port_from_ports_prefers_exact_name_before_index_lookup() {
        let ports = vec![
            port("1", SerialPortType::Unknown),
            port("/dev/ttyUSB1", usb_port_type("USB Serial 1")),
        ];

        assert_eq!(resolve_port_from_ports(Some("1"), &ports).unwrap(), "1");
    }

    #[test]
    fn resolve_port_from_ports_reports_out_of_range_index() {
        let ports = vec![port("/dev/ttyUSB0", usb_port_type("USB Serial 0"))];

        match resolve_port_from_ports(Some("2"), &ports).unwrap_err() {
            SerialError::PortIndexOutOfRange { index, count } => {
                assert_eq!(index, 2);
                assert_eq!(count, 1);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    fn port(port_name: &str, port_type: SerialPortType) -> SerialPortInfo {
        SerialPortInfo {
            port_name: port_name.to_owned(),
            port_type,
        }
    }

    fn usb_port_type(product: &str) -> SerialPortType {
        SerialPortType::UsbPort(UsbPortInfo {
            vid: 0x0483,
            pid: 0x5740,
            serial_number: Some(String::from("serial-1")),
            manufacturer: Some(String::from("STMicroelectronics")),
            product: Some(product.to_owned()),
        })
    }
}
