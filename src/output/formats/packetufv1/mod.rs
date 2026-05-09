use crate::output::formats::DummyPayloadGenerator;
use crate::output::formats::crc::crc16_ccitt_false;

const UF_PACKET_LEN: usize = 14;
const UF_PAYLOAD_LEN: usize = 12;
const UF_DUMMY_SEQ: u8 = 0x01;
const UF_DUMMY_FLAGS: u8 = 0x03;
const UF_DUMMY_LAT_E7: i32 = 356_812_362;
const UF_DUMMY_LON_E7: i32 = 1_397_671_248;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PacketUfV1Packet {
    pub(crate) seq: u8,
    pub(crate) flags: u8,
    pub(crate) lat_e7: i32,
    pub(crate) lon_e7: i32,
}

impl PacketUfV1Packet {
    fn lat_degrees(self) -> f64 {
        self.lat_e7 as f64 / 10_000_000.0
    }

    fn lon_degrees(self) -> f64 {
        self.lon_e7 as f64 / 10_000_000.0
    }

    fn flag_enabled(self, bit: u8) -> bool {
        self.flags & (1 << bit) != 0
    }
}

pub(crate) const fn packet_len() -> usize {
    UF_PACKET_LEN
}

pub(crate) fn create_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String> {
    Ok(Box::new(PacketUfV1DummyGenerator::default()))
}

fn build_uf_frame(seq: u8, flags: u8, lat_e7: i32, lon_e7: i32) -> [u8; UF_PACKET_LEN] {
    let mut frame = [0u8; UF_PACKET_LEN];
    frame[0] = b'U';
    frame[1] = b'F';
    frame[2] = seq;
    frame[3] = flags;
    frame[4..8].copy_from_slice(&lat_e7.to_le_bytes());
    frame[8..12].copy_from_slice(&lon_e7.to_le_bytes());

    let crc = crc16_ccitt_false(&frame[..UF_PAYLOAD_LEN]).to_le_bytes();
    frame[12] = crc[0];
    frame[13] = crc[1];
    frame
}

pub(crate) fn decode_packet(packet: &[u8]) -> Result<PacketUfV1Packet, String> {
    if packet.len() != UF_PACKET_LEN {
        return Err(format!(
            "PacketUFv1 length must be {UF_PACKET_LEN} bytes, got {}",
            packet.len()
        ));
    }
    if !packet.starts_with(b"UF") {
        return Err(String::from("PacketUFv1 header must be UF"));
    }

    let actual_crc = u16::from_le_bytes([packet[12], packet[13]]);
    let expected_crc = crc16_ccitt_false(&packet[..UF_PAYLOAD_LEN]);
    if actual_crc != expected_crc {
        return Err(format!(
            "PacketUFv1 CRC mismatch: expected 0x{expected_crc:04X}, got 0x{actual_crc:04X}"
        ));
    }

    Ok(PacketUfV1Packet {
        seq: packet[2],
        flags: packet[3],
        lat_e7: i32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]),
        lon_e7: i32::from_le_bytes([packet[8], packet[9], packet[10], packet[11]]),
    })
}

pub(crate) fn format_decoded_packet(packet: &[u8]) -> Result<String, String> {
    let packet = decode_packet(packet)?;
    let reserved = packet.flags >> 4;
    Ok(format!(
        "UF seq={} flags=0x{:02X}(valid={}, usb_present={}, read_busy={}, read_error={}, reserved=0x{:X}) lat={:.7} lon={:.7} lat_e7={} lon_e7={}",
        packet.seq,
        packet.flags,
        flag_value(packet.flag_enabled(0)),
        flag_value(packet.flag_enabled(1)),
        flag_value(packet.flag_enabled(2)),
        flag_value(packet.flag_enabled(3)),
        reserved,
        packet.lat_degrees(),
        packet.lon_degrees(),
        packet.lat_e7,
        packet.lon_e7
    ))
}

fn flag_value(enabled: bool) -> u8 {
    u8::from(enabled)
}

#[derive(Default)]
struct PacketUfV1DummyGenerator {
    step: u8,
}

impl DummyPayloadGenerator for PacketUfV1DummyGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String> {
        let step = self.step as i32;
        let payload = build_uf_frame(
            UF_DUMMY_SEQ.wrapping_add(self.step),
            UF_DUMMY_FLAGS,
            UF_DUMMY_LAT_E7.saturating_add(step * 10),
            UF_DUMMY_LON_E7.saturating_add(step * 10),
        )
        .to_vec();
        self.step = self.step.wrapping_add(1);
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::{UF_PAYLOAD_LEN, create_dummy_generator, decode_packet, format_decoded_packet};
    use crate::output::formats::crc16_ccitt_false;

    #[test]
    fn dummy_payload_generator_builds_crc_checked_uf_frame() {
        let mut generator = create_dummy_generator().expect("should create generator");
        let payload = generator.next_payload().expect("should encode");
        let next_payload = generator.next_payload().expect("should encode");

        assert_eq!(payload.len(), 14);
        assert_eq!(&payload[..2], b"UF");
        assert_eq!(payload[2], 0x01);
        assert_eq!(payload[3], 0x03);
        assert_eq!(
            i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]),
            356_812_362
        );
        assert_eq!(
            i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]),
            1_397_671_248
        );
        assert_eq!(
            u16::from_le_bytes([payload[12], payload[13]]),
            crc16_ccitt_false(&payload[..UF_PAYLOAD_LEN])
        );
        assert_eq!(next_payload[2], 0x02);
        assert_ne!(payload, next_payload);
    }

    #[test]
    fn decode_packet_extracts_flags_and_lat_lon() {
        let mut generator = create_dummy_generator().expect("should create generator");
        let payload = generator.next_payload().expect("should encode");

        let decoded = decode_packet(&payload).expect("should decode");

        assert_eq!(decoded.seq, 0x01);
        assert_eq!(decoded.flags, 0x03);
        assert_eq!(decoded.lat_e7, 356_812_362);
        assert_eq!(decoded.lon_e7, 1_397_671_248);
    }

    #[test]
    fn format_decoded_packet_is_human_readable_ascii() {
        let mut generator = create_dummy_generator().expect("should create generator");
        let payload = generator.next_payload().expect("should encode");

        assert_eq!(
            format_decoded_packet(&payload).expect("should format"),
            "UF seq=1 flags=0x03(valid=1, usb_present=1, read_busy=0, read_error=0, reserved=0x0) lat=35.6812362 lon=139.7671248 lat_e7=356812362 lon_e7=1397671248"
        );
    }
}
