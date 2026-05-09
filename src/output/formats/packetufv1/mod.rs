use crate::output::formats::DummyPayloadGenerator;
use crate::output::formats::crc::crc16_ccitt_false;

const UF_PACKET_LEN: usize = 14;
const UF_PAYLOAD_LEN: usize = 12;
const UF_DUMMY_SEQ: u8 = 0x01;
const UF_DUMMY_FLAGS: u8 = 0x03;
const UF_DUMMY_LAT_E7: i32 = 356_812_362;
const UF_DUMMY_LON_E7: i32 = 1_397_671_248;

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
    use super::{UF_PAYLOAD_LEN, create_dummy_generator};
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
}
