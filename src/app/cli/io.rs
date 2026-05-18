use super::common::{
    default_baud_rate, default_log_dir, next_value, parse_key_value_args, parse_port_spec,
    parse_u32_arg,
};
use super::help::{is_help_flag, print_io_help};
use super::signal;
use crate::ingress::IngressFrame;
use crate::output::OutputFormat;
use crate::output::formats::{
    DummyPayloadGenerator, PacketUfV2Packet, crc16_ccitt_false, decode_packet_ufv2,
};
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

const DEFAULT_IO_SEND_RATE_HZ: u32 = 10;
pub(crate) const IO_SEND_RATE_CHOICES: &[u32] = &[10, 50, 100, 20, 1];
const IO_SEND_RATE_PROMPT_CHOICES: &[&str] = &["10", "50", "100", "20", "1", "2s", "3s", "5s"];
const SEND_LOOP_INTERVAL: Duration = Duration::from_millis(1);
const STATUS_INTERVAL: Duration = Duration::from_millis(200);
const RATE_WINDOW: Duration = Duration::from_secs(1);
const DISPLAY_FLUSH_SLICE: usize = 32;
const DISPLAY_FLUSH_PACKET_BUDGET: usize = 256;
const DISPLAY_QUEUE_LIMIT: usize = 65_536;
const SERIAL_FRAME_BITS_PER_BYTE: u64 = 10;
const UF_V2_REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(5);
const ROVER_DOWN_GENERAL_PREFIX_LEN: usize = 6;
const ROVER_DOWN_GENERAL_MAX_PACKET_LEN: usize = 256;

#[derive(Debug, Default)]
struct IoRuntimeOptions {
    outputs: Vec<IoOutputBinding>,
    baud: Option<u32>,
    rate_hz: Option<IoSendRate>,
    format: Option<String>,
    display: PortDisplayConfig,
    inputs: Vec<IoInputBinding>,
    log_dir: Option<PathBuf>,
    no_log: bool,
    s3b: bool,
}

#[derive(Debug, Clone)]
struct IoOutputBinding {
    id: String,
    port: String,
    baud: Option<u32>,
    rate_hz: Option<IoSendRate>,
    format: Option<String>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
}

#[derive(Debug, Clone)]
struct IoInputBinding {
    port: String,
    baud: Option<u32>,
    formats: Vec<String>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
    line_break_mode: Option<crate::port_display::LineBreakMode>,
}

#[derive(Debug, Clone)]
struct IoOutputSettings {
    session: SessionOutputSpec,
    format: OutputFormat,
    rate_hz: IoSendRate,
}

#[derive(Debug, Clone)]
struct IoObservedInputSpec {
    input_id: String,
    port: String,
    formats: Vec<OutputFormat>,
    per_format_display_modes: Option<BTreeMap<OutputFormat, PortDisplayMode>>,
    preserve_line_breaks: bool,
}

struct IoRuntimeSettings {
    inputs: Vec<SessionInputSpec>,
    outputs: Vec<IoOutputSettings>,
    observed_inputs: Vec<IoObservedInputSpec>,
    log_dir: PathBuf,
    logging_enabled: bool,
    s3b: bool,
    executed_command: Option<String>,
}

struct IoOutputRunResult {
    id: String,
    port: String,
    baud_rate: u32,
    format: OutputFormat,
    rate_hz: IoSendRate,
    sent_count: u64,
    payload_len: usize,
}

struct IoRunResult {
    outputs: Vec<IoOutputRunResult>,
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
    rate_hz: IoSendRate,
}

struct OutputSchedule {
    next_send_at: Instant,
    period: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct IoSendRate {
    hz: f64,
}

impl IoSendRate {
    fn hz(hz: f64) -> Self {
        Self { hz }
    }

    fn parse(context: &str, value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(format!("{context} must not be empty"));
        }

        let lower = trimmed.to_ascii_lowercase();
        let (number_text, unit) = parse_rate_unit(&lower);
        let number = number_text
            .parse::<f64>()
            .map_err(|_| format!("invalid {context}: {value}"))?;
        if !number.is_finite() || number <= 0.0 {
            return Err(format!("{context} must be greater than 0"));
        }

        let hz = match unit {
            RateUnit::Hz => number,
            RateUnit::Seconds => 1.0 / number,
        };
        Ok(Self { hz })
    }

    fn period(self) -> Duration {
        Duration::from_secs_f64(1.0 / self.hz)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RateUnit {
    Hz,
    Seconds,
}

fn parse_rate_unit(value: &str) -> (&str, RateUnit) {
    let trimmed = value.trim();
    for suffix in ["seconds", "second", "secs", "sec", "s"] {
        if let Some(number) = trimmed.strip_suffix(suffix) {
            return (number.trim(), RateUnit::Seconds);
        }
    }
    for suffix in ["hz", "h"] {
        if let Some(number) = trimmed.strip_suffix(suffix) {
            return (number.trim(), RateUnit::Hz);
        }
    }
    (trimmed, RateUnit::Hz)
}

fn looks_like_rate_value(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return true;
    }

    let lower = trimmed.to_ascii_lowercase();
    let (number_text, unit) = parse_rate_unit(&lower);
    unit != RateUnit::Hz
        && number_text
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_digit() || byte == b'.')
}

fn default_io_send_rate() -> IoSendRate {
    IoSendRate::hz(DEFAULT_IO_SEND_RATE_HZ as f64)
}

#[derive(Debug, Clone)]
struct IoHeaderOutput {
    id: String,
    port: String,
    baud_rate: u32,
    format: OutputFormat,
    rate_hz: IoSendRate,
    payload_len: Option<usize>,
}

struct IoRuntimeState {
    logging_enabled: bool,
    log_path_display: String,
    header_outputs: Vec<IoHeaderOutput>,
    observed_inputs: BTreeMap<String, ObservedInput>,
    last_status_update: Instant,
    header_lines: Vec<String>,
}

impl IoRuntimeState {
    fn new(
        settings: &IoRuntimeSettings,
        output_specs: &[IoOutputSettings],
        payload_lengths: &BTreeMap<String, usize>,
        log_path_display: String,
        started_at: Instant,
    ) -> Self {
        let header_outputs = output_specs
            .iter()
            .map(|output| IoHeaderOutput {
                id: output.session.id.clone(),
                port: output.session.port.clone(),
                baud_rate: output.session.baud_rate,
                format: output.format,
                rate_hz: output.rate_hz,
                payload_len: payload_lengths.get(&output.session.id).copied(),
            })
            .collect::<Vec<_>>();
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
                        spec.preserve_line_breaks,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();

        Self {
            logging_enabled: settings.logging_enabled,
            log_path_display,
            header_outputs,
            observed_inputs,
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
        let now = Instant::now();
        for input in self.observed_inputs.values_mut() {
            input.expire_pending_transfers(now);
        }
        self.flush_display_queues(session)?;

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
            lines.push(format!(
                "output[{}]: {} @ {} baud, format={}, tx_target={}, rx={rx_rate_hz:.1} Hz, payload={} bytes",
                output.id,
                output.port,
                output.baud_rate,
                output.format.as_str(),
                format_rate_label(output.rate_hz),
                output.payload_len.unwrap_or_default()
            ));
        }

        if self.logging_enabled {
            lines.push(format!("log: {}", self.log_path_display));
        } else {
            lines.push(String::from("log: disabled (--no-log)"));
        }
        lines.push(String::from("Space で表示を一時停止/再開  Ctrl-C で終了"));
        lines
    }

    fn rx_rate_hz(&mut self, port: &str, format: OutputFormat, now: Instant) -> Option<f64> {
        self.observed_inputs
            .values_mut()
            .find(|input| input.port == port)
            .and_then(|input| input.packet_rate_hz(format, now))
    }
}

pub(crate) struct ObservedInput {
    pub(crate) input_id: String,
    pub(crate) port: String,
    formats: Vec<OutputFormat>,
    decoder: MixedFormatDecoder,
    display_queue: PacketDisplayQueue,
    uf_v2_reassembler: UfV2TextReassembler,
    rate_trackers: BTreeMap<OutputFormat, PacketRateTracker>,
    per_format_display_modes: Option<BTreeMap<OutputFormat, PortDisplayMode>>,
    preserve_line_breaks: bool,
}

