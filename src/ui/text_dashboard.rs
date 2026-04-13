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
const RESET: &str = "\x1b[0m";
const REVERSE: &str = "\x1b[7m";
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
    entries: VecDeque<Entry>,
    rate_samples: VecDeque<RateSample>,
}

struct Entry {
    timestamp: String,
    hex: String,
    ascii: String,
    utf8: String,
}

struct RateSample {
    at: Instant,
    byte_len: usize,
}

pub struct TextDashboard {
    title: String,
    header_lines: Vec<String>,
    sections: BTreeMap<(SectionKind, String), Section>,
    previous_lines: Vec<String>,
    stdout: io::Stdout,
    #[cfg(unix)]
    terminal_input_guard: Option<TerminalInputGuard>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDashboardAction {
    TogglePause,
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
        })
    }

    pub fn set_header_lines(&mut self, lines: Vec<String>) {
        self.header_lines = lines;
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

    pub fn record_output_bytes(&mut self, port: &str, bytes: &[u8]) {
        self.section_mut(SectionKind::Output, port)
            .record_rate_sample(bytes.len());
    }

    pub fn record_input_bytes(&mut self, port: &str, bytes: &[u8]) {
        self.section_mut(SectionKind::Input, port)
            .record_rate_sample(bytes.len());
    }

    pub fn add_output(&mut self, port: &str, bytes: &[u8]) {
        self.section_mut(SectionKind::Output, port)
            .push_entry(bytes);
    }

    pub fn add_input(&mut self, port: &str, bytes: &[u8]) {
        self.section_mut(SectionKind::Input, port).push_entry(bytes);
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
            lines.push(format_heading_line(&heading, terminal_width));

            if section.entries.is_empty() {
                lines.push(String::from("(no data)"));
            } else {
                for entry in &section.entries {
                    let mut line = String::new();
                    line.push_str(&entry.timestamp);
                    line.push_str(" | ");
                    match section.display_mode {
                        PortDisplayMode::Hex => {
                            line.push_str(&entry.hex);
                        }
                        PortDisplayMode::Ascii => {
                            line.push_str(&entry.ascii);
                        }
                        PortDisplayMode::Utf8 => {
                            line.push_str(&entry.utf8);
                        }
                        PortDisplayMode::HexAscii => {
                            line.push_str(&entry.hex);
                            line.push_str(" | ");
                            line.push_str(&entry.ascii);
                        }
                        PortDisplayMode::HexUtf8 => {
                            line.push_str(&entry.hex);
                            line.push_str(" | ");
                            line.push_str(&entry.utf8);
                        }
                    }
                    lines.push(line);
                }
            }

            lines.push(String::new());
        }

        let frame = format_screen_delta(&lines, &self.previous_lines);
        if frame.is_empty() {
            return Ok(());
        }

        write!(self.stdout, "{frame}")?;
        self.stdout.flush().inspect(|_| self.previous_lines = lines)
    }

    pub fn poll_action(&mut self) -> io::Result<Option<TextDashboardAction>> {
        #[cfg(unix)]
        {
            if let Some(guard) = self.terminal_input_guard.as_mut() {
                return guard.poll_action();
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
                entries: VecDeque::new(),
                rate_samples: VecDeque::new(),
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

    fn poll_action(&mut self) -> io::Result<Option<TextDashboardAction>> {
        let fd = self.stdin.as_raw_fd();
        let mut buffer = [0u8; 32];
        let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            return Ok(None);
        }

        let mut action = None;
        for byte in &buffer[..count as usize] {
            if *byte == b' ' {
                action = Some(TextDashboardAction::TogglePause);
            }
        }

        Ok(action)
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

#[cfg(unix)]
fn disable_terminal_input_echo(termios: &mut libc::termios) {
    termios.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON);
    termios.c_cc[libc::VMIN] = 0;
    termios.c_cc[libc::VTIME] = 0;
}

impl Section {
    fn push_entry(&mut self, bytes: &[u8]) {
        // 新しいデータを先頭へ積み、各ポート直近 10 件だけを残す。
        self.entries.push_front(Entry {
            timestamp: now_display_timestamp(),
            hex: format_bytes_hex(bytes),
            ascii: format_bytes_ascii(bytes),
            utf8: format_bytes_utf8(bytes),
        });
        self.entries.truncate(HISTORY_LIMIT);
    }

    fn record_rate_sample(&mut self, byte_len: usize) {
        self.rate_samples.push_back(RateSample {
            at: Instant::now(),
            byte_len,
        });
    }

    fn prune_rate_samples(&mut self, now: Instant) {
        while let Some(sample) = self.rate_samples.front() {
            if now.duration_since(sample.at) <= RATE_WINDOW {
                break;
            }
            self.rate_samples.pop_front();
        }
    }

    fn rate_label(&self) -> String {
        let bytes_per_second = self
            .rate_samples
            .iter()
            .map(|sample| sample.byte_len)
            .sum::<usize>() as f64
            / RATE_WINDOW.as_secs_f64();
        let name = match self.kind {
            SectionKind::Output => "tx",
            SectionKind::Input => "rx",
        };
        match self.baud_rate {
            Some(baud_rate) => format!(
                "{name}={} ({})",
                format_rate(bytes_per_second),
                format_utilization(bytes_per_second, baud_rate)
            ),
            None => format!("{name}={}", format_rate(bytes_per_second)),
        }
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
        CLEAR_LINE_END, CLEAR_TO_SCREEN_END, HOME_CURSOR, RESET, REVERSE, format_heading_line,
        format_screen_delta, format_status_line,
    };

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
}
