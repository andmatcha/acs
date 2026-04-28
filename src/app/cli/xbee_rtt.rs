use super::common::{
    default_baud_rate, next_value, parse_key_value_args, parse_port_spec, parse_u32_arg,
};
use super::help::{is_help_flag, print_xbee_rtt_help};
use super::signal;
use crate::common::format_bytes_hex;
use crate::output::formats::crc16_ccitt_false;
use crate::serial::{
    SerialCallback, SerialConfig, SerialEvent, SerialMonitor, SerialWriter,
    open_monitor_and_writer, resolve_port,
};
use std::cmp::{max, min};
use std::collections::VecDeque;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_PROBE_COUNT: u16 = 10;
const DEFAULT_PAYLOAD_SIZE: usize = 32;
const DEFAULT_INTERVAL_MS: u32 = 100;
const DEFAULT_CONNECT_TIMEOUT_MS: u32 = 3_000;
const DEFAULT_PROBE_TIMEOUT_MS: u32 = 1_000;
const CONTROL_RETRY_INTERVAL_MS: u64 = 200;
const EVENT_POLL_SLICE_MS: u64 = 20;
const PROTOCOL_VERSION: u8 = 2;
const PROTOCOL_MAGIC: [u8; 2] = *b"XR";
const MAX_FRAME_PAYLOAD_SIZE: usize = 4_096;
const FRAME_FIXED_PREFIX_LEN: usize = 13;
const FRAME_OVERHEAD_BYTES: usize = 15;
const ROUND_ID_FIRST: u8 = 1;
const ROUND_ID_SECOND: u8 = 2;
const ANSI_BLUE: &str = "\x1b[34m";
const ANSI_RED: &str = "\x1b[31m";
const ANSI_RESET: &str = "\x1b[0m";

