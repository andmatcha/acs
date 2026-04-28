use std::env;
use std::path::PathBuf;

const LOCAL_LOG_DIR_NAME: &str = "logs";

pub(crate) fn default_log_dir() -> PathBuf {
    current_dir_or_dot().join(LOCAL_LOG_DIR_NAME)
}

fn current_dir_or_dot() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::default_log_dir;

    #[test]
    fn default_log_dir_ends_with_logs() {
        assert!(default_log_dir().ends_with("logs"));
    }
}
