mod crc;
mod packetacv6;
mod packetjfv1;

use crate::input::compact::CompactReport;
pub(crate) use crc::crc16_ccitt_false;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    PacketAcV6,
    PacketJfV1,
}

struct OutputFormatDefinition {
    format: OutputFormat,
    names: &'static [&'static str],
    packet_len: usize,
    create_driver: Option<fn() -> Box<dyn OutputDriver>>,
    create_dummy_generator: fn() -> Result<Box<dyn DummyPayloadGenerator>, String>,
}

const OUTPUT_FORMATS: &[OutputFormatDefinition] = &[
    OutputFormatDefinition {
        format: OutputFormat::PacketAcV6,
        names: &["packetacv6"],
        packet_len: packetacv6::packet_len(),
        create_driver: Some(packetacv6::create_driver),
        create_dummy_generator: packetacv6::create_dummy_generator,
    },
    OutputFormatDefinition {
        format: OutputFormat::PacketJfV1,
        names: &["packetjfv1"],
        packet_len: packetjfv1::packet_len(),
        create_driver: None,
        create_dummy_generator: packetjfv1::create_dummy_generator,
    },
];

impl OutputFormat {
    pub fn parse(value: &str) -> Result<Self, String> {
        OUTPUT_FORMATS
            .iter()
            .find(|definition| {
                definition
                    .names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(value))
            })
            .map(|definition| definition.format)
            .ok_or_else(|| format!("unsupported output format: {value}"))
    }

    pub fn as_str(self) -> &'static str {
        find_definition(self).names[0]
    }

    pub fn create_driver(self) -> Result<Box<dyn OutputDriver>, String> {
        find_definition(self)
            .create_driver
            .map(|create_driver| create_driver())
            .ok_or_else(|| {
                format!(
                    "output format `{}` does not support compact encoding",
                    self.as_str()
                )
            })
    }

    pub fn encode_dummy_payload(self) -> Result<Vec<u8>, String> {
        let mut generator = self.create_dummy_generator()?;
        generator.next_payload()
    }

    pub fn create_dummy_generator(self) -> Result<Box<dyn DummyPayloadGenerator>, String> {
        (find_definition(self).create_dummy_generator)()
    }

    pub fn packet_len(self) -> usize {
        find_definition(self).packet_len
    }
}

pub trait OutputDriver {
    fn encode(&mut self, compact_report: &CompactReport) -> Result<Vec<u8>, String>;
}

pub trait DummyPayloadGenerator {
    fn next_payload(&mut self) -> Result<Vec<u8>, String>;
}

fn find_definition(format: OutputFormat) -> &'static OutputFormatDefinition {
    OUTPUT_FORMATS
        .iter()
        .find(|definition| definition.format == format)
        .expect("every output format variant must be registered")
}

#[cfg(test)]
mod tests {
    use super::OutputFormat;

    #[test]
    fn parse_rejects_arm9() {
        assert_eq!(
            OutputFormat::parse("arm9").expect_err("should reject"),
            "unsupported output format: arm9"
        );
    }

    #[test]
    fn parse_supports_packetacv6_alias() {
        assert_eq!(
            OutputFormat::parse("packetacv6").expect("should parse"),
            OutputFormat::PacketAcV6
        );
        assert_eq!(
            OutputFormat::parse("PacketACv6").expect("should parse"),
            OutputFormat::PacketAcV6
        );
    }

    #[test]
    fn packetacv6_dummy_payload_has_ac_header() {
        let mut generator = OutputFormat::PacketAcV6
            .create_dummy_generator()
            .expect("should create generator");
        let payload = generator.next_payload().expect("should encode");
        let next_payload = generator.next_payload().expect("should encode");

        assert_eq!(payload.len(), 39);
        assert_eq!(&payload[..2], b"AC");
        assert_ne!(payload, next_payload);
    }

    #[test]
    fn parse_supports_packetjfv1() {
        assert_eq!(
            OutputFormat::parse("packetjfv1").expect("should parse"),
            OutputFormat::PacketJfV1
        );
        assert_eq!(
            OutputFormat::parse("PacketJFv1").expect("should parse"),
            OutputFormat::PacketJfV1
        );
    }

    #[test]
    fn parse_rejects_jf() {
        assert_eq!(
            OutputFormat::parse("jf").expect_err("should reject"),
            "unsupported output format: jf"
        );
    }

    #[test]
    fn packetjfv1_rejects_compact_encoding_driver() {
        match OutputFormat::PacketJfV1.create_driver() {
            Ok(_) => panic!("packetjfv1 should not create a compact encoding driver"),
            Err(error) => {
                assert_eq!(
                    error,
                    "output format `packetjfv1` does not support compact encoding"
                );
            }
        }
    }

    #[test]
    fn packetjfv1_dummy_generator_advances_sequence() {
        let mut generator = OutputFormat::PacketJfV1
            .create_dummy_generator()
            .expect("should create generator");
        let first = generator.next_payload().expect("should encode");
        let second = generator.next_payload().expect("should encode");

        assert_eq!(first[2], 0x01);
        assert_eq!(second[2], 0x02);
        assert_ne!(first, second);
    }

    #[test]
    fn packet_lengths_match_documented_formats() {
        assert_eq!(OutputFormat::PacketAcV6.packet_len(), 39);
        assert_eq!(OutputFormat::PacketJfV1.packet_len(), 16);
    }
}
