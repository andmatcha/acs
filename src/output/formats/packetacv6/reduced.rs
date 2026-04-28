use super::definition::{PACKET_ACV6_DEFINITION, PACKET_ACV6_PACKET_LEN, PACKET_ACV6_PAYLOAD_LEN};
use crate::output::formats::DummyPayloadGenerator;
use crate::output::formats::crc::crc16_ccitt_false;

const AC_PACKET_HEADER_LEN: usize = 2;
const AC_PACKET_CONTROL_MODE_BYTE_INDEX: usize = 3;
const AC_PACKET_CONTROL_MODE_SHIFT: u8 = 4;
const AC_PACKET_CONTROL_MODE_MASK: u8 = 0x03;

const PACKET_MV1_PACKET_LEN: usize = 19;
const PACKET_IV1_PACKET_LEN: usize = 19;
const PACKET_BV1_PACKET_LEN: usize = 15;

const CONTROL_MODE_IK: u8 = 0;
const CONTROL_MODE_MANUAL: u8 = 1;
const CONTROL_MODE_KEYBOARD_AUTO: u8 = 2;

const M_PACKET_DELETE_RANGES: &[AcPacketByteRange] = &[
    AcPacketByteRange { first: 3, last: 3 },
    AcPacketByteRange {
        first: 18,
        last: 29,
    },
    AcPacketByteRange {
        first: 31,
        last: 36,
    },
];

const I_PACKET_DELETE_RANGES: &[AcPacketByteRange] = &[
    AcPacketByteRange { first: 3, last: 3 },
    AcPacketByteRange { first: 8, last: 13 },
    AcPacketByteRange {
        first: 24,
        last: 29,
    },
    AcPacketByteRange {
        first: 31,
        last: 36,
    },
];

const B_PACKET_DELETE_RANGES: &[AcPacketByteRange] = &[
    AcPacketByteRange { first: 3, last: 17 },
    AcPacketByteRange {
        first: 24,
        last: 29,
    },
    AcPacketByteRange {
        first: 35,
        last: 36,
    },
];

const DUMMY_CURRENT_BASES: [u16; 7] = [155, 315, 230, 225, 210, 190, 155];
const DUMMY_CURRENT_STRIDES: [u16; 7] = [7, 11, 13, 17, 19, 23, 29];
const DUMMY_IK_CURRENT_BASES: [u16; 7] = [255, 255, 255, 255, 255, 245, 275];
const DUMMY_ANGLE_BASES: [u16; 3] = [1024, 4096, 8192];
const DUMMY_ANGLE_STRIDES: [u16; 3] = [31, 47, 59];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReducedAcPacketKind {
    PacketMv1,
    PacketIv1,
    PacketBv1,
}

#[derive(Debug, Clone, Copy)]
struct AcPacketByteRange {
    first: usize,
    last: usize,
}

#[derive(Debug, Clone, Copy)]
struct PacketAcV6WireFields {
    seq: u8,
    control_mode: u8,
    enable: bool,
    gripper: bool,
    mission_panel: bool,
    currents: [u16; 7],
    angles: [u16; 3],
    velocities: [i16; 3],
    control_byte: u8,
    base_target_mm_j0: i16,
    auto_flags: u16,
    fault_code: u16,
}

pub(crate) const fn packet_len(kind: ReducedAcPacketKind) -> usize {
    match kind {
        ReducedAcPacketKind::PacketMv1 => PACKET_MV1_PACKET_LEN,
        ReducedAcPacketKind::PacketIv1 => PACKET_IV1_PACKET_LEN,
        ReducedAcPacketKind::PacketBv1 => PACKET_BV1_PACKET_LEN,
    }
}

pub(crate) fn create_dummy_generator(
    kind: ReducedAcPacketKind,
) -> Result<Box<dyn DummyPayloadGenerator>, String> {
    Ok(Box::new(ReducedAcDummyGenerator { kind, step: 0 }))
}

