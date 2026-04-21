use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const CLEAR_CURRENT_LINE: &str = "\r\x1b[2K";

pub struct ModeSoundPlayer {
    afplay_path: Option<String>,
    active_child: Option<Child>,
    sound_dir: PathBuf,
    warning_visible: bool,
}

impl ModeSoundPlayer {
    pub fn new() -> Self {
        Self {
            afplay_path: resolve_afplay_path(),
            active_child: None,
            sound_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join("sound"),
            warning_visible: false,
        }
    }

    pub fn play(&mut self, mode_name: &str) {
        let Some(afplay_path) = self.afplay_path.clone() else {
            return;
        };

        let sound_path = self.sound_dir.join(format!("{mode_name}.mp3"));
        if !sound_path.exists() {
            self.clear_warning();
            return;
        }

        self.clear_warning();
        self.stop_active_child();

        match Command::new(&afplay_path)
            .arg(&sound_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => self.active_child = Some(child),
            Err(error) => self.show_warning(format!(
                "failed to play sound {}: {}",
                sound_path.display(),
                error
            )),
        }
    }

    fn show_warning(&mut self, message: String) {
        let mut stderr = io::stderr().lock();
        let _ = write!(stderr, "{CLEAR_CURRENT_LINE}{message}");
        let _ = stderr.flush();
        self.warning_visible = true;
    }

    fn clear_warning(&mut self) {
        if !self.warning_visible {
            return;
        }

        let mut stderr = io::stderr().lock();
        let _ = write!(stderr, "{CLEAR_CURRENT_LINE}");
        let _ = stderr.flush();
        self.warning_visible = false;
    }

    fn stop_active_child(&mut self) {
        if let Some(mut child) = self.active_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ModeSoundPlayer {
    fn drop(&mut self) {
        self.stop_active_child();
        self.clear_warning();
    }
}

fn resolve_afplay_path() -> Option<String> {
    let absolute = Path::new("/usr/bin/afplay");
    if absolute.exists() {
        return Some(String::from("/usr/bin/afplay"));
    }

    if Command::new("afplay")
        .arg("-h")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
    {
        return Some(String::from("afplay"));
    }

    None
}
