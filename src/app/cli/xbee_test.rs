use super::common::{
    default_baud_rate, default_log_dir, next_value, parse_key_value_args, parse_port_spec,
    parse_u32_arg,
};
use super::help::{is_help_flag, print_xbee_test_help};
use super::signal;
use crate::ingress::IngressFrame;
use crate::output::OutputFormat;
use crate::output::formats::{DummyPayloadGenerator, crc16_ccitt_false};
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use crate::{port_display::LineBreakMode, port_display::PortDisplayMode, serial};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const BASE_PORT_ID: &str = "base";
pub(crate) const REMOTE_PORT_ID: &str = "remote";
pub(crate) const BASE_AU_OUTPUT_ID: &str = "base-au";
pub(crate) const BASE_RU_OUTPUT_ID: &str = "base-ru";
pub(crate) const BASE_POLL_OUTPUT_ID: &str = "base-poll";
pub(crate) const REMOTE_AD_OUTPUT_ID: &str = "remote-ad";
pub(crate) const REMOTE_RD_OUTPUT_ID: &str = "remote-rd";
pub(crate) const REMOTE_POLL_OUTPUT_ID: &str = "remote-poll";
pub(crate) const MODEL_BASE_AU_OUTPUT_ID: &str = "model-base-au";
pub(crate) const MODEL_BASE_RU_OUTPUT_ID: &str = "model-base-ru";
pub(crate) const MODEL_BASE_POLL_OUTPUT_ID: &str = "model-base-poll";
pub(crate) const MODEL_REMOTE_AD_OUTPUT_ID: &str = "model-remote-ad";
pub(crate) const MODEL_REMOTE_RD_OUTPUT_ID: &str = "model-remote-rd";
pub(crate) const MODEL_REMOTE_POLL_OUTPUT_ID: &str = "model-remote-poll";
pub(crate) const DEFAULT_RATE_HZ: u32 = 100;
const DEFAULT_REAL_PERCENT: u32 = 0;
pub(crate) const LOOP_INTERVAL: Duration = Duration::from_millis(1);
pub(crate) const STATUS_INTERVAL: Duration = Duration::from_millis(200);
const EXPECTED_QUEUE_LIMIT: usize = 8_192;
pub(crate) const DISPLAY_FLUSH_SLICE: usize = 16;
pub(crate) const DISPLAY_FLUSH_PACKET_BUDGET: usize = 256;
const DISPLAY_QUEUE_LIMIT: usize = 65_536;
pub(crate) const SPACE_HINT: &str = "Space で表示を一時停止/再開  Ctrl-C で終了";
const POLL_FRAME_PACKET_LEN: usize = 5;
const POLL_FRAME_PAYLOAD_LEN: usize = 3;
const POLL_GREETING_FORMAT_NAME: &str = "PollGreeting";
const POLL_RESPONSE_FORMAT_NAME: &str = "PollResponse";
const POLL_GREETING_HEADER: [u8; 2] = *b"HI";
const POLL_RESPONSE_HEADER: [u8; 2] = *b"OK";
const ROVER_DOWN_GENERAL_PREFIX_LEN: usize = 6;
const ROVER_DOWN_GENERAL_MAX_PACKET_LEN: usize = 256;

#[derive(Debug, Default)]
struct XbeeTestCliOptions {
    ports: Vec<XbeeTestPortBinding>,
    mode: Option<String>,
    poll_rate_hz: Option<u32>,
    base_real_percent: Option<u32>,
    remote_real_percent: Option<u32>,
    au_rate_hz: Option<u32>,
    ru_rate_hz: Option<u32>,
    ad_rate_hz: Option<u32>,
    rd_rate_hz: Option<u32>,
    log_dir: Option<PathBuf>,
    no_log: bool,
    s3b: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XbeeTestPortBinding {
    id: String,
    port: String,
    baud: Option<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedXbeeTestPort {
    pub(crate) port: String,
    pub(crate) baud_rate: u32,
}

#[derive(Debug, Clone)]
struct XbeeTestSettings {
    base_port: ResolvedXbeeTestPort,
    remote_port: ResolvedXbeeTestPort,
    mode: XbeeTestMode,
    poll_rate_hz: u32,
    base_real_percent: u32,
    remote_real_percent: u32,
    au_rate_hz: u32,
    ru_rate_hz: u32,
    ad_rate_hz: u32,
    rd_rate_hz: u32,
    log_dir: PathBuf,
    logging_enabled: bool,
    s3b: bool,
}

struct XbeeTestRunResult {
    base_port: ResolvedXbeeTestPort,
    remote_port: ResolvedXbeeTestPort,
    mode: XbeeTestMode,
    poll_rate_hz: u32,
    base_real_percent: u32,
    remote_real_percent: u32,
    greeting_stats: PacketMatchStats,
    response_stats: PacketMatchStats,
    au_rate_hz: u32,
    ru_rate_hz: u32,
    ad_rate_hz: u32,
    rd_rate_hz: u32,
    au_stats: PacketMatchStats,
    ru_stats: PacketMatchStats,
    ad_stats: PacketMatchStats,
    rd_stats: PacketMatchStats,
    logging_enabled: bool,
    log_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XbeeTestMode {
    Flood,
    PingPong,
    Polling,
}

impl XbeeTestMode {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "flood" => Ok(Self::Flood),
            "ping-pong" | "pingpong" => Ok(Self::PingPong),
            "polling" | "poll" => Ok(Self::Polling),
            other => Err(format!(
                "unsupported xbee-test mode: {other} (expected `flood`, `ping-pong`, or `polling`)"
            )),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Flood => "flood",
            Self::PingPong => "ping-pong",
            Self::Polling => "polling",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum XbeeTestFrameKind {
    Format(OutputFormat),
    PollGreeting,
    PollResponse,
}

impl XbeeTestFrameKind {
    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Format(format) => format.display_name(),
            Self::PollGreeting => POLL_GREETING_FORMAT_NAME,
            Self::PollResponse => POLL_RESPONSE_FORMAT_NAME,
        }
    }

    pub(crate) fn session_format_name(self) -> &'static str {
        match self {
            Self::Format(format) => format.as_str(),
            Self::PollGreeting => POLL_GREETING_FORMAT_NAME,
            Self::PollResponse => POLL_RESPONSE_FORMAT_NAME,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PacketDefinition {
    packet_len: usize,
    payload_len: usize,
    header: [u8; 2],
}

impl PacketDefinition {
    fn for_format(format: OutputFormat) -> Self {
        match format {
            OutputFormat::PacketAcV6 => Self {
                packet_len: 39,
                payload_len: 37,
                header: *b"AC",
            },
            OutputFormat::PacketGcV1 => Self {
                packet_len: 9,
                payload_len: 7,
                header: *b"GC",
            },
            OutputFormat::PacketJfV1 => Self {
                packet_len: 16,
                payload_len: 14,
                header: *b"JF",
            },
            OutputFormat::PacketMv1
            | OutputFormat::PacketIv1
            | OutputFormat::PacketBv1
            | OutputFormat::RoverUpGeneral
            | OutputFormat::RoverDownGeneral => {
                panic!(
                    "xbee-test does not support output format `{}`",
                    format.as_str()
                )
            }
        }
    }
}

pub(crate) fn xbee_test_format_label(kind: XbeeTestFrameKind) -> &'static str {
    match kind {
        XbeeTestFrameKind::Format(OutputFormat::PacketAcV6) => "AU(PacketACv6)",
        XbeeTestFrameKind::Format(OutputFormat::PacketMv1) => "M(PacketMv1)",
        XbeeTestFrameKind::Format(OutputFormat::PacketIv1) => "I(PacketIv1)",
        XbeeTestFrameKind::Format(OutputFormat::PacketBv1) => "B(PacketBv1)",
        XbeeTestFrameKind::Format(OutputFormat::PacketGcV1) => "GC(PacketGCv1)",
        XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral) => "RU(RoverUpGeneral)",
        XbeeTestFrameKind::Format(OutputFormat::PacketJfV1) => "AD(PacketJFv1)",
        XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral) => "RD(RoverDownGeneral)",
        XbeeTestFrameKind::PollGreeting => "PollGreeting",
        XbeeTestFrameKind::PollResponse => "PollResponse",
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct PacketMatchStats {
    pub(crate) matched_packets: u64,
    pub(crate) missing_packets: u64,
    pub(crate) invalid_packets: u64,
    pub(crate) unexpected_packets: u64,
    pub(crate) queue_overflow_packets: u64,
}

impl PacketMatchStats {
    fn total_errors(&self) -> u64 {
        self.missing_packets
            .saturating_add(self.invalid_packets)
            .saturating_add(self.unexpected_packets)
            .saturating_add(self.queue_overflow_packets)
    }

    pub(crate) fn error_rate_percent(&self) -> f64 {
        let total = self.matched_packets.saturating_add(self.total_errors());
        if total == 0 {
            return 0.0;
        }

        self.total_errors() as f64 * 100.0 / total as f64
    }
}

struct PacketStreamDecoder {
    kind: XbeeTestFrameKind,
    buffer: Vec<u8>,
}

impl PacketStreamDecoder {
    fn new(kind: XbeeTestFrameKind) -> Self {
        Self {
            kind,
            buffer: Vec::new(),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> DecodedPacketBatch {
        self.buffer.extend_from_slice(bytes);
        let mut packets = Vec::new();
        let mut invalid_packets = 0u64;

        loop {
            if self.buffer.len() < packet_start_len(self.kind) {
                break;
            }

            let Some(header_index) = find_packet_start(&self.buffer, self.kind) else {
                let keep_len = self
                    .buffer
                    .len()
                    .min(packet_start_len(self.kind).saturating_sub(1));
                let drain_len = self.buffer.len().saturating_sub(keep_len);
                if drain_len > 0 {
                    self.buffer.drain(..drain_len);
                }
                break;
            };

            if header_index > 0 {
                self.buffer.drain(..header_index);
            }

            if is_rover_down_kind(self.kind) {
                if let Some(packet_len) = rover_down_packet_len(&self.buffer) {
                    packets.push(self.buffer.drain(..packet_len).collect());
                    continue;
                }
                if matches_rover_down_prefix(&self.buffer) {
                    break;
                }
                self.buffer.drain(..1);
                invalid_packets = invalid_packets.saturating_add(1);
                continue;
            }

            if self.buffer.len() < packet_len(self.kind) {
                break;
            }

            if packet_matches(self.kind, &self.buffer[..packet_len(self.kind)]) {
                packets.push(self.buffer.drain(..packet_len(self.kind)).collect());
            } else {
                self.buffer.drain(..1);
                invalid_packets = invalid_packets.saturating_add(1);
            }
        }

        DecodedPacketBatch {
            packets,
            invalid_packets,
        }
    }
}

fn packet_len(kind: XbeeTestFrameKind) -> usize {
    match kind {
        XbeeTestFrameKind::Format(format) => format.packet_len(),
        XbeeTestFrameKind::PollGreeting | XbeeTestFrameKind::PollResponse => POLL_FRAME_PACKET_LEN,
    }
}

fn packet_start_len(kind: XbeeTestFrameKind) -> usize {
    match kind {
        XbeeTestFrameKind::Format(OutputFormat::PacketAcV6)
        | XbeeTestFrameKind::Format(OutputFormat::PacketGcV1)
        | XbeeTestFrameKind::Format(OutputFormat::PacketJfV1)
        | XbeeTestFrameKind::PollGreeting
        | XbeeTestFrameKind::PollResponse => 2,
        XbeeTestFrameKind::Format(OutputFormat::PacketMv1)
        | XbeeTestFrameKind::Format(OutputFormat::PacketIv1)
        | XbeeTestFrameKind::Format(OutputFormat::PacketBv1) => 1,
        XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral) => 6,
        XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral) => ROVER_DOWN_GENERAL_PREFIX_LEN,
    }
}

fn find_packet_start(buffer: &[u8], kind: XbeeTestFrameKind) -> Option<usize> {
    match kind {
        XbeeTestFrameKind::Format(OutputFormat::PacketAcV6) => find_header(buffer, b"AC"),
        XbeeTestFrameKind::Format(OutputFormat::PacketGcV1) => find_header(buffer, b"GC"),
        XbeeTestFrameKind::Format(OutputFormat::PacketMv1) => find_byte(buffer, b'M'),
        XbeeTestFrameKind::Format(OutputFormat::PacketIv1) => find_byte(buffer, b'I'),
        XbeeTestFrameKind::Format(OutputFormat::PacketBv1) => find_byte(buffer, b'B'),
        XbeeTestFrameKind::Format(OutputFormat::PacketJfV1) => find_header(buffer, b"JF"),
        XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral) => find_rover_up_start(buffer),
        XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral) => find_rover_down_start(buffer),
        XbeeTestFrameKind::PollGreeting => find_header(buffer, &POLL_GREETING_HEADER),
        XbeeTestFrameKind::PollResponse => find_header(buffer, &POLL_RESPONSE_HEADER),
    }
}

fn packet_matches(kind: XbeeTestFrameKind, packet: &[u8]) -> bool {
    match kind {
        XbeeTestFrameKind::Format(OutputFormat::PacketAcV6)
        | XbeeTestFrameKind::Format(OutputFormat::PacketGcV1)
        | XbeeTestFrameKind::Format(OutputFormat::PacketJfV1) => {
            let XbeeTestFrameKind::Format(format) = kind else {
                unreachable!()
            };
            packet_is_valid(packet, PacketDefinition::for_format(format))
        }
        XbeeTestFrameKind::Format(OutputFormat::PacketMv1) => {
            matches_reduced_ac_packet(packet, b'M', OutputFormat::PacketMv1.packet_len())
        }
        XbeeTestFrameKind::Format(OutputFormat::PacketIv1) => {
            matches_reduced_ac_packet(packet, b'I', OutputFormat::PacketIv1.packet_len())
        }
        XbeeTestFrameKind::Format(OutputFormat::PacketBv1) => {
            matches_reduced_ac_packet(packet, b'B', OutputFormat::PacketBv1.packet_len())
        }
        XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral) => matches_rover_up_packet(packet),
        XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral) => {
            matches_rover_down_packet(packet)
        }
        XbeeTestFrameKind::PollGreeting => matches_poll_frame(packet, &POLL_GREETING_HEADER),
        XbeeTestFrameKind::PollResponse => matches_poll_frame(packet, &POLL_RESPONSE_HEADER),
    }
}