pub(crate) fn reduce_packetacv6_for_kind(
    ac_packet: &[u8],
    kind: ReducedAcPacketKind,
) -> Result<Vec<u8>, String> {
    if ac_packet.len() != PACKET_ACV6_PACKET_LEN {
        return Err(format!(
            "PacketACv6 source length must be {PACKET_ACV6_PACKET_LEN} bytes"
        ));
    }
    if !ac_packet.starts_with(&PACKET_ACV6_DEFINITION.header) {
        return Err(String::from("PacketACv6 source header must be AC"));
    }

    let control_mode = extract_control_mode(ac_packet);
    if control_mode != kind.control_mode() {
        return Err(format!(
            "PacketACv6 source control mode {control_mode} does not match {}",
            kind.display_name()
        ));
    }

    let mut reduced = Vec::with_capacity(packet_len(kind));
    reduced.push(kind.header_byte());
    for index in AC_PACKET_HEADER_LEN..PACKET_ACV6_PAYLOAD_LEN {
        if !kind.is_deleted_index(index) {
            reduced.push(ac_packet[index]);
        }
    }
    append_crc16(&mut reduced);

    debug_assert_eq!(reduced.len(), packet_len(kind));
    Ok(reduced)
}

impl ReducedAcPacketKind {
    const fn display_name(self) -> &'static str {
        match self {
            Self::PacketMv1 => "PacketMv1",
            Self::PacketIv1 => "PacketIv1",
            Self::PacketBv1 => "PacketBv1",
        }
    }

    const fn header_byte(self) -> u8 {
        match self {
            Self::PacketMv1 => b'M',
            Self::PacketIv1 => b'I',
            Self::PacketBv1 => b'B',
        }
    }

    const fn control_mode(self) -> u8 {
        match self {
            Self::PacketMv1 => CONTROL_MODE_MANUAL,
            Self::PacketIv1 => CONTROL_MODE_IK,
            Self::PacketBv1 => CONTROL_MODE_KEYBOARD_AUTO,
        }
    }

    fn delete_ranges(self) -> &'static [AcPacketByteRange] {
        match self {
            Self::PacketMv1 => M_PACKET_DELETE_RANGES,
            Self::PacketIv1 => I_PACKET_DELETE_RANGES,
            Self::PacketBv1 => B_PACKET_DELETE_RANGES,
        }
    }

    fn is_deleted_index(self, index: usize) -> bool {
        self.delete_ranges()
            .iter()
            .any(|range| index >= range.first && index <= range.last)
    }
}

struct ReducedAcDummyGenerator {
    kind: ReducedAcPacketKind,
    step: u8,
}

impl DummyPayloadGenerator for ReducedAcDummyGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String> {
        let source = build_packetacv6_dummy_source(self.kind, self.step);
        let payload = reduce_packetacv6_for_kind(&source, self.kind)?;
        self.step = self.step.wrapping_add(1);
        Ok(payload)
    }
}

fn build_packetacv6_dummy_source(
    kind: ReducedAcPacketKind,
    step: u8,
) -> [u8; PACKET_ACV6_PACKET_LEN] {
    let fields = match kind {
        ReducedAcPacketKind::PacketMv1 => manual_dummy_fields(step),
        ReducedAcPacketKind::PacketIv1 => ik_dummy_fields(step),
        ReducedAcPacketKind::PacketBv1 => keyboard_auto_dummy_fields(step),
    };
    build_packetacv6_source_packet(fields)
}

fn manual_dummy_fields(step: u8) -> PacketAcV6WireFields {
    let step = step as u16;
    PacketAcV6WireFields {
        seq: step.wrapping_add(1) as u8,
        control_mode: CONTROL_MODE_MANUAL,
        enable: true,
        gripper: false,
        mission_panel: false,
        currents: std::array::from_fn(|index| {
            DUMMY_CURRENT_BASES[index].wrapping_add(step.wrapping_mul(DUMMY_CURRENT_STRIDES[index]))
                % 512
        }),
        angles: [0; 3],
        velocities: [0; 3],
        control_byte: dummy_manual_control_byte(step as u8),
        base_target_mm_j0: 0,
        auto_flags: 0,
        fault_code: 0,
    }
}

