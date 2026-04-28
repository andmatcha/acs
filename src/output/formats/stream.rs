use super::{OutputFormat, crc16_ccitt_false};

const ROVER_DOWN_GENERAL_PREFIX_LEN: usize = 6;
const ROVER_DOWN_GENERAL_MAX_PACKET_LEN: usize = 256;

pub(crate) struct MixedFormatDecoder {
    formats: Vec<PacketMatcher>,
    buffer: Vec<u8>,
}

impl MixedFormatDecoder {
    pub(crate) fn new(formats: Vec<OutputFormat>) -> Self {
        Self {
            formats: formats.into_iter().map(PacketMatcher::new).collect(),
            buffer: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<DecodedPacket> {
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
            if let Some(packet_len) = format.matching_packet_len(&self.buffer) {
                return Some(DecodedPacket {
                    format: format.format(),
                    bytes: self.buffer.drain(..packet_len).collect(),
                });
            }
        }

        None
    }
}

pub(crate) struct DecodedPacket {
    pub(crate) format: OutputFormat,
    pub(crate) bytes: Vec<u8>,
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

    fn matching_packet_len(self, bytes: &[u8]) -> Option<usize> {
        if matches!(self, Self::RoverDownGeneral) {
            return rover_down_packet_len(bytes);
        }

        if bytes.len() < self.packet_len() {
            return None;
        }

        self.matches_packet(&bytes[..self.packet_len()])
            .then_some(self.packet_len())
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

pub(crate) fn matches_reduced_ac_packet(bytes: &[u8], header: u8, packet_len: usize) -> bool {
    if bytes.len() != packet_len || bytes.first().copied() != Some(header) {
        return false;
    }

    let payload_len = packet_len - 2;
    let expected_crc = u16::from_le_bytes([bytes[payload_len], bytes[payload_len + 1]]);
    crc16_ccitt_false(&bytes[..payload_len]) == expected_crc
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
    if bytes.len() > ROVER_DOWN_GENERAL_MAX_PACKET_LEN {
        return false;
    }

    if bytes.len() <= ROVER_DOWN_GENERAL_PREFIX_LEN {
        return bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| rover_down_header_byte_matches(index, *byte));
    }

    if !rover_down_header_matches(bytes) {
        return false;
    }

    let data = &bytes[ROVER_DOWN_GENERAL_PREFIX_LEN..];
    if let Some(end_index) = find_crlf(data) {
        let packet_len = ROVER_DOWN_GENERAL_PREFIX_LEN + end_index + 2;
        return packet_len == bytes.len() && matches_rover_down_packet(&bytes[..packet_len]);
    }

    data.iter().enumerate().all(|(index, byte)| match *byte {
        b'\n' => false,
        b'\r' => index + 1 == data.len(),
        _ => true,
    })
}

pub(crate) fn matches_rover_down_packet(bytes: &[u8]) -> bool {
    if bytes.len() < ROVER_DOWN_GENERAL_PREFIX_LEN + 3
        || bytes.len() > ROVER_DOWN_GENERAL_MAX_PACKET_LEN
        || !bytes.ends_with(b"\r\n")
        || !rover_down_header_matches(bytes)
    {
        return false;
    }

    let data = &bytes[ROVER_DOWN_GENERAL_PREFIX_LEN..bytes.len() - 2];
    !data.is_empty() && !data.iter().any(|byte| matches!(*byte, b'\r' | b'\n'))
}

fn rover_down_packet_len(bytes: &[u8]) -> Option<usize> {
    if !rover_down_header_matches(bytes) {
        return None;
    }
    let packet_len =
        ROVER_DOWN_GENERAL_PREFIX_LEN + find_crlf(&bytes[ROVER_DOWN_GENERAL_PREFIX_LEN..])? + 2;
    matches_rover_down_packet(&bytes[..packet_len]).then_some(packet_len)
}

fn rover_down_header_matches(bytes: &[u8]) -> bool {
    bytes.len() >= ROVER_DOWN_GENERAL_PREFIX_LEN
        && bytes[..ROVER_DOWN_GENERAL_PREFIX_LEN]
            .iter()
            .enumerate()
            .all(|(index, byte)| rover_down_header_byte_matches(index, *byte))
}

fn rover_down_header_byte_matches(index: usize, byte: u8) -> bool {
    match index {
        0 => byte == b'0',
        1 => byte == b'x',
        2 => matches!(byte, b'3' | b'4'),
        3 | 4 => byte.is_ascii_hexdigit(),
        5 => byte == b',',
        _ => false,
    }
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\r\n")
}