fn find_rover_up_start(buffer: &[u8]) -> Option<usize> {
    buffer.windows(6).position(|window| {
        window[0] == b'0'
            && window[1] == b'x'
            && window[2] == b'3'
            && window[3].is_ascii_hexdigit()
            && window[4].is_ascii_hexdigit()
            && window[5] == b','
    })
}

fn find_rover_down_start(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(ROVER_DOWN_GENERAL_PREFIX_LEN)
        .position(|window| {
            window[0] == b'0'
                && window[1] == b'x'
                && matches!(window[2], b'3' | b'4')
                && window[3].is_ascii_hexdigit()
                && window[4].is_ascii_hexdigit()
                && window[5] == b','
        })
}

struct DecodedPacketBatch {
    packets: Vec<Vec<u8>>,
    invalid_packets: u64,
}

fn build_poll_frame(header: &[u8; 2], seq: u8) -> Vec<u8> {
    let mut packet = [0u8; POLL_FRAME_PACKET_LEN];
    packet[0] = header[0];
    packet[1] = header[1];
    packet[2] = seq;
    let crc = crc16_ccitt_false(&packet[..POLL_FRAME_PAYLOAD_LEN]).to_le_bytes();
    packet[3] = crc[0];
    packet[4] = crc[1];
    packet.to_vec()
}

fn matches_poll_frame(bytes: &[u8], header: &[u8; 2]) -> bool {
    if bytes.len() != POLL_FRAME_PACKET_LEN || &bytes[..2] != header {
        return false;
    }

    let expected_crc = crc16_ccitt_false(&bytes[..POLL_FRAME_PAYLOAD_LEN]);
    let actual_crc = u16::from_le_bytes([
        bytes[POLL_FRAME_PAYLOAD_LEN],
        bytes[POLL_FRAME_PAYLOAD_LEN + 1],
    ]);
    expected_crc == actual_crc
}

#[derive(Default)]
struct PollGreetingDummyGenerator {
    seq: u8,
}

impl DummyPayloadGenerator for PollGreetingDummyGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String> {
        let payload = build_poll_frame(&POLL_GREETING_HEADER, self.seq);
        self.seq = self.seq.wrapping_add(1);
        Ok(payload)
    }
}

#[derive(Default)]
struct PollResponseDummyGenerator {
    seq: u8,
}

impl DummyPayloadGenerator for PollResponseDummyGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String> {
        let payload = build_poll_frame(&POLL_RESPONSE_HEADER, self.seq);
        self.seq = self.seq.wrapping_add(1);
        Ok(payload)
    }
}

fn create_xbee_test_generator(
    kind: XbeeTestFrameKind,
) -> Result<Box<dyn DummyPayloadGenerator>, String> {
    match kind {
        XbeeTestFrameKind::Format(format) => format.create_dummy_generator(),
        XbeeTestFrameKind::PollGreeting => Ok(Box::new(PollGreetingDummyGenerator::default())),
        XbeeTestFrameKind::PollResponse => Ok(Box::new(PollResponseDummyGenerator::default())),
    }
}

struct ExpectedPacketTracker {
    expected_packets: VecDeque<Vec<u8>>,
    stats: PacketMatchStats,
}

impl ExpectedPacketTracker {
    fn new() -> Self {
        Self {
            expected_packets: VecDeque::new(),
            stats: PacketMatchStats::default(),
        }
    }

    fn expect(&mut self, packet: Vec<u8>) {
        self.expected_packets.push_back(packet);
        while self.expected_packets.len() > EXPECTED_QUEUE_LIMIT {
            self.expected_packets.pop_front();
            self.stats.queue_overflow_packets = self.stats.queue_overflow_packets.saturating_add(1);
        }
    }

    fn record_invalid_packets(&mut self, count: u64) {
        self.stats.invalid_packets = self.stats.invalid_packets.saturating_add(count);
    }

    fn observe(&mut self, packet: Vec<u8>) {
        if self.expected_packets.front() == Some(&packet) {
            self.expected_packets.pop_front();
            self.stats.matched_packets = self.stats.matched_packets.saturating_add(1);
            return;
        }

        if let Some(index) = self
            .expected_packets
            .iter()
            .position(|expected| *expected == packet)
        {
            for _ in 0..index {
                self.expected_packets.pop_front();
            }
            self.expected_packets.pop_front();
            self.stats.missing_packets = self.stats.missing_packets.saturating_add(index as u64);
            self.stats.matched_packets = self.stats.matched_packets.saturating_add(1);
            return;
        }

        self.stats.unexpected_packets = self.stats.unexpected_packets.saturating_add(1);
    }

    fn observe_untracked_valid_packet(&mut self) {
        self.stats.matched_packets = self.stats.matched_packets.saturating_add(1);
    }

    fn stats(&self) -> PacketMatchStats {
        self.stats.clone()
    }

    fn pending_packets(&self) -> usize {
        self.expected_packets.len()
    }
}

pub(crate) struct ObservedInput {
    input_id: &'static str,
    input_port: String,
    display_queue: PacketDisplayQueue,
    format_states: Vec<ObservedInputFormatState>,
}

struct ObservedInputFormatState {
    from_port_id: &'static str,
    kind: XbeeTestFrameKind,
    track_expected_packets: bool,
    decoder: PacketStreamDecoder,
    tracker: ExpectedPacketTracker,
    rate_samples: VecDeque<PacketRateSample>,
}

#[derive(Clone, Copy)]
pub(crate) struct ObservedInputKindSpec {
    kind: XbeeTestFrameKind,
    track_expected_packets: bool,
}

impl ObservedInputKindSpec {
    fn strict(kind: XbeeTestFrameKind) -> Self {
        Self {
            kind,
            track_expected_packets: true,
        }
    }

    pub(crate) fn new(kind: XbeeTestFrameKind, track_expected_packets: bool) -> Self {
        Self {
            kind,
            track_expected_packets,
        }
    }
}

impl ObservedInput {
    pub(crate) fn new(
        input_id: &'static str,
        input_port: String,
        from_port_id: &'static str,
        kinds: Vec<ObservedInputKindSpec>,
    ) -> Self {
        Self {
            input_id,
            input_port,
            display_queue: PacketDisplayQueue::new(),
            format_states: kinds
                .into_iter()
                .map(|kind| ObservedInputFormatState {
                    from_port_id,
                    kind: kind.kind,
                    track_expected_packets: kind.track_expected_packets,
                    decoder: PacketStreamDecoder::new(kind.kind),
                    tracker: ExpectedPacketTracker::new(),
                    rate_samples: VecDeque::new(),
                })
                .collect(),
        }
    }

    fn expect(&mut self, kind: XbeeTestFrameKind, packet: Vec<u8>) {
        if let Some(state) = self
            .format_states
            .iter_mut()
            .find(|state| state.kind == kind)
        {
            state.tracker.expect(packet);
        }
    }

