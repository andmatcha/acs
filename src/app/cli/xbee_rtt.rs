use super::common::{default_baud_rate, next_value, parse_port_spec, parse_u32_arg};
use super::help::{is_help_flag, print_xbee_rtt_help};
use super::signal;
use crate::output::formats::crc16_ccitt_false;
use crate::serial::{
    SerialCallback, SerialConfig, SerialEvent, SerialMonitor, SerialWriter,
    open_monitor_and_writer, resolve_port,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BASE_PORT_ID: &str = "base";
const REMOTE_PORT_ID: &str = "remote";
const DEFAULT_PROBE_COUNT: u16 = 10;
const DEFAULT_PAYLOAD_SIZE: usize = 32;
const DEFAULT_INTERVAL_MS: u32 = 100;
const DEFAULT_CONNECT_TIMEOUT_MS: u32 = 3_000;
const DEFAULT_PROBE_TIMEOUT_MS: u32 = 1_000;
const CONTROL_RETRY_INTERVAL_MS: u64 = 200;
const EVENT_POLL_SLICE_MS: u64 = 20;
const PROTOCOL_VERSION: u8 = 1;
const PROTOCOL_MAGIC: [u8; 2] = *b"XR";
const MAX_FRAME_PAYLOAD_SIZE: usize = 4_096;
const FRAME_FIXED_PREFIX_LEN: usize = 14;
const FRAME_OVERHEAD_BYTES: usize = 16;
const MEASUREMENT_ID_BASE: u8 = 1;
const MEASUREMENT_ID_REMOTE: u8 = 2;

#[derive(Debug, Default)]
struct XbeeRttCliOptions {
    ports: Vec<XbeeRttPortBinding>,
    payload_size: Option<usize>,
    probe_count: Option<u32>,
    interval_ms: Option<u32>,
    connect_timeout_ms: Option<u32>,
    probe_timeout_ms: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XbeeRttPortBinding {
    id: String,
    port: String,
    baud: Option<u32>,
}

#[derive(Debug, Clone)]
struct ResolvedXbeeRttPort {
    port: String,
    baud_rate: u32,
}

#[derive(Debug, Clone)]
struct XbeeRttSettings {
    base_port: ResolvedXbeeRttPort,
    remote_port: ResolvedXbeeRttPort,
    payload_size: usize,
    probe_count: u16,
    interval: Duration,
    interval_ms: u32,
    connect_timeout: Duration,
    connect_timeout_ms: u32,
    probe_timeout: Duration,
    probe_timeout_ms: u32,
}

struct XbeeRttRunResult {
    session_id: u32,
    base_port: ResolvedXbeeRttPort,
    remote_port: ResolvedXbeeRttPort,
    base_hello_nonce: u32,
    remote_hello_nonce: u32,
    starter: NodeId,
    payload_size: usize,
    probe_count: u16,
    interval_ms: u32,
    connect_timeout_ms: u32,
    probe_timeout_ms: u32,
    base_to_remote: MeasurementOutcome,
    remote_to_base: MeasurementOutcome,
    base_received_remote: bool,
    remote_received_base: bool,
}

impl XbeeRttRunResult {
    fn protocol_name(&self) -> &'static str {
        "xbee-rtt/1"
    }

    fn round_trip_wire_bytes(&self) -> usize {
        (FRAME_OVERHEAD_BYTES + self.payload_size) * 2
    }

    fn is_clean(&self) -> bool {
        self.base_to_remote.is_clean()
            && self.remote_to_base.is_clean()
            && self.base_received_remote
            && self.remote_received_base
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MeasurementOutcome {
    initiator: NodeId,
    responder: NodeId,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum NodeId {
    Base,
    Remote,
}

impl NodeId {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            BASE_PORT_ID => Ok(Self::Base),
            REMOTE_PORT_ID => Ok(Self::Remote),
            other => Err(format!(
                "unsupported xbee-rtt port label: {other} (expected `base` or `remote`)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Base => BASE_PORT_ID,
            Self::Remote => REMOTE_PORT_ID,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Base => 1,
            Self::Remote => 2,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Base),
            2 => Some(Self::Remote),
            _ => None,
        }
    }

    fn peer(self) -> Self {
        match self {
            Self::Base => Self::Remote,
            Self::Remote => Self::Base,
        }
    }

    fn measurement_id(self) -> u8 {
        match self {
            Self::Base => MEASUREMENT_ID_BASE,
            Self::Remote => MEASUREMENT_ID_REMOTE,
        }
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
    sender: NodeId,
    measurement_id: u8,
    session_id: u32,
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
        bytes.push(self.sender.as_u8());
        bytes.push(self.measurement_id);
        bytes.extend_from_slice(&self.session_id.to_le_bytes());
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
            let sender = NodeId::from_u8(self.buffer[4]);
            let payload_len = u16::from_le_bytes([self.buffer[12], self.buffer[13]]) as usize;
            if version != PROTOCOL_VERSION
                || kind.is_none()
                || sender.is_none()
                || payload_len > MAX_FRAME_PAYLOAD_SIZE
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
                sender: sender.expect("checked above"),
                measurement_id: self.buffer[5],
                session_id: u32::from_le_bytes([
                    self.buffer[6],
                    self.buffer[7],
                    self.buffer[8],
                    self.buffer[9],
                ]),
                seq: u16::from_le_bytes([self.buffer[10], self.buffer[11]]),
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
                "invalid xbee-rtt measure-start payload length: {}",
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
    initiator: NodeId,
    responder: NodeId,
    payload_size: usize,
    probe_count: u16,
    timeouts: u16,
    mismatches: u16,
    samples: Vec<Duration>,
}

impl MeasurementAccumulator {
    fn new(initiator: NodeId, payload_size: usize, probe_count: u16) -> Self {
        Self {
            initiator,
            responder: initiator.peer(),
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
        let min_rtt = self.samples.iter().copied().min();
        let max_rtt = self.samples.iter().copied().max();

        MeasurementOutcome {
            initiator: self.initiator,
            responder: self.responder,
            payload_size: self.payload_size,
            probe_count: self.probe_count,
            success_count: self.samples.len() as u16,
            timeout_count: self.timeouts,
            mismatch_count: self.mismatches,
            average_rtt,
            min_rtt,
            max_rtt,
        }
    }
}

struct ReceivedProbeEcho {
    measurement_id: u8,
    seq: u16,
    payload: Vec<u8>,
    received_at: Instant,
}

struct Endpoint {
    port: String,
    baud_rate: u32,
    _monitor: SerialMonitor,
    writer: SerialWriter,
    decoder: FrameDecoder,
    hello_nonce: u32,
    peer_nonce: Option<u32>,
    hello_acked: bool,
    measure_acks: BTreeSet<u8>,
    result_acks: BTreeSet<u8>,
    probe_echoes: VecDeque<ReceivedProbeEcho>,
    received_results: BTreeMap<u8, MeasurementSummaryWire>,
    active_measurements: BTreeMap<u8, MeasurementSpecWire>,
}

impl Endpoint {
    fn open(
        id: NodeId,
        port: String,
        baud_rate: u32,
        event_tx: mpsc::Sender<XbeeRttEvent>,
        session_id: u32,
    ) -> Result<Self, String> {
        let callback = make_xbee_rtt_callback(event_tx, id);
        let (monitor, writer) = open_monitor_and_writer(
            &SerialConfig {
                port: port.clone(),
                baud_rate,
            },
            callback,
        )
        .map_err(|error| error.to_string())?;

        Ok(Self {
            port,
            baud_rate,
            _monitor: monitor,
            writer,
            decoder: FrameDecoder::default(),
            hello_nonce: fresh_u32(u64::from(session_id) ^ u64::from(id.as_u8())),
            peer_nonce: None,
            hello_acked: false,
            measure_acks: BTreeSet::new(),
            result_acks: BTreeSet::new(),
            probe_echoes: VecDeque::new(),
            received_results: BTreeMap::new(),
            active_measurements: BTreeMap::new(),
        })
    }

    fn take_probe_echo(&mut self, measurement_id: u8, seq: u16) -> Option<ReceivedProbeEcho> {
        let mut matched = None;
        let mut retained = VecDeque::new();

        while let Some(echo) = self.probe_echoes.pop_front() {
            if echo.measurement_id == measurement_id && echo.seq == seq && matched.is_none() {
                matched = Some(echo);
                continue;
            }

            if echo.measurement_id == measurement_id && echo.seq < seq {
                continue;
            }

            retained.push_back(echo);
        }

        self.probe_echoes = retained;
        matched
    }
}

enum XbeeRttEvent {
    Data { receiver: NodeId, bytes: Vec<u8> },
    Error { receiver: NodeId, message: String },
}

struct XbeeRttCoordinator {
    session_id: u32,
    connect_timeout: Duration,
    control_retry_interval: Duration,
    event_rx: Receiver<XbeeRttEvent>,
    base: Endpoint,
    remote: Endpoint,
}

impl XbeeRttCoordinator {
    fn open(settings: &XbeeRttSettings) -> Result<Self, String> {
        let session_id = fresh_u32(
            u64::from(std::process::id())
                ^ SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
        );
        let (event_tx, event_rx) = mpsc::channel();

        let base = Endpoint::open(
            NodeId::Base,
            settings.base_port.port.clone(),
            settings.base_port.baud_rate,
            event_tx.clone(),
            session_id,
        )?;
        let remote = Endpoint::open(
            NodeId::Remote,
            settings.remote_port.port.clone(),
            settings.remote_port.baud_rate,
            event_tx,
            session_id,
        )?;

        Ok(Self {
            session_id,
            connect_timeout: settings.connect_timeout,
            control_retry_interval: Duration::from_millis(CONTROL_RETRY_INTERVAL_MS),
            event_rx,
            base,
            remote,
        })
    }

    fn run(mut self, settings: &XbeeRttSettings) -> Result<XbeeRttRunResult, String> {
        let starter = self.run_connectivity_check()?;
        let first = self.perform_measurement(starter, settings)?;
        let second = self.perform_measurement(starter.peer(), settings)?;

        let base_to_remote = if first.initiator == NodeId::Base {
            first.clone()
        } else {
            second.clone()
        };
        let remote_to_base = if first.initiator == NodeId::Remote {
            first
        } else {
            second
        };

        let base_received_remote = self
            .base
            .received_results
            .get(&NodeId::Remote.measurement_id())
            .copied()
            == Some(MeasurementSummaryWire::from_outcome(&remote_to_base));
        let remote_received_base = self
            .remote
            .received_results
            .get(&NodeId::Base.measurement_id())
            .copied()
            == Some(MeasurementSummaryWire::from_outcome(&base_to_remote));

        Ok(XbeeRttRunResult {
            session_id: self.session_id,
            base_port: ResolvedXbeeRttPort {
                port: self.base.port,
                baud_rate: self.base.baud_rate,
            },
            remote_port: ResolvedXbeeRttPort {
                port: self.remote.port,
                baud_rate: self.remote.baud_rate,
            },
            base_hello_nonce: self.base.hello_nonce,
            remote_hello_nonce: self.remote.hello_nonce,
            starter,
            payload_size: settings.payload_size,
            probe_count: settings.probe_count,
            interval_ms: settings.interval_ms,
            connect_timeout_ms: settings.connect_timeout_ms,
            probe_timeout_ms: settings.probe_timeout_ms,
            base_to_remote,
            remote_to_base,
            base_received_remote,
            remote_received_base,
        })
    }

    fn run_connectivity_check(&mut self) -> Result<NodeId, String> {
        let deadline = Instant::now() + self.connect_timeout;
        let mut next_base_hello = Instant::now();
        let mut next_remote_hello = Instant::now();

        loop {
            self.ensure_not_interrupted()?;

            if self.base.hello_acked
                && self.remote.hello_acked
                && self.base.peer_nonce == Some(self.remote.hello_nonce)
                && self.remote.peer_nonce == Some(self.base.hello_nonce)
            {
                return Ok(select_starter(
                    self.base.hello_nonce,
                    self.remote.hello_nonce,
                ));
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(String::from(
                    "xbee-rtt connectivity check timed out before both ports acknowledged each other",
                ));
            }

            if !self.base.hello_acked && now >= next_base_hello {
                self.send_hello(NodeId::Base)?;
                next_base_hello = now + self.control_retry_interval;
            }
            if !self.remote.hello_acked && now >= next_remote_hello {
                self.send_hello(NodeId::Remote)?;
                next_remote_hello = now + self.control_retry_interval;
            }

            let mut next_action = deadline;
            if next_base_hello < next_action {
                next_action = next_base_hello;
            }
            if next_remote_hello < next_action {
                next_action = next_remote_hello;
            }
            self.pump_until(next_action)?;
        }
    }

    fn perform_measurement(
        &mut self,
        initiator: NodeId,
        settings: &XbeeRttSettings,
    ) -> Result<MeasurementOutcome, String> {
        let measurement_id = initiator.measurement_id();
        let spec = MeasurementSpecWire {
            payload_size: settings.payload_size as u16,
            probe_count: settings.probe_count,
            interval_ms: settings.interval_ms,
            probe_timeout_ms: settings.probe_timeout_ms,
        };

        self.endpoint_mut(initiator)
            .measure_acks
            .remove(&measurement_id);
        self.endpoint_mut(initiator)
            .result_acks
            .remove(&measurement_id);
        self.wait_for_measure_ack(initiator, spec)?;

        let mut next_probe_at = Instant::now();
        let mut accumulator =
            MeasurementAccumulator::new(initiator, settings.payload_size, settings.probe_count);

        for probe_index in 0..settings.probe_count {
            self.wait_until(next_probe_at)?;

            let seq = probe_index.saturating_add(1);
            let payload =
                build_probe_payload(self.session_id, measurement_id, seq, settings.payload_size);
            let sent_at = Instant::now();
            self.send_probe(initiator, measurement_id, seq, payload.clone())?;

            match self.wait_for_probe_echo(
                initiator,
                measurement_id,
                seq,
                sent_at,
                settings.probe_timeout,
            )? {
                Some(echo) if echo.payload == payload => {
                    accumulator.record_success(echo.received_at.duration_since(sent_at));
                }
                Some(_) => {
                    accumulator.record_mismatch();
                }
                None => {
                    accumulator.record_timeout();
                }
            }

            next_probe_at = sent_at + settings.interval;
        }

        let outcome = accumulator.finish();
        self.wait_for_result_ack(initiator, MeasurementSummaryWire::from_outcome(&outcome))?;
        Ok(outcome)
    }

    fn wait_for_measure_ack(
        &mut self,
        initiator: NodeId,
        spec: MeasurementSpecWire,
    ) -> Result<(), String> {
        let measurement_id = initiator.measurement_id();
        let deadline = Instant::now() + self.connect_timeout;
        let mut next_send_at = Instant::now();

        loop {
            self.ensure_not_interrupted()?;
            if self
                .endpoint(initiator)
                .measure_acks
                .contains(&measurement_id)
            {
                self.endpoint_mut(initiator)
                    .measure_acks
                    .remove(&measurement_id);
                return Ok(());
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "xbee-rtt failed to synchronize {} -> {} measurement start within {} ms",
                    initiator.as_str(),
                    initiator.peer().as_str(),
                    self.connect_timeout.as_millis()
                ));
            }

            if now >= next_send_at {
                self.send_measure_start(initiator, spec)?;
                next_send_at = now + self.control_retry_interval;
            }

            let next_action = next_send_at.min(deadline);
            self.pump_until(next_action)?;
        }
    }

    fn wait_for_probe_echo(
        &mut self,
        initiator: NodeId,
        measurement_id: u8,
        seq: u16,
        _sent_at: Instant,
        probe_timeout: Duration,
    ) -> Result<Option<ReceivedProbeEcho>, String> {
        let deadline = Instant::now() + probe_timeout;

        loop {
            self.ensure_not_interrupted()?;
            if let Some(echo) = self
                .endpoint_mut(initiator)
                .take_probe_echo(measurement_id, seq)
            {
                return Ok(Some(echo));
            }

            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }

            self.pump_until(deadline)?;
        }
    }

    fn wait_for_result_ack(
        &mut self,
        initiator: NodeId,
        summary: MeasurementSummaryWire,
    ) -> Result<(), String> {
        let measurement_id = initiator.measurement_id();
        let deadline = Instant::now() + self.connect_timeout;
        let mut next_send_at = Instant::now();

        loop {
            self.ensure_not_interrupted()?;
            if self
                .endpoint(initiator)
                .result_acks
                .contains(&measurement_id)
            {
                self.endpoint_mut(initiator)
                    .result_acks
                    .remove(&measurement_id);
                return Ok(());
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "xbee-rtt failed to exchange {} -> {} measurement result within {} ms",
                    initiator.as_str(),
                    initiator.peer().as_str(),
                    self.connect_timeout.as_millis()
                ));
            }

            if now >= next_send_at {
                self.send_result(initiator, summary)?;
                next_send_at = now + self.control_retry_interval;
            }

            let next_action = next_send_at.min(deadline);
            self.pump_until(next_action)?;
        }
    }

    fn wait_until(&mut self, deadline: Instant) -> Result<(), String> {
        while Instant::now() < deadline {
            self.ensure_not_interrupted()?;
            self.pump_until(deadline)?;
        }
        Ok(())
    }

    fn pump_until(&mut self, deadline: Instant) -> Result<(), String> {
        let now = Instant::now();
        if now >= deadline {
            return Ok(());
        }

        let timeout = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(EVENT_POLL_SLICE_MS));
        self.pump_once(timeout)
    }

    fn pump_once(&mut self, timeout: Duration) -> Result<(), String> {
        match self.event_rx.recv_timeout(timeout) {
            Ok(XbeeRttEvent::Data { receiver, bytes }) => {
                let frames = self.endpoint_mut(receiver).decoder.push(&bytes);
                for frame in frames {
                    self.handle_frame(receiver, frame)?;
                }
                Ok(())
            }
            Ok(XbeeRttEvent::Error { receiver, message }) => Err(format!(
                "serial input error on {} ({}): {message}",
                receiver.as_str(),
                self.endpoint(receiver).port
            )),
            Err(RecvTimeoutError::Timeout) => Ok(()),
            Err(RecvTimeoutError::Disconnected) => {
                Err(String::from("xbee-rtt internal event channel disconnected"))
            }
        }
    }

    fn handle_frame(&mut self, receiver: NodeId, frame: ProtocolFrame) -> Result<(), String> {
        if frame.session_id != self.session_id {
            return Ok(());
        }
        if frame.sender != receiver.peer() {
            return Ok(());
        }

        match frame.kind {
            FrameKind::Hello => {
                let nonce = decode_u32_payload(&frame.payload, "hello")?;
                self.endpoint_mut(receiver).peer_nonce = Some(nonce);
                self.send_frame(
                    receiver,
                    ProtocolFrame {
                        kind: FrameKind::HelloAck,
                        sender: receiver,
                        measurement_id: 0,
                        session_id: self.session_id,
                        seq: 0,
                        payload: frame.payload,
                    },
                )
            }
            FrameKind::HelloAck => {
                let nonce = decode_u32_payload(&frame.payload, "hello-ack")?;
                if nonce == self.endpoint(receiver).hello_nonce {
                    self.endpoint_mut(receiver).hello_acked = true;
                }
                Ok(())
            }
            FrameKind::MeasureStart => {
                let spec = MeasurementSpecWire::decode(&frame.payload)?;
                self.endpoint_mut(receiver)
                    .active_measurements
                    .insert(frame.measurement_id, spec);
                self.send_empty(receiver, FrameKind::MeasureAck, frame.measurement_id, 0)
            }
            FrameKind::MeasureAck => {
                self.endpoint_mut(receiver)
                    .measure_acks
                    .insert(frame.measurement_id);
                Ok(())
            }
            FrameKind::Probe => {
                let payload_size_matches = self
                    .endpoint(receiver)
                    .active_measurements
                    .get(&frame.measurement_id)
                    .map(|spec| usize::from(spec.payload_size) == frame.payload.len())
                    .unwrap_or(false);
                if payload_size_matches {
                    self.send_frame(
                        receiver,
                        ProtocolFrame {
                            kind: FrameKind::ProbeEcho,
                            sender: receiver,
                            measurement_id: frame.measurement_id,
                            session_id: self.session_id,
                            seq: frame.seq,
                            payload: frame.payload,
                        },
                    )?;
                }
                Ok(())
            }
            FrameKind::ProbeEcho => {
                self.endpoint_mut(receiver)
                    .probe_echoes
                    .push_back(ReceivedProbeEcho {
                        measurement_id: frame.measurement_id,
                        seq: frame.seq,
                        payload: frame.payload,
                        received_at: Instant::now(),
                    });
                Ok(())
            }
            FrameKind::Result => {
                let summary = MeasurementSummaryWire::decode(&frame.payload)?;
                self.endpoint_mut(receiver)
                    .received_results
                    .insert(frame.measurement_id, summary);
                self.endpoint_mut(receiver)
                    .active_measurements
                    .remove(&frame.measurement_id);
                self.send_empty(receiver, FrameKind::ResultAck, frame.measurement_id, 0)
            }
            FrameKind::ResultAck => {
                self.endpoint_mut(receiver)
                    .result_acks
                    .insert(frame.measurement_id);
                Ok(())
            }
        }
    }

    fn send_hello(&mut self, sender: NodeId) -> Result<(), String> {
        let nonce = self.endpoint(sender).hello_nonce;
        self.send_frame(
            sender,
            ProtocolFrame {
                kind: FrameKind::Hello,
                sender,
                measurement_id: 0,
                session_id: self.session_id,
                seq: 0,
                payload: nonce.to_le_bytes().to_vec(),
            },
        )
    }

    fn send_measure_start(
        &mut self,
        sender: NodeId,
        spec: MeasurementSpecWire,
    ) -> Result<(), String> {
        self.send_frame(
            sender,
            ProtocolFrame {
                kind: FrameKind::MeasureStart,
                sender,
                measurement_id: sender.measurement_id(),
                session_id: self.session_id,
                seq: 0,
                payload: spec.encode(),
            },
        )
    }

    fn send_probe(
        &mut self,
        sender: NodeId,
        measurement_id: u8,
        seq: u16,
        payload: Vec<u8>,
    ) -> Result<(), String> {
        self.send_frame(
            sender,
            ProtocolFrame {
                kind: FrameKind::Probe,
                sender,
                measurement_id,
                session_id: self.session_id,
                seq,
                payload,
            },
        )
    }

    fn send_result(
        &mut self,
        sender: NodeId,
        summary: MeasurementSummaryWire,
    ) -> Result<(), String> {
        self.send_frame(
            sender,
            ProtocolFrame {
                kind: FrameKind::Result,
                sender,
                measurement_id: sender.measurement_id(),
                session_id: self.session_id,
                seq: 0,
                payload: summary.encode(),
            },
        )
    }

    fn send_empty(
        &mut self,
        sender: NodeId,
        kind: FrameKind,
        measurement_id: u8,
        seq: u16,
    ) -> Result<(), String> {
        self.send_frame(
            sender,
            ProtocolFrame {
                kind,
                sender,
                measurement_id,
                session_id: self.session_id,
                seq,
                payload: Vec::new(),
            },
        )
    }

    fn send_frame(&mut self, sender: NodeId, frame: ProtocolFrame) -> Result<(), String> {
        self.endpoint_mut(sender)
            .writer
            .write_bytes(&frame.encode())
            .map_err(|error| error.to_string())
    }

    fn ensure_not_interrupted(&self) -> Result<(), String> {
        if signal::is_stop_requested() {
            Err(String::from("xbee-rtt interrupted by Ctrl-C"))
        } else {
            Ok(())
        }
    }

    fn endpoint(&self, node: NodeId) -> &Endpoint {
        match node {
            NodeId::Base => &self.base,
            NodeId::Remote => &self.remote,
        }
    }

    fn endpoint_mut(&mut self, node: NodeId) -> &mut Endpoint {
        match node {
            NodeId::Base => &mut self.base,
            NodeId::Remote => &mut self.remote,
        }
    }
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
    signal::install_handler();
    let coordinator = XbeeRttCoordinator::open(&settings)?;
    coordinator.run(&settings)
}

