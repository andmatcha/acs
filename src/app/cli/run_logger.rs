use crate::input::ds4_hid::InputReportEvent;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) struct RunLogger {
    writer: Mutex<BufWriter<std::fs::File>>,
}

impl RunLogger {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            writer: Mutex::new(BufWriter::new(file)),
        })
    }

    pub(crate) fn log_input_report(&self, event: &InputReportEvent) -> io::Result<()> {
        self.write_line(format!(
            "ts_ms={} kind=hid seq={} transport={} len={} vid=0x{:04X} pid=0x{:04X} iface={} hex={} ascii={}",
            unix_timestamp_millis(),
            event.sequence,
            event.device.transport,
            event.report.len(),
            event.device.vendor_id,
            event.device.product_id,
            event.device.interface_number,
            format_bytes_hex(&event.report),
            format_bytes_ascii(&event.report),
        ))
    }

    pub(crate) fn log_output_bytes(
        &self,
        sequence: u64,
        format: &str,
        status: &str,
        bytes: &[u8],
    ) -> io::Result<()> {
        self.write_line(format!(
            "ts_ms={} kind=tx seq={} format={} status={} hex={} ascii={}",
            unix_timestamp_millis(),
            sequence,
            format,
            status,
            format_bytes_hex(bytes),
            format_bytes_ascii(bytes),
        ))
    }

    pub(crate) fn log_serial_rx(&self, status: &str, bytes: &[u8]) -> io::Result<()> {
        self.write_line(format!(
            "ts_ms={} kind=rx status={} hex={} ascii={}",
            unix_timestamp_millis(),
            status,
            format_bytes_hex(bytes),
            format_bytes_ascii(bytes),
        ))
    }

    fn write_line(&self, line: String) -> io::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()
    }
}

fn unix_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn format_bytes_hex(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::from("(none)");
    }

    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_bytes_ascii(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::from("\"\"");
    }

    let escaped = bytes
        .iter()
        .flat_map(|byte| std::ascii::escape_default(*byte))
        .map(char::from)
        .collect::<String>();
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::{format_bytes_ascii, format_bytes_hex};

    #[test]
    fn format_bytes_hex_handles_empty_input() {
        assert_eq!(format_bytes_hex(&[]), "(none)");
    }

    #[test]
    fn format_bytes_ascii_escapes_control_bytes() {
        assert_eq!(format_bytes_ascii(b"OK\r\n"), "\"OK\\r\\n\"");
    }
}