    pub(crate) fn observe(&mut self, bytes: &[u8], now: Instant) -> ObservedInputBatch {
        let mut observed = ObservedInputBatch::default();

        for state in &mut self.format_states {
            let batch = state.decoder.push(bytes);
            state.tracker.record_invalid_packets(batch.invalid_packets);
            let mut valid_packet_count = 0usize;
            let mut valid_byte_len = 0usize;
            for packet in batch.packets {
                valid_packet_count += 1;
                valid_byte_len += packet.len();
                self.display_queue.enqueue(packet.clone());
                if state.track_expected_packets {
                    state.tracker.observe(packet);
                } else {
                    state.tracker.observe_untracked_valid_packet();
                }
            }
            if valid_packet_count > 0 {
                state.rate_samples.push_back(PacketRateSample {
                    at: now,
                    byte_len: valid_byte_len,
                    packet_count: valid_packet_count,
                });
                observed.valid_byte_len += valid_byte_len;
                observed.valid_packet_count += valid_packet_count;
                observed
                    .per_kind_totals
                    .insert(state.kind, (valid_byte_len, valid_packet_count));
            }
            state.prune_rate_samples(now);
        }

        observed
    }

    pub(crate) fn flush_display_batch(
        &mut self,
        session: &mut SessionRuntime,
        max_packets: usize,
    ) -> Result<usize, String> {
        self.display_queue
            .flush_input_batch(session, &self.input_port, max_packets)
    }

    pub(crate) fn status_lines(
        &self,
        target_rates_hz: &BTreeMap<XbeeTestFrameKind, f64>,
    ) -> Vec<String> {
        self.format_states
            .iter()
            .map(|state| state.status_line(self.input_id, target_rates_hz))
            .collect()
    }

    pub(crate) fn stats(&self, kind: XbeeTestFrameKind) -> PacketMatchStats {
        self.format_states
            .iter()
            .find(|state| state.kind == kind)
            .map(|state| state.tracker.stats())
            .unwrap_or_default()
    }

    pub(crate) fn input_port(&self) -> &str {
        &self.input_port
    }

    pub(crate) fn pending_display_packets(&self) -> usize {
        self.display_queue.pending_packets()
    }

    pub(crate) fn overflow_display_packets(&self) -> u64 {
        self.display_queue.overflow_packets()
    }
}

impl ObservedInputFormatState {
    fn prune_rate_samples(&mut self, now: Instant) {
        while let Some(sample) = self.rate_samples.front() {
            if now.duration_since(sample.at) <= Duration::from_secs(1) {
                break;
            }
            self.rate_samples.pop_front();
        }
    }

    fn rx_rate_hz(&self) -> f64 {
        self.rate_samples
            .iter()
            .map(|sample| sample.packet_count)
            .sum::<usize>() as f64
    }

    fn rx_bytes_per_second(&self) -> f64 {
        self.rate_samples
            .iter()
            .map(|sample| sample.byte_len)
            .sum::<usize>() as f64
    }

    fn status_line(
        &self,
        input_id: &str,
        target_rates_hz: &BTreeMap<XbeeTestFrameKind, f64>,
    ) -> String {
        let stats = self.tracker.stats();
        let target_rate_hz = target_rates_hz.get(&self.kind).copied().unwrap_or_default();
        if self.track_expected_packets {
            format!(
                "{} <- {}  format={}  target={:.1} Hz  rx={:.1} Hz {:.0} B/s  matched={}  err={:.2}%  pending={}  miss={}  bad={}  unexp={}  overflow={}",
                input_id,
                self.from_port_id,
                xbee_test_format_label(self.kind),
                target_rate_hz,
                self.rx_rate_hz(),
                self.rx_bytes_per_second(),
                stats.matched_packets,
                stats.error_rate_percent(),
                self.tracker.pending_packets(),
                stats.missing_packets,
                stats.invalid_packets,
                stats.unexpected_packets,
                stats.queue_overflow_packets
            )
        } else {
            format!(
                "{} <- {}  format={}  target={:.1} Hz  rx={:.1} Hz {:.0} B/s  valid={}  bad={}  overflow={}  track=passive",
                input_id,
                self.from_port_id,
                xbee_test_format_label(self.kind),
                target_rate_hz,
                self.rx_rate_hz(),
                self.rx_bytes_per_second(),
                stats.matched_packets,
                stats.invalid_packets,
                stats.queue_overflow_packets
            )
        }
    }
}

struct PacketRateSample {
    at: Instant,
    byte_len: usize,
    packet_count: usize,
}

#[derive(Default)]
pub(crate) struct ObservedInputBatch {
    pub(crate) valid_byte_len: usize,
    pub(crate) valid_packet_count: usize,
    pub(crate) per_kind_totals: BTreeMap<XbeeTestFrameKind, (usize, usize)>,
}

struct PacketDisplayQueue {
    pending_packets: VecDeque<Vec<u8>>,
    overflow_packets: u64,
}

impl PacketDisplayQueue {
    fn new() -> Self {
        Self {
            pending_packets: VecDeque::new(),
            overflow_packets: 0,
        }
    }

    fn enqueue(&mut self, packet: Vec<u8>) {
        self.pending_packets.push_back(packet);
        while self.pending_packets.len() > DISPLAY_QUEUE_LIMIT {
            self.pending_packets.pop_front();
            self.overflow_packets = self.overflow_packets.saturating_add(1);
        }
    }

    fn flush_input_batch(
        &mut self,
        session: &mut SessionRuntime,
        port: &str,
        max_packets: usize,
    ) -> Result<usize, String> {
        self.flush_batch(max_packets, |packet| session.add_input_entry(port, packet))
    }

    fn flush_output_batch(
        &mut self,
        session: &mut SessionRuntime,
        port: &str,
        max_packets: usize,
    ) -> Result<usize, String> {
        self.flush_batch(max_packets, |packet| session.add_output_entry(port, packet))
    }

    fn flush_batch(
        &mut self,
        max_packets: usize,
        mut write_entry: impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<usize, String> {
        let mut flushed = 0usize;
        while flushed < max_packets {
            let Some(packet) = self.pending_packets.pop_front() else {
                break;
            };
            write_entry(&packet)?;
            flushed += 1;
        }
        Ok(flushed)
    }

    fn pending_packets(&self) -> usize {
        self.pending_packets.len()
    }

    fn overflow_packets(&self) -> u64 {
        self.overflow_packets
    }
}

struct GeneratedSender {
    output_id: &'static str,
    target_input_id: &'static str,
    kind: XbeeTestFrameKind,
    generator: Box<dyn DummyPayloadGenerator>,
    sent_packets: u64,
    write_errors: u64,
    last_error: Option<String>,
}

impl GeneratedSender {
    fn new(
        output_id: &'static str,
        target_input_id: &'static str,
        kind: XbeeTestFrameKind,
    ) -> Result<Self, String> {
        Ok(Self {
            output_id,
            target_input_id,
            kind,
            generator: create_xbee_test_generator(kind)?,
            sent_packets: 0,
            write_errors: 0,
            last_error: None,
        })
    }

    fn send_once(
        &mut self,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) -> bool {
        let payload = match self.generator.next_payload() {
            Ok(payload) => payload,
            Err(error) => {
                self.write_errors = self.write_errors.saturating_add(1);
                self.last_error = Some(error);
                return false;
            }
        };

        match session.write_output(self.output_id, &payload) {
            Ok(()) => {
                session.record_output_sample(&output.port, payload.len(), 1);
                session.record_output_format_sample(
                    &output.port,
                    self.kind.display_name(),
                    payload.len(),
                    1,
                );
                output.display_queue.enqueue(payload.clone());
                input.expect(self.kind, payload);
                self.sent_packets = self.sent_packets.saturating_add(1);
                self.last_error = None;
                true
            }
            Err(error) => {
                self.write_errors = self.write_errors.saturating_add(1);
                self.last_error = Some(error);
                false
            }
        }
    }

    fn queue_expected_packet(&mut self, input: &mut ObservedInput) -> bool {
        let payload = match self.generator.next_payload() {
            Ok(payload) => payload,
            Err(error) => {
                self.write_errors = self.write_errors.saturating_add(1);
                self.last_error = Some(error);
                return false;
            }
        };

        input.expect(self.kind, payload);
        self.sent_packets = self.sent_packets.saturating_add(1);
        self.last_error = None;
        true
    }
}

struct SendSchedule {
    target_rate_hz: u32,
    period: Duration,
    next_send_at: Instant,
}

impl SendSchedule {
    fn new(target_rate_hz: u32, started_at: Instant, label: &str) -> Result<Self, String> {
        if target_rate_hz == 0 {
            return Err(format!("{label} rate must be greater than 0"));
        }

        Ok(Self {
            target_rate_hz,
            period: Duration::from_secs_f64(1.0 / target_rate_hz as f64),
            next_send_at: started_at,
        })
    }

    fn take_due_count(&mut self, now: Instant) -> usize {
        let mut due_count = 0usize;
        while now >= self.next_send_at {
            due_count += 1;
            self.next_send_at += self.period;
        }
        due_count
    }
}

pub(crate) struct ScheduledSender {
    sender: GeneratedSender,
    schedule: SendSchedule,
}

impl ScheduledSender {
    pub(crate) fn new(
        output_id: &'static str,
        target_input_id: &'static str,
        kind: XbeeTestFrameKind,
        target_rate_hz: u32,
        started_at: Instant,
    ) -> Result<Self, String> {
        Ok(Self {
            sender: GeneratedSender::new(output_id, target_input_id, kind)?,
            schedule: SendSchedule::new(target_rate_hz, started_at, kind.display_name())?,
        })
    }

    pub(crate) fn send_due_packets(
        &mut self,
        now: Instant,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) -> usize {
        let mut sent_packets = 0usize;
        for _ in 0..self.schedule.take_due_count(now) {
            if self.sender.send_once(session, input, output) {
                sent_packets += 1;
            }
        }
        sent_packets
    }

    pub(crate) fn send_once(
        &mut self,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) -> bool {
        self.sender.send_once(session, input, output)
    }

    pub(crate) fn queue_expected_due_packets(
        &mut self,
        now: Instant,
        input: &mut ObservedInput,
    ) -> usize {
        let mut generated_packets = 0usize;
        for _ in 0..self.schedule.take_due_count(now) {
            if self.sender.queue_expected_packet(input) {
                generated_packets += 1;
            }
        }
        generated_packets
    }

    pub(crate) fn queue_expected_once(&mut self, input: &mut ObservedInput) -> bool {
        self.sender.queue_expected_packet(input)
    }

    pub(crate) fn status_line(&self) -> String {
        let mut line = format!(
            "{} -> {}  format={}  target={} Hz  sent={}  tx_err={}",
            self.sender.output_id,
            self.sender.target_input_id,
            xbee_test_format_label(self.sender.kind),
            self.schedule.target_rate_hz,
            self.sender.sent_packets,
            self.sender.write_errors
        );
        if let Some(error) = &self.sender.last_error {
            line.push_str("  last_error=");
            line.push_str(error);
        }
        line
    }

    fn output_id(&self) -> &'static str {
        self.sender.output_id
    }

    fn target_input_id(&self) -> &'static str {
        self.sender.target_input_id
    }

    fn kind(&self) -> XbeeTestFrameKind {
        self.sender.kind
    }

    pub(crate) fn target_rate_hz(&self) -> u32 {
        self.schedule.target_rate_hz
    }

    pub(crate) fn sent_packets(&self) -> u64 {
        self.sender.sent_packets
    }

    pub(crate) fn write_errors(&self) -> u64 {
        self.sender.write_errors
    }

    fn last_error(&self) -> Option<&str> {
        self.sender.last_error.as_deref()
    }
}

