use super::paths::default_log_dir as default_log_dir_from_paths;
use crate::port_display::{LineBreakMode, PortDisplayMode, parse_display_value};

pub(crate) fn default_baud_rate() -> u32 {
    115_200
}

pub(crate) fn default_log_dir() -> std::path::PathBuf {
    default_log_dir_from_paths()
}

pub(crate) fn next_value(
    iter: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("missing value for {option}"))
}

pub(crate) fn parse_u32_arg(option: &str, value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("invalid value for {option}: {value}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyValueArg {
    pub key: String,
    pub value: String,
}

pub(crate) fn parse_key_value_args(option: &str, value: &str) -> Result<Vec<KeyValueArg>, String> {
    if value.trim().is_empty() {
        return Err(format!("missing value for {option}"));
    }

    let mut args = Vec::new();
    for assignment in value.split(',') {
        let assignment = assignment.trim();
        let Some((key, raw_value)) = assignment.split_once('=') else {
            return Err(format!(
                "invalid value for {option}: {value} (expected KEY=VALUE or KEY=VALUE,KEY=VALUE)"
            ));
        };
        let key = key.trim();
        let raw_value = raw_value.trim();
        if key.is_empty() || raw_value.is_empty() {
            return Err(format!(
                "invalid value for {option}: {value} (expected KEY=VALUE or KEY=VALUE,KEY=VALUE)"
            ));
        }
        args.push(KeyValueArg {
            key: key.to_owned(),
            value: raw_value.to_owned(),
        });
    }

    Ok(args)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PortSpec {
    pub port: String,
    pub baud: Option<u32>,
    pub display_mode: Option<PortDisplayMode>,
    pub line_break_mode: Option<LineBreakMode>,
}

impl PortSpec {
    pub(crate) fn normalized(self) -> Option<Self> {
        let trimmed = self.port.trim();
        (!trimmed.is_empty()).then_some(Self {
            port: trimmed.to_owned(),
            baud: self.baud,
            display_mode: self.display_mode,
            line_break_mode: self.line_break_mode,
        })
    }
}

pub(crate) fn parse_port_spec(option: &str, value: &str) -> Result<PortSpec, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{option} must not be empty"));
    }

    let (port_and_baud, display_mode, line_break_mode) = match value.rsplit_once(',') {
        Some((port_and_baud, mode)) => match parse_display_value(mode) {
            Ok((display_mode, line_break_mode))
                if display_mode.is_some() || line_break_mode.is_some() =>
            {
                (port_and_baud, display_mode, line_break_mode)
            }
            _ => (value, None, None),
        },
        None => (value, None, None),
    };

    let (port, baud) = match port_and_baud.rsplit_once('@') {
        Some((port, baud)) => (
            port,
            Some(parse_u32_arg(option, baud.trim()).map_err(|_| {
                format!("invalid value for {option}: {value} (expected PORT[@BAUD][,DISPLAY])")
            })?),
        ),
        None => (port_and_baud, None),
    };

    let port = port.trim();
    if port.is_empty() {
        return Err(format!(
            "invalid value for {option}: {value} (expected PORT[@BAUD][,DISPLAY])"
        ));
    }

    Ok(PortSpec {
        port: port.to_owned(),
        baud,
        display_mode,
        line_break_mode,
    })
}

#[cfg(test)]
mod tests {
    use super::{KeyValueArg, PortSpec, parse_key_value_args, parse_port_spec};
    use crate::port_display::{LineBreakMode, PortDisplayMode};

    #[test]
    fn parse_port_spec_accepts_baud_and_display_suffixes() {
        assert_eq!(
            parse_port_spec("--port", "/dev/ttyUSB0@921600,utf8").unwrap(),
            PortSpec {
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
                display_mode: Some(PortDisplayMode::Utf8),
                line_break_mode: None,
            }
        );
        assert_eq!(
            parse_port_spec("--port", "/dev/ttyUSB1,hex+ascii").unwrap(),
            PortSpec {
                port: String::from("/dev/ttyUSB1"),
                baud: None,
                display_mode: Some(PortDisplayMode::HexAscii),
                line_break_mode: None,
            }
        );
        assert_eq!(
            parse_port_spec("--port", "/dev/ttyUSB2,hex+packet").unwrap(),
            PortSpec {
                port: String::from("/dev/ttyUSB2"),
                baud: None,
                display_mode: Some(PortDisplayMode::Hex),
                line_break_mode: Some(LineBreakMode::Packet),
            }
        );
        assert_eq!(
            parse_port_spec("--port", "0@460800,hex").unwrap(),
            PortSpec {
                port: String::from("0"),
                baud: Some(460_800),
                display_mode: Some(PortDisplayMode::Hex),
                line_break_mode: None,
            }
        );
    }

    #[test]
    fn port_spec_normalized_skips_empty_port_names() {
        assert_eq!(
            PortSpec {
                port: String::from(" "),
                baud: Some(115_200),
                display_mode: None,
                line_break_mode: None,
            }
            .normalized(),
            None
        );
        assert_eq!(
            PortSpec {
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
                display_mode: Some(PortDisplayMode::Hex),
                line_break_mode: Some(LineBreakMode::Packet),
            }
            .normalized(),
            Some(PortSpec {
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
                display_mode: Some(PortDisplayMode::Hex),
                line_break_mode: Some(LineBreakMode::Packet),
            })
        );
    }

    #[test]
    fn parse_key_value_args_accepts_csv_assignments() {
        assert_eq!(
            parse_key_value_args("--config", "FORMAT=PacketACv6,RATE=100,DISPLAY=input:default=utf8+packet")
                .unwrap(),
            vec![
                KeyValueArg {
                    key: String::from("FORMAT"),
                    value: String::from("PacketACv6"),
                },
                KeyValueArg {
                    key: String::from("RATE"),
                    value: String::from("100"),
                },
                KeyValueArg {
                    key: String::from("DISPLAY"),
                    value: String::from("input:default=utf8+packet"),
                },
            ]
        );
    }
}
