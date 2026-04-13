use std::env;
use std::path::{Path, PathBuf};

pub(crate) const LOCAL_CONFIG_FILE_NAME: &str = "acs.config.json";
pub(crate) const LOCAL_CONFIG_DIR_NAME: &str = "config";
const LOCAL_LOG_DIR_NAME: &str = "logs";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigLookup {
    Explicit(PathBuf),
    LocalDefault(PathBuf),
    GlobalDefault(PathBuf),
    None,
}

impl ConfigLookup {
    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Self::Explicit(path) | Self::LocalDefault(path) | Self::GlobalDefault(path) => {
                Some(path.as_path())
            }
            Self::None => None,
        }
    }
}

pub(crate) fn default_config_help() -> String {
    match global_config_dir() {
        Some(path) => format!(
            "./{LOCAL_CONFIG_DIR_NAME}/, ./{LOCAL_CONFIG_FILE_NAME}, then {}",
            path.display()
        ),
        None => format!("./{LOCAL_CONFIG_DIR_NAME}/, fallback: ./{LOCAL_CONFIG_FILE_NAME}"),
    }
}

pub(crate) fn default_log_help() -> String {
    match global_log_dir() {
        Some(path) => format!("./logs or {}", path.display()),
        None => String::from("./logs"),
    }
}

pub(crate) fn find_default_config_lookup() -> ConfigLookup {
    if let Some(path) = find_local_default_config_path() {
        return ConfigLookup::LocalDefault(path);
    }

    if let Some(path) = global_config_dir().filter(|path| path.is_dir()) {
        return ConfigLookup::GlobalDefault(path);
    }

    ConfigLookup::None
}

pub(crate) fn default_log_dir(lookup: &ConfigLookup) -> PathBuf {
    match lookup {
        ConfigLookup::Explicit(path) => explicit_log_dir(path),
        ConfigLookup::LocalDefault(_) | ConfigLookup::None => local_log_dir(),
        ConfigLookup::GlobalDefault(_) => global_log_dir().unwrap_or_else(local_log_dir),
    }
}

pub(crate) fn global_config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        user_home_dir().map(|home| home.join("Library/Application Support/acs"))
    }

    #[cfg(target_os = "windows")]
    {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| user_home_dir().map(|home| home.join("AppData/Roaming")))
            .map(|base| base.join("acs"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| user_home_dir().map(|home| home.join(".config")))
            .map(|base| base.join("acs"))
    }
}

pub(crate) fn global_log_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        user_home_dir().map(|home| home.join("Library/Logs/acs"))
    }

    #[cfg(target_os = "windows")]
    {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| user_home_dir().map(|home| home.join("AppData/Local")))
            .map(|base| base.join("acs/logs"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| user_home_dir().map(|home| home.join(".local/state")))
            .map(|base| base.join("acs/logs"))
    }
}

fn find_local_default_config_path() -> Option<PathBuf> {
    let current_dir = current_dir_or_dot();
    let config_dir = current_dir.join(LOCAL_CONFIG_DIR_NAME);
    if config_dir.is_dir() {
        return Some(config_dir);
    }

    let config_file = current_dir.join(LOCAL_CONFIG_FILE_NAME);
    config_file.is_file().then_some(config_file)
}

fn local_log_dir() -> PathBuf {
    current_dir_or_dot().join(LOCAL_LOG_DIR_NAME)
}

fn explicit_log_dir(path: &Path) -> PathBuf {
    let absolute_path = absolute_path(path);
    let local_config_dir = current_dir_or_dot().join(LOCAL_CONFIG_DIR_NAME);
    let local_config_file = current_dir_or_dot().join(LOCAL_CONFIG_FILE_NAME);
    if absolute_path == local_config_dir
        || absolute_path == local_config_file
        || absolute_path.starts_with(&local_config_dir)
    {
        return local_log_dir();
    }

    if global_config_dir()
        .as_deref()
        .is_some_and(|global_config_dir| {
            absolute_path == global_config_dir || absolute_path.starts_with(global_config_dir)
        })
    {
        return global_log_dir().unwrap_or_else(local_log_dir);
    }

    let base_dir = if absolute_path.is_dir() {
        absolute_path
    } else {
        absolute_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(local_log_dir)
    };
    base_dir.join(LOCAL_LOG_DIR_NAME)
}

fn current_dir_or_dot() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir_or_dot().join(path)
    }
}

fn user_home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::{ConfigLookup, default_log_dir};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn explicit_config_directory_uses_sibling_logs_directory() {
        let temp_dir = make_temp_dir("acs_paths_config_dir");
        assert_eq!(
            default_log_dir(&ConfigLookup::Explicit(temp_dir.clone())),
            temp_dir.join("logs")
        );
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn explicit_config_file_uses_parent_logs_directory() {
        assert_eq!(
            default_log_dir(&ConfigLookup::Explicit(PathBuf::from(
                "/tmp/acs/config.json"
            ))),
            PathBuf::from("/tmp/acs/logs")
        );
    }

    #[test]
    fn explicit_local_config_directory_keeps_repo_local_logs_directory() {
        assert!(
            default_log_dir(&ConfigLookup::Explicit(PathBuf::from("config"))).ends_with("logs")
        );
        assert!(
            !default_log_dir(&ConfigLookup::Explicit(PathBuf::from("config")))
                .ends_with("config/logs")
        );
    }

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}_{}_{}", std::process::id(), unique));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