struct SimpleRng {
    state: u64,
}

impl Default for SimpleRng {
    fn default() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let seed = nanos ^ ((std::process::id() as u64) << 32) ^ 0x9E37_79B9_7F4A_7C15;
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }
}

impl SimpleRng {
    fn next_u64(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.state = value;
        value.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn percent_chance(&mut self, percent: u32) -> bool {
        if percent == 0 {
            return false;
        }
        if percent >= 100 {
            return true;
        }

        self.next_u64() % 100 < percent as u64
    }
}

struct PollingModeState {
    poll_schedule: SendSchedule,
    base_poll_sender: GeneratedSender,
    remote_poll_sender: GeneratedSender,
    base_real_percent: u32,
    remote_real_percent: u32,
    pending_greeting_replies: usize,
    pending_real_replies: usize,
    base_greeting_cycles: u64,
    base_real_cycles: u64,
    remote_response_cycles: u64,
    remote_real_cycles: u64,
    rng: SimpleRng,
}

impl PollingModeState {
    fn new(settings: &XbeeTestSettings, started_at: Instant) -> Result<Self, String> {
        Ok(Self {
            poll_schedule: SendSchedule::new(
                settings.poll_rate_hz,
                started_at,
                POLL_GREETING_FORMAT_NAME,
            )?,
            base_poll_sender: GeneratedSender::new(
                BASE_POLL_OUTPUT_ID,
                REMOTE_PORT_ID,
                XbeeTestFrameKind::PollGreeting,
            )?,
            remote_poll_sender: GeneratedSender::new(
                REMOTE_POLL_OUTPUT_ID,
                BASE_PORT_ID,
                XbeeTestFrameKind::PollResponse,
            )?,
            base_real_percent: settings.base_real_percent,
            remote_real_percent: settings.remote_real_percent,
            pending_greeting_replies: 0,
            pending_real_replies: 0,
            base_greeting_cycles: 0,
            base_real_cycles: 0,
            remote_response_cycles: 0,
            remote_real_cycles: 0,
            rng: SimpleRng::default(),
        })
    }

    fn take_due_polls(&mut self, now: Instant) -> usize {
        self.poll_schedule.take_due_count(now)
    }

    fn base_send_real(&mut self) -> bool {
        self.rng.percent_chance(self.base_real_percent)
    }

    fn remote_send_real(&mut self) -> bool {
        self.rng.percent_chance(self.remote_real_percent)
    }

    fn note_base_greeting_cycle(&mut self, sent: bool) {
        if sent {
            self.pending_greeting_replies += 1;
            self.base_greeting_cycles = self.base_greeting_cycles.saturating_add(1);
        }
    }

    fn note_base_real_cycle(&mut self, sent: bool) {
        if sent {
            self.pending_real_replies += 1;
            self.base_real_cycles = self.base_real_cycles.saturating_add(1);
        }
    }

    fn consume_ready_remote_replies(
        &mut self,
        observed: &BTreeMap<XbeeTestFrameKind, (usize, usize)>,
    ) -> usize {
        let greeting_count = observed
            .get(&XbeeTestFrameKind::PollGreeting)
            .map(|(_, packet_count)| *packet_count)
            .unwrap_or(0);
        let au_count = observed
            .get(&XbeeTestFrameKind::Format(OutputFormat::PacketAcV6))
            .map(|(_, packet_count)| *packet_count)
            .unwrap_or(0);
        let ru_count = observed
            .get(&XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral))
            .map(|(_, packet_count)| *packet_count)
            .unwrap_or(0);

        let greeting_ready = greeting_count.min(self.pending_greeting_replies);
        self.pending_greeting_replies -= greeting_ready;

        let real_ready = au_count.max(ru_count).min(self.pending_real_replies);
        self.pending_real_replies -= real_ready;

        greeting_ready + real_ready
    }

    fn note_remote_response_cycle(&mut self, sent: bool, used_real_packets: bool) {
        if sent {
            self.remote_response_cycles = self.remote_response_cycles.saturating_add(1);
            if used_real_packets {
                self.remote_real_cycles = self.remote_real_cycles.saturating_add(1);
            }
        }
    }
}

pub(crate) struct DisplayedOutput {
    port: String,
    display_queue: PacketDisplayQueue,
}

impl DisplayedOutput {
    pub(crate) fn new(port: String) -> Self {
        Self {
            port,
            display_queue: PacketDisplayQueue::new(),
        }
    }

    pub(crate) fn flush_display_batch(
        &mut self,
        session: &mut SessionRuntime,
        max_packets: usize,
    ) -> Result<usize, String> {
        self.display_queue
            .flush_output_batch(session, &self.port, max_packets)
    }

    pub(crate) fn pending_packets(&self) -> usize {
        self.display_queue.pending_packets()
    }

    pub(crate) fn overflow_packets(&self) -> u64 {
        self.display_queue.overflow_packets()
    }
}

struct XbeeTestState {
    base_port: ResolvedXbeeTestPort,
    remote_port: ResolvedXbeeTestPort,
    mode: XbeeTestMode,
    poll_rate_hz: u32,
    base_real_percent: u32,
    remote_real_percent: u32,
    log_path_display: String,
    logging_enabled: bool,
    display_fps: f64,
    au_sender: ScheduledSender,
    ru_sender: ScheduledSender,
    ad_sender: ScheduledSender,
    rd_sender: ScheduledSender,
    polling: Option<PollingModeState>,
    base_output: DisplayedOutput,
    remote_output: DisplayedOutput,
    base_input: ObservedInput,
    remote_input: ObservedInput,
    last_status_update: Instant,
    header_lines: Vec<String>,
}

impl XbeeTestState {
    fn new(
        settings: &XbeeTestSettings,
        log_path_display: String,
        started_at: Instant,
    ) -> Result<Self, String> {
        let mut base_input_kinds = vec![
            ObservedInputKindSpec::strict(XbeeTestFrameKind::Format(OutputFormat::PacketJfV1)),
            ObservedInputKindSpec::strict(XbeeTestFrameKind::Format(
                OutputFormat::RoverDownGeneral,
            )),
        ];
        let mut remote_input_kinds = vec![
            ObservedInputKindSpec::strict(XbeeTestFrameKind::Format(OutputFormat::PacketAcV6)),
            ObservedInputKindSpec::strict(XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral)),
        ];
        if settings.mode == XbeeTestMode::Polling {
            base_input_kinds.insert(
                0,
                ObservedInputKindSpec::strict(XbeeTestFrameKind::PollResponse),
            );
            remote_input_kinds.insert(
                0,
                ObservedInputKindSpec::strict(XbeeTestFrameKind::PollGreeting),
            );
        }

