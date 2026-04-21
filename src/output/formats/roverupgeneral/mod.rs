use crate::output::formats::DummyPayloadGenerator;

const ROVER_UP_GENERAL_PACKET_LEN: usize = 12;
const COMMAND_IDS: [u16; 10] = [
    0x300, 0x310, 0x311, 0x312, 0x313, 0x314, 0x315, 0x316, 0x317, 0x318,
];
const COMMAND_VALUES: [u16; 10] = [1234, 1800, 1800, 1200, 1800, 1200, 450, 1200, 900, 1200];

pub(crate) const fn packet_len() -> usize {
    ROVER_UP_GENERAL_PACKET_LEN
}

pub(crate) fn create_dummy_generator() -> Result<Box<dyn DummyPayloadGenerator>, String> {
    Ok(Box::new(RoverUpGeneralDummyGenerator::default()))
}

#[derive(Default)]
struct RoverUpGeneralDummyGenerator {
    step: u8,
}

impl DummyPayloadGenerator for RoverUpGeneralDummyGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String> {
        let index = self.step as usize % COMMAND_IDS.len();
        let payload = format!(
            "0x{:03X},{:04}\r\n",
            COMMAND_IDS[index], COMMAND_VALUES[index]
        );
        debug_assert_eq!(payload.len(), ROVER_UP_GENERAL_PACKET_LEN);
        self.step = self.step.wrapping_add(1);
        Ok(payload.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::{create_dummy_generator, packet_len};

    #[test]
    fn dummy_payload_generator_emits_fixed_width_ascii_control_snapshot() {
        let mut generator = create_dummy_generator().expect("should create generator");
        let first = generator.next_payload().expect("should encode");
        let second = generator.next_payload().expect("should encode");
        let first_text = String::from_utf8(first.clone()).expect("ascii payload");

        assert_eq!(first.len(), packet_len());
        assert_eq!(first_text, "0x300,1234\r\n");
        assert_ne!(first, second);
    }
}
