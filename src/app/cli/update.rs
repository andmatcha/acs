use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use super::help::{is_help_flag, print_update_help};

const DEFAULT_REPO_URL: &str = "https://github.com/andmatcha/acs.git";
const PACKAGE_NAME: &str = env!("CARGO_PKG_NAME");

#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateAction {
    List,
    Latest,
    Version(String),
}

pub(crate) fn run(args: Vec<String>, bin_name: &str) -> ExitCode {
    if args.iter().any(|arg| is_help_flag(arg)) {
        print_update_help(bin_name);
        return ExitCode::SUCCESS;
    }

    let action = match parse_update_args(args) {
        Ok(action) => action,
        Err(error) => {
            eprintln!("{error}");
            print_update_help(bin_name);
            return ExitCode::from(2);
        }
    };

    match run_update_action(action) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn parse_update_args(args: Vec<String>) -> Result<UpdateAction, String> {
    let mut iter = args.into_iter();
    let Some(target) = iter.next() else {
        return Err(String::from(
            "更新対象が指定されていません。`list`、`latest`、または VERSION を指定してください。",
        ));
    };

    if let Some(extra) = iter.next() {
        return Err(format!("update に不要な引数があります: {extra}"));
    }

    match target.as_str() {
        "list" => Ok(UpdateAction::List),
        "latest" => Ok(UpdateAction::Latest),
        other if other.starts_with('-') => Err(format!("unknown option for update: {other}")),
        other if other.trim().is_empty() => Err(String::from(
            "更新対象が指定されていません。`list`、`latest`、または VERSION を指定してください。",
        )),
        other => Ok(UpdateAction::Version(other.to_owned())),
    }
}

fn run_update_action(action: UpdateAction) -> Result<(), String> {
    let repo_url = repo_url();
    let tags = fetch_remote_tags(&repo_url)?;
    if tags.is_empty() {
        return Err(format!(
            "GitHub 上に利用可能な tag がありません: {repo_url}"
        ));
    }

    match action {
        UpdateAction::List => {
            for tag in tags {
                println!("{tag}");
            }
            Ok(())
        }
        UpdateAction::Latest => {
            let tag = tags
                .first()
                .ok_or_else(|| format!("GitHub 上に利用可能な tag がありません: {repo_url}"))?;
            update_to_tag(&repo_url, tag)
        }
        UpdateAction::Version(version) => {
            let tag = resolve_requested_tag(&version, &tags)?;
            update_to_tag(&repo_url, &tag)
        }
    }
}

fn repo_url() -> String {
    env::var("ACS_REPO_URL")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_REPO_URL.to_owned())
}

fn fetch_remote_tags(repo_url: &str) -> Result<Vec<String>, String> {
    let mut command = Command::new("git");
    command.args([
        "ls-remote",
        "--tags",
        "--refs",
        "--sort=-v:refname",
        repo_url,
    ]);
    let output = read_command_output(command, "GitHub tag の取得に失敗しました")?;
    Ok(parse_ls_remote_tags(&output))
}

fn parse_ls_remote_tags(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.split_once("refs/tags/").map(|(_, tag)| tag.trim()))
        .filter(|tag| !tag.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn resolve_requested_tag(version: &str, tags: &[String]) -> Result<String, String> {
    let requested = version.trim();
    if requested.is_empty() {
        return Err(String::from("更新する VERSION が空です"));
    }

    if tags.iter().any(|tag| tag == requested) {
        return Ok(requested.to_owned());
    }

    if !requested.starts_with('v') && !requested.starts_with('V') {
        let prefixed = format!("v{requested}");
        if tags.iter().any(|tag| tag == &prefixed) {
            return Ok(prefixed);
        }
    }

    Err(format!(
        "指定された VERSION は GitHub tag にありません: {requested}"
    ))
}

fn update_to_tag(repo_url: &str, tag: &str) -> Result<(), String> {
    println!("{PACKAGE_NAME} を GitHub tag {tag} へ更新します");

    let temp_dir = TempDir::new("acs-update")
        .map_err(|error| format!("一時ディレクトリを作成できませんでした: {error}"))?;
    let source_dir = temp_dir.path().join("source");

    clone_remote_tag(repo_url, tag, &source_dir)?;
    let commit = git_commit_id(&source_dir)?;
    install_binary_from_source(&source_dir, tag, &commit)?;

    println!("{PACKAGE_NAME} の更新が完了しました: {tag}");
    Ok(())
}

fn clone_remote_tag(repo_url: &str, tag: &str, source_dir: &Path) -> Result<(), String> {
    let mut command = Command::new("git");
    command.args(["-c", "advice.detachedHead=false", "clone"]);
    command.args(["--depth", "1", "--branch", tag, repo_url]);
    command.arg(source_dir);
    run_command(command, "GitHub tag の clone に失敗しました")
}

fn git_commit_id(source_dir: &Path) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(source_dir)
        .args(["rev-parse", "HEAD"]);
    read_command_output(command, "更新元 commit の取得に失敗しました")
}

