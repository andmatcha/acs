use std::path::PathBuf;

pub(crate) use crate::common::{
    format_bytes_ascii, format_bytes_hex, now_display_timestamp, now_file_timestamp,
};

pub(crate) fn default_baud_rate() -> u32 {
    115_200
}

pub(crate) fn default_log_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("logs")
}

pub(crate) fn dedup_strings(values: Vec<String>) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        if !unique.iter().any(|existing| existing == &value) {
            unique.push(value);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::dedup_strings;

    #[test]
    fn dedup_strings_preserves_first_seen_order() {
        assert_eq!(
            dedup_strings(vec![
                String::from("A"),
                String::from("B"),
                String::from("A"),
                String::from("C"),
            ]),
            vec!["A", "B", "C"]
        );
    }
}