        Ok(Self {
            base_port: settings.base_port.clone(),
            remote_port: settings.remote_port.clone(),
            mode: settings.mode,
            poll_rate_hz: settings.poll_rate_hz,
            base_real_percent: settings.base_real_percent,
            remote_real_percent: settings.remote_real_percent,
            log_path_display,
            logging_enabled: settings.logging_enabled,
            display_fps: 0.0,
            au_sender: ScheduledSender::new(
                BASE_AU_OUTPUT_ID,
                REMOTE_PORT_ID,
                XbeeTestFrameKind::Format(OutputFormat::PacketAcV6),
                settings.au_rate_hz,
                started_at,
            )?,
            ru_sender: ScheduledSender::new(
                BASE_RU_OUTPUT_ID,
                REMOTE_PORT_ID,
                XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral),
                settings.ru_rate_hz,
                started_at,
            )?,
            ad_sender: ScheduledSender::new(
                REMOTE_AD_OUTPUT_ID,
                BASE_PORT_ID,
                XbeeTestFrameKind::Format(OutputFormat::PacketJfV1),
                settings.ad_rate_hz,
                started_at,
            )?,
            rd_sender: ScheduledSender::new(
                REMOTE_RD_OUTPUT_ID,
                BASE_PORT_ID,
                XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral),
                settings.rd_rate_hz,
                started_at,
            )?,
            polling: (settings.mode == XbeeTestMode::Polling)
                .then(|| PollingModeState::new(settings, started_at))
                .transpose()?,
            base_output: DisplayedOutput::new(settings.base_port.port.clone()),
            remote_output: DisplayedOutput::new(settings.remote_port.port.clone()),
            base_input: ObservedInput::new(
                BASE_PORT_ID,
                settings.base_port.port.clone(),
                REMOTE_PORT_ID,
                base_input_kinds,
            ),
            remote_input: ObservedInput::new(
                REMOTE_PORT_ID,
                settings.remote_port.port.clone(),
                BASE_PORT_ID,
                remote_input_kinds,
            ),
            last_status_update: started_at
                .checked_sub(STATUS_INTERVAL)
                .unwrap_or(started_at),
            header_lines: Vec::new(),
        })
    }

    fn handle_input(&mut self, frame: &IngressFrame, session: &mut SessionRuntime) {
        let now = Instant::now();
        match frame.input_id.as_str() {
            BASE_PORT_ID => {
                let batch = self.base_input.observe(&frame.bytes, now);
                if batch.valid_packet_count > 0 {
                    Self::record_observed_input_batch(session, &self.base_input.input_port, &batch);
                }
            }
            REMOTE_PORT_ID => {
                let batch = self.remote_input.observe(&frame.bytes, now);
                if batch.valid_packet_count > 0 {
                    Self::record_observed_input_batch(
                        session,
                        &self.remote_input.input_port,
                        &batch,
                    );
                    if self.mode == XbeeTestMode::PingPong {
                        let ac_count = batch
                            .per_kind_totals
                            .get(&XbeeTestFrameKind::Format(OutputFormat::PacketAcV6))
                            .map(|(_, packet_count)| *packet_count)
                            .unwrap_or(0);
                        for _ in 0..ac_count {
                            self.ad_sender.send_once(
                                session,
                                &mut self.base_input,
                                &mut self.remote_output,
                            );
                        }
                        let up_count = batch
                            .per_kind_totals
                            .get(&XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral))
                            .map(|(_, packet_count)| *packet_count)
                            .unwrap_or(0);
                        for _ in 0..up_count {
                            self.rd_sender.send_once(
                                session,
                                &mut self.base_input,
                                &mut self.remote_output,
                            );
                        }
                    } else if self.mode == XbeeTestMode::Polling {
                        let reply_count = self
                            .polling
                            .as_mut()
                            .map(|polling| {
                                polling.consume_ready_remote_replies(&batch.per_kind_totals)
                            })
                            .unwrap_or(0);
                        for _ in 0..reply_count {
                            self.send_polling_remote_reply(session);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn on_tick(&mut self, session: &mut SessionRuntime) -> Result<(), String> {
        let now = Instant::now();
        match self.mode {
            XbeeTestMode::Flood => {
                self.au_sender.send_due_packets(
                    now,
                    session,
                    &mut self.remote_input,
                    &mut self.base_output,
                );
                self.ru_sender.send_due_packets(
                    now,
                    session,
                    &mut self.remote_input,
                    &mut self.base_output,
                );
                self.ad_sender.send_due_packets(
                    now,
                    session,
                    &mut self.base_input,
                    &mut self.remote_output,
                );
                self.rd_sender.send_due_packets(
                    now,
                    session,
                    &mut self.base_input,
                    &mut self.remote_output,
                );
            }
            XbeeTestMode::PingPong => {
                self.au_sender.send_due_packets(
                    now,
                    session,
                    &mut self.remote_input,
                    &mut self.base_output,
                );
                self.ru_sender.send_due_packets(
                    now,
                    session,
                    &mut self.remote_input,
                    &mut self.base_output,
                );
            }
            XbeeTestMode::Polling => {
                let due_polls = self
                    .polling
                    .as_mut()
                    .map(|polling| polling.take_due_polls(now))
                    .unwrap_or(0);
                for _ in 0..due_polls {
                    self.send_poll_cycle(session);
                }
            }
        }

        self.flush_display_queues(session)?;

        if now.duration_since(self.last_status_update) >= STATUS_INTERVAL {
            self.last_status_update = now;
            self.display_fps = session.current_render_fps();
            let header_lines = self.build_header_lines();
            if header_lines != self.header_lines {
                self.header_lines = header_lines.clone();
                session.set_header_lines(header_lines);
            }
        }
        Ok(())
    }

    fn send_poll_cycle(&mut self, session: &mut SessionRuntime) {
        let send_real = self
            .polling
            .as_mut()
            .map(PollingModeState::base_send_real)
            .unwrap_or(false);

        if send_real {
            let sent_au =
                self.au_sender
                    .send_once(session, &mut self.remote_input, &mut self.base_output);
            let sent_ru =
                self.ru_sender
                    .send_once(session, &mut self.remote_input, &mut self.base_output);
            if let Some(polling) = self.polling.as_mut() {
                polling.note_base_real_cycle(sent_au || sent_ru);
            }
        } else {
            let sent = {
                let polling = self.polling.as_mut().expect("polling state exists");
                polling.base_poll_sender.send_once(
                    session,
                    &mut self.remote_input,
                    &mut self.base_output,
                )
            };
            if let Some(polling) = self.polling.as_mut() {
                polling.note_base_greeting_cycle(sent);
            }
        }
    }

    fn send_polling_remote_reply(&mut self, session: &mut SessionRuntime) {
        let send_real = self
            .polling
            .as_mut()
            .map(PollingModeState::remote_send_real)
            .unwrap_or(false);

        if send_real {
            let sent_ad =
                self.ad_sender
                    .send_once(session, &mut self.base_input, &mut self.remote_output);
            let sent_rd =
                self.rd_sender
                    .send_once(session, &mut self.base_input, &mut self.remote_output);
            if let Some(polling) = self.polling.as_mut() {
                polling.note_remote_response_cycle(sent_ad || sent_rd, true);
            }
        } else {
            let sent = {
                let polling = self.polling.as_mut().expect("polling state exists");
                polling.remote_poll_sender.send_once(
                    session,
                    &mut self.base_input,
                    &mut self.remote_output,
                )
            };
            if let Some(polling) = self.polling.as_mut() {
                polling.note_remote_response_cycle(sent, false);
            }
        }
    }

    fn record_observed_input_batch(
        session: &mut SessionRuntime,
        input_port: &str,
        batch: &ObservedInputBatch,
    ) {
        session.record_input_sample(input_port, batch.valid_byte_len, batch.valid_packet_count);
        for (&kind, &(byte_len, packet_count)) in &batch.per_kind_totals {
            session.record_input_format_sample(
                input_port,
                kind.display_name(),
                byte_len,
                packet_count,
            );
        }
    }

    fn flush_display_queues(&mut self, session: &mut SessionRuntime) -> Result<(), String> {
        let mut remaining_budget = DISPLAY_FLUSH_PACKET_BUDGET;

        while remaining_budget > 0 {
            let mut flushed_total = 0usize;

            let slice = remaining_budget.min(DISPLAY_FLUSH_SLICE);
            let flushed = self.base_output.flush_display_batch(session, slice)?;
            remaining_budget = remaining_budget.saturating_sub(flushed);
            flushed_total += flushed;
            if remaining_budget == 0 {
                break;
            }

            let slice = remaining_budget.min(DISPLAY_FLUSH_SLICE);
            let flushed = self.remote_output.flush_display_batch(session, slice)?;
            remaining_budget = remaining_budget.saturating_sub(flushed);
            flushed_total += flushed;
            if remaining_budget == 0 {
                break;
            }

            let slice = remaining_budget.min(DISPLAY_FLUSH_SLICE);
            let flushed = self.base_input.flush_display_batch(session, slice)?;
            remaining_budget = remaining_budget.saturating_sub(flushed);
            flushed_total += flushed;
            if remaining_budget == 0 {
                break;
            }

            let slice = remaining_budget.min(DISPLAY_FLUSH_SLICE);
            let flushed = self.remote_input.flush_display_batch(session, slice)?;
            remaining_budget = remaining_budget.saturating_sub(flushed);
            flushed_total += flushed;

            if flushed_total == 0 {
                break;
            }
        }

        Ok(())
    }

    fn build_header_lines(&self) -> Vec<String> {
        let remote_targets = self.remote_target_rates();
        let base_targets = self.base_target_rates();
        let mut lines = match self.mode {
            XbeeTestMode::Polling => {
                let polling = self.polling.as_ref().expect("polling state exists");
                vec![
                    format!(
                        "mode=polling  poll={}Hz  fps={:.1}  real(base={}%% remote={}%%)  pending={}/{}",
                        self.poll_rate_hz,
                        self.display_fps,
                        self.base_real_percent,
                        self.remote_real_percent,
                        polling.pending_greeting_replies,
                        polling.pending_real_replies
                    ),
                    format!(
                        "base={}@{}  remote={}@{}",
                        self.base_port.port,
                        self.base_port.baud_rate,
                        self.remote_port.port,
                        self.remote_port.baud_rate
                    ),
                ]
            }
            _ => vec![
                format!(
                    "mode={}  display={:.1} fps",
                    self.mode.as_str(),
                    self.display_fps
                ),
                format!(
                    "{BASE_PORT_ID}: {} @ {} baud  tx=AU+RU  rx=AD+RD",
                    self.base_port.port, self.base_port.baud_rate,
                ),
                format!(
                    "{REMOTE_PORT_ID}: {} @ {} baud  tx=AD+RD  rx=AU+RU",
                    self.remote_port.port, self.remote_port.baud_rate,
                ),
            ],
        };

        match self.mode {
            XbeeTestMode::Flood | XbeeTestMode::PingPong => {
                lines.push(self.au_sender.status_line());
                lines.push(self.ru_sender.status_line());
                lines.extend(self.remote_input.status_lines(&remote_targets));
                lines.push(match self.mode {
                    XbeeTestMode::Flood => self.ad_sender.status_line(),
                    XbeeTestMode::PingPong => {
                        self.triggered_sender_line(&self.ad_sender, "trigger=au-rx")
                    }
                    XbeeTestMode::Polling => unreachable!(),
                });
                lines.push(match self.mode {
                    XbeeTestMode::Flood => self.rd_sender.status_line(),
                    XbeeTestMode::PingPong => {
                        self.triggered_sender_line(&self.rd_sender, "trigger=ru-rx")
                    }
                    XbeeTestMode::Polling => unreachable!(),
                });
                lines.extend(self.base_input.status_lines(&base_targets));
            }
            XbeeTestMode::Polling => {
                let polling = self.polling.as_ref().expect("polling state exists");
                lines.push(Self::generated_sender_line(
                    &polling.base_poll_sender,
                    &format!("role=base-greeting  target={} Hz", self.poll_rate_hz),
                ));
                lines.push(Self::scheduled_sender_line(
                    &self.au_sender,
                    "role=base-real",
                ));
                lines.push(Self::scheduled_sender_line(
                    &self.ru_sender,
                    "role=base-real",
                ));
                lines.extend(self.remote_input.status_lines(&remote_targets));
                lines.push(Self::generated_sender_line(
                    &polling.remote_poll_sender,
                    "role=remote-response",
                ));
                lines.push(Self::scheduled_sender_line(
                    &self.ad_sender,
                    "role=remote-real",
                ));
                lines.push(Self::scheduled_sender_line(
                    &self.rd_sender,
                    "role=remote-real",
                ));
                lines.extend(self.base_input.status_lines(&base_targets));
            }
        }

        lines.extend([
            format!(
                "display backlog  out(base={}, remote={})  in(base={}, remote={})  overflow={}",
                self.base_output.pending_packets(),
                self.remote_output.pending_packets(),
                self.base_input.display_queue.pending_packets(),
                self.remote_input.display_queue.pending_packets(),
                self.base_output.overflow_packets()
                    + self.remote_output.overflow_packets()
                    + self.base_input.display_queue.overflow_packets()
                    + self.remote_input.display_queue.overflow_packets()
            ),
            if self.logging_enabled {
                format!("log: {}", self.log_path_display)
            } else {
                String::from("log: disabled (--no-log)")
            },
            String::from(SPACE_HINT),
        ]);
        lines
    }

    fn triggered_sender_line(&self, sender: &ScheduledSender, detail: &str) -> String {
        let mut line = format!(
            "{} -> {}  format={}  {}  sent={}  tx_err={}",
            sender.output_id(),
            sender.target_input_id(),
            xbee_test_format_label(sender.kind()),
            detail,
            sender.sent_packets(),
            sender.write_errors()
        );
        if let Some(error) = sender.last_error() {
            line.push_str("  last_error=");
            line.push_str(error);
        }
        line
    }

    fn scheduled_sender_line(sender: &ScheduledSender, detail: &str) -> String {
        let mut line = format!(
            "{} -> {}  format={}  {}  sent={}  tx_err={}",
            sender.output_id(),
            sender.target_input_id(),
            xbee_test_format_label(sender.kind()),
            detail,
            sender.sent_packets(),
            sender.write_errors()
        );
        if let Some(error) = sender.last_error() {
            line.push_str("  last_error=");
            line.push_str(error);
        }
        line
    }

    fn generated_sender_line(sender: &GeneratedSender, detail: &str) -> String {
        let mut line = format!(
            "{} -> {}  format={}  {}  sent={}  tx_err={}",
            sender.output_id,
            sender.target_input_id,
            xbee_test_format_label(sender.kind),
            detail,
            sender.sent_packets,
            sender.write_errors
        );
        if let Some(error) = &sender.last_error {
            line.push_str("  last_error=");
            line.push_str(error);
        }
        line
    }

    fn run_result(self, log_path: PathBuf) -> XbeeTestRunResult {
        XbeeTestRunResult {
            base_port: self.base_port,
            remote_port: self.remote_port,
            mode: self.mode,
            poll_rate_hz: self.poll_rate_hz,
            base_real_percent: self.base_real_percent,
            remote_real_percent: self.remote_real_percent,
            greeting_stats: self.remote_input.stats(XbeeTestFrameKind::PollGreeting),
            response_stats: self.base_input.stats(XbeeTestFrameKind::PollResponse),
            au_rate_hz: self.au_sender.target_rate_hz(),
            ru_rate_hz: self.ru_sender.target_rate_hz(),
            ad_rate_hz: self.ad_sender.target_rate_hz(),
            rd_rate_hz: self.rd_sender.target_rate_hz(),
            au_stats: self
                .remote_input
                .stats(XbeeTestFrameKind::Format(OutputFormat::PacketAcV6)),
            ru_stats: self
                .remote_input
                .stats(XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral)),
            ad_stats: self
                .base_input
                .stats(XbeeTestFrameKind::Format(OutputFormat::PacketJfV1)),
            rd_stats: self
                .base_input
                .stats(XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral)),
            logging_enabled: self.logging_enabled,
            log_path,
        }
    }

    fn remote_target_rates(&self) -> BTreeMap<XbeeTestFrameKind, f64> {
        match self.mode {
            XbeeTestMode::Flood | XbeeTestMode::PingPong => BTreeMap::from([
                (
                    XbeeTestFrameKind::Format(OutputFormat::PacketAcV6),
                    self.au_sender.target_rate_hz() as f64,
                ),
                (
                    XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral),
                    self.ru_sender.target_rate_hz() as f64,
                ),
            ]),
            XbeeTestMode::Polling => {
                let real_rate = self.poll_rate_hz as f64 * self.base_real_percent as f64 / 100.0;
                BTreeMap::from([
                    (
                        XbeeTestFrameKind::PollGreeting,
                        self.poll_rate_hz as f64 - real_rate,
                    ),
                    (
                        XbeeTestFrameKind::Format(OutputFormat::PacketAcV6),
                        real_rate,
                    ),
                    (
                        XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral),
                        real_rate,
                    ),
                ])
            }
        }
    }

    fn base_target_rates(&self) -> BTreeMap<XbeeTestFrameKind, f64> {
        match self.mode {
            XbeeTestMode::Flood => BTreeMap::from([
                (
                    XbeeTestFrameKind::Format(OutputFormat::PacketJfV1),
                    self.ad_sender.target_rate_hz() as f64,
                ),
                (
                    XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral),
                    self.rd_sender.target_rate_hz() as f64,
                ),
            ]),
            XbeeTestMode::PingPong => BTreeMap::from([
                (
                    XbeeTestFrameKind::Format(OutputFormat::PacketJfV1),
                    self.au_sender.target_rate_hz() as f64,
                ),
                (
                    XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral),
                    self.ru_sender.target_rate_hz() as f64,
                ),
            ]),
            XbeeTestMode::Polling => {
                let real_rate = self.poll_rate_hz as f64 * self.remote_real_percent as f64 / 100.0;
                BTreeMap::from([
                    (
                        XbeeTestFrameKind::PollResponse,
                        self.poll_rate_hz as f64 - real_rate,
                    ),
                    (
                        XbeeTestFrameKind::Format(OutputFormat::PacketJfV1),
                        real_rate,
                    ),
                    (
                        XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral),
                        real_rate,
                    ),
                ])
            }
        }
    }
}

