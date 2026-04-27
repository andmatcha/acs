use super::common::{
    PortSpec, default_baud_rate, default_log_dir, next_value, parse_key_value_args,
    parse_port_spec, parse_u32_arg,
};
use super::help::{is_help_flag, print_send_help};
use super::signal;
use crate::ingress::IngressFrame;
use crate::output::OutputFormat;
use crate::output::formats::{DummyPayloadGenerator, crc16_ccitt_false};
use crate::port_display::{
    LineBreakMode, PortDisplayConfig, PortDisplayMode, parse_display_assignment,
};
use crate::serial;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const DEFAULT_SEND_RATE_HZ: u32 = 50;
const SEND_LOOP_INTERVAL: Duration = Duration::from_millis(1);
const STATUS_INTERVAL: Duration = Duration::from_millis(200);
const RATE_WINDOW: Duration = Duration::from_secs(1);
const DISPLAY_FLUSH_SLICE: usize = 32;
const DISPLAY_FLUSH_PACKET_BUDGET: usize = 256;
const DISPLAY_QUEUE_LIMIT: usize = 65_536;
const SERIAL_FRAME_BITS_PER_BYTE: u64 = 10;

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
            if self.buffer.len() < format.packet_len() {
                continue;
            }

            let candidate = &self.buffer[..format.packet_len()];
            if format.matches_packet(candidate) {
                return Some(DecodedPacket {
                    format: format.format(),
                    bytes: self.buffer.drain(..format.packet_len()).collect(),
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
    if bytes.len() > OutputFormat::RoverDownGeneral.packet_len() {
        return false;
    }

    bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| rover_down_byte_matches(index, *byte))
}

fn matches_rover_down_packet(bytes: &[u8]) -> bool {
    bytes.len() == OutputFormat::RoverDownGeneral.packet_len() && matches_rover_down_prefix(bytes)
}

fn rover_down_byte_matches(index: usize, byte: u8) -> bool {
    match index {
        0 => byte == b'4',
        1 | 2 => byte.is_ascii_hexdigit(),
        3 => byte == b',',
        4 | 5 | 7 | 8 => byte.is_ascii_digit(),
        6 => byte == b'.',
        9 => byte == b'\r',
        10 => byte == b'\n',
        _ => false,
    }
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
        title: String::from("acs send"),
        command_name: String::from("send"),
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
    let output_bindings =
        resolve_output_bindings(&cli_options, default_baud, default_rate_hz, default_format)?;
    let monitor_port_specs = resolve_monitor_bindings(&cli_options)?;
    let display = cli_options.display;
    let log_dir = cli_options.log_dir.unwrap_or_else(default_log_dir);

    let mut outputs = Vec::new();
    let mut inputs = Vec::new();
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
        if !inputs
            .iter()
            .any(|input: &SessionInputSpec| input.port == monitor_port)
        {
            inputs.push(SessionInputSpec {
                id: monitor_port.clone(),
                port: monitor_port.clone(),
                baud_rate: port_spec.baud.unwrap_or(default_baud),
                display_mode: resolve_display_mode(
                    port_spec.display_mode,
                    display.resolve_input_override(&monitor_port),
                    monitor_formats
                        .first()
                        .copied()
                        .map(OutputFormat::default_display_mode),
                ),
                line_break_mode: resolve_monitor_line_break_mode(
                    port_spec.line_break_mode,
                    monitor_has_formats,
                    display.resolve_line_break_input(&monitor_port),
                ),
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

    Ok(SendSettings {
        inputs,
        outputs,
        observed_inputs,
        log_dir,
        logging_enabled: !cli_options.no_log,
        interactive: cli_options.interactive,
        s3b: cli_options.s3b,
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
        MixedFormatDecoder, ObservedInput, OutputSchedule, SendOutputSettings,
        estimated_output_line_bps, matches_rover_down_packet, parse_output_format_list,
        parse_send_args, parse_send_monitor_binding, parse_send_output_binding,
        realign_output_schedule, resolve_display_mode, resolve_monitor_line_break_mode,
        validate_output_port_loads,
    };
    use crate::output::OutputFormat;
    use crate::port_display::{LineBreakMode, PortDisplayMode};
    use crate::session::runtime::SessionOutputSpec;
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