fn print_run_result(result: &XbeeRttRunResult) {
    println!(
        "status={} protocol={} session=0x{:08x} connectivity=ok starter={}",
        if result.is_clean() { "ok" } else { "degraded" },
        result.protocol_name(),
        result.session_id,
        result.starter.as_str()
    );
    println!(
        "base={}@{} hello=0x{:08x}",
        result.base_port.port, result.base_port.baud_rate, result.base_hello_nonce
    );
    println!(
        "remote={}@{} hello=0x{:08x}",
        result.remote_port.port, result.remote_port.baud_rate, result.remote_hello_nonce
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
    print_measurement_result(&result.base_to_remote, result.remote_received_base);
    print_measurement_result(&result.remote_to_base, result.base_received_remote);
    println!(
        "shared-results base<=remote={} remote<=base={}",
        if result.base_received_remote {
            "ok"
        } else {
            "missing"
        },
        if result.remote_received_base {
            "ok"
        } else {
            "missing"
        }
    );
}

fn print_measurement_result(outcome: &MeasurementOutcome, shared_with_peer: bool) {
    println!(
        "{}->{} avg={} min={} max={} success={}/{} timeout={} mismatch={} shared={}",
        outcome.initiator.as_str(),
        outcome.responder.as_str(),
        outcome.average_rtt_display(),
        outcome.min_rtt_display(),
        outcome.max_rtt_display(),
        outcome.success_count,
        outcome.probe_count,
        outcome.timeout_count,
        outcome.mismatch_count,
        if shared_with_peer { "ok" } else { "missing" }
    );
}

fn build_settings(cli_options: XbeeRttCliOptions) -> Result<XbeeRttSettings, String> {
    let mut ports = BTreeMap::new();
    for binding in cli_options.ports {
        ports.insert(
            binding.id.clone(),
            XbeeRttPortBinding {
                id: binding.id,
                port: binding.port,
                baud: binding.baud,
            },
        );
    }

    let base_binding = ports.remove(BASE_PORT_ID).ok_or_else(|| {
        String::from("xbee-rtt requires `--port base=PORT` (optional `@BAUD`, default: 115200)")
    })?;
    let remote_binding = ports.remove(REMOTE_PORT_ID).ok_or_else(|| {
        String::from("xbee-rtt requires `--port remote=PORT` (optional `@BAUD`, default: 115200)")
    })?;

    let base_port = resolve_port(Some(&base_binding.port)).map_err(|error| error.to_string())?;
    let remote_port =
        resolve_port(Some(&remote_binding.port)).map_err(|error| error.to_string())?;
    if base_port == remote_port {
        return Err(String::from(
            "xbee-rtt requires different ports for `base` and `remote`",
        ));
    }

    let payload_size = cli_options.payload_size.unwrap_or(DEFAULT_PAYLOAD_SIZE);
    if payload_size > MAX_FRAME_PAYLOAD_SIZE {
        return Err(format!(
            "--payload-size must be <= {MAX_FRAME_PAYLOAD_SIZE}"
        ));
    }

    let probe_count = cli_options
        .probe_count
        .unwrap_or(u32::from(DEFAULT_PROBE_COUNT));
    if probe_count == 0 {
        return Err(String::from("--count must be greater than 0"));
    }
    if probe_count > u32::from(u16::MAX) {
        return Err(format!("--count must be <= {}", u16::MAX));
    }

    let interval_ms = cli_options.interval_ms.unwrap_or(DEFAULT_INTERVAL_MS);
    let connect_timeout_ms = cli_options
        .connect_timeout_ms
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS);
    if connect_timeout_ms == 0 {
        return Err(String::from("--connect-timeout-ms must be greater than 0"));
    }

    let probe_timeout_ms = cli_options
        .probe_timeout_ms
        .unwrap_or(DEFAULT_PROBE_TIMEOUT_MS);
    if probe_timeout_ms == 0 {
        return Err(String::from("--probe-timeout-ms must be greater than 0"));
    }

    Ok(XbeeRttSettings {
        base_port: ResolvedXbeeRttPort {
            port: base_port,
            baud_rate: base_binding.baud.unwrap_or_else(default_baud_rate),
        },
        remote_port: ResolvedXbeeRttPort {
            port: remote_port,
            baud_rate: remote_binding.baud.unwrap_or_else(default_baud_rate),
        },
        payload_size,
        probe_count: probe_count as u16,
        interval: Duration::from_millis(u64::from(interval_ms)),
        interval_ms,
        connect_timeout: Duration::from_millis(u64::from(connect_timeout_ms)),
        connect_timeout_ms,
        probe_timeout: Duration::from_millis(u64::from(probe_timeout_ms)),
        probe_timeout_ms,
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
            other => return Err(format!("unknown option for xbee-rtt: {other}")),
        }
    }

    Ok(options)
}

