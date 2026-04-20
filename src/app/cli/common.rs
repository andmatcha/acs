use super::paths::{ConfigLookup, default_log_dir as default_log_dir_for_lookup};
use crate::port_display::PortDisplayMode;

pub(crate) fn default_baud_rate() -> u32 {
    115_200
}

pub(crate) fn default_log_dir(lookup: &ConfigLookup) -> std::path::PathBuf {
    default_log_dir_for_lookup(lookup)
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PortSpec {
    pub port: String,
    pub baud: Option<u32>,
    pub display_mode: Option<PortDisplayMode>,
}

impl PortSpec {
    pub(crate) fn normalized(self) -> Option<Self> {
        let trimmed = self.port.trim();
        (!trimmed.is_empty()).then_some(Self {
            port: trimmed.to_owned(),
            baud: self.baud,
            display_mode: self.display_mode,
        })
    }
}

pub(crate) fn resolve_requested_port_spec(
    cli_port: Option<PortSpec>,
    config_port: Option<PortSpec>,
) -> Option<PortSpec> {
    cli_port
        .and_then(PortSpec::normalized)
        .or_else(|| config_port.and_then(PortSpec::normalized))
}

pub(crate) fn parse_port_spec(option: &str, value: &str) -> Result<PortSpec, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{option} must not be empty"));
    }

    let (port_and_baud, display_mode) = match value.rsplit_once(',') {
        Some((port_and_baud, mode)) => match PortDisplayMode::parse(mode) {
            Ok(mode) => (port_and_baud, Some(mode)),
            Err(_) => (value, None),
        },
        None => (value, None),
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
    })
}

pub(crate) fn merge_port_specs(target: &mut Vec<PortSpec>, specs: Vec<PortSpec>) {
    for spec in specs {
        if let Some(existing) = target
            .iter_mut()
            .find(|existing| existing.port == spec.port)
        {
            *existing = spec;
        } else {
            target.push(spec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PortSpec, merge_port_specs, parse_port_spec, resolve_requested_port_spec};
    use crate::port_display::PortDisplayMode;

    #[test]
    fn parse_port_spec_accepts_baud_and_display_suffixes() {
        assert_eq!(
            parse_port_spec("--port", "/dev/ttyUSB0@921600,utf8").unwrap(),
            PortSpec {
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
                display_mode: Some(PortDisplayMode::Utf8),
            }
        );
        assert_eq!(
            parse_port_spec("--port", "/dev/ttyUSB1,hex+ascii").unwrap(),
            PortSpec {
                port: String::from("/dev/ttyUSB1"),
                baud: None,
                display_mode: Some(PortDisplayMode::HexAscii),
            }
        );
    }

    #[test]
    fn resolve_requested_port_spec_skips_empty_port_names() {
        assert_eq!(
            resolve_requested_port_spec(
                Some(PortSpec {
                    port: String::from(" "),
                    baud: Some(115_200),
                    display_mode: None,
                }),
                Some(PortSpec {
                    port: String::from("/dev/ttyUSB0"),
                    baud: Some(921_600),
                    display_mode: Some(PortDisplayMode::Hex),
                })
            ),
            Some(PortSpec {
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
                display_mode: Some(PortDisplayMode::Hex),
            })
        );
    }

    #[test]
    fn merge_port_specs_replaces_existing_entry_for_same_port() {
        let mut specs = vec![PortSpec {
            port: String::from("/dev/ttyUSB0"),
            baud: Some(115_200),
            display_mode: None,
        }];
        merge_port_specs(
            &mut specs,
            vec![PortSpec {
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
                display_mode: Some(PortDisplayMode::Hex),
            }],
        );

        assert_eq!(
            specs,
            vec![PortSpec {
                port: String::from("/dev/ttyUSB0"),
                baud: Some(921_600),
                display_mode: Some(PortDisplayMode::Hex),
            }]
        );
    }
}
