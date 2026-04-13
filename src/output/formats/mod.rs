mod crc;
mod jf;
mod packetacv6;

use crate::input::compact::CompactReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    PacketAcV6,
    Jf,
}

struct OutputFormatDefinition {
    format: OutputFormat,
    names: &'static [&'static str],
    create_driver: Option<fn() -> Box<dyn OutputDriver>>,
    encode_dummy_payload: fn() -> Result<Vec<u8>, String>,
}

const OUTPUT_FORMATS: &[OutputFormatDefinition] = &[
    OutputFormatDefinition {
        format: OutputFormat::PacketAcV6,
        names: &["packetacv6"],
        create_driver: Some(packetacv6::create_driver),
        encode_dummy_payload: packetacv6::encode_dummy_payload,
    },
    OutputFormatDefinition {
        format: OutputFormat::Jf,
        names: &["jf"],
        create_driver: None,
        encode_dummy_payload: jf::encode_dummy_payload,
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
        (find_definition(self).encode_dummy_payload)()
    }
}

pub trait OutputDriver {
    fn encode(&mut self, compact_report: &CompactReport) -> Result<Vec<u8>, String>;
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
        let payload = OutputFormat::PacketAcV6
            .encode_dummy_payload()
            .expect("should encode");

        assert_eq!(payload.len(), 39);
        assert_eq!(&payload[..2], b"AC");
    }

    #[test]
    fn parse_supports_jf() {
        assert_eq!(
            OutputFormat::parse("jf").expect("should parse"),
            OutputFormat::Jf
        );
        assert_eq!(
            OutputFormat::parse("JF").expect("should parse"),
            OutputFormat::Jf
        );
    }

    #[test]
    fn jf_rejects_compact_encoding_driver() {
        match OutputFormat::Jf.create_driver() {
            Ok(_) => panic!("jf should not create a compact encoding driver"),
            Err(error) => {
                assert_eq!(
                    error,
                    "output format `jf` does not support compact encoding"
                );
            }
        }
    }
}
