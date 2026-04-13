use crate::input::compact::CompactReport;
use std::env;
use std::fmt::Write as _;
use std::io::{self, Stdin, Stdout, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;

const RESET: &str = "\x1b[0m";
const WHITE: &str = "\x1b[37m";
const GREEN: &str = "\x1b[32m";
const GRAY: &str = "\x1b[90m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const ENTER_ALTERNATE_SCREEN: &str = "\x1b[?1049h";
const LEAVE_ALTERNATE_SCREEN: &str = "\x1b[?1049l";
const CLEAR_SCREEN: &str = "\x1b[2J";
const HOME_CURSOR: &str = "\x1b[H";
const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";

const CANVAS_WIDTH: usize = 125;
const CANVAS_HEIGHT: usize = 44;
const FULL_MONITOR_CONTROLLER_WIDTH: usize = 116;
const FULL_MONITOR_LOG_GAP: usize = 2;
const FULL_MONITOR_MIN_LOG_WIDTH: usize = 6;
const FULL_MONITOR_SHIFT_X: usize = 2;
const HID_HISTORY_LIMIT: usize = 5;
const TX_HISTORY_LIMIT: usize = 10;
const RX_HISTORY_LIMIT: usize = 10;
const LEFT_PAD_CENTER_X: usize = 22;
const LEFT_STICK_CENTER_X: usize = 40;
const CENTER_X: usize = 62;
const RIGHT_STICK_CENTER_X: usize = 84;
const RIGHT_FACE_CENTER_X: usize = 102;
const STICK_PIXEL_DIAMETER: usize = 33;
const STICK_POINTER_CHAR_WIDTH: usize = 15;
const STICK_POINTER_CHAR_HEIGHT: usize = 7;
const TOUCHPAD_PIXEL_WIDTH: usize = 54;
const TOUCHPAD_PIXEL_HEIGHT: usize = 33;
const TOUCHPAD_MAX_X: u16 = 1919;
const TOUCHPAD_MAX_Y: u16 = 942;
const BATTERY_PIXEL_WIDTH: usize = 28;
const BATTERY_PIXEL_HEIGHT: usize = 12;
const BATTERY_CAP_WIDTH: usize = 2;
const SENSOR_ROW_WIDTH: usize = 22;
const SENSOR_BAR_HALF_WIDTH: usize = 4;
const GYRO_BAR_MAX: i16 = 4096;
const ACCEL_BAR_MAX: i16 = 8192;
const BATTERY_BODY_CELL_WIDTH: usize = (BATTERY_PIXEL_WIDTH - BATTERY_CAP_WIDTH).div_ceil(2);
const BATTERY_CAP_CELL_X: usize = BATTERY_BODY_CELL_WIDTH;
const BATTERY_CELL_HEIGHT: usize = BATTERY_PIXEL_HEIGHT.div_ceil(4);
const BATTERY_OUTLINE_CHAR_WIDTH: usize = BATTERY_BODY_CELL_WIDTH + 3;
const BATTERY_OUTLINE_CHAR_HEIGHT: usize = BATTERY_CELL_HEIGHT + 2;
const BRAILLE_LEFT_COLUMN_BITS: u8 = 0x47;
const BRAILLE_RIGHT_COLUMN_BITS: u8 = 0xB8;
const BRAILLE_TOP_ROW_BITS: u8 = 0x09;
const BRAILLE_BOTTOM_ROW_BITS: u8 = 0xC0;
const BRAILLE_FULL_CELL_BITS: u8 = 0xFF;
const POINTER_CELL_CHAR: &str = "⣿";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    Full,
    Raw,
    Compact,
    None,
}

impl DisplayMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "graphic" => Ok(Self::Full),
            "raw" => Ok(Self::Raw),
            "compact" => Ok(Self::Compact),
            "none" => Ok(Self::None),
            other => Err(format!(
                "invalid monitor mode: {other} (expected graphic/raw/compact/none)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "graphic",
            Self::Raw => "raw",
            Self::Compact => "compact",
            Self::None => "none",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Full => {
                "DS4 Live Monitor                         Space pause  q quit  Ctrl-C exit"
            }
            Self::Raw => {
                "DS4 Raw HID Monitor                      Space pause  q quit  Ctrl-C exit"
            }
            Self::Compact => {
                "DS4 Compact Monitor                  Space pause  q quit  Ctrl-C exit"
            }
            Self::None => "DS4 Output                            Space pause  q quit  Ctrl-C exit",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MonitorFrame {
    pub sequence: u64,
    pub transport: &'static str,
    pub report_len: usize,
    pub device_name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface_number: i32,
    pub raw_report: Vec<u8>,
    pub compact: CompactReport,
    pub output_format: Option<String>,
    pub output_state: Option<String>,
    pub output_bytes: Vec<u8>,
    pub port_rx_state: Option<String>,
    pub port_rx_bytes: Vec<u8>,
    pub hid_history: Vec<Vec<u8>>,
    pub tx_history: Vec<Vec<u8>>,
    pub rx_history: Vec<String>,
}

impl MonitorFrame {
    pub fn idle() -> Self {
        Self {
            sequence: 0,
            transport: "waiting",
            report_len: 0,
            device_name: String::from("unknown"),
            vendor_id: 0,
            product_id: 0,
            interface_number: 0,
            raw_report: Vec::new(),
            compact: [0, 0, 128, 128, 128, 128, 0, 0],
            output_format: None,
            output_state: None,
            output_bytes: Vec::new(),
            port_rx_state: None,
            port_rx_bytes: Vec::new(),
            hid_history: Vec::new(),
            tx_history: Vec::new(),
            rx_history: Vec::new(),
        }
    }

    pub fn push_hid_history(&mut self, bytes: Vec<u8>) {
        self.hid_history.insert(0, bytes);
        self.hid_history.truncate(HID_HISTORY_LIMIT);
    }

    pub fn push_tx_history(&mut self, bytes: Vec<u8>) {
        self.tx_history.insert(0, bytes);
        self.tx_history.truncate(TX_HISTORY_LIMIT);
    }

    pub fn push_rx_history(&mut self, text: String) {
        self.rx_history.insert(0, text);
        self.rx_history.truncate(RX_HISTORY_LIMIT);
    }
}

pub struct MonitorUi {
    mode: DisplayMode,
    stdout: Stdout,
    #[cfg(unix)]
    _terminal_input_guard: Option<TerminalInputGuard>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorAction {
    TogglePause,
    Quit,
}

impl MonitorUi {
    pub fn new(mode: DisplayMode) -> io::Result<Self> {
        let mut stdout = io::stdout();
        #[cfg(unix)]
        let terminal_input_guard = TerminalInputGuard::new()?;
        write!(
            stdout,
            "{ENTER_ALTERNATE_SCREEN}{CLEAR_SCREEN}{HOME_CURSOR}{HIDE_CURSOR}"
        )?;
        stdout.flush()?;
        Ok(Self {
            mode,
            stdout,
            #[cfg(unix)]
            _terminal_input_guard: terminal_input_guard,
        })
    }

    pub fn render(&mut self, frame: &MonitorFrame, status: Option<&str>) -> io::Result<()> {
        let mut screen = String::new();
        let content_width = match self.mode {
            DisplayMode::Full => terminal_size()
                .map(|(width, _)| width)
                .unwrap_or(CANVAS_WIDTH),
            _ => CANVAS_WIDTH,
        };

        writeln!(
            screen,
            "{}",
            pad_visible_line(self.mode.title(), content_width)
        )
        .expect("writing to String should not fail");
        writeln!(screen).expect("writing to String should not fail");

        match self.mode {
            DisplayMode::Full => {
                let panel_width =
                    full_monitor_panel_width(content_width).max(FULL_MONITOR_MIN_LOG_WIDTH);
                write_full_monitor_header(&mut screen, frame, status, content_width);
                render_full_monitor(&mut screen, frame, panel_width);
            }
            DisplayMode::Raw => {
                write_common_monitor_lines(&mut screen, frame, status, self.mode);
                render_raw_monitor(&mut screen, frame);
            }
            DisplayMode::Compact => {
                write_common_monitor_lines(&mut screen, frame, status, self.mode);
                render_compact_monitor(&mut screen, frame);
            }
            DisplayMode::None => {}
        }

        let fitted_screen = fit_screen_to_terminal(&screen, content_width);
        write!(self.stdout, "\x1b[H{fitted_screen}\x1b[J")?;
        self.stdout.flush()
    }

    pub fn poll_action(&mut self) -> io::Result<Option<MonitorAction>> {
        #[cfg(unix)]
        {
            if let Some(guard) = self._terminal_input_guard.as_mut() {
                return guard.poll_action();
            }
        }

        Ok(None)
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

    fn poll_action(&mut self) -> io::Result<Option<MonitorAction>> {
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
            match monitor_action_from_byte(*byte) {
                Some(MonitorAction::Quit) => return Ok(Some(MonitorAction::Quit)),
                Some(MonitorAction::TogglePause) => action = Some(MonitorAction::TogglePause),
                None => {}
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

fn monitor_action_from_byte(byte: u8) -> Option<MonitorAction> {
    match byte {
        b' ' => Some(MonitorAction::TogglePause),
        b'q' | b'Q' => Some(MonitorAction::Quit),
        _ => None,
    }
}

impl Drop for MonitorUi {
    fn drop(&mut self) {
        let _ = write!(
            self.stdout,
            "{RESET}{SHOW_CURSOR}{LEAVE_ALTERNATE_SCREEN}{SHOW_CURSOR}"
        );
        let _ = self.stdout.flush();
    }
}

fn render_full_monitor(screen: &mut String, frame: &MonitorFrame, panel_width: usize) {
    let view = CompactView::new(frame.compact);
    let touchpad_positions = if frame.transport == "usb" {
        usb_touch_positions(&frame.raw_report)
    } else {
        [None, None]
    };
    let battery_status = usb_battery_status(&frame.raw_report);
    let sensor_readings = usb_sensor_readings(&frame.raw_report);
    let mut canvas = Canvas::new(FULL_MONITOR_CONTROLLER_WIDTH, CANVAS_HEIGHT);
    let left_pad_center_x = LEFT_PAD_CENTER_X - FULL_MONITOR_SHIFT_X;
    let left_stick_center_x = LEFT_STICK_CENTER_X - FULL_MONITOR_SHIFT_X;
    let center_x = CENTER_X - FULL_MONITOR_SHIFT_X;
    let right_stick_center_x = RIGHT_STICK_CENTER_X - FULL_MONITOR_SHIFT_X;
    let right_face_center_x = RIGHT_FACE_CENTER_X - FULL_MONITOR_SHIFT_X;

    draw_bitmap_trigger_button(&mut canvas, left_pad_center_x, 0, "L2", view.l2_raw());
    draw_bitmap_trigger_button(&mut canvas, right_face_center_x, 0, "R2", view.r2_raw());
    draw_bitmap_shoulder_button(&mut canvas, left_pad_center_x, 6, "L1", view.l1_pressed());
    draw_bitmap_shoulder_button(&mut canvas, right_face_center_x, 6, "R1", view.r1_pressed());
    draw_bitmap_battery(&mut canvas, center_x, 0, battery_status);

    draw_dot_pattern(
        &mut canvas,
        left_pad_center_x,
        13,
        dpad_up_bitmap(),
        view.up_pressed(),
    );
    draw_dot_pattern(
        &mut canvas,
        left_pad_center_x.saturating_sub(6),
        16,
        dpad_left_bitmap(),
        view.left_pressed(),
    );
    draw_dot_pattern(
        &mut canvas,
        left_pad_center_x + 6,
        16,
        dpad_right_bitmap(),
        view.right_pressed(),
    );
    draw_dot_pattern(
        &mut canvas,
        left_pad_center_x,
        19,
        dpad_down_bitmap(),
        view.down_pressed(),
    );

    draw_dot_pattern(
        &mut canvas,
        right_face_center_x,
        12,
        triangle_bitmap(),
        view.triangle_pressed(),
    );
    draw_dot_pattern(
        &mut canvas,
        right_face_center_x - 9,
        16,
        square_bitmap(),
        view.square_pressed(),
    );
    draw_dot_pattern(
        &mut canvas,
        right_face_center_x + 9,
        16,
        circle_bitmap(),
        view.circle_pressed(),
    );
    draw_dot_pattern(
        &mut canvas,
        right_face_center_x,
        20,
        cross_bitmap(),
        view.cross_pressed(),
    );

    draw_bitmap_touchpad(
        &mut canvas,
        center_x,
        12,
        view.trackpad_pressed(),
        touchpad_positions,
        TOUCHPAD_PIXEL_WIDTH,
        TOUCHPAD_PIXEL_HEIGHT,
    );
    draw_bitmap_capsule_button(
        &mut canvas,
        center_x - 18,
        12,
        "SHARE",
        view.share_pressed(),
    );
    draw_bitmap_capsule_button(
        &mut canvas,
        center_x + 18,
        12,
        "OPTIONS",
        view.options_pressed(),
    );

    draw_bitmap_stick(
        &mut canvas,
        left_stick_center_x,
        30,
        view.lx_raw(),
        view.ly_raw(),
        view.l3_pressed(),
        StickSide::Left,
    );
    draw_bitmap_stick(
        &mut canvas,
        right_stick_center_x,
        30,
        view.rx_raw(),
        view.ry_raw(),
        view.r3_pressed(),
        StickSide::Right,
    );

    draw_bitmap_circle_button(&mut canvas, center_x, 29, "PS", view.ps_pressed());
    let stick_char_width = STICK_PIXEL_DIAMETER.div_ceil(2);
    put_centered_in_box(
        &mut canvas,
        left_stick_center_x.saturating_sub(stick_char_width / 2),
        stick_char_width,
        36,
        "Left Stick",
        Color::Gray,
    );
    put_centered_in_box(
        &mut canvas,
        right_stick_center_x.saturating_sub(stick_char_width / 2),
        stick_char_width,
        36,
        "Right Stick",
        Color::Gray,
    );
    canvas.put_centered(
        left_stick_center_x,
        37,
        &format!(
            "X:{:+4}% Y:{:+4}%",
            stick_percent_x(view.lx_raw()),
            stick_percent_y(view.ly_raw())
        ),
        Color::Gray,
    );
    canvas.put_centered(
        right_stick_center_x,
        37,
        &format!(
            "X:{:+4}% Y:{:+4}%",
            stick_percent_x(view.rx_raw()),
            stick_percent_y(view.ry_raw())
        ),
        Color::Gray,
    );
    draw_sensor_group(
        &mut canvas,
        left_stick_center_x.saturating_sub(SENSOR_ROW_WIDTH / 2),
        39,
        "GYRO",
        sensor_readings.map(|sensors| sensors.gyro),
        ["GX", "GY", "GZ"],
        GYRO_BAR_MAX,
    );
    draw_sensor_group(
        &mut canvas,
        right_stick_center_x.saturating_sub(SENSOR_ROW_WIDTH / 2),
        39,
        "ACCEL",
        sensor_readings.map(|sensors| sensors.accel),
        ["AX", "AY", "AZ"],
        ACCEL_BAR_MAX,
    );
    match sensor_readings {
        Some(sensors) => canvas.put_centered(
            center_x,
            43,
            &format!(
                "TS:{:05} TEMP:{:02}C",
                sensors.timestamp, sensors.temperature
            ),
            Color::Gray,
        ),
        None => canvas.put_centered(center_x, 43, "TS:N/A TEMP:N/A", Color::Gray),
    }

    let controller_lines = canvas
        .render()
        .lines()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let log_lines = build_full_monitor_log_lines(frame, panel_width);
    let gap = " ".repeat(FULL_MONITOR_LOG_GAP);

    for row in 0..controller_lines.len() {
        let controller_line = controller_lines.get(row).map(String::as_str).unwrap_or("");
        let log_line = log_lines.get(row).map(String::as_str).unwrap_or("");
        writeln!(
            screen,
            "{controller_line}{gap}{}",
            pad_visible_line(log_line, panel_width)
        )
        .expect("writing to String should not fail");
    }
}

fn write_full_monitor_header(
    screen: &mut String,
    frame: &MonitorFrame,
    status: Option<&str>,
    content_width: usize,
) {
    write_colored_line_width(
        screen,
        &format!(
            "Seq: {:>6}   Transport: {:<10}   Bytes: {:>3}   Status: {}",
            frame.sequence,
            frame.transport,
            frame.report_len,
            status.unwrap_or("receiving")
        ),
        GRAY,
        content_width,
    );
    write_wrapped_colored_line_width(
        screen,
        "Device: ",
        &format!(
            "{}   VID:0x{:04X}   PID:0x{:04X}   IF:{}",
            frame.device_name, frame.vendor_id, frame.product_id, frame.interface_number
        ),
        GRAY,
        content_width,
    );
}

fn build_full_monitor_log_lines(frame: &MonitorFrame, panel_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    push_full_monitor_log_section(
        &mut lines,
        "HID Recent",
        &frame.hid_history,
        panel_width,
        |bytes| format_report_hex_wrapped_bytes(bytes, 32),
    );
    push_full_monitor_log_section(
        &mut lines,
        "TX Recent",
        &frame.tx_history,
        panel_width,
        |bytes| format_report_hex(bytes),
    );
    push_full_monitor_log_section(
        &mut lines,
        "RX Text",
        &frame.rx_history,
        panel_width,
        |text| text.clone(),
    );
    lines.truncate(CANVAS_HEIGHT);
    lines
}

fn push_full_monitor_log_section<T, F>(
    lines: &mut Vec<String>,
    title: &str,
    entries: &[T],
    panel_width: usize,
    format_entry: F,
) where
    F: Fn(&T) -> String,
{
    lines.push(format!("{GRAY}{title}{RESET}"));
    if entries.is_empty() {
        lines.push(format!("{GRAY}  (none){RESET}"));
        lines.push(String::new());
        return;
    }

    for (index, entry) in entries.iter().enumerate() {
        let color = if index == 0 { WHITE } else { GRAY };
        let wrapped_lines = wrap_plain_text(&format_entry(entry), panel_width.saturating_sub(2));
        for wrapped_line in wrapped_lines {
            lines.push(format!("{color}  {wrapped_line}{RESET}"));
        }
    }
    lines.push(String::new());
}

fn render_raw_monitor(screen: &mut String, frame: &MonitorFrame) {
    write_wrapped_colored_line(screen, "HID: ", &format_report_hex(&frame.raw_report), GRAY);
    writeln!(screen).expect("writing to String should not fail");
}

fn render_compact_monitor(screen: &mut String, frame: &MonitorFrame) {
    write_wrapped_colored_line(
        screen,
        "Compact: ",
        &format_report_hex(&frame.compact),
        GRAY,
    );
    writeln!(screen).expect("writing to String should not fail");
}

fn write_common_monitor_lines(
    screen: &mut String,
    frame: &MonitorFrame,
    status: Option<&str>,
    mode: DisplayMode,
) {
    match status {
        Some(message) => write_colored_line(screen, &format!("Status: {message}"), YELLOW),
        None => write_colored_line(screen, "Status: receiving", GRAY),
    }
    if mode != DisplayMode::Full {
        write_wrapped_colored_line(
            screen,
            "Device: ",
            &format!(
                "{}   VID:0x{:04X}   PID:0x{:04X}   IF:{}",
                frame.device_name, frame.vendor_id, frame.product_id, frame.interface_number
            ),
            GRAY,
        );
    }
}

#[derive(Debug, Clone, Copy)]
struct CompactView {
    report: CompactReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UsbBatteryStatus {
    level: u8,
    percent: u8,
    full: bool,
}

impl UsbBatteryStatus {
    fn full(self) -> bool {
        self.full
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UsbSensorReadings {
    timestamp: u16,
    temperature: u8,
    gyro: [i16; 3],
    accel: [i16; 3],
}

impl CompactView {
    fn new(report: CompactReport) -> Self {
        Self { report }
    }

    fn button0(self, bit: u8) -> bool {
        (self.report[0] & (1u8 << bit)) != 0
    }

    fn button1(self, bit: u8) -> bool {
        (self.report[1] & (1u8 << bit)) != 0
    }

    fn analog(self, index: usize) -> u8 {
        self.report[index]
    }

    fn up_pressed(self) -> bool {
        self.button0(0)
    }

    fn right_pressed(self) -> bool {
        self.button0(1)
    }

    fn down_pressed(self) -> bool {
        self.button0(2)
    }

    fn left_pressed(self) -> bool {
        self.button0(3)
    }

    fn square_pressed(self) -> bool {
        self.button0(4)
    }

    fn cross_pressed(self) -> bool {
        self.button0(5)
    }

    fn circle_pressed(self) -> bool {
        self.button0(6)
    }

    fn triangle_pressed(self) -> bool {
        self.button0(7)
    }

    fn l1_pressed(self) -> bool {
        self.button1(0)
    }

    fn r1_pressed(self) -> bool {
        self.button1(1)
    }

    fn share_pressed(self) -> bool {
        self.button1(2)
    }

    fn options_pressed(self) -> bool {
        self.button1(3)
    }

    fn l3_pressed(self) -> bool {
        self.button1(4)
    }

    fn r3_pressed(self) -> bool {
        self.button1(5)
    }

    fn ps_pressed(self) -> bool {
        self.button1(6)
    }

    fn trackpad_pressed(self) -> bool {
        self.button1(7)
    }

    fn lx_raw(self) -> u8 {
        self.analog(2)
    }

    fn ly_raw(self) -> u8 {
        self.analog(3)
    }

    fn rx_raw(self) -> u8 {
        self.analog(4)
    }

    fn ry_raw(self) -> u8 {
        self.analog(5)
    }

    fn l2_raw(self) -> u8 {
        self.analog(6)
    }

    fn r2_raw(self) -> u8 {
        self.analog(7)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    Default,
    Gray,
    Green,
    Cyan,
}

impl Color {
    fn ansi(self) -> &'static str {
        match self {
            Self::Default => RESET,
            Self::Gray => GRAY,
            Self::Green => GREEN,
            Self::Cyan => CYAN,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Cell {
    ch: char,
    color: Color,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            color: Color::Default,
        }
    }
}

struct Canvas {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::default(); width * height],
        }
    }

    fn put(&mut self, x: usize, y: usize, ch: char, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }

        self.cells[y * self.width + x] = Cell { ch, color };
    }

    fn put_text(&mut self, x: usize, y: usize, text: &str, color: Color) {
        for (offset, ch) in text.chars().enumerate() {
            self.put(x + offset, y, ch, color);
        }
    }

    fn put_centered(&mut self, center_x: usize, y: usize, text: &str, color: Color) {
        let width = text.chars().count();
        let start_x = center_x.saturating_sub(width / 2);
        self.put_text(start_x, y, text, color);
    }

    fn render(&self) -> String {
        let mut output = String::new();

        for y in 0..self.height {
            let mut current_color = Color::Default;

            for x in 0..self.width {
                let cell = self.cells[y * self.width + x];
                if cell.color != current_color {
                    output.push_str(cell.color.ansi());
                    current_color = cell.color;
                }
                output.push(cell.ch);
            }

            if current_color != Color::Default {
                output.push_str(RESET);
            }
            output.push('\n');
        }

        output
    }
}

#[derive(Debug, Clone, Copy)]
enum StickSide {
    Left,
    Right,
}

fn draw_bitmap_touchpad(
    canvas: &mut Canvas,
    center_x: usize,
    top_y: usize,
    pressed: bool,
    touch_positions: [Option<(u16, u16)>; 2],
    pixel_width: usize,
    pixel_height: usize,
) {
    let fill_color = if pressed { Color::Green } else { Color::Gray };
    let pixels = vec![vec![Some(fill_color); pixel_width]; pixel_height];
    let mut pixels = pixels;
    if let Some(last_row) = pixels.last_mut() {
        last_row.fill(None);
    }
    let char_width = pixel_width.div_ceil(2);
    let char_height = pixel_height.div_ceil(4);
    let start_x = center_x.saturating_sub(char_width / 2);

    draw_braille_color_grid(canvas, center_x, top_y, &pixels);

    for touch_position in touch_positions {
        if let Some((touch_x, touch_y)) = touch_position {
            let (marker_x, marker_y) =
                touchpad_char_marker_position(touch_x, touch_y, char_width, char_height);
            draw_pointer_cell(canvas, start_x + marker_x, top_y + marker_y);
        }
    }
}

fn draw_bitmap_battery(
    canvas: &mut Canvas,
    center_x: usize,
    top_y: usize,
    battery: Option<UsbBatteryStatus>,
) {
    let label_color = battery.map_or(Color::Gray, |_| Color::Green);
    let summary = match battery {
        Some(status) => format!("BATTERY: {:>3}%", status.percent),
        None => String::from("BATTERY: N/A"),
    };
    let start_x = center_x.saturating_sub(BATTERY_OUTLINE_CHAR_WIDTH / 2);

    draw_braille_bits_grid(
        canvas,
        start_x,
        top_y,
        &battery_outline_cell_grid(),
        Color::Gray,
    );
    draw_braille_bits_grid(
        canvas,
        start_x + 1,
        top_y + 1,
        &battery_fill_cell_grid(battery),
        Color::Green,
    );
    canvas.put_centered(center_x, top_y + 5, &summary, label_color);
}

fn draw_sensor_group(
    canvas: &mut Canvas,
    left_x: usize,
    top_y: usize,
    title: &str,
    values: Option<[i16; 3]>,
    labels: [&str; 3],
    max_magnitude: i16,
) {
    put_centered_in_box(canvas, left_x, SENSOR_ROW_WIDTH, top_y, title, Color::Gray);

    match values {
        Some(values) => {
            for (row, (label, value)) in labels.iter().zip(values.iter()).enumerate() {
                draw_sensor_row(
                    canvas,
                    left_x,
                    top_y + 1 + row,
                    label,
                    *value,
                    max_magnitude,
                );
            }
        }
        None => {
            put_centered_in_box(
                canvas,
                left_x,
                SENSOR_ROW_WIDTH,
                top_y + 1,
                "USB 64B ONLY",
                Color::Gray,
            );
        }
    }
}

fn draw_sensor_row(
    canvas: &mut Canvas,
    left_x: usize,
    y: usize,
    label: &str,
    value: i16,
    max_magnitude: i16,
) {
    canvas.put_text(left_x, y, label, Color::Gray);
    draw_signed_progress_bar(canvas, left_x + 3, y, value, max_magnitude);
    canvas.put_text(left_x + 15, y, &format!("{value:+6}"), Color::Gray);
}

fn draw_signed_progress_bar(
    canvas: &mut Canvas,
    left_x: usize,
    y: usize,
    value: i16,
    max_magnitude: i16,
) {
    canvas.put(left_x, y, '[', Color::Gray);
    canvas.put(left_x + SENSOR_BAR_HALF_WIDTH + 1, y, '|', Color::Gray);
    canvas.put(left_x + SENSOR_BAR_HALF_WIDTH * 2 + 2, y, ']', Color::Gray);

    for offset in 0..SENSOR_BAR_HALF_WIDTH {
        canvas.put(left_x + 1 + offset, y, '.', Color::Gray);
        canvas.put(
            left_x + SENSOR_BAR_HALF_WIDTH + 2 + offset,
            y,
            '.',
            Color::Gray,
        );
    }

    let fill_count = signed_bar_fill_count(value, max_magnitude);
    if value < 0 {
        for offset in 0..fill_count {
            let x = left_x + SENSOR_BAR_HALF_WIDTH - offset;
            canvas.put(x, y, '#', Color::Green);
        }
    } else if value > 0 {
        for offset in 0..fill_count {
            let x = left_x + SENSOR_BAR_HALF_WIDTH + 2 + offset;
            canvas.put(x, y, '#', Color::Green);
        }
    }
}

fn draw_bitmap_shoulder_button(
    canvas: &mut Canvas,
    center_x: usize,
    top_y: usize,
    label: &str,
    pressed: bool,
) {
    let color = if pressed { Color::Green } else { Color::Gray };
    let bitmap = shoulder_button_bitmap(24, 9);
    let braille_lines = bitmap_to_braille(&bitmap_refs(&bitmap));
    let button_width = braille_lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);

    draw_braille_lines(canvas, center_x, top_y + 1, &braille_lines, color);

    let label_y = top_y + 2;
    if label == "L1" {
        let label_x = center_x.saturating_sub(button_width / 2 + label.chars().count() + 1);
        canvas.put_text(label_x, label_y, label, color);
    } else {
        let label_x = center_x + button_width / 2 + 2;
        canvas.put_text(label_x, label_y, label, color);
    }
}

fn draw_bitmap_trigger_button(
    canvas: &mut Canvas,
    center_x: usize,
    top_y: usize,
    label: &str,
    value: u8,
) {
    let label_color = if value == u8::MAX {
        Color::Green
    } else {
        Color::Gray
    };
    let bitmap = trigger_button_bitmap(24, 18);
    let bitmap_refs = bitmap_refs(&bitmap);
    let braille_lines = bitmap_to_braille(&bitmap_refs);
    let button_width = braille_lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);

    draw_braille_fill(canvas, center_x, top_y + 1, &bitmap_refs, value);

    let label_y = top_y + 2;
    if label == "L2" {
        let label_x = center_x.saturating_sub(button_width / 2 + label.chars().count() + 1);
        canvas.put_text(label_x, label_y, label, label_color);
    } else {
        let label_x = center_x + button_width / 2 + 2;
        canvas.put_text(label_x, label_y, label, label_color);
    }
}

fn draw_bitmap_circle_button(
    canvas: &mut Canvas,
    center_x: usize,
    top_y: usize,
    label: &str,
    pressed: bool,
) {
    let color = if pressed { Color::Green } else { Color::Gray };
    let bitmap = circle_outline_bitmap(16, 12);
    let braille_lines = bitmap_to_braille(&bitmap_refs(&bitmap));
    let text_row = top_y + braille_lines.len() / 2;

    draw_braille_lines(canvas, center_x, top_y, &braille_lines, color);
    canvas.put_centered(center_x, text_row, label, color);
}

fn draw_bitmap_capsule_button(
    canvas: &mut Canvas,
    center_x: usize,
    top_y: usize,
    label: &str,
    pressed: bool,
) {
    let color = if pressed { Color::Green } else { Color::Gray };
    let bitmap = vertical_capsule_filled_bitmap(6, 12);
    let braille_lines = bitmap_to_braille(&bitmap_refs(&bitmap));

    canvas.put_centered(center_x, top_y, label, color);
    draw_braille_lines(canvas, center_x, top_y + 1, &braille_lines, color);
}

fn draw_dot_pattern(
    canvas: &mut Canvas,
    center_x: usize,
    top_y: usize,
    bitmap: &[&str],
    pressed: bool,
) {
    let color = if pressed { Color::Green } else { Color::Gray };
    let braille_lines = bitmap_to_braille(bitmap);
    draw_braille_lines(canvas, center_x, top_y, &braille_lines, color);
}

fn draw_bitmap_stick(
    canvas: &mut Canvas,
    center_x: usize,
    center_y: usize,
    x_raw: u8,
    y_raw: u8,
    pressed: bool,
    side: StickSide,
) {
    let border_color = if pressed { Color::Green } else { Color::Gray };
    let pixels = stick_outline_pixel_grid(border_color);
    let char_width = STICK_PIXEL_DIAMETER.div_ceil(2);
    let char_height = STICK_PIXEL_DIAMETER.div_ceil(4);
    let top_y = center_y.saturating_sub(char_height / 2);
    let start_x = center_x.saturating_sub(char_width / 2);

    draw_braille_color_grid(canvas, center_x, top_y, &pixels);
    let (marker_x, marker_y) = stick_char_marker_position(x_raw, y_raw, char_width, char_height);
    draw_pointer_cell(canvas, start_x + marker_x, top_y + marker_y);

    match side {
        StickSide::Left => {
            let label_x = center_x.saturating_sub(char_width / 2 + 3);
            canvas.put_text(label_x, top_y, "L3", border_color)
        }
        StickSide::Right => {
            canvas.put_text(center_x + char_width / 2 + 1, top_y, "R3", border_color)
        }
    }
}

fn stick_char_marker_position(
    x_raw: u8,
    y_raw: u8,
    char_width: usize,
    char_height: usize,
) -> (usize, usize) {
    let x = normalized_x(x_raw);
    let y = normalized_y(y_raw);
    let active_width = STICK_POINTER_CHAR_WIDTH.min(char_width.max(1));
    let active_height = STICK_POINTER_CHAR_HEIGHT.min(char_height.max(1));
    let inset_x = (char_width.saturating_sub(active_width)) / 2;
    let inset_y = (char_height.saturating_sub(active_height)) / 2;
    let max_x = (active_width.saturating_sub(1)) as f32;
    let max_y = (active_height.saturating_sub(1)) as f32;
    let col = (((x + 1.0) * 0.5) * max_x).round() as usize;
    let row = (((1.0 - y) * 0.5) * max_y).round() as usize;
    (
        (inset_x + col).min(char_width.saturating_sub(1)),
        (inset_y + row).min(char_height.saturating_sub(1)),
    )
}

fn touchpad_char_marker_position(
    x_raw: u16,
    y_raw: u16,
    char_width: usize,
    char_height: usize,
) -> (usize, usize) {
    let x = usize::from(x_raw.min(TOUCHPAD_MAX_X));
    let y = usize::from(y_raw.min(TOUCHPAD_MAX_Y));
    let max_x = char_width.saturating_sub(1);
    let max_y = char_height.saturating_sub(2);
    let col = (x * max_x + usize::from(TOUCHPAD_MAX_X) / 2) / usize::from(TOUCHPAD_MAX_X);
    let row = (y * max_y + usize::from(TOUCHPAD_MAX_Y) / 2) / usize::from(TOUCHPAD_MAX_Y);
    (col.min(max_x), row.min(max_y))
}

fn normalized_x(value: u8) -> f32 {
    (((value as f32) - 128.0) / 127.0).clamp(-1.0, 1.0)
}

fn normalized_y(value: u8) -> f32 {
    ((128.0 - (value as f32)) / 127.0).clamp(-1.0, 1.0)
}

fn stick_percent_x(value: u8) -> i16 {
    (normalized_x(value) * 100.0).round() as i16
}

fn stick_percent_y(value: u8) -> i16 {
    (normalized_y(value) * 100.0).round() as i16
}

#[cfg(test)]
fn trigger_percent(value: u8) -> u8 {
    (((value as f32) / 255.0) * 100.0).round() as u8
}

fn bitmap_to_braille(bitmap: &[&str]) -> Vec<String> {
    let pixel_width = bitmap
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let pixel_height = bitmap.len();
    let char_width = pixel_width.div_ceil(2);
    let char_height = pixel_height.div_ceil(4);
    let mut lines = Vec::with_capacity(char_height);

    for char_y in 0..char_height {
        let mut line = String::with_capacity(char_width);

        for char_x in 0..char_width {
            let mut bits = 0u8;

            for pixel_y in 0..4 {
                for pixel_x in 0..2 {
                    let source_x = char_x * 2 + pixel_x;
                    let source_y = char_y * 4 + pixel_y;

                    if source_y >= pixel_height || !bitmap_pixel_is_on(bitmap, source_x, source_y) {
                        continue;
                    }

                    bits |= braille_bit(pixel_x, pixel_y);
                }
            }

            let braille = char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ');
            line.push(braille);
        }

        while line.ends_with('\u{2800}') {
            line.pop();
        }
        lines.push(line);
    }

    lines
}

fn bitmap_refs(rows: &[String]) -> Vec<&str> {
    rows.iter().map(String::as_str).collect()
}

fn draw_braille_lines(
    canvas: &mut Canvas,
    center_x: usize,
    top_y: usize,
    lines: &[String],
    color: Color,
) {
    let width = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let start_x = center_x.saturating_sub(width / 2);

    for (row, line) in lines.iter().enumerate() {
        canvas.put_text(start_x, top_y + row, line, color);
    }
}

fn draw_braille_fill(
    canvas: &mut Canvas,
    center_x: usize,
    top_y: usize,
    bitmap: &[&str],
    value: u8,
) {
    let pixel_width = bitmap
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let pixel_height = bitmap.len();
    let char_width = pixel_width.div_ceil(2);
    let char_height = pixel_height.div_ceil(4);
    let start_x = center_x.saturating_sub(char_width / 2);
    let fill_start_row =
        pixel_height.saturating_sub((usize::from(value) * pixel_height).div_ceil(255));

    for char_y in 0..char_height {
        for char_x in 0..char_width {
            let mut gray_bits = 0u8;
            let mut green_bits = 0u8;

            for pixel_y in 0..4 {
                for pixel_x in 0..2 {
                    let source_x = char_x * 2 + pixel_x;
                    let source_y = char_y * 4 + pixel_y;

                    if source_y >= pixel_height || !bitmap_pixel_is_on(bitmap, source_x, source_y) {
                        continue;
                    }

                    let bit = braille_bit(pixel_x, pixel_y);
                    if source_y >= fill_start_row {
                        green_bits |= bit;
                    } else {
                        gray_bits |= bit;
                    }
                }
            }

            let (bits, color) = if green_bits != 0 {
                (green_bits | gray_bits, Color::Green)
            } else if gray_bits != 0 {
                (gray_bits, Color::Gray)
            } else {
                continue;
            };

            let ch = char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ');
            canvas.put(start_x + char_x, top_y + char_y, ch, color);
        }
    }
}

fn put_centered_in_box(
    canvas: &mut Canvas,
    left_x: usize,
    box_width: usize,
    y: usize,
    text: &str,
    color: Color,
) {
    let text_width = text.chars().count();
    let x = left_x + box_width.saturating_sub(text_width) / 2;
    canvas.put_text(x, y, text, color);
}

fn draw_braille_color_grid(
    canvas: &mut Canvas,
    center_x: usize,
    top_y: usize,
    pixels: &[Vec<Option<Color>>],
) {
    let pixel_height = pixels.len();
    let pixel_width = pixels.first().map(Vec::len).unwrap_or(0);
    let char_width = pixel_width.div_ceil(2);
    let char_height = pixel_height.div_ceil(4);
    let start_x = center_x.saturating_sub(char_width / 2);

    for char_y in 0..char_height {
        for char_x in 0..char_width {
            let mut gray_bits = 0u8;
            let mut green_bits = 0u8;
            let mut cyan_bits = 0u8;

            for pixel_y in 0..4 {
                for pixel_x in 0..2 {
                    let source_x = char_x * 2 + pixel_x;
                    let source_y = char_y * 4 + pixel_y;
                    let Some(row) = pixels.get(source_y) else {
                        continue;
                    };
                    let Some(Some(color)) = row.get(source_x) else {
                        continue;
                    };

                    let bit = braille_bit(pixel_x, pixel_y);
                    match color {
                        Color::Gray => gray_bits |= bit,
                        Color::Green => green_bits |= bit,
                        Color::Cyan => cyan_bits |= bit,
                        Color::Default => {}
                    }
                }
            }

            let (bits, color) = if cyan_bits != 0 {
                (cyan_bits | green_bits | gray_bits, Color::Cyan)
            } else if green_bits != 0 {
                (green_bits | gray_bits, Color::Green)
            } else if gray_bits != 0 {
                (gray_bits, Color::Gray)
            } else {
                continue;
            };

            let ch = char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ');
            canvas.put(start_x + char_x, top_y + char_y, ch, color);
        }
    }
}

fn bitmap_pixel_is_on(bitmap: &[&str], x: usize, y: usize) -> bool {
    bitmap
        .get(y)
        .and_then(|line| line.chars().nth(x))
        .is_some_and(|ch| ch != ' ')
}

fn braille_bit(pixel_x: usize, pixel_y: usize) -> u8 {
    match (pixel_x, pixel_y) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (0, 3) => 0x40,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        (1, 3) => 0x80,
        _ => 0,
    }
}

fn usb_touch_positions(report: &[u8]) -> [Option<(u16, u16)>; 2] {
    let mut positions = [None, None];

    if report.len() < 43 || report.first().copied() != Some(0x01) {
        return positions;
    }

    let report_count = usize::from(report[33]).min(3);
    let mut next_index = 0usize;

    for report_index in 0..report_count {
        let base = 34 + report_index * 9;
        for point_offset in [1usize, 5usize] {
            let point = &report[base + point_offset..base + point_offset + 4];
            if let Some(position) = parse_usb_touch_point(point) {
                positions[next_index] = Some(position);
                next_index += 1;
                if next_index == positions.len() {
                    return positions;
                }
            }
        }
    }

    positions
}

fn parse_usb_touch_point(point: &[u8]) -> Option<(u16, u16)> {
    if point.len() < 4 || (point[0] & 0x80) != 0 {
        return None;
    }

    let x = u16::from(point[1]) | (u16::from(point[2] & 0x0F) << 8);
    let y = u16::from(point[2] >> 4) | (u16::from(point[3]) << 4);
    Some((x, y))
}

fn usb_battery_status(report: &[u8]) -> Option<UsbBatteryStatus> {
    if report.len() < 64 || report.first().copied() != Some(0x01) {
        return None;
    }

    let status0 = report[30];
    let status1 = report[31];
    let level = (status0 & 0x0F).min(10);

    Some(UsbBatteryStatus {
        level,
        percent: level.saturating_mul(10),
        full: (status1 & 0x0F) == 0x02,
    })
}

fn usb_sensor_readings(report: &[u8]) -> Option<UsbSensorReadings> {
    if report.len() < 25 || report.first().copied() != Some(0x01) {
        return None;
    }

    Some(UsbSensorReadings {
        timestamp: u16::from_le_bytes([report[10], report[11]]),
        temperature: report[12],
        gyro: [
            read_le_i16(report, 13)?,
            read_le_i16(report, 15)?,
            read_le_i16(report, 17)?,
        ],
        accel: [
            read_le_i16(report, 19)?,
            read_le_i16(report, 21)?,
            read_le_i16(report, 23)?,
        ],
    })
}

fn read_le_i16(report: &[u8], offset: usize) -> Option<i16> {
    let bytes = report.get(offset..offset + 2)?;
    Some(i16::from_le_bytes([bytes[0], bytes[1]]))
}

fn battery_fill_cell_grid(battery: Option<UsbBatteryStatus>) -> Vec<Vec<Option<u8>>> {
    let mut cells = vec![vec![None; BATTERY_BODY_CELL_WIDTH]; BATTERY_CELL_HEIGHT];
    let fill_dot_columns = battery.map_or(0, |status| {
        let max = BATTERY_BODY_CELL_WIDTH * 2;
        if status.full() {
            max
        } else {
            (usize::from(status.level) * max).div_ceil(10)
        }
    });

    for row in &mut cells {
        for (x, cell) in row.iter_mut().enumerate() {
            let remaining_columns = fill_dot_columns.saturating_sub(x * 2).min(2);
            *cell = match remaining_columns {
                0 => None,
                1 => Some(BRAILLE_LEFT_COLUMN_BITS),
                _ => Some(BRAILLE_FULL_CELL_BITS),
            };
        }
    }

    cells
}

fn battery_outline_cell_grid() -> Vec<Vec<Option<u8>>> {
    let mut cells = vec![vec![None; BATTERY_OUTLINE_CHAR_WIDTH]; BATTERY_OUTLINE_CHAR_HEIGHT];

    for (y, row) in cells.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            if battery_occupied_cell(x, y) {
                continue;
            }

            let mut bits = 0u8;

            if x.checked_sub(1)
                .is_some_and(|left_x| battery_occupied_cell(left_x, y))
            {
                bits |= BRAILLE_LEFT_COLUMN_BITS;
            }
            if battery_occupied_cell(x + 1, y) {
                bits |= BRAILLE_RIGHT_COLUMN_BITS;
            }
            if y.checked_sub(1)
                .is_some_and(|top_y| battery_occupied_cell(x, top_y))
            {
                bits |= BRAILLE_TOP_ROW_BITS;
            }
            if battery_occupied_cell(x, y + 1) {
                bits |= BRAILLE_BOTTOM_ROW_BITS;
            }

            if bits != 0 {
                *cell = Some(bits);
            }
        }
    }

    cells
}

fn battery_occupied_cell(x: usize, y: usize) -> bool {
    let Some(body_x) = x.checked_sub(1) else {
        return false;
    };
    let Some(body_y) = y.checked_sub(1) else {
        return false;
    };

    if body_y >= BATTERY_CELL_HEIGHT {
        return false;
    }

    body_x < BATTERY_BODY_CELL_WIDTH || (body_x == BATTERY_CAP_CELL_X && body_y == 1)
}

fn draw_braille_bits_grid(
    canvas: &mut Canvas,
    start_x: usize,
    top_y: usize,
    grid: &[Vec<Option<u8>>],
    color: Color,
) {
    for (row_index, row) in grid.iter().enumerate() {
        for (col_index, bits) in row.iter().enumerate() {
            let Some(bits) = bits else {
                continue;
            };
            canvas.put(
                start_x + col_index,
                top_y + row_index,
                braille_char(*bits),
                color,
            );
        }
    }
}

fn braille_char(bits: u8) -> char {
    char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ')
}

fn signed_bar_fill_count(value: i16, max_magnitude: i16) -> usize {
    if max_magnitude <= 0 {
        return 0;
    }

    let magnitude = i32::from(value).unsigned_abs() as usize;
    let max = i32::from(max_magnitude).unsigned_abs() as usize;
    ((magnitude.min(max) * SENSOR_BAR_HALF_WIDTH) + max / 2) / max
}

fn stick_outline_pixel_grid(fill_color: Color) -> Vec<Vec<Option<Color>>> {
    let mut pixels = vec![vec![None; STICK_PIXEL_DIAMETER]; STICK_PIXEL_DIAMETER];
    let center_x = (STICK_PIXEL_DIAMETER as f32 - 1.0) / 2.0;
    let center_y = center_x + 0.35;
    let radius_x = center_x;
    let radius_y = center_x + 0.35;

    for (row_index, row) in pixels.iter_mut().enumerate() {
        for (col_index, cell) in row.iter_mut().enumerate() {
            let dx = (col_index as f32 - center_x) / radius_x;
            let dy = (row_index as f32 - center_y) / radius_y;
            if dx * dx + dy * dy <= 1.0 {
                *cell = Some(fill_color);
            }
        }
    }

    pixels
}

fn draw_pointer_cell(canvas: &mut Canvas, x: usize, y: usize) {
    canvas.put_text(x, y, POINTER_CELL_CHAR, Color::Cyan);
}

fn shoulder_button_bitmap(width: usize, height: usize) -> Vec<String> {
    let mut rows = Vec::with_capacity(height);
    let center_x = (width as f32 - 1.0) / 2.0;
    let flat_height = height / 3;
    let curve_height = (height - flat_height).max(1) as f32;

    for y in 0..height {
        let mut row = String::with_capacity(width);
        if y < flat_height {
            row.push_str(&"#".repeat(width));
            rows.push(row);
            continue;
        }

        let dy = (y - flat_height) as f32 / curve_height;
        let half_width = center_x * (1.0 - dy * dy).max(0.0).sqrt();

        for x in 0..width {
            let dx = (x as f32 - center_x).abs();
            row.push(if dx <= half_width { '#' } else { ' ' });
        }

        rows.push(row);
    }

    rows
}

fn trigger_button_bitmap(width: usize, height: usize) -> Vec<String> {
    let mut rows = Vec::with_capacity(height);
    let center_x = (width as f32 - 1.0) / 2.0;
    let curve_height = height / 3;
    let flat_start = curve_height;
    let curve_height_f = curve_height.max(1) as f32;

    for y in 0..height {
        let mut row = String::with_capacity(width);
        if y >= flat_start {
            row.push_str(&"#".repeat(width));
            rows.push(row);
            continue;
        }

        let dy = (flat_start - y) as f32 / curve_height_f;
        let half_width = center_x * (1.0 - dy * dy).max(0.0).sqrt();

        for x in 0..width {
            let dx = (x as f32 - center_x).abs();
            row.push(if dx <= half_width { '#' } else { ' ' });
        }

        rows.push(row);
    }

    rows
}

fn circle_outline_bitmap(width: usize, height: usize) -> Vec<String> {
    let mut rows = Vec::with_capacity(height);
    let radius_x = (width as f32 - 1.0) / 2.0;
    let radius_y = (height as f32 - 1.0) / 2.0;

    for y in 0..height {
        let mut row = String::with_capacity(width);
        for x in 0..width {
            let dx = (x as f32 - radius_x) / radius_x;
            let dy = (y as f32 - radius_y) / radius_y;
            let value = dx * dx + dy * dy;
            row.push(if (0.72..=1.08).contains(&value) {
                '#'
            } else {
                ' '
            });
        }
        rows.push(row);
    }

    rows
}

fn vertical_capsule_filled_bitmap(width: usize, height: usize) -> Vec<String> {
    let mut rows = Vec::with_capacity(height);
    let radius_x = (width as f32 - 1.0) / 2.0;
    let radius_y = radius_x;
    let center_x = radius_x;
    let top_center_y = radius_y;
    let bottom_center_y = height as f32 - 1.0 - radius_y;

    for y in 0..height {
        let mut row = String::with_capacity(width);
        for x in 0..width {
            let xf = x as f32;
            let yf = y as f32;

            let value = if yf < top_center_y {
                let dx = (xf - center_x) / radius_x;
                let dy = (yf - top_center_y) / radius_y;
                dx * dx + dy * dy
            } else if yf > bottom_center_y {
                let dx = (xf - center_x) / radius_x;
                let dy = (yf - bottom_center_y) / radius_y;
                dx * dx + dy * dy
            } else {
                let dx = (xf - center_x) / radius_x;
                dx * dx
            };

            row.push(if value <= 1.0 { '#' } else { ' ' });
        }
        rows.push(row);
    }

    rows
}

fn dpad_up_bitmap() -> &'static [&'static str] {
    &[
        "##############",
        "##############",
        "##############",
        "##############",
        "##############",
        "##############",
        "##############",
        "##############",
        " ############ ",
        "  ##########  ",
        "   ########   ",
        "    ######    ",
        "      ##      ",
    ]
}

fn dpad_left_bitmap() -> &'static [&'static str] {
    &[
        "#########     ",
        "##########    ",
        "###########   ",
        "############  ",
        "############# ",
        "##############",
        "##############",
        "############# ",
        "############  ",
        "###########   ",
        "##########    ",
        "#########     ",
    ]
}

fn dpad_right_bitmap() -> &'static [&'static str] {
    &[
        "     #########",
        "    ##########",
        "   ###########",
        "  ############",
        " #############",
        "##############",
        "##############",
        " #############",
        "  ############",
        "   ###########",
        "    ##########",
        "     #########",
    ]
}

