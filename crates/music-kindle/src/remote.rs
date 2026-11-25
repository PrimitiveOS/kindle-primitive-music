//! The app's side of the player. The player is a separate process (`music-kindle --player`) that keeps playing when the
//! app closes. The app writes commands to a control file in RAM, starting the player for `Play`, and reads the player's
//! status file. The saved place is in the music folder, for when no player runs.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command as Process, Stdio};

use music_core::{Command, PlayState, Saved, Snapshot, Track};
use music_render::Remote;

/// The installed program, started again as the player.
pub const PROGRAM: &str = "/mnt/us/music/bin/music-kindle";
/// Commands from the app. The player takes them away as it reads them.
pub const CONTROL: &str = "/var/tmp/music-control";
/// The player's status, rewritten every second (RAM).
pub const STATUS: &str = "/var/tmp/music-status";
/// The player's process id. Removed when the player exits.
pub const PLAYER_PID: &str = "/var/tmp/music-player.pid";
/// The place where playback stopped or paused (the music folder, so it survives a restart).
pub const PLACE: &str = "/mnt/us/music/state.txt";

/// Whether a player process is running.
pub fn player_running() -> bool {
    let Ok(text) = fs::read_to_string(PLAYER_PID) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<i32>() else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// The name of the connected Bluetooth audio device, from the stock Bluetooth stack.
pub fn headphones() -> Option<String> {
    let output = Process::new("lipc-get-prop")
        .args(["com.lab126.btfd", "BTconnectedDevName"])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !name.is_empty()).then_some(name)
}

pub struct FilePlayer {
    tracks: Vec<Track>,
}

impl FilePlayer {
    pub fn new(tracks: Vec<Track>) -> Self {
        Self { tracks }
    }
}

impl Remote for FilePlayer {
    fn snapshot(&self) -> Snapshot {
        if let Ok(text) = fs::read_to_string(STATUS) {
            let mut snapshot = Snapshot::from_text(&text);
            // A status that says playing, from a player that has since gone, is stopped at its place.
            if snapshot.state != PlayState::Stopped && !player_running() {
                snapshot.state = PlayState::Stopped;
            }
            return snapshot;
        }
        let saved = Saved::from_text(&fs::read_to_string(PLACE).unwrap_or_default());
        Snapshot {
            state: PlayState::Stopped,
            index: saved.index_in(&self.tracks),
            position: saved.position,
            message: None,
        }
    }

    fn send(&mut self, command: Command) -> Result<(), String> {
        if !player_running() {
            // Only Play starts a player; other commands have nothing to act on.
            if !matches!(command, Command::Play { .. }) {
                return Ok(());
            }
            // Old lines must not reach the new player.
            let _ = fs::remove_file(CONTROL);
            Process::new(PROGRAM)
                .arg("--player")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .map_err(|e| format!("could not start the player: {e}"))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(CONTROL)
            .map_err(|e| format!("could not send the command: {e}"))?;
        writeln!(file, "{}", command.to_line()).map_err(|e| format!("could not send the command: {e}"))
    }
}
