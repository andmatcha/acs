use crate::common::format_bytes_utf8;
use crate::output::formats::DummyPayloadGenerator;
use crate::output::formats::crc::crc16_ccitt_false;

pub(crate) const UF_PACKET_LEN: usize = 40;
pub(crate) const UF_CRC_INPUT_LEN: usize = 38;
const UF_TEXT_PAYLOAD_LEN: usize = 32;
const UF_DUMMY_SEQ: u8 = 0x01;
const UF_FLAG_VALID: u8 = 1 << 0;
const UF_FLAG_USB_PRESENT: u8 = 1 << 1;
const UF_FLAG_READ_BUSY: u8 = 1 << 2;
const UF_FLAG_READ_ERROR: u8 = 1 << 3;
const UF_FLAG_END: u8 = 1 << 4;
const UF_DUMMY_FLAGS: u8 = UF_FLAG_VALID | UF_FLAG_USB_PRESENT | UF_FLAG_END;
const UF_DUMMY_TEXT: &[u8] = b"38.12345, -110.98765\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacketUfV2Packet {
    pub(crate) seq: u8,
    pub(crate) flags: u8,
    pub(crate) chunk_index: u8,
    pub(crate) payload_len: u8,
    pub(crate) payload: [u8; UF_TEXT_PAYLOAD_LEN],
}

impl PacketUfV2Packet {
    pub(crate) fn valid(&self) -> bool {
        self.flags & UF_FLAG_VALID != 0
    }

    pub(crate) fn usb_present(&self) -> bool {
        self.flags & UF_FLAG_USB_PRESENT != 0
    }

    pub(crate) fn read_busy(&self) -> bool {
        self.flags & UF_FLAG_READ_BUSY != 0
    }

    pub(crate) fn read_error(&self) -> bool {
        self.flags & UF_FLAG_READ_ERROR != 0
    }

    pub(crate) fn end(&self) -> bool {
        self.flags & UF_FLAG_END != 0
    }

    pub(crate) fn reserved_flags(&self) -> u8 {
        self.flags >> 5
    }

    pub(crate) fn payload_bytes(&self) -> &[u8] {
        &self.payload[..usize::from(self.payload_len)]
    }
}

pub(crate) const fn packet_len() -> usize {
    UF_PACKET_LEN
}

pub(crate) fn create_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String> {
    Ok(Box::new(PacketUfV2DummyGenerator::default()))
}

fn build_uf_frame(
    seq: u8,
    flags: u8,
    chunk_index: u8,
    payload: &[u8],
) -> Result<[u8; UF_PACKET_LEN], String> {
    if payload.len() > UF_TEXT_PAYLOAD_LEN {
        return Err(format!(
            "PacketUFv2 payload must be at most {UF_TEXT_PAYLOAD_LEN} bytes, got {}",
            payload.len()
        ));
    }

    let mut frame = [0u8; UF_PACKET_LEN];
    frame[0] = b'U';
    frame[1] = b'F';
    frame[2] = seq;
    frame[3] = flags;
    frame[4] = chunk_index;
    frame[5] = payload.len() as u8;
    frame[6..6 + payload.len()].copy_from_slice(payload);

    let crc = crc16_ccitt_false(&frame[..UF_CRC_INPUT_LEN]).to_le_bytes();
    frame[38] = crc[0];
    frame[39] = crc[1];
    Ok(frame)
}

pub(crate) fn decode_packet(packet: &[u8]) -> Result<PacketUfV2Packet, String> {
    if packet.len() != UF_PACKET_LEN {
        return Err(format!(
            "PacketUFv2 length must be {UF_PACKET_LEN} bytes, got {}",
            packet.len()
        ));
    }
    if !packet.starts_with(b"UF") {
        return Err(String::from("PacketUFv2 header must be UF"));
    }

    let actual_crc = u16::from_le_bytes([packet[38], packet[39]]);
    let expected_crc = crc16_ccitt_false(&packet[..UF_CRC_INPUT_LEN]);
    if actual_crc != expected_crc {
        return Err(format!(
            "PacketUFv2 CRC mismatch: expected 0x{expected_crc:04X}, got 0x{actual_crc:04X}"
        ));
    }

    let payload_len = packet[5];
    if usize::from(payload_len) > UF_TEXT_PAYLOAD_LEN {
        return Err(format!(
            "PacketUFv2 payload_len must be 0..={UF_TEXT_PAYLOAD_LEN}, got {payload_len}"
        ));
    }

    let mut payload = [0u8; UF_TEXT_PAYLOAD_LEN];
    payload.copy_from_slice(&packet[6..38]);

    Ok(PacketUfV2Packet {
        seq: packet[2],
        flags: packet[3],
        chunk_index: packet[4],
        payload_len,
        payload,
    })
}