pub(crate) fn run(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_xbee_test_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let cli_options = match parse_xbee_test_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_xbee_test_help(bin_name);
            return ExitCode::from(2);
        }
    };

    match run_with_options(cli_options) {
        Ok(result) => {
            match result.mode {
                XbeeTestMode::Flood => println!(
                    "mode={} base={}@{} remote={}@{} au={}Hz ru={}Hz ad={}Hz rd={}Hz",
                    result.mode.as_str(),
                    result.base_port.port,
                    result.base_port.baud_rate,
                    result.remote_port.port,
                    result.remote_port.baud_rate,
                    result.au_rate_hz,
                    result.ru_rate_hz,
                    result.ad_rate_hz,
                    result.rd_rate_hz
                ),
                XbeeTestMode::PingPong => println!(
                    "mode={} base={}@{} remote={}@{} au={}Hz ru={}Hz ad=reply-to-au rd=reply-to-ru",
                    result.mode.as_str(),
                    result.base_port.port,
                    result.base_port.baud_rate,
                    result.remote_port.port,
                    result.remote_port.baud_rate,
                    result.au_rate_hz,
                    result.ru_rate_hz
                ),
                XbeeTestMode::Polling => println!(
                    "mode={} base={}@{} remote={}@{} poll={}Hz base-real={}%% remote-real={}%%",
                    result.mode.as_str(),
                    result.base_port.port,
                    result.base_port.baud_rate,
                    result.remote_port.port,
                    result.remote_port.baud_rate,
                    result.poll_rate_hz,
                    result.base_real_percent,
                    result.remote_real_percent
                ),
            }
            if result.mode == XbeeTestMode::Polling {
                println!(
                    "  PollGreeting base->remote: matched={} err={:.2}% miss={} bad={} unexp={} overflow={}",
                    result.greeting_stats.matched_packets,
                    result.greeting_stats.error_rate_percent(),
                    result.greeting_stats.missing_packets,
                    result.greeting_stats.invalid_packets,
                    result.greeting_stats.unexpected_packets,
                    result.greeting_stats.queue_overflow_packets
                );
                println!(
                    "  PollResponse remote->base: matched={} err={:.2}% miss={} bad={} unexp={} overflow={}",
                    result.response_stats.matched_packets,
                    result.response_stats.error_rate_percent(),
                    result.response_stats.missing_packets,
                    result.response_stats.invalid_packets,
                    result.response_stats.unexpected_packets,
                    result.response_stats.queue_overflow_packets
                );
            }
            println!(
                "  AU base->remote: matched={} err={:.2}% miss={} bad={} unexp={} overflow={}",
                result.au_stats.matched_packets,
                result.au_stats.error_rate_percent(),
                result.au_stats.missing_packets,
                result.au_stats.invalid_packets,
                result.au_stats.unexpected_packets,
                result.au_stats.queue_overflow_packets
            );
            println!(
                "  RU base->remote: matched={} err={:.2}% miss={} bad={} unexp={} overflow={}",
                result.ru_stats.matched_packets,
                result.ru_stats.error_rate_percent(),
                result.ru_stats.missing_packets,
                result.ru_stats.invalid_packets,
                result.ru_stats.unexpected_packets,
                result.ru_stats.queue_overflow_packets
            );
            println!(
                "  AD remote->base: matched={} err={:.2}% miss={} bad={} unexp={} overflow={}",
                result.ad_stats.matched_packets,
                result.ad_stats.error_rate_percent(),
                result.ad_stats.missing_packets,
                result.ad_stats.invalid_packets,
                result.ad_stats.unexpected_packets,
                result.ad_stats.queue_overflow_packets
            );
            println!(
                "  RD remote->base: matched={} err={:.2}% miss={} bad={} unexp={} overflow={}",
                result.rd_stats.matched_packets,
                result.rd_stats.error_rate_percent(),
                result.rd_stats.missing_packets,
                result.rd_stats.invalid_packets,
                result.rd_stats.unexpected_packets,
                result.rd_stats.queue_overflow_packets
            );
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

fn run_with_options(cli_options: XbeeTestCliOptions) -> Result<XbeeTestRunResult, String> {
    let settings = build_settings(cli_options)?;
    let mut outputs = vec![
        SessionOutputSpec {
            id: BASE_AU_OUTPUT_ID.to_owned(),
            port: settings.base_port.port.clone(),
            baud_rate: settings.base_port.baud_rate,
            format_name: OutputFormat::PacketAcV6.as_str().to_owned(),
            display_mode: PortDisplayMode::Hex,
        },
        SessionOutputSpec {
            id: BASE_RU_OUTPUT_ID.to_owned(),
            port: settings.base_port.port.clone(),
            baud_rate: settings.base_port.baud_rate,
            format_name: OutputFormat::RoverUpGeneral.as_str().to_owned(),
            display_mode: PortDisplayMode::Hex,
        },
        SessionOutputSpec {
            id: REMOTE_AD_OUTPUT_ID.to_owned(),
            port: settings.remote_port.port.clone(),
            baud_rate: settings.remote_port.baud_rate,
            format_name: OutputFormat::PacketJfV1.as_str().to_owned(),
            display_mode: PortDisplayMode::Hex,
        },
        SessionOutputSpec {
            id: REMOTE_RD_OUTPUT_ID.to_owned(),
            port: settings.remote_port.port.clone(),
            baud_rate: settings.remote_port.baud_rate,
            format_name: OutputFormat::RoverDownGeneral.as_str().to_owned(),
            display_mode: PortDisplayMode::Hex,
        },
    ];
    if settings.mode == XbeeTestMode::Polling {
        outputs.push(SessionOutputSpec {
            id: BASE_POLL_OUTPUT_ID.to_owned(),
            port: settings.base_port.port.clone(),
            baud_rate: settings.base_port.baud_rate,
            format_name: XbeeTestFrameKind::PollGreeting
                .session_format_name()
                .to_owned(),
            display_mode: PortDisplayMode::Hex,
        });
        outputs.push(SessionOutputSpec {
            id: REMOTE_POLL_OUTPUT_ID.to_owned(),
            port: settings.remote_port.port.clone(),
            baud_rate: settings.remote_port.baud_rate,
            format_name: XbeeTestFrameKind::PollResponse
                .session_format_name()
                .to_owned(),
            display_mode: PortDisplayMode::Hex,
        });
    }
    let mut base_known_formats = vec![
        OutputFormat::PacketJfV1.display_name().to_owned(),
        OutputFormat::RoverDownGeneral.display_name().to_owned(),
    ];
    let mut remote_known_formats = vec![
        OutputFormat::PacketAcV6.display_name().to_owned(),
        OutputFormat::RoverUpGeneral.display_name().to_owned(),
    ];
    if settings.mode == XbeeTestMode::Polling {
        base_known_formats.insert(0, String::from(POLL_RESPONSE_FORMAT_NAME));
        remote_known_formats.insert(0, String::from(POLL_GREETING_FORMAT_NAME));
    }

    let mut session = SessionRuntime::new(SessionSpec {
        title: String::from("acs xbee-test"),
        command_name: String::from("xbee-test"),
        log_dir: settings.log_dir.clone(),
        logging_enabled: settings.logging_enabled,
        xbee_s3b_recovery: settings.s3b,
        inputs: vec![
            SessionInputSpec {
                id: BASE_PORT_ID.to_owned(),
                port: settings.base_port.port.clone(),
                baud_rate: settings.base_port.baud_rate,
                display_mode: PortDisplayMode::Hex,
                line_break_mode: LineBreakMode::Packet,
            },
            SessionInputSpec {
                id: REMOTE_PORT_ID.to_owned(),
                port: settings.remote_port.port.clone(),
                baud_rate: settings.remote_port.baud_rate,
                display_mode: PortDisplayMode::Hex,
                line_break_mode: LineBreakMode::Packet,
            },
        ],
        outputs,
    })?;
    session.set_output_packet_rate_enabled(&settings.base_port.port, true);
    session.set_output_packet_rate_enabled(&settings.remote_port.port, true);
    session.set_input_packet_rate_enabled(&settings.base_port.port, true);
    session.set_input_packet_rate_enabled(&settings.remote_port.port, true);
    session.set_input_known_formats(&settings.base_port.port, base_known_formats);
    session.set_input_known_formats(&settings.remote_port.port, remote_known_formats);
    session.set_manual_input_recording(BASE_PORT_ID, true);
    session.set_manual_input_recording(REMOTE_PORT_ID, true);
    session.set_manual_output_recording(BASE_AU_OUTPUT_ID, true);
    session.set_manual_output_recording(BASE_RU_OUTPUT_ID, true);
    session.set_manual_output_recording(REMOTE_AD_OUTPUT_ID, true);
    session.set_manual_output_recording(REMOTE_RD_OUTPUT_ID, true);
    if settings.mode == XbeeTestMode::Polling {
        session.set_manual_output_recording(BASE_POLL_OUTPUT_ID, true);
        session.set_manual_output_recording(REMOTE_POLL_OUTPUT_ID, true);
    }

    let log_path = session.log_path().to_path_buf();
    let started_at = Instant::now();
    let state = RefCell::new(XbeeTestState::new(
        &settings,
        log_path.display().to_string(),
        started_at,
    )?);
    session.set_header_lines(state.borrow().build_header_lines());
    signal::install_handler();

    session.run_loop_with_tick(
        LOOP_INTERVAL,
        signal::is_stop_requested,
        |frame, session| {
            state.borrow_mut().handle_input(frame, session);
            Ok(())
        },
        |session| state.borrow_mut().on_tick(session),
    )?;

    Ok(state.into_inner().run_result(log_path))
}

fn build_settings(cli_options: XbeeTestCliOptions) -> Result<XbeeTestSettings, String> {
    let mut ports = BTreeMap::new();

    for binding in cli_options.ports {
        ports.insert(
            binding.id.clone(),
            XbeeTestPortBinding {
                id: binding.id,
                port: binding.port,
                baud: binding.baud,
            },
        );
    }

    let base_binding = ports.remove(BASE_PORT_ID).ok_or_else(|| {
        String::from("xbee-test requires `--port base=PORT` (optional `@BAUD`, default: 115200)")
    })?;
    let remote_binding = ports.remove(REMOTE_PORT_ID).ok_or_else(|| {
        String::from("xbee-test requires `--port remote=PORT` (optional `@BAUD`, default: 115200)")
    })?;

    let base_port =
        serial::resolve_port(Some(&base_binding.port)).map_err(|error| error.to_string())?;
    let remote_port =
        serial::resolve_port(Some(&remote_binding.port)).map_err(|error| error.to_string())?;
    if base_port == remote_port {
        return Err(String::from(
            "xbee-test requires different ports for `base` and `remote`",
        ));
    }

    let mode = cli_options
        .mode
        .as_deref()
        .map(XbeeTestMode::parse)
        .transpose()?
        .unwrap_or(XbeeTestMode::Flood);
    let poll_rate_hz = cli_options.poll_rate_hz.unwrap_or(DEFAULT_RATE_HZ);
    let base_real_percent = cli_options
        .base_real_percent
        .unwrap_or(DEFAULT_REAL_PERCENT);
    let remote_real_percent = cli_options
        .remote_real_percent
        .unwrap_or(DEFAULT_REAL_PERCENT);

    let au_rate_hz = cli_options.au_rate_hz.unwrap_or(DEFAULT_RATE_HZ);
    let ru_rate_hz = cli_options.ru_rate_hz.unwrap_or(DEFAULT_RATE_HZ);
    let ad_rate_hz = cli_options.ad_rate_hz.unwrap_or(DEFAULT_RATE_HZ);
    let rd_rate_hz = cli_options.rd_rate_hz.unwrap_or(DEFAULT_RATE_HZ);
    if au_rate_hz == 0 {
        return Err(String::from("--au-rate must be greater than 0"));
    }
    if ru_rate_hz == 0 {
        return Err(String::from("--ru-rate must be greater than 0"));
    }
    if mode == XbeeTestMode::Polling && poll_rate_hz == 0 {
        return Err(String::from("--poll-rate must be greater than 0"));
    }
    if mode == XbeeTestMode::Flood && ad_rate_hz == 0 {
        return Err(String::from("--ad-rate must be greater than 0"));
    }
    if mode == XbeeTestMode::Flood && rd_rate_hz == 0 {
        return Err(String::from("--rd-rate must be greater than 0"));
    }
    if base_real_percent > 100 {
        return Err(String::from(
            "--base-real-percent must be between 0 and 100",
        ));
    }
    if remote_real_percent > 100 {
        return Err(String::from(
            "--remote-real-percent must be between 0 and 100",
        ));
    }
    let poll_rate_hz = poll_rate_hz.max(1);
    let ad_rate_hz = ad_rate_hz.max(1);
    let rd_rate_hz = rd_rate_hz.max(1);

    let log_dir = cli_options.log_dir.unwrap_or_else(default_log_dir);

    Ok(XbeeTestSettings {
        base_port: ResolvedXbeeTestPort {
            port: base_port,
            baud_rate: base_binding.baud.unwrap_or_else(default_baud_rate),
        },
        remote_port: ResolvedXbeeTestPort {
            port: remote_port,
            baud_rate: remote_binding.baud.unwrap_or_else(default_baud_rate),
        },
        mode,
        poll_rate_hz,
        base_real_percent,
        remote_real_percent,
        au_rate_hz,
        ru_rate_hz,
        ad_rate_hz,
        rd_rate_hz,
        log_dir,
        logging_enabled: !cli_options.no_log,
        s3b: cli_options.s3b,
    })
}

fn parse_xbee_test_args(args: Vec<String>) -> Result<XbeeTestCliOptions, String> {
    let mut options = XbeeTestCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--port" | "-p" => options.ports.push(parse_xbee_test_port_binding(&next_value(
                &mut iter, "--port",
            )?)?),
            "--config" => {
                apply_xbee_test_config_args(&mut options, &next_value(&mut iter, "--config")?)?
            }
            "--mode" => options.mode = Some(next_value(&mut iter, "--mode")?),
            "--poll-rate" => {
                let value = next_value(&mut iter, "--poll-rate")?;
                options.poll_rate_hz = Some(parse_u32_arg("--poll-rate", &value)?);
            }
            "--base-real-percent" => {
                let value = next_value(&mut iter, "--base-real-percent")?;
                options.base_real_percent = Some(parse_u32_arg("--base-real-percent", &value)?);
            }
            "--remote-real-percent" => {
                let value = next_value(&mut iter, "--remote-real-percent")?;
                options.remote_real_percent = Some(parse_u32_arg("--remote-real-percent", &value)?);
            }
            "--au-rate" => {
                let value = next_value(&mut iter, "--au-rate")?;
                options.au_rate_hz = Some(parse_u32_arg("--au-rate", &value)?);
            }
            "--ru-rate" => {
                let value = next_value(&mut iter, "--ru-rate")?;
                options.ru_rate_hz = Some(parse_u32_arg("--ru-rate", &value)?);
            }
            "--ad-rate" => {
                let value = next_value(&mut iter, "--ad-rate")?;
                options.ad_rate_hz = Some(parse_u32_arg("--ad-rate", &value)?);
            }
            "--rd-rate" => {
                let value = next_value(&mut iter, "--rd-rate")?;
                options.rd_rate_hz = Some(parse_u32_arg("--rd-rate", &value)?);
            }
            "--log-dir" => {
                options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            "--no-log" => options.no_log = true,
            "--s3b" => options.s3b = true,
            other => return Err(format!("unknown option for xbee-test: {other}")),
        }
    }

    Ok(options)
}