fn install_binary_from_source(source_dir: &Path, tag: &str, commit: &str) -> Result<(), String> {
    let mut command = Command::new(cargo_command());
    command
        .arg("install")
        .arg("--path")
        .arg(source_dir)
        .args(["--locked", "--force"]);
    command.env("ACS_BUILD_BRANCH", "detached");
    command.env("ACS_BUILD_COMMIT", commit);
    command.env("ACS_BUILD_DIRTY", "clean");
    command.env("ACS_BUILD_SOURCE_KIND", "remote-tag");
    command.env("ACS_BUILD_SOURCE_REF", tag);
    run_command(command, "cargo install に失敗しました")
}

fn cargo_command() -> OsString {
    if let Some(cargo) = env::var_os("CARGO").filter(|value| !value.is_empty()) {
        return cargo;
    }

    if let Some(cargo_home) = env::var_os("CARGO_HOME") {
        let candidate = PathBuf::from(cargo_home)
            .join("bin")
            .join(cargo_binary_name());
        if candidate.is_file() {
            return candidate.into_os_string();
        }
    }

    if let Some(home) = env::var_os("HOME") {
        let candidate = PathBuf::from(home)
            .join(".cargo")
            .join("bin")
            .join(cargo_binary_name());
        if candidate.is_file() {
            return candidate.into_os_string();
        }
    }

    OsString::from(cargo_binary_name())
}

fn cargo_binary_name() -> &'static str {
    if cfg!(windows) { "cargo.exe" } else { "cargo" }
}

fn read_command_output(mut command: Command, failure_message: &str) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|error| format!("{failure_message}: {error}"))?;
    if !output.status.success() {
        return Err(format_command_failure(failure_message, &output.stderr));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("{failure_message}: stdout is not UTF-8 ({error})"))?;
    Ok(stdout.trim().to_owned())
}

fn run_command(mut command: Command, failure_message: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("{failure_message}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{failure_message}: exit status {status}"))
    }
}

fn format_command_failure(message: &str, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("{message}: command failed")
    } else {
        format!("{message}: {stderr}")
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> std::io::Result<Self> {
        let base = env::temp_dir();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for attempt in 0..100 {
            let path = base.join(format!(
                "{prefix}.{}.{}.{}",
                std::process::id(),
                now,
                attempt
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "failed to create a unique temporary directory",
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateAction, parse_ls_remote_tags, parse_update_args, resolve_requested_tag};

    #[test]
    fn parse_update_args_requires_target() {
        assert!(parse_update_args(Vec::new()).is_err());
    }

    #[test]
    fn parse_update_args_accepts_list_latest_and_version() {
        assert_eq!(
            parse_update_args(vec![String::from("list")]).unwrap(),
            UpdateAction::List
        );
        assert_eq!(
            parse_update_args(vec![String::from("latest")]).unwrap(),
            UpdateAction::Latest
        );
        assert_eq!(
            parse_update_args(vec![String::from("v1.2.3")]).unwrap(),
            UpdateAction::Version(String::from("v1.2.3"))
        );
    }

    #[test]
    fn parse_update_args_rejects_extra_arguments() {
        assert!(parse_update_args(vec![String::from("latest"), String::from("v1.2.3")]).is_err());
    }

    #[test]
    fn parse_ls_remote_tags_extracts_tag_names() {
        let output = "\
abcd1234\trefs/tags/v1.2.3
beef5678\trefs/tags/v1.2.2
ignored\trefs/heads/main
";
        assert_eq!(
            parse_ls_remote_tags(output),
            vec![String::from("v1.2.3"), String::from("v1.2.2")]
        );
    }

    #[test]
    fn resolve_requested_tag_accepts_exact_or_unprefixed_version() {
        let tags = vec![String::from("v1.2.3"), String::from("release-test")];
        assert_eq!(resolve_requested_tag("v1.2.3", &tags).unwrap(), "v1.2.3");
        assert_eq!(resolve_requested_tag("1.2.3", &tags).unwrap(), "v1.2.3");
        assert_eq!(
            resolve_requested_tag("release-test", &tags).unwrap(),
            "release-test"
        );
    }

    #[test]
    fn resolve_requested_tag_rejects_missing_tag() {
        let tags = vec![String::from("v1.2.3")];
        assert!(resolve_requested_tag("v9.9.9", &tags).is_err());
    }
}