fn dpad_down_bitmap() -> &'static [&'static str] {
    &[
        "      ##      ",
        "    ######    ",
        "   ########   ",
        "  ##########  ",
        " ############ ",
        "##############",
        "##############",
        "##############",
        "##############",
        "##############",
        "##############",
        "##############",
        "##############",
    ]
}

fn triangle_bitmap() -> &'static [&'static str] {
    &[
        "        #       ",
        "       ###      ",
        "       ###      ",
        "      ## ##     ",
        "     ##   ##    ",
        "    ##     ##   ",
        "    ##     ##   ",
        "   ##       ##  ",
        "  ##         ## ",
        "  ##         ## ",
        " ###############",
        " ###############",
    ]
}

fn square_bitmap() -> &'static [&'static str] {
    &[
        "############",
        "############",
        "##        ##",
        "##        ##",
        "##        ##",
        "##        ##",
        "##        ##",
        "##        ##",
        "##        ##",
        "############",
        "############",
    ]
}

fn circle_bitmap() -> &'static [&'static str] {
    &[
        "   #######   ",
        "  #########  ",
        " ##       ## ",
        " ##       ## ",
        "##         ##",
        "##         ##",
        "##         ##",
        "##         ##",
        " ##       ## ",
        " ##       ## ",
        "  #########  ",
        "   #######   ",
    ]
}

