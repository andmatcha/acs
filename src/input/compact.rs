use std::fmt;

const BLUETOOTH_REPORT_LEN: usize = 10;
const USB_REPORT_LEN: usize = 64;
const NORMALIZED_LEN: usize = 10;
const COMPACT_LEN: usize = 8;

pub type CompactReport = [u8; COMPACT_LEN];

#[derive(Debug)]
pub enum CompactError {
    ReportTooShort { expected: usize, actual: usize },
    InvalidDpad(u8),
}

impl fmt::Display for CompactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReportTooShort { expected, actual } => {
                write!(
                    f,
                    "report too short: expected at least {expected} bytes, got {actual}"
                )
            }
            Self::InvalidDpad(value) => write!(f, "invalid d-pad value: {value}"),
        }
    }
}

pub fn convert_input_report(report: &[u8]) -> Result<CompactReport, CompactError> {
    let hid10 = normalize_input_report(report)?;
    convert_hid10(&hid10)
}

fn normalize_input_report(report: &[u8]) -> Result<[u8; NORMALIZED_LEN], CompactError> {
    if report.len() >= USB_REPORT_LEN {
        return Ok(report[..NORMALIZED_LEN]
            .try_into()
            .expect("slice has fixed size"));
    }

    if report.len() >= BLUETOOTH_REPORT_LEN {
        return Ok(report[..NORMALIZED_LEN]
            .try_into()
            .expect("slice has fixed size"));
    }

    Err(CompactError::ReportTooShort {
        expected: BLUETOOTH_REPORT_LEN,
        actual: report.len(),
    })
}

fn convert_hid10(hid10: &[u8; NORMALIZED_LEN]) -> Result<CompactReport, CompactError> {
    let dpad = hid10[5] & 0x0f;
    let dpad_bits = match dpad {
        0 => 0b0001,
        1 => 0b0011,
        2 => 0b0010,
        3 => 0b0110,
        4 => 0b0100,
        5 => 0b1100,
        6 => 0b1000,
        7 => 0b1001,
        8 => 0,
        value => return Err(CompactError::InvalidDpad(value)),
    };

    let compact0 = dpad_bits | (hid10[5] & 0xf0);
    let compact1 = (hid10[6] & 0x03) | ((hid10[6] >> 2) & 0x3c) | (hid10[7] & 0x03) << 6;

    Ok([
        compact0, compact1, hid10[1], hid10[2], hid10[3], hid10[4], hid10[8], hid10[9],
    ])
}

#[cfg(test)]
mod tests {
    use super::{CompactReport, convert_input_report};

    #[test]
    fn converts_bluetooth_example_from_requirements() {
        let report = [0x11, 255, 128, 1, 127, 7, 0x52, 0x03, 255, 64];

        assert_eq!(
            convert_input_report(&report).unwrap(),
            [0x09, 0xD6, 0xFF, 0x80, 0x01, 0x7F, 0xFF, 0x40]
        );
    }

    #[test]
    fn usb_and_bluetooth_with_same_prefix_produce_same_compact() {
        let bluetooth = [0x01, 10, 20, 30, 40, 0x21, 0x73, 0x03, 50, 60];
        let mut usb = [0u8; 64];
        usb[..10].copy_from_slice(&bluetooth);

        assert_eq!(convert_input_report(&bluetooth).unwrap(), expected_report());
        assert_eq!(convert_input_report(&usb).unwrap(), expected_report());
    }

    fn expected_report() -> CompactReport {
        [0x23, 0xDF, 10, 20, 30, 40, 50, 60]
    }
}
