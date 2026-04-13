use crate::output::formats::crc::crc16_ccitt_false;

const JF_PACKET_LEN: usize = 16;
const JF_PAYLOAD_LEN: usize = 14;
const JF_DUMMY_SEQ: u8 = 0x01;
const JF_DUMMY_FLAGS: u8 = 0x00;
const JF_DUMMY_ENCODERS: [u16; 5] = [0, 0, 3281, 6397, 980];

pub(crate) fn encode_dummy_payload() -> Result<Vec<u8>, String> {
    Ok(build_jf_frame(JF_DUMMY_SEQ, JF_DUMMY_FLAGS, JF_DUMMY_ENCODERS).to_vec())
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

#[cfg(test)]
mod tests {
    use super::encode_dummy_payload;
    use crate::common::format_bytes_hex;

    #[test]
    fn dummy_payload_matches_documented_home_frame() {
        let payload = encode_dummy_payload().expect("should encode");

        assert_eq!(payload.len(), 16);
        assert_eq!(&payload[..2], b"JF");
        assert_eq!(
            format_bytes_hex(&payload),
            "4A 46 01 00 00 00 00 00 D1 0C FD 18 D4 03 04 86"
        );
    }
}
