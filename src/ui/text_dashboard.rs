use crate::common::{format_bytes_ascii, format_bytes_hex, now_display_timestamp};
use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};

const HISTORY_LIMIT: usize = 10;
const RESET: &str = "\x1b[0m";
const REVERSE: &str = "\x1b[7m";
const ENTER_ALTERNATE_SCREEN: &str = "\x1b[?1049h";
const LEAVE_ALTERNATE_SCREEN: &str = "\x1b[?1049l";
const CLEAR_SCREEN: &str = "\x1b[2J";
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
    entries: VecDeque<Entry>,
}

struct Entry {
    timestamp: String,
    hex: String,
    ascii: String,
}

pub struct TextDashboard {
    title: String,
    header_lines: Vec<String>,
    sections: BTreeMap<(SectionKind, String), Section>,
    stdout: io::Stdout,
}

impl TextDashboard {
    pub fn new(title: impl Into<String>) -> io::Result<Self> {
        let mut stdout = io::stdout();
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
            stdout,
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

    pub fn add_output(&mut self, port: &str, bytes: &[u8]) {
        push_entry(self.section_mut(SectionKind::Output, port), bytes);
    }

    pub fn add_input(&mut self, port: &str, bytes: &[u8]) {
        push_entry(self.section_mut(SectionKind::Input, port), bytes);
    }

    pub fn render(&mut self, status: Option<&str>) -> io::Result<()> {
        let mut screen = String::new();
        screen.push_str(&self.title);
        screen.push('\n');

        for line in &self.header_lines {
            screen.push_str(line);
            screen.push('\n');
        }

        if let Some(status) = status {
            screen.push_str("status: ");
            screen.push_str(status);
            screen.push('\n');
        }

        screen.push('\n');

        for section in self.sections.values() {
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
            screen.push_str(REVERSE);
            screen.push_str(&heading);
            screen.push_str(RESET);
            screen.push('\n');

            if section.entries.is_empty() {
                screen.push_str("(no data)\n");
            } else {
                for entry in &section.entries {
                    screen.push_str(&entry.timestamp);
                    screen.push_str(" | ");
                    screen.push_str(&entry.hex);
                    screen.push_str(" | ");
                    screen.push_str(&entry.ascii);
                    screen.push('\n');
                }
            }

            screen.push('\n');
        }

        write!(self.stdout, "{CLEAR_SCREEN}{HOME_CURSOR}{screen}")?;
        self.stdout.flush()
    }

    fn section_mut(&mut self, kind: SectionKind, port: &str) -> &mut Section {
        self.sections
            .entry((kind, String::from(port)))
            .or_insert_with(|| Section {
                kind,
                port: String::from(port),
                status: String::new(),
                entries: VecDeque::new(),
            })
    }
}

impl Drop for TextDashboard {
    fn drop(&mut self) {
        let _ = write!(self.stdout, "{SHOW_CURSOR}{LEAVE_ALTERNATE_SCREEN}");
        let _ = self.stdout.flush();
    }
}

fn push_entry(section: &mut Section, bytes: &[u8]) {
    // 新しいデータを先頭へ積み、各ポート直近 10 件だけを残す。
    section.entries.push_front(Entry {
        timestamp: now_display_timestamp(),
        hex: format_bytes_hex(bytes),
        ascii: format_bytes_ascii(bytes),
    });
    section.entries.truncate(HISTORY_LIMIT);
}
