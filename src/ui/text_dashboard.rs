use crate::common::{
    format_bytes_ascii, format_bytes_hex, format_bytes_utf8, now_display_timestamp,
};
use crate::port_display::PortDisplayMode;
use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::fmt::Write as _;
use std::io::{self, Stdin, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

const HISTORY_LIMIT: usize = 10;
const RATE_WINDOW: Duration = Duration::from_secs(1);
const DEFAULT_STREAM_WIDTH: usize = 120;
const RESET: &str = "\x1b[0m";
const REVERSE: &str = "\x1b[7m";
const FG_CYAN: &str = "\x1b[36m";
const FG_GREEN: &str = "\x1b[32m";
const ENTER_ALTERNATE_SCREEN: &str = "\x1b[?1049h";
const LEAVE_ALTERNATE_SCREEN: &str = "\x1b[?1049l";
const CLEAR_SCREEN: &str = "\x1b[2J";
const CLEAR_TO_SCREEN_END: &str = "\x1b[J";
const CLEAR_LINE_END: &str = "\x1b[K";
const HOME_CURSOR: &str = "\x1b[H";
const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SectionKind {
    Output,
    Input,
}

struct Section {
    kind: SectionKind,
    port: String,
    status: String,
    baud_rate: Option<u32>,
    display_mode: PortDisplayMode,
    packet_rate_enabled: bool,
    known_formats: Vec<String>,
    entries: VecDeque<Entry>,
    rate_samples: VecDeque<RateSample>,
    format_rate_samples: BTreeMap<String, VecDeque<RateSample>>,
}

struct Entry {
    timestamp: String,
    bytes: Vec<u8>,
    hex: String,
    ascii: String,
    utf8: String,
    display_mode_override: Option<PortDisplayMode>,
    preserve_line_breaks: bool,
}

struct RateSample {
    at: Instant,
    byte_len: usize,
    packet_count: usize,
}

pub struct TextDashboard {
    title: String,
    header_lines: Vec<String>,
    sections: BTreeMap<(SectionKind, String), Section>,
    previous_lines: Vec<String>,
    stdout: io::Stdout,
    #[cfg(unix)]
    terminal_input_guard: Option<TerminalInputGuard>,
    interactive_mode: bool,
    input_buffer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextDashboardAction {
    TogglePause,
    InputChanged,
    Submit(String),
}

impl TextDashboard {
    pub fn new(title: impl Into<String>) -> io::Result<Self> {
        let mut stdout = io::stdout();
        #[cfg(unix)]
        let terminal_input_guard = TerminalInputGuard::new()?;
        // 代替スクリーンを使うと、終了後に元のターミナル表示へ自然に戻せる。
        write!(
            stdout,
            "{ENTER_ALTERNATE_SCREEN}{CLEAR_SCREEN}{HOME_CURSOR}{HIDE_CURSOR}"
        )?;
        stdout.flush()?;

        Ok(Self {
            title: title.into(),
            header_lines: Vec::new(),
            sections: BTreeMap::new(),
            previous_lines: Vec::new(),
            stdout,
            #[cfg(unix)]
            terminal_input_guard,
            interactive_mode: false,
            input_buffer: String::new(),
        })
    }

    pub fn set_header_lines(&mut self, lines: Vec<String>) {
        self.header_lines = lines;
    }

    pub fn set_interactive_mode(&mut self, enabled: bool) {
        self.interactive_mode = enabled;
    }

    pub fn set_output_status(&mut self, port: &str, status: impl Into<String>) {
        self.section_mut(SectionKind::Output, port).status = status.into();
    }

    pub fn set_input_status(&mut self, port: &str, status: impl Into<String>) {
        self.section_mut(SectionKind::Input, port).status = status.into();
    }

    pub fn set_output_baud_rate(&mut self, port: &str, baud_rate: u32) {
        self.section_mut(SectionKind::Output, port).baud_rate = Some(baud_rate);
    }

    pub fn set_input_baud_rate(&mut self, port: &str, baud_rate: u32) {
        self.section_mut(SectionKind::Input, port).baud_rate = Some(baud_rate);
    }

    pub fn set_output_display_mode(&mut self, port: &str, display_mode: PortDisplayMode) {
        self.section_mut(SectionKind::Output, port).display_mode = display_mode;
    }

    pub fn set_input_display_mode(&mut self, port: &str, display_mode: PortDisplayMode) {
        self.section_mut(SectionKind::Input, port).display_mode = display_mode;
    }

    pub fn set_output_packet_rate_enabled(&mut self, port: &str, enabled: bool) {
        self.section_mut(SectionKind::Output, port)
            .packet_rate_enabled = enabled;
    }

    pub fn set_input_packet_rate_enabled(&mut self, port: &str, enabled: bool) {
        self.section_mut(SectionKind::Input, port)
            .packet_rate_enabled = enabled;
    }

    pub fn record_input_bytes(&mut self, port: &str, bytes: &[u8]) {
        self.record_input_sample(port, bytes.len(), 0);
    }

    pub fn record_output_sample(&mut self, port: &str, byte_len: usize, packet_count: usize) {
        self.section_mut(SectionKind::Output, port)
            .record_rate_sample(byte_len, packet_count);
    }

    pub fn record_output_format_sample(
        &mut self,
        port: &str,
        format_name: &str,
        byte_len: usize,
        packet_count: usize,
    ) {
        self.section_mut(SectionKind::Output, port)
            .record_format_rate_sample(format_name, byte_len, packet_count);
    }

    pub fn record_input_sample(&mut self, port: &str, byte_len: usize, packet_count: usize) {
        self.section_mut(SectionKind::Input, port)
            .record_rate_sample(byte_len, packet_count);
    }

    pub fn record_input_format_sample(
        &mut self,
        port: &str,
        format_name: &str,
        byte_len: usize,
        packet_count: usize,
    ) {
        self.section_mut(SectionKind::Input, port)
            .record_format_rate_sample(format_name, byte_len, packet_count);
    }

    pub fn set_output_known_formats(&mut self, port: &str, formats: Vec<String>) {
        self.section_mut(SectionKind::Output, port)
            .set_known_formats(formats);
    }

    pub fn set_input_known_formats(&mut self, port: &str, formats: Vec<String>) {
        self.section_mut(SectionKind::Input, port)
            .set_known_formats(formats);
    }

    pub fn add_input(&mut self, port: &str, bytes: &[u8]) {
        self.add_input_with_options(port, bytes, None, false);
    }

    pub fn add_output_with_options(
        &mut self,
        port: &str,
        bytes: &[u8],
        display_mode: Option<PortDisplayMode>,
        preserve_line_breaks: bool,
    ) {
        self.section_mut(SectionKind::Output, port).push_entry(
            bytes,
            display_mode,
            preserve_line_breaks,
        );
    }

    pub fn add_input_with_options(
        &mut self,
        port: &str,
        bytes: &[u8],
        display_mode: Option<PortDisplayMode>,
        preserve_line_breaks: bool,
    ) {
        self.section_mut(SectionKind::Input, port).push_entry(
            bytes,
            display_mode,
            preserve_line_breaks,
        );
    }

    pub fn add_input_stream_wrapped(
        &mut self,
        port: &str,
        bytes: &[u8],
        display_mode: Option<PortDisplayMode>,
    ) {
        let terminal_width = terminal_width().or(Some(DEFAULT_STREAM_WIDTH));
        self.section_mut(SectionKind::Input, port)
            .push_stream_wrapped(bytes, display_mode, terminal_width);
    }

    pub fn render(&mut self, status: Option<&str>) -> io::Result<()> {
        let mut lines = Vec::new();
        let now = Instant::now();
        let terminal_width = terminal_width();
        lines.push(self.title.clone());

        for line in &self.header_lines {
            lines.push(line.clone());
        }

        lines.push(format_status_line(status));
        lines.push(String::new());

        for section in self.sections.values_mut() {
            section.prune_rate_samples(now);
            let mut heading = String::new();
            heading.push('[');
            heading.push_str(match section.kind {
                SectionKind::Output => "output",
                SectionKind::Input => "input",
            });
            heading.push_str("] ");
            heading.push_str(&section.port);
            if !section.status.is_empty() {
                heading.push_str("  ");
                heading.push_str(&section.status);
            }
            heading.push_str("  ");
            heading.push_str(&section.rate_label());
            let format_rate_label = section.format_rate_label();
            if !format_rate_label.is_empty() {
                heading.push_str("  ");
                heading.push_str(&format_rate_label);
            }
            lines.push(format_heading_line(&heading, terminal_width));

            if section.entries.is_empty() {
                lines.push(String::from("(no data)"));
            } else {
                for entry in &section.entries {
                    let display_mode = entry.display_mode_override.unwrap_or(section.display_mode);
                    if entry.preserve_line_breaks
                        && matches!(display_mode, PortDisplayMode::Ascii | PortDisplayMode::Utf8)
                    {
                        append_multiline_entry(&mut lines, &entry.timestamp, display_mode, entry);
                        continue;
                    }

                    lines.push(format_entry_line(entry, display_mode));
                }
            }

            lines.push(String::new());
        }

        if self.interactive_mode {
            lines.push(format!("> {}", self.input_buffer));
        }

        if terminal_width.is_some() {
            lines = lines
                .iter()
                .map(|line| fit_line_to_terminal_width(line, terminal_width))
                .collect();
        }

        let frame = format_screen_delta(&lines, &self.previous_lines);

        if frame.is_empty() && !self.interactive_mode {
            return Ok(());
        }

        if !frame.is_empty() {
            write!(self.stdout, "{frame}")?;
        }

        if self.interactive_mode {
            let input_row = lines.len();
            let input_col = 3 + self.input_buffer.chars().count();
            write!(self.stdout, "{SHOW_CURSOR}\x1b[{input_row};{input_col}H")?;
        }

        self.stdout.flush()?;
        if !frame.is_empty() {
            self.previous_lines = lines;
        }
        Ok(())
    }

    pub fn poll_action(&mut self) -> io::Result<Option<TextDashboardAction>> {
        #[cfg(unix)]
        {
            if let Some(guard) = self.terminal_input_guard.as_mut()
                && let Some(bytes) = guard.read_raw()?
            {
                if self.interactive_mode {
                    return Ok(process_interactive_input(&bytes, &mut self.input_buffer));
                } else {
                    let mut action = None;
                    for b in &bytes {
                        if *b == b' ' {
                            action = Some(TextDashboardAction::TogglePause);
                        }
                    }
                    return Ok(action);
                }
            }
        }

        Ok(None)
    }

    fn section_mut(&mut self, kind: SectionKind, port: &str) -> &mut Section {
        self.sections
            .entry((kind, String::from(port)))
            .or_insert_with(|| Section {
                kind,
                port: String::from(port),
                status: String::new(),
                baud_rate: None,
                display_mode: PortDisplayMode::HexUtf8,
                packet_rate_enabled: false,
                known_formats: Vec::new(),
                entries: VecDeque::new(),
                rate_samples: VecDeque::new(),
                format_rate_samples: BTreeMap::new(),
            })
    }
}

impl Drop for TextDashboard {
    fn drop(&mut self) {
        let _ = write!(self.stdout, "{SHOW_CURSOR}{LEAVE_ALTERNATE_SCREEN}");
        let _ = self.stdout.flush();
    }
}

#[cfg(unix)]
struct TerminalInputGuard {
    stdin: Stdin,
    original_termios: libc::termios,
}

#[cfg(unix)]
impl TerminalInputGuard {
    fn new() -> io::Result<Option<Self>> {
        let stdin = io::stdin();
        let fd = stdin.as_raw_fd();
        let is_tty = unsafe { libc::isatty(fd) } == 1;
        if !is_tty {
            return Ok(None);
        }

        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        let get_result = unsafe { libc::tcgetattr(fd, &mut termios) };
        if get_result != 0 {
            return Err(io::Error::last_os_error());
        }

        let original_termios = termios;
        disable_terminal_input_echo(&mut termios);

        let set_result = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) };
        if set_result != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Some(Self {
            stdin,
            original_termios,
        }))
    }

    fn read_raw(&mut self) -> io::Result<Option<Vec<u8>>> {
        let fd = self.stdin.as_raw_fd();
        let mut buffer = [0u8; 32];
        let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            return Ok(None);
        }
        Ok(Some(buffer[..count as usize].to_vec()))
    }
}