fn apply_xbee_test_config_args(
    options: &mut XbeeTestCliOptions,
    value: &str,
) -> Result<(), String> {
    for assignment in parse_key_value_args("--config", value)? {
        let key = assignment.key.to_ascii_uppercase();
        match key.as_str() {
            "MODE" => options.mode = Some(assignment.value),
            "POLL_RATE" => {
                options.poll_rate_hz = Some(parse_u32_arg("POLL_RATE", &assignment.value)?);
            }
            "BASE_REAL_PERCENT" => {
                options.base_real_percent =
                    Some(parse_u32_arg("BASE_REAL_PERCENT", &assignment.value)?);
            }
            "REMOTE_REAL_PERCENT" => {
                options.remote_real_percent =
                    Some(parse_u32_arg("REMOTE_REAL_PERCENT", &assignment.value)?);
            }
            "AU_RATE" => options.au_rate_hz = Some(parse_u32_arg("AU_RATE", &assignment.value)?),
            "RU_RATE" => options.ru_rate_hz = Some(parse_u32_arg("RU_RATE", &assignment.value)?),
            "AD_RATE" => options.ad_rate_hz = Some(parse_u32_arg("AD_RATE", &assignment.value)?),
            "RD_RATE" => options.rd_rate_hz = Some(parse_u32_arg("RD_RATE", &assignment.value)?),
            "LOG_DIR" => options.log_dir = Some(PathBuf::from(assignment.value)),
            other => return Err(format!("unknown xbee-test config key: {other}")),
        }
    }

    Ok(())
}

fn parse_xbee_test_port_binding(value: &str) -> Result<XbeeTestPortBinding, String> {
    let Some((id, port_text)) = value.split_once('=') else {
        return Err(format!(
            "invalid xbee-test port binding: {value} (expected base=PORT or remote=PORT; append `@BAUD` to override 115200)"
        ));
    };

    let id = normalize_port_id(id)?;
    let port_spec = parse_port_spec("xbee-test port binding", port_text)?;
    if port_spec.display_mode.is_some() || port_spec.line_break_mode.is_some() {
        return Err(String::from(
            "xbee-test fixes display to hex packet mode; omit `,DISPLAY` from --port",
        ));
    }

    Ok(XbeeTestPortBinding {
        id,
        port: port_spec.port,
        baud: port_spec.baud,
    })
}

fn normalize_port_id(value: &str) -> Result<String, String> {
    let id = value.trim().to_ascii_lowercase();
    match id.as_str() {
        BASE_PORT_ID | REMOTE_PORT_ID => Ok(id),
        _ => Err(format!(
            "unsupported xbee-test port label: {value} (expected `base` or `remote`)"
        )),
    }
}

