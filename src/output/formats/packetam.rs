use super::crc::crc16_ccitt_false;

pub(crate) const PACKET_AM_LEN: usize = 4;
const PACKET_AM_HEADER: [u8; 2] = *b"AM";

pub(crate) fn encode_packet_am() -> [u8; PACKET_AM_LEN] {
    let mut packet = [0u8; PACKET_AM_LEN];
    packet[..PACKET_AM_HEADER.len()].copy_from_slice(&PACKET_AM_HEADER);
    packet[PACKET_AM_HEADER.len()..]
        .copy_from_slice(&crc16_ccitt_false(&PACKET_AM_HEADER).to_le_bytes());
    packet
}

#[cfg(test)]
mod tests {
    use super::{PACKET_AM_LEN, encode_packet_am};

    #[test]
    fn packet_am_contains_header_and_little_endian_crc() {
        let packet = encode_packet_am();

        assert_eq!(packet.len(), PACKET_AM_LEN);
        assert_eq!(packet, [0x41, 0x4D, 0x9B, 0xBA]);
    }
}