#[cfg(unix)]
impl Drop for TerminalInputGuard {
    fn drop(&mut self) {
        let _ = unsafe {
            libc::tcsetattr(
                self.stdin.as_raw_fd(),
                libc::TCSANOW,
                &self.original_termios,
            )
        };
    }
}

fn process_interactive_input(
    bytes: &[u8],
    input_buffer: &mut String,
) -> Option<TextDashboardAction> {
    let mut changed = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x0D || b == 0x0A {
            // Enter: submit and clear buffer
            let text = std::mem::take(input_buffer);
            return Some(TextDashboardAction::Submit(text));
        } else if b == 0x7F || b == 0x08 {
            // Backspace / DEL
            input_buffer.pop();
            changed = true;
            i += 1;
        } else if b < 0x20 {
            // Other control chars: skip
            i += 1;
        } else {
            // Printable ASCII or start of multi-byte UTF-8
            let seq_len = utf8_sequence_len(b);
            let end = (i + seq_len).min(bytes.len());
            if let Ok(s) = std::str::from_utf8(&bytes[i..end]) {
                for c in s.chars() {
                    input_buffer.push(c);
                }
                changed = true;
                i = end;
            } else {
                i += 1;
            }
        }
    }
    if changed {
        Some(TextDashboardAction::InputChanged)
    } else {
        None
    }
}

