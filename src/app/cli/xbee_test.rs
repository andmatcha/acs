use super::common::{
    default_baud_rate, default_log_dir, next_value, parse_port_spec, parse_u32_arg,
};
use super::help::{is_help_flag, print_xbee_mock_help, print_xbee_test_help};
use super::signal;
use crate::output::OutputFormat;
use crate::output::formats::{DummyPayloadGenerator, crc16_ccitt_false};
use crate::session::event::IngressFrame;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use crate::{port_display::LineBreakMode, port_display::PortDisplayMode, serial};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BASE_PORT_ID: &str = "base";
const REMOTE_PORT_ID: &str = "remote";
const BASE_AU_OUTPUT_ID: &str = "base-au";
const BASE_RU_OUTPUT_ID: &str = "base-ru";
const BASE_POLL_OUTPUT_ID: &str = "base-poll";
const REMOTE_AD_OUTPUT_ID: &str = "remote-ad";
const REMOTE_RD_OUTPUT_ID: &str = "remote-rd";
const REMOTE_POLL_OUTPUT_ID: &str = "remote-poll";
const MODEL_BASE_AU_OUTPUT_ID: &str = "model-base-au";
const MODEL_BASE_RU_OUTPUT_ID: &str = "model-base-ru";
const MODEL_BASE_POLL_OUTPUT_ID: &str = "model-base-poll";
const MODEL_REMOTE_AD_OUTPUT_ID: &str = "model-remote-ad";
const MODEL_REMOTE_RD_OUTPUT_ID: &str = "model-remote-rd";
const MODEL_REMOTE_POLL_OUTPUT_ID: &str = "model-remote-poll";
const DEFAULT_RATE_HZ: u32 = 100;
const DEFAULT_REAL_PERCENT: u32 = 0;
const LOOP_INTERVAL: Duration = Duration::from_millis(1);
const STATUS_INTERVAL: Duration = Duration::from_millis(200);
const EXPECTED_QUEUE_LIMIT: usize = 8_192;
const DISPLAY_FLUSH_SLICE: usize = 16;
const DISPLAY_FLUSH_PACKET_BUDGET: usize = 256;
const DISPLAY_QUEUE_LIMIT: usize = 65_536;
const SPACE_HINT: &str = "Space で表示を一時停止/再開  Ctrl-C で終了";
const POLL_FRAME_PACKET_LEN: usize = 5;
const POLL_FRAME_PAYLOAD_LEN: usize = 3;
const POLL_GREETING_FORMAT_NAME: &str = "PollGreeting";
const POLL_RESPONSE_FORMAT_NAME: &str = "PollResponse";
const POLL_GREETING_HEADER: [u8; 2] = *b"HI";
const POLL_RESPONSE_HEADER: [u8; 2] = *b"OK";

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XbeeTestPortBinding {
    id: String,
    port: String,
    baud: Option<u32>,
}