fn ik_dummy_fields(step: u8) -> PacketAcV6WireFields {
    let step = step as u16;
    PacketAcV6WireFields {
        seq: step.wrapping_add(1) as u8,
        control_mode: CONTROL_MODE_IK,
        enable: true,
        gripper: step % 4 >= 2,
        mission_panel: step % 8 >= 4,
        currents: std::array::from_fn(|index| {
            DUMMY_IK_CURRENT_BASES[index]
                .wrapping_add(step.wrapping_mul(DUMMY_CURRENT_STRIDES[index]))
                % 512
        }),
        angles: dummy_angles(step),
        velocities: [0; 3],
        control_byte: dummy_ik_control_byte(step as u8),
        base_target_mm_j0: 0,
        auto_flags: 0,
        fault_code: 0,
    }
}

fn keyboard_auto_dummy_fields(step: u8) -> PacketAcV6WireFields {
    let step_u16 = step as u16;
    let step_i16 = step as i16;
    PacketAcV6WireFields {
        seq: step.wrapping_add(1),
        control_mode: CONTROL_MODE_KEYBOARD_AUTO,
        enable: true,
        gripper: false,
        mission_panel: true,
        currents: [PACKET_ACV6_DEFINITION.neutral_current; 7],
        angles: dummy_angles(step_u16),
        velocities: [0; 3],
        control_byte: dummy_keyboard_auto_control_byte(step),
        base_target_mm_j0: -120 + step_i16.saturating_mul(3),
        auto_flags: 1u16 << (step % 7),
        fault_code: 0,
    }
}

fn dummy_angles(step: u16) -> [u16; 3] {
    std::array::from_fn(|index| {
        DUMMY_ANGLE_BASES[index].wrapping_add(step.wrapping_mul(DUMMY_ANGLE_STRIDES[index]))
            % 16_384
    })
}

fn dummy_manual_control_byte(step: u8) -> u8 {
    match step % 4 {
        1 => 1 << 3,
        2 => 1 << 4,
        3 => (1 << 5) | (1 << 6),
        _ => 0,
    }
}

fn dummy_ik_control_byte(step: u8) -> u8 {
    if step % 6 == 0 { 1 << 2 } else { 0 }
}

fn dummy_keyboard_auto_control_byte(step: u8) -> u8 {
    let mut control_byte = 1 << 7;
    if step % 5 == 0 {
        control_byte |= 1 << 1;
    }
    control_byte
}

fn build_packetacv6_source_packet(fields: PacketAcV6WireFields) -> [u8; PACKET_ACV6_PACKET_LEN] {
    let mut packet = [0u8; PACKET_ACV6_PACKET_LEN];
    let mut cursor = 0usize;

    write_bytes(&mut packet, &mut cursor, &PACKET_ACV6_DEFINITION.header);

    packet[cursor] = fields.seq;
    cursor += 1;

    packet[cursor] = build_flags(fields);
    cursor += 1;

    for current in fields.currents {
        write_u16_le(&mut packet, &mut cursor, current);
    }

    for angle in fields.angles {
        write_u16_le(&mut packet, &mut cursor, angle);
    }

    for velocity in fields.velocities {
        write_i16_le(&mut packet, &mut cursor, velocity);
    }

    packet[cursor] = fields.control_byte;
    cursor += 1;

    write_i16_le(&mut packet, &mut cursor, fields.base_target_mm_j0);
    write_u16_le(&mut packet, &mut cursor, fields.auto_flags);
    write_u16_le(&mut packet, &mut cursor, fields.fault_code);

    debug_assert_eq!(cursor, PACKET_ACV6_PAYLOAD_LEN);

    let crc = crc16_ccitt_false(&packet[..PACKET_ACV6_PAYLOAD_LEN]);
    write_u16_le(&mut packet, &mut cursor, crc);

    debug_assert_eq!(cursor, PACKET_ACV6_PACKET_LEN);
    packet
}