fn utf8_sequence_len(first_byte: u8) -> usize {
    if first_byte < 0x80 {
        1
    } else if first_byte < 0xE0 {
        2
    } else if first_byte < 0xF0 {
        3
    } else {
        4
    }
}

#[cfg(unix)]
fn disable_terminal_input_echo(termios: &mut libc::termios) {
    termios.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON);
    termios.c_cc[libc::VMIN] = 0;
    termios.c_cc[libc::VTIME] = 0;
}

impl Entry {
    fn new(
        bytes: &[u8],
        display_mode_override: Option<PortDisplayMode>,
        preserve_line_breaks: bool,
    ) -> Self {
        let mut entry = Self {
            timestamp: now_display_timestamp(),
            bytes: bytes.to_vec(),
            hex: String::new(),
            ascii: String::new(),
            utf8: String::new(),
            display_mode_override,
            preserve_line_breaks,
        };
        entry.refresh_text();
        entry
    }

    fn can_append_stream_byte(
        &self,
        byte: u8,
        display_mode: PortDisplayMode,
        display_mode_override: Option<PortDisplayMode>,
        terminal_width: Option<usize>,
    ) -> bool {
        if self.preserve_line_breaks || self.display_mode_override != display_mode_override {
            return false;
        }

        let Some(width) = terminal_width else {
            return true;
        };
        if width == 0 {
            return false;
        }

        let mut candidate = self.bytes.clone();
        candidate.push(byte);
        let line = format_entry_line_for_bytes(&self.timestamp, &candidate, display_mode);
        visible_width(&line) <= width
    }