impl ObservedInput {
    pub(crate) fn new(
        input_id: String,
        port: String,
        formats: Vec<OutputFormat>,
        per_format_display_modes: Option<BTreeMap<OutputFormat, PortDisplayMode>>,
        preserve_line_breaks: bool,
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
            uf_v2_reassembler: UfV2TextReassembler::new(),
            rate_trackers,
            per_format_display_modes,
            preserve_line_breaks,
        }
    }

    pub(crate) fn observe(&mut self, bytes: &[u8], at: Instant) -> ObservedInputBatch {
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
            let entries = self.display_entries_for_packet(&packet, at);
            for entry in entries {
                self.display_queue.enqueue_entry(entry);
            }
        }

        ObservedInputBatch {
            valid_byte_len,
            valid_packet_count,
            per_format_totals,
        }
    }

    pub(crate) fn flush_display_batch(
        &mut self,
        session: &mut SessionRuntime,
        max_packets: usize,
    ) -> Result<usize, String> {
        self.display_queue
            .flush_input_batch(session, &self.port, max_packets)
    }

    pub(crate) fn expire_pending_transfers(&mut self, now: Instant) {
        if let Some(entry) = self.uf_v2_reassembler.expire(now) {
            self.display_queue.enqueue_entry(entry);
        }
    }

    pub(crate) fn packet_rate_hz(&mut self, format: OutputFormat, now: Instant) -> Option<f64> {
        self.rate_trackers
            .get_mut(&format)
            .map(|tracker| tracker.packets_per_second(now))
    }

    pub(crate) fn known_formats(&self) -> &[OutputFormat] {
        &self.formats
    }

    fn display_entries_for_packet(
        &mut self,
        packet: &DecodedPacket,
        at: Instant,
    ) -> Vec<DisplayedPacket> {
        if packet.format == OutputFormat::PacketUfV2 {
            return self.uf_v2_reassembler.observe(&packet.bytes, at);
        }

        match packet.format.decoded_display_payload(&packet.bytes) {
            Ok(Some(display_payload)) => vec![DisplayedPacket::new(
                display_payload,
                Some(PortDisplayMode::Ascii),
                true,
            )],
            Err(error) => vec![DisplayedPacket::new(
                format!("{} decode error: {error}", packet.format.display_name()).into_bytes(),
                Some(PortDisplayMode::Ascii),
                true,
            )],
            Ok(None) => {
                let display_mode = if packet.format == OutputFormat::RoverUpGeneral {
                    Some(PortDisplayMode::Ascii)
                } else {
                    self.per_format_display_modes
                        .as_ref()
                        .and_then(|display_modes| display_modes.get(&packet.format).copied())
                };
                vec![DisplayedPacket::new(
                    packet.bytes.clone(),
                    display_mode,
                    self.preserve_line_breaks,
                )]
            }
        }
    }
}

pub(crate) struct ObservedInputBatch {
    pub(crate) valid_byte_len: usize,
    pub(crate) valid_packet_count: usize,
    pub(crate) per_format_totals: BTreeMap<OutputFormat, (usize, usize)>,
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

    fn enqueue_entry(&mut self, packet: DisplayedPacket) {
        self.pending_packets.push_back(packet);
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

impl DisplayedPacket {
    fn new(
        bytes: Vec<u8>,
        display_mode: Option<PortDisplayMode>,
        preserve_line_breaks: bool,
    ) -> Self {
        Self {
            bytes,
            display_mode,
            preserve_line_breaks,
        }
    }
}

struct UfV2TextReassembler {
    buffer: Vec<u8>,
    expected_chunk_index: u8,
    active: bool,
    last_valid_at: Option<Instant>,
}

impl UfV2TextReassembler {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            expected_chunk_index: 0,
            active: false,
            last_valid_at: None,
        }
    }

    fn observe(&mut self, bytes: &[u8], at: Instant) -> Vec<DisplayedPacket> {
        let mut entries = Vec::new();
        if let Some(entry) = self.expire(at) {
            entries.push(entry);
        }

        let packet = match decode_packet_ufv2(bytes) {
            Ok(packet) => packet,
            Err(error) => {
                entries.push(uf_v2_status_entry(format!("UFv2 decode error: {error}")));
                return entries;
            }
        };

        if packet.read_error() {
            self.reset();
            entries.push(uf_v2_status_entry(format_uf_v2_packet_status(
                &packet, "error",
            )));
            return entries;
        }

        if packet.read_busy() {
            entries.push(uf_v2_status_entry(format_uf_v2_packet_status(
                &packet, "busy",
            )));
            return entries;
        }

        if !packet.valid() {
            entries.push(uf_v2_status_entry(format_uf_v2_packet_status(
                &packet, "idle",
            )));
            return entries;
        }

        if packet.chunk_index != self.expected_chunk_index {
            let expected = self.expected_chunk_index;
            let buffered_len = self.buffer.len();
            self.reset();
            entries.push(uf_v2_status_entry(format!(
                "UFv2 seq={} status=incomplete expected_chunk={} got_chunk={} discarded={} bytes",
                packet.seq, expected, packet.chunk_index, buffered_len
            )));
            return entries;
        }

        if !self.active {
            self.active = true;
        }
        self.buffer.extend_from_slice(packet.payload_bytes());
        self.last_valid_at = Some(at);

        if packet.end() {
            let text = std::mem::take(&mut self.buffer);
            self.reset();
            entries.push(DisplayedPacket::new(
                text,
                Some(PortDisplayMode::Utf8),
                true,
            ));
        } else {
            self.expected_chunk_index = self.expected_chunk_index.wrapping_add(1);
        }

        entries
    }

    fn expire(&mut self, now: Instant) -> Option<DisplayedPacket> {
        if !self.active {
            return None;
        }

        let last_valid_at = self.last_valid_at?;
        if now.saturating_duration_since(last_valid_at) < UF_V2_REASSEMBLY_TIMEOUT {
            return None;
        }

        let expected = self.expected_chunk_index;
        let buffered_len = self.buffer.len();
        self.reset();
        Some(uf_v2_status_entry(format!(
            "UFv2 status=timeout expected_chunk={} discarded={} bytes",
            expected, buffered_len
        )))
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.expected_chunk_index = 0;
        self.active = false;
        self.last_valid_at = None;
    }
}

fn uf_v2_status_entry(message: String) -> DisplayedPacket {
    DisplayedPacket::new(message.into_bytes(), Some(PortDisplayMode::Ascii), true)
}

fn format_uf_v2_packet_status(packet: &PacketUfV2Packet, status: &str) -> String {
    format!(
        "UFv2 seq={} status={} flags=0x{:02X}(valid={}, usb_present={}, read_busy={}, read_error={}, end={}, reserved=0x{:X}) chunk={} len={}",
        packet.seq,
        status,
        packet.flags,
        flag_value(packet.valid()),
        flag_value(packet.usb_present()),
        flag_value(packet.read_busy()),
        flag_value(packet.read_error()),
        flag_value(packet.end()),
        packet.reserved_flags(),
        packet.chunk_index,
        packet.payload_len,
    )
}

fn flag_value(enabled: bool) -> u8 {
    u8::from(enabled)
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
    PacketAcV6Usb,
    PacketMv1,
    PacketIv1,
    PacketBv1,
    PacketGcV1,
    PacketJfV1,
    PacketUfV2,
    RoverUpGeneral,
    RoverDownGeneral,
}

impl PacketMatcher {
    fn new(format: OutputFormat) -> Self {
        match format {
            OutputFormat::PacketAcV6 => Self::PacketAcV6,
            OutputFormat::PacketAcV6Usb => Self::PacketAcV6Usb,
            OutputFormat::PacketMv1 => Self::PacketMv1,
            OutputFormat::PacketIv1 => Self::PacketIv1,
            OutputFormat::PacketBv1 => Self::PacketBv1,
            OutputFormat::PacketGcV1 => Self::PacketGcV1,
            OutputFormat::PacketJfV1 => Self::PacketJfV1,
            OutputFormat::PacketUfV2 => Self::PacketUfV2,
            OutputFormat::RoverUpGeneral => Self::RoverUpGeneral,
            OutputFormat::RoverDownGeneral => Self::RoverDownGeneral,
        }
    }

    fn format(self) -> OutputFormat {
        match self {
            Self::PacketAcV6 => OutputFormat::PacketAcV6,
            Self::PacketAcV6Usb => OutputFormat::PacketAcV6Usb,
            Self::PacketMv1 => OutputFormat::PacketMv1,
            Self::PacketIv1 => OutputFormat::PacketIv1,
            Self::PacketBv1 => OutputFormat::PacketBv1,
            Self::PacketGcV1 => OutputFormat::PacketGcV1,
            Self::PacketJfV1 => OutputFormat::PacketJfV1,
            Self::PacketUfV2 => OutputFormat::PacketUfV2,
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
            Self::PacketAcV6 | Self::PacketAcV6Usb => matches_crc_packet(bytes, b"AC", 37),
            Self::PacketMv1 => matches_reduced_ac_packet(bytes, b'M', 19),
            Self::PacketIv1 => matches_reduced_ac_packet(bytes, b'I', 19),
            Self::PacketBv1 => matches_reduced_ac_packet(bytes, b'B', 15),
            Self::PacketGcV1 => matches_crc_packet(bytes, b"GC", 7),
            Self::PacketJfV1 => matches_crc_packet(bytes, b"JF", 14),
            Self::PacketUfV2 => matches_crc_packet(bytes, b"UF", 38),
            Self::RoverUpGeneral => matches_rover_up_packet(bytes),
            Self::RoverDownGeneral => matches_rover_down_packet(bytes),
        }
    }

    fn could_match_prefix(self, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }

