use super::common::{
    default_baud_rate, default_log_dir, next_value, parse_key_value_args, parse_port_spec,
    parse_u32_arg,
};
use super::help::{is_help_flag, print_xbee_mock_help};
use super::signal;
use super::xbee_test::{
    BASE_AU_OUTPUT_ID, BASE_POLL_OUTPUT_ID, BASE_PORT_ID, BASE_RU_OUTPUT_ID, DEFAULT_RATE_HZ,
    DISPLAY_FLUSH_PACKET_BUDGET, DISPLAY_FLUSH_SLICE, DisplayedOutput, LOOP_INTERVAL,
    MODEL_BASE_AU_OUTPUT_ID, MODEL_BASE_POLL_OUTPUT_ID, MODEL_BASE_RU_OUTPUT_ID,
    MODEL_REMOTE_AD_OUTPUT_ID, MODEL_REMOTE_POLL_OUTPUT_ID, MODEL_REMOTE_RD_OUTPUT_ID,
    ObservedInput, ObservedInputBatch, ObservedInputKindSpec, PacketMatchStats,
    REMOTE_AD_OUTPUT_ID, REMOTE_POLL_OUTPUT_ID, REMOTE_PORT_ID, REMOTE_RD_OUTPUT_ID,
    ResolvedXbeeTestPort, SPACE_HINT, STATUS_INTERVAL, ScheduledSender, XbeeTestFrameKind,
    XbeeTestMode, xbee_test_format_label,
};
use crate::ingress::IngressFrame;
use crate::output::OutputFormat;
use crate::session::runtime::{SessionInputSpec, SessionOutputSpec, SessionRuntime, SessionSpec};
use crate::{port_display::LineBreakMode, port_display::PortDisplayMode, serial};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

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
    s3b: bool,
}

