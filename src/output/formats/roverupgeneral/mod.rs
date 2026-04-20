use crate::output::formats::DummyPayloadGenerator;

const ROVER_UP_GENERAL_PACKET_LEN: usize = 110;
const RAW_ANGLE_CENTER: u16 = 180;
const RAW_SPEED_CENTER: u16 = 120;
const PIVOT_DEGREE_RAW: u16 = 45;
const SLIDE_DEGREE_RAW: u16 = 90;

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
        let phase = (self.step % 6) as u16;
        let angle_delta = phase * 5;
        let speed_delta = phase * 3;
        let payload = format!(
            concat!(
                "0x300,000\r\n",
                "0x310,{:03}\r\n",
                "0x311,{:03}\r\n",
                "0x312,{:03}\r\n",
                "0x313,{:03}\r\n",
                "0x314,{:03}\r\n",
                "0x315,{:03}\r\n",
                "0x316,{:03}\r\n",
                "0x317,{:03}\r\n",
                "0x318,{:03}\r\n",
            ),
            RAW_ANGLE_CENTER + angle_delta,
            RAW_ANGLE_CENTER - angle_delta,
            RAW_SPEED_CENTER + speed_delta,
            RAW_ANGLE_CENTER + angle_delta,
            RAW_SPEED_CENTER - speed_delta,
            PIVOT_DEGREE_RAW,
            RAW_SPEED_CENTER + speed_delta,
            SLIDE_DEGREE_RAW,
            RAW_SPEED_CENTER + speed_delta,
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
        assert_eq!(
            first_text,
            concat!(
                "0x300,000\r\n",
                "0x310,180\r\n",
                "0x311,180\r\n",
                "0x312,120\r\n",
                "0x313,180\r\n",
                "0x314,120\r\n",
                "0x315,045\r\n",
                "0x316,120\r\n",
                "0x317,090\r\n",
                "0x318,120\r\n",
            )
        );
        assert_ne!(first, second);
    }
}
