use crate::output::OutputFormat;
use crate::ui::live_monitor::DisplayMode;
use std::env;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitCode;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

static LIVE_MONITOR_STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
const SIGINT: i32 = 2;
const BACKGROUND_RUN_ENV: &str = "DS4MAP_BACKGROUND_RUN";
const RUNTIME_STATE_DIR: &str = "ds4map";
const RUN_PID_FILE: &str = "run.pid";
const RUN_INFO_FILE: &str = "run.info";
const STOP_REQUEST_FILE: &str = "run.stop";

#[derive(Debug, Clone)]
pub(crate) struct RuntimeRegistration {
    pub display_mode: DisplayMode,
    pub output_format: Option<OutputFormat>,
    pub port: Option<String>,
    pub baud_rate: Option<u32>,
    pub log_file: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeStatus {
    pub pid: u32,
    pub display_mode: DisplayMode,
    pub output_format: Option<OutputFormat>,
    pub port: Option<String>,
    pub baud_rate: Option<u32>,
    pub log_file: Option<String>,
    pub is_running: bool,
}

pub(crate) fn should_background_run(display_mode: DisplayMode) -> bool {
    display_mode == DisplayMode::None && env::var_os(BACKGROUND_RUN_ENV).is_none()
}

pub(crate) fn spawn_background_run(args: &[String]) -> ExitCode {
    let exe = match env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("failed to resolve executable path: {error}");
            return ExitCode::from(1);
        }
    };

    let mut command = Command::new(exe);
    command
        .arg("run")
        .args(args)
        .env(BACKGROUND_RUN_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    configure_background_command(&mut command);

    match command.spawn() {
        Ok(_) => {
            println!("started in background");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to start background run: {error}");
            ExitCode::from(1)
        }
    }
}

pub(crate) fn install_live_monitor_sigint_handler() {
    install_sigint_handler_impl();
}

pub(crate) fn should_stop_running() -> bool {
    LIVE_MONITOR_STOP_REQUESTED.load(Ordering::SeqCst) || stop_request_path().exists()
}

pub(crate) fn request_live_monitor_stop() {
    LIVE_MONITOR_STOP_REQUESTED.store(true, Ordering::SeqCst);
}

pub(crate) fn request_stop() -> Result<(), String> {
    fs::create_dir_all(runtime_state_dir())
        .map_err(|error| format!("failed to prepare runtime state: {error}"))?;
    fs::write(stop_request_path(), b"stop\n")
        .map_err(|error| format!("failed to request stop: {error}"))
}

pub(crate) fn stop_requested() -> bool {
    stop_request_path().exists()
}

pub(crate) fn read_runtime_status() -> Option<RuntimeStatus> {
    let info = fs::read_to_string(run_info_path()).ok()?;
    let mut pid = None;
    let mut display_mode = None;
    let mut output_format = None;
    let mut port = None;
    let mut baud_rate = None;
    let mut log_file = None;

    for line in info.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "pid" => pid = value.parse::<u32>().ok(),
            "monitor" => display_mode = DisplayMode::parse(value).ok(),
            "format" => output_format = OutputFormat::parse(value).ok(),
            "port" if !value.is_empty() => port = Some(String::from(value)),
            "baud" if !value.is_empty() => baud_rate = value.parse::<u32>().ok(),
            "log_file" if !value.is_empty() => log_file = Some(String::from(value)),
            _ => {}
        }
    }

    let pid = pid?;
    Some(RuntimeStatus {
        pid,
        display_mode: display_mode?,
        output_format,
        port,
        baud_rate,
        log_file,
        is_running: is_process_running(pid),
    })
}

pub(crate) fn cleanup_runtime_state() {
    let _ = fs::remove_file(run_pid_path());
    let _ = fs::remove_file(run_info_path());
    let _ = fs::remove_file(stop_request_path());
}

pub(crate) struct RunStateGuard {
    pid_path: PathBuf,
    info_path: PathBuf,
    stop_path: PathBuf,
}

impl RunStateGuard {
    pub(crate) fn acquire(config: &RuntimeRegistration) -> Result<Self, io::Error> {
        let state_dir = runtime_state_dir();
        fs::create_dir_all(&state_dir)?;
        let pid_path = state_dir.join(RUN_PID_FILE);
        let info_path = state_dir.join(RUN_INFO_FILE);
        let stop_path = state_dir.join(STOP_REQUEST_FILE);
        let _ = fs::remove_file(&stop_path);
        let pid = std::process::id();
        fs::write(&pid_path, format!("{pid}\n"))?;
        fs::write(
            &info_path,
            format!(
                "pid={pid}\nmonitor={}\nformat={}\nport={}\nbaud={}\nlog_file={}\n",
                config.display_mode.as_str(),
                config
                    .output_format
                    .map(|format| format.as_str())
                    .unwrap_or(""),
                config.port.as_deref().unwrap_or(""),
                config
                    .baud_rate
                    .map(|baud| baud.to_string())
                    .unwrap_or_default(),
                config.log_file.as_deref().unwrap_or(""),
            ),
        )?;
        Ok(Self {
            pid_path,
            info_path,
            stop_path,
        })
    }
}

impl Drop for RunStateGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.pid_path);
        let _ = fs::remove_file(&self.info_path);
        let _ = fs::remove_file(&self.stop_path);
    }
}

#[cfg(unix)]
fn configure_background_command(command: &mut Command) {
    unsafe {
        command.pre_exec(detach_background_process);
    }
}

#[cfg(not(unix))]
fn configure_background_command(_: &mut Command) {}

#[cfg(unix)]
fn detach_background_process() -> io::Result<()> {
    unsafe extern "C" {
        fn setsid() -> i32;
    }

    let result = unsafe { setsid() };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(unix)]
fn install_sigint_handler_impl() {
    type SignalHandler = unsafe extern "C" fn(i32);

    unsafe extern "C" fn handle_sigint(_: i32) {
        LIVE_MONITOR_STOP_REQUESTED.store(true, Ordering::SeqCst);
    }

    unsafe extern "C" {
        fn signal(signum: i32, handler: SignalHandler) -> SignalHandler;
    }

    unsafe {
        signal(SIGINT, handle_sigint);
    }
}

#[cfg(not(unix))]
fn install_sigint_handler_impl() {}

fn runtime_state_dir() -> PathBuf {
    env::temp_dir().join(RUNTIME_STATE_DIR)
}

fn run_pid_path() -> PathBuf {
    runtime_state_dir().join(RUN_PID_FILE)
}

fn run_info_path() -> PathBuf {
    runtime_state_dir().join(RUN_INFO_FILE)
}

fn stop_request_path() -> PathBuf {
    runtime_state_dir().join(STOP_REQUEST_FILE)
}

#[cfg(unix)]
fn is_process_running(pid: u32) -> bool {
    use std::ffi::c_int;

    unsafe extern "C" {
        fn kill(pid: c_int, sig: c_int) -> c_int;
    }

    let result = unsafe { kill(pid as c_int, 0) };
    if result == 0 {
        return true;
    }

    let error = io::Error::last_os_error();
    matches!(error.raw_os_error(), Some(1))
}

#[cfg(not(unix))]
fn is_process_running(_: u32) -> bool {
    run_pid_path().exists()
}
