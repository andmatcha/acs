use crate::input::compact::CompactReport;
use crate::output::formats::crc::crc16_ccitt_false;
use crate::output::formats::{DummyPayloadGenerator, OutputDriver};

const GC_PACKET_LEN: usize = 9;
const GC_PAYLOAD_LEN: usize = 7;

const GC_TYPE_MANUAL_POSITION: u8 = 0x02;
const GC_TYPE_MANUAL_RATE: u8 = 0x03;
const GC_TYPE_STOP: u8 = 0x04;
const GC_TYPE_HOME: u8 = 0x05;

const MANUAL_POSITION_L1_TENTHS: i16 = 2700;
const MANUAL_POSITION_R1_TENTHS: i16 = 0;
const MANUAL_RATE_FULL_SCALE: i16 = 1000;

pub(crate) const fn packet_len() -> usize {
    GC_PACKET_LEN
}

pub(crate) fn create_driver() -> Box<dyn OutputDriver> {
    Box::new(PacketGcV1OutputDriver::default())
}

pub(crate) fn create_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String> {
    Ok(Box::new(PacketGcV1DummyGenerator::default()))
}

#[derive(Debug, Clone, Copy)]
struct GcCommand {
    packet_type: u8,
    value: i16,
}

impl GcCommand {
    const fn manual_position(value: i16) -> Self {
        Self {
            packet_type: GC_TYPE_MANUAL_POSITION,
            value,
        }
    }

    const fn manual_rate(value: i16) -> Self {
        Self {
            packet_type: GC_TYPE_MANUAL_RATE,
            value,
        }
    }

    const fn stop() -> Self {
        Self {
            packet_type: GC_TYPE_STOP,
            value: 0,
        }
    }

    const fn home() -> Self {
        Self {
            packet_type: GC_TYPE_HOME,
            value: 0,
        }
    }
}

#[derive(Debug, Default)]
struct PacketGcV1OutputDriver {
    seq: u16,
    previous_rate_active: bool,
}

impl PacketGcV1OutputDriver {
    fn command_for_report(&self, compact_report: &CompactReport) -> (Option<GcCommand>, bool) {
        let state = CompactGcState::new(compact_report);
        let rate = state.manual_rate_per_mille();
        let rate_active = rate != 0;

        let command = if state.cross_pressed() {
            Some(GcCommand::stop())
        } else if state.circle_pressed() {
            Some(GcCommand::home())
        } else if state.l1_pressed() {
            Some(GcCommand::manual_position(MANUAL_POSITION_L1_TENTHS))
        } else if state.r1_pressed() {
            Some(GcCommand::manual_position(MANUAL_POSITION_R1_TENTHS))
        } else if rate_active {
            Some(GcCommand::manual_rate(rate))
        } else if self.previous_rate_active {
            Some(GcCommand::stop())
        } else {
            None
        };

        (command, rate_active)
    }
}

impl OutputDriver for PacketGcV1OutputDriver {
    fn encode(&mut self, compact_report: &CompactReport) -> Result<Vec<u8>, String> {
        let (command, rate_active) = self.command_for_report(compact_report);
        self.previous_rate_active = rate_active;

        let Some(command) = command else {
            return Ok(Vec::new());
        };

        self.seq = self.seq.wrapping_add(1);
        Ok(build_gc_frame(self.seq, command).to_vec())
    }
}

#[derive(Debug, Default)]
struct PacketGcV1DummyGenerator {
    seq: u16,
    step: u8,
}

impl DummyPayloadGenerator for PacketGcV1DummyGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String> {
        self.seq = self.seq.wrapping_add(1);
        let command = match self.step % 5 {
            0 => GcCommand::manual_position(MANUAL_POSITION_L1_TENTHS),
            1 => GcCommand::manual_position(MANUAL_POSITION_R1_TENTHS),
            2 => GcCommand::manual_rate(MANUAL_RATE_FULL_SCALE),
            3 => GcCommand::stop(),
            _ => GcCommand::home(),
        };
        self.step = self.step.wrapping_add(1);
        Ok(build_gc_frame(self.seq, command).to_vec())
    }
}

fn build_gc_frame(seq: u16, command: GcCommand) -> [u8; GC_PACKET_LEN] {
    let mut frame = [0u8; GC_PACKET_LEN];
    frame[0] = b'G';
    frame[1] = b'C';
    frame[2..4].copy_from_slice(&seq.to_le_bytes());
    frame[4] = command.packet_type;
    frame[5..7].copy_from_slice(&command.value.to_le_bytes());

    let crc = crc16_ccitt_false(&frame[..GC_PAYLOAD_LEN]).to_le_bytes();
    frame[7] = crc[0];
    frame[8] = crc[1];
    frame
}

fn trigger_per_mille(value: u8) -> i16 {
    (((i32::from(value) * i32::from(MANUAL_RATE_FULL_SCALE)) + 127) / 255) as i16
}

struct CompactGcState<'a> {
    report: &'a CompactReport,
}