    fn append_byte(&mut self, byte: u8) {
        self.bytes.push(byte);
        self.refresh_text();
    }

    fn refresh_text(&mut self) {
        self.hex = format_bytes_hex(&self.bytes);
        self.ascii = format_bytes_ascii(&self.bytes);
        self.utf8 = format_bytes_utf8(&self.bytes);
    }
}

impl Section {
    fn push_entry(
        &mut self,
        bytes: &[u8],
        display_mode_override: Option<PortDisplayMode>,
        preserve_line_breaks: bool,
    ) {
        // 新しいデータを先頭へ積み、各ポート直近 10 件だけを残す。
        self.entries.push_front(Entry::new(
            bytes,
            display_mode_override,
            preserve_line_breaks,
        ));
        self.entries.truncate(HISTORY_LIMIT);
    }

    fn push_stream_wrapped(
        &mut self,
        bytes: &[u8],
        display_mode_override: Option<PortDisplayMode>,
        terminal_width: Option<usize>,
    ) {
        for byte in bytes {
            let display_mode = display_mode_override.unwrap_or(self.display_mode);
            if let Some(entry) = self.entries.front_mut()
                && entry.can_append_stream_byte(
                    *byte,
                    display_mode,
                    display_mode_override,
                    terminal_width,
                )
            {
                entry.append_byte(*byte);
                continue;
            }

            self.entries
                .push_front(Entry::new(&[*byte], display_mode_override, false));
            self.entries.truncate(HISTORY_LIMIT);
        }
    }

    fn record_rate_sample(&mut self, byte_len: usize, packet_count: usize) {
        self.rate_samples.push_back(RateSample {
            at: Instant::now(),
            byte_len,
            packet_count,
        });
    }

    fn prune_rate_samples(&mut self, now: Instant) {
        while let Some(sample) = self.rate_samples.front() {
            if now.duration_since(sample.at) <= RATE_WINDOW {
                break;
            }
            self.rate_samples.pop_front();
        }

        for samples in self.format_rate_samples.values_mut() {
            while let Some(sample) = samples.front() {
                if now.duration_since(sample.at) <= RATE_WINDOW {
                    break;
                }
                samples.pop_front();
            }
        }
    }

