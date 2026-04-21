use super::common::{
    default_baud_rate, default_log_dir, next_value, parse_port_spec, parse_u32_arg,
};
use super::config;
use super::help::{is_help_flag, print_xbee_test_help};
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
use std::time::{Duration, Instant};

const BASE_PORT_ID: &str = "base";
const REMOTE_PORT_ID: &str = "remote";
const BASE_AU_OUTPUT_ID: &str = "base-au";
const BASE_RU_OUTPUT_ID: &str = "base-ru";
const REMOTE_AD_OUTPUT_ID: &str = "remote-ad";
const REMOTE_RD_OUTPUT_ID: &str = "remote-rd";
const DEFAULT_RATE_HZ: u32 = 100;
const LOOP_INTERVAL: Duration = Duration::from_millis(1);
const STATUS_INTERVAL: Duration = Duration::from_millis(200);
const EXPECTED_QUEUE_LIMIT: usize = 8_192;
const DISPLAY_FLUSH_SLICE: usize = 16;
const DISPLAY_FLUSH_PACKET_BUDGET: usize = 256;
const DISPLAY_QUEUE_LIMIT: usize = 65_536;
const SPACE_HINT: &str = "Space で表示を一時停止/再開  Ctrl-C で終了";

#[derive(Debug, Default)]
struct XbeeTestCliOptions {
    ports: Vec<XbeeTestPortBinding>,
    mode: Option<String>,
    au_rate_hz: Option<u32>,
    ru_rate_hz: Option<u32>,
    ad_rate_hz: Option<u32>,
    rd_rate_hz: Option<u32>,
    config_path: Option<PathBuf>,
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
enum XbeeTestMode {
    Flood,
    PingPong,
}

impl XbeeTestMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "flood" => Ok(Self::Flood),
            "ping-pong" | "pingpong" => Ok(Self::PingPong),
            other => Err(format!(
                "unsupported xbee-test mode: {other} (expected `flood` or `ping-pong`)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Flood => "flood",
            Self::PingPong => "ping-pong",
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

fn xbee_test_format_label(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::PacketAcV6 => "AU(PacketACv6)",
        OutputFormat::RoverUpGeneral => "RU(RoverUpGeneral)",
        OutputFormat::PacketJfV1 => "AD(PacketJFv1)",
        OutputFormat::RoverDownGeneral => "RD(RoverDownGeneral)",
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
    format: OutputFormat,
    buffer: Vec<u8>,
}

impl PacketStreamDecoder {
    fn new(format: OutputFormat) -> Self {
        Self {
            format,
            buffer: Vec::new(),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> DecodedPacketBatch {
        self.buffer.extend_from_slice(bytes);
        let mut packets = Vec::new();
        let mut invalid_packets = 0u64;

        loop {
            if self.buffer.len() < packet_start_len(self.format) {
                break;
            }

            let Some(header_index) = find_packet_start(&self.buffer, self.format) else {
                let keep_len = self
                    .buffer
                    .len()
                    .min(packet_start_len(self.format).saturating_sub(1));
                let drain_len = self.buffer.len().saturating_sub(keep_len);
                if drain_len > 0 {
                    self.buffer.drain(..drain_len);
                }
                break;
            };

            if header_index > 0 {
                self.buffer.drain(..header_index);
            }

            if self.buffer.len() < self.format.packet_len() {
                break;
            }

            if packet_matches(self.format, &self.buffer[..self.format.packet_len()]) {
                packets.push(self.buffer.drain(..self.format.packet_len()).collect());
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

fn packet_start_len(format: OutputFormat) -> usize {
    match format {
        OutputFormat::PacketAcV6 | OutputFormat::PacketJfV1 => 2,
        OutputFormat::RoverUpGeneral => 6,
        OutputFormat::RoverDownGeneral => 4,
    }
}

fn find_packet_start(buffer: &[u8], format: OutputFormat) -> Option<usize> {
    match format {
        OutputFormat::PacketAcV6 => find_header(buffer, b"AC"),
        OutputFormat::PacketJfV1 => find_header(buffer, b"JF"),
        OutputFormat::RoverUpGeneral => find_rover_up_start(buffer),
        OutputFormat::RoverDownGeneral => find_rover_down_start(buffer),
    }
}

fn packet_matches(format: OutputFormat, packet: &[u8]) -> bool {
    match format {
        OutputFormat::PacketAcV6 | OutputFormat::PacketJfV1 => {
            packet_is_valid(packet, PacketDefinition::for_format(format))
        }
        OutputFormat::RoverUpGeneral => matches_rover_up_packet(packet),
        OutputFormat::RoverDownGeneral => matches_rover_down_packet(packet),
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
    format: OutputFormat,
    decoder: PacketStreamDecoder,
    tracker: ExpectedPacketTracker,
    rate_samples: VecDeque<PacketRateSample>,
}

impl ObservedInput {
    fn new(
        input_id: &'static str,
        input_port: String,
        from_port_id: &'static str,
        formats: Vec<OutputFormat>,
    ) -> Self {
        Self {
            input_id,
            input_port,
            display_queue: PacketDisplayQueue::new(),
            format_states: formats
                .into_iter()
                .map(|format| ObservedInputFormatState {
                    from_port_id,
                    format,
                    decoder: PacketStreamDecoder::new(format),
                    tracker: ExpectedPacketTracker::new(),
                    rate_samples: VecDeque::new(),
                })
                .collect(),
        }
    }

    fn expect(&mut self, format: OutputFormat, packet: Vec<u8>) {
        if let Some(state) = self
            .format_states
            .iter_mut()
            .find(|state| state.format == format)
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
                state.tracker.observe(packet);
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
                    .per_format_totals
                    .insert(state.format, (valid_byte_len, valid_packet_count));
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

    fn status_lines(&self, target_rates_hz: &BTreeMap<OutputFormat, u32>) -> Vec<String> {
        self.format_states
            .iter()
            .map(|state| state.status_line(self.input_id, target_rates_hz))
            .collect()
    }

    fn stats(&self, format: OutputFormat) -> PacketMatchStats {
        self.format_states
            .iter()
            .find(|state| state.format == format)
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

    fn status_line(&self, input_id: &str, target_rates_hz: &BTreeMap<OutputFormat, u32>) -> String {
        let stats = self.tracker.stats();
        let target_rate_hz = target_rates_hz
            .get(&self.format)
            .copied()
            .unwrap_or_default();
        format!(
            "{} <- {}  format={}  target={} Hz  rx={:.1} Hz {:.0} B/s  matched={}  err={:.2}%  pending={}  miss={}  bad={}  unexp={}  overflow={}",
            input_id,
            self.from_port_id,
            xbee_test_format_label(self.format),
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
    per_format_totals: BTreeMap<OutputFormat, (usize, usize)>,
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

struct ScheduledSender {
    output_id: &'static str,
    target_input_id: &'static str,
    format: OutputFormat,
    target_rate_hz: u32,
    period: Duration,
    next_send_at: Instant,
    generator: Box<dyn DummyPayloadGenerator>,
    sent_packets: u64,
    write_errors: u64,
    last_error: Option<String>,
}

impl ScheduledSender {
    fn new(
        output_id: &'static str,
        target_input_id: &'static str,
        format: OutputFormat,
        target_rate_hz: u32,
        started_at: Instant,
    ) -> Result<Self, String> {
        if target_rate_hz == 0 {
            return Err(format!("{} rate must be greater than 0", format.as_str()));
        }

        Ok(Self {
            output_id,
            target_input_id,
            format,
            target_rate_hz,
            period: Duration::from_secs_f64(1.0 / target_rate_hz as f64),
            next_send_at: started_at,
            generator: format.create_dummy_generator()?,
            sent_packets: 0,
            write_errors: 0,
            last_error: None,
        })
    }

    fn send_due_packets(
        &mut self,
        now: Instant,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) {
        while now >= self.next_send_at {
            self.send_once(session, input, output);
            self.next_send_at += self.period;
        }
    }

    fn send_once(
        &mut self,
        session: &mut SessionRuntime,
        input: &mut ObservedInput,
        output: &mut DisplayedOutput,
    ) {
        let payload = match self.generator.next_payload() {
            Ok(payload) => payload,
            Err(error) => {
                self.write_errors = self.write_errors.saturating_add(1);
                self.last_error = Some(error);
                return;
            }
        };

        match session.write_output(self.output_id, &payload) {
            Ok(()) => {
                session.record_output_sample(&output.port, payload.len(), 1);
                session.record_output_format_sample(
                    &output.port,
                    self.format.display_name(),
                    payload.len(),
                    1,
                );
                output.display_queue.enqueue(payload.clone());
                input.expect(self.format, payload);
                self.sent_packets = self.sent_packets.saturating_add(1);
                self.last_error = None;
            }
            Err(error) => {
                self.write_errors = self.write_errors.saturating_add(1);
                self.last_error = Some(error);
            }
        }
    }

    fn status_line(&self) -> String {
        let mut line = format!(
            "{} -> {}  format={}  target={} Hz  sent={}  tx_err={}",
            self.output_id,
            self.target_input_id,
            xbee_test_format_label(self.format),
            self.target_rate_hz,
            self.sent_packets,
            self.write_errors
        );
        if let Some(error) = &self.last_error {
            line.push_str("  last_error=");
            line.push_str(error);
        }
        line
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
    log_path_display: String,
    logging_enabled: bool,
    display_fps: f64,
    au_sender: ScheduledSender,
    ru_sender: ScheduledSender,
    ad_sender: ScheduledSender,
    rd_sender: ScheduledSender,
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
        Ok(Self {
            base_port: settings.base_port.clone(),
            remote_port: settings.remote_port.clone(),
            mode: settings.mode,
            log_path_display,
            logging_enabled: settings.logging_enabled,
            display_fps: 0.0,
            au_sender: ScheduledSender::new(
                BASE_AU_OUTPUT_ID,
                REMOTE_PORT_ID,
                OutputFormat::PacketAcV6,
                settings.au_rate_hz,
                started_at,
            )?,
            ru_sender: ScheduledSender::new(
                BASE_RU_OUTPUT_ID,
                REMOTE_PORT_ID,
                OutputFormat::RoverUpGeneral,
                settings.ru_rate_hz,
                started_at,
            )?,
            ad_sender: ScheduledSender::new(
                REMOTE_AD_OUTPUT_ID,
                BASE_PORT_ID,
                OutputFormat::PacketJfV1,
                settings.ad_rate_hz,
                started_at,
            )?,
            rd_sender: ScheduledSender::new(
                REMOTE_RD_OUTPUT_ID,
                BASE_PORT_ID,
                OutputFormat::RoverDownGeneral,
                settings.rd_rate_hz,
                started_at,
            )?,
            base_output: DisplayedOutput::new(settings.base_port.port.clone()),
            remote_output: DisplayedOutput::new(settings.remote_port.port.clone()),
            base_input: ObservedInput::new(
                BASE_PORT_ID,
                settings.base_port.port.clone(),
                REMOTE_PORT_ID,
                vec![OutputFormat::PacketJfV1, OutputFormat::RoverDownGeneral],
            ),
            remote_input: ObservedInput::new(
                REMOTE_PORT_ID,
                settings.remote_port.port.clone(),
                BASE_PORT_ID,
                vec![OutputFormat::PacketAcV6, OutputFormat::RoverUpGeneral],
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
                    session.record_input_sample(
                        &self.base_input.input_port,
                        batch.valid_byte_len,
                        batch.valid_packet_count,
                    );
                    for (format, (byte_len, packet_count)) in batch.per_format_totals {
                        session.record_input_format_sample(
                            &self.base_input.input_port,
                            format.display_name(),
                            byte_len,
                            packet_count,
                        );
                    }
                }
            }
            REMOTE_PORT_ID => {
                let batch = self.remote_input.observe(&frame.bytes, now);
                if batch.valid_packet_count > 0 {
                    session.record_input_sample(
                        &self.remote_input.input_port,
                        batch.valid_byte_len,
                        batch.valid_packet_count,
                    );
                    for (&format, &(byte_len, packet_count)) in &batch.per_format_totals {
                        session.record_input_format_sample(
                            &self.remote_input.input_port,
                            format.display_name(),
                            byte_len,
                            packet_count,
                        );
                    }
                    if self.mode == XbeeTestMode::PingPong {
                        let ac_count = batch
                            .per_format_totals
                            .get(&OutputFormat::PacketAcV6)
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
                            .per_format_totals
                            .get(&OutputFormat::RoverUpGeneral)
                            .map(|(_, packet_count)| *packet_count)
                            .unwrap_or(0);
                        for _ in 0..up_count {
                            self.rd_sender.send_once(
                                session,
                                &mut self.base_input,
                                &mut self.remote_output,
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn on_tick(&mut self, session: &mut SessionRuntime) -> Result<(), String> {
        let now = Instant::now();
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
        if self.mode == XbeeTestMode::Flood {
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
        let mut lines = vec![
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
            self.au_sender.status_line(),
            self.ru_sender.status_line(),
        ];
        lines.extend(self.remote_input.status_lines(&remote_targets));
        lines.push(match self.mode {
            XbeeTestMode::Flood => self.ad_sender.status_line(),
            XbeeTestMode::PingPong => format!(
                "{} -> {}  format={}  trigger=au-rx  sent={}  tx_err={}",
                self.ad_sender.output_id,
                self.ad_sender.target_input_id,
                xbee_test_format_label(self.ad_sender.format),
                self.ad_sender.sent_packets,
                self.ad_sender.write_errors
            ),
        });
        lines.push(match self.mode {
            XbeeTestMode::Flood => self.rd_sender.status_line(),
            XbeeTestMode::PingPong => format!(
                "{} -> {}  format={}  trigger=ru-rx  sent={}  tx_err={}",
                self.rd_sender.output_id,
                self.rd_sender.target_input_id,
                xbee_test_format_label(self.rd_sender.format),
                self.rd_sender.sent_packets,
                self.rd_sender.write_errors
            ),
        });
        lines.extend(self.base_input.status_lines(&base_targets));
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

    fn run_result(self, log_path: PathBuf) -> XbeeTestRunResult {
        XbeeTestRunResult {
            base_port: self.base_port,
            remote_port: self.remote_port,
            mode: self.mode,
            au_rate_hz: self.au_sender.target_rate_hz,
            ru_rate_hz: self.ru_sender.target_rate_hz,
            ad_rate_hz: self.ad_sender.target_rate_hz,
            rd_rate_hz: self.rd_sender.target_rate_hz,
            au_stats: self.remote_input.stats(OutputFormat::PacketAcV6),
            ru_stats: self.remote_input.stats(OutputFormat::RoverUpGeneral),
            ad_stats: self.base_input.stats(OutputFormat::PacketJfV1),
            rd_stats: self.base_input.stats(OutputFormat::RoverDownGeneral),
            logging_enabled: self.logging_enabled,
            log_path,
        }
    }

    fn remote_target_rates(&self) -> BTreeMap<OutputFormat, u32> {
        BTreeMap::from([
            (OutputFormat::PacketAcV6, self.au_sender.target_rate_hz),
            (OutputFormat::RoverUpGeneral, self.ru_sender.target_rate_hz),
        ])
    }

    fn base_target_rates(&self) -> BTreeMap<OutputFormat, u32> {
        match self.mode {
            XbeeTestMode::Flood => BTreeMap::from([
                (OutputFormat::PacketJfV1, self.ad_sender.target_rate_hz),
                (
                    OutputFormat::RoverDownGeneral,
                    self.rd_sender.target_rate_hz,
                ),
            ]),
            XbeeTestMode::PingPong => BTreeMap::from([
                (OutputFormat::PacketJfV1, self.au_sender.target_rate_hz),
                (
                    OutputFormat::RoverDownGeneral,
                    self.ru_sender.target_rate_hz,
                ),
            ]),
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
    let loaded_config = config::load_config_or_default(cli_options.config_path.as_deref())?;
    let settings = build_settings(cli_options, loaded_config.config, &loaded_config.lookup)?;

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
        outputs: vec![
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
        ],
    })?;
    session.set_output_packet_rate_enabled(&settings.base_port.port, true);
    session.set_output_packet_rate_enabled(&settings.remote_port.port, true);
    session.set_input_packet_rate_enabled(&settings.base_port.port, true);
    session.set_input_packet_rate_enabled(&settings.remote_port.port, true);
    session.set_input_known_formats(
        &settings.base_port.port,
        vec![
            OutputFormat::PacketJfV1.display_name().to_owned(),
            OutputFormat::RoverDownGeneral.display_name().to_owned(),
        ],
    );
    session.set_input_known_formats(
        &settings.remote_port.port,
        vec![
            OutputFormat::PacketAcV6.display_name().to_owned(),
            OutputFormat::RoverUpGeneral.display_name().to_owned(),
        ],
    );
    session.set_manual_input_recording(BASE_PORT_ID, true);
    session.set_manual_input_recording(REMOTE_PORT_ID, true);
    session.set_manual_output_recording(BASE_AU_OUTPUT_ID, true);
    session.set_manual_output_recording(BASE_RU_OUTPUT_ID, true);
    session.set_manual_output_recording(REMOTE_AD_OUTPUT_ID, true);
    session.set_manual_output_recording(REMOTE_RD_OUTPUT_ID, true);

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

fn build_settings(
    cli_options: XbeeTestCliOptions,
    file_config: config::AppConfig,
    config_lookup: &super::paths::ConfigLookup,
) -> Result<XbeeTestSettings, String> {
    let mut ports = file_config
        .xbee_test
        .ports
        .into_iter()
        .map(|binding| {
            (
                binding.id,
                XbeeTestPortBinding {
                    id: String::new(),
                    port: binding.port,
                    baud: binding.baud,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

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

    let base_binding = ports
        .remove(BASE_PORT_ID)
        .ok_or_else(|| String::from("xbee-test requires `--port base=PORT[@BAUD]`"))?;
    let remote_binding = ports
        .remove(REMOTE_PORT_ID)
        .ok_or_else(|| String::from("xbee-test requires `--port remote=PORT[@BAUD]`"))?;

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
        .or(file_config.xbee_test.mode.as_deref())
        .map(XbeeTestMode::parse)
        .transpose()?
        .unwrap_or(XbeeTestMode::Flood);

    let au_rate_hz = cli_options
        .au_rate_hz
        .or(file_config.xbee_test.au_rate_hz)
        .unwrap_or(DEFAULT_RATE_HZ);
    let ru_rate_hz = cli_options
        .ru_rate_hz
        .or(file_config.xbee_test.ru_rate_hz)
        .unwrap_or(DEFAULT_RATE_HZ);
    let ad_rate_hz = cli_options
        .ad_rate_hz
        .or(file_config.xbee_test.ad_rate_hz)
        .unwrap_or(DEFAULT_RATE_HZ);
    let rd_rate_hz = cli_options
        .rd_rate_hz
        .or(file_config.xbee_test.rd_rate_hz)
        .unwrap_or(DEFAULT_RATE_HZ);
    if au_rate_hz == 0 {
        return Err(String::from("--au-rate must be greater than 0"));
    }
    if ru_rate_hz == 0 {
        return Err(String::from("--ru-rate must be greater than 0"));
    }
    if mode == XbeeTestMode::Flood && ad_rate_hz == 0 {
        return Err(String::from("--ad-rate must be greater than 0"));
    }
    if mode == XbeeTestMode::Flood && rd_rate_hz == 0 {
        return Err(String::from("--rd-rate must be greater than 0"));
    }
    let ad_rate_hz = ad_rate_hz.max(1);
    let rd_rate_hz = rd_rate_hz.max(1);

    let log_dir = cli_options
        .log_dir
        .or(file_config.xbee_test.log_dir)
        .or(file_config.log_dir)
        .unwrap_or_else(|| default_log_dir(config_lookup));

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
            "--config" => {
                options.config_path = Some(PathBuf::from(next_value(&mut iter, "--config")?))
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

fn parse_xbee_test_port_binding(value: &str) -> Result<XbeeTestPortBinding, String> {
    let Some((id, port_text)) = value.split_once('=') else {
        return Err(format!(
            "invalid xbee-test port binding: {value} (expected base=PORT[@BAUD] or remote=PORT[@BAUD])"
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
        ExpectedPacketTracker, OutputFormat, PacketDefinition, PacketStreamDecoder, XbeeTestMode,
        parse_xbee_test_args, parse_xbee_test_port_binding,
    };
    use std::path::PathBuf;

    #[test]
    fn parse_xbee_test_args_accepts_labeled_ports_and_rates() {
        let options = parse_xbee_test_args(vec![
            String::from("--port"),
            String::from("base=/dev/ttyUSB0@921600"),
            String::from("--port"),
            String::from("remote=/dev/ttyUSB1@115200"),
            String::from("--mode"),
            String::from("ping-pong"),
            String::from("--au-rate"),
            String::from("100"),
            String::from("--ru-rate"),
            String::from("90"),
            String::from("--ad-rate"),
            String::from("80"),
            String::from("--rd-rate"),
            String::from("70"),
            String::from("--config"),
            String::from("config"),
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
        assert_eq!(options.au_rate_hz, Some(100));
        assert_eq!(options.ru_rate_hz, Some(90));
        assert_eq!(options.ad_rate_hz, Some(80));
        assert_eq!(options.rd_rate_hz, Some(70));
        assert_eq!(options.config_path, Some(PathBuf::from("config")));
        assert_eq!(options.log_dir, Some(PathBuf::from("tmp/logs")));
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
    }

    #[test]
    fn packet_stream_decoder_resyncs_after_invalid_crc_for_jf() {
        let mut generator = OutputFormat::PacketJfV1
            .create_dummy_generator()
            .expect("generator");
        let payload = generator.next_payload().expect("payload");
        let mut corrupted = payload.clone();
        corrupted[15] ^= 0x01;
        let mut decoder = PacketStreamDecoder::new(OutputFormat::PacketJfV1);
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
}
