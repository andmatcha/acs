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
const ROVER_PORT_ID: &str = "rover";
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
    ac_rate_hz: Option<u32>,
    jf_rate_hz: Option<u32>,
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
    rover_port: ResolvedXbeeTestPort,
    mode: XbeeTestMode,
    ac_rate_hz: u32,
    jf_rate_hz: u32,
    log_dir: PathBuf,
    logging_enabled: bool,
}

struct XbeeTestRunResult {
    base_port: ResolvedXbeeTestPort,
    rover_port: ResolvedXbeeTestPort,
    mode: XbeeTestMode,
    ac_rate_hz: u32,
    jf_rate_hz: u32,
    ac_stats: PacketMatchStats,
    jf_stats: PacketMatchStats,
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
    definition: PacketDefinition,
    buffer: Vec<u8>,
}

impl PacketStreamDecoder {
    fn new(format: OutputFormat) -> Self {
        Self {
            definition: PacketDefinition::for_format(format),
            buffer: Vec::new(),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> DecodedPacketBatch {
        self.buffer.extend_from_slice(bytes);
        let mut packets = Vec::new();
        let mut invalid_packets = 0u64;

        loop {
            if self.buffer.len() < self.definition.header.len() {
                break;
            }

            let Some(header_index) = find_header(&self.buffer, &self.definition.header) else {
                let keep_len = self.buffer.len().min(self.definition.header.len() - 1);
                let drain_len = self.buffer.len().saturating_sub(keep_len);
                if drain_len > 0 {
                    self.buffer.drain(..drain_len);
                }
                break;
            };

            if header_index > 0 {
                self.buffer.drain(..header_index);
            }

            if self.buffer.len() < self.definition.packet_len {
                break;
            }

            if packet_is_valid(&self.buffer[..self.definition.packet_len], self.definition) {
                packets.push(self.buffer.drain(..self.definition.packet_len).collect());
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
    from_port_id: &'static str,
    format: OutputFormat,
    decoder: PacketStreamDecoder,
    tracker: ExpectedPacketTracker,
    rate_samples: VecDeque<PacketRateSample>,
    display_queue: PacketDisplayQueue,
}

impl ObservedInput {
    fn new(
        input_id: &'static str,
        input_port: String,
        from_port_id: &'static str,
        format: OutputFormat,
    ) -> Self {
        Self {
            input_id,
            input_port,
            from_port_id,
            format,
            decoder: PacketStreamDecoder::new(format),
            tracker: ExpectedPacketTracker::new(),
            rate_samples: VecDeque::new(),
            display_queue: PacketDisplayQueue::new(),
        }
    }

    fn expect(&mut self, packet: Vec<u8>) {
        self.tracker.expect(packet);
    }

    fn observe(&mut self, bytes: &[u8], now: Instant) -> ObservedInputBatch {
        let batch = self.decoder.push(bytes);
        self.tracker.record_invalid_packets(batch.invalid_packets);
        let mut valid_packet_count = 0usize;
        let mut valid_byte_len = 0usize;
        for packet in batch.packets {
            valid_packet_count += 1;
            valid_byte_len += packet.len();
            self.display_queue.enqueue(packet.clone());
            self.tracker.observe(packet);
        }
        if valid_packet_count > 0 {
            self.rate_samples.push_back(PacketRateSample {
                at: now,
                byte_len: valid_byte_len,
                packet_count: valid_packet_count,
            });
        }
        self.prune_rate_samples(now);

        ObservedInputBatch {
            valid_byte_len,
            valid_packet_count,
        }
    }

    fn flush_display_batch(
        &mut self,
        session: &mut SessionRuntime,
        max_packets: usize,
    ) -> Result<usize, String> {
        self.display_queue
            .flush_input_batch(session, &self.input_port, max_packets)
    }

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

    fn status_line(&self, target_rate_hz: u32) -> String {
        let stats = self.tracker.stats();
        format!(
            "{} <- {}  format={}  target={} Hz  rx={:.1} Hz {:.0} B/s  matched={}  err={:.2}%  match_pending={}  disp_pending={}  miss={}  bad={}  unexp={}  overflow={}",
            self.input_id,
            self.from_port_id,
            self.format.as_str(),
            target_rate_hz,
            self.rx_rate_hz(),
            self.rx_bytes_per_second(),
            stats.matched_packets,
            stats.error_rate_percent(),
            self.tracker.pending_packets(),
            self.display_queue.pending_packets(),
            stats.missing_packets,
            stats.invalid_packets,
            stats.unexpected_packets,
            stats.queue_overflow_packets + self.display_queue.overflow_packets()
        )
    }
}

struct PacketRateSample {
    at: Instant,
    byte_len: usize,
    packet_count: usize,
}

struct ObservedInputBatch {
    valid_byte_len: usize,
    valid_packet_count: usize,
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
                output.display_queue.enqueue(payload.clone());
                input.expect(payload);
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
            self.format.as_str(),
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
    rover_port: ResolvedXbeeTestPort,
    mode: XbeeTestMode,
    log_path_display: String,
    logging_enabled: bool,
    display_fps: f64,
    ac_sender: ScheduledSender,
    jf_sender: ScheduledSender,
    base_output: DisplayedOutput,
    rover_output: DisplayedOutput,
    base_input: ObservedInput,
    rover_input: ObservedInput,
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
            rover_port: settings.rover_port.clone(),
            mode: settings.mode,
            log_path_display,
            logging_enabled: settings.logging_enabled,
            display_fps: 0.0,
            ac_sender: ScheduledSender::new(
                BASE_PORT_ID,
                ROVER_PORT_ID,
                OutputFormat::PacketAcV6,
                settings.ac_rate_hz,
                started_at,
            )?,
            jf_sender: ScheduledSender::new(
                ROVER_PORT_ID,
                BASE_PORT_ID,
                OutputFormat::PacketJfV1,
                settings.jf_rate_hz,
                started_at,
            )?,
            base_output: DisplayedOutput::new(settings.base_port.port.clone()),
            rover_output: DisplayedOutput::new(settings.rover_port.port.clone()),
            base_input: ObservedInput::new(
                BASE_PORT_ID,
                settings.base_port.port.clone(),
                ROVER_PORT_ID,
                OutputFormat::PacketJfV1,
            ),
            rover_input: ObservedInput::new(
                ROVER_PORT_ID,
                settings.rover_port.port.clone(),
                BASE_PORT_ID,
                OutputFormat::PacketAcV6,
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
                }
            }
            ROVER_PORT_ID => {
                let batch = self.rover_input.observe(&frame.bytes, now);
                if batch.valid_packet_count > 0 {
                    session.record_input_sample(
                        &self.rover_input.input_port,
                        batch.valid_byte_len,
                        batch.valid_packet_count,
                    );
                    if self.mode == XbeeTestMode::PingPong {
                        for _ in 0..batch.valid_packet_count {
                            self.jf_sender.send_once(
                                session,
                                &mut self.base_input,
                                &mut self.rover_output,
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
        self.ac_sender
            .send_due_packets(now, session, &mut self.rover_input, &mut self.base_output);
        if self.mode == XbeeTestMode::Flood {
            self.jf_sender.send_due_packets(
                now,
                session,
                &mut self.base_input,
                &mut self.rover_output,
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
            let flushed = self.rover_output.flush_display_batch(session, slice)?;
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
            let flushed = self.rover_input.flush_display_batch(session, slice)?;
            remaining_budget = remaining_budget.saturating_sub(flushed);
            flushed_total += flushed;

            if flushed_total == 0 {
                break;
            }
        }

        Ok(())
    }

    fn build_header_lines(&self) -> Vec<String> {
        vec![
            format!(
                "mode={}  display={:.1} fps",
                self.mode.as_str(),
                self.display_fps
            ),
            format!(
                "{BASE_PORT_ID}: {} @ {} baud  tx={}  rx={}",
                self.base_port.port,
                self.base_port.baud_rate,
                OutputFormat::PacketAcV6.as_str(),
                OutputFormat::PacketJfV1.as_str()
            ),
            format!(
                "{ROVER_PORT_ID}: {} @ {} baud  tx={}  rx={}",
                self.rover_port.port,
                self.rover_port.baud_rate,
                OutputFormat::PacketJfV1.as_str(),
                OutputFormat::PacketAcV6.as_str()
            ),
            self.ac_sender.status_line(),
            self.rover_input.status_line(self.ac_sender.target_rate_hz),
            match self.mode {
                XbeeTestMode::Flood => self.jf_sender.status_line(),
                XbeeTestMode::PingPong => format!(
                    "{} -> {}  format={}  trigger=ac-rx  sent={}  tx_err={}",
                    self.jf_sender.output_id,
                    self.jf_sender.target_input_id,
                    self.jf_sender.format.as_str(),
                    self.jf_sender.sent_packets,
                    self.jf_sender.write_errors
                ),
            },
            self.base_input.status_line(match self.mode {
                XbeeTestMode::Flood => self.jf_sender.target_rate_hz,
                XbeeTestMode::PingPong => self.ac_sender.target_rate_hz,
            }),
            format!(
                "display backlog  out(base={}, rover={})  in(base={}, rover={})  overflow={}",
                self.base_output.pending_packets(),
                self.rover_output.pending_packets(),
                self.base_input.display_queue.pending_packets(),
                self.rover_input.display_queue.pending_packets(),
                self.base_output.overflow_packets()
                    + self.rover_output.overflow_packets()
                    + self.base_input.display_queue.overflow_packets()
                    + self.rover_input.display_queue.overflow_packets()
            ),
            if self.logging_enabled {
                format!("log: {}", self.log_path_display)
            } else {
                String::from("log: disabled (--no-log)")
            },
            String::from(SPACE_HINT),
        ]
    }

    fn run_result(self, log_path: PathBuf) -> XbeeTestRunResult {
        XbeeTestRunResult {
            base_port: self.base_port,
            rover_port: self.rover_port,
            mode: self.mode,
            ac_rate_hz: self.ac_sender.target_rate_hz,
            jf_rate_hz: self.jf_sender.target_rate_hz,
            ac_stats: self.rover_input.tracker.stats(),
            jf_stats: self.base_input.tracker.stats(),
            logging_enabled: self.logging_enabled,
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
                    "mode={} base={}@{} rover={}@{} ac={}Hz jf={}Hz",
                    result.mode.as_str(),
                    result.base_port.port,
                    result.base_port.baud_rate,
                    result.rover_port.port,
                    result.rover_port.baud_rate,
                    result.ac_rate_hz,
                    result.jf_rate_hz
                ),
                XbeeTestMode::PingPong => println!(
                    "mode={} base={}@{} rover={}@{} ac={}Hz jf=reply-to-ac",
                    result.mode.as_str(),
                    result.base_port.port,
                    result.base_port.baud_rate,
                    result.rover_port.port,
                    result.rover_port.baud_rate,
                    result.ac_rate_hz
                ),
            }
            println!(
                "  AC base->rover: matched={} err={:.2}% miss={} bad={} unexp={} overflow={}",
                result.ac_stats.matched_packets,
                result.ac_stats.error_rate_percent(),
                result.ac_stats.missing_packets,
                result.ac_stats.invalid_packets,
                result.ac_stats.unexpected_packets,
                result.ac_stats.queue_overflow_packets
            );
            println!(
                "  JF rover->base: matched={} err={:.2}% miss={} bad={} unexp={} overflow={}",
                result.jf_stats.matched_packets,
                result.jf_stats.error_rate_percent(),
                result.jf_stats.missing_packets,
                result.jf_stats.invalid_packets,
                result.jf_stats.unexpected_packets,
                result.jf_stats.queue_overflow_packets
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
                id: ROVER_PORT_ID.to_owned(),
                port: settings.rover_port.port.clone(),
                baud_rate: settings.rover_port.baud_rate,
                display_mode: PortDisplayMode::Hex,
                line_break_mode: LineBreakMode::Packet,
            },
        ],
        outputs: vec![
            SessionOutputSpec {
                id: BASE_PORT_ID.to_owned(),
                port: settings.base_port.port.clone(),
                baud_rate: settings.base_port.baud_rate,
                format_name: OutputFormat::PacketAcV6.as_str().to_owned(),
                display_mode: PortDisplayMode::Hex,
            },
            SessionOutputSpec {
                id: ROVER_PORT_ID.to_owned(),
                port: settings.rover_port.port.clone(),
                baud_rate: settings.rover_port.baud_rate,
                format_name: OutputFormat::PacketJfV1.as_str().to_owned(),
                display_mode: PortDisplayMode::Hex,
            },
        ],
    })?;
    session.set_output_packet_rate_enabled(&settings.base_port.port, true);
    session.set_output_packet_rate_enabled(&settings.rover_port.port, true);
    session.set_input_packet_rate_enabled(&settings.base_port.port, true);
    session.set_input_packet_rate_enabled(&settings.rover_port.port, true);
    session.set_manual_input_recording(BASE_PORT_ID, true);
    session.set_manual_input_recording(ROVER_PORT_ID, true);
    session.set_manual_output_recording(BASE_PORT_ID, true);
    session.set_manual_output_recording(ROVER_PORT_ID, true);

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
    let rover_binding = ports
        .remove(ROVER_PORT_ID)
        .ok_or_else(|| String::from("xbee-test requires `--port rover=PORT[@BAUD]`"))?;

    let base_port =
        serial::resolve_port(Some(&base_binding.port)).map_err(|error| error.to_string())?;
    let rover_port =
        serial::resolve_port(Some(&rover_binding.port)).map_err(|error| error.to_string())?;
    if base_port == rover_port {
        return Err(String::from(
            "xbee-test requires different ports for `base` and `rover`",
        ));
    }

    let mode = cli_options
        .mode
        .as_deref()
        .or(file_config.xbee_test.mode.as_deref())
        .map(XbeeTestMode::parse)
        .transpose()?
        .unwrap_or(XbeeTestMode::Flood);

    let ac_rate_hz = cli_options
        .ac_rate_hz
        .or(file_config.xbee_test.ac_rate_hz)
        .unwrap_or(DEFAULT_RATE_HZ);
    let jf_rate_hz = cli_options
        .jf_rate_hz
        .or(file_config.xbee_test.jf_rate_hz)
        .unwrap_or(DEFAULT_RATE_HZ);
    if ac_rate_hz == 0 {
        return Err(String::from("--ac-rate must be greater than 0"));
    }
    if mode == XbeeTestMode::Flood && jf_rate_hz == 0 {
        return Err(String::from("--jf-rate must be greater than 0"));
    }
    let jf_rate_hz = jf_rate_hz.max(1);

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
        rover_port: ResolvedXbeeTestPort {
            port: rover_port,
            baud_rate: rover_binding.baud.unwrap_or_else(default_baud_rate),
        },
        mode,
        ac_rate_hz,
        jf_rate_hz,
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
            "--ac-rate" => {
                let value = next_value(&mut iter, "--ac-rate")?;
                options.ac_rate_hz = Some(parse_u32_arg("--ac-rate", &value)?);
            }
            "--jf-rate" => {
                let value = next_value(&mut iter, "--jf-rate")?;
                options.jf_rate_hz = Some(parse_u32_arg("--jf-rate", &value)?);
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
            "invalid xbee-test port binding: {value} (expected base=PORT[@BAUD] or rover=PORT[@BAUD])"
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
        BASE_PORT_ID | ROVER_PORT_ID => Ok(id),
        _ => Err(format!(
            "unsupported xbee-test port label: {value} (expected `base` or `rover`)"
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
            String::from("rover=/dev/ttyUSB1@115200"),
            String::from("--mode"),
            String::from("ping-pong"),
            String::from("--ac-rate"),
            String::from("100"),
            String::from("--jf-rate"),
            String::from("80"),
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
        assert_eq!(options.ports[1].id, "rover");
        assert_eq!(options.ports[1].baud, Some(115_200));
        assert_eq!(options.mode.as_deref(), Some("ping-pong"));
        assert_eq!(options.ac_rate_hz, Some(100));
        assert_eq!(options.jf_rate_hz, Some(80));
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