pub(crate) fn format_decoded_packet(packet: &[u8]) -> Result<String, String> {
    let packet = decode_packet(packet)?;
    let text = format_bytes_utf8(packet.payload_bytes());
    Ok(format!(
        "UFv2 seq={} flags=0x{:02X}(valid={}, usb_present={}, read_busy={}, read_error={}, end={}, reserved=0x{:X}) chunk={} len={} text={text}",
        packet.seq,
        packet.flags,
        flag_value(packet.valid()),
        flag_value(packet.usb_present()),
        flag_value(packet.read_busy()),
        flag_value(packet.read_error()),
        flag_value(packet.end()),
        packet.reserved_flags(),
        packet.chunk_index,
        packet.payload_len,
    ))
}

fn flag_value(enabled: bool) -> u8 {
    u8::from(enabled)
}

#[derive(Default)]
struct PacketUfV2DummyGenerator {
    step: u8,
}

impl DummyPayloadGenerator for PacketUfV2DummyGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String> {
        let payload = build_uf_frame(
            UF_DUMMY_SEQ.wrapping_add(self.step),
            UF_DUMMY_FLAGS,
            0,
            UF_DUMMY_TEXT,
        )?
        .to_vec();
        self.step = self.step.wrapping_add(1);
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::{UF_CRC_INPUT_LEN, create_dummy_generator, decode_packet, format_decoded_packet};
    use crate::output::formats::crc16_ccitt_false;

    #[test]
    fn dummy_payload_generator_builds_crc_checked_uf_v2_frame() {
        let mut generator = create_dummy_generator().expect("should create generator");
        let payload = generator.next_payload().expect("should encode");
        let next_payload = generator.next_payload().expect("should encode");

        assert_eq!(payload.len(), 40);
        assert_eq!(&payload[..2], b"UF");
        assert_eq!(payload[2], 0x01);
        assert_eq!(payload[3], 0x13);
        assert_eq!(payload[4], 0);
        assert_eq!(payload[5], 21);
        assert_eq!(&payload[6..27], b"38.12345, -110.98765\n");
        assert!(payload[27..38].iter().all(|byte| *byte == 0));
        assert_eq!(
            u16::from_le_bytes([payload[38], payload[39]]),
            crc16_ccitt_false(&payload[..UF_CRC_INPUT_LEN])
        );
        assert_eq!(next_payload[2], 0x02);
        assert_ne!(payload, next_payload);
    }

    #[test]
    fn decode_packet_extracts_text_chunk_fields() {
        let mut generator = create_dummy_generator().expect("should create generator");
        let payload = generator.next_payload().expect("should encode");

        let decoded = decode_packet(&payload).expect("should decode");

        assert_eq!(decoded.seq, 0x01);
        assert_eq!(decoded.flags, 0x13);
        assert!(decoded.valid());
        assert!(decoded.usb_present());
        assert!(decoded.end());
        assert_eq!(decoded.chunk_index, 0);
        assert_eq!(decoded.payload_bytes(), b"38.12345, -110.98765\n");
    }

    #[test]
    fn decode_packet_rejects_legacy_uf_v1_length() {
        let packet = [0u8; 14];

        assert_eq!(
            decode_packet(&packet).expect_err("should reject"),
            "PacketUFv2 length must be 40 bytes, got 14"
        );
    }

    #[test]
    fn decode_packet_rejects_payload_len_over_32() {
        let mut generator = create_dummy_generator().expect("should create generator");
        let mut payload = generator.next_payload().expect("should encode");
        payload[5] = 33;
        let crc = crc16_ccitt_false(&payload[..UF_CRC_INPUT_LEN]).to_le_bytes();
        payload[38] = crc[0];
        payload[39] = crc[1];

        assert_eq!(
            decode_packet(&payload).expect_err("should reject"),
            "PacketUFv2 payload_len must be 0..=32, got 33"
        );
    }

    #[test]
    fn format_decoded_packet_is_human_readable_ascii() {
        let mut generator = create_dummy_generator().expect("should create generator");
        let payload = generator.next_payload().expect("should encode");

        assert_eq!(
            format_decoded_packet(&payload).expect("should format"),
            "UFv2 seq=1 flags=0x13(valid=1, usb_present=1, read_busy=0, read_error=0, end=1, reserved=0x0) chunk=0 len=21 text=\"38.12345, -110.98765\\n\""
        );
    }
}
