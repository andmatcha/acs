use super::common::{
    PortSpec, default_baud_rate, default_log_dir, next_value, parse_key_value_args,
    parse_port_spec, parse_u32_arg,
};
use super::help::{is_help_flag, print_io_help, print_send_help};
use super::signal;
use crate::ingress::IngressFrame;
use crate::output::OutputFormat;
use crate::output::formats::{DummyPayloadGenerator, crc16_ccitt_false};
use crate::port_display::{
    LineBreakMode, PortDisplayConfig, PortDisplayMode, parse_display_assignment,
    parse_display_value,
};
use crate::serial;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[cfg(unix)]
use std::io::Read;
use std::io::{self, IsTerminal, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const DEFAULT_SEND_RATE_HZ: u32 = 50;
const DEFAULT_IO_SEND_RATE_HZ: u32 = 10;
const SEND_LOOP_INTERVAL: Duration = Duration::from_millis(1);
const STATUS_INTERVAL: Duration = Duration::from_millis(200);
const RATE_WINDOW: Duration = Duration::from_secs(1);
const DISPLAY_FLUSH_SLICE: usize = 32;
const DISPLAY_FLUSH_PACKET_BUDGET: usize = 256;
const DISPLAY_QUEUE_LIMIT: usize = 65_536;
const SERIAL_FRAME_BITS_PER_BYTE: u64 = 10;
const ROVER_DOWN_GENERAL_PREFIX_LEN: usize = 6;
const ROVER_DOWN_GENERAL_MAX_PACKET_LEN: usize = 256;

#[derive(Debug, Default)]
struct SendCliOptions {
    port: Option<PortSpec>,
    outputs: Vec<SendOutputBinding>,
    baud: Option<u32>,
    rate_hz: Option<u32>,
    format: Option<String>,
    display: PortDisplayConfig,
    monitor_ports: Vec<SendMonitorBinding>,
    log_dir: Option<PathBuf>,
    no_log: bool,
    interactive: bool,
    s3b: bool,
    allow_receive_only: bool,
    title: Option<String>,
    command_name: Option<String>,
}

#[derive(Debug, Clone)]
struct SendOutputBinding {
    id: String,
    port: String,
    baud: Option<u32>,
    rate_hz: Option<u32>,
    format: Option<String>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
    line_break_mode: Option<crate::port_display::LineBreakMode>,
}

#[derive(Debug, Clone)]
struct SendMonitorBinding {
    port: String,
    baud: Option<u32>,
    formats: Vec<String>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
    line_break_mode: Option<crate::port_display::LineBreakMode>,
}

#[derive(Debug, Clone)]
struct SendOutputSettings {
    session: SessionOutputSpec,
    format: OutputFormat,
    rate_hz: u32,
}

#[derive(Debug, Clone)]
struct SendObservedInputSpec {
    input_id: String,
    port: String,
    formats: Vec<OutputFormat>,
    per_format_display_modes: Option<BTreeMap<OutputFormat, PortDisplayMode>>,
}

struct SendSettings {
    inputs: Vec<SessionInputSpec>,
    outputs: Vec<SendOutputSettings>,
    observed_inputs: Vec<SendObservedInputSpec>,
    log_dir: PathBuf,
    logging_enabled: bool,
    interactive: bool,
    s3b: bool,
    title: String,
    command_name: String,
    executed_command: Option<String>,
}

struct SendOutputRunResult {
    id: String,
    port: String,
    baud_rate: u32,
    format: OutputFormat,
    rate_hz: u32,
    sent_count: u64,
    payload_len: usize,
}

struct SendRunResult {
    interactive: bool,
    message_count: u64,
    outputs: Vec<SendOutputRunResult>,
    logging_enabled: bool,
    log_path: PathBuf,
    executed_command: Option<String>,
}

struct IoCommandInput {
    port: String,
    baud_rate: u32,
    display_mode: PortDisplayMode,
    line_break_mode: LineBreakMode,
    formats: Vec<OutputFormat>,
}

struct IoCommandOutput {
    id: String,
    port: String,
    baud_rate: u32,
    display_mode: PortDisplayMode,
    format: OutputFormat,
    rate_hz: u32,
}

struct OutputSchedule {
    next_send_at: Instant,
    period: Duration,
}

#[derive(Debug, Clone)]
struct SendHeaderOutput {
    id: String,
    port: String,
    baud_rate: u32,
    format: OutputFormat,
    rate_hz: Option<u32>,
    payload_len: Option<usize>,
}

#[derive(Debug, Clone)]
struct SendHeaderObservedFormat {
    port: String,
    format: OutputFormat,
}

struct SendRuntimeState {
    interactive: bool,
    logging_enabled: bool,
    log_path_display: String,
    header_outputs: Vec<SendHeaderOutput>,
    observed_inputs: BTreeMap<String, ObservedInput>,
    observed_format_lines: Vec<SendHeaderObservedFormat>,
    last_status_update: Instant,
    header_lines: Vec<String>,
}

impl SendRuntimeState {
    fn new(
        settings: &SendSettings,
        output_specs: &[SendOutputSettings],
        payload_lengths: &BTreeMap<String, usize>,
        log_path_display: String,
        started_at: Instant,
    ) -> Self {
        let header_outputs = output_specs
            .iter()
            .map(|output| SendHeaderOutput {
                id: output.session.id.clone(),
                port: output.session.port.clone(),
                baud_rate: output.session.baud_rate,
                format: output.format,
                rate_hz: (!settings.interactive).then_some(output.rate_hz),
                payload_len: payload_lengths.get(&output.session.id).copied(),
            })
            .collect::<Vec<_>>();
        let output_observed_pairs = header_outputs
            .iter()
            .map(|output| (output.port.clone(), output.format))
            .collect::<BTreeSet<_>>();
        let observed_inputs = settings
            .observed_inputs
            .iter()
            .cloned()
            .map(|spec| {
                (
                    spec.input_id.clone(),
                    ObservedInput::new(
                        spec.input_id,
                        spec.port,
                        spec.formats,
                        spec.per_format_display_modes,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let observed_format_lines = settings
            .observed_inputs
            .iter()
            .flat_map(|spec| {
                spec.formats.iter().filter_map(|format| {
                    let key = (spec.port.clone(), *format);
                    (!output_observed_pairs.contains(&key)).then_some(SendHeaderObservedFormat {
                        port: spec.port.clone(),
                        format: *format,
                    })
                })
            })
            .collect::<Vec<_>>();

        Self {
            interactive: settings.interactive,
            logging_enabled: settings.logging_enabled,
            log_path_display,
            header_outputs,
            observed_inputs,
            observed_format_lines,
            last_status_update: started_at
                .checked_sub(STATUS_INTERVAL)
                .unwrap_or(started_at),
            header_lines: Vec::new(),
        }
    }

    fn configure_session(&self, session: &mut SessionRuntime) {
        for input in self.observed_inputs.values() {
            session.set_manual_input_recording(&input.input_id, true);
            session.set_input_packet_rate_enabled(&input.port, true);
            session.set_input_known_formats(
                &input.port,
                input
                    .known_formats()
                    .iter()
                    .map(|format| format.display_name().to_owned())
                    .collect(),
            );
        }
    }

    fn handle_input(
        &mut self,
        frame: &IngressFrame,
        session: &mut SessionRuntime,
    ) -> Result<(), String> {
        let Some(input) = self.observed_inputs.get_mut(&frame.input_id) else {
            return Ok(());
        };

        let batch = input.observe(&frame.bytes, Instant::now());
        if batch.valid_packet_count > 0 {
            session.record_input_sample(
                &input.port,
                batch.valid_byte_len,
                batch.valid_packet_count,
            );
            for (format, (byte_len, packet_count)) in batch.per_format_totals {
                session.record_input_format_sample(
                    &input.port,
                    format.display_name(),
                    byte_len,
                    packet_count,
                );
            }
        }
        Ok(())
    }

    fn on_tick(&mut self, session: &mut SessionRuntime) -> Result<(), String> {
        self.flush_display_queues(session)?;

        let now = Instant::now();
        if now.duration_since(self.last_status_update) >= STATUS_INTERVAL {
            self.last_status_update = now;
            let header_lines = self.build_header_lines(now);
            if header_lines != self.header_lines {
                self.header_lines = header_lines.clone();
                session.set_header_lines(header_lines);
            }
        }

        Ok(())
    }

    fn flush_display_queues(&mut self, session: &mut SessionRuntime) -> Result<(), String> {
        let mut remaining_budget = DISPLAY_FLUSH_PACKET_BUDGET;

        while remaining_budget > 0 {
            let mut flushed_total = 0usize;
            for input in self.observed_inputs.values_mut() {
                if remaining_budget == 0 {
                    break;
                }
                let slice = remaining_budget.min(DISPLAY_FLUSH_SLICE);
                let flushed = input.flush_display_batch(session, slice)?;
                remaining_budget = remaining_budget.saturating_sub(flushed);
                flushed_total += flushed;
            }

            if flushed_total == 0 {
                break;
            }
        }

        Ok(())
    }

    fn build_header_lines(&mut self, now: Instant) -> Vec<String> {
        let mut lines = Vec::new();
        for output in self.header_outputs.clone() {
            let rx_rate_hz = self
                .rx_rate_hz(&output.port, output.format, now)
                .unwrap_or_default();
            if self.interactive {
                lines.push(format!(
                    "output[{}]: {} @ {} baud, format={}, rx={rx_rate_hz:.1} Hz",
                    output.id,
                    output.port,
                    output.baud_rate,
                    output.format.as_str()
                ));
            } else {
                lines.push(format!(
                    "output[{}]: {} @ {} baud, format={}, tx_target={} Hz, rx={rx_rate_hz:.1} Hz, payload={} bytes",
                    output.id,
                    output.port,
                    output.baud_rate,
                    output.format.as_str(),
                    output.rate_hz.unwrap_or_default(),
                    output.payload_len.unwrap_or_default()
                ));
            }
        }

        for observed in self.observed_format_lines.clone() {
            let rx_rate_hz = self
                .rx_rate_hz(&observed.port, observed.format, now)
                .unwrap_or_default();
            lines.push(format!(
                "input[{}:{}]: rx={rx_rate_hz:.1} Hz",
                observed.port,
                observed.format.as_str()
            ));
        }

        if self.logging_enabled {
            lines.push(format!("log: {}", self.log_path_display));
        } else {
            lines.push(String::from("log: disabled (--no-log)"));
        }
        if self.interactive {
            lines.push(String::from(
                "Enter で全出力ポートへ送信 (\\r\\n を末尾に付加)",
            ));
            lines.push(String::from("Ctrl-C で終了"));
        } else {
            lines.push(String::from("Space で表示を一時停止/再開  Ctrl-C で終了"));
        }
        lines
    }

    fn rx_rate_hz(&mut self, port: &str, format: OutputFormat, now: Instant) -> Option<f64> {
        self.observed_inputs
            .values_mut()
            .find(|input| input.port == port)
            .and_then(|input| input.packet_rate_hz(format, now))
    }
}

struct ObservedInput {
    input_id: String,
    port: String,
    formats: Vec<OutputFormat>,
    decoder: MixedFormatDecoder,
    display_queue: PacketDisplayQueue,
    rate_trackers: BTreeMap<OutputFormat, PacketRateTracker>,
    per_format_display_modes: Option<BTreeMap<OutputFormat, PortDisplayMode>>,
}

impl ObservedInput {
    fn new(
        input_id: String,
        port: String,
        formats: Vec<OutputFormat>,
        per_format_display_modes: Option<BTreeMap<OutputFormat, PortDisplayMode>>,
    ) -> Self {
        let rate_trackers = formats
            .iter()
            .map(|format| (*format, PacketRateTracker::new()))
            .collect();
        Self {
            input_id,
            port,
            formats: formats.clone(),
            decoder: MixedFormatDecoder::new(formats),
            display_queue: PacketDisplayQueue::new(),
            rate_trackers,
            per_format_display_modes,
        }
    }

    fn observe(&mut self, bytes: &[u8], at: Instant) -> ObservedInputBatch {
        let packets = self.decoder.push(bytes);
        let mut valid_byte_len = 0usize;
        let mut valid_packet_count = 0usize;
        let mut per_format_totals = BTreeMap::<OutputFormat, (usize, usize)>::new();

        for packet in packets {
            valid_byte_len += packet.bytes.len();
            valid_packet_count += 1;
            if let Some(rate_tracker) = self.rate_trackers.get_mut(&packet.format) {
                rate_tracker.record(packet.bytes.len(), at);
            }
            let totals = per_format_totals.entry(packet.format).or_insert((0, 0));
            totals.0 += packet.bytes.len();
            totals.1 += 1;
            let display_mode = if packet.format == OutputFormat::RoverUpGeneral {
                Some(PortDisplayMode::Ascii)
            } else {
                self.per_format_display_modes
                    .as_ref()
                    .and_then(|display_modes| display_modes.get(&packet.format).copied())
            };
            self.display_queue
                .enqueue(packet.bytes, display_mode, false);
        }

        ObservedInputBatch {
            valid_byte_len,
            valid_packet_count,
            per_format_totals,
        }
    }

    fn flush_display_batch(
        &mut self,
        session: &mut SessionRuntime,
        max_packets: usize,
    ) -> Result<usize, String> {
        self.display_queue
            .flush_input_batch(session, &self.port, max_packets)
    }

    fn packet_rate_hz(&mut self, format: OutputFormat, now: Instant) -> Option<f64> {
        self.rate_trackers
            .get_mut(&format)
            .map(|tracker| tracker.packets_per_second(now))
    }

    fn known_formats(&self) -> &[OutputFormat] {
        &self.formats
    }
}

struct ObservedInputBatch {
    valid_byte_len: usize,
    valid_packet_count: usize,
    per_format_totals: BTreeMap<OutputFormat, (usize, usize)>,
}

struct PacketRateTracker {
    samples: VecDeque<PacketRateSample>,
}

impl PacketRateTracker {
    fn new() -> Self {
        Self {
            samples: VecDeque::new(),
        }
    }

    fn record(&mut self, _byte_len: usize, at: Instant) {
        self.samples.push_back(PacketRateSample {
            at,
            packet_count: 1,
        });
    }

    fn packets_per_second(&mut self, now: Instant) -> f64 {
        self.prune(now);
        self.samples
            .iter()
            .map(|sample| sample.packet_count)
            .sum::<usize>() as f64
            / RATE_WINDOW.as_secs_f64()
    }

    fn prune(&mut self, now: Instant) {
        while let Some(sample) = self.samples.front() {
            if now.duration_since(sample.at) <= RATE_WINDOW {
                break;
            }
            self.samples.pop_front();
        }
    }
}

struct PacketRateSample {
    at: Instant,
    packet_count: usize,
}

struct PacketDisplayQueue {
    pending_packets: VecDeque<DisplayedPacket>,
}

impl PacketDisplayQueue {
    fn new() -> Self {
        Self {
            pending_packets: VecDeque::new(),
        }
    }

    fn enqueue(
        &mut self,
        packet: Vec<u8>,
        display_mode: Option<PortDisplayMode>,
        preserve_line_breaks: bool,
    ) {
        self.pending_packets.push_back(DisplayedPacket {
            bytes: packet,
            display_mode,
            preserve_line_breaks,
        });
        while self.pending_packets.len() > DISPLAY_QUEUE_LIMIT {
            self.pending_packets.pop_front();
        }
    }

    fn flush_input_batch(
        &mut self,
        session: &mut SessionRuntime,
        port: &str,
        max_packets: usize,
    ) -> Result<usize, String> {
        let mut flushed = 0usize;
        while flushed < max_packets {
            let Some(packet) = self.pending_packets.pop_front() else {
                break;
            };
            session.add_input_entry_with_options(
                port,
                &packet.bytes,
                packet.display_mode,
                packet.preserve_line_breaks,
            )?;
            flushed += 1;
        }
        Ok(flushed)
    }
}

struct DisplayedPacket {
    bytes: Vec<u8>,
    display_mode: Option<PortDisplayMode>,
    preserve_line_breaks: bool,
}

struct MixedFormatDecoder {
    formats: Vec<PacketMatcher>,
    buffer: Vec<u8>,
}

impl MixedFormatDecoder {
    fn new(formats: Vec<OutputFormat>) -> Self {
        Self {
            formats: formats.into_iter().map(PacketMatcher::new).collect(),
            buffer: Vec::new(),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Vec<DecodedPacket> {
        self.buffer.extend_from_slice(bytes);
        let mut packets = Vec::new();

        loop {
            if self.buffer.is_empty() {
                break;
            }

            if let Some(packet) = self.try_decode_packet() {
                packets.push(packet);
                continue;
            }

            if self
                .formats
                .iter()
                .any(|format| format.could_match_prefix(&self.buffer))
            {
                break;
            }

            self.buffer.drain(..1);
        }

        packets
    }

    fn try_decode_packet(&mut self) -> Option<DecodedPacket> {
        for format in &self.formats {
            if let Some(packet_len) = format.matching_packet_len(&self.buffer) {
                return Some(DecodedPacket {
                    format: format.format(),
                    bytes: self.buffer.drain(..packet_len).collect(),
                });
            }
        }

        None
    }
}

struct DecodedPacket {
    format: OutputFormat,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
enum PacketMatcher {
    PacketAcV6,
    PacketMv1,
    PacketIv1,
    PacketBv1,
    PacketJfV1,
    RoverUpGeneral,
    RoverDownGeneral,
}

impl PacketMatcher {
    fn new(format: OutputFormat) -> Self {
        match format {
            OutputFormat::PacketAcV6 => Self::PacketAcV6,
            OutputFormat::PacketMv1 => Self::PacketMv1,
            OutputFormat::PacketIv1 => Self::PacketIv1,
            OutputFormat::PacketBv1 => Self::PacketBv1,
            OutputFormat::PacketJfV1 => Self::PacketJfV1,
            OutputFormat::RoverUpGeneral => Self::RoverUpGeneral,
            OutputFormat::RoverDownGeneral => Self::RoverDownGeneral,
        }
    }

    fn format(self) -> OutputFormat {
        match self {
            Self::PacketAcV6 => OutputFormat::PacketAcV6,
            Self::PacketMv1 => OutputFormat::PacketMv1,
            Self::PacketIv1 => OutputFormat::PacketIv1,
            Self::PacketBv1 => OutputFormat::PacketBv1,
            Self::PacketJfV1 => OutputFormat::PacketJfV1,
            Self::RoverUpGeneral => OutputFormat::RoverUpGeneral,
            Self::RoverDownGeneral => OutputFormat::RoverDownGeneral,
        }
    }

    fn packet_len(self) -> usize {
        self.format().packet_len()
    }

    fn matching_packet_len(self, bytes: &[u8]) -> Option<usize> {
        if matches!(self, Self::RoverDownGeneral) {
            return rover_down_packet_len(bytes);
        }

        if bytes.len() < self.packet_len() {
            return None;
        }

        self.matches_packet(&bytes[..self.packet_len()])
            .then_some(self.packet_len())
    }

    fn matches_packet(self, bytes: &[u8]) -> bool {
        match self {
            Self::PacketAcV6 => matches_crc_packet(bytes, b"AC", 37),
            Self::PacketMv1 => matches_reduced_ac_packet(bytes, b'M', 19),
            Self::PacketIv1 => matches_reduced_ac_packet(bytes, b'I', 19),
            Self::PacketBv1 => matches_reduced_ac_packet(bytes, b'B', 15),
            Self::PacketJfV1 => matches_crc_packet(bytes, b"JF", 14),
            Self::RoverUpGeneral => matches_rover_up_packet(bytes),
            Self::RoverDownGeneral => matches_rover_down_packet(bytes),
        }
    }

    fn could_match_prefix(self, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }

        match self {
            Self::PacketAcV6 => could_match_crc_packet_prefix(bytes, b"AC", 39),
            Self::PacketMv1 => could_match_reduced_ac_packet_prefix(bytes, b'M', 19),
            Self::PacketIv1 => could_match_reduced_ac_packet_prefix(bytes, b'I', 19),
            Self::PacketBv1 => could_match_reduced_ac_packet_prefix(bytes, b'B', 15),
            Self::PacketJfV1 => could_match_crc_packet_prefix(bytes, b"JF", 16),
            Self::RoverUpGeneral => matches_rover_up_prefix(bytes),
            Self::RoverDownGeneral => matches_rover_down_prefix(bytes),
        }
    }
}

fn matches_crc_packet(bytes: &[u8], header: &[u8; 2], payload_len: usize) -> bool {
    if bytes.len() != payload_len + 2 {
        return false;
    }

    if !bytes.starts_with(header) {
        return false;
    }

    let expected_crc = u16::from_le_bytes([bytes[payload_len], bytes[payload_len + 1]]);
    crc16_ccitt_false(&bytes[..payload_len]) == expected_crc
}

fn could_match_crc_packet_prefix(bytes: &[u8], header: &[u8; 2], packet_len: usize) -> bool {
    if bytes.len() >= packet_len {
        return false;
    }

    if bytes.len() <= header.len() {
        return header[..bytes.len()] == bytes[..];
    }

    bytes.starts_with(header)
}

fn matches_reduced_ac_packet(bytes: &[u8], header: u8, packet_len: usize) -> bool {
    bytes.len() == packet_len && bytes.first().copied() == Some(header)
}

fn could_match_reduced_ac_packet_prefix(bytes: &[u8], header: u8, packet_len: usize) -> bool {
    if bytes.len() >= packet_len {
        return false;
    }

    bytes.first().copied() == Some(header)
}

fn matches_rover_up_prefix(bytes: &[u8]) -> bool {
    if bytes.len() > OutputFormat::RoverUpGeneral.packet_len() {
        return false;
    }

    bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| rover_up_byte_matches(index, *byte))
}

fn matches_rover_up_packet(bytes: &[u8]) -> bool {
    bytes.len() == OutputFormat::RoverUpGeneral.packet_len() && matches_rover_up_prefix(bytes)
}

fn rover_up_byte_matches(index: usize, byte: u8) -> bool {
    match index {
        0 => byte == b'0',
        1 => byte == b'x',
        2 => byte == b'3',
        3 | 4 => byte.is_ascii_hexdigit(),
        5 => byte == b',',
        6..=9 => byte.is_ascii_digit(),
        10 => byte == b'\r',
        11 => byte == b'\n',
        _ => false,
    }
}

fn matches_rover_down_prefix(bytes: &[u8]) -> bool {
    if bytes.len() > ROVER_DOWN_GENERAL_MAX_PACKET_LEN {
        return false;
    }

    if bytes.len() <= ROVER_DOWN_GENERAL_PREFIX_LEN {
        return bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| rover_down_header_byte_matches(index, *byte));
    }

    if !rover_down_header_matches(bytes) {
        return false;
    }

    let data = &bytes[ROVER_DOWN_GENERAL_PREFIX_LEN..];
    if let Some(end_index) = find_crlf(data) {
        let packet_len = ROVER_DOWN_GENERAL_PREFIX_LEN + end_index + 2;
        return packet_len == bytes.len() && matches_rover_down_packet(&bytes[..packet_len]);
    }

    data.iter().enumerate().all(|(index, byte)| match *byte {
        b'\n' => false,
        b'\r' => index + 1 == data.len(),
        _ => true,
    })
}

fn matches_rover_down_packet(bytes: &[u8]) -> bool {
    if bytes.len() < ROVER_DOWN_GENERAL_PREFIX_LEN + 3
        || bytes.len() > ROVER_DOWN_GENERAL_MAX_PACKET_LEN
        || !bytes.ends_with(b"\r\n")
        || !rover_down_header_matches(bytes)
    {
        return false;
    }

    let data = &bytes[ROVER_DOWN_GENERAL_PREFIX_LEN..bytes.len() - 2];
    !data.is_empty() && !data.iter().any(|byte| matches!(*byte, b'\r' | b'\n'))
}

fn rover_down_packet_len(bytes: &[u8]) -> Option<usize> {
    if !rover_down_header_matches(bytes) {
        return None;
    }
    let packet_len =
        ROVER_DOWN_GENERAL_PREFIX_LEN + find_crlf(&bytes[ROVER_DOWN_GENERAL_PREFIX_LEN..])? + 2;
    matches_rover_down_packet(&bytes[..packet_len]).then_some(packet_len)
}

fn rover_down_header_matches(bytes: &[u8]) -> bool {
    bytes.len() >= ROVER_DOWN_GENERAL_PREFIX_LEN
        && bytes[..ROVER_DOWN_GENERAL_PREFIX_LEN]
            .iter()
            .enumerate()
            .all(|(index, byte)| rover_down_header_byte_matches(index, *byte))
}

fn rover_down_header_byte_matches(index: usize, byte: u8) -> bool {
    match index {
        0 => byte == b'0',
        1 => byte == b'x',
        2 => matches!(byte, b'3' | b'4'),
        3 | 4 => byte.is_ascii_hexdigit(),
        5 => byte == b',',
        _ => false,
    }
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\r\n")
}

pub(crate) fn run(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_send_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let cli_options = match parse_send_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_send_help(bin_name);
            return ExitCode::from(2);
        }
    };

    match run_with_options(cli_options) {
        Ok(result) => {
            if result.interactive {
                if result.outputs.len() == 1 {
                    let output = &result.outputs[0];
                    println!(
                        "sent {} messages to {} @ {} baud",
                        result.message_count, output.port, output.baud_rate
                    );
                } else {
                    println!(
                        "sent {} messages to {} outputs",
                        result.message_count,
                        result.outputs.len()
                    );
                    for output in &result.outputs {
                        println!(
                            "  {}: {} writes to {} @ {} baud",
                            output.id, output.sent_count, output.port, output.baud_rate
                        );
                    }
                }
            } else if result.outputs.len() == 1 {
                let output = &result.outputs[0];
                println!(
                    "sent {} packets ({} bytes each) to {} @ {} baud, format={}, target={} Hz",
                    output.sent_count,
                    output.payload_len,
                    output.port,
                    output.baud_rate,
                    output.format.as_str(),
                    output.rate_hz
                );
            } else {
                println!("sent dummy packets to {} outputs", result.outputs.len());
                for output in &result.outputs {
                    println!(
                        "  {}: {} packets ({} bytes each) to {} @ {} baud, format={}, target={} Hz",
                        output.id,
                        output.sent_count,
                        output.payload_len,
                        output.port,
                        output.baud_rate,
                        output.format.as_str(),
                        output.rate_hz
                    );
                }
            }
            if result.logging_enabled {
                println!("log saved to {}", result.log_path.display());
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

#[derive(Debug, Default)]
struct IoCliOptions {
    inputs: Vec<IoBindingArg>,
    outputs: Vec<IoBindingArg>,
    send_options: SendCliOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IoBindingArg {
    Provided(String),
    Prompt,
}

pub(crate) fn run_io(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_io_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let io_options = match parse_io_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_io_help(bin_name);
            return ExitCode::from(2);
        }
    };

    let send_options = match resolve_io_options(io_options) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    match run_with_options(send_options) {
        Ok(result) => {
            print_io_result(&result);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn print_io_result(result: &SendRunResult) {
    if result.outputs.is_empty() {
        println!("io session finished (receive only)");
    } else if result.outputs.len() == 1 {
        let output = &result.outputs[0];
        println!(
            "sent {} packets ({} bytes each) to {} @ {} baud, format={}, target={} Hz",
            output.sent_count,
            output.payload_len,
            output.port,
            output.baud_rate,
            output.format.as_str(),
            output.rate_hz
        );
    } else {
        println!("sent dummy packets to {} outputs", result.outputs.len());
        for output in &result.outputs {
            println!(
                "  {}: {} packets ({} bytes each) to {} @ {} baud, format={}, target={} Hz",
                output.id,
                output.sent_count,
                output.payload_len,
                output.port,
                output.baud_rate,
                output.format.as_str(),
                output.rate_hz
            );
        }
    }
    if result.logging_enabled {
        println!("log saved to {}", result.log_path.display());
    }
    if let Some(command) = &result.executed_command {
        println!("実行コマンド: {command}");
    }
}

fn parse_io_args(args: Vec<String>) -> Result<IoCliOptions, String> {
    let mut options = IoCliOptions {
        inputs: Vec::new(),
        outputs: Vec::new(),
        send_options: SendCliOptions {
            rate_hz: Some(DEFAULT_IO_SEND_RATE_HZ),
            allow_receive_only: true,
            title: Some(String::from("acs io")),
            command_name: Some(String::from("io")),
            ..SendCliOptions::default()
        },
    };
    let mut iter = args.into_iter().peekable();

    while let Some(arg) = iter.next() {
        if let Some(value) = strip_io_value(&arg, &["-i", "--input", "--input-port"]) {
            options.inputs.push(IoBindingArg::Provided(value));
            continue;
        }
        if let Some(value) = strip_io_value(&arg, &["-o", "--output", "--output-port"]) {
            options.outputs.push(IoBindingArg::Provided(value));
            continue;
        }

        match arg.as_str() {
            "--input" | "--input-port" | "-i" => {
                options.inputs.push(
                    next_optional_io_binding(&mut iter)
                        .map_or(IoBindingArg::Prompt, IoBindingArg::Provided),
                );
            }
            "--output" | "--output-port" | "-o" => {
                options.outputs.push(
                    next_optional_io_binding(&mut iter)
                        .map_or(IoBindingArg::Prompt, IoBindingArg::Provided),
                );
            }
            "--config" => apply_send_config_args(
                &mut options.send_options,
                &next_value(&mut iter, "--config")?,
            )?,
            "--baud" | "-b" => {
                let value = next_value(&mut iter, "--baud")?;
                options.send_options.baud = Some(parse_u32_arg("--baud", &value)?);
            }
            "--rate" | "-r" => {
                let value = next_value(&mut iter, "--rate")?;
                options.send_options.rate_hz = Some(parse_u32_arg("--rate", &value)?);
            }
            "--format" | "-f" => {
                options.send_options.format = Some(next_value(&mut iter, "--format")?)
            }
            "--display" => {
                let value = next_value(&mut iter, "--display")?;
                let assignment = parse_display_assignment(&value)?;
                assignment.apply_to(&mut options.send_options.display);
            }
            "--log-dir" => {
                options.send_options.log_dir =
                    Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            "--no-log" => options.send_options.no_log = true,
            "--s3b" => options.send_options.s3b = true,
            other => return Err(format!("unknown option for io: {other}")),
        }
    }

    if options.inputs.is_empty() && options.outputs.is_empty() {
        return Err(String::from(
            "io requires at least one -i/--input or -o/--output",
        ));
    }

    Ok(options)
}

fn strip_io_value(arg: &str, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| arg.strip_prefix(&format!("{name}=")))
        .map(str::to_owned)
}

fn next_optional_io_binding(
    iter: &mut std::iter::Peekable<impl Iterator<Item = String>>,
) -> Option<String> {
    match iter.peek() {
        Some(next) if !next.starts_with('-') => iter.next(),
        _ => None,
    }
}

fn resolve_io_options(io_options: IoCliOptions) -> Result<SendCliOptions, String> {
    let mut send_options = io_options.send_options;

    for input in io_options.inputs {
        let value = match input {
            IoBindingArg::Provided(value) => value,
            IoBindingArg::Prompt => prompt_io_input_binding()?,
        };
        send_options
            .monitor_ports
            .push(parse_send_monitor_binding(&value)?);
    }

    for output in io_options.outputs {
        let value = match output {
            IoBindingArg::Provided(value) => value,
            IoBindingArg::Prompt => prompt_io_output_binding()?,
        };
        send_options
            .outputs
            .push(parse_send_output_binding(&value)?);
    }

    Ok(send_options)
}

fn prompt_io_input_binding() -> Result<String, String> {
    let port = prompt_serial_port("受信ポートを選択")?;
    let baud = prompt_u32_choice(
        "受信ボーレート",
        default_baud_rate(),
        &[115_200, 921_600, 460_800, 230_400, 57_600, 38_400, 9_600],
    )?;
    let format = prompt_input_format()?;
    let display = prompt_display_mode("受信表示形式", true)?;

    Ok(format_io_input_binding(
        &port,
        baud,
        display.as_deref(),
        format.as_deref(),
    ))
}

fn prompt_io_output_binding() -> Result<String, String> {
    let port = prompt_serial_port("送信ポートを選択")?;
    let baud = prompt_u32_choice(
        "送信ボーレート",
        default_baud_rate(),
        &[115_200, 921_600, 460_800, 230_400, 57_600, 38_400, 9_600],
    )?;
    let display = prompt_display_mode("送信表示形式", false)?;
    let format = prompt_output_format()?;
    let rate_hz = prompt_u32_choice(
        "送信レート (Hz)",
        DEFAULT_IO_SEND_RATE_HZ,
        &[10, 50, 100, 20, 1],
    )?;

    Ok(format_io_output_binding(
        &port,
        baud,
        display.as_deref(),
        &format,
        rate_hz,
    ))
}

fn format_io_input_binding(
    port: &str,
    baud: u32,
    display: Option<&str>,
    format: Option<&str>,
) -> String {
    let mut value = format!("{port}@{baud}");
    if let Some(display) = display {
        value.push(',');
        value.push_str(display);
    }
    if let Some(format) = format {
        value.push(',');
        value.push_str(format);
    }
    value
}

fn format_io_output_binding(
    port: &str,
    baud: u32,
    display: Option<&str>,
    format: &str,
    rate_hz: u32,
) -> String {
    let mut value = format!("{port}@{baud}");
    if let Some(display) = display {
        value.push(',');
        value.push_str(display);
    }
    value.push(',');
    value.push_str(format);
    value.push(',');
    value.push_str(&rate_hz.to_string());
    value
}

fn build_io_executed_command(
    inputs: &[IoCommandInput],
    outputs: &[IoCommandOutput],
    log_dir: Option<&PathBuf>,
    no_log: bool,
    s3b: bool,
) -> String {
    let mut args = vec![String::from("acs"), String::from("io")];

    for input in inputs {
        args.push(String::from("-i"));
        args.push(format_io_input_binding(
            &input.port,
            input.baud_rate,
            Some(&format_input_display_value(
                input.display_mode,
                input.line_break_mode,
            )),
            format_output_format_list(&input.formats).as_deref(),
        ));
    }

    for output in outputs {
        args.push(String::from("-o"));
        args.push(format_io_output_command_binding(output));
    }

    if let Some(log_dir) = log_dir {
        args.push(String::from("--log-dir"));
        args.push(log_dir.display().to_string());
    }
    if no_log {
        args.push(String::from("--no-log"));
    }
    if s3b {
        args.push(String::from("--s3b"));
    }

    args.iter()
        .map(|arg| shell_quote_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_io_output_command_binding(output: &IoCommandOutput) -> String {
    let binding = format_io_output_binding(
        &output.port,
        output.baud_rate,
        Some(display_mode_value(output.display_mode)),
        output.format.as_str(),
        output.rate_hz,
    );
    if output.id == output.port {
        binding
    } else {
        format!("{}={binding}", output.id)
    }
}

fn format_output_format_list(formats: &[OutputFormat]) -> Option<String> {
    (!formats.is_empty()).then(|| {
        formats
            .iter()
            .map(|format| format.as_str())
            .collect::<Vec<_>>()
            .join("+")
    })
}

fn format_input_display_value(
    display_mode: PortDisplayMode,
    line_break_mode: LineBreakMode,
) -> String {
    format!(
        "{}+{}",
        display_mode_value(display_mode),
        line_break_mode_value(line_break_mode)
    )
}

fn display_mode_value(mode: PortDisplayMode) -> &'static str {
    match mode {
        PortDisplayMode::Hex => "hex",
        PortDisplayMode::Ascii => "ascii",
        PortDisplayMode::Utf8 => "utf8",
        PortDisplayMode::HexAscii => "hex+ascii",
        PortDisplayMode::HexUtf8 => "hex+utf8",
    }
}

fn line_break_mode_value(mode: LineBreakMode) -> &'static str {
    match mode {
        LineBreakMode::Line => "line",
        LineBreakMode::Packet => "packet",
        LineBreakMode::Wrap => "wrap",
    }
}

fn shell_quote_arg(value: &str) -> String {
    if value.is_empty() {
        return String::from("''");
    }

    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:@=,+".contains(&byte))
    {
        return value.to_owned();
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}

fn prompt_serial_port(prompt: &str) -> Result<String, String> {
    let ports = serial::available_ports().map_err(|error| error.to_string())?;
    if ports.is_empty() {
        return prompt_text(&format!("{prompt}: "), None);
    }

    let mut labels = ports
        .iter()
        .map(format_serial_port_choice)
        .collect::<Vec<_>>();
    labels.push(String::from("手入力..."));
    let selected = choose_from_menu(prompt, &labels, 0)?;
    if selected == ports.len() {
        prompt_text("ポート名: ", None)
    } else {
        Ok(ports[selected].port_name.clone())
    }
}

fn format_serial_port_choice(port: &serialport::SerialPortInfo) -> String {
    match &port.port_type {
        serialport::SerialPortType::UsbPort(info) => format!(
            "{}  usb vid=0x{:04x} pid=0x{:04x} product={}",
            port.port_name,
            info.vid,
            info.pid,
            info.product.as_deref().unwrap_or("unknown")
        ),
        serialport::SerialPortType::BluetoothPort => {
            format!("{}  bluetooth", port.port_name)
        }
        serialport::SerialPortType::PciPort => format!("{}  pci", port.port_name),
        serialport::SerialPortType::Unknown => port.port_name.clone(),
    }
}

fn prompt_u32_choice(prompt: &str, default: u32, choices: &[u32]) -> Result<u32, String> {
    let mut values = Vec::new();
    values.push(default);
    for choice in choices {
        if !values.contains(choice) {
            values.push(*choice);
        }
    }
    let mut labels = values
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    labels.push(String::from("手入力..."));

    let selected = choose_from_menu(prompt, &labels, 0)?;
    if selected == values.len() {
        let value = prompt_text(&format!("{prompt}: "), Some(&default.to_string()))?;
        let parsed = parse_u32_arg(prompt, &value)?;
        if parsed == 0 {
            return Err(format!("{prompt} must be greater than 0"));
        }
        Ok(parsed)
    } else {
        Ok(values[selected])
    }
}

fn prompt_output_format() -> Result<String, String> {
    let formats = output_format_choices();
    let labels = formats
        .iter()
        .map(|format| format.display_name().to_owned())
        .collect::<Vec<_>>();
    let selected = choose_from_menu("送信フォーマット", &labels, 0)?;
    Ok(formats[selected].as_str().to_owned())
}

fn prompt_input_format() -> Result<Option<String>, String> {
    let formats = output_format_choices();
    let mut labels = vec![String::from("raw (フォーマット指定なし)")];
    labels.extend(
        formats
            .iter()
            .map(|format| format.display_name().to_owned()),
    );
    labels.push(String::from("複数/手入力..."));

    let selected = choose_from_menu("受信フォーマット", &labels, 0)?;
    if selected == 0 {
        return Ok(None);
    }
    if selected == formats.len() + 1 {
        let value = prompt_text("フォーマット (例: packetacv6+packetjfv1): ", None)?;
        let formats = parse_output_format_list(&value)
            .ok_or_else(|| format!("invalid input format list: {value}"))?;
        return Ok(Some(formats.join("+")));
    }
    Ok(Some(formats[selected - 1].as_str().to_owned()))
}

fn prompt_display_mode(prompt: &str, input: bool) -> Result<Option<String>, String> {
    let mut choices = vec![
        String::from("default"),
        String::from("hex"),
        String::from("ascii"),
        String::from("utf8"),
        String::from("hex+ascii"),
        String::from("hex+utf8"),
    ];
    if input {
        choices.extend([
            String::from("hex+packet"),
            String::from("utf8+packet"),
            String::from("hex+utf8+wrap"),
        ]);
    }
    choices.push(String::from("手入力..."));

    let selected = choose_from_menu(prompt, &choices, 0)?;
    if selected == 0 {
        return Ok(None);
    }
    if selected == choices.len() - 1 {
        let value = prompt_text(&format!("{prompt}: "), Some("hex+utf8"))?;
        parse_display_value(&value)?;
        Ok(Some(value))
    } else {
        Ok(Some(choices[selected].clone()))
    }
}

fn output_format_choices() -> Vec<OutputFormat> {
    vec![
        OutputFormat::PacketAcV6,
        OutputFormat::PacketMv1,
        OutputFormat::PacketIv1,
        OutputFormat::PacketBv1,
        OutputFormat::PacketJfV1,
        OutputFormat::RoverUpGeneral,
        OutputFormat::RoverDownGeneral,
    ]
}

fn choose_from_menu(
    prompt: &str,
    labels: &[String],
    default_index: usize,
) -> Result<usize, String> {
    if labels.is_empty() {
        return Err(format!("{prompt}: no choices available"));
    }

    let default_index = default_index.min(labels.len() - 1);
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return choose_from_numbered_prompt(prompt, labels, default_index);
    }

    choose_from_menu_interactive(prompt, labels, default_index)
}

#[cfg(unix)]
fn choose_from_menu_interactive(
    prompt: &str,
    labels: &[String],
    default_index: usize,
) -> Result<usize, String> {
    let _raw_mode = RawTerminalMode::enable()?;
    let mut selected = default_index;
    let mut stdin = io::stdin();

    loop {
        render_menu(prompt, labels, selected)?;
        let mut byte = [0u8; 1];
        stdin
            .read_exact(&mut byte)
            .map_err(|error| format!("failed to read input: {error}"))?;
        match byte[0] {
            b'\r' | b'\n' => {
                clear_screen()?;
                return Ok(selected);
            }
            3 => return Err(String::from("interrupted")),
            b'k' => selected = selected.saturating_sub(1),
            b'j' => {
                if selected + 1 < labels.len() {
                    selected += 1;
                }
            }
            0x1b => {
                let mut seq = [0u8; 2];
                if stdin.read_exact(&mut seq).is_ok() && seq[0] == b'[' {
                    match seq[1] {
                        b'A' => selected = selected.saturating_sub(1),
                        b'B' => {
                            if selected + 1 < labels.len() {
                                selected += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(not(unix))]
fn choose_from_menu_interactive(
    prompt: &str,
    labels: &[String],
    default_index: usize,
) -> Result<usize, String> {
    choose_from_numbered_prompt(prompt, labels, default_index)
}

fn render_menu(prompt: &str, labels: &[String], selected: usize) -> Result<(), String> {
    const SELECTED_BG: &str = "\x1b[48;5;218m\x1b[30m";
    const RESET: &str = "\x1b[0m";

    clear_screen()?;
    println!("{prompt}");
    println!("↑/↓ で選択、Enter で決定");
    for (index, label) in labels.iter().enumerate() {
        if index == selected {
            println!("{SELECTED_BG}> {label}{RESET}");
        } else {
            println!("  {label}");
        }
    }
    io::stdout()
        .flush()
        .map_err(|error| format!("failed to flush output: {error}"))
}

fn clear_screen() -> Result<(), String> {
    print!("\x1b[2J\x1b[H");
    io::stdout()
        .flush()
        .map_err(|error| format!("failed to flush output: {error}"))
}

fn choose_from_numbered_prompt(
    prompt: &str,
    labels: &[String],
    default_index: usize,
) -> Result<usize, String> {
    println!("{prompt}");
    for (index, label) in labels.iter().enumerate() {
        println!("  {}. {}", index + 1, label);
    }
    let default = (default_index + 1).to_string();
    let value = prompt_text("番号: ", Some(&default))?;
    let selected = value
        .parse::<usize>()
        .map_err(|_| format!("invalid selection: {value}"))?;
    if selected == 0 || selected > labels.len() {
        return Err(format!("selection out of range: {value}"));
    }
    Ok(selected - 1)
}

fn prompt_text(prompt: &str, default: Option<&str>) -> Result<String, String> {
    match default {
        Some(default) => print!("{prompt}[{default}] "),
        None => print!("{prompt}"),
    }
    io::stdout()
        .flush()
        .map_err(|error| format!("failed to flush output: {error}"))?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|error| format!("failed to read input: {error}"))?;
    let value = value.trim();
    if value.is_empty() {
        default
            .map(str::to_owned)
            .ok_or_else(|| String::from("empty value"))
    } else {
        Ok(value.to_owned())
    }
}

#[cfg(unix)]
struct RawTerminalMode {
    fd: i32,
    original: libc::termios,
}

#[cfg(unix)]
impl RawTerminalMode {
    fn enable() -> Result<Self, String> {
        let fd = io::stdin().as_raw_fd();
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err(format!(
                "failed to read terminal settings: {}",
                io::Error::last_os_error()
            ));
        }

        let mut raw = original;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(format!(
                "failed to update terminal settings: {}",
                io::Error::last_os_error()
            ));
        }

        Ok(Self { fd, original })
    }
}

#[cfg(unix)]
impl Drop for RawTerminalMode {
    fn drop(&mut self) {
        let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.original) };
    }
}

fn run_with_options(cli_options: SendCliOptions) -> Result<SendRunResult, String> {
    let settings = build_settings(cli_options)?;
    let output_specs = settings.outputs.clone();

    let mut payload_lengths = BTreeMap::new();
    let mut generators = BTreeMap::<String, Box<dyn DummyPayloadGenerator>>::new();
    if !settings.interactive {
        for output in &output_specs {
            payload_lengths.insert(
                output.session.id.clone(),
                output.format.encode_dummy_payload()?.len(),
            );
            generators.insert(
                output.session.id.clone(),
                output.format.create_dummy_generator()?,
            );
        }
    }

    let started_at = Instant::now();
    let mut session = SessionRuntime::new(SessionSpec {
        title: settings.title.clone(),
        command_name: settings.command_name.clone(),
        log_dir: settings.log_dir.clone(),
        logging_enabled: settings.logging_enabled,
        xbee_s3b_recovery: settings.s3b,
        inputs: settings.inputs.clone(),
        outputs: output_specs
            .iter()
            .map(|output| output.session.clone())
            .collect(),
    })?;
    for output in &output_specs {
        session.set_output_packet_rate_enabled(&output.session.port, true);
    }
    let log_path = session.log_path().to_path_buf();
    let log_path_display = log_path.display().to_string();
    let runtime_state = RefCell::new(SendRuntimeState::new(
        &settings,
        &output_specs,
        &payload_lengths,
        log_path_display,
        started_at,
    ));
    runtime_state.borrow().configure_session(&mut session);
    let initial_header_lines = runtime_state.borrow_mut().build_header_lines(started_at);
    session.set_header_lines(initial_header_lines);
    signal::install_handler();

    let mut sent_counts = output_specs
        .iter()
        .map(|output| (output.session.id.clone(), 0u64))
        .collect::<BTreeMap<_, _>>();
    let mut output_has_error = output_specs
        .iter()
        .map(|output| (output.session.id.clone(), false))
        .collect::<BTreeMap<_, _>>();
    let mut last_errors = BTreeMap::<String, String>::new();
    let mut message_count = 0u64;

    if settings.interactive {
        session.set_interactive_input(true);
        session.run_loop_with_tick(
            SEND_LOOP_INTERVAL,
            signal::is_stop_requested,
            |frame, session| runtime_state.borrow_mut().handle_input(frame, session),
            |session| {
                if let Some(input) = session.take_user_input() {
                    message_count = message_count.saturating_add(1);
                    let mut bytes = input.into_bytes();
                    bytes.extend_from_slice(b"\r\n");

                    for output in &output_specs {
                        let output_id = &output.session.id;
                        match session.write_output(output_id, &bytes) {
                            Ok(()) => {
                                if output_has_error.get(output_id).copied().unwrap_or(false) {
                                    session.clear_output_error(output_id)?;
                                    output_has_error.insert(output_id.clone(), false);
                                }
                                *sent_counts.entry(output_id.clone()).or_insert(0) += 1;
                                last_errors.remove(output_id);
                            }
                            Err(error) => {
                                session.set_output_error(output_id, &error)?;
                                output_has_error.insert(output_id.clone(), true);
                                last_errors.insert(output_id.clone(), error);
                            }
                        }
                    }
                }
                runtime_state.borrow_mut().on_tick(session)?;
                Ok(())
            },
        )?;
    } else {
        let mut schedules = output_specs
            .iter()
            .map(|output| {
                (
                    output.session.id.clone(),
                    OutputSchedule {
                        next_send_at: started_at,
                        period: Duration::from_secs_f64(1.0 / output.rate_hz as f64),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        session.run_loop_with_tick(
            SEND_LOOP_INTERVAL,
            signal::is_stop_requested,
            |frame, session| runtime_state.borrow_mut().handle_input(frame, session),
            |session| {
                let now = Instant::now();
                for output in &output_specs {
                    let output_id = &output.session.id;
                    let schedule = schedules
                        .get_mut(output_id)
                        .ok_or_else(|| format!("missing send schedule for output `{output_id}`"))?;
                    if now >= schedule.next_send_at {
                        realign_output_schedule(schedule, now);
                        let payload = generators
                            .get_mut(output_id)
                            .ok_or_else(|| {
                                format!("missing dummy generator for output `{output_id}`")
                            })?
                            .next_payload()?;
                        match session.write_output(output_id, &payload) {
                            Ok(()) => {
                                if output_has_error.get(output_id).copied().unwrap_or(false) {
                                    session.clear_output_error(output_id)?;
                                    output_has_error.insert(output_id.clone(), false);
                                }
                                *sent_counts.entry(output_id.clone()).or_insert(0) += 1;
                                last_errors.remove(output_id);
                            }
                            Err(error) => {
                                session.set_output_error(output_id, &error)?;
                                output_has_error.insert(output_id.clone(), true);
                                last_errors.insert(output_id.clone(), error);
                            }
                        }
                        schedule.next_send_at += schedule.period;
                    }
                }
                runtime_state.borrow_mut().on_tick(session)?;
                Ok(())
            },
        )?;

        if sent_counts.values().all(|count| *count == 0)
            && let Some(error) = last_errors.into_values().next()
        {
            return Err(error);
        }
    }

    Ok(SendRunResult {
        interactive: settings.interactive,
        message_count,
        outputs: output_specs
            .into_iter()
            .map(|output| SendOutputRunResult {
                id: output.session.id.clone(),
                port: output.session.port,
                baud_rate: output.session.baud_rate,
                format: output.format,
                rate_hz: output.rate_hz,
                sent_count: sent_counts.remove(&output.session.id).unwrap_or_default(),
                payload_len: payload_lengths
                    .remove(&output.session.id)
                    .unwrap_or_default(),
            })
            .collect(),
        logging_enabled: settings.logging_enabled,
        log_path,
        executed_command: settings.executed_command,
    })
}

fn build_settings(cli_options: SendCliOptions) -> Result<SendSettings, String> {
    let using_cli_outputs = !cli_options.outputs.is_empty();
    let using_cli_port = cli_options.port.is_some();
    if using_cli_outputs && using_cli_port {
        return Err(String::from(
            "cannot combine --port with --output-port; use one style or the other",
        ));
    }

    let default_baud = cli_options.baud.unwrap_or_else(default_baud_rate);
    let default_rate_hz = cli_options.rate_hz.unwrap_or(DEFAULT_SEND_RATE_HZ);
    if default_rate_hz == 0 {
        return Err(String::from("--rate must be greater than 0"));
    }
    let default_format_name = cli_options
        .format
        .clone()
        .unwrap_or_else(|| String::from("packetacv6"));
    let default_format = OutputFormat::parse(&default_format_name)?;
    let output_bindings = if cli_options.allow_receive_only && !using_cli_outputs && !using_cli_port
    {
        Vec::new()
    } else {
        resolve_output_bindings(&cli_options, default_baud, default_rate_hz, default_format)?
    };
    let monitor_port_specs = resolve_monitor_bindings(&cli_options)?;
    if output_bindings.is_empty() && monitor_port_specs.is_empty() {
        return Err(String::from(
            "at least one input or output port is required",
        ));
    }
    let display = cli_options.display;
    let explicit_log_dir = cli_options.log_dir.clone();
    let log_dir = cli_options.log_dir.unwrap_or_else(default_log_dir);
    let title = cli_options
        .title
        .clone()
        .unwrap_or_else(|| String::from("acs send"));
    let command_name = cli_options
        .command_name
        .clone()
        .unwrap_or_else(|| String::from("send"));
    let capture_io_command = command_name == "io";

    let mut outputs = Vec::new();
    let mut inputs = Vec::new();
    let mut io_command_inputs = Vec::new();
    let mut io_command_outputs = Vec::new();
    let mut seen_output_ids = BTreeSet::new();
    let mut output_port_bauds = BTreeMap::<String, u32>::new();
    let mut input_packet_format_candidates = BTreeMap::<String, Vec<OutputFormat>>::new();
    let mut input_display_is_explicit = BTreeMap::<String, bool>::new();

    for binding in output_bindings {
        if !seen_output_ids.insert(binding.id.clone()) {
            return Err(format!("duplicate send output id: {}", binding.id));
        }

        let port = serial::resolve_port(Some(&binding.port)).map_err(|error| error.to_string())?;
        let baud_rate = binding.baud.unwrap_or(default_baud);
        if let Some(existing_baud_rate) = output_port_bauds.get(&port) {
            if *existing_baud_rate != baud_rate {
                return Err(format!(
                    "send output port `{port}` cannot use multiple baud rates ({existing_baud_rate} and {baud_rate})"
                ));
            }
        } else {
            output_port_bauds.insert(port.clone(), baud_rate);
        }
        let format = binding.format.unwrap_or(default_format);
        let output_display_mode = resolve_display_mode(
            binding.display_mode,
            display.resolve_output_override(&port),
            Some(format.default_display_mode()),
        );
        let input_display_override = binding
            .display_mode
            .or(display.resolve_input_override(&port));
        let input_display_mode = input_display_override.unwrap_or(format.default_display_mode());
        let line_break_mode = binding
            .line_break_mode
            .unwrap_or(display.resolve_line_break_input(&port));
        input_display_is_explicit
            .entry(port.clone())
            .and_modify(|explicit| *explicit |= input_display_override.is_some())
            .or_insert(input_display_override.is_some());

        outputs.push(SendOutputSettings {
            session: SessionOutputSpec {
                id: binding.id.clone(),
                port: port.clone(),
                baud_rate,
                format_name: format.as_str().to_owned(),
                display_mode: output_display_mode,
            },
            format,
            rate_hz: binding.rate_hz,
        });
        if capture_io_command {
            io_command_outputs.push(IoCommandOutput {
                id: binding.id.clone(),
                port: port.clone(),
                baud_rate,
                display_mode: output_display_mode,
                format,
                rate_hz: binding.rate_hz,
            });
        }
        if !cli_options.interactive {
            let formats = input_packet_format_candidates
                .entry(port.clone())
                .or_default();
            if !formats.contains(&format) {
                formats.push(format);
            }
        }
        if !inputs
            .iter()
            .any(|input: &SessionInputSpec| input.port == port && input.baud_rate == baud_rate)
        {
            inputs.push(SessionInputSpec {
                id: port.clone(),
                port: port.clone(),
                baud_rate,
                display_mode: input_display_mode,
                line_break_mode,
            });
        }
    }

    if !cli_options.interactive {
        validate_output_port_loads(&outputs)?;
    }

    for port_spec in monitor_port_specs {
        let monitor_port =
            serial::resolve_port(Some(&port_spec.port)).map_err(|error| error.to_string())?;
        let monitor_formats = port_spec.formats;
        let monitor_has_formats = !monitor_formats.is_empty();
        let baud_rate = port_spec.baud.unwrap_or(default_baud);
        let display_mode = resolve_display_mode(
            port_spec.display_mode,
            display.resolve_input_override(&monitor_port),
            monitor_formats
                .first()
                .copied()
                .map(OutputFormat::default_display_mode),
        );
        let line_break_mode = resolve_monitor_line_break_mode(
            port_spec.line_break_mode,
            monitor_has_formats,
            display.resolve_line_break_input(&monitor_port),
        );
        if capture_io_command {
            io_command_inputs.push(IoCommandInput {
                port: monitor_port.clone(),
                baud_rate,
                display_mode,
                line_break_mode,
                formats: monitor_formats.clone(),
            });
        }
        if !inputs
            .iter()
            .any(|input: &SessionInputSpec| input.port == monitor_port)
        {
            inputs.push(SessionInputSpec {
                id: monitor_port.clone(),
                port: monitor_port.clone(),
                baud_rate,
                display_mode,
                line_break_mode,
            });
        }
        if !monitor_formats.is_empty() {
            let input_display_override = port_spec
                .display_mode
                .or(display.resolve_input_override(&monitor_port));
            input_display_is_explicit
                .entry(monitor_port.clone())
                .and_modify(|explicit| *explicit |= input_display_override.is_some())
                .or_insert(input_display_override.is_some());
            let formats = input_packet_format_candidates
                .entry(monitor_port)
                .or_default();
            for format in monitor_formats {
                if !formats.contains(&format) {
                    formats.push(format);
                }
            }
        }
    }

    let observed_inputs = input_packet_format_candidates
        .into_iter()
        .filter_map(|(port, mut formats)| {
            if formats.is_empty() {
                return None;
            }
            formats.sort_by_key(|format| format.as_str());
            let explicit_input_display = input_display_is_explicit
                .get(&port)
                .copied()
                .unwrap_or(false);
            let per_format_display_modes = if explicit_input_display || formats.len() <= 1 {
                None
            } else {
                Some(
                    formats
                        .iter()
                        .map(|format| (*format, format.default_display_mode()))
                        .collect(),
                )
            };
            let input_id = inputs
                .iter()
                .find(|input| input.port == port)
                .map(|input| input.id.clone())?;
            Some(SendObservedInputSpec {
                input_id,
                port,
                formats,
                per_format_display_modes,
            })
        })
        .collect();
    let executed_command = capture_io_command.then(|| {
        build_io_executed_command(
            &io_command_inputs,
            &io_command_outputs,
            explicit_log_dir.as_ref(),
            cli_options.no_log,
            cli_options.s3b,
        )
    });

    Ok(SendSettings {
        inputs,
        outputs,
        observed_inputs,
        log_dir,
        logging_enabled: !cli_options.no_log,
        interactive: cli_options.interactive,
        s3b: cli_options.s3b,
        title,
        command_name,
        executed_command,
    })
}

fn resolve_display_mode(
    explicit_mode: Option<PortDisplayMode>,
    configured_mode: Option<PortDisplayMode>,
    built_in_mode: Option<PortDisplayMode>,
) -> PortDisplayMode {
    explicit_mode
        .or(configured_mode)
        .or(built_in_mode)
        .unwrap_or_default()
}

fn resolve_monitor_line_break_mode(
    explicit_mode: Option<LineBreakMode>,
    has_formats: bool,
    configured_mode: LineBreakMode,
) -> LineBreakMode {
    explicit_mode.unwrap_or(if has_formats {
        configured_mode
    } else {
        LineBreakMode::Wrap
    })
}

fn validate_output_port_loads(outputs: &[SendOutputSettings]) -> Result<(), String> {
    let mut loads = BTreeMap::<(String, u32), Vec<(String, u32, u64)>>::new();

    for output in outputs {
        let estimated_bps = estimated_output_line_bps(output.format, output.rate_hz);
        loads
            .entry((output.session.port.clone(), output.session.baud_rate))
            .or_default()
            .push((
                output.format.display_name().to_owned(),
                output.rate_hz,
                estimated_bps,
            ));
    }

    for ((port, baud_rate), entries) in loads {
        let total_bps = entries.iter().map(|(_, _, bps)| *bps).sum::<u64>();
        if total_bps > baud_rate as u64 {
            let detail = entries
                .into_iter()
                .map(|(format_name, rate_hz, bps)| format!("{format_name} {rate_hz}Hz={bps}bps"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "send output load on `{port}` @ {baud_rate} baud exceeds serial capacity: estimated {total_bps} bps assuming 10 bits/byte ({detail}) > {baud_rate} baud. Reduce rates or increase baud."
            ));
        }
    }

    Ok(())
}

fn estimated_output_line_bps(format: OutputFormat, rate_hz: u32) -> u64 {
    format.packet_len() as u64 * SERIAL_FRAME_BITS_PER_BYTE * rate_hz as u64
}

fn realign_output_schedule(schedule: &mut OutputSchedule, now: Instant) {
    if now <= schedule.next_send_at || schedule.period.is_zero() {
        return;
    }

    let overdue = now.duration_since(schedule.next_send_at);
    if overdue < schedule.period {
        return;
    }

    let skipped_periods = (overdue.as_secs_f64() / schedule.period.as_secs_f64()).floor() as u32;
    if skipped_periods > 0 {
        if let Some(advance) = schedule.period.checked_mul(skipped_periods) {
            schedule.next_send_at += advance;
        } else {
            schedule.next_send_at = now;
        }
    }
}

fn resolve_output_bindings(
    cli_options: &SendCliOptions,
    default_baud: u32,
    default_rate_hz: u32,
    default_format: OutputFormat,
) -> Result<Vec<ResolvedSendOutputBinding>, String> {
    if !cli_options.outputs.is_empty() {
        return cli_options
            .outputs
            .iter()
            .cloned()
            .map(|binding| {
                resolve_send_output_binding(binding, default_baud, default_rate_hz, default_format)
            })
            .collect();
    }

    let selected_port = cli_options.port.clone().and_then(PortSpec::normalized);
    let port = match &selected_port {
        Some(port_spec) => {
            serial::resolve_port(Some(&port_spec.port)).map_err(|error| error.to_string())?
        }
        None => serial::resolve_port(None).map_err(|error| error.to_string())?,
    };

    resolve_send_output_binding(
        SendOutputBinding {
            id: String::from("main"),
            port,
            baud: selected_port.as_ref().and_then(|port_spec| port_spec.baud),
            rate_hz: None,
            format: None,
            display_mode: selected_port
                .as_ref()
                .and_then(|port_spec| port_spec.display_mode),
            line_break_mode: selected_port
                .as_ref()
                .and_then(|port_spec| port_spec.line_break_mode),
        },
        default_baud,
        default_rate_hz,
        default_format,
    )
    .map(|binding| vec![binding])
}

#[derive(Debug, Clone)]
struct ResolvedSendOutputBinding {
    id: String,
    port: String,
    baud: Option<u32>,
    rate_hz: u32,
    format: Option<OutputFormat>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
    line_break_mode: Option<crate::port_display::LineBreakMode>,
}

fn resolve_send_output_binding(
    binding: SendOutputBinding,
    _default_baud: u32,
    default_rate_hz: u32,
    default_format: OutputFormat,
) -> Result<ResolvedSendOutputBinding, String> {
    let rate_hz = binding.rate_hz.unwrap_or(default_rate_hz);
    if rate_hz == 0 {
        return Err(String::from("send output rate must be greater than 0"));
    }

    Ok(ResolvedSendOutputBinding {
        id: binding.id,
        port: binding.port,
        baud: binding.baud,
        rate_hz,
        format: Some(match binding.format {
            Some(format) => OutputFormat::parse(&format)?,
            None => default_format,
        }),
        display_mode: binding.display_mode,
        line_break_mode: binding.line_break_mode,
    })
}

#[derive(Debug, Clone)]
struct ResolvedSendMonitorBinding {
    port: String,
    baud: Option<u32>,
    formats: Vec<OutputFormat>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
    line_break_mode: Option<crate::port_display::LineBreakMode>,
}

fn resolve_monitor_bindings(
    cli_options: &SendCliOptions,
) -> Result<Vec<ResolvedSendMonitorBinding>, String> {
    cli_options
        .monitor_ports
        .iter()
        .cloned()
        .map(resolve_send_monitor_binding)
        .collect()
}

fn resolve_send_monitor_binding(
    binding: SendMonitorBinding,
) -> Result<ResolvedSendMonitorBinding, String> {
    Ok(ResolvedSendMonitorBinding {
        port: binding.port,
        baud: binding.baud,
        formats: binding
            .formats
            .iter()
            .map(|format| OutputFormat::parse(format))
            .collect::<Result<Vec<_>, _>>()?,
        display_mode: binding.display_mode,
        line_break_mode: binding.line_break_mode,
    })
}

fn parse_send_args(args: Vec<String>) -> Result<SendCliOptions, String> {
    let mut options = SendCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--port" | "-p" => {
                options.port = Some(parse_port_spec(
                    "--port",
                    &next_value(&mut iter, "--port")?,
                )?)
            }
            "--output-port" | "-o" => options.outputs.push(parse_send_output_binding(
                &next_value(&mut iter, "--output-port")?,
            )?),
            "--config" => {
                apply_send_config_args(&mut options, &next_value(&mut iter, "--config")?)?
            }
            "--baud" | "-b" => {
                let value = next_value(&mut iter, "--baud")?;
                options.baud = Some(parse_u32_arg("--baud", &value)?);
            }
            "--rate" | "-r" => {
                let value = next_value(&mut iter, "--rate")?;
                options.rate_hz = Some(parse_u32_arg("--rate", &value)?);
            }
            "--format" | "-f" => options.format = Some(next_value(&mut iter, "--format")?),
            "--display" => {
                let value = next_value(&mut iter, "--display")?;
                let assignment = parse_display_assignment(&value)?;
                assignment.apply_to(&mut options.display);
            }
            "--monitor" | "-m" => {
                options
                    .monitor_ports
                    .push(parse_send_monitor_binding(&next_value(
                        &mut iter,
                        "--monitor",
                    )?)?)
            }
            "--interactive" | "-i" => options.interactive = true,
            "--log-dir" => {
                options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            "--no-log" => options.no_log = true,
            "--s3b" => options.s3b = true,
            other => return Err(format!("unknown option for send: {other}")),
        }
    }

    Ok(options)
}

fn apply_send_config_args(options: &mut SendCliOptions, value: &str) -> Result<(), String> {
    for assignment in parse_key_value_args("--config", value)? {
        let key = assignment.key.to_ascii_uppercase();
        match key.as_str() {
            "RATE" => options.rate_hz = Some(parse_u32_arg("RATE", &assignment.value)?),
            "FORMAT" => options.format = Some(assignment.value),
            "DISPLAY" => {
                let display = parse_display_assignment(&assignment.value)?;
                display.apply_to(&mut options.display);
            }
            "LOG_DIR" => options.log_dir = Some(PathBuf::from(assignment.value)),
            other => return Err(format!("unknown send config key: {other}")),
        }
    }

    Ok(())
}

fn parse_send_monitor_binding(value: &str) -> Result<SendMonitorBinding, String> {
    if value.is_empty() {
        return Err(String::from("send monitor binding must not be empty"));
    }

    let (port_text, formats) = if let Some((port_text, format_names)) = value.rsplit_once(',') {
        if let Some(formats) = parse_output_format_list(format_names) {
            (port_text, formats)
        } else {
            (value, Vec::new())
        }
    } else {
        (value, Vec::new())
    };

    let port_spec = parse_port_spec("--monitor", port_text)?;
    Ok(SendMonitorBinding {
        port: port_spec.port,
        baud: port_spec.baud,
        formats,
        display_mode: port_spec.display_mode,
        line_break_mode: port_spec.line_break_mode,
    })
}

fn parse_output_format_list(value: &str) -> Option<Vec<String>> {
    let mut formats = Vec::new();

    for candidate in value.split('+') {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            return None;
        }

        let format = OutputFormat::parse(candidate).ok()?;
        let canonical = format.as_str().to_owned();
        if !formats.contains(&canonical) {
            formats.push(canonical);
        }
    }

    (!formats.is_empty()).then_some(formats)
}

fn parse_send_output_binding(value: &str) -> Result<SendOutputBinding, String> {
    if value.is_empty() {
        return Err(String::from("send output binding must not be empty"));
    }

    let mut binding_text = value;
    let mut rate_hz = None;
    if let Some((candidate_binding, candidate_rate)) = binding_text.rsplit_once(',')
        && !candidate_rate.is_empty()
        && candidate_rate.chars().all(|ch| ch.is_ascii_digit())
    {
        rate_hz = Some(parse_u32_arg("send output rate", candidate_rate)?);
        binding_text = candidate_binding;
    }
    let (binding_text, format) =
        if let Some((candidate_binding, format_name)) = binding_text.rsplit_once(',') {
            if OutputFormat::parse(format_name).is_ok() {
                (candidate_binding, Some(format_name.to_owned()))
            } else {
                (binding_text, None)
            }
        } else {
            (binding_text, None)
        };

    let (id, port_text) = if let Some((id, port_text)) = binding_text.split_once('=') {
        if id.is_empty() || port_text.is_empty() {
            return Err(format!("invalid send output binding: {value}"));
        }
        (Some(id.to_owned()), port_text)
    } else {
        (None, binding_text)
    };

    let port_spec = parse_port_spec("send output binding", port_text)?;
    Ok(SendOutputBinding {
        id: id.unwrap_or_else(|| port_spec.port.clone()),
        port: port_spec.port,
        baud: port_spec.baud,
        rate_hz,
        format,
        display_mode: port_spec.display_mode,
        line_break_mode: port_spec.line_break_mode,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        IoCommandInput, IoCommandOutput, MixedFormatDecoder, ObservedInput, OutputSchedule,
        SendOutputSettings, build_io_executed_command, estimated_output_line_bps,
        format_io_input_binding, format_io_output_binding, matches_rover_down_packet,
        parse_io_args, parse_output_format_list, parse_send_args, parse_send_monitor_binding,
        parse_send_output_binding, realign_output_schedule, resolve_display_mode,
        resolve_monitor_line_break_mode, validate_output_port_loads,
    };
    use crate::output::OutputFormat;
    use crate::port_display::{LineBreakMode, PortDisplayMode};
    use crate::session::runtime::SessionOutputSpec;
    use std::path::PathBuf;
    use std::time::Instant;

    #[test]
    fn parse_send_args_accepts_port_baud_and_format() {
        let options = parse_send_args(vec![
            String::from("--port"),
            String::from("/dev/ttyUSB0"),
            String::from("--baud"),
            String::from("921600"),
            String::from("--rate"),
            String::from("100"),
            String::from("--format"),
            String::from("PacketACv6"),
            String::from("--monitor"),
            String::from("/dev/ttyUSB1"),
        ])
        .expect("should parse");

        assert_eq!(
            options.port.as_ref().map(|port| port.port.as_str()),
            Some("/dev/ttyUSB0")
        );
        assert_eq!(options.baud, Some(921_600));
        assert_eq!(options.rate_hz, Some(100));
        assert_eq!(options.format.as_deref(), Some("PacketACv6"));
        assert_eq!(options.monitor_ports.len(), 1);
        assert_eq!(options.monitor_ports[0].port, "/dev/ttyUSB1");
        assert_eq!(options.monitor_ports[0].baud, None);
        assert!(options.monitor_ports[0].formats.is_empty());
    }

    #[test]
    fn parse_send_monitor_binding_accepts_format_and_packet_mode() {
        let binding =
            parse_send_monitor_binding("/dev/ttyUSB1@115200,utf8+line,packetjfv1").unwrap();

        assert_eq!(binding.port, "/dev/ttyUSB1");
        assert_eq!(binding.baud, Some(115_200));
        assert_eq!(binding.formats, vec![String::from("packetjfv1")]);
        assert_eq!(binding.display_mode, Some(PortDisplayMode::Utf8));
        assert_eq!(binding.line_break_mode, Some(LineBreakMode::Line));
    }

    #[test]
    fn parse_send_monitor_binding_accepts_multiple_formats() {
        let binding =
            parse_send_monitor_binding("/dev/ttyUSB1,packetacv6+packetmv1+packetjfv1").unwrap();

        assert_eq!(binding.port, "/dev/ttyUSB1");
        assert_eq!(
            binding.formats,
            vec![
                String::from("packetacv6"),
                String::from("packetmv1"),
                String::from("packetjfv1"),
            ]
        );
    }

    #[test]
    fn monitor_without_format_defaults_to_wrap_mode() {
        assert_eq!(
            resolve_monitor_line_break_mode(None, false, LineBreakMode::Line),
            LineBreakMode::Wrap
        );
    }

    #[test]
    fn monitor_with_format_uses_configured_line_break_mode() {
        assert_eq!(
            resolve_monitor_line_break_mode(None, true, LineBreakMode::Packet),
            LineBreakMode::Packet
        );
        assert_eq!(
            resolve_monitor_line_break_mode(
                Some(LineBreakMode::Line),
                false,
                LineBreakMode::Packet
            ),
            LineBreakMode::Line
        );
    }

    #[test]
    fn parse_send_output_binding_accepts_id_format_and_packet_mode() {
        let binding =
            parse_send_output_binding("main=/dev/ttyUSB0@921600,hex+packet,packetacv6,100")
                .unwrap();

        assert_eq!(binding.id, "main");
        assert_eq!(binding.port, "/dev/ttyUSB0");
        assert_eq!(binding.baud, Some(921_600));
        assert_eq!(binding.rate_hz, Some(100));
        assert_eq!(binding.format.as_deref(), Some("packetacv6"));
        assert_eq!(binding.display_mode, Some(PortDisplayMode::Hex));
        assert_eq!(binding.line_break_mode, Some(LineBreakMode::Packet));
    }

    #[test]
    fn parse_send_args_accepts_display_and_log_dir() {
        let options = parse_send_args(vec![
            String::from("--display"),
            String::from("output:default=hex"),
            String::from("--log-dir"),
            String::from("tmp/send-logs"),
            String::from("--no-log"),
        ])
        .expect("should parse");

        assert_eq!(
            options.display.resolve_output("/dev/ttyUSB0"),
            PortDisplayMode::Hex
        );
        assert_eq!(
            options.log_dir,
            Some(std::path::PathBuf::from("tmp/send-logs"))
        );
        assert!(options.no_log);
    }

    #[test]
    fn parse_send_args_accepts_config_aliases() {
        let options = parse_send_args(vec![
            String::from("--port"),
            String::from("/dev/ttyUSB0@921600"),
            String::from("--config"),
            String::from(
                "FORMAT=PacketACv6,RATE=100,DISPLAY=output:default=hex,LOG_DIR=tmp/send-logs",
            ),
            String::from("--interactive"),
        ])
        .expect("should parse");

        assert_eq!(
            options.port.as_ref().map(|port| port.port.as_str()),
            Some("/dev/ttyUSB0")
        );
        assert_eq!(
            options.port.as_ref().and_then(|port| port.baud),
            Some(921_600)
        );
        assert_eq!(options.rate_hz, Some(100));
        assert_eq!(options.format.as_deref(), Some("PacketACv6"));
        assert_eq!(
            options.log_dir,
            Some(std::path::PathBuf::from("tmp/send-logs"))
        );
        assert!(options.interactive);
    }

    #[test]
    fn parse_io_args_accepts_input_output_values_and_defaults_rate() {
        let options = parse_io_args(vec![
            String::from("-i"),
            String::from("/dev/ttyUSB1@115200,utf8,packetjfv1"),
            String::from("-o"),
            String::from("main=/dev/ttyUSB0@921600,hex,packetacv6,100"),
            String::from("--no-log"),
        ])
        .expect("should parse");

        assert_eq!(options.inputs.len(), 1);
        assert_eq!(options.outputs.len(), 1);
        assert_eq!(options.send_options.rate_hz, Some(10));
        assert!(options.send_options.allow_receive_only);
        assert!(options.send_options.no_log);
    }

    #[test]
    fn parse_io_args_accepts_bare_input_output_for_prompting() {
        let options =
            parse_io_args(vec![String::from("-i"), String::from("-o")]).expect("should parse");

        assert_eq!(options.inputs.len(), 1);
        assert_eq!(options.outputs.len(), 1);
    }

    #[test]
    fn io_binding_format_matches_send_parsers() {
        let input = format_io_input_binding(
            "/dev/ttyUSB1",
            115_200,
            Some("utf8+packet"),
            Some("packetacv6+packetjfv1"),
        );
        let output =
            format_io_output_binding("/dev/ttyUSB0", 921_600, Some("hex"), "packetacv6", 10);

        let input = parse_send_monitor_binding(&input).expect("input binding should parse");
        let output = parse_send_output_binding(&output).expect("output binding should parse");

        assert_eq!(input.port, "/dev/ttyUSB1");
        assert_eq!(input.baud, Some(115_200));
        assert_eq!(
            input.formats,
            vec![String::from("packetacv6"), String::from("packetjfv1")]
        );
        assert_eq!(input.display_mode, Some(PortDisplayMode::Utf8));
        assert_eq!(input.line_break_mode, Some(LineBreakMode::Packet));
        assert_eq!(output.port, "/dev/ttyUSB0");
        assert_eq!(output.baud, Some(921_600));
        assert_eq!(output.format.as_deref(), Some("packetacv6"));
        assert_eq!(output.rate_hz, Some(10));
    }

    #[test]
    fn io_executed_command_includes_resolved_ports_defaults_and_flags() {
        let command = build_io_executed_command(
            &[IoCommandInput {
                port: String::from("/dev/ttyUSB1"),
                baud_rate: 115_200,
                display_mode: PortDisplayMode::Utf8,
                line_break_mode: LineBreakMode::Packet,
                formats: vec![OutputFormat::PacketAcV6, OutputFormat::PacketJfV1],
            }],
            &[IoCommandOutput {
                id: String::from("ac"),
                port: String::from("/dev/ttyUSB0"),
                baud_rate: 921_600,
                display_mode: PortDisplayMode::Hex,
                format: OutputFormat::PacketAcV6,
                rate_hz: 10,
            }],
            Some(&PathBuf::from("tmp/io logs")),
            true,
            true,
        );

        assert_eq!(
            command,
            "acs io -i /dev/ttyUSB1@115200,utf8+packet,packetacv6+packetjfv1 -o ac=/dev/ttyUSB0@921600,hex,packetacv6,10 --log-dir 'tmp/io logs' --no-log --s3b"
        );
    }

    #[test]
    fn mixed_decoder_recovers_packet_boundaries_across_chunks() {
        let ac = OutputFormat::PacketAcV6
            .encode_dummy_payload()
            .expect("packetacv6 dummy payload");
        let m = OutputFormat::PacketMv1
            .encode_dummy_payload()
            .expect("packetmv1 dummy payload");
        let up = OutputFormat::RoverUpGeneral
            .encode_dummy_payload()
            .expect("roverupgeneral dummy payload");
        let mut decoder = MixedFormatDecoder::new(vec![
            OutputFormat::PacketAcV6,
            OutputFormat::PacketMv1,
            OutputFormat::RoverUpGeneral,
        ]);

        assert!(decoder.push(&ac[..7]).is_empty());

        let mut decoded = decoder.push(&[&ac[7..], &m[..4]].concat());
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].format, OutputFormat::PacketAcV6);
        assert_eq!(decoded[0].bytes, ac);

        decoded = decoder.push(&[&m[4..], &up[..5]].concat());
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].format, OutputFormat::PacketMv1);
        assert_eq!(decoded[0].bytes, m);

        decoded = decoder.push(&up[5..]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].format, OutputFormat::RoverUpGeneral);
        assert_eq!(decoded[0].bytes, up);
    }

    #[test]
    fn observed_input_forces_rover_up_receive_display_to_ascii() {
        let packet = OutputFormat::RoverUpGeneral
            .encode_dummy_payload()
            .expect("roverupgeneral dummy payload");
        let mut observed = ObservedInput::new(
            String::from("monitor-up"),
            String::from("/dev/ttyUSB1"),
            vec![OutputFormat::RoverUpGeneral],
            None,
        );

        let batch = observed.observe(&packet, Instant::now());

        assert_eq!(batch.valid_packet_count, 1);
        assert_eq!(batch.valid_byte_len, packet.len());
        assert_eq!(observed.display_queue.pending_packets.len(), 1);
        let queued = observed
            .display_queue
            .pending_packets
            .front()
            .expect("queued roverup packet");
        assert_eq!(queued.display_mode, Some(PortDisplayMode::Ascii));
        assert!(!queued.preserve_line_breaks);
        assert_eq!(queued.bytes, packet);
    }

    #[test]
    fn estimated_output_line_bps_uses_packet_length_and_rate() {
        assert_eq!(
            estimated_output_line_bps(OutputFormat::PacketAcV6, 100),
            39_000
        );
        assert_eq!(
            estimated_output_line_bps(OutputFormat::RoverUpGeneral, 100),
            12_000
        );
    }

    #[test]
    fn validate_output_port_loads_rejects_oversubscribed_shared_port() {
        let outputs = vec![
            SendOutputSettings {
                session: SessionOutputSpec {
                    id: String::from("arm"),
                    port: String::from("/dev/ttyUSB0"),
                    baud_rate: 115_200,
                    format_name: String::from("packetacv6"),
                    display_mode: PortDisplayMode::Hex,
                },
                format: OutputFormat::PacketAcV6,
                rate_hz: 1_000,
            },
            SendOutputSettings {
                session: SessionOutputSpec {
                    id: String::from("rover"),
                    port: String::from("/dev/ttyUSB0"),
                    baud_rate: 115_200,
                    format_name: String::from("roverupgeneral"),
                    display_mode: PortDisplayMode::Ascii,
                },
                format: OutputFormat::RoverUpGeneral,
                rate_hz: 1_000,
            },
        ];

        let error = validate_output_port_loads(&outputs).expect_err("should reject");
        assert!(error.contains("exceeds serial capacity"));
        assert!(error.contains("510000 bps"));
    }

    #[test]
    fn realign_output_schedule_discards_backlog_after_long_stall() {
        let started_at = Instant::now();
        let mut schedule = OutputSchedule {
            next_send_at: started_at,
            period: std::time::Duration::from_millis(10),
        };

        realign_output_schedule(
            &mut schedule,
            started_at + std::time::Duration::from_millis(55),
        );

        assert_eq!(
            schedule.next_send_at.duration_since(started_at),
            std::time::Duration::from_millis(50)
        );
    }

    #[test]
    fn rover_down_validator_accepts_documented_dummy_payload() {
        let payload = OutputFormat::RoverDownGeneral
            .encode_dummy_payload()
            .expect("roverdowngeneral dummy payload");

        assert!(matches_rover_down_packet(&payload));
    }

    #[test]
    fn rover_down_validator_accepts_0x3xx_and_0x4xx_lines_with_arbitrary_data() {
        assert!(matches_rover_down_packet(b"0x300,OK\r\n"));
        assert!(matches_rover_down_packet(b"0x4A2,TEMP=21.10;MODE=A\r\n"));
        assert!(!matches_rover_down_packet(b"400,21.10\r\n"));
        assert!(!matches_rover_down_packet(b"0x500,21.10\r\n"));
        assert!(!matches_rover_down_packet(b"0x400,\r\n"));
    }

    #[test]
    fn mixed_decoder_accepts_variable_length_rover_down_lines() {
        let mut decoder = MixedFormatDecoder::new(vec![OutputFormat::RoverDownGeneral]);

        assert!(decoder.push(b"noise0x4A2,TEMP=21.10").is_empty());
        let decoded = decoder.push(b"\r\n0x300,OK\r\n");

        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].format, OutputFormat::RoverDownGeneral);
        assert_eq!(decoded[0].bytes, b"0x4A2,TEMP=21.10\r\n");
        assert_eq!(decoded[1].bytes, b"0x300,OK\r\n");
    }

    #[test]
    fn parse_output_format_list_returns_none_for_display_modes() {
        assert_eq!(parse_output_format_list("utf8+line"), None);
        assert_eq!(parse_output_format_list("hex"), None);
    }

    #[test]
    fn resolve_display_mode_prefers_explicit_then_configured_then_builtin() {
        assert_eq!(
            resolve_display_mode(
                Some(PortDisplayMode::Utf8),
                Some(PortDisplayMode::Hex),
                Some(OutputFormat::PacketAcV6.default_display_mode()),
            ),
            PortDisplayMode::Utf8
        );
        assert_eq!(
            resolve_display_mode(
                None,
                Some(PortDisplayMode::HexAscii),
                Some(OutputFormat::RoverUpGeneral.default_display_mode()),
            ),
            PortDisplayMode::HexAscii
        );
        assert_eq!(
            resolve_display_mode(
                None,
                None,
                Some(OutputFormat::RoverDownGeneral.default_display_mode()),
            ),
            PortDisplayMode::Ascii
        );
    }
}