    fn rate_label(&self) -> String {
        let bytes_per_second = self
            .rate_samples
            .iter()
            .map(|sample| sample.byte_len)
            .sum::<usize>() as f64
            / RATE_WINDOW.as_secs_f64();
        let packets_per_second = self
            .rate_samples
            .iter()
            .map(|sample| sample.packet_count)
            .sum::<usize>() as f64
            / RATE_WINDOW.as_secs_f64();
        let name = match self.kind {
            SectionKind::Output => "tx",
            SectionKind::Input => "rx",
        };
        if self.packet_rate_enabled {
            return match self.baud_rate {
                Some(baud_rate) => format!(
                    "{name}={packets_per_second:.1} Hz {} ({})",
                    format_rate(bytes_per_second),
                    format_utilization(bytes_per_second, baud_rate)
                ),
                None => format!(
                    "{name}={packets_per_second:.1} Hz {}",
                    format_rate(bytes_per_second)
                ),
            };
        }
        match self.baud_rate {
            Some(baud_rate) => format!(
                "{name}={} ({})",
                format_rate(bytes_per_second),
                format_utilization(bytes_per_second, baud_rate)
            ),
            None => format!("{name}={}", format_rate(bytes_per_second)),
        }
    }

    fn set_known_formats(&mut self, formats: Vec<String>) {
        self.known_formats.clear();
        for format in formats {
            if !self.known_formats.contains(&format) {
                self.known_formats.push(format);
            }
        }
    }

    fn record_format_rate_sample(
        &mut self,
        format_name: &str,
        byte_len: usize,
        packet_count: usize,
    ) {
        if !self
            .known_formats
            .iter()
            .any(|format| format == format_name)
        {
            self.known_formats.push(format_name.to_owned());
        }
        self.format_rate_samples
            .entry(format_name.to_owned())
            .or_default()
            .push_back(RateSample {
                at: Instant::now(),
                byte_len,
                packet_count,
            });
    }