impl<'a> CompactGcState<'a> {
    fn new(report: &'a CompactReport) -> Self {
        Self { report }
    }

    fn button0(&self, bit: u8) -> bool {
        (self.report[0] & (1u8 << bit)) != 0
    }

    fn button1(&self, bit: u8) -> bool {
        (self.report[1] & (1u8 << bit)) != 0
    }

    fn cross_pressed(&self) -> bool {
        self.button0(5)
    }

    fn circle_pressed(&self) -> bool {
        self.button0(6)
    }

    fn l1_pressed(&self) -> bool {
        self.button1(0)
    }

    fn r1_pressed(&self) -> bool {
        self.button1(1)
    }

    fn manual_rate_per_mille(&self) -> i16 {
        trigger_per_mille(self.report[6]) - trigger_per_mille(self.report[7])
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GC_PACKET_LEN, GC_TYPE_HOME, GC_TYPE_MANUAL_POSITION, GC_TYPE_MANUAL_RATE, GC_TYPE_STOP,
        MANUAL_POSITION_L1_TENTHS, MANUAL_POSITION_R1_TENTHS, PacketGcV1OutputDriver,
        crc16_ccitt_false,
    };
    use crate::output::formats::OutputDriver;

    #[test]
    fn l2_and_r2_encode_signed_manual_rate() {
        let mut driver = PacketGcV1OutputDriver::default();

        let positive = driver
            .encode(&[0, 0, 0, 128, 0, 128, 255, 0])
            .expect("should encode L2");
        let negative = driver
            .encode(&[0, 0, 0, 128, 0, 128, 0, 255])
            .expect("should encode R2");

        assert_gc_packet(&positive, 1, GC_TYPE_MANUAL_RATE, 1000);
        assert_gc_packet(&negative, 2, GC_TYPE_MANUAL_RATE, -1000);
    }

    #[test]
    fn trigger_release_sends_one_stop() {
        let mut driver = PacketGcV1OutputDriver::default();

        let rate = driver
            .encode(&[0, 0, 0, 128, 0, 128, 128, 0])
            .expect("should encode rate");
        let stop = driver
            .encode(&[0, 0, 0, 128, 0, 128, 0, 0])
            .expect("should encode release stop");
        let idle = driver
            .encode(&[0, 0, 0, 128, 0, 128, 0, 0])
            .expect("should ignore repeated idle");

        assert_gc_packet(&rate, 1, GC_TYPE_MANUAL_RATE, 502);
        assert_gc_packet(&stop, 2, GC_TYPE_STOP, 0);
        assert!(idle.is_empty());
    }

    #[test]
    fn supported_buttons_encode_discrete_commands() {
        assert_single_command([1 << 6, 0, 0, 128, 0, 128, 0, 0], GC_TYPE_HOME, 0);
        assert_single_command([1 << 5, 0, 0, 128, 0, 128, 0, 0], GC_TYPE_STOP, 0);
        assert_single_command(
            [0, 1 << 0, 0, 128, 0, 128, 0, 0],
            GC_TYPE_MANUAL_POSITION,
            MANUAL_POSITION_L1_TENTHS,
        );
        assert_single_command(
            [0, 1 << 1, 0, 128, 0, 128, 0, 0],
            GC_TYPE_MANUAL_POSITION,
            MANUAL_POSITION_R1_TENTHS,
        );
    }

    #[test]
    fn unsupported_controls_are_ignored() {
        let mut driver = PacketGcV1OutputDriver::default();
        let packet = driver
            .encode(&[0b1001_1111, 0b1111_1100, 0, 255, 0, 0, 0, 0])
            .expect("should ignore unsupported controls");

        assert!(packet.is_empty());
    }

    #[test]
    fn dummy_payload_generator_advances_sequence() {
        let mut generator = super::create_dummy_generator().expect("should create generator");
        let first = generator.next_payload().expect("should encode");
        let second = generator.next_payload().expect("should encode");

        assert_gc_packet(&first, 1, GC_TYPE_MANUAL_POSITION, 2700);
        assert_gc_packet(&second, 2, GC_TYPE_MANUAL_POSITION, 0);
    }

    fn assert_single_command(report: [u8; 8], packet_type: u8, value: i16) {
        let mut driver = PacketGcV1OutputDriver::default();
        let packet = driver.encode(&report).expect("should encode command");

        assert_gc_packet(&packet, 1, packet_type, value);
    }

    fn assert_gc_packet(packet: &[u8], seq: u16, packet_type: u8, value: i16) {
        assert_eq!(packet.len(), GC_PACKET_LEN);
        assert_eq!(&packet[..2], b"GC");
        assert_eq!(u16::from_le_bytes([packet[2], packet[3]]), seq);
        assert_eq!(packet[4], packet_type);
        assert_eq!(i16::from_le_bytes([packet[5], packet[6]]), value);
        assert_eq!(
            u16::from_le_bytes([packet[7], packet[8]]),
            crc16_ccitt_false(&packet[..7])
        );
    }
}