#[derive(Debug, Default)]
struct XbeeRttCliOptions {
    ports: Vec<XbeeRttPortBinding>,
    payload_size: Option<usize>,
    probe_count: Option<u32>,
    interval_ms: Option<u32>,
    connect_timeout_ms: Option<u32>,
    probe_timeout_ms: Option<u32>,
    show_wire: bool,
    show_protocol: bool,
    s3b: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XbeeRttPortBinding {
    port: String,
    baud: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedXbeeRttPort {
    port: String,
    baud_rate: u32,
}

#[derive(Debug, Clone)]
struct XbeeRttSettings {
    local_ports: Vec<ResolvedXbeeRttPort>,
    payload_size: usize,
    probe_count: u16,
    interval: Duration,
    interval_ms: u32,
    connect_timeout: Duration,
    connect_timeout_ms: u32,
    probe_timeout: Duration,
    probe_timeout_ms: u32,
    trace_tap: Option<Arc<LiveTraceTap>>,
    s3b: bool,
}

impl XbeeRttSettings {
    fn spec(&self) -> MeasurementSpecWire {
        MeasurementSpecWire {
            payload_size: self.payload_size as u16,
            probe_count: self.probe_count,
            interval_ms: self.interval_ms,
            probe_timeout_ms: self.probe_timeout_ms,
        }
    }

    fn estimated_round_timeout(&self) -> Duration {
        let max_per_probe_ms = u64::from(max(self.interval_ms, self.probe_timeout_ms));
        let probe_budget_ms = max_per_probe_ms.saturating_mul(u64::from(self.probe_count));
        Duration::from_millis(
            probe_budget_ms.saturating_add(u64::from(self.connect_timeout_ms) * 2),
        )
    }
}

enum XbeeRttRunResult {
    Single(PeerRunResult),
    LocalPair(LocalPairRunResult),
}

impl XbeeRttRunResult {
    fn is_clean(&self) -> bool {
        match self {
            Self::Single(result) => result.is_clean(),
            Self::LocalPair(result) => result.is_clean(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PeerRunResult {
    local_port: ResolvedXbeeRttPort,
    local_nonce: u64,
    peer_nonce: u64,
    session_id: u32,
    payload_size: usize,
    probe_count: u16,
    interval_ms: u32,
    connect_timeout_ms: u32,
    probe_timeout_ms: u32,
    first_initiator_nonce: u64,
    outbound: MeasurementOutcome,
    inbound: MeasurementOutcome,
}

impl PeerRunResult {
    fn protocol_name(&self) -> &'static str {
        "xbee-rtt/2"
    }

    fn is_clean(&self) -> bool {
        self.outbound.is_clean() && self.inbound.is_clean()
    }

    fn round_trip_wire_bytes(&self) -> usize {
        (FRAME_OVERHEAD_BYTES + self.payload_size) * 2
    }

    fn first_initiator_is_local(&self) -> bool {
        self.first_initiator_nonce == self.local_nonce
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalPairRunResult {
    first_port: ResolvedXbeeRttPort,
    second_port: ResolvedXbeeRttPort,
    first_nonce: u64,
    second_nonce: u64,
    session_id: u32,
    payload_size: usize,
    probe_count: u16,
    interval_ms: u32,
    connect_timeout_ms: u32,
    probe_timeout_ms: u32,
    first_initiator_nonce: u64,
    first_to_second: MeasurementOutcome,
    second_to_first: MeasurementOutcome,
    summaries_match: bool,
}

impl LocalPairRunResult {
    fn protocol_name(&self) -> &'static str {
        "xbee-rtt/2"
    }

    fn is_clean(&self) -> bool {
        self.first_to_second.is_clean() && self.second_to_first.is_clean() && self.summaries_match
    }

    fn round_trip_wire_bytes(&self) -> usize {
        (FRAME_OVERHEAD_BYTES + self.payload_size) * 2
    }

    fn first_initiator_is_first_port(&self) -> bool {
        self.first_initiator_nonce == self.first_nonce
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MeasurementOutcome {
    initiator_nonce: u64,
    responder_nonce: u64,
    payload_size: usize,
    probe_count: u16,
    success_count: u16,
    timeout_count: u16,
    mismatch_count: u16,
    average_rtt: Option<Duration>,
    min_rtt: Option<Duration>,
    max_rtt: Option<Duration>,
}

impl MeasurementOutcome {
    fn is_clean(&self) -> bool {
        self.success_count == self.probe_count
            && self.timeout_count == 0
            && self.mismatch_count == 0
    }

    fn average_rtt_display(&self) -> String {
        format_duration(self.average_rtt)
    }

    fn min_rtt_display(&self) -> String {
        format_duration(self.min_rtt)
    }

    fn max_rtt_display(&self) -> String {
        format_duration(self.max_rtt)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireDirection {
    Tx,
    Rx,
}

impl WireDirection {
    fn color(self) -> &'static str {
        match self {
            Self::Tx => ANSI_BLUE,
            Self::Rx => ANSI_RED,
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Tx => ">",
            Self::Rx => "<",
        }
    }
}

#[derive(Debug)]
struct LiveTraceTap {
    local_pair: bool,
    show_wire: bool,
    show_protocol: bool,
    state: Mutex<LiveTraceTapState>,
}

#[derive(Debug, Default)]
struct LiveTraceTapState {
    heading_printed: bool,
    stream_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TraceKind {
    Wire,
    Protocol,
}

impl TraceKind {
    fn label(self) -> &'static str {
        match self {
            Self::Wire => "wire",
            Self::Protocol => "proto",
        }
    }
}

impl LiveTraceTap {
    fn new(local_pair: bool, show_wire: bool, show_protocol: bool) -> Self {
        Self {
            local_pair,
            show_wire,
            show_protocol,
            state: Mutex::new(LiveTraceTapState::default()),
        }
    }

    fn record_wire(&self, stream_label: Option<&str>, direction: WireDirection, bytes: &[u8]) {
        if !self.show_wire || bytes.is_empty() {
            return;
        }

        let mut state = self.state.lock().expect("trace tap mutex poisoned");
        self.ensure_heading(&mut state);
        print!(
            "{}",
            format_trace_chunk(
                TraceKind::Wire,
                stream_label,
                direction,
                &format_wire_bytes(bytes),
            )
        );
        let _ = io::stdout().flush();
        state.stream_open = true;
    }

    fn record_protocol(
        &self,
        stream_label: Option<&str>,
        direction: WireDirection,
        frame: &ProtocolFrame,
    ) {
        if !self.show_protocol {
            return;
        }

        let mut state = self.state.lock().expect("trace tap mutex poisoned");
        self.ensure_heading(&mut state);
        print!(
            "{}",
            format_trace_chunk(
                TraceKind::Protocol,
                stream_label,
                direction,
                &format_protocol_frame(frame),
            )
        );
        let _ = io::stdout().flush();
        state.stream_open = true;
    }

    fn ensure_heading(&self, state: &mut LiveTraceTapState) {
        if state.heading_printed {
            return;
        }

        let mut modes = Vec::new();
        if self.show_wire {
            modes.push("wire=hex");
        }
        if self.show_protocol {
            modes.push("protocol=decoded");
        }
        println!(
            "live trace: tx=blue rx=red {}{}",
            modes.join(" "),
            if self.local_pair {
                " local-pair prefixes [0>] / [1<]"
            } else {
                ""
            }
        );
        state.heading_printed = true;
    }

    fn finish_line(&self) {
        let mut state = self.state.lock().expect("trace tap mutex poisoned");
        if state.stream_open {
            println!();
        }
        state.stream_open = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Hello,
    HelloAck,
    MeasureStart,
    MeasureAck,
    Probe,
    ProbeEcho,
    Result,
    ResultAck,
}

impl FrameKind {
    fn as_u8(self) -> u8 {
        match self {
            Self::Hello => 1,
            Self::HelloAck => 2,
            Self::MeasureStart => 3,
            Self::MeasureAck => 4,
            Self::Probe => 5,
            Self::ProbeEcho => 6,
            Self::Result => 7,
            Self::ResultAck => 8,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Hello),
            2 => Some(Self::HelloAck),
            3 => Some(Self::MeasureStart),
            4 => Some(Self::MeasureAck),
            5 => Some(Self::Probe),
            6 => Some(Self::ProbeEcho),
            7 => Some(Self::Result),
            8 => Some(Self::ResultAck),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtocolFrame {
    kind: FrameKind,
    session_id: u32,
    round_id: u8,
    seq: u16,
    payload: Vec<u8>,
}

impl ProtocolFrame {
    fn encode(&self) -> Vec<u8> {
        let payload_len = self.payload.len() as u16;
        let mut bytes = Vec::with_capacity(FRAME_OVERHEAD_BYTES + self.payload.len());
        bytes.extend_from_slice(&PROTOCOL_MAGIC);
        bytes.push(PROTOCOL_VERSION);
        bytes.push(self.kind.as_u8());
        bytes.extend_from_slice(&self.session_id.to_le_bytes());
        bytes.push(self.round_id);
        bytes.extend_from_slice(&self.seq.to_le_bytes());
        bytes.extend_from_slice(&payload_len.to_le_bytes());
        bytes.extend_from_slice(&self.payload);
        let crc = crc16_ccitt_false(&bytes[2..]).to_le_bytes();
        bytes.extend_from_slice(&crc);
        bytes
    }
}

#[derive(Default)]
struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<ProtocolFrame> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();

        loop {
            if self.buffer.len() < PROTOCOL_MAGIC.len() {
                break;
            }

            let Some(header_index) = find_magic(&self.buffer) else {
                let keep_len = self
                    .buffer
                    .len()
                    .min(PROTOCOL_MAGIC.len().saturating_sub(1));
                let drain_len = self.buffer.len().saturating_sub(keep_len);
                if drain_len > 0 {
                    self.buffer.drain(..drain_len);
                }
                break;
            };

            if header_index > 0 {
                self.buffer.drain(..header_index);
            }

            if self.buffer.len() < FRAME_OVERHEAD_BYTES {
                break;
            }

            let version = self.buffer[2];
            let kind = FrameKind::from_u8(self.buffer[3]);
            let payload_len = u16::from_le_bytes([self.buffer[11], self.buffer[12]]) as usize;
            if version != PROTOCOL_VERSION || kind.is_none() || payload_len > MAX_FRAME_PAYLOAD_SIZE
            {
                self.buffer.drain(..1);
                continue;
            }

            let frame_len = FRAME_OVERHEAD_BYTES + payload_len;
            if self.buffer.len() < frame_len {
                break;
            }

            let expected_crc = crc16_ccitt_false(&self.buffer[2..frame_len - 2]);
            let actual_crc =
                u16::from_le_bytes([self.buffer[frame_len - 2], self.buffer[frame_len - 1]]);
            if expected_crc != actual_crc {
                self.buffer.drain(..1);
                continue;
            }

            frames.push(ProtocolFrame {
                kind: kind.expect("checked above"),
                session_id: u32::from_le_bytes([
                    self.buffer[4],
                    self.buffer[5],
                    self.buffer[6],
                    self.buffer[7],
                ]),
                round_id: self.buffer[8],
                seq: u16::from_le_bytes([self.buffer[9], self.buffer[10]]),
                payload: self.buffer[FRAME_FIXED_PREFIX_LEN..FRAME_FIXED_PREFIX_LEN + payload_len]
                    .to_vec(),
            });
            self.buffer.drain(..frame_len);
        }

        frames
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MeasurementSpecWire {
    payload_size: u16,
    probe_count: u16,
    interval_ms: u32,
    probe_timeout_ms: u32,
}

impl MeasurementSpecWire {
    fn encode(self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(12);
        payload.extend_from_slice(&self.payload_size.to_le_bytes());
        payload.extend_from_slice(&self.probe_count.to_le_bytes());
        payload.extend_from_slice(&self.interval_ms.to_le_bytes());
        payload.extend_from_slice(&self.probe_timeout_ms.to_le_bytes());
        payload
    }

    fn decode(payload: &[u8]) -> Result<Self, String> {
        if payload.len() != 12 {
            return Err(format!(
                "invalid xbee-rtt measurement spec payload length: {}",
                payload.len()
            ));
        }

        Ok(Self {
            payload_size: u16::from_le_bytes([payload[0], payload[1]]),
            probe_count: u16::from_le_bytes([payload[2], payload[3]]),
            interval_ms: u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]),
            probe_timeout_ms: u32::from_le_bytes([
                payload[8],
                payload[9],
                payload[10],
                payload[11],
            ]),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HelloWire {
    nonce: u64,
    spec: MeasurementSpecWire,
}

impl HelloWire {
    fn encode(self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(20);
        payload.extend_from_slice(&self.nonce.to_le_bytes());
        payload.extend_from_slice(&self.spec.encode());
        payload
    }

    fn decode(payload: &[u8]) -> Result<Self, String> {
        if payload.len() != 20 {
            return Err(format!(
                "invalid xbee-rtt hello payload length: {}",
                payload.len()
            ));
        }

        Ok(Self {
            nonce: u64::from_le_bytes([
                payload[0], payload[1], payload[2], payload[3], payload[4], payload[5], payload[6],
                payload[7],
            ]),
            spec: MeasurementSpecWire::decode(&payload[8..20])?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HelloAckWire {
    ack_nonce: u64,
}

impl HelloAckWire {
    fn encode(self) -> Vec<u8> {
        self.ack_nonce.to_le_bytes().to_vec()
    }

    fn decode(payload: &[u8]) -> Result<Self, String> {
        if payload.len() != 8 {
            return Err(format!(
                "invalid xbee-rtt hello-ack payload length: {}",
                payload.len()
            ));
        }

        Ok(Self {
            ack_nonce: u64::from_le_bytes([
                payload[0], payload[1], payload[2], payload[3], payload[4], payload[5], payload[6],
                payload[7],
            ]),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MeasurementSummaryWire {
    probe_count: u16,
    success_count: u16,
    timeout_count: u16,
    mismatch_count: u16,
    payload_size: u16,
    mean_rtt_us: u32,
    min_rtt_us: u32,
    max_rtt_us: u32,
}

impl MeasurementSummaryWire {
    fn from_outcome(outcome: &MeasurementOutcome) -> Self {
        Self {
            probe_count: outcome.probe_count,
            success_count: outcome.success_count,
            timeout_count: outcome.timeout_count,
            mismatch_count: outcome.mismatch_count,
            payload_size: outcome.payload_size as u16,
            mean_rtt_us: duration_to_micros_u32(outcome.average_rtt),
            min_rtt_us: duration_to_micros_u32(outcome.min_rtt),
            max_rtt_us: duration_to_micros_u32(outcome.max_rtt),
        }
    }

    fn encode(self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(22);
        payload.extend_from_slice(&self.probe_count.to_le_bytes());
        payload.extend_from_slice(&self.success_count.to_le_bytes());
        payload.extend_from_slice(&self.timeout_count.to_le_bytes());
        payload.extend_from_slice(&self.mismatch_count.to_le_bytes());
        payload.extend_from_slice(&self.payload_size.to_le_bytes());
        payload.extend_from_slice(&self.mean_rtt_us.to_le_bytes());
        payload.extend_from_slice(&self.min_rtt_us.to_le_bytes());
        payload.extend_from_slice(&self.max_rtt_us.to_le_bytes());
        payload
    }

    fn decode(payload: &[u8]) -> Result<Self, String> {
        if payload.len() != 22 {
            return Err(format!(
                "invalid xbee-rtt result payload length: {}",
                payload.len()
            ));
        }

        Ok(Self {
            probe_count: u16::from_le_bytes([payload[0], payload[1]]),
            success_count: u16::from_le_bytes([payload[2], payload[3]]),
            timeout_count: u16::from_le_bytes([payload[4], payload[5]]),
            mismatch_count: u16::from_le_bytes([payload[6], payload[7]]),
            payload_size: u16::from_le_bytes([payload[8], payload[9]]),
            mean_rtt_us: u32::from_le_bytes([payload[10], payload[11], payload[12], payload[13]]),
            min_rtt_us: u32::from_le_bytes([payload[14], payload[15], payload[16], payload[17]]),
            max_rtt_us: u32::from_le_bytes([payload[18], payload[19], payload[20], payload[21]]),
        })
    }
}

struct MeasurementAccumulator {
    initiator_nonce: u64,
    responder_nonce: u64,
    payload_size: usize,
    probe_count: u16,
    timeouts: u16,
    mismatches: u16,
    samples: Vec<Duration>,
}

impl MeasurementAccumulator {
    fn new(
        initiator_nonce: u64,
        responder_nonce: u64,
        payload_size: usize,
        probe_count: u16,
    ) -> Self {
        Self {
            initiator_nonce,
            responder_nonce,
            payload_size,
            probe_count,
            timeouts: 0,
            mismatches: 0,
            samples: Vec::with_capacity(usize::from(probe_count)),
        }
    }

    fn record_timeout(&mut self) {
        self.timeouts = self.timeouts.saturating_add(1);
    }

    fn record_mismatch(&mut self) {
        self.mismatches = self.mismatches.saturating_add(1);
    }

    fn record_success(&mut self, sample: Duration) {
        self.samples.push(sample);
    }

    fn finish(self) -> MeasurementOutcome {
        let average_rtt = if self.samples.is_empty() {
            None
        } else {
            let total_micros = self
                .samples
                .iter()
                .map(|sample| sample.as_micros())
                .sum::<u128>();
            let sample_count = self.samples.len() as u128;
            Some(Duration::from_micros(
                ((total_micros + sample_count / 2) / sample_count) as u64,
            ))
        };

        MeasurementOutcome {
            initiator_nonce: self.initiator_nonce,
            responder_nonce: self.responder_nonce,
            payload_size: self.payload_size,
            probe_count: self.probe_count,
            success_count: self.samples.len() as u16,
            timeout_count: self.timeouts,
            mismatch_count: self.mismatches,
            average_rtt,
            min_rtt: self.samples.iter().copied().min(),
            max_rtt: self.samples.iter().copied().max(),
        }
    }
}

enum PeerEvent {
    Data(Vec<u8>),
    Error(String),
}

struct LocalPeer {
    local_port: ResolvedXbeeRttPort,
    stream_label: Option<String>,
    local_nonce: u64,
    local_spec: MeasurementSpecWire,
    connect_timeout: Duration,
    control_retry_interval: Duration,
    trace_tap: Option<Arc<LiveTraceTap>>,
    event_rx: Receiver<PeerEvent>,
    _monitor: SerialMonitor,
    writer: SerialWriter,
    decoder: FrameDecoder,
    pending_frames: VecDeque<ProtocolFrame>,
    peer_hello: Option<HelloWire>,
    hello_acked: bool,
    session_id: Option<u32>,
}

impl LocalPeer {
    fn open(
        local_port: ResolvedXbeeRttPort,
        stream_label: Option<String>,
        settings: &XbeeRttSettings,
    ) -> Result<Self, String> {
        let (event_tx, event_rx) = mpsc::channel();
        let callback = make_xbee_rtt_callback(event_tx);
        let (monitor, writer) = open_monitor_and_writer(
            &SerialConfig {
                port: local_port.port.clone(),
                baud_rate: local_port.baud_rate,
                xbee_s3b_recovery: settings.s3b,
            },
            callback,
        )
        .map_err(|error| error.to_string())?;

        Ok(Self {
            local_port,
            stream_label,
            local_nonce: fresh_u64(),
            local_spec: settings.spec(),
            connect_timeout: settings.connect_timeout,
            control_retry_interval: Duration::from_millis(CONTROL_RETRY_INTERVAL_MS),
            trace_tap: settings.trace_tap.clone(),
            event_rx,
            _monitor: monitor,
            writer,
            decoder: FrameDecoder::default(),
            pending_frames: VecDeque::new(),
            peer_hello: None,
            hello_acked: false,
            session_id: None,
        })
    }

    fn run(mut self, settings: &XbeeRttSettings) -> Result<PeerRunResult, String> {
        self.run_connectivity_check()?;

        let peer_nonce = self.peer_nonce()?;
        let session_id = self.session_id.ok_or_else(|| {
            String::from("xbee-rtt internal error: session_id missing after connectivity check")
        })?;
        let first_initiator_nonce = first_initiator_nonce(self.local_nonce, peer_nonce)?;
        let second_initiator_nonce =
            other_nonce(first_initiator_nonce, self.local_nonce, peer_nonce);

        let first_round = self.run_round(
            ROUND_ID_FIRST,
            round_initiator_nonce(
                first_initiator_nonce,
                second_initiator_nonce,
                ROUND_ID_FIRST,
            ),
            settings,
        )?;
        let second_round = self.run_round(
            ROUND_ID_SECOND,
            round_initiator_nonce(
                first_initiator_nonce,
                second_initiator_nonce,
                ROUND_ID_SECOND,
            ),
            settings,
        )?;

        let (outbound, inbound) = if first_round.initiator_nonce == self.local_nonce {
            (first_round, second_round)
        } else {
            (second_round, first_round)
        };

        Ok(PeerRunResult {
            local_port: self.local_port,
            local_nonce: self.local_nonce,
            peer_nonce,
            session_id,
            payload_size: settings.payload_size,
            probe_count: settings.probe_count,
            interval_ms: settings.interval_ms,
            connect_timeout_ms: settings.connect_timeout_ms,
            probe_timeout_ms: settings.probe_timeout_ms,
            first_initiator_nonce,
            outbound,
            inbound,
        })
    }

    fn run_connectivity_check(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + self.connect_timeout;
        let mut next_hello_at = Instant::now();

        loop {
            self.ensure_not_interrupted()?;

            if self.peer_hello.is_some() && self.hello_acked {
                return Ok(());
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "xbee-rtt connectivity check timed out on {} before the peer handshake completed",
                    self.local_port.port
                ));
            }

            if now >= next_hello_at {
                self.send_hello()?;
                next_hello_at = now + self.control_retry_interval;
            }

            let next_action = min(next_hello_at, deadline);
            if let Some(frame) = self.next_frame_until(next_action)? {
                self.handle_hello_or_ack(frame)?;
            }
        }
    }

    fn run_round(
        &mut self,
        round_id: u8,
        initiator_nonce: u64,
        settings: &XbeeRttSettings,
    ) -> Result<MeasurementOutcome, String> {
        if initiator_nonce == self.local_nonce {
            self.run_initiator_round(round_id, settings)
        } else {
            self.run_responder_round(round_id, initiator_nonce, settings)
        }
    }

    fn run_initiator_round(
        &mut self,
        round_id: u8,
        settings: &XbeeRttSettings,
    ) -> Result<MeasurementOutcome, String> {
        let peer_nonce = self.peer_nonce()?;
        self.wait_for_measure_ack(round_id, settings.spec())?;

        let mut next_probe_at = Instant::now();
        let mut accumulator = MeasurementAccumulator::new(
            self.local_nonce,
            peer_nonce,
            settings.payload_size,
            settings.probe_count,
        );

        for probe_index in 0..settings.probe_count {
            self.wait_until(next_probe_at)?;

            let seq = probe_index.saturating_add(1);
            let payload =
                build_probe_payload(self.session_id()?, round_id, seq, settings.payload_size);
            let sent_at = Instant::now();
            self.send_probe(round_id, seq, payload.clone())?;

            match self.wait_for_probe_echo(round_id, seq, payload, settings.probe_timeout)? {
                ProbeOutcome::Success(received_at) => {
                    accumulator.record_success(received_at.duration_since(sent_at));
                }
                ProbeOutcome::Mismatch => accumulator.record_mismatch(),
                ProbeOutcome::Timeout => accumulator.record_timeout(),
            }

            next_probe_at = sent_at + settings.interval;
        }

        let outcome = accumulator.finish();
        self.wait_for_result_ack(round_id, MeasurementSummaryWire::from_outcome(&outcome))?;
        Ok(outcome)
    }

    fn run_responder_round(
        &mut self,
        round_id: u8,
        initiator_nonce: u64,
        settings: &XbeeRttSettings,
    ) -> Result<MeasurementOutcome, String> {
        let round_label = round_label(round_id);
        let expected_spec = settings.spec();
        let start_deadline = Instant::now() + self.connect_timeout;
        let mut round_started = false;
        let mut round_deadline = start_deadline;

        loop {
            self.ensure_not_interrupted()?;

            let now = Instant::now();
            let active_deadline = if round_started {
                round_deadline
            } else {
                start_deadline
            };
            if now >= active_deadline {
                return Err(format!(
                    "xbee-rtt timed out on {} while waiting for the peer to finish {round_label}",
                    self.local_port.port
                ));
            }

            let Some(frame) = self.next_frame_until(active_deadline)? else {
                continue;
            };

            if self.handle_hello_or_ack(frame.clone())? {
                continue;
            }
            if !self.matches_session(&frame) || frame.round_id != round_id {
                continue;
            }

            match frame.kind {
                FrameKind::MeasureStart => {
                    let spec = MeasurementSpecWire::decode(&frame.payload)?;
                    if spec != expected_spec {
                        return Err(format!(
                            "xbee-rtt round spec mismatch on {}: local={} peer={}",
                            self.local_port.port,
                            format_spec(expected_spec),
                            format_spec(spec),
                        ));
                    }
                    self.send_measure_ack(round_id)?;
                    round_started = true;
                    round_deadline = Instant::now() + settings.estimated_round_timeout();
                }
                FrameKind::Probe if round_started => {
                    if frame.payload.len() == settings.payload_size {
                        self.send_probe_echo(round_id, frame.seq, frame.payload)?;
                    }
                }
                FrameKind::Result if round_started => {
                    let summary = MeasurementSummaryWire::decode(&frame.payload)?;
                    validate_summary(summary, expected_spec, self.local_port.port.as_str())?;
                    self.send_result_ack(round_id)?;
                    return Ok(summary_to_outcome(
                        summary,
                        initiator_nonce,
                        self.local_nonce,
                    ));
                }
                _ => {}
            }
        }
    }

    fn wait_for_measure_ack(
        &mut self,
        round_id: u8,
        spec: MeasurementSpecWire,
    ) -> Result<(), String> {
        let deadline = Instant::now() + self.connect_timeout;
        let mut next_send_at = Instant::now();

        loop {
            self.ensure_not_interrupted()?;

            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "xbee-rtt failed to synchronize {} on {} within {} ms",
                    round_label(round_id),
                    self.local_port.port,
                    self.connect_timeout.as_millis()
                ));
            }

            if now >= next_send_at {
                self.send_measure_start(round_id, spec)?;
                next_send_at = now + self.control_retry_interval;
            }

            let next_action = min(next_send_at, deadline);
            let Some(frame) = self.next_frame_until(next_action)? else {
                continue;
            };

            if self.handle_hello_or_ack(frame.clone())? {
                continue;
            }
            if !self.matches_session(&frame) || frame.round_id != round_id {
                continue;
            }

            if frame.kind == FrameKind::MeasureAck {
                return Ok(());
            }
        }
    }

    fn wait_for_probe_echo(
        &mut self,
        round_id: u8,
        seq: u16,
        payload: Vec<u8>,
        probe_timeout: Duration,
    ) -> Result<ProbeOutcome, String> {
        let deadline = Instant::now() + probe_timeout;

        loop {
            self.ensure_not_interrupted()?;

            let now = Instant::now();
            if now >= deadline {
                return Ok(ProbeOutcome::Timeout);
            }

            let Some(frame) = self.next_frame_until(deadline)? else {
                continue;
            };

            if self.handle_hello_or_ack(frame.clone())? {
                continue;
            }
            if !self.matches_session(&frame) || frame.round_id != round_id {
                continue;
            }

            if frame.kind == FrameKind::ProbeEcho && frame.seq == seq {
                if frame.payload == payload {
                    return Ok(ProbeOutcome::Success(Instant::now()));
                }
                return Ok(ProbeOutcome::Mismatch);
            }
        }
    }

    fn wait_for_result_ack(
        &mut self,
        round_id: u8,
        summary: MeasurementSummaryWire,
    ) -> Result<(), String> {
        let deadline = Instant::now() + self.connect_timeout;
        let mut next_send_at = Instant::now();

        loop {
            self.ensure_not_interrupted()?;

            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "xbee-rtt failed to exchange {} result on {} within {} ms",
                    round_label(round_id),
                    self.local_port.port,
                    self.connect_timeout.as_millis()
                ));
            }

            if now >= next_send_at {
                self.send_result(round_id, summary)?;
                next_send_at = now + self.control_retry_interval;
            }

            let next_action = min(next_send_at, deadline);
            let Some(frame) = self.next_frame_until(next_action)? else {
                continue;
            };

            if self.handle_hello_or_ack(frame.clone())? {
                continue;
            }
            if !self.matches_session(&frame) || frame.round_id != round_id {
                continue;
            }

            if frame.kind == FrameKind::ResultAck {
                return Ok(());
            }
        }
    }

    fn wait_until(&mut self, deadline: Instant) -> Result<(), String> {
        while Instant::now() < deadline {
            self.ensure_not_interrupted()?;
            let now = Instant::now();
            let next_action = min(deadline, now + Duration::from_millis(EVENT_POLL_SLICE_MS));
            if let Some(frame) = self.next_frame_until(next_action)? {
                let _ = self.handle_hello_or_ack(frame)?;
            }
        }
        Ok(())
    }

    fn next_frame_until(&mut self, deadline: Instant) -> Result<Option<ProtocolFrame>, String> {
        loop {
            if let Some(frame) = self.pending_frames.pop_front() {
                return Ok(Some(frame));
            }

            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }

            let timeout = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(EVENT_POLL_SLICE_MS));
            match self.event_rx.recv_timeout(timeout) {
                Ok(PeerEvent::Data(bytes)) => {
                    self.trace_wire(WireDirection::Rx, &bytes);
                    for frame in self.decoder.push(&bytes) {
                        self.trace_protocol(WireDirection::Rx, &frame);
                        self.pending_frames.push_back(frame);
                    }
                }
                Ok(PeerEvent::Error(message)) => {
                    return Err(format!(
                        "serial input error on {} ({}): {message}",
                        self.local_nonce_hex(),
                        self.local_port.port
                    ));
                }
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(String::from("xbee-rtt internal event channel disconnected"));
                }
            }
        }
    }

    fn handle_hello_or_ack(&mut self, frame: ProtocolFrame) -> Result<bool, String> {
        if frame.session_id != 0 || frame.round_id != 0 || frame.seq != 0 {
            return Ok(false);
        }

        match frame.kind {
            FrameKind::Hello => {
                let hello = HelloWire::decode(&frame.payload)?;
                self.observe_peer_hello(hello)?;
                self.send_hello_ack(hello.nonce)?;
                Ok(true)
            }
            FrameKind::HelloAck => {
                let ack = HelloAckWire::decode(&frame.payload)?;
                if ack.ack_nonce == self.local_nonce {
                    self.hello_acked = true;
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn observe_peer_hello(&mut self, hello: HelloWire) -> Result<(), String> {
        if hello.nonce == self.local_nonce {
            return Err(format!(
                "xbee-rtt nonce collision on {} (0x{:016x}); rerun the command",
                self.local_port.port, self.local_nonce
            ));
        }
        if hello.spec != self.local_spec {
            return Err(format!(
                "xbee-rtt option mismatch on {}: local={} peer={}",
                self.local_port.port,
                format_spec(self.local_spec),
                format_spec(hello.spec),
            ));
        }

        if let Some(existing) = self.peer_hello {
            if existing.nonce != hello.nonce {
                return Err(format!(
                    "xbee-rtt received HELLO from multiple peers on {} (0x{:016x} and 0x{:016x})",
                    self.local_port.port, existing.nonce, hello.nonce
                ));
            }
        }

        self.session_id = Some(derive_session_id(self.local_nonce, hello.nonce));
        self.peer_hello = Some(hello);
        Ok(())
    }

    fn send_hello(&mut self) -> Result<(), String> {
        self.send_frame(ProtocolFrame {
            kind: FrameKind::Hello,
            session_id: 0,
            round_id: 0,
            seq: 0,
            payload: HelloWire {
                nonce: self.local_nonce,
                spec: self.local_spec,
            }
            .encode(),
        })
    }

    fn send_hello_ack(&mut self, ack_nonce: u64) -> Result<(), String> {
        self.send_frame(ProtocolFrame {
            kind: FrameKind::HelloAck,
            session_id: 0,
            round_id: 0,
            seq: 0,
            payload: HelloAckWire { ack_nonce }.encode(),
        })
    }

    fn send_measure_start(
        &mut self,
        round_id: u8,
        spec: MeasurementSpecWire,
    ) -> Result<(), String> {
        self.send_session_frame(round_id, 0, FrameKind::MeasureStart, spec.encode())
    }

    fn send_measure_ack(&mut self, round_id: u8) -> Result<(), String> {
        self.send_session_frame(round_id, 0, FrameKind::MeasureAck, Vec::new())
    }

    fn send_probe(&mut self, round_id: u8, seq: u16, payload: Vec<u8>) -> Result<(), String> {
        self.send_session_frame(round_id, seq, FrameKind::Probe, payload)
    }

    fn send_probe_echo(&mut self, round_id: u8, seq: u16, payload: Vec<u8>) -> Result<(), String> {
        self.send_session_frame(round_id, seq, FrameKind::ProbeEcho, payload)
    }

    fn send_result(&mut self, round_id: u8, summary: MeasurementSummaryWire) -> Result<(), String> {
        self.send_session_frame(round_id, 0, FrameKind::Result, summary.encode())
    }

    fn send_result_ack(&mut self, round_id: u8) -> Result<(), String> {
        self.send_session_frame(round_id, 0, FrameKind::ResultAck, Vec::new())
    }

    fn send_session_frame(
        &mut self,
        round_id: u8,
        seq: u16,
        kind: FrameKind,
        payload: Vec<u8>,
    ) -> Result<(), String> {
        self.send_frame(ProtocolFrame {
            kind,
            session_id: self.session_id()?,
            round_id,
            seq,
            payload,
        })
    }

    fn send_frame(&mut self, frame: ProtocolFrame) -> Result<(), String> {
        let bytes = frame.encode();
        self.writer
            .write_bytes(&bytes)
            .map_err(|error| error.to_string())?;
        self.trace_wire(WireDirection::Tx, &bytes);
        self.trace_protocol(WireDirection::Tx, &frame);
        Ok(())
    }

    fn matches_session(&self, frame: &ProtocolFrame) -> bool {
        Some(frame.session_id) == self.session_id
    }

    fn session_id(&self) -> Result<u32, String> {
        self.session_id
            .ok_or_else(|| String::from("xbee-rtt internal error: session_id is not ready"))
    }

    fn peer_nonce(&self) -> Result<u64, String> {
        self.peer_hello
            .map(|hello| hello.nonce)
            .ok_or_else(|| String::from("xbee-rtt internal error: peer hello is not ready"))
    }

    fn local_nonce_hex(&self) -> String {
        format!("0x{:016x}", self.local_nonce)
    }

    fn ensure_not_interrupted(&self) -> Result<(), String> {
        if signal::is_stop_requested() {
            Err(String::from("xbee-rtt interrupted by Ctrl-C"))
        } else {
            Ok(())
        }
    }

    fn trace_wire(&self, direction: WireDirection, bytes: &[u8]) {
        if let Some(trace_tap) = &self.trace_tap {
            trace_tap.record_wire(self.stream_label.as_deref(), direction, bytes);
        }
    }

    fn trace_protocol(&self, direction: WireDirection, frame: &ProtocolFrame) {
        if let Some(trace_tap) = &self.trace_tap {
            trace_tap.record_protocol(self.stream_label.as_deref(), direction, frame);
        }
    }
}

enum ProbeOutcome {
    Success(Instant),
    Mismatch,
    Timeout,
}

pub(crate) fn run(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_xbee_rtt_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let cli_options = match parse_xbee_rtt_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            print_xbee_rtt_help(bin_name);
            return ExitCode::from(2);
        }
    };

    match run_with_options(cli_options) {
        Ok(result) => {
            print_run_result(&result);
            if result.is_clean() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn run_with_options(cli_options: XbeeRttCliOptions) -> Result<XbeeRttRunResult, String> {
    let settings = build_settings(cli_options)?;
    let trace_tap = settings.trace_tap.clone();
    signal::install_handler();

    let result = (|| {
        if settings.local_ports.len() == 1 {
            let result = execute_peer(settings.local_ports[0].clone(), None, settings.clone())?;
            return Ok(XbeeRttRunResult::Single(result));
        }

        let first_port = settings.local_ports[0].clone();
        let second_port = settings.local_ports[1].clone();
        let settings_for_first = settings.clone();
        let settings_for_second = settings.clone();

        let first_worker = thread::spawn(move || {
            execute_peer(first_port, Some(String::from("0")), settings_for_first)
        });
        let second_worker = thread::spawn(move || {
            execute_peer(second_port, Some(String::from("1")), settings_for_second)
        });

        let first_result = join_peer_worker(first_worker)?;
        let second_result = join_peer_worker(second_worker)?;
        Ok(XbeeRttRunResult::LocalPair(aggregate_local_pair(
            first_result,
            second_result,
        )?))
    })();

    if let Some(trace_tap) = trace_tap {
        trace_tap.finish_line();
    }
    result
}

fn execute_peer(
    local_port: ResolvedXbeeRttPort,
    stream_label: Option<String>,
    settings: XbeeRttSettings,
) -> Result<PeerRunResult, String> {
    let peer = LocalPeer::open(local_port, stream_label, &settings)?;
    peer.run(&settings)
}

fn join_peer_worker(
    worker: thread::JoinHandle<Result<PeerRunResult, String>>,
) -> Result<PeerRunResult, String> {
    worker
        .join()
        .map_err(|_| String::from("xbee-rtt local worker thread panicked"))?
}

fn aggregate_local_pair(
    first: PeerRunResult,
    second: PeerRunResult,
) -> Result<LocalPairRunResult, String> {
    if first.peer_nonce != second.local_nonce || second.peer_nonce != first.local_nonce {
        return Err(format!(
            "xbee-rtt local-pair mode expected the two local ports to pair with each other, but got {} -> 0x{:016x} and {} -> 0x{:016x}",
            first.local_port.port, first.peer_nonce, second.local_port.port, second.peer_nonce
        ));
    }
    if first.session_id != second.session_id {
        return Err(String::from(
            "xbee-rtt local-pair mode failed because the two local ports negotiated different sessions",
        ));
    }
    if first.payload_size != second.payload_size
        || first.probe_count != second.probe_count
        || first.interval_ms != second.interval_ms
        || first.connect_timeout_ms != second.connect_timeout_ms
        || first.probe_timeout_ms != second.probe_timeout_ms
    {
        return Err(String::from(
            "xbee-rtt local-pair mode failed because the two local workers disagreed on measurement metadata",
        ));
    }

    let summaries_match = first.inbound == second.outbound && second.inbound == first.outbound;

    Ok(LocalPairRunResult {
        first_port: first.local_port.clone(),
        second_port: second.local_port.clone(),
        first_nonce: first.local_nonce,
        second_nonce: second.local_nonce,
        session_id: first.session_id,
        payload_size: first.payload_size,
        probe_count: first.probe_count,
        interval_ms: first.interval_ms,
        connect_timeout_ms: first.connect_timeout_ms,
        probe_timeout_ms: first.probe_timeout_ms,
        first_initiator_nonce: first.first_initiator_nonce,
        first_to_second: first.outbound,
        second_to_first: second.outbound,
        summaries_match,
    })
}

fn print_run_result(result: &XbeeRttRunResult) {
    match result {
        XbeeRttRunResult::Single(result) => print_single_result(result),
        XbeeRttRunResult::LocalPair(result) => print_local_pair_result(result),
    }
}

fn print_single_result(result: &PeerRunResult) {
    println!(
        "status={} protocol={} mode=single-port session=0x{:08x}",
        if result.is_clean() { "ok" } else { "degraded" },
        result.protocol_name(),
        result.session_id
    );
    println!(
        "local={}@{} nonce=0x{:016x}",
        result.local_port.port, result.local_port.baud_rate, result.local_nonce
    );
    println!(
        "peer nonce=0x{:016x} first-initiator={}",
        result.peer_nonce,
        if result.first_initiator_is_local() {
            "local"
        } else {
            "peer"
        }
    );
    println!(
        "probe payload={}B count={} interval={}ms probe-timeout={}ms connect-timeout={}ms frame-overhead={}B round-trip-wire={}B",
        result.payload_size,
        result.probe_count,
        result.interval_ms,
        result.probe_timeout_ms,
        result.connect_timeout_ms,
        FRAME_OVERHEAD_BYTES,
        result.round_trip_wire_bytes()
    );
    print_labeled_measurement("local->peer", &result.outbound);
    print_labeled_measurement("peer->local", &result.inbound);
}

fn print_local_pair_result(result: &LocalPairRunResult) {
    println!(
        "status={} protocol={} mode=local-pair session=0x{:08x}",
        if result.is_clean() { "ok" } else { "degraded" },
        result.protocol_name(),
        result.session_id
    );
    println!(
        "port[0]={}@{} nonce=0x{:016x}",
        result.first_port.port, result.first_port.baud_rate, result.first_nonce
    );
    println!(
        "port[1]={}@{} nonce=0x{:016x}",
        result.second_port.port, result.second_port.baud_rate, result.second_nonce
    );
    println!(
        "first-initiator={} cross-check={}",
        if result.first_initiator_is_first_port() {
            "port[0]"
        } else {
            "port[1]"
        },
        if result.summaries_match {
            "ok"
        } else {
            "mismatch"
        }
    );
    println!(
        "probe payload={}B count={} interval={}ms probe-timeout={}ms connect-timeout={}ms frame-overhead={}B round-trip-wire={}B",
        result.payload_size,
        result.probe_count,
        result.interval_ms,
        result.probe_timeout_ms,
        result.connect_timeout_ms,
        FRAME_OVERHEAD_BYTES,
        result.round_trip_wire_bytes()
    );
    print_labeled_measurement(
        &format!("{}->{}", result.first_port.port, result.second_port.port),
        &result.first_to_second,
    );
    print_labeled_measurement(
        &format!("{}->{}", result.second_port.port, result.first_port.port),
        &result.second_to_first,
    );
}

fn print_labeled_measurement(label: &str, outcome: &MeasurementOutcome) {
    println!(
        "{label} avg={} min={} max={} success={}/{} timeout={} mismatch={}",
        outcome.average_rtt_display(),
        outcome.min_rtt_display(),
        outcome.max_rtt_display(),
        outcome.success_count,
        outcome.probe_count,
        outcome.timeout_count,
        outcome.mismatch_count,
    );
}

fn build_settings(cli_options: XbeeRttCliOptions) -> Result<XbeeRttSettings, String> {
    let XbeeRttCliOptions {
        ports,
        payload_size,
        probe_count,
        interval_ms,
        connect_timeout_ms,
        probe_timeout_ms,
        show_wire,
        show_protocol,
        s3b,
    } = cli_options;

    if ports.len() > 2 {
        return Err(String::from(
            "xbee-rtt accepts at most two `--port PORT[@BAUD]` options",
        ));
    }

    let resolved_ports = if ports.is_empty() {
        vec![ResolvedXbeeRttPort {
            port: resolve_port(None).map_err(|error| error.to_string())?,
            baud_rate: default_baud_rate(),
        }]
    } else {
        ports
            .into_iter()
            .map(|binding| {
                Ok(ResolvedXbeeRttPort {
                    port: resolve_port(Some(&binding.port)).map_err(|error| error.to_string())?,
                    baud_rate: binding.baud.unwrap_or_else(default_baud_rate),
                })
            })
            .collect::<Result<Vec<_>, String>>()?
    };

    if resolved_ports.len() == 2 && resolved_ports[0].port == resolved_ports[1].port {
        return Err(String::from(
            "xbee-rtt local-pair mode requires two different ports",
        ));
    }

    let payload_size = payload_size.unwrap_or(DEFAULT_PAYLOAD_SIZE);
    if payload_size > MAX_FRAME_PAYLOAD_SIZE {
        return Err(format!(
            "--payload-size must be <= {MAX_FRAME_PAYLOAD_SIZE}"
        ));
    }

    let probe_count = probe_count.unwrap_or(u32::from(DEFAULT_PROBE_COUNT));
    if probe_count == 0 {
        return Err(String::from("--count must be greater than 0"));
    }
    if probe_count > u32::from(u16::MAX) {
        return Err(format!("--count must be <= {}", u16::MAX));
    }

    let interval_ms = interval_ms.unwrap_or(DEFAULT_INTERVAL_MS);
    let connect_timeout_ms = connect_timeout_ms.unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS);
    if connect_timeout_ms == 0 {
        return Err(String::from("--connect-timeout-ms must be greater than 0"));
    }

    let probe_timeout_ms = probe_timeout_ms.unwrap_or(DEFAULT_PROBE_TIMEOUT_MS);
    if probe_timeout_ms == 0 {
        return Err(String::from("--probe-timeout-ms must be greater than 0"));
    }

    let trace_tap = (show_wire || show_protocol).then(|| {
        Arc::new(LiveTraceTap::new(
            resolved_ports.len() == 2,
            show_wire,
            show_protocol,
        ))
    });

    Ok(XbeeRttSettings {
        local_ports: resolved_ports,
        payload_size,
        probe_count: probe_count as u16,
        interval: Duration::from_millis(u64::from(interval_ms)),
        interval_ms,
        connect_timeout: Duration::from_millis(u64::from(connect_timeout_ms)),
        connect_timeout_ms,
        probe_timeout: Duration::from_millis(u64::from(probe_timeout_ms)),
        probe_timeout_ms,
        trace_tap,
        s3b,
    })
}

fn parse_xbee_rtt_args(args: Vec<String>) -> Result<XbeeRttCliOptions, String> {
    let mut options = XbeeRttCliOptions::default();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--port" | "-p" => options.ports.push(parse_xbee_rtt_port_binding(&next_value(
                &mut iter, "--port",
            )?)?),
            "--config" => {
                apply_xbee_rtt_config_args(&mut options, &next_value(&mut iter, "--config")?)?
            }
            "--payload-size" => {
                let value = next_value(&mut iter, "--payload-size")?;
                options.payload_size = Some(parse_usize_arg("--payload-size", &value)?);
            }
            "--count" | "-n" => {
                let value = next_value(&mut iter, "--count")?;
                options.probe_count = Some(parse_u32_arg("--count", &value)?);
            }
            "--interval-ms" => {
                let value = next_value(&mut iter, "--interval-ms")?;
                options.interval_ms = Some(parse_u32_arg("--interval-ms", &value)?);
            }
            "--connect-timeout-ms" => {
                let value = next_value(&mut iter, "--connect-timeout-ms")?;
                options.connect_timeout_ms = Some(parse_u32_arg("--connect-timeout-ms", &value)?);
            }
            "--probe-timeout-ms" => {
                let value = next_value(&mut iter, "--probe-timeout-ms")?;
                options.probe_timeout_ms = Some(parse_u32_arg("--probe-timeout-ms", &value)?);
            }
            "--show-wire" => options.show_wire = true,
            "--show-protocol" => options.show_protocol = true,
            "--s3b" => options.s3b = true,
            other => return Err(format!("unknown option for xbee-rtt: {other}")),
        }
    }

    Ok(options)
}

fn apply_xbee_rtt_config_args(options: &mut XbeeRttCliOptions, value: &str) -> Result<(), String> {
    for assignment in parse_key_value_args("--config", value)? {
        let key = assignment.key.to_ascii_uppercase();
        match key.as_str() {
            "PAYLOAD_SIZE" => {
                options.payload_size = Some(parse_usize_arg("PAYLOAD_SIZE", &assignment.value)?);
            }
            "COUNT" => options.probe_count = Some(parse_u32_arg("COUNT", &assignment.value)?),
            "INTERVAL_MS" => {
                options.interval_ms = Some(parse_u32_arg("INTERVAL_MS", &assignment.value)?);
            }
            "CONNECT_TIMEOUT_MS" => {
                options.connect_timeout_ms =
                    Some(parse_u32_arg("CONNECT_TIMEOUT_MS", &assignment.value)?);
            }
            "PROBE_TIMEOUT_MS" => {
                options.probe_timeout_ms =
                    Some(parse_u32_arg("PROBE_TIMEOUT_MS", &assignment.value)?);
            }
            other => return Err(format!("unknown xbee-rtt config key: {other}")),
        }
    }

    Ok(())
}

fn parse_xbee_rtt_port_binding(value: &str) -> Result<XbeeRttPortBinding, String> {
    let port_spec = parse_port_spec("xbee-rtt port", value)?;
    if port_spec.display_mode.is_some() || port_spec.line_break_mode.is_some() {
        return Err(String::from(
            "xbee-rtt uses a fixed binary protocol; omit `,DISPLAY` from --port",
        ));
    }

    Ok(XbeeRttPortBinding {
        port: port_spec.port,
        baud: port_spec.baud,
    })
}

fn parse_usize_arg(option: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid value for {option}: {value}"))
}

fn find_magic(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(PROTOCOL_MAGIC.len())
        .position(|window| window == PROTOCOL_MAGIC)
}

fn validate_summary(
    summary: MeasurementSummaryWire,
    spec: MeasurementSpecWire,
    port: &str,
) -> Result<(), String> {
    if summary.payload_size != spec.payload_size || summary.probe_count != spec.probe_count {
        return Err(format!(
            "xbee-rtt received inconsistent result metadata on {port}: expected={} peer=payload={}B,count={}",
            format_spec(spec),
            summary.payload_size,
            summary.probe_count
        ));
    }
    Ok(())
}

fn summary_to_outcome(
    summary: MeasurementSummaryWire,
    initiator_nonce: u64,
    responder_nonce: u64,
) -> MeasurementOutcome {
    MeasurementOutcome {
        initiator_nonce,
        responder_nonce,
        payload_size: usize::from(summary.payload_size),
        probe_count: summary.probe_count,
        success_count: summary.success_count,
        timeout_count: summary.timeout_count,
        mismatch_count: summary.mismatch_count,
        average_rtt: micros_to_duration(summary.mean_rtt_us),
        min_rtt: micros_to_duration(summary.min_rtt_us),
        max_rtt: micros_to_duration(summary.max_rtt_us),
    }
}

fn micros_to_duration(value: u32) -> Option<Duration> {
    (value != 0).then(|| Duration::from_micros(u64::from(value)))
}

fn build_probe_payload(session_id: u32, round_id: u8, seq: u16, payload_size: usize) -> Vec<u8> {
    let mut state = (u64::from(session_id) << 16)
        ^ (u64::from(round_id) << 8)
        ^ u64::from(seq)
        ^ 0x9E37_79B9_7F4A_7C15;
    let mut payload = Vec::with_capacity(payload_size);
    for index in 0..payload_size {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        payload.push((state as u8) ^ (index as u8));
    }
    payload
}

fn first_initiator_nonce(local_nonce: u64, peer_nonce: u64) -> Result<u64, String> {
    if local_nonce == peer_nonce {
        return Err(String::from(
            "xbee-rtt nonce collision detected; rerun the command on both peers",
        ));
    }
    Ok(min(local_nonce, peer_nonce))
}

fn round_initiator_nonce(
    first_initiator_nonce: u64,
    second_initiator_nonce: u64,
    round_id: u8,
) -> u64 {
    match round_id {
        ROUND_ID_FIRST => first_initiator_nonce,
        ROUND_ID_SECOND => second_initiator_nonce,
        _ => panic!("unsupported round_id: {round_id}"),
    }
}

fn other_nonce(first_nonce: u64, local_nonce: u64, peer_nonce: u64) -> u64 {
    if first_nonce == local_nonce {
        peer_nonce
    } else {
        local_nonce
    }
}

fn round_label(round_id: u8) -> &'static str {
    match round_id {
        ROUND_ID_FIRST => "round-1",
        ROUND_ID_SECOND => "round-2",
        _ => "unknown-round",
    }
}

fn format_spec(spec: MeasurementSpecWire) -> String {
    format!(
        "payload={}B,count={},interval={}ms,probe-timeout={}ms",
        spec.payload_size, spec.probe_count, spec.interval_ms, spec.probe_timeout_ms
    )
}

fn duration_to_micros_u32(value: Option<Duration>) -> u32 {
    value
        .map(|duration| duration.as_micros().min(u128::from(u32::MAX)) as u32)
        .unwrap_or(0)
}

fn format_duration(value: Option<Duration>) -> String {
    match value {
        Some(duration) => format!("{:.3} ms", duration.as_secs_f64() * 1_000.0),
        None => String::from("n/a"),
    }
}

fn format_trace_chunk(
    kind: TraceKind,
    stream_label: Option<&str>,
    direction: WireDirection,
    body: &str,
) -> String {
    let prefix = match stream_label {
        Some(label) => format!("{}[{label}{}] ", kind.label(), direction.marker()),
        None => format!("{}[{}] ", kind.label(), direction.marker()),
    };
    format!("{}{}{}{} ", direction.color(), prefix, body, ANSI_RESET)
}

fn format_wire_bytes(bytes: &[u8]) -> String {
    format_bytes_hex(bytes)
}

fn format_protocol_frame(frame: &ProtocolFrame) -> String {
    match frame.kind {
        FrameKind::Hello => match HelloWire::decode(&frame.payload) {
            Ok(hello) => format!(
                "HELLO(nonce=0x{:016x}, {})",
                hello.nonce,
                format_spec(hello.spec)
            ),
            Err(_) => format!("HELLO(invalid-payload-len={})", frame.payload.len()),
        },
        FrameKind::HelloAck => match HelloAckWire::decode(&frame.payload) {
            Ok(ack) => format!("HELLO_ACK(ack=0x{:016x})", ack.ack_nonce),
            Err(_) => format!("HELLO_ACK(invalid-payload-len={})", frame.payload.len()),
        },
        FrameKind::MeasureStart => match MeasurementSpecWire::decode(&frame.payload) {
            Ok(spec) => format!(
                "MEASURE_START(session=0x{:08x}, round={}, {})",
                frame.session_id,
                trace_round_label(frame.round_id),
                format_spec(spec)
            ),
            Err(_) => format!(
                "MEASURE_START(session=0x{:08x}, round={}, invalid-payload-len={})",
                frame.session_id,
                trace_round_label(frame.round_id),
                frame.payload.len()
            ),
        },
        FrameKind::MeasureAck => format!(
            "MEASURE_ACK(session=0x{:08x}, round={})",
            frame.session_id,
            trace_round_label(frame.round_id)
        ),
        FrameKind::Probe => format!(
            "PROBE(session=0x{:08x}, round={}, seq={}, bytes={})",
            frame.session_id,
            trace_round_label(frame.round_id),
            frame.seq,
            frame.payload.len()
        ),
        FrameKind::ProbeEcho => format!(
            "PROBE_ECHO(session=0x{:08x}, round={}, seq={}, bytes={})",
            frame.session_id,
            trace_round_label(frame.round_id),
            frame.seq,
            frame.payload.len()
        ),
        FrameKind::Result => match MeasurementSummaryWire::decode(&frame.payload) {
            Ok(summary) => format!(
                "RESULT(session=0x{:08x}, round={}, payload={}B, success={}/{}, timeout={}, mismatch={}, avg={}, min={}, max={})",
                frame.session_id,
                trace_round_label(frame.round_id),
                summary.payload_size,
                summary.success_count,
                summary.probe_count,
                summary.timeout_count,
                summary.mismatch_count,
                format_duration(micros_to_duration(summary.mean_rtt_us)),
                format_duration(micros_to_duration(summary.min_rtt_us)),
                format_duration(micros_to_duration(summary.max_rtt_us))
            ),
            Err(_) => format!(
                "RESULT(session=0x{:08x}, round={}, invalid-payload-len={})",
                frame.session_id,
                trace_round_label(frame.round_id),
                frame.payload.len()
            ),
        },
        FrameKind::ResultAck => format!(
            "RESULT_ACK(session=0x{:08x}, round={})",
            frame.session_id,
            trace_round_label(frame.round_id)
        ),
    }
}

fn trace_round_label(round_id: u8) -> &'static str {
    match round_id {
        0 => "control",
        ROUND_ID_FIRST => "round-1",
        ROUND_ID_SECOND => "round-2",
        _ => "unknown-round",
    }
}

fn derive_session_id(first_nonce: u64, second_nonce: u64) -> u32 {
    let (low, high) = if first_nonce < second_nonce {
        (first_nonce, second_nonce)
    } else {
        (second_nonce, first_nonce)
    };

    let mut state = low ^ 0xA5A5_5A5A_C3C3_3C3C;
    state ^= state >> 12;
    state ^= state << 25;
    state ^= state >> 27;
    state = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
    state ^= high.rotate_left(17);
    state ^= state >> 28;
    state = state.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (state ^ (state >> 32)) as u32
}

fn fresh_u64() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut value = nanos ^ ((std::process::id() as u64) << 32) ^ 0xD1B5_4A32_9F12_CE77;
    value ^= value >> 12;
    value ^= value << 25;
    value ^= value >> 27;
    value = value.wrapping_mul(0x2545_F491_4F6C_DD1D);
    if value == 0 { 1 } else { value }
}

fn make_xbee_rtt_callback(event_tx: mpsc::Sender<PeerEvent>) -> SerialCallback {
    Arc::new(move |event| match event {
        SerialEvent::Data { bytes, .. } => {
            let _ = event_tx.send(PeerEvent::Data(bytes));
        }
        SerialEvent::Error { message, .. } => {
            let _ = event_tx.send(PeerEvent::Error(message));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_CONNECT_TIMEOUT_MS, DEFAULT_INTERVAL_MS, DEFAULT_PAYLOAD_SIZE, DEFAULT_PROBE_COUNT,
        DEFAULT_PROBE_TIMEOUT_MS, FrameDecoder, FrameKind, HelloAckWire, HelloWire,
        MeasurementSpecWire, MeasurementSummaryWire, ProtocolFrame, TraceKind, WireDirection,
        XbeeRttPortBinding, build_probe_payload, derive_session_id, format_protocol_frame,
        format_trace_chunk, format_wire_bytes, parse_xbee_rtt_args, parse_xbee_rtt_port_binding,
    };

    #[test]
    fn parse_xbee_rtt_args_accepts_one_or_two_ports_and_options() {
        let options = parse_xbee_rtt_args(vec![
            String::from("--port"),
            String::from("/dev/ttyUSB0@921600"),
            String::from("--port"),
            String::from("/dev/ttyUSB1"),
            String::from("--payload-size"),
            String::from("48"),
            String::from("--count"),
            String::from("12"),
            String::from("--interval-ms"),
            String::from("25"),
            String::from("--connect-timeout-ms"),
            String::from("5000"),
            String::from("--probe-timeout-ms"),
            String::from("750"),
        ])
        .expect("should parse");

        assert_eq!(options.ports.len(), 2);
        assert_eq!(
            options.ports[0],
            XbeeRttPortBinding {
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
            }
        );
        assert_eq!(
            options.ports[1],
            XbeeRttPortBinding {
                port: String::from("/dev/ttyUSB1"),
                baud: None,
            }
        );
        assert_eq!(options.payload_size, Some(48));
        assert_eq!(options.probe_count, Some(12));
        assert_eq!(options.interval_ms, Some(25));
        assert_eq!(options.connect_timeout_ms, Some(5_000));
        assert_eq!(options.probe_timeout_ms, Some(750));
        assert!(!options.show_wire);
        assert!(!options.show_protocol);
    }

    #[test]
    fn parse_xbee_rtt_args_accepts_show_wire() {
        let options = parse_xbee_rtt_args(vec![String::from("--show-wire")]).expect("should parse");
        assert!(options.show_wire);
        assert!(!options.show_protocol);
    }

    #[test]
    fn parse_xbee_rtt_args_accepts_show_protocol() {
        let options =
            parse_xbee_rtt_args(vec![String::from("--show-protocol")]).expect("should parse");
        assert!(options.show_protocol);
        assert!(!options.show_wire);
    }

    #[test]
    fn parse_xbee_rtt_args_accepts_config_aliases() {
        let options = parse_xbee_rtt_args(vec![
            String::from("--port"),
            String::from("/dev/ttyUSB0@921600"),
            String::from("--config"),
            String::from(
                "PAYLOAD_SIZE=64,COUNT=20,INTERVAL_MS=50,CONNECT_TIMEOUT_MS=5000,PROBE_TIMEOUT_MS=750",
            ),
        ])
        .expect("should parse");

        assert_eq!(options.ports.len(), 1);
        assert_eq!(options.ports[0].port, "/dev/ttyUSB0");
        assert_eq!(options.ports[0].baud, Some(921_600));
        assert_eq!(options.payload_size, Some(64));
        assert_eq!(options.probe_count, Some(20));
        assert_eq!(options.interval_ms, Some(50));
        assert_eq!(options.connect_timeout_ms, Some(5_000));
        assert_eq!(options.probe_timeout_ms, Some(750));
    }

    #[test]
    fn parse_xbee_rtt_port_binding_rejects_display_override() {
        let error = parse_xbee_rtt_port_binding("/dev/ttyUSB0,hex").expect_err("should reject");
        assert_eq!(
            error,
            "xbee-rtt uses a fixed binary protocol; omit `,DISPLAY` from --port"
        );
    }

    #[test]
    fn protocol_frame_round_trip_survives_noise() {
        let frame = ProtocolFrame {
            kind: FrameKind::Probe,
            session_id: 0x1122_3344,
            round_id: 1,
            seq: 7,
            payload: vec![1, 2, 3, 4, 5],
        };
        let mut stream = vec![0x00, 0xFF, 0x10];
        stream.extend_from_slice(&frame.encode());
        stream.extend_from_slice(&[0x99, 0x88]);

        let mut decoder = FrameDecoder::default();
        let decoded = decoder.push(&stream);

        assert_eq!(decoded, vec![frame]);
    }

    #[test]
    fn wire_structs_round_trip() {
        let spec = MeasurementSpecWire {
            payload_size: 32,
            probe_count: 10,
            interval_ms: 100,
            probe_timeout_ms: 1_000,
        };
        assert_eq!(MeasurementSpecWire::decode(&spec.encode()).unwrap(), spec);

        let hello = HelloWire {
            nonce: 0x1122_3344_5566_7788,
            spec,
        };
        assert_eq!(HelloWire::decode(&hello.encode()).unwrap(), hello);

        let ack = HelloAckWire {
            ack_nonce: 0x8877_6655_4433_2211,
        };
        assert_eq!(HelloAckWire::decode(&ack.encode()).unwrap(), ack);

        let summary = MeasurementSummaryWire {
            probe_count: 10,
            success_count: 9,
            timeout_count: 1,
            mismatch_count: 0,
            payload_size: 32,
            mean_rtt_us: 12_345,
            min_rtt_us: 11_000,
            max_rtt_us: 14_000,
        };
        assert_eq!(
            MeasurementSummaryWire::decode(&summary.encode()).unwrap(),
            summary
        );
    }

    #[test]
    fn build_probe_payload_is_deterministic() {
        assert_eq!(
            build_probe_payload(0x1234_5678, 1, 2, 8),
            build_probe_payload(0x1234_5678, 1, 2, 8)
        );
        assert_ne!(
            build_probe_payload(0x1234_5678, 1, 2, 8),
            build_probe_payload(0x1234_5678, 2, 2, 8)
        );
    }

    #[test]
    fn derive_session_id_is_symmetric() {
        assert_eq!(
            derive_session_id(0x1111, 0x2222),
            derive_session_id(0x2222, 0x1111)
        );
    }

    #[test]
    fn defaults_are_expected() {
        assert_eq!(DEFAULT_PAYLOAD_SIZE, 32);
        assert_eq!(DEFAULT_PROBE_COUNT, 10);
        assert_eq!(DEFAULT_INTERVAL_MS, 100);
        assert_eq!(DEFAULT_CONNECT_TIMEOUT_MS, 3_000);
        assert_eq!(DEFAULT_PROBE_TIMEOUT_MS, 1_000);
    }

    #[test]
    fn format_wire_bytes_is_fixed_hex() {
        assert_eq!(format_wire_bytes(b"OK\r\n"), "4F 4B 0D 0A");
    }

    #[test]
    fn format_wire_bytes_falls_back_to_hex_for_binary() {
        assert_eq!(format_wire_bytes(&[0x58, 0x52, 0x02, 0x00]), "58 52 02 00");
    }

    #[test]
    fn format_trace_chunk_includes_kind_and_local_pair_label() {
        let chunk = format_trace_chunk(TraceKind::Wire, Some("1"), WireDirection::Rx, "01 02");
        assert!(chunk.contains("wire[1<] 01 02"));
        assert!(chunk.starts_with("\u{1b}[31m"));
        assert!(chunk.ends_with("\u{1b}[0m "));
    }

    #[test]
    fn format_protocol_frame_describes_result_summary() {
        let frame = ProtocolFrame {
            kind: FrameKind::Result,
            session_id: 0x1122_3344,
            round_id: 2,
            seq: 0,
            payload: MeasurementSummaryWire {
                probe_count: 10,
                success_count: 9,
                timeout_count: 1,
                mismatch_count: 0,
                payload_size: 32,
                mean_rtt_us: 12_345,
                min_rtt_us: 11_000,
                max_rtt_us: 14_000,
            }
            .encode(),
        };

        assert_eq!(
            format_protocol_frame(&frame),
            "RESULT(session=0x11223344, round=round-2, payload=32B, success=9/10, timeout=1, mismatch=0, avg=12.345 ms, min=11.000 ms, max=14.000 ms)"
        );
    }
}
