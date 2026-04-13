mod arm9;

use crate::input::compact::CompactReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Arm9,
}

struct OutputFormatDefinition {
    format: OutputFormat,
    name: &'static str,
    create_driver: fn() -> Box<dyn OutputDriver>,
}

const OUTPUT_FORMATS: &[OutputFormatDefinition] = &[OutputFormatDefinition {
    format: OutputFormat::Arm9,
    name: "arm9",
    create_driver: arm9::create_driver,
}];

impl OutputFormat {
    pub fn parse(value: &str) -> Result<Self, String> {
        OUTPUT_FORMATS
            .iter()
            .find(|definition| definition.name == value)
            .map(|definition| definition.format)
            .ok_or_else(|| format!("unsupported output format: {value}"))
    }

    pub fn as_str(self) -> &'static str {
        find_definition(self).name
    }

    pub fn create_driver(self) -> Box<dyn OutputDriver> {
        (find_definition(self).create_driver)()
    }
}

pub trait OutputDriver {
    fn format_name(&self) -> &'static str;
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
    fn parse_supports_arm9() {
        assert_eq!(
            OutputFormat::parse("arm9").expect("should parse"),
            OutputFormat::Arm9
        );
    }
}
