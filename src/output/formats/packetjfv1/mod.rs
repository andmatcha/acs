use crate::output::formats::DummyPayloadGenerator;
use crate::output::formats::crc::crc16_ccitt_false;

const JF_PACKET_LEN: usize = 16;
const JF_PAYLOAD_LEN: usize = 14;
const JF_DUMMY_SEQ: u8 = 0x01;
const JF_DUMMY_FLAGS: u8 = 0x00;
const JF_DUMMY_ENCODERS: [u16; 5] = [0, 0, 3281, 6397, 980];
const JF_DUMMY_ENCODER_STRIDES: [u16; 5] = [11, 17, 23, 31, 43];

pub(crate) const fn packet_len() -> usize {
    JF_PACKET_LEN
}

pub(crate) fn create_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String> {
    Ok(Box::new(PacketJfV1DummyGenerator::default()))
}

fn build_jf_frame(seq: u8, flags: u8, encoders: [u16; 5]) -> [u8; JF_PACKET_LEN] {
    let mut frame = [0u8; JF_PACKET_LEN];
    frame[0] = b'J';
    frame[1] = b'F';
    frame[2] = seq;
    frame[3] = flags;

    for (index, value) in encoders.iter().enumerate() {
        let bytes = value.to_le_bytes();
        let offset = 4 + index * 2;
        frame[offset] = bytes[0];
        frame[offset + 1] = bytes[1];
    }

    let crc = crc16_ccitt_false(&frame[..JF_PAYLOAD_LEN]).to_le_bytes();
    frame[14] = crc[0];
    frame[15] = crc[1];
    frame
}

#[derive(Default)]
struct PacketJfV1DummyGenerator {
    step: u8,
}

impl DummyPayloadGenerator for PacketJfV1DummyGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String> {
        let step = self.step as u16;
        let payload = build_jf_frame(
            JF_DUMMY_SEQ.wrapping_add(self.step),
            JF_DUMMY_FLAGS ^ ((self.step / 8) & 0x03),
            std::array::from_fn(|index| {
                JF_DUMMY_ENCODERS[index]
                    .wrapping_add(step.wrapping_mul(JF_DUMMY_ENCODER_STRIDES[index]))
            }),
        )
        .to_vec();
        self.step = self.step.wrapping_add(1);
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::create_dummy_generator;
    use crate::common::format_bytes_hex;

    #[test]
    fn dummy_payload_generator_starts_from_documented_home_frame() {
        let mut generator = create_dummy_generator().expect("should create generator");
        let payload = generator.next_payload().expect("should encode");
        let next_payload = generator.next_payload().expect("should encode");

        assert_eq!(payload.len(), 16);
        assert_eq!(&payload[..2], b"JF");
        assert_eq!(
            format_bytes_hex(&payload),
            "4A 46 01 00 00 00 00 00 D1 0C FD 18 D4 03 04 86"
        );
        assert_eq!(next_payload[2], 0x02);
        assert_ne!(payload, next_payload);
    }
}
