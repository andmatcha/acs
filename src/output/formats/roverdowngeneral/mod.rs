use crate::output::formats::DummyPayloadGenerator;

const ROVER_DOWN_GENERAL_PACKET_LEN: usize = 13;
const SENSOR_IDS: [u16; 16] = [
    0x300, 0x301, 0x302, 0x303, 0x400, 0x401, 0x402, 0x403, 0x404, 0x405, 0x406, 0x407, 0x408,
    0x410, 0x411, 0x412,
];
const SENSOR_BASE_VALUES_2DP: [u64; 16] = [
    1010, 1120, 1230, 1340, 2110, 2220, 2330, 3110, 3220, 3330, 4110, 4220, 4330, 5110, 5220, 8760,
];

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
        let index = self.step as usize % SENSOR_IDS.len();
        let scaled_value = SENSOR_BASE_VALUES_2DP[index];
        let payload = format!(
            "0x{:03X},{:02}.{:02}\r\n",
            SENSOR_IDS[index],
            scaled_value / 100,
            scaled_value % 100
        );
        debug_assert_eq!(payload.len(), ROVER_DOWN_GENERAL_PACKET_LEN);
        self.step = self.step.wrapping_add(1);
        Ok(payload.into_bytes())
    }
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
        assert_eq!(first_text, "0x300,10.10\r\n");
        assert_ne!(first, second);
    }
}