fn parse_xbee_rtt_port_binding(value: &str) -> Result<XbeeRttPortBinding, String> {
    let Some((id, port_text)) = value.split_once('=') else {
        return Err(format!(
            "invalid xbee-rtt port binding: {value} (expected base=PORT or remote=PORT; append `@BAUD` to override 115200)"
        ));
    };
    let node = NodeId::parse(id)?;
    let port_spec = parse_port_spec("xbee-rtt port binding", port_text)?;
    if port_spec.display_mode.is_some() || port_spec.line_break_mode.is_some() {
        return Err(String::from(
            "xbee-rtt uses a fixed binary protocol; omit `,DISPLAY` from --port",
        ));
    }

    Ok(XbeeRttPortBinding {
        id: String::from(node.as_str()),
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

fn decode_u32_payload(payload: &[u8], label: &str) -> Result<u32, String> {
    if payload.len() != 4 {
        return Err(format!(
            "invalid xbee-rtt {label} payload length: {}",
            payload.len()
        ));
    }
    Ok(u32::from_le_bytes([
        payload[0], payload[1], payload[2], payload[3],
    ]))
}

fn build_probe_payload(
    session_id: u32,
    measurement_id: u8,
    seq: u16,
    payload_size: usize,
) -> Vec<u8> {
    let mut state = (u64::from(session_id) << 16)
        ^ (u64::from(measurement_id) << 8)
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

fn select_starter(base_nonce: u32, remote_nonce: u32) -> NodeId {
    if base_nonce <= remote_nonce {
        NodeId::Base
    } else {
        NodeId::Remote
    }
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

fn fresh_u32(extra: u64) -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut value = nanos ^ extra ^ ((std::process::id() as u64) << 32) ^ 0xA5A5_5A5A_C3C3_3C3C;
    value ^= value >> 12;
    value ^= value << 25;
    value ^= value >> 27;
    value = value.wrapping_mul(0x2545_F491_4F6C_DD1D);
    value as u32
}

fn make_xbee_rtt_callback(
    event_tx: mpsc::Sender<XbeeRttEvent>,
    receiver: NodeId,
) -> SerialCallback {
    Arc::new(move |event| match event {
        SerialEvent::Data { bytes, .. } => {
            let _ = event_tx.send(XbeeRttEvent::Data { receiver, bytes });
        }
        SerialEvent::Error { message, .. } => {
            let _ = event_tx.send(XbeeRttEvent::Error { receiver, message });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_CONNECT_TIMEOUT_MS, DEFAULT_INTERVAL_MS, DEFAULT_PAYLOAD_SIZE, DEFAULT_PROBE_COUNT,
        DEFAULT_PROBE_TIMEOUT_MS, FrameDecoder, FrameKind, MeasurementSpecWire,
        MeasurementSummaryWire, NodeId, ProtocolFrame, XbeeRttPortBinding, build_probe_payload,
        parse_xbee_rtt_args, parse_xbee_rtt_port_binding, select_starter,
    };

    #[test]
    fn parse_xbee_rtt_args_accepts_ports_and_options() {
        let options = parse_xbee_rtt_args(vec![
            String::from("--port"),
            String::from("base=/dev/ttyUSB0@921600"),
            String::from("--port"),
            String::from("remote=/dev/ttyUSB1"),
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
                id: String::from("base"),
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
            }
        );
        assert_eq!(
            options.ports[1],
            XbeeRttPortBinding {
                id: String::from("remote"),
                port: String::from("/dev/ttyUSB1"),
                baud: None,
            }
        );
        assert_eq!(options.payload_size, Some(48));
        assert_eq!(options.probe_count, Some(12));
        assert_eq!(options.interval_ms, Some(25));
        assert_eq!(options.connect_timeout_ms, Some(5_000));
        assert_eq!(options.probe_timeout_ms, Some(750));
    }

    #[test]
    fn parse_xbee_rtt_port_binding_rejects_display_override() {
        let error =
            parse_xbee_rtt_port_binding("base=/dev/ttyUSB0,hex").expect_err("should reject");
        assert_eq!(
            error,
            "xbee-rtt uses a fixed binary protocol; omit `,DISPLAY` from --port"
        );
    }

    #[test]
    fn protocol_frame_round_trip_survives_noise() {
        let frame = ProtocolFrame {
            kind: FrameKind::Probe,
            sender: NodeId::Base,
            measurement_id: 1,
            session_id: 0x1122_3344,
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
    fn measurement_wire_structs_round_trip() {
        let spec = MeasurementSpecWire {
            payload_size: 32,
            probe_count: 10,
            interval_ms: 100,
            probe_timeout_ms: 1_000,
        };
        assert_eq!(MeasurementSpecWire::decode(&spec.encode()).unwrap(), spec);

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
            build_probe_payload(0x1234_5678, 1, 3, 8)
        );
    }

    #[test]
    fn select_starter_uses_nonce_and_tie_breaker() {
        assert_eq!(select_starter(10, 20), NodeId::Base);
        assert_eq!(select_starter(30, 20), NodeId::Remote);
        assert_eq!(select_starter(42, 42), NodeId::Base);
    }

    #[test]
    fn defaults_are_expected() {
        assert_eq!(DEFAULT_PAYLOAD_SIZE, 32);
        assert_eq!(DEFAULT_PROBE_COUNT, 10);
        assert_eq!(DEFAULT_INTERVAL_MS, 100);
        assert_eq!(DEFAULT_CONNECT_TIMEOUT_MS, 3_000);
        assert_eq!(DEFAULT_PROBE_TIMEOUT_MS, 1_000);
    }
}
