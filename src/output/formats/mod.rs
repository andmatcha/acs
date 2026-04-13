mod packetacv6;

use crate::input::compact::CompactReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    PacketAcV6,
}

struct OutputFormatDefinition {
    format: OutputFormat,
    names: &'static [&'static str],
    create_driver: fn() -> Box<dyn OutputDriver>,
}

const OUTPUT_FORMATS: &[OutputFormatDefinition] = &[OutputFormatDefinition {
    format: OutputFormat::PacketAcV6,
    names: &["packetacv6"],
    create_driver: packetacv6::create_driver,
}];

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

    pub fn create_driver(self) -> Box<dyn OutputDriver> {
        (find_definition(self).create_driver)()
    }

    pub fn encode_dummy_payload(self) -> Result<Vec<u8>, String> {
        let mut driver = self.create_driver();
        driver.encode(&DEFAULT_DUMMY_COMPACT_REPORT)
    }
}

pub trait OutputDriver {
    fn encode(&mut self, compact_report: &CompactReport) -> Result<Vec<u8>, String>;
}

const DEFAULT_DUMMY_COMPACT_REPORT: CompactReport = [0; 8];

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
}
