use crate::input::compact::CompactReport;

const MANUAL_MODE_VALUE: u8 = 1;
const MANUAL_PACKET_LEN: usize = 39;
const PAYLOAD_LEN: usize = 37;
const TRIGGER_THRESHOLD: u8 = 205;
const STICK_LOW_THRESHOLD: u8 = 25;
const STICK_HIGH_THRESHOLD: u8 = 230;

const CONTROL_BYTE_KBD_PP: u8 = 1 << 0;
const CONTROL_BYTE_KBD_EN: u8 = 1 << 1;
const CONTROL_BYTE_KBD_YAMAN: u8 = 1 << 2;
const CONTROL_BYTE_NYOKKI_PUSH: u8 = 1 << 3;
const CONTROL_BYTE_NYOKKI_PULL: u8 = 1 << 4;
const CONTROL_BYTE_INIT: u8 = 1 << 5;
const CONTROL_BYTE_HOME: u8 = 1 << 6;
const CONTROL_BYTE_KBD_START: u8 = 1 << 7;
const CONTROL_BYTE_MANUAL_UNUSED_BITS: u8 =
    CONTROL_BYTE_KBD_PP | CONTROL_BYTE_KBD_EN | CONTROL_BYTE_KBD_YAMAN | CONTROL_BYTE_KBD_START;

const DEFAULT_HEADER: [u8; 2] = *b"AC";
const DEFAULT_NEUTRAL_CURRENT: u16 = 255;

pub type ManualPacket = [u8; MANUAL_PACKET_LEN];

#[derive(Debug, Clone, Copy)]
pub struct Arm9PacketUpdate {
    pub packet: ManualPacket,
    pub profile: ManualProfile,
    pub profile_changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualProfile {
    Normal,
    Power,
    Sensitive,
}

#[derive(Debug, Clone, Copy)]
pub struct ManualPacketEncoder {
    seq: u8,
    enable: bool,
    profile: ManualProfile,
    previous_options_pressed: bool,
    previous_share_pressed: bool,
    header: [u8; 2],
    neutral_current: u16,
}

#[derive(Debug, Clone, Copy)]
struct ManualConstants {
    base_horizon_positive: u16,
    base_horizon_negative: u16,
    base_roll_positive: u16,
    base_roll_negative: u16,
    pitch1_down: u16,
    pitch1_up: u16,
    pitch2_down: u16,
    pitch2_up: u16,
    pitch3_up: u16,
    pitch3_down: u16,
    roll_positive: u16,
    roll_negative: u16,
    gripper_close: u16,
    gripper_open: u16,
}

impl Default for ManualPacketEncoder {
    fn default() -> Self {
        Self {
            seq: 0,
            enable: false,
            profile: ManualProfile::Normal,
            previous_options_pressed: false,
            previous_share_pressed: false,
            header: DEFAULT_HEADER,
            neutral_current: DEFAULT_NEUTRAL_CURRENT,
        }
    }
}

impl ManualPacketEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode_compact_report_update(&mut self, compact: &CompactReport) -> Arm9PacketUpdate {
        let state = CompactState::new(compact);

        self.update_enable_toggle(state.options_pressed());
        let profile_changed = self.update_profile_toggle(state.share_pressed());

        let control_byte = build_manual_control_byte(
            state.r3_pressed(),
            state.l3_pressed(),
            state.up_pressed(),
            state.down_pressed(),
        );

        let constants = self.profile.constants();
        let mut currents = [
            calc_current(
                state.r2_pressed(),
                state.l2_pressed(),
                constants.base_horizon_positive,
                constants.base_horizon_negative,
                self.neutral_current,
            ),
            calc_current(
                state.r1_pressed(),
                state.l1_pressed(),
                constants.base_roll_positive,
                constants.base_roll_negative,
                self.neutral_current,
            ),
            calc_current(
                state.right_stick_down(),
                state.right_stick_up(),
                constants.pitch1_down,
                constants.pitch1_up,
                self.neutral_current,
            ),
            calc_current(
                state.left_stick_down(),
                state.left_stick_up(),
                constants.pitch2_down,
                constants.pitch2_up,
                self.neutral_current,
            ),
            calc_current(
                state.triangle_pressed(),
                state.cross_pressed(),
                constants.pitch3_up,
                constants.pitch3_down,
                self.neutral_current,
            ),
            calc_current(
                state.circle_pressed(),
                state.square_pressed(),
                constants.roll_positive,
                constants.roll_negative,
                self.neutral_current,
            ),
            calc_current(
                state.right_pressed(),
                state.left_pressed(),
                constants.gripper_close,
                constants.gripper_open,
                self.neutral_current,
            ),
        ];

