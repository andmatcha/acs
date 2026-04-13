use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PortDisplayMode {
    Hex,
    Ascii,
    Utf8,
    HexAscii,
    #[default]
    HexUtf8,
}

impl PortDisplayMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "hex" => Ok(Self::Hex),
            "ascii" => Ok(Self::Ascii),
            "utf8" => Ok(Self::Utf8),
            "hex+ascii" => Ok(Self::HexAscii),
            "hex+utf8" | "both" => Ok(Self::HexUtf8),
            other => Err(format!(
                "invalid display mode: {other} (expected hex/ascii/utf8/hex+ascii/hex+utf8)"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDisplayStream {
    Input,
    Output,
}

#[derive(Debug, Clone, Default)]
struct PortDisplayScopeConfig {
    default_mode: Option<PortDisplayMode>,
    per_port: BTreeMap<String, PortDisplayMode>,
}

impl PortDisplayScopeConfig {
    fn set(&mut self, target: impl Into<String>, mode: PortDisplayMode) {
        let target = target.into();
        if target == "default" {
            self.default_mode = Some(mode);
        } else {
            self.per_port.insert(target, mode);
        }
    }

    fn merge_from(&mut self, other: PortDisplayScopeConfig) {
        if let Some(mode) = other.default_mode {
            self.default_mode = Some(mode);
        }

        for (port, mode) in other.per_port {
            self.per_port.insert(port, mode);
        }
    }

    fn resolve(&self, port: &str) -> PortDisplayMode {
        self.per_port
            .get(port)
            .copied()
            .or(self.default_mode)
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PortDisplayConfig {
    input: PortDisplayScopeConfig,
    output: PortDisplayScopeConfig,
}

impl PortDisplayConfig {
    pub fn set_both(&mut self, target: impl Into<String>, mode: PortDisplayMode) {
        let target = target.into();
        self.input.set(target.clone(), mode);
        self.output.set(target, mode);
    }

    pub fn set_input(&mut self, target: impl Into<String>, mode: PortDisplayMode) {
        self.input.set(target, mode);
    }

    pub fn set_output(&mut self, target: impl Into<String>, mode: PortDisplayMode) {
        self.output.set(target, mode);
    }

    pub fn set_for_stream(
        &mut self,
        stream: Option<PortDisplayStream>,
        target: impl Into<String>,
        mode: PortDisplayMode,
    ) {
        let target = target.into();
        match stream {
            Some(PortDisplayStream::Input) => self.input.set(target, mode),
            Some(PortDisplayStream::Output) => self.output.set(target, mode),
            None => self.set_both(target, mode),
        }
    }

    pub fn merge_from(&mut self, other: PortDisplayConfig) {
        self.input.merge_from(other.input);
        self.output.merge_from(other.output);
    }

    pub fn resolve_input(&self, port: &str) -> PortDisplayMode {
        self.input.resolve(port)
    }

    pub fn resolve_output(&self, port: &str) -> PortDisplayMode {
        self.output.resolve(port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortDisplayAssignment {
    pub stream: Option<PortDisplayStream>,
    pub target: String,
    pub mode: PortDisplayMode,
}

pub fn parse_display_assignment(value: &str) -> Result<PortDisplayAssignment, String> {
    let (target, mode) = value.split_once('=').ok_or_else(|| {
        String::from("display must be in the form <PORT>=<hex|ascii|utf8|hex+ascii|hex+utf8>")
    })?;

    if target.is_empty() {
        return Err(String::from("display target must not be empty"));
    }

    let (stream, target) = if let Some((prefix, rest)) = target.split_once(':') {
        match prefix {
            "input" | "rx" => (Some(PortDisplayStream::Input), rest),
            "output" | "tx" => (Some(PortDisplayStream::Output), rest),
            _ => (None, target),
        }
    } else {
        (None, target)
    };

    if target.is_empty() {
        return Err(String::from("display target must not be empty"));
    }

    Ok(PortDisplayAssignment {
        stream,
        target: target.to_owned(),
        mode: PortDisplayMode::parse(mode)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        PortDisplayAssignment, PortDisplayConfig, PortDisplayMode, PortDisplayStream,
        parse_display_assignment,
    };

    #[test]
    fn parse_display_assignment_supports_port_and_direction() {
        assert_eq!(
            parse_display_assignment("/dev/ttyUSB0=ascii").unwrap(),
            PortDisplayAssignment {
                stream: None,
                target: String::from("/dev/ttyUSB0"),
                mode: PortDisplayMode::Ascii,
            }
        );
        assert_eq!(
            parse_display_assignment("input:/dev/ttyUSB1=utf8").unwrap(),
            PortDisplayAssignment {
                stream: Some(PortDisplayStream::Input),
                target: String::from("/dev/ttyUSB1"),
                mode: PortDisplayMode::Utf8,
            }
        );
        assert_eq!(
            parse_display_assignment("output:default=hex").unwrap(),
            PortDisplayAssignment {
                stream: Some(PortDisplayStream::Output),
                target: String::from("default"),
                mode: PortDisplayMode::Hex,
            }
        );
    }

    #[test]
    fn display_config_resolves_per_direction() {
        let mut config = PortDisplayConfig::default();
        config.set_both("default", PortDisplayMode::HexUtf8);
        config.set_input("/dev/ttyUSB0", PortDisplayMode::Utf8);
        config.set_output("/dev/ttyUSB0", PortDisplayMode::Hex);

        assert_eq!(config.resolve_input("/dev/ttyUSB0"), PortDisplayMode::Utf8);
        assert_eq!(config.resolve_output("/dev/ttyUSB0"), PortDisplayMode::Hex);
        assert_eq!(
            config.resolve_input("/dev/ttyUSB1"),
            PortDisplayMode::HexUtf8
        );
        assert_eq!(
            config.resolve_output("/dev/ttyUSB1"),
            PortDisplayMode::HexUtf8
        );
    }
}