#[derive(Debug, Clone)]
struct ResolvedXbeeTestPort {
    port: String,
    baud_rate: u32,
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

#[derive(Debug, Default)]
struct XbeeMockCliOptions {
    role: Option<String>,
    ports: Vec<XbeeMockPortBinding>,
    pair_number: Option<u32>,
    tx_formats: Option<Vec<XbeeMockTxFormat>>,
    rx_formats: Option<Vec<XbeeTestFrameKind>>,
    traffic_pattern: Option<String>,
    log_dir: Option<PathBuf>,
    no_log: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct XbeeMockSettings {
    role: XbeeMockRole,
    pair_number: u32,
    uplink_port: ResolvedXbeeTestPort,
    downlink_port: ResolvedXbeeTestPort,
    tx_formats: Vec<XbeeMockTxFormat>,
    rx_formats: Vec<XbeeTestFrameKind>,
    traffic_pattern: XbeeTestMode,
    port: ResolvedXbeeTestPort,
    monitor_ports: Vec<ResolvedXbeeTestPort>,
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
}

struct XbeeMockRunResult {
    role: XbeeMockRole,
    pair_number: u32,
    uplink_port: ResolvedXbeeTestPort,
    downlink_port: ResolvedXbeeTestPort,
    traffic_pattern: XbeeTestMode,
    tx_formats: Vec<XbeeMockTxRunResult>,
    rx_formats: Vec<XbeeMockRxRunResult>,
    logging_enabled: bool,
    log_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XbeeMockPortId {
    Up,
    Down,
}

impl XbeeMockPortId {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            other => Err(format!(
                "unsupported xbee-mock port label: {other} (expected `up` or `down`)"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XbeeMockPortBinding {
    id: Option<XbeeMockPortId>,
    port: String,
    baud: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XbeeMockResolvedPortBinding {
    port: String,
    baud: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct XbeeMockTxFormat {
    kind: XbeeTestFrameKind,
    rate_hz: u32,
}

struct XbeeMockTxRunResult {
    kind: XbeeTestFrameKind,
    rate_hz: u32,
    sent_packets: u64,
    write_errors: u64,
}

struct XbeeMockRxRunResult {
    kind: XbeeTestFrameKind,
    track_expected_packets: bool,
    target_rate_hz: f64,
    stats: PacketMatchStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XbeeMockRole {
    Base,
    Remote,
}

impl XbeeMockRole {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            BASE_PORT_ID => Ok(Self::Base),
            REMOTE_PORT_ID => Ok(Self::Remote),
            other => Err(format!(
                "unsupported xbee-mock role: {other} (expected `base` or `remote`)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Base => BASE_PORT_ID,
            Self::Remote => REMOTE_PORT_ID,
        }
    }
}

impl XbeeMockSettings {
    fn tx_port(&self) -> &ResolvedXbeeTestPort {
        match self.role {
            XbeeMockRole::Base => &self.uplink_port,
            XbeeMockRole::Remote => &self.downlink_port,
        }
    }

    fn rx_port(&self) -> &ResolvedXbeeTestPort {
        match self.role {
            XbeeMockRole::Base => &self.downlink_port,
            XbeeMockRole::Remote => &self.uplink_port,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XbeeTestMode {
    Flood,
    PingPong,
    Polling,
}

impl XbeeTestMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "flood" => Ok(Self::Flood),
            "ping-pong" | "pingpong" => Ok(Self::PingPong),
            "polling" | "poll" => Ok(Self::Polling),
            other => Err(format!(
                "unsupported xbee-test mode: {other} (expected `flood`, `ping-pong`, or `polling`)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Flood => "flood",
            Self::PingPong => "ping-pong",
            Self::Polling => "polling",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum XbeeTestFrameKind {
    Format(OutputFormat),
    PollGreeting,
    PollResponse,
}

impl XbeeTestFrameKind {
    fn display_name(self) -> &'static str {
        match self {
            Self::Format(format) => format.display_name(),
            Self::PollGreeting => POLL_GREETING_FORMAT_NAME,
            Self::PollResponse => POLL_RESPONSE_FORMAT_NAME,
        }
    }

    fn session_format_name(self) -> &'static str {
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
            OutputFormat::PacketJfV1 => Self {
                packet_len: 16,
                payload_len: 14,
                header: *b"JF",
            },
            OutputFormat::RoverUpGeneral | OutputFormat::RoverDownGeneral => {
                panic!(
                    "xbee-test does not support output format `{}`",
                    format.as_str()
                )
            }
        }
    }
}

fn xbee_test_format_label(kind: XbeeTestFrameKind) -> &'static str {
    match kind {
        XbeeTestFrameKind::Format(OutputFormat::PacketAcV6) => "AU(PacketACv6)",
        XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral) => "RU(RoverUpGeneral)",
        XbeeTestFrameKind::Format(OutputFormat::PacketJfV1) => "AD(PacketJFv1)",
        XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral) => "RD(RoverDownGeneral)",
        XbeeTestFrameKind::PollGreeting => "PollGreeting",
        XbeeTestFrameKind::PollResponse => "PollResponse",
    }
}

#[derive(Debug, Default, Clone)]
struct PacketMatchStats {
    matched_packets: u64,
    missing_packets: u64,
    invalid_packets: u64,
    unexpected_packets: u64,
    queue_overflow_packets: u64,
}

impl PacketMatchStats {
    fn total_errors(&self) -> u64 {
        self.missing_packets
            .saturating_add(self.invalid_packets)
            .saturating_add(self.unexpected_packets)
            .saturating_add(self.queue_overflow_packets)
    }

    fn error_rate_percent(&self) -> f64 {
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
        | XbeeTestFrameKind::Format(OutputFormat::PacketJfV1)
        | XbeeTestFrameKind::PollGreeting
        | XbeeTestFrameKind::PollResponse => 2,
        XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral) => 6,
        XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral) => 4,
    }
}

fn find_packet_start(buffer: &[u8], kind: XbeeTestFrameKind) -> Option<usize> {
    match kind {
        XbeeTestFrameKind::Format(OutputFormat::PacketAcV6) => find_header(buffer, b"AC"),
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
        | XbeeTestFrameKind::Format(OutputFormat::PacketJfV1) => {
            let XbeeTestFrameKind::Format(format) = kind else {
                unreachable!()
            };
            packet_is_valid(packet, PacketDefinition::for_format(format))
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
    buffer.windows(4).position(|window| {
        window[0] == b'4'
            && window[1].is_ascii_hexdigit()
            && window[2].is_ascii_hexdigit()
            && window[3] == b','
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

struct ObservedInput {
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
struct ObservedInputKindSpec {
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
}

impl ObservedInput {
    fn new(
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

    fn observe(&mut self, bytes: &[u8], now: Instant) -> ObservedInputBatch {
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

    fn flush_display_batch(
        &mut self,
        session: &mut SessionRuntime,
        max_packets: usize,
    ) -> Result<usize, String> {
        self.display_queue
            .flush_input_batch(session, &self.input_port, max_packets)
    }

    fn status_lines(&self, target_rates_hz: &BTreeMap<XbeeTestFrameKind, f64>) -> Vec<String> {
        self.format_states
            .iter()
            .map(|state| state.status_line(self.input_id, target_rates_hz))
            .collect()
    }

    fn stats(&self, kind: XbeeTestFrameKind) -> PacketMatchStats {
        self.format_states
            .iter()
            .find(|state| state.kind == kind)
            .map(|state| state.tracker.stats())
            .unwrap_or_default()
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
struct ObservedInputBatch {
    valid_byte_len: usize,
    valid_packet_count: usize,
    per_kind_totals: BTreeMap<XbeeTestFrameKind, (usize, usize)>,
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

struct ScheduledSender {
    sender: GeneratedSender,
    schedule: SendSchedule,
}

impl ScheduledSender {
    fn new(
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

    fn send_due_packets(
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

    fn send_once(
        &mut self,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) -> bool {
        self.sender.send_once(session, input, output)
    }

    fn queue_expected_due_packets(&mut self, now: Instant, input: &mut ObservedInput) -> usize {
        let mut generated_packets = 0usize;
        for _ in 0..self.schedule.take_due_count(now) {
            if self.sender.queue_expected_packet(input) {
                generated_packets += 1;
            }
        }
        generated_packets
    }

    fn queue_expected_once(&mut self, input: &mut ObservedInput) -> bool {
        self.sender.queue_expected_packet(input)
    }

    fn status_line(&self) -> String {
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

    fn target_rate_hz(&self) -> u32 {
        self.schedule.target_rate_hz
    }

    fn sent_packets(&self) -> u64 {
        self.sender.sent_packets
    }

    fn write_errors(&self) -> u64 {
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

struct DisplayedOutput {
    port: String,
    display_queue: PacketDisplayQueue,
}

impl DisplayedOutput {
    fn new(port: String) -> Self {
        Self {
            port,
            display_queue: PacketDisplayQueue::new(),
        }
    }

    fn flush_display_batch(
        &mut self,
        session: &mut SessionRuntime,
        max_packets: usize,
    ) -> Result<usize, String> {
        self.display_queue
            .flush_output_batch(session, &self.port, max_packets)
    }

    fn pending_packets(&self) -> usize {
        self.display_queue.pending_packets()
    }

    fn overflow_packets(&self) -> u64 {
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

#[allow(dead_code)]
struct BaseMockPollingState {
    poll_schedule: SendSchedule,
    poll_sender: GeneratedSender,
    expected_response_sender: GeneratedSender,
    base_real_percent: u32,
    remote_real_percent: u32,
    base_rng: SimpleRng,
    remote_rng: SimpleRng,
}

#[allow(dead_code)]
impl BaseMockPollingState {
    fn new(settings: &XbeeMockSettings, started_at: Instant) -> Result<Self, String> {
        Ok(Self {
            poll_schedule: SendSchedule::new(
                settings.poll_rate_hz,
                started_at,
                POLL_GREETING_FORMAT_NAME,
            )?,
            poll_sender: GeneratedSender::new(
                BASE_POLL_OUTPUT_ID,
                REMOTE_PORT_ID,
                XbeeTestFrameKind::PollGreeting,
            )?,
            expected_response_sender: GeneratedSender::new(
                MODEL_REMOTE_POLL_OUTPUT_ID,
                BASE_PORT_ID,
                XbeeTestFrameKind::PollResponse,
            )?,
            base_real_percent: settings.base_real_percent,
            remote_real_percent: settings.remote_real_percent,
            base_rng: SimpleRng::default(),
            remote_rng: SimpleRng::default(),
        })
    }

    fn take_due_polls(&mut self, now: Instant) -> usize {
        self.poll_schedule.take_due_count(now)
    }

    fn base_send_real(&mut self) -> bool {
        self.base_rng.percent_chance(self.base_real_percent)
    }

    fn remote_send_real(&mut self) -> bool {
        self.remote_rng.percent_chance(self.remote_real_percent)
    }
}

#[allow(dead_code)]
struct BaseMockDriver {
    au_sender: ScheduledSender,
    ru_sender: ScheduledSender,
    expected_ad: ScheduledSender,
    expected_rd: ScheduledSender,
    polling: Option<BaseMockPollingState>,
}

#[allow(dead_code)]
impl BaseMockDriver {
    fn new(settings: &XbeeMockSettings, started_at: Instant) -> Result<Self, String> {
        Ok(Self {
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
            expected_ad: ScheduledSender::new(
                MODEL_REMOTE_AD_OUTPUT_ID,
                BASE_PORT_ID,
                XbeeTestFrameKind::Format(OutputFormat::PacketJfV1),
                settings.ad_rate_hz,
                started_at,
            )?,
            expected_rd: ScheduledSender::new(
                MODEL_REMOTE_RD_OUTPUT_ID,
                BASE_PORT_ID,
                XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral),
                settings.rd_rate_hz,
                started_at,
            )?,
            polling: (settings.mode == XbeeTestMode::Polling)
                .then(|| BaseMockPollingState::new(settings, started_at))
                .transpose()?,
        })
    }

    fn handle_input(
        &mut self,
        _mode: XbeeTestMode,
        _batch: &ObservedInputBatch,
        _session: &mut SessionRuntime,
        _input: &mut ObservedInput,
        _output: &mut DisplayedOutput,
    ) {
    }

    fn on_tick(
        &mut self,
        mode: XbeeTestMode,
        now: Instant,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) {
        match mode {
            XbeeTestMode::Flood => {
                self.au_sender.send_due_packets(now, session, input, output);
                self.ru_sender.send_due_packets(now, session, input, output);
                self.expected_ad.queue_expected_due_packets(now, input);
                self.expected_rd.queue_expected_due_packets(now, input);
            }
            XbeeTestMode::PingPong => {
                let sent_au = self.au_sender.send_due_packets(now, session, input, output);
                for _ in 0..sent_au {
                    self.expected_ad.queue_expected_once(input);
                }
                let sent_ru = self.ru_sender.send_due_packets(now, session, input, output);
                for _ in 0..sent_ru {
                    self.expected_rd.queue_expected_once(input);
                }
            }
            XbeeTestMode::Polling => {
                let due_polls = self
                    .polling
                    .as_mut()
                    .map(|polling| polling.take_due_polls(now))
                    .unwrap_or(0);
                for _ in 0..due_polls {
                    self.send_poll_cycle(session, input, output);
                }
            }
        }
    }

    fn send_poll_cycle(
        &mut self,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) {
        let send_real = self
            .polling
            .as_mut()
            .map(BaseMockPollingState::base_send_real)
            .unwrap_or(false);

        let sent_cycle = if send_real {
            let sent_au = self.au_sender.send_once(session, input, output);
            let sent_ru = self.ru_sender.send_once(session, input, output);
            sent_au || sent_ru
        } else {
            self.polling
                .as_mut()
                .expect("polling state exists")
                .poll_sender
                .send_once(session, input, output)
        };

        if !sent_cycle {
            return;
        }

        let remote_send_real = self
            .polling
            .as_mut()
            .map(BaseMockPollingState::remote_send_real)
            .unwrap_or(false);
        if remote_send_real {
            self.expected_ad.queue_expected_once(input);
            self.expected_rd.queue_expected_once(input);
        } else {
            self.polling
                .as_mut()
                .expect("polling state exists")
                .expected_response_sender
                .queue_expected_packet(input);
        }
    }

    fn sender_lines(&self, mode: XbeeTestMode, poll_rate_hz: u32) -> Vec<String> {
        match mode {
            XbeeTestMode::Flood | XbeeTestMode::PingPong => {
                vec![self.au_sender.status_line(), self.ru_sender.status_line()]
            }
            XbeeTestMode::Polling => {
                let polling = self.polling.as_ref().expect("polling state exists");
                vec![
                    XbeeTestState::generated_sender_line(
                        &polling.poll_sender,
                        &format!("role=base-greeting  target={} Hz", poll_rate_hz),
                    ),
                    XbeeTestState::scheduled_sender_line(&self.au_sender, "role=base-real"),
                    XbeeTestState::scheduled_sender_line(&self.ru_sender, "role=base-real"),
                ]
            }
        }
    }

    fn target_rates(
        &self,
        mode: XbeeTestMode,
        poll_rate_hz: u32,
        _base_real_percent: u32,
        remote_real_percent: u32,
    ) -> BTreeMap<XbeeTestFrameKind, f64> {
        match mode {
            XbeeTestMode::Flood => BTreeMap::from([
                (
                    XbeeTestFrameKind::Format(OutputFormat::PacketJfV1),
                    self.expected_ad.target_rate_hz() as f64,
                ),
                (
                    XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral),
                    self.expected_rd.target_rate_hz() as f64,
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
                let real_rate = poll_rate_hz as f64 * remote_real_percent as f64 / 100.0;
                BTreeMap::from([
                    (
                        XbeeTestFrameKind::PollResponse,
                        poll_rate_hz as f64 - real_rate,
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

    fn poll_tx_sent(&self) -> u64 {
        self.polling
            .as_ref()
            .map(|polling| polling.poll_sender.sent_packets)
            .unwrap_or(0)
    }
}

#[allow(dead_code)]
struct RemoteMockPollingState {
    poll_schedule: SendSchedule,
    expected_greeting_sender: GeneratedSender,
    response_sender: GeneratedSender,
    base_real_percent: u32,
    remote_real_percent: u32,
    base_rng: SimpleRng,
    remote_rng: SimpleRng,
    observed_greetings: u64,
    observed_au: u64,
    observed_ru: u64,
    replied_greetings: u64,
    replied_real_cycles: u64,
}

#[allow(dead_code)]
impl RemoteMockPollingState {
    fn new(settings: &XbeeMockSettings, started_at: Instant) -> Result<Self, String> {
        Ok(Self {
            poll_schedule: SendSchedule::new(
                settings.poll_rate_hz,
                started_at,
                POLL_GREETING_FORMAT_NAME,
            )?,
            expected_greeting_sender: GeneratedSender::new(
                MODEL_BASE_POLL_OUTPUT_ID,
                REMOTE_PORT_ID,
                XbeeTestFrameKind::PollGreeting,
            )?,
            response_sender: GeneratedSender::new(
                REMOTE_POLL_OUTPUT_ID,
                BASE_PORT_ID,
                XbeeTestFrameKind::PollResponse,
            )?,
            base_real_percent: settings.base_real_percent,
            remote_real_percent: settings.remote_real_percent,
            base_rng: SimpleRng::default(),
            remote_rng: SimpleRng::default(),
            observed_greetings: 0,
            observed_au: 0,
            observed_ru: 0,
            replied_greetings: 0,
            replied_real_cycles: 0,
        })
    }

    fn take_due_polls(&mut self, now: Instant) -> usize {
        self.poll_schedule.take_due_count(now)
    }

    fn base_send_real(&mut self) -> bool {
        self.base_rng.percent_chance(self.base_real_percent)
    }

    fn remote_send_real(&mut self) -> bool {
        self.remote_rng.percent_chance(self.remote_real_percent)
    }

    fn observe_and_take_due_replies(
        &mut self,
        observed: &BTreeMap<XbeeTestFrameKind, (usize, usize)>,
    ) -> (usize, usize) {
        self.observed_greetings = self.observed_greetings.saturating_add(
            observed
                .get(&XbeeTestFrameKind::PollGreeting)
                .map(|(_, packet_count)| *packet_count as u64)
                .unwrap_or(0),
        );
        self.observed_au = self.observed_au.saturating_add(
            observed
                .get(&XbeeTestFrameKind::Format(OutputFormat::PacketAcV6))
                .map(|(_, packet_count)| *packet_count as u64)
                .unwrap_or(0),
        );
        self.observed_ru = self.observed_ru.saturating_add(
            observed
                .get(&XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral))
                .map(|(_, packet_count)| *packet_count as u64)
                .unwrap_or(0),
        );

        let due_greetings = self
            .observed_greetings
            .saturating_sub(self.replied_greetings);
        let due_real_cycles = self
            .observed_au
            .max(self.observed_ru)
            .saturating_sub(self.replied_real_cycles);
        (due_greetings as usize, due_real_cycles as usize)
    }

    fn note_reply(&mut self, trigger_is_greeting: bool) {
        if trigger_is_greeting {
            self.replied_greetings = self.replied_greetings.saturating_add(1);
        } else {
            self.replied_real_cycles = self.replied_real_cycles.saturating_add(1);
        }
    }
}

#[allow(dead_code)]
struct RemoteMockDriver {
    ad_sender: ScheduledSender,
    rd_sender: ScheduledSender,
    expected_au: ScheduledSender,
    expected_ru: ScheduledSender,
    polling: Option<RemoteMockPollingState>,
}

#[allow(dead_code)]
impl RemoteMockDriver {
    fn new(settings: &XbeeMockSettings, started_at: Instant) -> Result<Self, String> {
        Ok(Self {
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
            expected_au: ScheduledSender::new(
                MODEL_BASE_AU_OUTPUT_ID,
                REMOTE_PORT_ID,
                XbeeTestFrameKind::Format(OutputFormat::PacketAcV6),
                settings.au_rate_hz,
                started_at,
            )?,
            expected_ru: ScheduledSender::new(
                MODEL_BASE_RU_OUTPUT_ID,
                REMOTE_PORT_ID,
                XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral),
                settings.ru_rate_hz,
                started_at,
            )?,
            polling: (settings.mode == XbeeTestMode::Polling)
                .then(|| RemoteMockPollingState::new(settings, started_at))
                .transpose()?,
        })
    }

    fn handle_input(
        &mut self,
        mode: XbeeTestMode,
        batch: &ObservedInputBatch,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) {
        match mode {
            XbeeTestMode::Flood => {}
            XbeeTestMode::PingPong => {
                let au_count = batch
                    .per_kind_totals
                    .get(&XbeeTestFrameKind::Format(OutputFormat::PacketAcV6))
                    .map(|(_, packet_count)| *packet_count)
                    .unwrap_or(0);
                for _ in 0..au_count {
                    self.ad_sender.send_once(session, input, output);
                }
                let ru_count = batch
                    .per_kind_totals
                    .get(&XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral))
                    .map(|(_, packet_count)| *packet_count)
                    .unwrap_or(0);
                for _ in 0..ru_count {
                    self.rd_sender.send_once(session, input, output);
                }
            }
            XbeeTestMode::Polling => {
                let (due_greetings, due_real_cycles) = {
                    let polling = self.polling.as_mut().expect("polling state exists");
                    polling.observe_and_take_due_replies(&batch.per_kind_totals)
                };
                for _ in 0..due_greetings {
                    self.send_polling_reply(session, input, output, true);
                }
                for _ in 0..due_real_cycles {
                    self.send_polling_reply(session, input, output, false);
                }
            }
        }
    }

    fn on_tick(
        &mut self,
        mode: XbeeTestMode,
        now: Instant,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) {
        match mode {
            XbeeTestMode::Flood => {
                self.ad_sender.send_due_packets(now, session, input, output);
                self.rd_sender.send_due_packets(now, session, input, output);
                self.expected_au.queue_expected_due_packets(now, input);
                self.expected_ru.queue_expected_due_packets(now, input);
            }
            XbeeTestMode::PingPong => {
                self.expected_au.queue_expected_due_packets(now, input);
                self.expected_ru.queue_expected_due_packets(now, input);
            }
            XbeeTestMode::Polling => {
                let due_polls = self
                    .polling
                    .as_mut()
                    .map(|polling| polling.take_due_polls(now))
                    .unwrap_or(0);
                for _ in 0..due_polls {
                    self.queue_expected_base_poll_cycle(input);
                }
            }
        }
    }

    fn queue_expected_base_poll_cycle(&mut self, input: &mut ObservedInput) {
        let send_real = self
            .polling
            .as_mut()
            .map(RemoteMockPollingState::base_send_real)
            .unwrap_or(false);

        if send_real {
            self.expected_au.queue_expected_once(input);
            self.expected_ru.queue_expected_once(input);
        } else {
            self.polling
                .as_mut()
                .expect("polling state exists")
                .expected_greeting_sender
                .queue_expected_packet(input);
        }
    }

    fn send_polling_reply(
        &mut self,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
        trigger_is_greeting: bool,
    ) {
        let send_real = self
            .polling
            .as_mut()
            .map(RemoteMockPollingState::remote_send_real)
            .unwrap_or(false);

        let sent = if send_real {
            let sent_ad = self.ad_sender.send_once(session, input, output);
            let sent_rd = self.rd_sender.send_once(session, input, output);
            sent_ad || sent_rd
        } else {
            self.polling
                .as_mut()
                .expect("polling state exists")
                .response_sender
                .send_once(session, input, output)
        };

        if sent {
            self.polling
                .as_mut()
                .expect("polling state exists")
                .note_reply(trigger_is_greeting);
        }
    }

    fn sender_lines(&self, mode: XbeeTestMode) -> Vec<String> {
        match mode {
            XbeeTestMode::Flood => vec![self.ad_sender.status_line(), self.rd_sender.status_line()],
            XbeeTestMode::PingPong => vec![
                XbeeTestState::scheduled_sender_line(&self.ad_sender, "trigger=au-rx"),
                XbeeTestState::scheduled_sender_line(&self.rd_sender, "trigger=ru-rx"),
            ],
            XbeeTestMode::Polling => {
                let polling = self.polling.as_ref().expect("polling state exists");
                vec![
                    XbeeTestState::generated_sender_line(
                        &polling.response_sender,
                        "role=remote-response",
                    ),
                    XbeeTestState::scheduled_sender_line(&self.ad_sender, "role=remote-real"),
                    XbeeTestState::scheduled_sender_line(&self.rd_sender, "role=remote-real"),
                ]
            }
        }
    }

    fn target_rates(
        &self,
        mode: XbeeTestMode,
        poll_rate_hz: u32,
        base_real_percent: u32,
        _remote_real_percent: u32,
    ) -> BTreeMap<XbeeTestFrameKind, f64> {
        match mode {
            XbeeTestMode::Flood | XbeeTestMode::PingPong => BTreeMap::from([
                (
                    XbeeTestFrameKind::Format(OutputFormat::PacketAcV6),
                    self.expected_au.target_rate_hz() as f64,
                ),
                (
                    XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral),
                    self.expected_ru.target_rate_hz() as f64,
                ),
            ]),
            XbeeTestMode::Polling => {
                let real_rate = poll_rate_hz as f64 * base_real_percent as f64 / 100.0;
                BTreeMap::from([
                    (
                        XbeeTestFrameKind::PollGreeting,
                        poll_rate_hz as f64 - real_rate,
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

    fn poll_tx_sent(&self) -> u64 {
        self.polling
            .as_ref()
            .map(|polling| polling.response_sender.sent_packets)
            .unwrap_or(0)
    }
}

#[derive(Clone, Copy)]
struct XbeeMockLaneTemplate {
    tx_kind: XbeeTestFrameKind,
    rx_kind: XbeeTestFrameKind,
    tx_output_id: &'static str,
    expected_output_id: &'static str,
    tx_target_input_id: &'static str,
    expected_target_input_id: &'static str,
}

const BASE_MOCK_LANE_TEMPLATES: &[XbeeMockLaneTemplate] = &[
    XbeeMockLaneTemplate {
        tx_kind: XbeeTestFrameKind::Format(OutputFormat::PacketAcV6),
        rx_kind: XbeeTestFrameKind::Format(OutputFormat::PacketJfV1),
        tx_output_id: BASE_AU_OUTPUT_ID,
        expected_output_id: MODEL_REMOTE_AD_OUTPUT_ID,
        tx_target_input_id: REMOTE_PORT_ID,
        expected_target_input_id: BASE_PORT_ID,
    },
    XbeeMockLaneTemplate {
        tx_kind: XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral),
        rx_kind: XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral),
        tx_output_id: BASE_RU_OUTPUT_ID,
        expected_output_id: MODEL_REMOTE_RD_OUTPUT_ID,
        tx_target_input_id: REMOTE_PORT_ID,
        expected_target_input_id: BASE_PORT_ID,
    },
    XbeeMockLaneTemplate {
        tx_kind: XbeeTestFrameKind::PollGreeting,
        rx_kind: XbeeTestFrameKind::PollResponse,
        tx_output_id: BASE_POLL_OUTPUT_ID,
        expected_output_id: MODEL_REMOTE_POLL_OUTPUT_ID,
        tx_target_input_id: REMOTE_PORT_ID,
        expected_target_input_id: BASE_PORT_ID,
    },
];

const REMOTE_MOCK_LANE_TEMPLATES: &[XbeeMockLaneTemplate] = &[
    XbeeMockLaneTemplate {
        tx_kind: XbeeTestFrameKind::Format(OutputFormat::PacketJfV1),
        rx_kind: XbeeTestFrameKind::Format(OutputFormat::PacketAcV6),
        tx_output_id: REMOTE_AD_OUTPUT_ID,
        expected_output_id: MODEL_BASE_AU_OUTPUT_ID,
        tx_target_input_id: BASE_PORT_ID,
        expected_target_input_id: REMOTE_PORT_ID,
    },
    XbeeMockLaneTemplate {
        tx_kind: XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral),
        rx_kind: XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral),
        tx_output_id: REMOTE_RD_OUTPUT_ID,
        expected_output_id: MODEL_BASE_RU_OUTPUT_ID,
        tx_target_input_id: BASE_PORT_ID,
        expected_target_input_id: REMOTE_PORT_ID,
    },
    XbeeMockLaneTemplate {
        tx_kind: XbeeTestFrameKind::PollResponse,
        rx_kind: XbeeTestFrameKind::PollGreeting,
        tx_output_id: REMOTE_POLL_OUTPUT_ID,
        expected_output_id: MODEL_BASE_POLL_OUTPUT_ID,
        tx_target_input_id: BASE_PORT_ID,
        expected_target_input_id: REMOTE_PORT_ID,
    },
];

fn xbee_mock_lane_templates(role: XbeeMockRole) -> &'static [XbeeMockLaneTemplate] {
    match role {
        XbeeMockRole::Base => BASE_MOCK_LANE_TEMPLATES,
        XbeeMockRole::Remote => REMOTE_MOCK_LANE_TEMPLATES,
    }
}

fn xbee_mock_input_source_id(role: XbeeMockRole) -> &'static str {
    match role {
        XbeeMockRole::Base => REMOTE_PORT_ID,
        XbeeMockRole::Remote => BASE_PORT_ID,
    }
}

struct XbeeMockLane {
    tx_kind: XbeeTestFrameKind,
    rx_kind: XbeeTestFrameKind,
    tx_sender: Option<ScheduledSender>,
    expected_rx_sender: Option<ScheduledSender>,
}

struct GenericXbeeMockDriver {
    role: XbeeMockRole,
    lanes: Vec<XbeeMockLane>,
}

impl GenericXbeeMockDriver {
    fn new(settings: &XbeeMockSettings, started_at: Instant) -> Result<Self, String> {
        let tx_rate_by_kind = settings
            .tx_formats
            .iter()
            .map(|format| (format.kind, format.rate_hz))
            .collect::<BTreeMap<_, _>>();
        let selected_rx_kinds = settings.rx_formats.iter().copied().collect::<Vec<_>>();
        let mut lanes = Vec::new();

        for template in xbee_mock_lane_templates(settings.role) {
            let tx_sender = tx_rate_by_kind
                .get(&template.tx_kind)
                .copied()
                .map(|rate_hz| {
                    ScheduledSender::new(
                        template.tx_output_id,
                        template.tx_target_input_id,
                        template.tx_kind,
                        rate_hz,
                        started_at,
                    )
                })
                .transpose()?;
            let expected_rx_sender = if selected_rx_kinds.contains(&template.rx_kind) {
                tx_rate_by_kind
                    .get(&template.tx_kind)
                    .copied()
                    .map(|rate_hz| {
                        ScheduledSender::new(
                            template.expected_output_id,
                            template.expected_target_input_id,
                            template.rx_kind,
                            rate_hz,
                            started_at,
                        )
                    })
                    .transpose()?
            } else {
                None
            };
            lanes.push(XbeeMockLane {
                tx_kind: template.tx_kind,
                rx_kind: template.rx_kind,
                tx_sender,
                expected_rx_sender,
            });
        }

        Ok(Self {
            role: settings.role,
            lanes,
        })
    }

    fn handle_input(
        &mut self,
        pattern: XbeeTestMode,
        batch: &ObservedInputBatch,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) {
        if self.role != XbeeMockRole::Remote || pattern == XbeeTestMode::Flood {
            return;
        }

        for lane in &mut self.lanes {
            let Some(tx_sender) = lane.tx_sender.as_mut() else {
                continue;
            };
            let packet_count = batch
                .per_kind_totals
                .get(&lane.rx_kind)
                .map(|(_, packet_count)| *packet_count)
                .unwrap_or(0);
            for _ in 0..packet_count {
                tx_sender.send_once(session, input, output);
            }
        }
    }

    fn on_tick(
        &mut self,
        pattern: XbeeTestMode,
        now: Instant,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) {
        match self.role {
            XbeeMockRole::Base => self.on_tick_base(pattern, now, session, input, output),
            XbeeMockRole::Remote => self.on_tick_remote(pattern, now, session, input, output),
        }
    }

    fn on_tick_base(
        &mut self,
        pattern: XbeeTestMode,
        now: Instant,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) {
        for lane in &mut self.lanes {
            match pattern {
                XbeeTestMode::Flood => {
                    if let Some(sender) = lane.tx_sender.as_mut() {
                        sender.send_due_packets(now, session, input, output);
                    }
                    if let Some(sender) = lane.expected_rx_sender.as_mut() {
                        sender.queue_expected_due_packets(now, input);
                    }
                }
                XbeeTestMode::PingPong | XbeeTestMode::Polling => {
                    let sent_packets = lane
                        .tx_sender
                        .as_mut()
                        .map(|sender| sender.send_due_packets(now, session, input, output))
                        .unwrap_or(0);
                    if let Some(sender) = lane.expected_rx_sender.as_mut() {
                        for _ in 0..sent_packets {
                            sender.queue_expected_once(input);
                        }
                    }
                }
            }
        }
    }

    fn on_tick_remote(
        &mut self,
        pattern: XbeeTestMode,
        now: Instant,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) {
        for lane in &mut self.lanes {
            match pattern {
                XbeeTestMode::Flood => {
                    if let Some(sender) = lane.tx_sender.as_mut() {
                        sender.send_due_packets(now, session, input, output);
                    }
                    if let Some(sender) = lane.expected_rx_sender.as_mut() {
                        sender.queue_expected_due_packets(now, input);
                    }
                }
                XbeeTestMode::PingPong | XbeeTestMode::Polling => {
                    if let Some(sender) = lane.expected_rx_sender.as_mut() {
                        sender.queue_expected_due_packets(now, input);
                    }
                }
            }
        }
    }

    fn sender_lines(&self) -> Vec<String> {
        self.lanes
            .iter()
            .filter_map(|lane| lane.tx_sender.as_ref().map(ScheduledSender::status_line))
            .collect()
    }

    fn target_rates(&self) -> BTreeMap<XbeeTestFrameKind, f64> {
        self.lanes
            .iter()
            .filter_map(|lane| {
                lane.expected_rx_sender
                    .as_ref()
                    .map(|sender| (lane.rx_kind, sender.target_rate_hz() as f64))
            })
            .collect()
    }

    fn rx_format_specs(
        &self,
        selected_rx_kinds: &[XbeeTestFrameKind],
    ) -> Vec<(XbeeTestFrameKind, bool)> {
        selected_rx_kinds
            .iter()
            .copied()
            .map(|kind| {
                let track_expected_packets = self
                    .lanes
                    .iter()
                    .find(|lane| lane.rx_kind == kind)
                    .and_then(|lane| lane.expected_rx_sender.as_ref())
                    .is_some();
                (kind, track_expected_packets)
            })
            .collect()
    }

    fn tx_results(&self) -> Vec<XbeeMockTxRunResult> {
        self.lanes
            .iter()
            .filter_map(|lane| {
                lane.tx_sender.as_ref().map(|sender| XbeeMockTxRunResult {
                    kind: lane.tx_kind,
                    rate_hz: sender.target_rate_hz(),
                    sent_packets: sender.sent_packets(),
                    write_errors: sender.write_errors(),
                })
            })
            .collect()
    }
}

#[allow(dead_code)]
enum XbeeMockDriver {
    Base(BaseMockDriver),
    Remote(RemoteMockDriver),
}

struct XbeeMockState {
    role: XbeeMockRole,
    pair_number: u32,
    uplink_port: ResolvedXbeeTestPort,
    downlink_port: ResolvedXbeeTestPort,
    tx_port: ResolvedXbeeTestPort,
    rx_port: ResolvedXbeeTestPort,
    traffic_pattern: XbeeTestMode,
    tx_formats: Vec<XbeeMockTxFormat>,
    rx_formats: Vec<(XbeeTestFrameKind, bool)>,
    log_path_display: String,
    logging_enabled: bool,
    display_fps: f64,
    output: DisplayedOutput,
    input: ObservedInput,
    driver: GenericXbeeMockDriver,
    last_status_update: Instant,
    header_lines: Vec<String>,
}

impl XbeeMockState {
    fn new(
        settings: &XbeeMockSettings,
        log_path_display: String,
        started_at: Instant,
    ) -> Result<Self, String> {
        let driver = GenericXbeeMockDriver::new(settings, started_at)?;
        let rx_formats = driver.rx_format_specs(&settings.rx_formats);
        let input_kinds = rx_formats
            .iter()
            .map(|(kind, track_expected_packets)| ObservedInputKindSpec {
                kind: *kind,
                track_expected_packets: *track_expected_packets,
            })
            .collect::<Vec<_>>();

        Ok(Self {
            role: settings.role,
            pair_number: settings.pair_number,
            uplink_port: settings.uplink_port.clone(),
            downlink_port: settings.downlink_port.clone(),
            tx_port: settings.tx_port().clone(),
            rx_port: settings.rx_port().clone(),
            traffic_pattern: settings.traffic_pattern,
            tx_formats: settings.tx_formats.clone(),
            rx_formats,
            log_path_display,
            logging_enabled: settings.logging_enabled,
            display_fps: 0.0,
            output: DisplayedOutput::new(settings.tx_port().port.clone()),
            input: ObservedInput::new(
                settings.role.as_str(),
                settings.rx_port().port.clone(),
                xbee_mock_input_source_id(settings.role),
                input_kinds,
            ),
            driver,
            last_status_update: started_at
                .checked_sub(STATUS_INTERVAL)
                .unwrap_or(started_at),
            header_lines: Vec::new(),
        })
    }

    fn handle_input(&mut self, frame: &IngressFrame, session: &mut SessionRuntime) {
        if frame.input_id != self.role.as_str() {
            return;
        }

        let now = Instant::now();
        let batch = self.input.observe(&frame.bytes, now);
        if batch.valid_packet_count > 0 {
            XbeeTestState::record_observed_input_batch(session, &self.input.input_port, &batch);
        }

        self.driver.handle_input(
            self.traffic_pattern,
            &batch,
            session,
            &mut self.input,
            &mut self.output,
        );
    }

    fn on_tick(&mut self, session: &mut SessionRuntime) -> Result<(), String> {
        let now = Instant::now();
        self.driver.on_tick(
            self.traffic_pattern,
            now,
            session,
            &mut self.input,
            &mut self.output,
        );

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

    fn flush_display_queues(&mut self, session: &mut SessionRuntime) -> Result<(), String> {
        let mut remaining_budget = DISPLAY_FLUSH_PACKET_BUDGET;

        while remaining_budget > 0 {
            let mut flushed_total = 0usize;

            let slice = remaining_budget.min(DISPLAY_FLUSH_SLICE);
            let flushed = self.output.flush_display_batch(session, slice)?;
            remaining_budget = remaining_budget.saturating_sub(flushed);
            flushed_total += flushed;
            if remaining_budget == 0 {
                break;
            }

            let slice = remaining_budget.min(DISPLAY_FLUSH_SLICE);
            let flushed = self.input.flush_display_batch(session, slice)?;
            remaining_budget = remaining_budget.saturating_sub(flushed);
            flushed_total += flushed;

            if flushed_total == 0 {
                break;
            }
        }

        Ok(())
    }

    fn build_header_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "role={}  pair={}  pattern={}  display={:.1} fps",
                self.role.as_str(),
                self.pair_number,
                self.traffic_pattern.as_str(),
                self.display_fps
            ),
            format!(
                "up={}@{}  down={}@{}",
                self.uplink_port.port,
                self.uplink_port.baud_rate,
                self.downlink_port.port,
                self.downlink_port.baud_rate
            ),
            format!(
                "tx-port={}@{}  rx-port={}@{}",
                self.tx_port.port,
                self.tx_port.baud_rate,
                self.rx_port.port,
                self.rx_port.baud_rate
            ),
            format!("tx={}  rx={}", self.tx_summary(), self.rx_summary()),
        ];

        lines.extend(self.driver.sender_lines());
        lines.extend(self.input.status_lines(&self.driver.target_rates()));

        lines.extend([
            format!(
                "display backlog  out={}  in={}  overflow={}",
                self.output.pending_packets(),
                self.input.display_queue.pending_packets(),
                self.output.overflow_packets() + self.input.display_queue.overflow_packets()
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

    fn tx_summary(&self) -> String {
        self.tx_formats
            .iter()
            .map(|format| {
                format!(
                    "{}@{}Hz",
                    xbee_test_format_label(format.kind),
                    format.rate_hz
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn rx_summary(&self) -> String {
        self.rx_formats
            .iter()
            .map(|(kind, track_expected_packets)| {
                if *track_expected_packets {
                    String::from(xbee_test_format_label(*kind))
                } else {
                    format!("{}(passive)", xbee_test_format_label(*kind))
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn run_result(self, log_path: PathBuf) -> XbeeMockRunResult {
        let XbeeMockState {
            role,
            pair_number,
            uplink_port,
            downlink_port,
            traffic_pattern,
            rx_formats,
            logging_enabled,
            input,
            driver,
            ..
        } = self;
        let target_rates = driver.target_rates();

        XbeeMockRunResult {
            role,
            pair_number,
            uplink_port,
            downlink_port,
            traffic_pattern,
            tx_formats: driver.tx_results(),
            rx_formats: rx_formats
                .into_iter()
                .map(|(kind, track_expected_packets)| XbeeMockRxRunResult {
                    kind,
                    track_expected_packets,
                    target_rate_hz: target_rates.get(&kind).copied().unwrap_or_default(),
                    stats: input.stats(kind),
                })
                .collect(),
            logging_enabled,
            log_path,
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

pub(crate) fn run_mock(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_xbee_mock_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let cli_options = match parse_xbee_mock_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_xbee_mock_help(bin_name);
            return ExitCode::from(2);
        }
    };

    match run_mock_with_options(cli_options) {
        Ok(result) => {
            println!(
                "role={} pair={} pattern={} up={}@{} down={}@{}",
                result.role.as_str(),
                result.pair_number,
                result.traffic_pattern.as_str(),
                result.uplink_port.port,
                result.uplink_port.baud_rate,
                result.downlink_port.port,
                result.downlink_port.baud_rate
            );

            for tx in &result.tx_formats {
                println!(
                    "  tx {}@{}Hz: sent={} tx_err={}",
                    xbee_test_format_label(tx.kind),
                    tx.rate_hz,
                    tx.sent_packets,
                    tx.write_errors
                );
            }
            for rx in &result.rx_formats {
                if rx.track_expected_packets {
                    println!(
                        "  rx {}: matched={} err={:.2}% target={:.1}Hz miss={} bad={} unexp={} overflow={}",
                        xbee_test_format_label(rx.kind),
                        rx.stats.matched_packets,
                        rx.stats.error_rate_percent(),
                        rx.target_rate_hz,
                        rx.stats.missing_packets,
                        rx.stats.invalid_packets,
                        rx.stats.unexpected_packets,
                        rx.stats.queue_overflow_packets
                    );
                } else {
                    println!(
                        "  rx {}: valid={} bad={} overflow={} track=passive",
                        xbee_test_format_label(rx.kind),
                        rx.stats.matched_packets,
                        rx.stats.invalid_packets,
                        rx.stats.queue_overflow_packets
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

fn run_mock_with_options(cli_options: XbeeMockCliOptions) -> Result<XbeeMockRunResult, String> {
    let settings = build_mock_settings(cli_options)?;
    let input_id = settings.role.as_str();
    let rx_port = settings.rx_port().clone();
    let tx_port = settings.tx_port().clone();
    let inputs = vec![SessionInputSpec {
        id: input_id.to_owned(),
        port: rx_port.port.clone(),
        baud_rate: rx_port.baud_rate,
        display_mode: PortDisplayMode::Hex,
        line_break_mode: LineBreakMode::Packet,
    }];
    let outputs = settings
        .tx_formats
        .iter()
        .map(|format| {
            let output_id = xbee_mock_lane_templates(settings.role)
                .iter()
                .find(|template| template.tx_kind == format.kind)
                .map(|template| template.tx_output_id)
                .ok_or_else(|| {
                    format!(
                        "missing xbee-mock output mapping for {}",
                        format.kind.display_name()
                    )
                })?;
            Ok(SessionOutputSpec {
                id: output_id.to_owned(),
                port: tx_port.port.clone(),
                baud_rate: tx_port.baud_rate,
                format_name: format.kind.session_format_name().to_owned(),
                display_mode: PortDisplayMode::Hex,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let known_formats = settings
        .rx_formats
        .iter()
        .map(|format| format.display_name().to_owned())
        .collect::<Vec<_>>();

    let mut session = SessionRuntime::new(SessionSpec {
        title: String::from("acs xbee-mock"),
        command_name: String::from("xbee-mock"),
        log_dir: settings.log_dir.clone(),
        logging_enabled: settings.logging_enabled,
        inputs,
        outputs,
    })?;
    session.set_output_packet_rate_enabled(&tx_port.port, true);
    session.set_input_packet_rate_enabled(&rx_port.port, true);
    session.set_input_known_formats(&rx_port.port, known_formats);
    session.set_manual_input_recording(input_id, true);
    for output in xbee_mock_lane_templates(settings.role)
        .iter()
        .filter(|template| {
            settings
                .tx_formats
                .iter()
                .any(|format| format.kind == template.tx_kind)
        })
    {
        session.set_manual_output_recording(output.tx_output_id, true);
    }

    let log_path = session.log_path().to_path_buf();
    let started_at = Instant::now();
    let state = RefCell::new(XbeeMockState::new(
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
    })
}

fn build_mock_settings(cli_options: XbeeMockCliOptions) -> Result<XbeeMockSettings, String> {
    let role = cli_options
        .role
        .as_deref()
        .map(XbeeMockRole::parse)
        .transpose()?
        .ok_or_else(|| String::from("xbee-mock requires a role: `base` or `remote`"))?;
    let pair_number = cli_options.pair_number.unwrap_or(1);
    if !matches!(pair_number, 1 | 2) {
        return Err(String::from("xbee-mock PAIR must be 1 or 2"));
    }
    let requested_ports = cli_options.ports;
    let (uplink_binding, downlink_binding) = resolve_xbee_mock_ports(pair_number, requested_ports)?;
    let uplink_port = ResolvedXbeeTestPort {
        port: serial::resolve_port(Some(&uplink_binding.port))
            .map_err(|error| error.to_string())?,
        baud_rate: uplink_binding.baud.unwrap_or_else(default_baud_rate),
    };
    let downlink_port = ResolvedXbeeTestPort {
        port: serial::resolve_port(Some(&downlink_binding.port))
            .map_err(|error| error.to_string())?,
        baud_rate: downlink_binding.baud.unwrap_or_else(default_baud_rate),
    };

    let traffic_pattern = cli_options
        .traffic_pattern
        .as_deref()
        .map(XbeeTestMode::parse)
        .transpose()?
        .unwrap_or(XbeeTestMode::Flood);
    let tx_formats = match cli_options.tx_formats {
        Some(tx_formats) => tx_formats,
        None => return Err(String::from("xbee-mock requires TX_FORMAT")),
    };
    let rx_formats = match cli_options.rx_formats {
        Some(rx_formats) => rx_formats,
        None => return Err(String::from("xbee-mock requires RX_FORMAT")),
    };
    validate_xbee_mock_traffic(role, traffic_pattern, &tx_formats, &rx_formats)?;

    let poll_rate_hz = tx_formats
        .iter()
        .find(|format| {
            matches!(
                format.kind,
                XbeeTestFrameKind::PollGreeting | XbeeTestFrameKind::PollResponse
            )
        })
        .map(|format| format.rate_hz)
        .unwrap_or(DEFAULT_RATE_HZ);
    let au_rate_hz = tx_formats
        .iter()
        .find(|format| format.kind == XbeeTestFrameKind::Format(OutputFormat::PacketAcV6))
        .map(|format| format.rate_hz)
        .unwrap_or(DEFAULT_RATE_HZ);
    let ru_rate_hz = tx_formats
        .iter()
        .find(|format| format.kind == XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral))
        .map(|format| format.rate_hz)
        .unwrap_or(DEFAULT_RATE_HZ);
    let ad_rate_hz = tx_formats
        .iter()
        .find(|format| format.kind == XbeeTestFrameKind::Format(OutputFormat::PacketJfV1))
        .map(|format| format.rate_hz)
        .unwrap_or(DEFAULT_RATE_HZ);
    let rd_rate_hz = tx_formats
        .iter()
        .find(|format| format.kind == XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral))
        .map(|format| format.rate_hz)
        .unwrap_or(DEFAULT_RATE_HZ);

    let log_dir = cli_options.log_dir.unwrap_or_else(default_log_dir);

    Ok(XbeeMockSettings {
        role,
        pair_number,
        uplink_port: uplink_port.clone(),
        downlink_port: downlink_port.clone(),
        tx_formats: tx_formats.clone(),
        rx_formats: rx_formats.clone(),
        traffic_pattern,
        port: match role {
            XbeeMockRole::Base => downlink_port.clone(),
            XbeeMockRole::Remote => uplink_port.clone(),
        },
        monitor_ports: Vec::new(),
        mode: traffic_pattern,
        poll_rate_hz,
        base_real_percent: DEFAULT_REAL_PERCENT,
        remote_real_percent: DEFAULT_REAL_PERCENT,
        au_rate_hz,
        ru_rate_hz,
        ad_rate_hz,
        rd_rate_hz,
        log_dir,
        logging_enabled: !cli_options.no_log,
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
            other => return Err(format!("unknown option for xbee-test: {other}")),
        }
    }

    Ok(options)
}

fn parse_xbee_mock_args(args: Vec<String>) -> Result<XbeeMockCliOptions, String> {
    let mut options = XbeeMockCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--role" => options.role = Some(next_value(&mut iter, "--role")?),
            "--port" | "-p" => options.ports.push(parse_xbee_mock_port_binding(&next_value(
                &mut iter, "--port",
            )?)?),
            "--option" => {
                apply_xbee_mock_option_assignment(
                    &mut options,
                    &next_value(&mut iter, "--option")?,
                )?;
            }
            "--log-dir" => {
                options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            "--no-log" => options.no_log = true,
            other if !other.starts_with('-') => {
                if options.role.is_some() {
                    return Err(format!("unexpected extra argument for xbee-mock: {other}"));
                }
                options.role = Some(other.to_owned());
            }
            other => return Err(format!("unknown option for xbee-mock: {other}")),
        }
    }

    Ok(options)
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

fn parse_xbee_mock_port_binding(value: &str) -> Result<XbeeMockPortBinding, String> {
    let (id, port_text) = if let Some((candidate_id, candidate_port)) = value.split_once('=') {
        (Some(XbeeMockPortId::parse(candidate_id)?), candidate_port)
    } else {
        (None, value)
    };
    let port_spec = parse_port_spec("xbee-mock port", port_text)?;
    if port_spec.display_mode.is_some() || port_spec.line_break_mode.is_some() {
        return Err(String::from(
            "xbee-mock fixes display to hex packet mode; omit `,DISPLAY` from --port",
        ));
    }

    Ok(XbeeMockPortBinding {
        id,
        port: port_spec.port,
        baud: port_spec.baud,
    })
}

fn resolve_xbee_mock_ports(
    pair_number: u32,
    ports: Vec<XbeeMockPortBinding>,
) -> Result<(XbeeMockResolvedPortBinding, XbeeMockResolvedPortBinding), String> {
    if ports.is_empty() {
        return Err(String::from(
            "xbee-mock requires at least one `-p PORT` (optional `@BAUD`, default: 115200)",
        ));
    }

    if pair_number == 1 {
        if ports.len() != 1 {
            return Err(String::from(
                "xbee-mock PAIR=1 requires exactly one `-p PORT` (optional `@BAUD`, default: 115200)",
            ));
        }
        let binding = ports.into_iter().next().expect("one binding exists");
        let resolved = XbeeMockResolvedPortBinding {
            port: binding.port,
            baud: binding.baud,
        };
        return Ok((resolved.clone(), resolved));
    }

    let mut up = None;
    let mut down = None;
    for binding in ports {
        let Some(id) = binding.id else {
            return Err(String::from(
                "xbee-mock PAIR=2 requires labeled ports: `-p up=PORT -p down=PORT` (optional `@BAUD`, default: 115200)",
            ));
        };
        match id {
            XbeeMockPortId::Up => {
                if up
                    .replace(XbeeMockResolvedPortBinding {
                        port: binding.port,
                        baud: binding.baud,
                    })
                    .is_some()
                {
                    return Err(String::from("xbee-mock received duplicate `up` port"));
                }
            }
            XbeeMockPortId::Down => {
                if down
                    .replace(XbeeMockResolvedPortBinding {
                        port: binding.port,
                        baud: binding.baud,
                    })
                    .is_some()
                {
                    return Err(String::from("xbee-mock received duplicate `down` port"));
                }
            }
        }
    }

    let up = up.ok_or_else(|| {
        String::from("xbee-mock PAIR=2 requires `-p up=PORT` (optional `@BAUD`, default: 115200)")
    })?;
    let down = down.ok_or_else(|| {
        String::from("xbee-mock PAIR=2 requires `-p down=PORT` (optional `@BAUD`, default: 115200)")
    })?;
    Ok((up, down))
}

fn apply_xbee_mock_option_assignment(
    options: &mut XbeeMockCliOptions,
    value: &str,
) -> Result<(), String> {
    for assignment in value.split(',') {
        let Some((key, raw_value)) = assignment.split_once('=') else {
            return Err(format!(
                "invalid xbee-mock option assignment: {assignment} (expected KEY=VALUE)"
            ));
        };
        let key = key.trim().to_ascii_uppercase();
        let raw_value = raw_value.trim();
        match key.as_str() {
            "PAIR" => options.pair_number = Some(parse_u32_arg("PAIR", raw_value)?),
            "TX_FORMAT" => options.tx_formats = Some(parse_xbee_mock_tx_format_list(raw_value)?),
            "RX_FORMAT" => options.rx_formats = Some(parse_xbee_mock_rx_format_list(raw_value)?),
            "TRAFFIC_PATTERN" => options.traffic_pattern = Some(raw_value.to_owned()),
            other => return Err(format!("unknown xbee-mock option key: {other}")),
        }
    }

    Ok(())
}

fn parse_xbee_mock_frame_kind(value: &str) -> Result<XbeeTestFrameKind, String> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "pollgreeting" | "poll-greeting" | "poll_greeting" => Ok(XbeeTestFrameKind::PollGreeting),
        "pollresponse" | "poll-response" | "poll_response" => Ok(XbeeTestFrameKind::PollResponse),
        _ => OutputFormat::parse(value).map(XbeeTestFrameKind::Format),
    }
}

fn parse_xbee_mock_tx_format_list(value: &str) -> Result<Vec<XbeeMockTxFormat>, String> {
    let mut formats = Vec::new();

    for entry in value.split('+') {
        let entry = entry.trim();
        if entry.is_empty() {
            return Err(String::from("TX_FORMAT must not contain empty entries"));
        }
        let (format_name, rate_hz) = if let Some((format_name, rate_text)) = entry.rsplit_once('@')
        {
            (format_name, parse_u32_arg("TX_FORMAT rate", rate_text)?)
        } else {
            (entry, DEFAULT_RATE_HZ)
        };
        if rate_hz == 0 {
            return Err(String::from("TX_FORMAT rate must be greater than 0"));
        }
        let kind = parse_xbee_mock_frame_kind(format_name)?;
        if formats
            .iter()
            .any(|existing: &XbeeMockTxFormat| existing.kind == kind)
        {
            return Err(format!(
                "TX_FORMAT contains duplicate format `{}`",
                kind.display_name()
            ));
        }
        formats.push(XbeeMockTxFormat { kind, rate_hz });
    }

    if formats.is_empty() {
        return Err(String::from("TX_FORMAT must not be empty"));
    }

    Ok(formats)
}

fn parse_xbee_mock_rx_format_list(value: &str) -> Result<Vec<XbeeTestFrameKind>, String> {
    let mut formats = Vec::new();

    for entry in value.split('+') {
        let entry = entry.trim();
        if entry.is_empty() {
            return Err(String::from("RX_FORMAT must not contain empty entries"));
        }
        if entry.contains('@') {
            return Err(String::from("RX_FORMAT does not accept `@RATE`"));
        }
        let kind = parse_xbee_mock_frame_kind(entry)?;
        if formats.contains(&kind) {
            return Err(format!(
                "RX_FORMAT contains duplicate format `{}`",
                kind.display_name()
            ));
        }
        formats.push(kind);
    }

    if formats.is_empty() {
        return Err(String::from("RX_FORMAT must not be empty"));
    }

    Ok(formats)
}

fn validate_xbee_mock_traffic(
    role: XbeeMockRole,
    traffic_pattern: XbeeTestMode,
    tx_formats: &[XbeeMockTxFormat],
    rx_formats: &[XbeeTestFrameKind],
) -> Result<(), String> {
    for tx in tx_formats {
        if !xbee_mock_tx_kind_allowed(role, tx.kind) {
            return Err(format!(
                "xbee-mock role={} cannot use {} in TX_FORMAT",
                role.as_str(),
                tx.kind.display_name()
            ));
        }
    }
    for &rx in rx_formats {
        if !xbee_mock_rx_kind_allowed(role, rx) {
            return Err(format!(
                "xbee-mock role={} cannot use {} in RX_FORMAT",
                role.as_str(),
                rx.display_name()
            ));
        }
    }

    match traffic_pattern {
        XbeeTestMode::Flood | XbeeTestMode::PingPong => {
            if tx_formats.iter().any(|format| {
                matches!(
                    format.kind,
                    XbeeTestFrameKind::PollGreeting | XbeeTestFrameKind::PollResponse
                )
            }) || rx_formats.iter().any(|format| {
                matches!(
                    format,
                    XbeeTestFrameKind::PollGreeting | XbeeTestFrameKind::PollResponse
                )
            }) {
                return Err(String::from(
                    "PollGreeting/PollResponse require TRAFFIC_PATTERN=polling",
                ));
            }
        }
        XbeeTestMode::Polling => {
            let expected_tx = match role {
                XbeeMockRole::Base => XbeeTestFrameKind::PollGreeting,
                XbeeMockRole::Remote => XbeeTestFrameKind::PollResponse,
            };
            let expected_rx = match role {
                XbeeMockRole::Base => XbeeTestFrameKind::PollResponse,
                XbeeMockRole::Remote => XbeeTestFrameKind::PollGreeting,
            };
            if tx_formats.len() != 1 || tx_formats[0].kind != expected_tx {
                return Err(format!(
                    "xbee-mock TRAFFIC_PATTERN=polling for role={} requires TX_FORMAT={}",
                    role.as_str(),
                    expected_tx.display_name()
                ));
            }
            if rx_formats.len() != 1 || rx_formats[0] != expected_rx {
                return Err(format!(
                    "xbee-mock TRAFFIC_PATTERN=polling for role={} requires RX_FORMAT={}",
                    role.as_str(),
                    expected_rx.display_name()
                ));
            }
        }
    }

    Ok(())
}

fn xbee_mock_tx_kind_allowed(role: XbeeMockRole, kind: XbeeTestFrameKind) -> bool {
    match role {
        XbeeMockRole::Base => matches!(
            kind,
            XbeeTestFrameKind::Format(OutputFormat::PacketAcV6)
                | XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral)
                | XbeeTestFrameKind::PollGreeting
        ),
        XbeeMockRole::Remote => matches!(
            kind,
            XbeeTestFrameKind::Format(OutputFormat::PacketJfV1)
                | XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral)
                | XbeeTestFrameKind::PollResponse
        ),
    }
}

fn xbee_mock_rx_kind_allowed(role: XbeeMockRole, kind: XbeeTestFrameKind) -> bool {
    match role {
        XbeeMockRole::Base => matches!(
            kind,
            XbeeTestFrameKind::Format(OutputFormat::PacketJfV1)
                | XbeeTestFrameKind::Format(OutputFormat::RoverDownGeneral)
                | XbeeTestFrameKind::PollResponse
        ),
        XbeeMockRole::Remote => matches!(
            kind,
            XbeeTestFrameKind::Format(OutputFormat::PacketAcV6)
                | XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral)
                | XbeeTestFrameKind::PollGreeting
        ),
    }
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
    bytes.len() == OutputFormat::RoverDownGeneral.packet_len()
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            0 => *byte == b'4',
            1 | 2 => byte.is_ascii_hexdigit(),
            3 => *byte == b',',
            4 | 5 | 7 | 8 => byte.is_ascii_digit(),
            6 => *byte == b'.',
            9 => *byte == b'\r',
            10 => *byte == b'\n',
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use super::{
        ExpectedPacketTracker, OutputFormat, PacketDefinition, PacketStreamDecoder,
        XbeeMockPortBinding, XbeeMockPortId, XbeeMockRole, XbeeTestFrameKind, XbeeTestMode,
        build_poll_frame, matches_poll_frame, parse_xbee_mock_args, parse_xbee_mock_port_binding,
        parse_xbee_test_args, parse_xbee_test_port_binding, resolve_xbee_mock_ports,
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
    fn parse_xbee_test_port_binding_rejects_display_override() {
        let error = parse_xbee_test_port_binding("base=/dev/ttyUSB0@921600,hex")
            .expect_err("should reject");
        assert!(error.contains("display"));
    }

    #[test]
    fn parse_xbee_mock_args_accepts_positional_role_and_port() {
        let options = parse_xbee_mock_args(vec![
            String::from("remote"),
            String::from("-p"),
            String::from("up=/dev/ttyUSB2@460800"),
            String::from("-p"),
            String::from("down=/dev/ttyUSB3@115200"),
            String::from("--option"),
            String::from(
                "PAIR=2,TX_FORMAT=packetjfv1@80+roverdowngeneral@70,RX_FORMAT=packetacv6+roverupgeneral,TRAFFIC_PATTERN=ping-pong",
            ),
            String::from("--log-dir"),
            String::from("tmp/logs"),
            String::from("--no-log"),
        ])
        .expect("should parse");

        assert_eq!(options.role.as_deref(), Some("remote"));
        assert_eq!(options.ports.len(), 2);
        assert_eq!(options.ports[0].id, Some(XbeeMockPortId::Up));
        assert_eq!(options.ports[0].port, "/dev/ttyUSB2");
        assert_eq!(options.ports[0].baud, Some(460_800));
        assert_eq!(options.ports[1].id, Some(XbeeMockPortId::Down));
        assert_eq!(options.ports[1].port, "/dev/ttyUSB3");
        assert_eq!(options.ports[1].baud, Some(115_200));
        assert_eq!(options.pair_number, Some(2));
        assert_eq!(options.traffic_pattern.as_deref(), Some("ping-pong"));
        assert_eq!(
            options.tx_formats.as_ref().map(|formats| formats.len()),
            Some(2)
        );
        assert_eq!(
            options.tx_formats.as_ref().map(|formats| formats[0].kind),
            Some(XbeeTestFrameKind::Format(OutputFormat::PacketJfV1))
        );
        assert_eq!(
            options
                .tx_formats
                .as_ref()
                .map(|formats| formats[0].rate_hz),
            Some(80)
        );
        assert_eq!(
            options.rx_formats.as_ref().map(|formats| formats.clone()),
            Some(vec![
                XbeeTestFrameKind::Format(OutputFormat::PacketAcV6),
                XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral),
            ])
        );
        assert_eq!(options.log_dir, Some(std::path::PathBuf::from("tmp/logs")));
        assert!(options.no_log);
    }

    #[test]
    fn parse_xbee_mock_args_accepts_ports_without_baud() {
        let options = parse_xbee_mock_args(vec![
            String::from("base"),
            String::from("-p"),
            String::from("up=/dev/ttyUSB2"),
            String::from("-p"),
            String::from("down=/dev/ttyUSB3"),
            String::from("--option"),
            String::from(
                "PAIR=2,TX_FORMAT=packetacv6@100,RX_FORMAT=packetjfv1,TRAFFIC_PATTERN=flood",
            ),
        ])
        .expect("should parse");

        assert_eq!(options.role.as_deref(), Some("base"));
        assert_eq!(options.ports.len(), 2);
        assert_eq!(options.ports[0].baud, None);
        assert_eq!(options.ports[1].baud, None);
    }

    #[test]
    fn parse_xbee_mock_port_rejects_display_override() {
        let error =
            parse_xbee_mock_port_binding("/dev/ttyUSB0@921600,hex").expect_err("should reject");
        assert!(error.contains("display"));
    }

    #[test]
    fn resolve_xbee_mock_ports_pair1_shares_single_port() {
        let (up, down) = resolve_xbee_mock_ports(
            1,
            vec![XbeeMockPortBinding {
                id: None,
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
            }],
        )
        .expect("should resolve");

        assert_eq!(up.port, "/dev/ttyUSB0");
        assert_eq!(up.baud, Some(921_600));
        assert_eq!(down.port, "/dev/ttyUSB0");
        assert_eq!(down.baud, Some(921_600));
    }

    #[test]
    fn resolve_xbee_mock_ports_pair1_rejects_multiple_ports() {
        let error = resolve_xbee_mock_ports(
            1,
            vec![
                XbeeMockPortBinding {
                    id: None,
                    port: String::from("/dev/ttyUSB0"),
                    baud: Some(921_600),
                },
                XbeeMockPortBinding {
                    id: None,
                    port: String::from("/dev/ttyUSB1"),
                    baud: Some(115_200),
                },
            ],
        )
        .expect_err("should reject");

        assert!(error.contains("PAIR=1 requires exactly one"));
    }

    #[test]
    fn resolve_xbee_mock_ports_pair2_requires_labeled_up_and_down() {
        let unlabeled_error = resolve_xbee_mock_ports(
            2,
            vec![XbeeMockPortBinding {
                id: None,
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
            }],
        )
        .expect_err("should reject unlabeled binding");
        assert!(unlabeled_error.contains("requires labeled ports"));

        let missing_down_error = resolve_xbee_mock_ports(
            2,
            vec![XbeeMockPortBinding {
                id: Some(XbeeMockPortId::Up),
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
            }],
        )
        .expect_err("should reject missing down");
        assert!(missing_down_error.contains("requires `-p down=PORT`"));
    }

    #[test]
    fn resolve_xbee_mock_ports_pair2_uses_labeled_up_and_down() {
        let (up, down) = resolve_xbee_mock_ports(
            2,
            vec![
                XbeeMockPortBinding {
                    id: Some(XbeeMockPortId::Up),
                    port: String::from("/dev/ttyUSB0"),
                    baud: Some(921_600),
                },
                XbeeMockPortBinding {
                    id: Some(XbeeMockPortId::Down),
                    port: String::from("/dev/ttyUSB1"),
                    baud: Some(115_200),
                },
            ],
        )
        .expect("should resolve");

        assert_eq!(up.port, "/dev/ttyUSB0");
        assert_eq!(up.baud, Some(921_600));
        assert_eq!(down.port, "/dev/ttyUSB1");
        assert_eq!(down.baud, Some(115_200));
    }

    #[test]
    fn xbee_mock_role_parse_accepts_base_and_remote() {
        assert_eq!(
            XbeeMockRole::parse("base").expect("role"),
            XbeeMockRole::Base
        );
        assert_eq!(
            XbeeMockRole::parse("remote").expect("role"),
            XbeeMockRole::Remote
        );
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
    fn poll_frame_uses_crc16_ccitt_false() {
        let payload = build_poll_frame(b"HI", 7);
        assert!(matches_poll_frame(&payload, b"HI"));

        let mut corrupted = payload;
        corrupted[4] ^= 0x01;
        assert!(!matches_poll_frame(&corrupted, b"HI"));
    }
}