        match self {
            Self::PacketAcV6 | Self::PacketAcV6Usb => {
                could_match_crc_packet_prefix(bytes, b"AC", 39)
            }
            Self::PacketMv1 => could_match_reduced_ac_packet_prefix(bytes, b'M', 19),
            Self::PacketIv1 => could_match_reduced_ac_packet_prefix(bytes, b'I', 19),
            Self::PacketBv1 => could_match_reduced_ac_packet_prefix(bytes, b'B', 15),
            Self::PacketGcV1 => could_match_crc_packet_prefix(bytes, b"GC", 9),
            Self::PacketJfV1 => could_match_crc_packet_prefix(bytes, b"JF", 16),
            Self::PacketUfV2 => could_match_crc_packet_prefix(bytes, b"UF", 40),
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
    if bytes.len() != packet_len || bytes.first().copied() != Some(header) {
        return false;
    }

    let payload_len = packet_len - 2;
    let expected_crc = u16::from_le_bytes([bytes[payload_len], bytes[payload_len + 1]]);
    crc16_ccitt_false(&bytes[..payload_len]) == expected_crc
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

    let runtime_options = match resolve_io_options(io_options) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    match run_with_options(runtime_options) {
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

#[derive(Debug, Default)]
struct IoCliOptions {
    inputs: Vec<IoBindingArg>,
    outputs: Vec<IoBindingArg>,
    runtime_options: IoRuntimeOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IoBindingArg {
    Provided(String),
    Prompt,
}

#[derive(Debug, Clone, Copy)]
struct IoPromptPortPosition {
    kind: &'static str,
    index: usize,
    total: usize,
}

impl IoPromptPortPosition {
    fn input(index: usize, total: usize) -> Self {
        Self {
            kind: "受信",
            index,
            total,
        }
    }

    fn output(index: usize, total: usize) -> Self {
        Self {
            kind: "送信",
            index,
            total,
        }
    }

    fn label(self, prompt: &str) -> String {
        if self.total > 1 {
            format!(
                "{prompt} ({}ポート {}/{})",
                self.kind, self.index, self.total
            )
        } else {
            prompt.to_owned()
        }
    }
}

#[derive(Debug, Clone)]
struct IoPromptCommand {
    inputs: Vec<String>,
    outputs: Vec<String>,
    log_dir: Option<PathBuf>,
    no_log: bool,
    s3b: bool,
}

#[derive(Debug, Clone, Copy)]
enum IoPromptBindingPreview<'a> {
    Input(&'a str),
    Output(&'a str),
}

impl IoPromptCommand {
    fn new(options: &IoRuntimeOptions) -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            log_dir: options.log_dir.clone(),
            no_log: options.no_log,
            s3b: options.s3b,
        }
    }

    fn push_input(&mut self, binding: String) {
        self.inputs.push(binding);
    }

    fn push_output(&mut self, binding: String) {
        self.outputs.push(binding);
    }

    fn render(&self, candidate: Option<IoPromptBindingPreview<'_>>) -> String {
        let mut args = vec![String::from("acs"), String::from("io")];

        for input in &self.inputs {
            args.push(String::from("-i"));
            args.push(input.clone());
        }
        if let Some(IoPromptBindingPreview::Input(input)) = candidate {
            args.push(String::from("-i"));
            args.push(input.to_owned());
        }

        for output in &self.outputs {
            args.push(String::from("-o"));
            args.push(output.clone());
        }
        if let Some(IoPromptBindingPreview::Output(output)) = candidate {
            args.push(String::from("-o"));
            args.push(output.to_owned());
        }

        append_prompt_common_args(&mut args, self.log_dir.as_ref(), self.no_log, self.s3b);
        format_command_preview(&args)
    }
}

fn print_io_result(result: &IoRunResult) {
    if result.outputs.is_empty() {
        println!("io session finished (receive only)");
    } else if result.outputs.len() == 1 {
        let output = &result.outputs[0];
        println!(
            "sent {} packets ({} bytes each) to {} @ {} baud, format={}, target={}",
            output.sent_count,
            output.payload_len,
            output.port,
            output.baud_rate,
            output.format.as_str(),
            format_rate_label(output.rate_hz)
        );
    } else {
        println!("sent dummy packets to {} outputs", result.outputs.len());
        for output in &result.outputs {
            println!(
                "  {}: {} packets ({} bytes each) to {} @ {} baud, format={}, target={}",
                output.id,
                output.sent_count,
                output.payload_len,
                output.port,
                output.baud_rate,
                output.format.as_str(),
                format_rate_label(output.rate_hz)
            );
        }
    }
    if result.logging_enabled {
        println!("log saved to {}", result.log_path.display());
    }
    if let Some(command) = &result.executed_command {
        println!("Command:");
        println!("{command}");
    }
}

fn parse_io_args(args: Vec<String>) -> Result<IoCliOptions, String> {
    let mut options = IoCliOptions {
        inputs: Vec::new(),
        outputs: Vec::new(),
        runtime_options: IoRuntimeOptions {
            rate_hz: Some(default_io_send_rate()),
            ..IoRuntimeOptions::default()
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
            "--config" => apply_io_config_args(
                &mut options.runtime_options,
                &next_value(&mut iter, "--config")?,
            )?,
            "--baud" | "-b" => {
                let value = next_value(&mut iter, "--baud")?;
                options.runtime_options.baud = Some(parse_u32_arg("--baud", &value)?);
            }
            "--rate" | "-r" => {
                let value = next_value(&mut iter, "--rate")?;
                options.runtime_options.rate_hz = Some(IoSendRate::parse("--rate", &value)?);
            }
            "--format" | "-f" => {
                options.runtime_options.format = Some(next_value(&mut iter, "--format")?)
            }
            "--display" => {
                let value = next_value(&mut iter, "--display")?;
                let assignment = parse_display_assignment(&value)?;
                assignment.apply_to(&mut options.runtime_options.display);
            }
            "--log-dir" => {
                options.runtime_options.log_dir =
                    Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            "--no-log" => options.runtime_options.no_log = true,
            "--s3b" => options.runtime_options.s3b = true,
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

fn resolve_io_options(io_options: IoCliOptions) -> Result<IoRuntimeOptions, String> {
    let total_inputs = io_options.inputs.len();
    let total_outputs = io_options.outputs.len();
    let mut input_index = 0;
    let mut output_index = 0;
    let mut runtime_options = io_options.runtime_options;
    let mut prompt_command = IoPromptCommand::new(&runtime_options);

    for input in io_options.inputs {
        input_index += 1;
        let position = IoPromptPortPosition::input(input_index, total_inputs);
        let value = match input {
            IoBindingArg::Provided(value) => value,
            IoBindingArg::Prompt => prompt_io_input_binding(&prompt_command, position)?,
        };
        runtime_options.inputs.push(parse_io_input_binding(&value)?);
        prompt_command.push_input(value);
    }

    for output in io_options.outputs {
        output_index += 1;
        let position = IoPromptPortPosition::output(output_index, total_outputs);
        let value = match output {
            IoBindingArg::Provided(value) => value,
            IoBindingArg::Prompt => prompt_io_output_binding(&prompt_command, position)?,
        };
        runtime_options
            .outputs
            .push(parse_io_output_binding(&value)?);
        prompt_command.push_output(value);
    }

    Ok(runtime_options)
}

fn prompt_io_input_binding(
    command: &IoPromptCommand,
    position: IoPromptPortPosition,
) -> Result<String, String> {
    let port_prompt = position.label("受信ポートを選択");
    let port = prompt_serial_port_with_preview(&port_prompt, |port| {
        Some(command.render(Some(IoPromptBindingPreview::Input(
            &format_prompt_io_input_binding(port, None, None, None),
        ))))
    })?;
    let baud_prompt = position.label("受信ボーレート");
    let baud = prompt_u32_choice_with_preview(
        &baud_prompt,
        default_baud_rate(),
        &[115_200, 921_600, 460_800, 230_400, 57_600, 38_400, 9_600],
        |baud| {
            Some(command.render(Some(IoPromptBindingPreview::Input(
                &format_prompt_io_input_binding(&port, Some(baud), None, None),
            ))))
        },
    )?;
    let baud_text = baud.to_string();
    let format_prompt = position.label("受信フォーマット");
    let format = prompt_input_format_with_title_and_preview(&format_prompt, |format| {
        Some(command.render(Some(IoPromptBindingPreview::Input(
            &format_prompt_io_input_binding(&port, Some(&baud_text), None, format),
        ))))
    })?;
    let display_prompt = position.label("受信表示形式");
    let display = prompt_display_mode_with_preview(&display_prompt, true, |display| {
        Some(command.render(Some(IoPromptBindingPreview::Input(
            &format_prompt_io_input_binding(
                &port,
                Some(&baud_text),
                Some(display),
                format.as_deref(),
            ),
        ))))
    })?;

    Ok(format_io_input_binding(
        &port,
        baud,
        display.as_deref(),
        format.as_deref(),
    ))
}

fn prompt_io_output_binding(
    command: &IoPromptCommand,
    position: IoPromptPortPosition,
) -> Result<String, String> {
    let port_prompt = position.label("送信ポートを選択");
    let port = prompt_serial_port_with_preview(&port_prompt, |port| {
        Some(command.render(Some(IoPromptBindingPreview::Output(
            &format_prompt_io_output_binding(port, None, None, None, None),
        ))))
    })?;
    let baud_prompt = position.label("送信ボーレート");
    let baud = prompt_u32_choice_with_preview(
        &baud_prompt,
        default_baud_rate(),
        &[115_200, 921_600, 460_800, 230_400, 57_600, 38_400, 9_600],
        |baud| {
            Some(command.render(Some(IoPromptBindingPreview::Output(
                &format_prompt_io_output_binding(&port, Some(baud), None, None, None),
            ))))
        },
    )?;
    let baud_text = baud.to_string();
    let display_prompt = position.label("送信表示形式");
    let display = prompt_display_mode_with_preview(&display_prompt, false, |display| {
        Some(command.render(Some(IoPromptBindingPreview::Output(
            &format_prompt_io_output_binding(&port, Some(&baud_text), Some(display), None, None),
        ))))
    })?;
    let format_prompt = position.label("送信フォーマット");
    let format = prompt_output_format_with_title_and_preview(&format_prompt, |format| {
        Some(command.render(Some(IoPromptBindingPreview::Output(
            &format_prompt_io_output_binding(
                &port,
                Some(&baud_text),
                display.as_deref(),
                Some(format),
                None,
            ),
        ))))
    })?;
    let rate_prompt = position.label("送信レート (Hz)");
    let rate_hz = prompt_io_send_rate_with_preview(&rate_prompt, |rate| {
        let rate_value = format_rate_value(rate);
        Some(command.render(Some(IoPromptBindingPreview::Output(
            &format_prompt_io_output_binding(
                &port,
                Some(&baud_text),
                display.as_deref(),
                Some(&format),
                Some(&rate_value),
            ),
        ))))
    })?;

    Ok(format_io_output_binding(
        &port,
        baud,
        display.as_deref(),
        &format,
        rate_hz,
    ))
}

fn prompt_io_send_rate_with_preview<F>(prompt: &str, preview: F) -> Result<IoSendRate, String>
where
    F: Fn(IoSendRate) -> Option<String>,
{
    let choices = IO_SEND_RATE_PROMPT_CHOICES
        .iter()
        .map(|value| IoSendRate::parse(prompt, value).map(|rate| (value.to_string(), rate)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut labels = choices
        .iter()
        .map(|(_, rate)| format_rate_label(*rate))
        .collect::<Vec<_>>();
    labels.push(String::from("手入力..."));

    let selected = choose_from_menu_with_preview(prompt, &labels, 0, |index| {
        if index == choices.len() {
            preview(default_io_send_rate())
        } else {
            preview(choices[index].1)
        }
    })?;
    if selected == choices.len() {
        let value = prompt_text(
            &format!("{prompt} (例: 10, 0.5, 2s): "),
            Some(&format_rate_value(default_io_send_rate())),
        )?;
        IoSendRate::parse(prompt, &value)
    } else {
        Ok(choices[selected].1)
    }
}

pub(crate) fn format_io_input_binding(
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

fn format_prompt_io_input_binding(
    port: &str,
    baud: Option<&str>,
    display: Option<&str>,
    format: Option<&str>,
) -> String {
    let mut value = port.to_owned();
    if let Some(baud) = baud {
        value.push('@');
        value.push_str(baud);
    }
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
    rate_hz: IoSendRate,
) -> String {
    let mut value = format!("{port}@{baud}");
    if let Some(display) = display {
        value.push(',');
        value.push_str(display);
    }
    value.push(',');
    value.push_str(format);
    value.push(',');
    value.push_str(&format_rate_value(rate_hz));
    value
}

fn format_prompt_io_output_binding(
    port: &str,
    baud: Option<&str>,
    display: Option<&str>,
    format: Option<&str>,
    rate_hz: Option<&str>,
) -> String {
    let mut value = format_prompt_io_input_binding(port, baud, display, format);
    if let Some(rate_hz) = rate_hz {
        value.push(',');
        value.push_str(rate_hz);
    }
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

pub(crate) fn format_input_display_value(
    display_mode: PortDisplayMode,
    line_break_mode: LineBreakMode,
) -> String {
    format!(
        "{}+{}",
        display_mode_value(display_mode),
        line_break_mode_value(line_break_mode)
    )
}

pub(crate) fn display_mode_value(mode: PortDisplayMode) -> &'static str {
    match mode {
        PortDisplayMode::Hex => "hex",
        PortDisplayMode::Ascii => "ascii",
        PortDisplayMode::Utf8 => "utf8",
        PortDisplayMode::HexAscii => "hex+ascii",
        PortDisplayMode::HexUtf8 => "hex+utf8",
    }
}

pub(crate) fn line_break_mode_value(mode: LineBreakMode) -> &'static str {
    match mode {
        LineBreakMode::Line => "line",
        LineBreakMode::Packet => "packet",
        LineBreakMode::Wrap => "wrap",
        LineBreakMode::Crlf => "crlf",
    }
}

fn format_rate_value(rate: IoSendRate) -> String {
    if let Some(integer_hz) = integer_if_close(rate.hz)
        && integer_hz >= 1
    {
        return integer_hz.to_string();
    }

    let period_secs = 1.0 / rate.hz;
    if let Some(integer_period_secs) = integer_if_close(period_secs)
        && integer_period_secs >= 1
    {
        return format!("{integer_period_secs}s");
    }

    format_decimal(rate.hz)
}

fn format_rate_label(rate: IoSendRate) -> String {
    if let Some(integer_hz) = integer_if_close(rate.hz)
        && integer_hz >= 1
    {
        return format!("{integer_hz} Hz");
    }

    let period_secs = 1.0 / rate.hz;
    if let Some(integer_period_secs) = integer_if_close(period_secs)
        && integer_period_secs >= 1
    {
        return format!(
            "every {integer_period_secs}s ({} Hz)",
            format_decimal(rate.hz)
        );
    }

    format!("{} Hz", format_decimal(rate.hz))
}

fn integer_if_close(value: f64) -> Option<u64> {
    let rounded = value.round();
    ((value - rounded).abs() < 0.000_000_001).then_some(rounded as u64)
}

fn format_decimal(value: f64) -> String {
    let mut text = format!("{value:.6}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

pub(crate) fn shell_quote_arg(value: &str) -> String {
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

pub(crate) fn format_command_preview(args: &[String]) -> String {
    args.iter()
        .map(|arg| shell_quote_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn append_prompt_common_args(
    args: &mut Vec<String>,
    log_dir: Option<&PathBuf>,
    no_log: bool,
    s3b: bool,
) {
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
}

pub(crate) fn prompt_serial_port_with_preview<F>(prompt: &str, preview: F) -> Result<String, String>
where
    F: Fn(&str) -> Option<String>,
{
    let ports = serial::available_ports().map_err(|error| error.to_string())?;
    if ports.is_empty() {
        if let Some(command) = preview("PORT") {
            println!("現在のコマンド:");
            println!("  {command}");
        }
        return prompt_text(&format!("{prompt}: "), None);
    }

    let mut labels = ports
        .iter()
        .enumerate()
        .map(|(index, port)| format_serial_port_choice(index, port))
        .collect::<Vec<_>>();
    labels.push(format!("[{}] 手入力...", ports.len()));
    let selected = choose_from_menu_with_preview(prompt, &labels, 0, |index| {
        if index == ports.len() {
            preview("PORT")
        } else {
            preview(&ports[index].port_name)
        }
    })?;
    if selected == ports.len() {
        prompt_text("ポート名: ", None)
    } else {
        Ok(ports[selected].port_name.clone())
    }
}

fn format_serial_port_choice(index: usize, port: &serialport::SerialPortInfo) -> String {
    match &port.port_type {
        serialport::SerialPortType::UsbPort(info) => format!(
            "[{index}] {}  usb vid=0x{:04x} pid=0x{:04x} product={}",
            port.port_name,
            info.vid,
            info.pid,
            info.product.as_deref().unwrap_or("unknown")
        ),
        serialport::SerialPortType::BluetoothPort => {
            format!("[{index}] {}  bluetooth", port.port_name)
        }
        serialport::SerialPortType::PciPort => format!("[{index}] {}  pci", port.port_name),
        serialport::SerialPortType::Unknown => format!("[{index}] {}", port.port_name),
    }
}

pub(crate) fn prompt_u32_choice_with_preview<F>(
    prompt: &str,
    default: u32,
    choices: &[u32],
    preview: F,
) -> Result<u32, String>
where
    F: Fn(&str) -> Option<String>,
{
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

    let selected = choose_from_menu_with_preview(prompt, &labels, 0, |index| {
        if index == values.len() {
            preview("VALUE")
        } else {
            preview(&values[index].to_string())
        }
    })?;
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

fn prompt_output_format_with_title_and_preview<F>(
    prompt: &str,
    preview: F,
) -> Result<String, String>
where
    F: Fn(&str) -> Option<String>,
{
    let formats = output_format_choices();
    let labels = formats
        .iter()
        .map(|format| format.display_name().to_owned())
        .collect::<Vec<_>>();
    let selected = choose_from_menu_with_preview(prompt, &labels, 0, |index| {
        preview(formats[index].as_str())
    })?;
    Ok(formats[selected].as_str().to_owned())
}

pub(crate) fn prompt_input_format_with_preview<F>(preview: F) -> Result<Option<String>, String>
where
    F: Fn(Option<&str>) -> Option<String>,
{
    prompt_input_format_with_title_and_preview("受信フォーマット", preview)
}

fn prompt_input_format_with_title_and_preview<F>(
    prompt: &str,
    preview: F,
) -> Result<Option<String>, String>
where
    F: Fn(Option<&str>) -> Option<String>,
{
    let formats = output_format_choices();
    let mut labels = vec![String::from("raw (フォーマット指定なし)")];
    labels.extend(
        formats
            .iter()
            .map(|format| format.display_name().to_owned()),
    );
    labels.push(String::from("複数/手入力..."));

    let selected = choose_from_menu_with_preview(prompt, &labels, 0, |index| {
        if index == 0 {
            preview(None)
        } else if index == formats.len() + 1 {
            preview(Some("FORMAT"))
        } else {
            preview(Some(formats[index - 1].as_str()))
        }
    })?;
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

pub(crate) fn prompt_display_mode_with_preview<F>(
    prompt: &str,
    input: bool,
    preview: F,
) -> Result<Option<String>, String>
where
    F: Fn(&str) -> Option<String>,
{
    let mut choices = vec![
        String::from("hex"),
        String::from("ascii"),
        String::from("utf8"),
        String::from("hex+ascii"),
        String::from("hex+utf8"),
    ];
    if input {
        choices.extend([
            String::from("ascii+crlf"),
            String::from("utf8+crlf"),
            String::from("hex+packet"),
            String::from("utf8+packet"),
            String::from("hex+utf8+wrap"),
        ]);
    }
    choices.push(String::from("手入力..."));

    let selected = choose_from_menu_with_preview(prompt, &choices, 0, |index| {
        if index == choices.len() - 1 {
            preview("DISPLAY")
        } else {
            preview(&choices[index])
        }
    })?;
    if selected == choices.len() - 1 {
        let value = prompt_text(&format!("{prompt}: "), Some("hex"))?;
        parse_display_value(&value)?;
        Ok(Some(value))
    } else {
        Ok(Some(choices[selected].clone()))
    }
}

pub(crate) fn output_format_choices() -> Vec<OutputFormat> {
    vec![
        OutputFormat::PacketAcV6,
        OutputFormat::PacketAcV6Usb,
        OutputFormat::PacketMv1,
        OutputFormat::PacketIv1,
        OutputFormat::PacketBv1,
        OutputFormat::PacketGcV1,
        OutputFormat::PacketJfV1,
        OutputFormat::PacketUfV2,
        OutputFormat::RoverUpGeneral,
        OutputFormat::RoverDownGeneral,
    ]
}

pub(crate) fn choose_from_menu_with_preview<F>(
    prompt: &str,
    labels: &[String],
    default_index: usize,
    preview: F,
) -> Result<usize, String>
where
    F: Fn(usize) -> Option<String>,
{
    if labels.is_empty() {
        return Err(format!("{prompt}: no choices available"));
    }

    let default_index = default_index.min(labels.len() - 1);
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return choose_from_numbered_prompt(prompt, labels, default_index, &preview);
    }

    choose_from_menu_interactive(prompt, labels, default_index, &preview)
}

#[cfg(unix)]
fn choose_from_menu_interactive(
    prompt: &str,
    labels: &[String],
    default_index: usize,
    preview: &dyn Fn(usize) -> Option<String>,
) -> Result<usize, String> {
    let _screen = AlternateScreen::enter()?;
    let _raw_mode = RawTerminalMode::enable()?;
    let mut selected = default_index;
    let mut stdin = io::stdin();

    loop {
        let command_preview = preview(selected);
        render_menu(prompt, labels, selected, command_preview.as_deref())?;
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
    preview: &dyn Fn(usize) -> Option<String>,
) -> Result<usize, String> {
    choose_from_numbered_prompt(prompt, labels, default_index, preview)
}

fn render_menu(
    prompt: &str,
    labels: &[String],
    selected: usize,
    command_preview: Option<&str>,
) -> Result<(), String> {
    const HEADER_BG: &str = "\x1b[47m\x1b[30m";
    const SELECTED_BG: &str = "\x1b[48;5;218m\x1b[30m";
    const CLEAR_LINE_END: &str = "\x1b[K";
    const RESET: &str = "\x1b[0m";

    clear_screen()?;
    println!("{HEADER_BG} {prompt}{CLEAR_LINE_END}{RESET}");
    println!("↑/↓ で選択、Enter で決定");
    if let Some(command_preview) = command_preview {
        println!();
        println!("現在のコマンド:");
        println!("  {command_preview}");
        println!();
    }
    for (index, label) in labels.iter().enumerate() {
        if index == selected {
            println!("{SELECTED_BG}  {label}{CLEAR_LINE_END}{RESET}");
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

struct AlternateScreen;

impl AlternateScreen {
    fn enter() -> Result<Self, String> {
        print!("\x1b[?1049h\x1b[H");
        io::stdout()
            .flush()
            .map_err(|error| format!("failed to enter alternate screen: {error}"))?;
        Ok(Self)
    }
}

impl Drop for AlternateScreen {
    fn drop(&mut self) {
        print!("\x1b[?1049l");
        let _ = io::stdout().flush();
    }
}

fn choose_from_numbered_prompt(
    prompt: &str,
    labels: &[String],
    default_index: usize,
    preview: &dyn Fn(usize) -> Option<String>,
) -> Result<usize, String> {
    println!("{prompt}");
    for (index, label) in labels.iter().enumerate() {
        println!("  {}. {}", index + 1, label);
    }
    if let Some(command_preview) = preview(default_index) {
        println!("現在のコマンド:");
        println!("  {command_preview}");
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

fn run_with_options(cli_options: IoRuntimeOptions) -> Result<IoRunResult, String> {
    let settings = build_settings(cli_options)?;
    let output_specs = settings.outputs.clone();

    let mut payload_lengths = BTreeMap::new();
    let mut generators = BTreeMap::<String, Box<dyn DummyPayloadGenerator>>::new();
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

    let started_at = Instant::now();
    let mut session = SessionRuntime::new(SessionSpec {
        title: settings
            .executed_command
            .clone()
            .unwrap_or_else(|| String::from("acs io")),
        command_name: String::from("io"),
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
    let runtime_state = RefCell::new(IoRuntimeState::new(
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
    let mut schedules = output_specs
        .iter()
        .map(|output| {
            (
                output.session.id.clone(),
                OutputSchedule {
                    next_send_at: started_at,
                    period: output.rate_hz.period(),
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
                    .ok_or_else(|| format!("missing io schedule for output `{output_id}`"))?;
                if now >= schedule.next_send_at {
                    realign_output_schedule(schedule, now);
                    let payload = generators
                        .get_mut(output_id)
                        .ok_or_else(|| format!("missing dummy generator for output `{output_id}`"))?
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

    Ok(IoRunResult {
        outputs: output_specs
            .into_iter()
            .map(|output| IoOutputRunResult {
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

fn build_settings(cli_options: IoRuntimeOptions) -> Result<IoRuntimeSettings, String> {
    let default_baud = cli_options.baud.unwrap_or_else(default_baud_rate);
    let default_rate_hz = cli_options.rate_hz.unwrap_or_else(default_io_send_rate);
    let default_format_name = cli_options
        .format
        .clone()
        .unwrap_or_else(|| String::from("packetacv6"));
    let default_format = OutputFormat::parse(&default_format_name)?;
    let output_bindings =
        resolve_output_bindings(&cli_options, default_baud, default_rate_hz, default_format)?;
    let input_port_specs = resolve_input_bindings(&cli_options)?;
    if output_bindings.is_empty() && input_port_specs.is_empty() {
        return Err(String::from(
            "at least one input or output port is required",
        ));
    }
    let display = cli_options.display;
    let explicit_log_dir = cli_options.log_dir.clone();
    let log_dir = cli_options.log_dir.unwrap_or_else(default_log_dir);

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
            return Err(format!("duplicate io output id: {}", binding.id));
        }

        let port = serial::resolve_port(Some(&binding.port)).map_err(|error| error.to_string())?;
        let baud_rate = binding.baud.unwrap_or(default_baud);
        if let Some(existing_baud_rate) = output_port_bauds.get(&port) {
            if *existing_baud_rate != baud_rate {
                return Err(format!(
                    "io output port `{port}` cannot use multiple baud rates ({existing_baud_rate} and {baud_rate})"
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
        outputs.push(IoOutputSettings {
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
        io_command_outputs.push(IoCommandOutput {
            id: binding.id.clone(),
            port: port.clone(),
            baud_rate,
            display_mode: output_display_mode,
            format,
            rate_hz: binding.rate_hz,
        });
    }

    validate_output_port_loads(&outputs)?;

    for port_spec in input_port_specs {
        let input_port =
            serial::resolve_port(Some(&port_spec.port)).map_err(|error| error.to_string())?;
        let input_formats = port_spec.formats;
        let input_has_formats = !input_formats.is_empty();
        let baud_rate = port_spec.baud.unwrap_or(default_baud);
        let display_mode = resolve_display_mode(
            port_spec.display_mode,
            display.resolve_input_override(&input_port),
            input_formats
                .first()
                .copied()
                .map(OutputFormat::default_display_mode),
        );
        let line_break_mode = resolve_input_line_break_mode(
            port_spec.line_break_mode,
            input_has_formats,
            display.resolve_line_break_input_override(&input_port),
        );
        io_command_inputs.push(IoCommandInput {
            port: input_port.clone(),
            baud_rate,
            display_mode,
            line_break_mode,
            formats: input_formats.clone(),
        });
        if !inputs
            .iter()
            .any(|input: &SessionInputSpec| input.port == input_port)
        {
            inputs.push(SessionInputSpec {
                id: input_port.clone(),
                port: input_port.clone(),
                baud_rate,
                display_mode,
                line_break_mode,
            });
        }
        if !input_formats.is_empty() {
            let input_display_override = port_spec
                .display_mode
                .or(display.resolve_input_override(&input_port));
            input_display_is_explicit
                .entry(input_port.clone())
                .and_modify(|explicit| *explicit |= input_display_override.is_some())
                .or_insert(input_display_override.is_some());
            let formats = input_packet_format_candidates
                .entry(input_port)
                .or_default();
            for format in input_formats {
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
            let preserve_line_breaks = inputs
                .iter()
                .find(|input| input.port == port)
                .map(|input| input.line_break_mode.preserve_entry_line_breaks())
                .unwrap_or(false);
            Some(IoObservedInputSpec {
                input_id,
                port,
                formats,
                per_format_display_modes,
                preserve_line_breaks,
            })
        })
        .collect();
    let executed_command = Some(build_io_executed_command(
        &io_command_inputs,
        &io_command_outputs,
        explicit_log_dir.as_ref(),
        cli_options.no_log,
        cli_options.s3b,
    ));

    Ok(IoRuntimeSettings {
        inputs,
        outputs,
        observed_inputs,
        log_dir,
        logging_enabled: !cli_options.no_log,
        s3b: cli_options.s3b,
        executed_command,
    })
}

pub(crate) fn resolve_display_mode(
    explicit_mode: Option<PortDisplayMode>,
    configured_mode: Option<PortDisplayMode>,
    built_in_mode: Option<PortDisplayMode>,
) -> PortDisplayMode {
    explicit_mode
        .or(configured_mode)
        .or(built_in_mode)
        .unwrap_or_default()
}

pub(crate) fn resolve_input_line_break_mode(
    explicit_mode: Option<LineBreakMode>,
    has_formats: bool,
    configured_mode: Option<LineBreakMode>,
) -> LineBreakMode {
    explicit_mode.or(configured_mode).unwrap_or(if has_formats {
        LineBreakMode::Line
    } else {
        LineBreakMode::Wrap
    })
}

fn validate_output_port_loads(outputs: &[IoOutputSettings]) -> Result<(), String> {
    let mut loads = BTreeMap::<(String, u32), Vec<(String, IoSendRate, u64)>>::new();

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
                .map(|(format_name, rate_hz, bps)| {
                    format!("{format_name} {}={bps}bps", format_rate_label(rate_hz))
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "io output load on `{port}` @ {baud_rate} baud exceeds serial capacity: estimated {total_bps} bps assuming 10 bits/byte ({detail}) > {baud_rate} baud. Reduce rates or increase baud."
            ));
        }
    }

    Ok(())
}

fn estimated_output_line_bps(format: OutputFormat, rate_hz: IoSendRate) -> u64 {
    (format.packet_len() as f64 * SERIAL_FRAME_BITS_PER_BYTE as f64 * rate_hz.hz).ceil() as u64
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
    cli_options: &IoRuntimeOptions,
    default_baud: u32,
    default_rate_hz: IoSendRate,
    default_format: OutputFormat,
) -> Result<Vec<ResolvedIoOutputBinding>, String> {
    cli_options
        .outputs
        .iter()
        .cloned()
        .map(|binding| {
            resolve_io_output_binding(binding, default_baud, default_rate_hz, default_format)
        })
        .collect()
}

#[derive(Debug, Clone)]
struct ResolvedIoOutputBinding {
    id: String,
    port: String,
    baud: Option<u32>,
    rate_hz: IoSendRate,
    format: Option<OutputFormat>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
}

fn resolve_io_output_binding(
    binding: IoOutputBinding,
    _default_baud: u32,
    default_rate_hz: IoSendRate,
    default_format: OutputFormat,
) -> Result<ResolvedIoOutputBinding, String> {
    let rate_hz = binding.rate_hz.unwrap_or(default_rate_hz);

    Ok(ResolvedIoOutputBinding {
        id: binding.id,
        port: binding.port,
        baud: binding.baud,
        rate_hz,
        format: Some(match binding.format {
            Some(format) => OutputFormat::parse(&format)?,
            None => default_format,
        }),
        display_mode: binding.display_mode,
    })
}

#[derive(Debug, Clone)]
struct ResolvedIoInputBinding {
    port: String,
    baud: Option<u32>,
    formats: Vec<OutputFormat>,
    display_mode: Option<crate::port_display::PortDisplayMode>,
    line_break_mode: Option<crate::port_display::LineBreakMode>,
}

fn resolve_input_bindings(
    cli_options: &IoRuntimeOptions,
) -> Result<Vec<ResolvedIoInputBinding>, String> {
    cli_options
        .inputs
        .iter()
        .cloned()
        .map(resolve_io_input_binding)
        .collect()
}

fn resolve_io_input_binding(binding: IoInputBinding) -> Result<ResolvedIoInputBinding, String> {
    Ok(ResolvedIoInputBinding {
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

fn apply_io_config_args(options: &mut IoRuntimeOptions, value: &str) -> Result<(), String> {
    for assignment in parse_key_value_args("--config", value)? {
        let key = assignment.key.to_ascii_uppercase();
        match key.as_str() {
            "RATE" => options.rate_hz = Some(IoSendRate::parse("RATE", &assignment.value)?),
            "FORMAT" => options.format = Some(assignment.value),
            "DISPLAY" => {
                let display = parse_display_assignment(&assignment.value)?;
                display.apply_to(&mut options.display);
            }
            "LOG_DIR" => options.log_dir = Some(PathBuf::from(assignment.value)),
            other => return Err(format!("unknown io config key: {other}")),
        }
    }

    Ok(())
}

fn parse_io_input_binding(value: &str) -> Result<IoInputBinding, String> {
    if value.is_empty() {
        return Err(String::from("io input binding must not be empty"));
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

    let port_spec = parse_port_spec("io input binding", port_text)?;
    Ok(IoInputBinding {
        port: port_spec.port,
        baud: port_spec.baud,
        formats,
        display_mode: port_spec.display_mode,
        line_break_mode: port_spec.line_break_mode,
    })
}

pub(crate) fn parse_output_format_list(value: &str) -> Option<Vec<String>> {
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

fn parse_io_output_binding(value: &str) -> Result<IoOutputBinding, String> {
    if value.is_empty() {
        return Err(String::from("io output binding must not be empty"));
    }

    let mut binding_text = value;
    let mut rate_hz = None;
    if let Some((candidate_binding, candidate_rate)) = binding_text.rsplit_once(',') {
        match IoSendRate::parse("io output rate", candidate_rate) {
            Ok(rate) => {
                rate_hz = Some(rate);
                binding_text = candidate_binding;
            }
            Err(error) if looks_like_rate_value(candidate_rate) => return Err(error),
            Err(_) => {}
        }
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
            return Err(format!("invalid io output binding: {value}"));
        }
        (Some(id.to_owned()), port_text)
    } else {
        (None, binding_text)
    };

    let port_spec = parse_port_spec("io output binding", port_text)?;
    Ok(IoOutputBinding {
        id: id.unwrap_or_else(|| port_spec.port.clone()),
        port: port_spec.port,
        baud: port_spec.baud,
        rate_hz,
        format,
        display_mode: port_spec.display_mode,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        IoCommandInput, IoCommandOutput, IoOutputSettings, IoPromptBindingPreview, IoPromptCommand,
        IoPromptPortPosition, IoRuntimeOptions, IoSendRate, MixedFormatDecoder, ObservedInput,
        OutputSchedule, build_io_executed_command, crc16_ccitt_false, estimated_output_line_bps,
        format_io_input_binding, format_io_output_binding, matches_reduced_ac_packet,
        matches_rover_down_packet, parse_io_args, parse_io_input_binding, parse_io_output_binding,
        parse_output_format_list, realign_output_schedule, resolve_display_mode,
        resolve_input_line_break_mode, validate_output_port_loads,
    };
    use crate::output::OutputFormat;
    use crate::port_display::{LineBreakMode, PortDisplayMode};
    use crate::session::runtime::SessionOutputSpec;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn build_uf_v2_packet(seq: u8, flags: u8, chunk_index: u8, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() <= 32);
        let mut packet = vec![0u8; 40];
        packet[0..2].copy_from_slice(b"UF");
        packet[2] = seq;
        packet[3] = flags;
        packet[4] = chunk_index;
        packet[5] = payload.len() as u8;
        packet[6..6 + payload.len()].copy_from_slice(payload);
        let crc = crc16_ccitt_false(&packet[..38]).to_le_bytes();
        packet[38] = crc[0];
        packet[39] = crc[1];
        packet
    }

    #[test]
    fn parse_io_input_binding_accepts_format_and_packet_mode() {
        let binding = parse_io_input_binding("/dev/ttyUSB1@115200,utf8+line,packetjfv1").unwrap();

        assert_eq!(binding.port, "/dev/ttyUSB1");
        assert_eq!(binding.baud, Some(115_200));
        assert_eq!(binding.formats, vec![String::from("packetjfv1")]);
        assert_eq!(binding.display_mode, Some(PortDisplayMode::Utf8));
        assert_eq!(binding.line_break_mode, Some(LineBreakMode::Line));
    }

    #[test]
    fn parse_io_input_binding_accepts_ascii_crlf_mode() {
        let binding =
            parse_io_input_binding("/dev/ttyUSB1@115200,ascii+crlf,roverdowngeneral").unwrap();

        assert_eq!(binding.port, "/dev/ttyUSB1");
        assert_eq!(binding.baud, Some(115_200));
        assert_eq!(binding.formats, vec![String::from("roverdowngeneral")]);
        assert_eq!(binding.display_mode, Some(PortDisplayMode::Ascii));
        assert_eq!(binding.line_break_mode, Some(LineBreakMode::Crlf));
    }

    #[test]
    fn parse_io_input_binding_accepts_packetufv2_format() {
        let binding = parse_io_input_binding("/dev/ttyUSB1@921600,hex+packet,PacketUFv2").unwrap();

        assert_eq!(binding.port, "/dev/ttyUSB1");
        assert_eq!(binding.baud, Some(921_600));
        assert_eq!(binding.formats, vec![String::from("packetufv2")]);
        assert_eq!(binding.display_mode, Some(PortDisplayMode::Hex));
        assert_eq!(binding.line_break_mode, Some(LineBreakMode::Packet));
    }

    #[test]
    fn parse_io_input_binding_accepts_multiple_formats() {
        let binding =
            parse_io_input_binding("/dev/ttyUSB1,packetacv6+packetmv1+packetjfv1").unwrap();

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
    fn input_without_format_defaults_to_wrap_mode() {
        assert_eq!(
            resolve_input_line_break_mode(None, false, None),
            LineBreakMode::Wrap
        );
    }

    #[test]
    fn input_with_format_uses_configured_line_break_mode() {
        assert_eq!(
            resolve_input_line_break_mode(None, true, Some(LineBreakMode::Packet)),
            LineBreakMode::Packet
        );
        assert_eq!(
            resolve_input_line_break_mode(
                Some(LineBreakMode::Line),
                false,
                Some(LineBreakMode::Packet)
            ),
            LineBreakMode::Line
        );
    }

    #[test]
    fn configured_input_line_break_mode_applies_without_format() {
        assert_eq!(
            resolve_input_line_break_mode(None, false, Some(LineBreakMode::Crlf)),
            LineBreakMode::Crlf
        );
    }

    #[test]
    fn parse_io_output_binding_accepts_id_format_and_packet_mode() {
        let binding =
            parse_io_output_binding("main=/dev/ttyUSB0@921600,hex+packet,packetacv6,100").unwrap();

        assert_eq!(binding.id, "main");
        assert_eq!(binding.port, "/dev/ttyUSB0");
        assert_eq!(binding.baud, Some(921_600));
        assert_eq!(binding.rate_hz, Some(IoSendRate::hz(100.0)));
        assert_eq!(binding.format.as_deref(), Some("packetacv6"));
        assert_eq!(binding.display_mode, Some(PortDisplayMode::Hex));
    }

    #[test]
    fn parse_io_output_binding_accepts_period_rate() {
        let binding =
            parse_io_output_binding("main=/dev/ttyUSB0@921600,hex,packetacv6,2s").unwrap();

        assert_eq!(binding.id, "main");
        assert_eq!(binding.format.as_deref(), Some("packetacv6"));
        assert_eq!(binding.rate_hz, Some(IoSendRate::hz(0.5)));
    }

    #[test]
    fn parse_io_output_binding_accepts_fractional_hz_rate() {
        let binding =
            parse_io_output_binding("main=/dev/ttyUSB0@921600,hex,packetacv6,0.333").unwrap();

        assert_eq!(binding.id, "main");
        assert_eq!(binding.format.as_deref(), Some("packetacv6"));
        assert_eq!(binding.rate_hz, Some(IoSendRate::hz(0.333)));
    }

    #[test]
    fn parse_io_args_accepts_period_rate_option() {
        let options = parse_io_args(vec![
            String::from("-o"),
            String::from("main=/dev/ttyUSB0@921600,hex,packetacv6"),
            String::from("--rate"),
            String::from("3s"),
        ])
        .expect("should parse");

        assert_eq!(options.outputs.len(), 1);
        assert_eq!(
            options.runtime_options.rate_hz,
            Some(IoSendRate::hz(1.0 / 3.0))
        );
    }

    #[test]
    fn format_rate_value_preserves_integer_periods() {
        assert_eq!(super::format_rate_value(IoSendRate::hz(10.0)), "10");
        assert_eq!(super::format_rate_value(IoSendRate::hz(0.5)), "2s");
        assert_eq!(
            super::format_rate_label(IoSendRate::hz(0.5)),
            "every 2s (0.5 Hz)"
        );
    }

    #[test]
    fn parse_io_output_binding_rejects_zero_rate() {
        let error = parse_io_output_binding("main=/dev/ttyUSB0@921600,hex,packetacv6,0")
            .expect_err("should reject");

        assert_eq!(error, "io output rate must be greater than 0");
    }

    #[test]
    fn parse_io_output_binding_accepts_packetacv6usb_format() {
        let binding =
            parse_io_output_binding("main=/dev/ttyUSB0@921600,hex,PacketACv6USB,10").unwrap();

        assert_eq!(binding.id, "main");
        assert_eq!(binding.format.as_deref(), Some("PacketACv6USB"));
        assert_eq!(binding.rate_hz, Some(IoSendRate::hz(10.0)));
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
        assert_eq!(options.runtime_options.rate_hz, Some(IoSendRate::hz(10.0)));
        assert!(options.runtime_options.no_log);
    }

    #[test]
    fn parse_io_args_accepts_bare_input_output_for_prompting() {
        let options =
            parse_io_args(vec![String::from("-i"), String::from("-o")]).expect("should parse");

        assert_eq!(options.inputs.len(), 1);
        assert_eq!(options.outputs.len(), 1);
    }

    #[test]
    fn io_binding_format_matches_io_parsers() {
        let input = format_io_input_binding(
            "/dev/ttyUSB1",
            115_200,
            Some("utf8+packet"),
            Some("packetacv6+packetjfv1"),
        );
        let output = format_io_output_binding(
            "/dev/ttyUSB0",
            921_600,
            Some("hex"),
            "packetacv6",
            IoSendRate::hz(10.0),
        );

        let input = parse_io_input_binding(&input).expect("input binding should parse");
        let output = parse_io_output_binding(&output).expect("output binding should parse");

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
        assert_eq!(output.rate_hz, Some(IoSendRate::hz(10.0)));
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
                rate_hz: IoSendRate::hz(10.0),
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
    fn io_prompt_command_renders_current_candidate() {
        let options = IoRuntimeOptions {
            log_dir: Some(PathBuf::from("tmp/io logs")),
            no_log: true,
            s3b: true,
            ..IoRuntimeOptions::default()
        };
        let mut command = IoPromptCommand::new(&options);
        command.push_input(String::from("/dev/ttyUSB1@115200,utf8+packet,packetacv6"));

        assert_eq!(
            command.render(Some(IoPromptBindingPreview::Output(
                "/dev/ttyUSB0@921600,hex,packetmv1,50"
            ))),
            "acs io -i /dev/ttyUSB1@115200,utf8+packet,packetacv6 -o /dev/ttyUSB0@921600,hex,packetmv1,50 --log-dir 'tmp/io logs' --no-log --s3b"
        );
    }

    #[test]
    fn io_prompt_port_position_labels_only_multiple_ports() {
        assert_eq!(
            IoPromptPortPosition::input(1, 1).label("受信ポートを選択"),
            "受信ポートを選択"
        );
        assert_eq!(
            IoPromptPortPosition::input(2, 3).label("受信ボーレート"),
            "受信ボーレート (受信ポート 2/3)"
        );
        assert_eq!(
            IoPromptPortPosition::output(1, 2).label("送信ボーレート"),
            "送信ボーレート (送信ポート 1/2)"
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
    fn mixed_decoder_accepts_packetacv6usb_packets() {
        let packet = OutputFormat::PacketAcV6Usb
            .encode_dummy_payload()
            .expect("packetacv6usb dummy payload");
        let mut decoder = MixedFormatDecoder::new(vec![OutputFormat::PacketAcV6Usb]);

        let decoded = decoder.push(&packet);

        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].format, OutputFormat::PacketAcV6Usb);
        assert_eq!(decoded[0].bytes, packet);
    }

    #[test]
    fn mixed_decoder_accepts_packetufv2_packets() {
        let packet = OutputFormat::PacketUfV2
            .encode_dummy_payload()
            .expect("packetufv2 dummy payload");
        let mut decoder = MixedFormatDecoder::new(vec![OutputFormat::PacketUfV2]);

        assert!(decoder.push(&packet[..5]).is_empty());
        let decoded = decoder.push(&packet[5..]);

        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].format, OutputFormat::PacketUfV2);
        assert_eq!(decoded[0].bytes, packet);
    }

    #[test]
    fn mixed_decoder_excludes_legacy_uf_packets() {
        let packet = OutputFormat::PacketUfV2
            .encode_dummy_payload()
            .expect("packetufv2 dummy payload");
        let mut legacy = [0u8; 14];
        legacy[0..2].copy_from_slice(b"UF");
        legacy[2] = 1;
        legacy[3] = 3;
        legacy[4..8].copy_from_slice(&356_812_362i32.to_le_bytes());
        legacy[8..12].copy_from_slice(&1_397_671_248i32.to_le_bytes());
        let crc = crc16_ccitt_false(&legacy[..12]).to_le_bytes();
        legacy[12] = crc[0];
        legacy[13] = crc[1];
        let mut decoder = MixedFormatDecoder::new(vec![OutputFormat::PacketUfV2]);

        assert!(decoder.push(&legacy).is_empty());
        let decoded = decoder.push(&packet);

        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].format, OutputFormat::PacketUfV2);
        assert_eq!(decoded[0].bytes, packet);
    }

    #[test]
    fn observed_input_displays_packetufv2_complete_text_as_utf8() {
        let packet = OutputFormat::PacketUfV2
            .encode_dummy_payload()
            .expect("packetufv2 dummy payload");
        let mut observed = ObservedInput::new(
            String::from("input-uf"),
            String::from("/dev/ttyUSB1"),
            vec![OutputFormat::PacketUfV2],
            None,
            false,
        );

        let batch = observed.observe(&packet, Instant::now());

        assert_eq!(batch.valid_packet_count, 1);
        assert_eq!(batch.valid_byte_len, packet.len());
        assert_eq!(observed.display_queue.pending_packets.len(), 1);
        let queued = observed
            .display_queue
            .pending_packets
            .front()
            .expect("queued uf packet");
        assert_eq!(queued.display_mode, Some(PortDisplayMode::Utf8));
        assert!(queued.preserve_line_breaks);
        assert_eq!(
            String::from_utf8_lossy(&queued.bytes),
            "38.12345, -110.98765\n"
        );
    }

    #[test]
    fn observed_input_reassembles_packetufv2_chunks() {
        let first = build_uf_v2_packet(1, 0x03, 0, b"hello ");
        let second = build_uf_v2_packet(2, 0x13, 1, b"world\n");
        let mut observed = ObservedInput::new(
            String::from("input-uf"),
            String::from("/dev/ttyUSB1"),
            vec![OutputFormat::PacketUfV2],
            None,
            false,
        );
        let now = Instant::now();

        let first_batch = observed.observe(&first, now);
        let second_batch = observed.observe(&second, now + Duration::from_millis(1));

        assert_eq!(first_batch.valid_packet_count, 1);
        assert_eq!(second_batch.valid_packet_count, 1);
        assert_eq!(observed.display_queue.pending_packets.len(), 1);
        let queued = observed
            .display_queue
            .pending_packets
            .front()
            .expect("queued uf text");
        assert_eq!(queued.display_mode, Some(PortDisplayMode::Utf8));
        assert_eq!(String::from_utf8_lossy(&queued.bytes), "hello world\n");
    }

    #[test]
    fn observed_input_reports_packetufv2_chunk_gap() {
        let packet = build_uf_v2_packet(1, 0x13, 1, b"world\n");
        let mut observed = ObservedInput::new(
            String::from("input-uf"),
            String::from("/dev/ttyUSB1"),
            vec![OutputFormat::PacketUfV2],
            None,
            false,
        );

        let batch = observed.observe(&packet, Instant::now());

        assert_eq!(batch.valid_packet_count, 1);
        assert_eq!(observed.display_queue.pending_packets.len(), 1);
        let queued = observed
            .display_queue
            .pending_packets
            .front()
            .expect("queued uf status");
        assert_eq!(queued.display_mode, Some(PortDisplayMode::Ascii));
        assert_eq!(
            String::from_utf8_lossy(&queued.bytes),
            "UFv2 seq=1 status=incomplete expected_chunk=0 got_chunk=1 discarded=0 bytes"
        );
    }

    #[test]
    fn observed_input_expires_packetufv2_incomplete_text() {
        let packet = build_uf_v2_packet(1, 0x03, 0, b"hello ");
        let mut observed = ObservedInput::new(
            String::from("input-uf"),
            String::from("/dev/ttyUSB1"),
            vec![OutputFormat::PacketUfV2],
            None,
            false,
        );
        let now = Instant::now();

        observed.observe(&packet, now);
        observed.expire_pending_transfers(now + Duration::from_secs(6));

        assert_eq!(observed.display_queue.pending_packets.len(), 1);
        let queued = observed
            .display_queue
            .pending_packets
            .front()
            .expect("queued uf timeout");
        assert_eq!(queued.display_mode, Some(PortDisplayMode::Ascii));
        assert_eq!(
            String::from_utf8_lossy(&queued.bytes),
            "UFv2 status=timeout expected_chunk=1 discarded=6 bytes"
        );
    }

    #[test]
    fn reduced_ac_packet_matcher_checks_crc() {
        let mut packet = OutputFormat::PacketMv1
            .encode_dummy_payload()
            .expect("packetmv1 dummy payload");

        assert!(matches_reduced_ac_packet(
            &packet,
            b'M',
            OutputFormat::PacketMv1.packet_len()
        ));

        packet[2] ^= 0x01;
        assert!(!matches_reduced_ac_packet(
            &packet,
            b'M',
            OutputFormat::PacketMv1.packet_len()
        ));
    }

    #[test]
    fn observed_input_forces_rover_up_receive_display_to_ascii() {
        let packet = OutputFormat::RoverUpGeneral
            .encode_dummy_payload()
            .expect("roverupgeneral dummy payload");
        let mut observed = ObservedInput::new(
            String::from("input-up"),
            String::from("/dev/ttyUSB1"),
            vec![OutputFormat::RoverUpGeneral],
            None,
            false,
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
    fn observed_input_preserves_line_breaks_when_requested() {
        let packet = OutputFormat::RoverUpGeneral
            .encode_dummy_payload()
            .expect("roverupgeneral dummy payload");
        let mut observed = ObservedInput::new(
            String::from("input-up"),
            String::from("/dev/ttyUSB1"),
            vec![OutputFormat::RoverUpGeneral],
            None,
            true,
        );

        let batch = observed.observe(&packet, Instant::now());

        assert_eq!(batch.valid_packet_count, 1);
        let queued = observed
            .display_queue
            .pending_packets
            .front()
            .expect("queued roverup packet");
        assert!(queued.preserve_line_breaks);
        assert_eq!(queued.display_mode, Some(PortDisplayMode::Ascii));
    }

    #[test]
    fn estimated_output_line_bps_uses_packet_length_and_rate() {
        assert_eq!(
            estimated_output_line_bps(OutputFormat::PacketAcV6, IoSendRate::hz(100.0)),
            39_000
        );
        assert_eq!(
            estimated_output_line_bps(OutputFormat::PacketAcV6Usb, IoSendRate::hz(100.0)),
            39_000
        );
        assert_eq!(
            estimated_output_line_bps(OutputFormat::PacketUfV2, IoSendRate::hz(100.0)),
            40_000
        );
        assert_eq!(
            estimated_output_line_bps(OutputFormat::RoverUpGeneral, IoSendRate::hz(100.0)),
            12_000
        );
        assert_eq!(
            estimated_output_line_bps(OutputFormat::PacketAcV6, IoSendRate::hz(0.5)),
            195
        );
    }

    #[test]
    fn validate_output_port_loads_rejects_oversubscribed_shared_port() {
        let outputs = vec![
            IoOutputSettings {
                session: SessionOutputSpec {
                    id: String::from("arm"),
                    port: String::from("/dev/ttyUSB0"),
                    baud_rate: 115_200,
                    format_name: String::from("packetacv6"),
                    display_mode: PortDisplayMode::Hex,
                },
                format: OutputFormat::PacketAcV6,
                rate_hz: IoSendRate::hz(1_000.0),
            },
            IoOutputSettings {
                session: SessionOutputSpec {
                    id: String::from("rover"),
                    port: String::from("/dev/ttyUSB0"),
                    baud_rate: 115_200,
                    format_name: String::from("roverupgeneral"),
                    display_mode: PortDisplayMode::Ascii,
                },
                format: OutputFormat::RoverUpGeneral,
                rate_hz: IoSendRate::hz(1_000.0),
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