fn build_flags(fields: PacketAcV6WireFields) -> u8 {
    let mut flags =
        (fields.control_mode & AC_PACKET_CONTROL_MODE_MASK) << AC_PACKET_CONTROL_MODE_SHIFT;
    if fields.enable {
        flags |= 1 << 0;
    }
    if fields.gripper {
        flags |= 1 << 1;
    }
    if fields.mission_panel {
        flags |= 1 << 2;
    }
    flags
}

fn extract_control_mode(ac_packet: &[u8]) -> u8 {
    (ac_packet[AC_PACKET_CONTROL_MODE_BYTE_INDEX] >> AC_PACKET_CONTROL_MODE_SHIFT)
        & AC_PACKET_CONTROL_MODE_MASK
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

fn append_crc16(packet: &mut Vec<u8>) {
    let crc = crc16_ccitt_false(packet);
    packet.extend_from_slice(&crc.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::{
        ReducedAcPacketKind, build_packetacv6_dummy_source, crc16_ccitt_false, packet_len,
        reduce_packetacv6_for_kind,
    };

    #[test]
    fn packetmv1_keeps_documented_manual_bytes() {
        let source = build_packetacv6_dummy_source(ReducedAcPacketKind::PacketMv1, 0);
        let reduced = reduce_packetacv6_for_kind(&source, ReducedAcPacketKind::PacketMv1).unwrap();

        assert_eq!(reduced.len(), packet_len(ReducedAcPacketKind::PacketMv1));
        assert_eq!(reduced[0], b'M');
        assert_eq!(reduced[1], source[2]);
        assert_eq!(&reduced[2..16], &source[4..18]);
        assert_eq!(reduced[16], source[30]);
        assert_eq!(
            u16::from_le_bytes([reduced[17], reduced[18]]),
            crc16_ccitt_false(&reduced[..17])
        );
    }

    #[test]
    fn packetiv1_keeps_documented_ik_bytes() {
        let source = build_packetacv6_dummy_source(ReducedAcPacketKind::PacketIv1, 0);
        let reduced = reduce_packetacv6_for_kind(&source, ReducedAcPacketKind::PacketIv1).unwrap();

        assert_eq!(reduced.len(), packet_len(ReducedAcPacketKind::PacketIv1));
        assert_eq!(reduced[0], b'I');
        assert_eq!(reduced[1], source[2]);
        assert_eq!(&reduced[2..6], &source[4..8]);
        assert_eq!(&reduced[6..10], &source[14..18]);
        assert_eq!(&reduced[10..16], &source[18..24]);
        assert_eq!(reduced[16], source[30]);
        assert_eq!(
            u16::from_le_bytes([reduced[17], reduced[18]]),
            crc16_ccitt_false(&reduced[..17])
        );
    }

    #[test]
    fn packetbv1_keeps_documented_keyboard_auto_bytes() {
        let source = build_packetacv6_dummy_source(ReducedAcPacketKind::PacketBv1, 0);
        let reduced = reduce_packetacv6_for_kind(&source, ReducedAcPacketKind::PacketBv1).unwrap();

        assert_eq!(reduced.len(), packet_len(ReducedAcPacketKind::PacketBv1));
        assert_eq!(reduced[0], b'B');
        assert_eq!(reduced[1], source[2]);
        assert_eq!(&reduced[2..8], &source[18..24]);
        assert_eq!(reduced[8], source[30]);
        assert_eq!(&reduced[9..13], &source[31..35]);
        assert_eq!(
            u16::from_le_bytes([reduced[13], reduced[14]]),
            crc16_ccitt_false(&reduced[..13])
        );
    }

    #[test]
    fn reduction_rejects_wrong_control_mode() {
        let source = build_packetacv6_dummy_source(ReducedAcPacketKind::PacketMv1, 0);

        assert!(
            reduce_packetacv6_for_kind(&source, ReducedAcPacketKind::PacketIv1)
                .expect_err("should reject")
                .contains("does not match PacketIv1")
        );
    }
}
