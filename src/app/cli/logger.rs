use super::common::{
    format_bytes_ascii, format_bytes_hex, now_display_timestamp, now_file_timestamp,
};
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

pub(crate) struct CommandLogger {
    path: PathBuf,
    writer: BufWriter<std::fs::File>,
}

impl CommandLogger {
    pub(crate) fn create(command_name: &str, log_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(log_dir)?;
        let path = log_dir.join(format!("{}_{}.log", now_file_timestamp(), command_name));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            writer: BufWriter::new(file),
        })
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
        write!(
            self.writer,
            "ts={} kind={} port={}",
            now_display_timestamp(),
            kind,
            port
        )?;

        if let Some(bytes) = bytes {
            write!(
                self.writer,
                " hex={} ascii={}",
                format_bytes_hex(bytes),
                format_bytes_ascii(bytes)
            )?;
        }

        if let Some(message) = message {
            write!(self.writer, " message={message}")?;
        }

        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }
}
