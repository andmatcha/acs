use super::definition::{
    PACKET_ACV6_DEFINITION, PACKET_ACV6_MANUAL_MODE_VALUE, PACKET_ACV6_PACKET_LEN,
    PACKET_ACV6_PAYLOAD_LEN, PacketAcV6Profile,
};
use crate::input::compact::CompactReport;
use crate::output::formats::crc::crc16_ccitt_false;

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

pub type PacketAcV6Packet = [u8; PACKET_ACV6_PACKET_LEN];

#[derive(Debug, Clone, Copy)]
pub struct PacketAcV6PacketUpdate {
    pub packet: PacketAcV6Packet,
    pub profile: PacketAcV6Profile,
    pub profile_changed: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PacketAcV6PacketEncoder {
    seq: u8,
    enable: bool,
    profile: PacketAcV6Profile,
    previous_options_pressed: bool,
    previous_share_pressed: bool,
    header: [u8; 2],
    neutral_current: u16,
}

impl Default for PacketAcV6PacketEncoder {
    fn default() -> Self {
        Self {
            seq: 0,
            enable: false,
            profile: PacketAcV6Profile::Normal,
            previous_options_pressed: false,
            previous_share_pressed: false,
            header: PACKET_ACV6_DEFINITION.header,
            neutral_current: PACKET_ACV6_DEFINITION.neutral_current,
        }
    }
}

impl PacketAcV6PacketEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode_compact_report_update(
        &mut self,
        compact: &CompactReport,
    ) -> PacketAcV6PacketUpdate {
        let state = CompactState::new(compact);

        self.update_enable_toggle(state.options_pressed());
        let profile_changed = self.update_profile_toggle(state.share_pressed());

        let control_byte = build_packetacv6_control_byte(
            state.r3_pressed(),
            state.l3_pressed(),
            state.up_pressed(),
            state.down_pressed(),
        );

        let constants = self.profile.definition();
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

        PacketAcV6PacketUpdate {
            packet: build_packetacv6_packet(
                self.header,
                self.seq,
                self.enable,
                currents,
                control_byte,
            ),
            profile: self.profile,
            profile_changed,
        }
    }

    #[cfg(test)]
    pub fn encode_compact_report(&mut self, compact: &CompactReport) -> PacketAcV6Packet {
        self.encode_compact_report_update(compact).packet
    }

    #[cfg(test)]
    fn profile(&self) -> PacketAcV6Profile {
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

fn build_packetacv6_control_byte(
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

fn build_packetacv6_packet(
    header: [u8; 2],
    seq: u8,
    enable: bool,
    currents: [u16; 7],
    control_byte: u8,
) -> PacketAcV6Packet {
    let mut packet = [0u8; PACKET_ACV6_PACKET_LEN];
    let mut cursor = 0usize;

    write_bytes(&mut packet, &mut cursor, &header);

    packet[cursor] = seq;
    cursor += 1;

    let mut flags = (PACKET_ACV6_MANUAL_MODE_VALUE & 0x03) << 4;
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

    debug_assert_eq!(cursor, PACKET_ACV6_PAYLOAD_LEN);

    let crc = crc16_ccitt_false(&packet[..PACKET_ACV6_PAYLOAD_LEN]);
    write_u16_le(&mut packet, &mut cursor, crc);

    debug_assert_eq!(cursor, PACKET_ACV6_PACKET_LEN);

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
        self.analog(3) >= PACKET_ACV6_DEFINITION.thresholds.stick_high
    }

    fn left_stick_up(&self) -> bool {
        self.analog(3) <= PACKET_ACV6_DEFINITION.thresholds.stick_low
    }

    fn right_stick_down(&self) -> bool {
        self.analog(5) >= PACKET_ACV6_DEFINITION.thresholds.stick_high
    }

    fn right_stick_up(&self) -> bool {
        self.analog(5) <= PACKET_ACV6_DEFINITION.thresholds.stick_low
    }

    fn l2_pressed(&self) -> bool {
        self.analog(6) >= PACKET_ACV6_DEFINITION.thresholds.trigger
    }

    fn r2_pressed(&self) -> bool {
        self.analog(7) >= PACKET_ACV6_DEFINITION.thresholds.trigger
    }
}

#[cfg(test)]
mod tests {
    use super::{PacketAcV6PacketEncoder, PacketAcV6Profile, crc16_ccitt_false};

    #[test]
    fn disabled_packet_keeps_manual_mode_and_neutral_currents() {
        let mut encoder = PacketAcV6PacketEncoder::new();
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
        let mut encoder = PacketAcV6PacketEncoder::new();
        let packet = encoder.encode_compact_report(&[0, 1 << 3, 0, 128, 0, 128, 0, 255]);

        assert_eq!(packet[3], 0x11);
        assert_eq!(read_u16_le(&packet, 4), 155);

        let packet_held = encoder.encode_compact_report(&[0, 1 << 3, 0, 128, 0, 128, 0, 255]);

        assert_eq!(packet_held[3], 0x11);
        assert_eq!(read_u16_le(&packet_held, 4), 155);
    }

    #[test]
    fn share_rising_edge_cycles_profiles() {
        let mut encoder = PacketAcV6PacketEncoder::new();

        encoder.encode_compact_report(&[0, 1 << 3, 0, 128, 0, 128, 0, 0]);

        let normal = encoder.encode_compact_report(&[0, 0, 0, 128, 0, 128, 0, 255]);
        assert_eq!(read_u16_le(&normal, 4), 155);

        let power = encoder.encode_compact_report(&[0, 1 << 2, 0, 128, 0, 128, 0, 255]);
        assert_eq!(encoder.profile(), PacketAcV6Profile::Power);
        assert_eq!(read_u16_le(&power, 4), 100);

        let power_held = encoder.encode_compact_report(&[0, 1 << 2, 0, 128, 0, 128, 0, 255]);
        assert_eq!(read_u16_le(&power_held, 4), 100);

        encoder.encode_compact_report(&[0, 0, 0, 128, 0, 128, 0, 255]);
        let sensitive = encoder.encode_compact_report(&[0, 1 << 2, 0, 128, 0, 128, 0, 255]);
        assert_eq!(encoder.profile(), PacketAcV6Profile::Sensitive);
        assert_eq!(read_u16_le(&sensitive, 4), 180);
    }

    #[test]
    fn control_byte_uses_latest_manual_bits() {
        let mut encoder = PacketAcV6PacketEncoder::new();
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