fn cross_bitmap() -> &'static [&'static str] {
    &[
        "##          ##",
        "###        ###",
        " ###      ### ",
        "  ###    ###  ",
        "   ###  ###   ",
        "    ######    ",
        "    ######    ",
        "   ###  ###   ",
        "  ###    ###  ",
        " ###      ### ",
        "###        ###",
        "##          ##",
    ]
}

fn pad_visible_line(text: &str, width: usize) -> String {
    let visible_width = text.chars().count();
    if visible_width >= width {
        return text.to_owned();
    }

    format!("{text}{}", " ".repeat(width - visible_width))
}

fn write_colored_line(screen: &mut String, text: &str, color: &str) {
    write_colored_line_width(screen, text, color, CANVAS_WIDTH);
}

fn write_colored_line_width(screen: &mut String, text: &str, color: &str, width: usize) {
    let line = pad_visible_line(text, width);
    writeln!(screen, "{color}{line}{RESET}").expect("writing to String should not fail");
}

fn write_wrapped_colored_line(screen: &mut String, prefix: &str, text: &str, color: &str) {
    write_wrapped_colored_line_width(screen, prefix, text, color, CANVAS_WIDTH);
}

fn write_wrapped_colored_line_width(
    screen: &mut String,
    prefix: &str,
    text: &str,
    color: &str,
    width: usize,
) {
    let prefix_width = prefix.chars().count();
    let indent = " ".repeat(prefix_width);
    let mut current = prefix.to_owned();
    let mut current_width = prefix_width;

    if text.is_empty() {
        write_colored_line_width(screen, prefix, color, width);
        return;
    }

    for token in text.split(' ') {
        let token_width = token.chars().count();
        let separator_width = usize::from(current_width > prefix_width);

        if current_width + separator_width + token_width > width {
            write_colored_line_width(screen, &current, color, width);
            current = indent.clone();
            current_width = prefix_width;
        }

        if current_width > prefix_width {
            current.push(' ');
            current_width += 1;
        }

        current.push_str(token);
        current_width += token_width;
    }

    write_colored_line_width(screen, &current, color, width);
}