#[derive(Debug, Clone)]
struct XbeeMockSettings {
    role: XbeeMockRole,
    pair_number: u32,
    uplink_port: ResolvedXbeeTestPort,
    downlink_port: ResolvedXbeeTestPort,
    tx_formats: Vec<XbeeMockTxFormat>,
    rx_formats: Vec<XbeeTestFrameKind>,
    traffic_pattern: XbeeTestMode,
    log_dir: PathBuf,
    logging_enabled: bool,
    s3b: bool,
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
            .map(|(kind, track_expected_packets)| {
                ObservedInputKindSpec::new(*kind, *track_expected_packets)
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

        let batch = self.input.observe(&frame.bytes, Instant::now());
        if batch.valid_packet_count > 0 {
            record_observed_input_batch(session, self.input.input_port(), &batch);
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
                self.input.pending_display_packets(),
                self.output.overflow_packets() + self.input.overflow_display_packets()
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

    match run_with_options(cli_options) {
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

fn run_with_options(cli_options: XbeeMockCliOptions) -> Result<XbeeMockRunResult, String> {
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
        xbee_s3b_recovery: settings.s3b,
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
    let (uplink_binding, downlink_binding) =
        resolve_xbee_mock_ports(pair_number, cli_options.ports)?;
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
    let tx_formats = cli_options
        .tx_formats
        .ok_or_else(|| String::from("xbee-mock requires TX_FORMAT"))?;
    let rx_formats = cli_options
        .rx_formats
        .ok_or_else(|| String::from("xbee-mock requires RX_FORMAT"))?;
    validate_xbee_mock_traffic(role, traffic_pattern, &tx_formats, &rx_formats)?;

    Ok(XbeeMockSettings {
        role,
        pair_number,
        uplink_port,
        downlink_port,
        tx_formats,
        rx_formats,
        traffic_pattern,
        log_dir: cli_options.log_dir.unwrap_or_else(default_log_dir),
        logging_enabled: !cli_options.no_log,
        s3b: cli_options.s3b,
    })
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
            "--config" | "--option" => {
                apply_xbee_mock_config_args(&mut options, &next_value(&mut iter, arg.as_str())?)?;
            }
            "--log-dir" => {
                options.log_dir = Some(PathBuf::from(next_value(&mut iter, "--log-dir")?))
            }
            "--no-log" => options.no_log = true,
            "--s3b" => options.s3b = true,
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

fn apply_xbee_mock_config_args(
    options: &mut XbeeMockCliOptions,
    value: &str,
) -> Result<(), String> {
    for assignment in parse_key_value_args("--config", value)? {
        let key = assignment.key.to_ascii_uppercase();
        let raw_value = assignment.value;
        match key.as_str() {
            "ROLE" => options.role = Some(raw_value),
            "PAIR" => options.pair_number = Some(parse_u32_arg("PAIR", &raw_value)?),
            "TX_FORMAT" => options.tx_formats = Some(parse_xbee_mock_tx_format_list(&raw_value)?),
            "RX_FORMAT" => options.rx_formats = Some(parse_xbee_mock_rx_format_list(&raw_value)?),
            "TRAFFIC_PATTERN" => options.traffic_pattern = Some(raw_value),
            "LOG_DIR" => options.log_dir = Some(PathBuf::from(raw_value)),
            other => return Err(format!("unknown xbee-mock config key: {other}")),
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
                | XbeeTestFrameKind::Format(OutputFormat::PacketAcV6Usb)
                | XbeeTestFrameKind::Format(OutputFormat::PacketMv1)
                | XbeeTestFrameKind::Format(OutputFormat::PacketIv1)
                | XbeeTestFrameKind::Format(OutputFormat::PacketBv1)
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
                | XbeeTestFrameKind::Format(OutputFormat::PacketAcV6Usb)
                | XbeeTestFrameKind::Format(OutputFormat::PacketMv1)
                | XbeeTestFrameKind::Format(OutputFormat::PacketIv1)
                | XbeeTestFrameKind::Format(OutputFormat::PacketBv1)
                | XbeeTestFrameKind::Format(OutputFormat::RoverUpGeneral)
                | XbeeTestFrameKind::PollGreeting
        ),
    }
}

fn record_observed_input_batch(
    session: &mut SessionRuntime,
    input_port: &str,
    batch: &ObservedInputBatch,
) {
    session.record_input_sample(input_port, batch.valid_byte_len, batch.valid_packet_count);
    for (&kind, &(byte_len, packet_count)) in &batch.per_kind_totals {
        session.record_input_format_sample(input_port, kind.display_name(), byte_len, packet_count);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        XbeeMockPortBinding, XbeeMockPortId, XbeeMockRole, parse_xbee_mock_args,
        parse_xbee_mock_port_binding, resolve_xbee_mock_ports,
    };
    use crate::app::cli::xbee_test::XbeeTestFrameKind;
    use crate::output::OutputFormat;

    #[test]
    fn parse_xbee_mock_args_accepts_positional_role_and_port() {
        let options = parse_xbee_mock_args(vec![
            String::from("remote"),
            String::from("-p"),
            String::from("up=/dev/ttyUSB2@460800"),
            String::from("-p"),
            String::from("down=/dev/ttyUSB3@115200"),
            String::from("--config"),
            String::from(
                "PAIR=2,TX_FORMAT=packetjfv1@80+roverdowngeneral@70,RX_FORMAT=packetacv6+roverupgeneral,TRAFFIC_PATTERN=ping-pong,LOG_DIR=tmp/logs",
            ),
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
    fn parse_xbee_mock_args_accepts_role_via_config() {
        let options = parse_xbee_mock_args(vec![
            String::from("--config"),
            String::from(
                "ROLE=base,PAIR=1,TX_FORMAT=packetacv6@100,RX_FORMAT=packetjfv1,TRAFFIC_PATTERN=flood",
            ),
            String::from("-p"),
            String::from("/dev/ttyUSB0@921600"),
        ])
        .expect("should parse");

        assert_eq!(options.role.as_deref(), Some("base"));
        assert_eq!(options.pair_number, Some(1));
        assert_eq!(options.ports.len(), 1);
        assert_eq!(options.ports[0].baud, Some(921_600));
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
}