    fn format_rate_label(&self) -> String {
        self.known_formats
            .iter()
            .filter_map(|format_name| {
                let packets_per_second = self
                    .format_rate_samples
                    .get(format_name)
                    .map(|samples| {
                        samples
                            .iter()
                            .map(|sample| sample.packet_count)
                            .sum::<usize>() as f64
                            / RATE_WINDOW.as_secs_f64()
                    })
                    .or_else(|| {
                        (self.known_formats.len() == 1).then(|| {
                            self.rate_samples
                                .iter()
                                .map(|sample| sample.packet_count)
                                .sum::<usize>() as f64
                                / RATE_WINDOW.as_secs_f64()
                        })
                    })
                    .unwrap_or_default();
                if self.packet_rate_enabled || packets_per_second > 0.0 {
                    Some(format!("{format_name} {packets_per_second:.1}Hz"))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn format_rate(bytes_per_second: f64) -> String {
    if bytes_per_second >= 1024.0 * 1024.0 {
        return format!("{:.1} MiB/s", bytes_per_second / (1024.0 * 1024.0));
    }

    if bytes_per_second >= 1024.0 {
        return format!("{:.1} KiB/s", bytes_per_second / 1024.0);
    }

    format!("{bytes_per_second:.0} B/s")
}

fn format_utilization(bytes_per_second: f64, baud_rate: u32) -> String {
    if baud_rate == 0 {
        return String::from("n/a");
    }

    // 一般的な 8N1 を前提に、1 byte = 10 bit として使用率を見積もる。
    let theoretical_bytes_per_second = baud_rate as f64 / 10.0;
    let utilization = if theoretical_bytes_per_second > 0.0 {
        (bytes_per_second / theoretical_bytes_per_second) * 100.0
    } else {
        0.0
    };

    format!("{utilization:.0}%")
}

fn format_heading_line(text: &str, terminal_width: Option<usize>) -> String {
    let visible_width = text.chars().count();
    let padded_width = terminal_width.unwrap_or(visible_width).max(visible_width);
    let padding = padded_width.saturating_sub(visible_width);

    let mut line = String::with_capacity(text.len() + padding + REVERSE.len() + RESET.len());
    line.push_str(REVERSE);
    line.push_str(text);
    for _ in 0..padding {
        line.push(' ');
    }
    line.push_str(RESET);
    line
}

fn format_status_line(status: Option<&str>) -> String {
    match status {
        Some(status) => format!("status: {status}"),
        None => String::from("status: running"),
    }
}

fn paint_text(text: &str, color: &str) -> String {
    format!("{color}{text}{RESET}")
}

fn format_entry_line(entry: &Entry, display_mode: PortDisplayMode) -> String {
    let mut line = String::new();
    line.push_str(&entry.timestamp);
    line.push_str(" | ");
    line.push_str(&format_entry_body(
        &entry.hex,
        &entry.ascii,
        &entry.utf8,
        display_mode,
    ));
    line
}

fn format_entry_line_for_bytes(
    timestamp: &str,
    bytes: &[u8],
    display_mode: PortDisplayMode,
) -> String {
    let mut line = String::new();
    line.push_str(timestamp);
    line.push_str(" | ");
    line.push_str(&format_entry_body(
        &format_bytes_hex(bytes),
        &format_bytes_ascii(bytes),
        &format_bytes_utf8(bytes),
        display_mode,
    ));
    line
}

fn format_entry_body(hex: &str, ascii: &str, utf8: &str, display_mode: PortDisplayMode) -> String {
    match display_mode {
        PortDisplayMode::Hex => hex.to_owned(),
        PortDisplayMode::Ascii => paint_text(ascii, FG_CYAN),
        PortDisplayMode::Utf8 => paint_text(utf8, FG_GREEN),
        PortDisplayMode::HexAscii => {
            format!("{hex} | {}", paint_text(ascii, FG_CYAN))
        }
        PortDisplayMode::HexUtf8 => {
            format!("{hex} | {}", paint_text(utf8, FG_GREEN))
        }
    }
}

fn append_multiline_entry(
    lines: &mut Vec<String>,
    timestamp: &str,
    display_mode: PortDisplayMode,
    entry: &Entry,
) {
    let raw_lines = match display_mode {
        PortDisplayMode::Ascii => bytes_text_lines(&entry.ascii, true),
        PortDisplayMode::Utf8 => bytes_text_lines(&entry.utf8, false),
        _ => unreachable!("multiline entry only supports ascii/utf8"),
    };
    let prefix = format!("{timestamp} | ");
    let continuation_prefix = format!("{} | ", " ".repeat(timestamp.chars().count()));
    let color = match display_mode {
        PortDisplayMode::Ascii => FG_CYAN,
        PortDisplayMode::Utf8 => FG_GREEN,
        _ => RESET,
    };

    if raw_lines.is_empty() {
        lines.push(format!("{prefix}{}", paint_text("\"\"", color)));
        return;
    }

    for (index, line) in raw_lines.iter().enumerate() {
        let prefix = if index == 0 {
            prefix.as_str()
        } else {
            continuation_prefix.as_str()
        };
        lines.push(format!("{prefix}{}", paint_text(line, color)));
    }
}

fn bytes_text_lines(text: &str, ascii_wrapped: bool) -> Vec<String> {
    let raw = if ascii_wrapped || text.starts_with('"') && text.ends_with('"') {
        text.trim_matches('"')
    } else {
        text
    };
    let normalized = raw
        .replace("\\r\\n", "\n")
        .replace("\\n", "\n")
        .replace("\\r", "\n");
    normalized
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(|line| line.to_owned())
        .collect()
}

fn visible_width(line: &str) -> usize {
    let mut width = 0usize;
    let mut chars = line.chars();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if let Some(next) = chars.next()
                && next == '['
            {
                for csi_ch in chars.by_ref() {
                    if ('@'..='~').contains(&csi_ch) {
                        break;
                    }
                }
            }
            continue;
        }

        width += 1;
    }

    width
}

fn fit_line_to_terminal_width(line: &str, terminal_width: Option<usize>) -> String {
    let Some(max_width) = terminal_width else {
        return line.to_owned();
    };
    if max_width == 0 {
        return String::new();
    }

    let mut output = String::new();
    let mut visible_width = 0usize;
    let mut chars = line.chars();
    let mut saw_escape = false;
    let mut truncated = false;

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            saw_escape = true;
            output.push(ch);
            if let Some(next) = chars.next() {
                output.push(next);
                if next == '[' {
                    for csi_ch in chars.by_ref() {
                        output.push(csi_ch);
                        if ('@'..='~').contains(&csi_ch) {
                            break;
                        }
                    }
                }
            }
            continue;
        }

        if visible_width >= max_width {
            truncated = true;
            break;
        }

        output.push(ch);
        visible_width += 1;
    }

    if truncated && saw_escape {
        output.push_str(RESET);
    }
    output
}

fn format_screen_delta(lines: &[String], previous_lines: &[String]) -> String {
    let mut frame = String::new();

    for (index, line) in lines.iter().enumerate() {
        if previous_lines.get(index) == Some(line) {
            continue;
        }

        push_cursor_to_line_start(&mut frame, index + 1);
        frame.push_str(line);
        frame.push_str(CLEAR_LINE_END);
    }

    if lines.len() < previous_lines.len() {
        push_cursor_to_line_start(&mut frame, lines.len() + 1);
        frame.push_str(CLEAR_TO_SCREEN_END);
    }

    frame
}

fn push_cursor_to_line_start(frame: &mut String, row: usize) {
    if row <= 1 {
        frame.push_str(HOME_CURSOR);
        return;
    }

    let _ = write!(frame, "\x1b[{row};1H");
}

fn terminal_width() -> Option<usize> {
    if let Ok(columns) = env::var("COLUMNS")
        && let Ok(width) = columns.parse::<usize>()
        && width > 0
    {
        return Some(width);
    }

    terminal_width_from_ioctl()
}

#[cfg(unix)]
fn terminal_width_from_ioctl() -> Option<usize> {
    use std::ffi::c_int;
    use std::ffi::c_ulong;
    use std::mem::MaybeUninit;

    #[repr(C)]
    struct WinSize {
        ws_row: u16,
        ws_col: u16,
        ws_xpixel: u16,
        ws_ypixel: u16,
    }

    unsafe extern "C" {
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    }

    #[cfg(target_os = "macos")]
    const TIOCGWINSZ: c_ulong = 0x4008_7468;
    #[cfg(not(target_os = "macos"))]
    const TIOCGWINSZ: c_ulong = 0x5413;

    let stdout = io::stdout();
    let fd = stdout.as_raw_fd();
    let mut winsize = MaybeUninit::<WinSize>::uninit();
    let result = unsafe { ioctl(fd, TIOCGWINSZ, winsize.as_mut_ptr()) };
    if result != 0 {
        return None;
    }

    let winsize = unsafe { winsize.assume_init() };
    if winsize.ws_col == 0 {
        return None;
    }

    Some(usize::from(winsize.ws_col))
}

#[cfg(not(unix))]
fn terminal_width_from_ioctl() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::{
        CLEAR_LINE_END, CLEAR_TO_SCREEN_END, Entry, FG_CYAN, FG_GREEN, HOME_CURSOR, RESET, REVERSE,
        RateSample, Section, SectionKind, append_multiline_entry, bytes_text_lines,
        fit_line_to_terminal_width, format_heading_line, format_screen_delta, format_status_line,
        paint_text,
    };
    use crate::port_display::PortDisplayMode;
    use std::collections::{BTreeMap, VecDeque};
    use std::time::Instant;

