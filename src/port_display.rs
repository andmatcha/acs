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

/// 入力データの表示単位。line=改行まで1行、packet=読み取りチャンク単位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineBreakMode {
    #[default]
    Line,
    Packet,
}

impl LineBreakMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "line" => Ok(Self::Line),
            "packet" | "raw" => Ok(Self::Packet),
            other => Err(format!(
                "invalid line break mode: {other} (expected line/packet)"
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

    fn resolve(&self, port: &str) -> PortDisplayMode {
        self.per_port
            .get(port)
            .copied()
            .or(self.default_mode)
            .unwrap_or_default()
    }

    fn resolve_override(&self, port: &str) -> Option<PortDisplayMode> {
        self.per_port.get(port).copied().or(self.default_mode)
    }
}

#[derive(Debug, Clone, Default)]
struct LineBreakScopeConfig {
    default_mode: Option<LineBreakMode>,
    per_port: BTreeMap<String, LineBreakMode>,
}

impl LineBreakScopeConfig {
    fn set(&mut self, target: impl Into<String>, mode: LineBreakMode) {
        let target = target.into();
        if target == "default" {
            self.default_mode = Some(mode);
        } else {
            self.per_port.insert(target, mode);
        }
    }

    fn resolve(&self, port: &str) -> LineBreakMode {
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
    input_line_break: LineBreakScopeConfig,
}

impl PortDisplayConfig {
    pub fn set_both(&mut self, target: impl Into<String>, mode: PortDisplayMode) {
        let target = target.into();
        self.input.set(target.clone(), mode);
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

    pub fn set_line_break_for_stream(
        &mut self,
        stream: Option<PortDisplayStream>,
        target: impl Into<String>,
        mode: LineBreakMode,
    ) {
        let target = target.into();
        match stream {
            Some(PortDisplayStream::Output) => {} // line break only applies to input
            _ => self.input_line_break.set(target, mode),
        }
    }

    pub fn resolve_input(&self, port: &str) -> PortDisplayMode {
        self.input.resolve(port)
    }

    pub fn resolve_output(&self, port: &str) -> PortDisplayMode {
        self.output.resolve(port)
    }

    pub fn resolve_input_override(&self, port: &str) -> Option<PortDisplayMode> {
        self.input.resolve_override(port)
    }

    pub fn resolve_output_override(&self, port: &str) -> Option<PortDisplayMode> {
        self.output.resolve_override(port)
    }

    pub fn resolve_line_break_input(&self, port: &str) -> LineBreakMode {
        self.input_line_break.resolve(port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortDisplayAssignment {
    pub stream: Option<PortDisplayStream>,
    pub target: String,
    pub encoding: Option<PortDisplayMode>,
    pub line_break: Option<LineBreakMode>,
}

impl PortDisplayAssignment {
    pub fn apply_to(&self, config: &mut PortDisplayConfig) {
        if let Some(enc) = self.encoding {
            config.set_for_stream(self.stream, self.target.clone(), enc);
        }
        if let Some(lb) = self.line_break {
            config.set_line_break_for_stream(self.stream, self.target.clone(), lb);
        }
    }
}

/// `[stream:]target=mode` 形式をパースする。
/// mode に `line` / `packet` を含む場合は LineBreakMode として解釈する。
/// `hex+packet` のように組み合わせ指定も可能。
pub fn parse_display_assignment(value: &str) -> Result<PortDisplayAssignment, String> {
    let (target, mode_str) = value.split_once('=').ok_or_else(|| {
        String::from(
            "display must be in the form <PORT>=<hex|ascii|utf8|hex+ascii|hex+utf8|line|packet>",
        )
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

    let (encoding, line_break) = parse_display_value(mode_str)?;

    if encoding.is_none() && line_break.is_none() {
        return Err(format!(
            "invalid display mode: {mode_str} (expected hex/ascii/utf8/hex+ascii/hex+utf8/line/packet)"
        ));
    }

    Ok(PortDisplayAssignment {
        stream,
        target: target.to_owned(),
        encoding,
        line_break,
    })
}

pub fn parse_display_value(
    value: &str,
) -> Result<(Option<PortDisplayMode>, Option<LineBreakMode>), String> {
    let parts: Vec<&str> = value.split('+').collect();
    let mut encoding_parts: Vec<&str> = Vec::new();
    let mut line_break: Option<LineBreakMode> = None;

    for part in &parts {
        match LineBreakMode::parse(part) {
            Ok(mode) => line_break = Some(mode),
            Err(_) => encoding_parts.push(part),
        }
    }

    let encoding = if encoding_parts.is_empty() {
        None
    } else {
        let enc_str = encoding_parts.join("+");
        Some(PortDisplayMode::parse(&enc_str).map_err(|_| {
            format!(
                "invalid display mode: {value} (expected hex/ascii/utf8/hex+ascii/hex+utf8/line/packet)"
            )
        })?)
    };

    Ok((encoding, line_break))
}

#[cfg(test)]
mod tests {
    use super::{
        LineBreakMode, PortDisplayConfig, PortDisplayMode, PortDisplayStream,
        parse_display_assignment, parse_display_value,
    };

    #[test]
    fn parse_display_assignment_supports_port_and_direction() {
        let a = parse_display_assignment("/dev/ttyUSB0=ascii").unwrap();
        assert_eq!(a.stream, None);
        assert_eq!(a.target, "/dev/ttyUSB0");
        assert_eq!(a.encoding, Some(PortDisplayMode::Ascii));
        assert_eq!(a.line_break, None);

        let b = parse_display_assignment("input:/dev/ttyUSB1=utf8").unwrap();
        assert_eq!(b.stream, Some(PortDisplayStream::Input));
        assert_eq!(b.target, "/dev/ttyUSB1");
        assert_eq!(b.encoding, Some(PortDisplayMode::Utf8));
        assert_eq!(b.line_break, None);

        let c = parse_display_assignment("output:default=hex").unwrap();
        assert_eq!(c.stream, Some(PortDisplayStream::Output));
        assert_eq!(c.target, "default");
        assert_eq!(c.encoding, Some(PortDisplayMode::Hex));
        assert_eq!(c.line_break, None);
    }

    #[test]
    fn parse_display_assignment_supports_line_break_modes() {
        let a = parse_display_assignment("input:default=packet").unwrap();
        assert_eq!(a.encoding, None);
        assert_eq!(a.line_break, Some(LineBreakMode::Packet));

        let b = parse_display_assignment("default=line").unwrap();
        assert_eq!(b.encoding, None);
        assert_eq!(b.line_break, Some(LineBreakMode::Line));

        let c = parse_display_assignment("input:/dev/ttyUSB0=hex+packet").unwrap();
        assert_eq!(c.encoding, Some(PortDisplayMode::Hex));
        assert_eq!(c.line_break, Some(LineBreakMode::Packet));

        let d = parse_display_assignment("input:default=utf8+line").unwrap();
        assert_eq!(d.encoding, Some(PortDisplayMode::Utf8));
        assert_eq!(d.line_break, Some(LineBreakMode::Line));
    }

    #[test]
    fn parse_display_value_accepts_encoding_and_line_break_aliases() {
        assert_eq!(
            parse_display_value("hex+packet").unwrap(),
            (Some(PortDisplayMode::Hex), Some(LineBreakMode::Packet))
        );
        assert_eq!(
            parse_display_value("ascii+raw").unwrap(),
            (Some(PortDisplayMode::Ascii), Some(LineBreakMode::Packet))
        );
        assert_eq!(
            parse_display_value("line").unwrap(),
            (None, Some(LineBreakMode::Line))
        );
    }

    #[test]
    fn display_config_resolves_per_direction() {
        let mut config = PortDisplayConfig::default();
        config.set_both("default", PortDisplayMode::HexUtf8);
        config.set_for_stream(
            Some(PortDisplayStream::Input),
            "/dev/ttyUSB0",
            PortDisplayMode::Utf8,
        );
        config.set_for_stream(
            Some(PortDisplayStream::Output),
            "/dev/ttyUSB0",
            PortDisplayMode::Hex,
        );

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
        assert_eq!(
            config.resolve_input_override("/dev/ttyUSB1"),
            Some(PortDisplayMode::HexUtf8)
        );
        assert_eq!(
            config.resolve_output_override("/dev/ttyUSB1"),
            Some(PortDisplayMode::HexUtf8)
        );
    }

    #[test]
    fn display_config_resolves_line_break_mode() {
        let mut config = PortDisplayConfig::default();
        config.set_line_break_for_stream(
            Some(PortDisplayStream::Input),
            "default",
            LineBreakMode::Packet,
        );
        config.set_line_break_for_stream(
            Some(PortDisplayStream::Input),
            "/dev/ttyUSB0",
            LineBreakMode::Line,
        );

        assert_eq!(
            config.resolve_line_break_input("/dev/ttyUSB0"),
            LineBreakMode::Line
        );
        assert_eq!(
            config.resolve_line_break_input("/dev/ttyUSB1"),
            LineBreakMode::Packet
        );
    }
}