fn format_report_hex(report: &[u8]) -> String {
    if report.is_empty() {
        return String::from("(none)");
    }

    report
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_report_hex_wrapped_bytes(report: &[u8], bytes_per_line: usize) -> String {
    if report.is_empty() {
        return String::from("(none)");
    }
    if bytes_per_line == 0 {
        return format_report_hex(report);
    }

    report
        .chunks(bytes_per_line)
        .map(format_report_hex)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
fn format_report_ascii(report: &[u8]) -> String {
    if report.is_empty() {
        return String::from("(none)");
    }

    let escaped = report
        .iter()
        .flat_map(|byte| std::ascii::escape_default(*byte))
        .map(char::from)
        .collect::<String>();
    format!("\"{escaped}\"")
}

#[cfg(test)]
fn format_report_hex_compact(report: &[u8], max_bytes: usize) -> String {
    if report.is_empty() {
        return String::from("(none)");
    }

    let shown = report
        .iter()
        .take(max_bytes)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    let remaining = report.len().saturating_sub(max_bytes);
    if remaining == 0 {
        shown
    } else {
        format!("{shown} ... (+{remaining}B)")
    }
}

#[cfg(test)]
fn truncate_chars(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_owned();
    }

    let keep = max_chars.saturating_sub(3);
    let truncated = text.chars().take(keep).collect::<String>();
    format!("{truncated}...")
}

fn wrap_plain_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for ch in text.chars() {
        if ch == '\n' {
            lines.push(current);
            current = String::new();
            current_width = 0;
            continue;
        }

        current.push(ch);
        current_width += 1;
        if current_width >= width {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn full_monitor_panel_width(content_width: usize) -> usize {
    content_width.saturating_sub(FULL_MONITOR_CONTROLLER_WIDTH + FULL_MONITOR_LOG_GAP)
}

fn centered_left_padding(terminal_width: usize, content_width: usize) -> usize {
    terminal_width.saturating_sub(content_width) / 2
}

fn fit_screen_to_terminal(screen: &str, content_width: usize) -> String {
    let Some((terminal_width, terminal_height)) = terminal_size() else {
        return screen.trim_end_matches('\n').to_owned();
    };

    let render_width = content_width.min(terminal_width);
    let left_padding = centered_left_padding(terminal_width, render_width);
    let padding = " ".repeat(left_padding);
    let mut fitted_lines = Vec::new();

    for line in screen.lines().take(terminal_height) {
        let clipped = truncate_ansi_line(line, render_width);
        if left_padding == 0 {
            fitted_lines.push(clipped);
        } else {
            fitted_lines.push(format!("{padding}{clipped}"));
        }
    }

    fitted_lines.join("\n")
}

fn truncate_ansi_line(line: &str, max_visible_width: usize) -> String {
    if max_visible_width == 0 {
        return String::new();
    }

    let mut output = String::new();
    let mut chars = line.chars().peekable();
    let mut visible_width = 0usize;
    let mut saw_escape = false;

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            saw_escape = true;
            output.push(ch);

            if let Some(next) = chars.next() {
                output.push(next);

                if next == '[' {
                    while let Some(csi_ch) = chars.next() {
                        output.push(csi_ch);
                        if ('@'..='~').contains(&csi_ch) {
                            break;
                        }
                    }
                }
            }

            continue;
        }

        if visible_width >= max_visible_width {
            break;
        }

        output.push(ch);
        visible_width += 1;
    }

    if saw_escape && !output.ends_with(RESET) {
        output.push_str(RESET);
    }

    output
}

fn terminal_size() -> Option<(usize, usize)> {
    if let Ok(columns) = env::var("COLUMNS") {
        if let Ok(lines) = env::var("LINES") {
            if let (Ok(width), Ok(height)) = (columns.parse::<usize>(), lines.parse::<usize>()) {
                if width > 0 && height > 0 {
                    return Some((width, height));
                }
            }
        }
    }

    terminal_size_from_ioctl()
}

#[cfg(unix)]
fn terminal_size_from_ioctl() -> Option<(usize, usize)> {
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
    if winsize.ws_col == 0 || winsize.ws_row == 0 {
        return None;
    }

    Some((usize::from(winsize.ws_col), usize::from(winsize.ws_row)))
}

#[cfg(not(unix))]
fn terminal_size_from_ioctl() -> Option<(usize, usize)> {
    None
}

#[cfg(test)]
mod tests {
    use super::{
        BATTERY_CELL_HEIGHT, BATTERY_OUTLINE_CHAR_HEIGHT, MonitorAction, MonitorFrame,
        battery_fill_cell_grid, battery_outline_cell_grid, centered_left_padding,
        fit_screen_to_terminal, format_report_ascii, format_report_hex, format_report_hex_compact,
        format_report_hex_wrapped_bytes, monitor_action_from_byte, pad_visible_line,
        signed_bar_fill_count, stick_char_marker_position, stick_percent_x, stick_percent_y,
        touchpad_char_marker_position, trigger_percent, truncate_ansi_line, truncate_chars,
        usb_battery_status, usb_sensor_readings, usb_touch_positions, wrap_plain_text,
    };
    use std::env;

    #[test]
    fn stick_center_maps_to_grid_center() {
        assert_eq!(stick_char_marker_position(128, 128, 17, 9), (8, 4));
    }

    #[test]
    fn stick_vertical_extremes_reach_top_and_bottom_cells() {
        assert_eq!(stick_char_marker_position(128, 0, 17, 9), (8, 1));
        assert_eq!(stick_char_marker_position(128, 255, 17, 9), (8, 7));
    }

    #[test]
    fn stick_edges_map_to_expected_percentages() {
        assert_eq!(stick_percent_x(255), 100);
        assert_eq!(stick_percent_x(0), -100);
        assert_eq!(stick_percent_y(0), 100);
        assert_eq!(stick_percent_y(255), -100);
    }

    #[test]
    fn trigger_percentage_covers_full_range() {
        assert_eq!(trigger_percent(0), 0);
        assert_eq!(trigger_percent(255), 100);
    }

    #[test]
    fn pad_visible_line_extends_shorter_text() {
        assert_eq!(pad_visible_line("abc", 5), "abc  ");
    }

    #[test]
    fn format_report_hex_handles_empty_reports() {
        assert_eq!(format_report_hex(&[]), "(none)");
    }

    #[test]
    fn format_report_hex_wrapped_bytes_breaks_at_32_bytes() {
        let report = (0u8..34).collect::<Vec<_>>();
        let formatted = format_report_hex_wrapped_bytes(&report, 32);
        let lines = formatted.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].split(' ').count(), 32);
        assert_eq!(lines[1], "20 21");
    }

    #[test]
    fn format_report_ascii_handles_empty_reports() {
        assert_eq!(format_report_ascii(&[]), "(none)");
    }

    #[test]
    fn format_report_ascii_escapes_non_printable_bytes() {
        assert_eq!(format_report_ascii(b"OK\r\n"), "\"OK\\r\\n\"");
    }

    #[test]
    fn format_report_hex_compact_shows_remaining_byte_count() {
        assert_eq!(
            format_report_hex_compact(&[0x10, 0x20, 0x30, 0x40], 2),
            "10 20 ... (+2B)"
        );
    }

    #[test]
    fn truncate_chars_appends_ellipsis() {
        assert_eq!(truncate_chars("abcdef", 5), "ab...");
    }

    #[test]
    fn wrap_plain_text_splits_long_lines_without_ellipsis() {
        assert_eq!(wrap_plain_text("abcdef", 3), vec!["abc", "def"]);
    }

    #[test]
    fn monitor_frame_hid_history_keeps_latest_five_entries() {
        let mut frame = MonitorFrame::idle();
        for index in 0..7 {
            frame.push_hid_history(vec![index as u8]);
        }

        assert_eq!(frame.hid_history.len(), 5);
        assert_eq!(frame.hid_history[0], vec![6]);
        assert_eq!(frame.hid_history[4], vec![2]);
    }

    #[test]
    fn monitor_frame_tx_history_keeps_latest_ten_entries() {
        let mut frame = MonitorFrame::idle();
        for index in 0..12 {
            frame.push_tx_history(vec![index as u8]);
        }

        assert_eq!(frame.tx_history.len(), 10);
        assert_eq!(frame.tx_history[0], vec![11]);
        assert_eq!(frame.tx_history[9], vec![2]);
    }

    #[test]
    fn monitor_frame_rx_history_keeps_latest_ten_entries() {
        let mut frame = MonitorFrame::idle();
        for index in 0..12 {
            frame.push_rx_history(format!("rx-{index}"));
        }

        assert_eq!(frame.rx_history.len(), 10);
        assert_eq!(frame.rx_history[0], "rx-11");
        assert_eq!(frame.rx_history[9], "rx-2");
    }

    #[test]
    fn centered_left_padding_uses_half_of_remaining_width() {
        assert_eq!(centered_left_padding(140, 120), 10);
    }

    #[test]
    fn truncate_ansi_line_respects_visible_width() {
        assert_eq!(
            truncate_ansi_line("\x1b[32mabcdef\x1b[0m", 3),
            "\x1b[32mabc\x1b[0m"
        );
    }

    #[test]
    fn fit_screen_to_terminal_limits_height_without_trailing_newline() {
        unsafe {
            env::set_var("COLUMNS", "10");
            env::set_var("LINES", "2");
        }
        let fitted = fit_screen_to_terminal("12345\n67890\nabcde\n", 5);
        assert_eq!(fitted, "  12345\n  67890");
    }

    #[test]
    fn usb_touch_positions_returns_first_active_touch_point_first() {
        let mut report = [0u8; 64];
        report[0] = 0x01;
        report[33] = 1;
        report[35] = 0x01;
        report[36] = 0xBC;
        report[37] = 0x3A;
        report[38] = 0x2D;

        assert_eq!(usb_touch_positions(&report)[0], Some((0x0ABC, 0x02D3)));
    }

    #[test]
    fn usb_touch_positions_capture_two_simultaneous_points() {
        let mut report = [0u8; 64];
        report[0] = 0x01;
        report[33] = 1;
        report[35] = 0x01;
        report[36] = 0xBC;
        report[37] = 0x3A;
        report[38] = 0x2D;
        report[39] = 0x02;
        report[40] = 0x34;
        report[41] = 0x62;
        report[42] = 0x19;

        assert_eq!(
            usb_touch_positions(&report),
            [Some((0x0ABC, 0x02D3)), Some((0x0234, 0x0196))]
        );
    }

    #[test]
    fn usb_battery_status_is_parsed_from_status_bytes() {
        let mut report = [0u8; 64];
        report[0] = 0x01;
        report[30] = 0x19;
        report[31] = 0x01;

        let battery = usb_battery_status(&report).expect("battery should parse");
        assert_eq!(battery.level, 9);
        assert_eq!(battery.percent, 90);
        assert!(!battery.full());
    }

    #[test]
    fn usb_battery_status_requires_full_usb_report() {
        let report = [0x01u8; 10];
        assert_eq!(usb_battery_status(&report), None);
    }

    #[test]
    fn usb_sensor_readings_are_parsed_from_raw_report() {
        let mut report = [0u8; 64];
        report[0] = 0x01;
        report[10..12].copy_from_slice(&0x1234u16.to_le_bytes());
        report[12] = 27;
        report[13..15].copy_from_slice(&1234i16.to_le_bytes());
        report[15..17].copy_from_slice(&(-2345i16).to_le_bytes());
        report[17..19].copy_from_slice(&3456i16.to_le_bytes());
        report[19..21].copy_from_slice(&(-4567i16).to_le_bytes());
        report[21..23].copy_from_slice(&5678i16.to_le_bytes());
        report[23..25].copy_from_slice(&(-6789i16).to_le_bytes());

        let sensors = usb_sensor_readings(&report).expect("sensor readings should parse");
        assert_eq!(sensors.timestamp, 0x1234);
        assert_eq!(sensors.temperature, 27);
        assert_eq!(sensors.gyro, [1234, -2345, 3456]);
        assert_eq!(sensors.accel, [-4567, 5678, -6789]);
    }

    #[test]
    fn usb_sensor_readings_require_sensor_bytes() {
        let report = [0x01u8; 24];
        assert_eq!(usb_sensor_readings(&report), None);
    }

    #[test]
    fn battery_bitmap_keeps_tip_gray_when_charge_is_full() {
        let mut report = [0u8; 64];
        report[0] = 0x01;
        report[30] = 0x1A;
        report[31] = 0x02;

        let fill = battery_fill_cell_grid(usb_battery_status(&report));
        let outline = battery_outline_cell_grid();
        assert_eq!(fill.len(), BATTERY_CELL_HEIGHT);
        assert_eq!(outline.len(), BATTERY_OUTLINE_CHAR_HEIGHT);
        assert_eq!(fill[1].last().copied().flatten(), Some(0xFF));
        assert_eq!(outline[2].last().copied().flatten(), Some(0x47));
    }

    #[test]
    fn signed_bar_fill_count_clamps_to_bar_width() {
        assert_eq!(signed_bar_fill_count(0, 4096), 0);
        assert_eq!(signed_bar_fill_count(2048, 4096), 2);
        assert_eq!(signed_bar_fill_count(9000, 4096), 4);
    }

    #[cfg(unix)]
    #[test]
    fn disable_terminal_input_echo_clears_echo_flags() {
        let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
        termios.c_lflag = libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG;
        termios.c_cc[libc::VMIN] = 1;
        termios.c_cc[libc::VTIME] = 1;

        super::disable_terminal_input_echo(&mut termios);

        assert_eq!(termios.c_lflag & libc::ECHO, 0);
        assert_eq!(termios.c_lflag & libc::ECHONL, 0);
        assert_eq!(termios.c_lflag & libc::ICANON, 0);
        assert_ne!(termios.c_lflag & libc::ISIG, 0);
        assert_eq!(termios.c_cc[libc::VMIN], 0);
        assert_eq!(termios.c_cc[libc::VTIME], 0);
    }

    #[test]
    fn touchpad_marker_position_stays_inside_bounds() {
        assert_eq!(touchpad_char_marker_position(0, 0, 27, 9), (0, 0));
        assert_eq!(touchpad_char_marker_position(1919, 942, 27, 9), (26, 7));
    }

    #[test]
    fn monitor_action_from_byte_maps_pause_and_quit_keys() {
        assert_eq!(
            monitor_action_from_byte(b' '),
            Some(MonitorAction::TogglePause)
        );
        assert_eq!(monitor_action_from_byte(b'q'), Some(MonitorAction::Quit));
        assert_eq!(monitor_action_from_byte(b'Q'), Some(MonitorAction::Quit));
        assert_eq!(monitor_action_from_byte(b'x'), None);
    }
}