    #[test]
    fn format_heading_line_pads_to_terminal_width() {
        let line = format_heading_line("[input] tty  baud=115200", Some(30));
        assert_eq!(
            line,
            format!("{REVERSE}[input] tty  baud=115200      {RESET}")
        );
    }

    #[test]
    fn format_heading_line_keeps_long_text() {
        let line = format_heading_line("[input] tty", Some(4));
        assert_eq!(line, format!("{REVERSE}[input] tty{RESET}"));
    }

    #[test]
    fn format_screen_delta_writes_initial_lines() {
        let frame = format_screen_delta(&[String::from("title"), String::from("body")], &[]);
        assert_eq!(
            frame,
            format!("{HOME_CURSOR}title{CLEAR_LINE_END}\x1b[2;1Hbody{CLEAR_LINE_END}")
        );
    }

    #[test]
    fn format_status_line_reserves_a_line_when_empty() {
        assert_eq!(format_status_line(None), "status: running");
        assert_eq!(
            format_status_line(Some("paused (space: resume)")),
            "status: paused (space: resume)"
        );
    }

    #[test]
    fn format_screen_delta_updates_only_changed_lines() {
        let frame = format_screen_delta(
            &[
                String::from("title"),
                String::from("status: paused"),
                String::from("body"),
            ],
            &[
                String::from("title"),
                String::from("status: running"),
                String::from("body"),
            ],
        );
        assert_eq!(frame, format!("\x1b[2;1Hstatus: paused{CLEAR_LINE_END}"));
    }

    #[test]
    fn format_screen_delta_clears_removed_trailing_lines() {
        let frame = format_screen_delta(
            &[String::from("title")],
            &[String::from("title"), String::from("body")],
        );
        assert_eq!(frame, format!("\x1b[2;1H{CLEAR_TO_SCREEN_END}"));
    }