        if !self.enable {
            currents.fill(self.neutral_current);
        }

        self.seq = self.seq.wrapping_add(1);

        Arm9PacketUpdate {
            packet: build_manual_packet(self.header, self.seq, self.enable, currents, control_byte),
            profile: self.profile,
            profile_changed,
        }
    }

    #[cfg(test)]
    pub fn encode_compact_report(&mut self, compact: &CompactReport) -> ManualPacket {
        self.encode_compact_report_update(compact).packet
    }

    #[cfg(test)]
    fn profile(&self) -> ManualProfile {
        self.profile
    }

    fn update_enable_toggle(&mut self, options_pressed: bool) {
        if options_pressed && !self.previous_options_pressed {
            self.enable = !self.enable;
        }
        self.previous_options_pressed = options_pressed;
    }

    fn update_profile_toggle(&mut self, share_pressed: bool) -> bool {
        let mut profile_changed = false;
        if share_pressed && !self.previous_share_pressed {
            self.profile = self.profile.next();
            profile_changed = true;
        }
        self.previous_share_pressed = share_pressed;
        profile_changed
    }
}

impl ManualProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Power => "power",
            Self::Sensitive => "sensitive",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Normal => Self::Power,
            Self::Power => Self::Sensitive,
            Self::Sensitive => Self::Normal,
        }
    }

    fn constants(self) -> ManualConstants {
        match self {
            Self::Normal => ManualConstants {
                base_horizon_positive: 155,
                base_horizon_negative: 355,
                base_roll_positive: 315,
                base_roll_negative: 205,
                pitch1_down: 230,
                pitch1_up: 280,
                pitch2_down: 225,
                pitch2_up: 275,
                pitch3_up: 210,
                pitch3_down: 400,
                roll_positive: 190,
                roll_negative: 310,
                gripper_close: 155,
                gripper_open: 285,
            },
            Self::Power => ManualConstants {
                base_horizon_positive: 100,
                base_horizon_negative: 400,
                base_roll_positive: 511,
                base_roll_negative: 1,
                pitch1_down: 170,
                pitch1_up: 340,
                pitch2_down: 175,
                pitch2_up: 325,
                pitch3_up: 160,
                pitch3_down: 450,
                roll_positive: 80,
                roll_negative: 430,
                gripper_close: 105,
                gripper_open: 335,
            },
            Self::Sensitive => ManualConstants {
                base_horizon_positive: 180,
                base_horizon_negative: 330,
                base_roll_positive: 295,
                base_roll_negative: 225,
                pitch1_down: 230,
                pitch1_up: 260,
                pitch2_down: 215,
                pitch2_up: 280,
                pitch3_up: 225,
                pitch3_down: 290,
                roll_positive: 210,
                roll_negative: 300,
                gripper_close: 240,
                gripper_open: 270,
            },
        }
    }
}

fn build_manual_control_byte(
    init_pressed: bool,
    home_pressed: bool,
    nyokki_push_pressed: bool,
    nyokki_pull_pressed: bool,
) -> u8 {
    let mut control_byte = 0u8;

    if nyokki_push_pressed {
        control_byte |= CONTROL_BYTE_NYOKKI_PUSH;
    }
    if nyokki_pull_pressed {
        control_byte |= CONTROL_BYTE_NYOKKI_PULL;
    }
    if init_pressed {
        control_byte |= CONTROL_BYTE_INIT;
    }
    if home_pressed {
        control_byte |= CONTROL_BYTE_HOME;
    }

    debug_assert_eq!(control_byte & CONTROL_BYTE_MANUAL_UNUSED_BITS, 0);

    control_byte
}

