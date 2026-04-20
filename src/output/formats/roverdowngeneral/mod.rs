use crate::output::formats::DummyPayloadGenerator;
use std::fmt::Write as _;

const ROVER_DOWN_GENERAL_PACKET_LEN: usize = 173;
const SENSOR_IDS: [u16; 12] = [
    0x400, 0x401, 0x402, 0x403, 0x404, 0x405, 0x406, 0x407, 0x408, 0x410, 0x411, 0x412,
];
const SENSOR_BASE_VALUES_2DP: [u64; 12] = [
    2110, 2220, 2330, 3110, 3220, 3330, 4110, 4220, 4330, 5110, 5220, 8760,
];
const LAT_BASE_VALUE_11DP: u64 = 3_512_345_678_901;
const LON_BASE_VALUE_11DP: u64 = 14_012_345_678_901;

pub(crate) const fn packet_len() -> usize {
    ROVER_DOWN_GENERAL_PACKET_LEN
}

pub(crate) fn create_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String> {
    Ok(Box::new(RoverDownGeneralDummyGenerator::default()))
}

#[derive(Default)]
struct RoverDownGeneralDummyGenerator {
    step: u8,
}

impl DummyPayloadGenerator for RoverDownGeneralDummyGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String> {
        let mut payload = String::with_capacity(ROVER_DOWN_GENERAL_PACKET_LEN);
        let delta_2dp = u64::from(self.step % 10) * 10;
        let delta_11dp = u64::from(self.step % 100);

        for (id, base_value) in SENSOR_IDS.iter().zip(SENSOR_BASE_VALUES_2DP.iter()) {
            push_scaled_line(&mut payload, *id, *base_value + delta_2dp, 2)?;
        }
        push_scaled_line(&mut payload, 0x415, LAT_BASE_VALUE_11DP + delta_11dp, 11)?;
        push_scaled_line(&mut payload, 0x416, LON_BASE_VALUE_11DP + delta_11dp, 11)?;

        debug_assert_eq!(payload.len(), ROVER_DOWN_GENERAL_PACKET_LEN);
        self.step = self.step.wrapping_add(1);
        Ok(payload.into_bytes())
    }
}

fn push_scaled_line(
    payload: &mut String,
    id: u16,
    scaled_value: u64,
    fractional_digits: u32,
) -> Result<(), String> {
    let divisor = 10u64.pow(fractional_digits);
    let whole = scaled_value / divisor;
    let fractional = scaled_value % divisor;
    write!(
        payload,
        "{id:03X},{whole}.{fractional:0width$}\r\n",
        width = fractional_digits as usize,
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{create_dummy_generator, packet_len};

    #[test]
    fn dummy_payload_generator_emits_ascii_telemetry_snapshot() {
        let mut generator = create_dummy_generator().expect("should create generator");
        let first = generator.next_payload().expect("should encode");
        let second = generator.next_payload().expect("should encode");
        let first_text = String::from_utf8(first.clone()).expect("ascii payload");

        assert_eq!(first.len(), packet_len());
        assert_eq!(
            first_text,
            concat!(
                "400,21.10\r\n",
                "401,22.20\r\n",
                "402,23.30\r\n",
                "403,31.10\r\n",
                "404,32.20\r\n",
                "405,33.30\r\n",
                "406,41.10\r\n",
                "407,42.20\r\n",
                "408,43.30\r\n",
                "410,51.10\r\n",
                "411,52.20\r\n",
                "412,87.60\r\n",
                "415,35.12345678901\r\n",
                "416,140.12345678901\r\n",
            )
        );
        assert_ne!(first, second);
    }
}
