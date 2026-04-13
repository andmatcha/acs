use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub struct ModeSoundPlayer {
    afplay_path: Option<String>,
    active_child: Option<Child>,
    sound_dir: PathBuf,
}

impl ModeSoundPlayer {
    pub fn new() -> Self {
        Self {
            afplay_path: resolve_afplay_path(),
            active_child: None,
            sound_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join("sound"),
        }
    }

    pub fn play(&mut self, mode_name: &str) {
        let Some(afplay_path) = self.afplay_path.clone() else {
            return;
        };

        let sound_path = self.sound_dir.join(format!("{mode_name}.mp3"));
        if !sound_path.exists() {
            eprintln!("sound file not found: {}", sound_path.display());
            return;
        }

        self.stop_active_child();

        match Command::new(&afplay_path)
            .arg(&sound_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => self.active_child = Some(child),
            Err(error) => eprintln!("failed to play sound {}: {}", sound_path.display(), error),
        }
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