fn find_header(buffer: &[u8], header: &[u8; 2]) -> Option<usize> {
    buffer
        .windows(header.len())
        .position(|window| window == header)
}

fn find_byte(buffer: &[u8], header: u8) -> Option<usize> {
    buffer.iter().position(|byte| *byte == header)
}

fn packet_is_valid(packet: &[u8], definition: PacketDefinition) -> bool {
    if packet.len() != definition.packet_len || packet[..2] != definition.header {
        return false;
    }

    let expected_crc = crc16_ccitt_false(&packet[..definition.payload_len]);
    let actual_crc = u16::from_le_bytes([
        packet[definition.payload_len],
        packet[definition.payload_len + 1],
    ]);
    expected_crc == actual_crc
}

fn matches_reduced_ac_packet(packet: &[u8], header: u8, packet_len: usize) -> bool {
    if packet.len() != packet_len || packet.first().copied() != Some(header) {
        return false;
    }

    let payload_len = packet_len - 2;
    let expected_crc = crc16_ccitt_false(&packet[..payload_len]);
    let actual_crc = u16::from_le_bytes([packet[payload_len], packet[payload_len + 1]]);
    expected_crc == actual_crc
}

fn matches_rover_up_packet(bytes: &[u8]) -> bool {
    bytes.len() == OutputFormat::RoverUpGeneral.packet_len()
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            0 => *byte == b'0',
            1 => *byte == b'x',
            2 => *byte == b'3',
            3 | 4 => byte.is_ascii_hexdigit(),
            5 => *byte == b',',
            6..=9 => byte.is_ascii_digit(),
            10 => *byte == b'\r',
            11 => *byte == b'\n',
            _ => false,
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

fn is_rover_down_kind(kind: XbeeTestFrameKind) -> bool {
    matches!(
        kind,
        XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral)
    )
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\r\n")
}

#[cfg(test)]
mod tests {
    use super::{
        ExpectedPacketTracker, OutputFormat, PacketDefinition, PacketStreamDecoder,
        XbeeTestFrameKind, XbeeTestMode, build_poll_frame, matches_poll_frame,
        matches_reduced_ac_packet, matches_rover_down_packet, parse_xbee_test_args,
        parse_xbee_test_port_binding,
    };

    #[test]
    fn parse_xbee_test_args_accepts_labeled_ports_and_rates() {
        let options = parse_xbee_test_args(vec![
            String::from("--port"),
            String::from("base=/dev/ttyUSB0@921600"),
            String::from("--port"),
            String::from("remote=/dev/ttyUSB1@115200"),
            String::from("--mode"),
            String::from("ping-pong"),
            String::from("--poll-rate"),
            String::from("100"),
            String::from("--base-real-percent"),
            String::from("15"),
            String::from("--remote-real-percent"),
            String::from("25"),
            String::from("--au-rate"),
            String::from("100"),
            String::from("--ru-rate"),
            String::from("90"),
            String::from("--ad-rate"),
            String::from("80"),
            String::from("--rd-rate"),
            String::from("70"),
            String::from("--log-dir"),
            String::from("tmp/logs"),
            String::from("--no-log"),
        ])
        .expect("should parse");

        assert_eq!(options.ports.len(), 2);
        assert_eq!(options.ports[0].id, "base");
        assert_eq!(options.ports[0].baud, Some(921_600));
        assert_eq!(options.ports[1].id, "remote");
        assert_eq!(options.ports[1].baud, Some(115_200));
        assert_eq!(options.mode.as_deref(), Some("ping-pong"));
        assert_eq!(options.poll_rate_hz, Some(100));
        assert_eq!(options.base_real_percent, Some(15));
        assert_eq!(options.remote_real_percent, Some(25));
        assert_eq!(options.au_rate_hz, Some(100));
        assert_eq!(options.ru_rate_hz, Some(90));
        assert_eq!(options.ad_rate_hz, Some(80));
        assert_eq!(options.rd_rate_hz, Some(70));
        assert_eq!(options.log_dir, Some(std::path::PathBuf::from("tmp/logs")));
        assert!(options.no_log);
    }

    #[test]
    fn parse_xbee_test_args_accepts_labeled_ports_without_baud() {
        let options = parse_xbee_test_args(vec![
            String::from("--port"),
            String::from("base=/dev/ttyUSB0"),
            String::from("--port"),
            String::from("remote=/dev/ttyUSB1"),
        ])
        .expect("should parse");

        assert_eq!(options.ports.len(), 2);
        assert_eq!(options.ports[0].id, "base");
        assert_eq!(options.ports[0].port, "/dev/ttyUSB0");
        assert_eq!(options.ports[0].baud, None);
        assert_eq!(options.ports[1].id, "remote");
        assert_eq!(options.ports[1].port, "/dev/ttyUSB1");
        assert_eq!(options.ports[1].baud, None);
    }

    #[test]
    fn parse_xbee_test_args_accepts_config_aliases() {
        let options = parse_xbee_test_args(vec![
            String::from("--port"),
            String::from("base=/dev/ttyUSB0@921600"),
            String::from("--port"),
            String::from("remote=/dev/ttyUSB1@115200"),
            String::from("--config"),
            String::from(
                "MODE=ping-pong,POLL_RATE=100,BASE_REAL_PERCENT=15,REMOTE_REAL_PERCENT=25,AU_RATE=100,RU_RATE=90,AD_RATE=80,RD_RATE=70,LOG_DIR=tmp/logs",
            ),
            String::from("--no-log"),
        ])
        .expect("should parse");

        assert_eq!(options.mode.as_deref(), Some("ping-pong"));
        assert_eq!(options.poll_rate_hz, Some(100));
        assert_eq!(options.base_real_percent, Some(15));
        assert_eq!(options.remote_real_percent, Some(25));
        assert_eq!(options.au_rate_hz, Some(100));
        assert_eq!(options.ru_rate_hz, Some(90));
        assert_eq!(options.ad_rate_hz, Some(80));
        assert_eq!(options.rd_rate_hz, Some(70));
        assert_eq!(options.log_dir, Some(std::path::PathBuf::from("tmp/logs")));
        assert!(options.no_log);
    }

    #[test]
    fn parse_xbee_test_port_binding_rejects_display_override() {
        let error = parse_xbee_test_port_binding("base=/dev/ttyUSB0@921600,hex")
            .expect_err("should reject");
        assert!(error.contains("display"));
    }

    #[test]
    fn xbee_test_mode_parse_accepts_aliases() {
        assert_eq!(
            XbeeTestMode::parse("flood").expect("mode"),
            XbeeTestMode::Flood
        );
        assert_eq!(
            XbeeTestMode::parse("ping-pong").expect("mode"),
            XbeeTestMode::PingPong
        );
        assert_eq!(
            XbeeTestMode::parse("pingpong").expect("mode"),
            XbeeTestMode::PingPong
        );
        assert_eq!(
            XbeeTestMode::parse("polling").expect("mode"),
            XbeeTestMode::Polling
        );
        assert_eq!(
            XbeeTestMode::parse("poll").expect("mode"),
            XbeeTestMode::Polling
        );
    }

    #[test]
    fn packet_stream_decoder_resyncs_after_invalid_crc_for_jf() {
        let mut generator = OutputFormat::PacketJfV1
            .create_dummy_generator()
            .expect("generator");
        let payload = generator.next_payload().expect("payload");
        let mut corrupted = payload.clone();
        corrupted[15] ^= 0x01;
        let mut decoder =
            PacketStreamDecoder::new(XbeeTestFrameKind::Format(OutputFormat::PacketJfV1));
        let mut input = corrupted;
        input.extend_from_slice(&payload);
        let batch = decoder.push(&input);

        assert_eq!(batch.invalid_packets, 1);
        assert_eq!(batch.packets, vec![payload]);
    }

    #[test]
    fn rover_down_decoder_accepts_variable_length_0x3xx_and_0x4xx_lines() {
        let mut decoder =
            PacketStreamDecoder::new(XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral));

        assert!(matches_rover_down_packet(b"0x300,OK\r\n"));
        assert!(matches_rover_down_packet(b"0x4A2,TEMP=21.10;MODE=A\r\n"));
        assert!(!matches_rover_down_packet(b"400,21.10\r\n"));
        assert!(decoder.push(b"noise0x4A2,TEMP=21.10").packets.is_empty());

        let batch = decoder.push(b"\r\n0x300,OK\r\n");

        assert_eq!(
            batch.packets,
            vec![b"0x4A2,TEMP=21.10\r\n".to_vec(), b"0x300,OK\r\n".to_vec()]
        );
    }

    #[test]
    fn expected_packet_tracker_counts_missing_packets_when_stream_skips_ahead() {
        let mut tracker = ExpectedPacketTracker::new();
        tracker.expect(vec![1]);
        tracker.expect(vec![2]);
        tracker.expect(vec![3]);

        tracker.observe(vec![3]);
        let stats = tracker.stats();

        assert_eq!(stats.matched_packets, 1);
        assert_eq!(stats.missing_packets, 2);
        assert_eq!(tracker.pending_packets(), 0);
    }

    #[test]
    fn expected_packet_tracker_can_count_passive_valid_packets() {
        let mut tracker = ExpectedPacketTracker::new();

        tracker.observe_untracked_valid_packet();
        tracker.observe_untracked_valid_packet();
        tracker.record_invalid_packets(1);
        let stats = tracker.stats();

        assert_eq!(stats.matched_packets, 2);
        assert_eq!(stats.invalid_packets, 1);
        assert_eq!(stats.missing_packets, 0);
        assert_eq!(stats.unexpected_packets, 0);
        assert_eq!(tracker.pending_packets(), 0);
    }

    #[test]
    fn packet_is_valid_checks_crc() {
        let mut generator = OutputFormat::PacketAcV6
            .create_dummy_generator()
            .expect("generator");
        let mut payload = generator.next_payload().expect("payload");
        let definition = PacketDefinition::for_format(OutputFormat::PacketAcV6);

        assert!(super::packet_is_valid(&payload, definition));
        payload[2] ^= 0x01;
        assert!(!super::packet_is_valid(&payload, definition));
    }

    #[test]
    fn reduced_ac_packet_matcher_checks_crc() {
        let mut payload = OutputFormat::PacketMv1
            .encode_dummy_payload()
            .expect("payload");

        assert!(matches_reduced_ac_packet(
            &payload,
            b'M',
            OutputFormat::PacketMv1.packet_len()
        ));

        payload[2] ^= 0x01;
        assert!(!matches_reduced_ac_packet(
            &payload,
            b'M',
            OutputFormat::PacketMv1.packet_len()
        ));
    }

    #[test]
    fn poll_frame_uses_crc16_ccitt_false() {
        let payload = build_poll_frame(b"HI", 7);
        assert!(matches_poll_frame(&payload, b"HI"));

        let mut corrupted = payload;
        corrupted[4] ^= 0x01;
        assert!(!matches_poll_frame(&corrupted, b"HI"));
    }
}