fn calc_current(
    positive_active: bool,
    negative_active: bool,
    positive_value: u16,
    negative_value: u16,
    neutral_value: u16,
) -> u16 {
    if positive_active {
        positive_value
    } else if negative_active {
        negative_value
    } else {
        neutral_value
    }
}

fn build_manual_packet(
    header: [u8; 2],
    seq: u8,
    enable: bool,
    currents: [u16; 7],
    control_byte: u8,
) -> ManualPacket {
    let mut packet = [0u8; MANUAL_PACKET_LEN];
    let mut cursor = 0usize;

    write_bytes(&mut packet, &mut cursor, &header);

    packet[cursor] = seq;
    cursor += 1;

    let mut flags = (MANUAL_MODE_VALUE & 0x03) << 4;
    if enable {
        flags |= 0x01;
    }
    packet[cursor] = flags;
    cursor += 1;

    for current in currents {
        write_u16_le(&mut packet, &mut cursor, current);
    }

    for _ in 0..3 {
        write_u16_le(&mut packet, &mut cursor, 0);
    }

    for _ in 0..3 {
        write_i16_le(&mut packet, &mut cursor, 0);
    }

    packet[cursor] = control_byte;
    cursor += 1;

    write_i16_le(&mut packet, &mut cursor, 0);
    write_u16_le(&mut packet, &mut cursor, 0);
    write_u16_le(&mut packet, &mut cursor, 0);

    debug_assert_eq!(cursor, PAYLOAD_LEN);

    let crc = crc16_ccitt_false(&packet[..PAYLOAD_LEN]);
    write_u16_le(&mut packet, &mut cursor, crc);

    debug_assert_eq!(cursor, MANUAL_PACKET_LEN);

    packet
}

fn write_bytes(packet: &mut [u8], cursor: &mut usize, bytes: &[u8]) {
    let end = *cursor + bytes.len();
    packet[*cursor..end].copy_from_slice(bytes);
    *cursor = end;
}

fn write_u16_le(packet: &mut [u8], cursor: &mut usize, value: u16) {
    write_bytes(packet, cursor, &value.to_le_bytes());
}

fn write_i16_le(packet: &mut [u8], cursor: &mut usize, value: i16) {
    write_bytes(packet, cursor, &value.to_le_bytes());
}

fn crc16_ccitt_false(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;

    for &byte in data {
        crc ^= u16::from(byte) << 8;

        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }

    crc
}

struct CompactState<'a> {
    report: &'a CompactReport,
}

impl<'a> CompactState<'a> {
    fn new(report: &'a CompactReport) -> Self {
        Self { report }
    }

    fn button0(&self, bit: u8) -> bool {
        (self.report[0] & (1u8 << bit)) != 0
    }

    fn button1(&self, bit: u8) -> bool {
        (self.report[1] & (1u8 << bit)) != 0
    }

    fn analog(&self, index: usize) -> u8 {
        self.report[index]
    }

    fn up_pressed(&self) -> bool {
        self.button0(0)
    }

    fn right_pressed(&self) -> bool {
        self.button0(1)
    }

    fn down_pressed(&self) -> bool {
        self.button0(2)
    }

    fn left_pressed(&self) -> bool {
        self.button0(3)
    }

    fn square_pressed(&self) -> bool {
        self.button0(4)
    }

    fn cross_pressed(&self) -> bool {
        self.button0(5)
    }

    fn circle_pressed(&self) -> bool {
        self.button0(6)
    }

    fn triangle_pressed(&self) -> bool {
        self.button0(7)
    }

    fn l1_pressed(&self) -> bool {
        self.button1(0)
    }

    fn r1_pressed(&self) -> bool {
        self.button1(1)
    }

    fn share_pressed(&self) -> bool {
        self.button1(2)
    }

    fn options_pressed(&self) -> bool {
        self.button1(3)
    }

    fn l3_pressed(&self) -> bool {
        self.button1(4)
    }

    fn r3_pressed(&self) -> bool {
        self.button1(5)
    }