    #[test]
    fn rate_label_shows_hz_when_packet_rate_enabled() {
        let section = Section {
            kind: SectionKind::Output,
            port: String::from("tty"),
            status: String::new(),
            baud_rate: Some(115_200),
            display_mode: PortDisplayMode::HexUtf8,
            packet_rate_enabled: true,
            known_formats: Vec::new(),
            entries: VecDeque::new(),
            rate_samples: VecDeque::from([RateSample {
                at: Instant::now(),
                byte_len: 39,
                packet_count: 2,
            }]),
            format_rate_samples: BTreeMap::new(),
        };

        let label = section.rate_label();
        assert!(label.contains("2.0 Hz"));
        assert!(label.contains("39 B/s"));
    }

    #[test]
    fn paint_text_wraps_ascii_and_utf8_colors() {
        assert_eq!(
            paint_text("\"abc\"", FG_CYAN),
            format!("{FG_CYAN}\"abc\"{RESET}")
        );
        assert_eq!(
            paint_text("\"abc\"", FG_GREEN),
            format!("{FG_GREEN}\"abc\"{RESET}")
        );
    }

    #[test]
    fn fit_line_to_terminal_width_truncates_plain_lines() {
        assert_eq!(fit_line_to_terminal_width("0123456789", Some(4)), "0123");
    }

    #[test]
    fn fit_line_to_terminal_width_preserves_ansi_reset() {
        let line = format!("{FG_CYAN}0123456789{RESET}");

        assert_eq!(
            fit_line_to_terminal_width(&line, Some(4)),
            format!("{FG_CYAN}0123{RESET}")
        );
    }

    #[test]
    fn stream_wrapped_input_fills_until_terminal_width() {
        let mut section = Section {
            kind: SectionKind::Input,
            port: String::from("tty"),
            status: String::new(),
            baud_rate: Some(115_200),
            display_mode: PortDisplayMode::Hex,
            packet_rate_enabled: false,
            known_formats: Vec::new(),
            entries: VecDeque::new(),
            rate_samples: VecDeque::new(),
            format_rate_samples: BTreeMap::new(),
        };

        section.push_stream_wrapped(&[1, 2, 3, 4, 5], Some(PortDisplayMode::Hex), Some(30));

        assert_eq!(section.entries.len(), 2);
        assert_eq!(section.entries[0].bytes, vec![4, 5]);
        assert_eq!(section.entries[1].bytes, vec![1, 2, 3]);
    }

    #[test]
    fn format_rate_label_lists_known_formats() {
        let mut format_samples = BTreeMap::new();
        format_samples.insert(
            String::from("PacketACv6"),
            VecDeque::from([RateSample {
                at: Instant::now(),
                byte_len: 39,
                packet_count: 2,
            }]),
        );
        format_samples.insert(
            String::from("RoverUpGeneral"),
            VecDeque::from([RateSample {
                at: Instant::now(),
                byte_len: 110,
                packet_count: 1,
            }]),
        );
        let section = Section {
            kind: SectionKind::Input,
            port: String::from("tty"),
            status: String::new(),
            baud_rate: Some(115_200),
            display_mode: PortDisplayMode::Ascii,
            packet_rate_enabled: true,
            known_formats: vec![String::from("PacketACv6"), String::from("RoverUpGeneral")],
            entries: VecDeque::new(),
            rate_samples: VecDeque::new(),
            format_rate_samples: format_samples,
        };

        let label = section.format_rate_label();
        assert!(label.contains("PacketACv6 2.0Hz"));
        assert!(label.contains("RoverUpGeneral 1.0Hz"));
    }

    #[test]
    fn multiline_ascii_entries_split_on_newlines() {
        assert_eq!(
            bytes_text_lines("\"0x300,000\\r\\n0x310,180\\r\\n\"", true),
            vec![String::from("0x300,000"), String::from("0x310,180")]
        );

        let entry = Entry {
            timestamp: String::from("2026-04-21 12:34:56"),
            bytes: Vec::new(),
            hex: String::new(),
            ascii: String::from("\"0x300,000\\r\\n0x310,180\\r\\n\""),
            utf8: String::new(),
            display_mode_override: Some(PortDisplayMode::Ascii),
            preserve_line_breaks: true,
        };
        let mut lines = Vec::new();
        append_multiline_entry(&mut lines, &entry.timestamp, PortDisplayMode::Ascii, &entry);

        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("0x300,000"));
        assert!(lines[1].contains("0x310,180"));
    }
}
