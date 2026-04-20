use crate::common::{
    format_bytes_ascii, format_bytes_hex, now_display_timestamp, now_file_timestamp,
};
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

pub(crate) struct CommandLogger {
    path: PathBuf,
    writer: Option<BufWriter<std::fs::File>>,
}

impl CommandLogger {
    pub(crate) fn create(command_name: &str, log_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(log_dir)?;
        let path = log_dir.join(format!("{}_{}.log", now_file_timestamp(), command_name));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            writer: Some(BufWriter::new(file)),
        })
    }

    pub(crate) fn disabled(command_name: &str) -> Self {
        Self {
            path: PathBuf::from(format!("(disabled:{command_name})")),
            writer: None,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn log_input(&mut self, port: &str, bytes: &[u8]) -> io::Result<()> {
        self.write_line("input", port, Some(bytes), None)
    }

    pub(crate) fn log_output(&mut self, port: &str, bytes: &[u8]) -> io::Result<()> {
        self.write_line("output", port, Some(bytes), None)
    }

    pub(crate) fn log_status(&mut self, port: &str, message: &str) -> io::Result<()> {
        self.write_line("status", port, None, Some(message))
    }

    fn write_line(
        &mut self,
        kind: &str,
        port: &str,
        bytes: Option<&[u8]>,
        message: Option<&str>,
    ) -> io::Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Ok(());
        };

        write!(
            writer,
            "ts={} kind={} port={}",
            now_display_timestamp(),
            kind,
            port
        )?;

        if let Some(bytes) = bytes {
            write!(
                writer,
                " hex={} ascii={}",
                format_bytes_hex(bytes),
                format_bytes_ascii(bytes)
            )?;
        }

        if let Some(message) = message {
            write!(writer, " message={message}")?;
        }

        writer.write_all(b"\n")?;
        writer.flush()
    }
}