    fn left_stick_down(&self) -> bool {
        self.analog(3) >= STICK_HIGH_THRESHOLD
    }

    fn left_stick_up(&self) -> bool {
        self.analog(3) <= STICK_LOW_THRESHOLD
    }

    fn right_stick_down(&self) -> bool {
        self.analog(5) >= STICK_HIGH_THRESHOLD
    }

    fn right_stick_up(&self) -> bool {
        self.analog(5) <= STICK_LOW_THRESHOLD
    }

    fn l2_pressed(&self) -> bool {
        self.analog(6) >= TRIGGER_THRESHOLD
    }

    fn r2_pressed(&self) -> bool {
        self.analog(7) >= TRIGGER_THRESHOLD
    }
}

#[cfg(test)]
mod tests {
    use super::{ManualPacketEncoder, ManualProfile, crc16_ccitt_false};

    #[test]
    fn disabled_packet_keeps_manual_mode_and_neutral_currents() {
        let mut encoder = ManualPacketEncoder::new();
        let packet = encoder.encode_compact_report(&[0; 8]);

        assert_eq!(&packet[0..2], b"AC");
        assert_eq!(packet[3], 0x10);

        for index in 0..7 {
            assert_eq!(read_u16_le(&packet, 4 + index * 2), 255);
        }

        assert_eq!(packet[30], 0);
        assert_eq!(read_u16_le(&packet, 37), crc16_ccitt_false(&packet[..37]));
    }

    #[test]
    fn options_toggle_enables_manual_currents() {
        let mut encoder = ManualPacketEncoder::new();
        let packet = encoder.encode_compact_report(&[0, 1 << 3, 0, 128, 0, 128, 0, 255]);

        assert_eq!(packet[3], 0x11);
        assert_eq!(read_u16_le(&packet, 4), 155);

        let packet_held = encoder.encode_compact_report(&[0, 1 << 3, 0, 128, 0, 128, 0, 255]);

        assert_eq!(packet_held[3], 0x11);
        assert_eq!(read_u16_le(&packet_held, 4), 155);
    }

    #[test]
    fn share_rising_edge_cycles_profiles() {
        let mut encoder = ManualPacketEncoder::new();

        encoder.encode_compact_report(&[0, 1 << 3, 0, 128, 0, 128, 0, 0]);

        let normal = encoder.encode_compact_report(&[0, 0, 0, 128, 0, 128, 0, 255]);
        assert_eq!(read_u16_le(&normal, 4), 155);

        let power = encoder.encode_compact_report(&[0, 1 << 2, 0, 128, 0, 128, 0, 255]);
        assert_eq!(encoder.profile(), ManualProfile::Power);
        assert_eq!(read_u16_le(&power, 4), 100);

        let power_held = encoder.encode_compact_report(&[0, 1 << 2, 0, 128, 0, 128, 0, 255]);
        assert_eq!(read_u16_le(&power_held, 4), 100);

        encoder.encode_compact_report(&[0, 0, 0, 128, 0, 128, 0, 255]);
        let sensitive = encoder.encode_compact_report(&[0, 1 << 2, 0, 128, 0, 128, 0, 255]);
        assert_eq!(encoder.profile(), ManualProfile::Sensitive);
        assert_eq!(read_u16_le(&sensitive, 4), 180);
    }

    #[test]
    fn control_byte_uses_latest_manual_bits() {
        let mut encoder = ManualPacketEncoder::new();
        let packet =
            encoder.encode_compact_report(&[1 << 0, (1 << 4) | (1 << 5), 0, 128, 0, 128, 0, 0]);

        assert_eq!(packet[30], (1 << 3) | (1 << 5) | (1 << 6));
        assert_eq!(packet[30] & ((1 << 0) | (1 << 1) | (1 << 2) | (1 << 7)), 0);

        let down = encoder.encode_compact_report(&[1 << 2, 0, 0, 128, 0, 128, 0, 0]);

        assert_eq!(down[30], 1 << 4);
    }

    fn read_u16_le(packet: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([packet[offset], packet[offset + 1]])
    }
}
