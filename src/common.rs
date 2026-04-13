use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn now_display_timestamp() -> String {
    local_timestamp("%Y-%m-%d %H:%M:%S")
}

pub(crate) fn now_file_timestamp() -> String {
    local_timestamp("%Y%m%d_%H%M%S")
}

pub(crate) fn format_bytes_hex(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::from("(empty)");
    }

    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn format_bytes_ascii(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::from("\"\"");
    }

    let escaped = bytes
        .iter()
        .flat_map(|byte| std::ascii::escape_default(*byte))
        .map(char::from)
        .collect::<String>();
    format!("\"{escaped}\"")
}

#[cfg(unix)]
fn local_timestamp(format: &str) -> String {
    use std::ffi::CStr;
    use std::mem::MaybeUninit;

    let now = unsafe { libc::time(std::ptr::null_mut()) };
    if now < 0 {
        return unix_fallback();
    }

    let mut local_time = MaybeUninit::<libc::tm>::uninit();
    let result = unsafe { libc::localtime_r(&now, local_time.as_mut_ptr()) };
    if result.is_null() {
        return unix_fallback();
    }

    let format = match std::ffi::CString::new(format) {
        Ok(value) => value,
        Err(_) => return unix_fallback(),
    };

    let mut buffer = [0i8; 32];
    let written = unsafe {
        libc::strftime(
            buffer.as_mut_ptr(),
            buffer.len(),
            format.as_ptr(),
            local_time.as_ptr(),
        )
    };

    if written == 0 {
        return unix_fallback();
    }

    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(not(unix))]
fn local_timestamp(_: &str) -> String {
    unix_fallback()
}

fn unix_fallback() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{format_bytes_ascii, format_bytes_hex};

    #[test]
    fn format_bytes_hex_handles_empty_input() {
        assert_eq!(format_bytes_hex(&[]), "(empty)");
    }

    #[test]
    fn format_bytes_ascii_escapes_control_bytes() {
        assert_eq!(format_bytes_ascii(b"OK\r\n"), "\"OK\\r\\n\"");
    }
}
