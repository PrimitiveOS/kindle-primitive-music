//! The app's side of the player. The player is a separate process (`music-kindle --player`) that keeps playing when the
//! app closes. The app writes commands to a control file in RAM, starting the player for `Play`, and reads the player's
//! status file. The saved place is in the music folder, for when no player runs.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command as Process, Stdio};

use music_core::{Command, PlayState, Saved, Snapshot, Track};
use music_render::{Remote, Sound, SOUND_LEVELS};

/// The installed program, started again as the player. `MUSIC_PROGRAM` overrides it (for tests on a computer).
pub fn program() -> String {
    std::env::var("MUSIC_PROGRAM").unwrap_or_else(|_| "/mnt/us/music/bin/music-kindle".into())
}
/// Commands from the app. The player takes them away as it reads them.
pub const CONTROL: &str = "/var/tmp/music-control";
/// The player's status, rewritten every second (RAM).
pub const STATUS: &str = "/var/tmp/music-status";
/// The media session the launcher's quick settings reads (docs/contracts.md §1.2), and the commands it sends.
pub const MEDIA_NOW: &str = "/var/tmp/media/music.now";
pub const MEDIA_CMD: &str = "/var/tmp/media/music.cmd";
/// The player's process id. Removed when the player exits.
pub const PLAYER_PID: &str = "/var/tmp/music-player.pid";

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

/// The name of the connected Bluetooth device, from the stack's `ListConnected` report (`lipc-hash-prop -n`). The
/// list is read now, not cached: a headset put in its case leaves the list. None when nothing is connected.
pub fn connected_name_in(report: &str) -> Option<String> {
    for block in report.split("Hash Index").skip(1) {
        let mut name = None;
        let mut connected = false;
        for line in block.lines() {
            if let Some((key, value)) = line.split_once('=') {
                let value = value.trim();
                match key.trim() {
                    "bd_name" => name = Some(value.trim_matches('"').to_string()),
                    "is_connected" => connected = matches!(value, "(1)" | "1" | "yes"),
                    _ => {}
                }
            }
        }
        if let (true, Some(name)) = (connected, name.filter(|n| !n.is_empty())) {
            return Some(name);
        }
    }
    None
}

/// The connected Bluetooth audio device's name (the stock stack's list).
pub fn headphones() -> Option<String> {
    let output = Process::new("lipc-hash-prop")
        .args(["-n", "com.lab126.btfd", "ListConnected"])
        .output()
        .ok()?;
    connected_name_in(&String::from_utf8_lossy(&output.stdout))
}

/// The Kindle's sound level: the audio manager's `speakerVolume` (integer). It takes eight levels, 10 to 80 in steps of
/// 10; writes above 80 read back as 80, and writes below 10 as 10. Level 1 is the quietest.
pub fn level_from_volume(text: &str) -> Option<u8> {
    let value: u32 = text.trim().parse().ok()?;
    Some(((value / 10) as u8).clamp(1, SOUND_LEVELS))
}

pub fn volume_for(level: u8) -> u32 {
    u32::from(level.clamp(1, SOUND_LEVELS)) * 10
}

/// The Kindle's own sound level, read and set through the audio manager.
pub struct DeviceSound;

impl Sound for DeviceSound {
    fn level(&self) -> Option<u8> {
        let output = Process::new("lipc-get-prop")
            .args(["com.lab126.audiomgrd", "speakerVolume"])
            .output()
            .ok()?;
        level_from_volume(&String::from_utf8_lossy(&output.stdout))
    }

    fn set_level(&mut self, level: u8) -> Result<(), String> {
        let status = Process::new("lipc-set-prop")
            .args(["-i", "com.lab126.audiomgrd", "speakerVolume", &volume_for(level).to_string()])
            .status()
            .map_err(|e| format!("could not change the sound: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("the sound level did not change".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::connected_name_in;

    const ONE_CONNECTED: &str = "Hash Index: 0\n\tbd_name = \"AirPods Pro\"\n\tbd_address = \"AA:BB:CC:DD:EE:01\"\n\tis_paired = (1)\n\tis_connected = (1)\n";

    #[test]
    fn a_connected_device_is_named_and_one_that_left_is_not() {
        assert_eq!(connected_name_in(ONE_CONNECTED).as_deref(), Some("AirPods Pro"));
        assert_eq!(connected_name_in(&ONE_CONNECTED.replace("is_connected = (1)", "is_connected = (0)")), None);
        assert_eq!(connected_name_in(""), None);
    }

    #[test]
    fn the_connected_one_is_found_among_several_devices() {
        let report = format!("{ONE_CONNECTED}Hash Index: 1\n\tbd_name = \"Speaker\"\n\tis_connected = (0)\n");
        assert_eq!(connected_name_in(&report).as_deref(), Some("AirPods Pro"));
    }

    #[test]
    fn the_sound_levels_map_to_the_audio_manager_values() {
        use super::{level_from_volume, volume_for};
        assert_eq!(volume_for(1), 10);
        assert_eq!(volume_for(8), 80);
        assert_eq!(volume_for(0), 10, "the lowest level is 1");
        assert_eq!(level_from_volume("40\n"), Some(4));
        assert_eq!(level_from_volume("80"), Some(8));
        assert_eq!(level_from_volume("3"), Some(1));
        assert_eq!(level_from_volume(""), None);
    }
}

pub struct FilePlayer {
    tracks: Vec<Track>,
    /// Where playback stopped or paused (in the music folder, so it survives a restart).
    place: PathBuf,
}

impl FilePlayer {
    pub fn new(tracks: Vec<Track>, place: PathBuf) -> Self {
        Self { tracks, place }
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
        let saved = Saved::from_text(&fs::read_to_string(&self.place).unwrap_or_default());
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
            Process::new(program())
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
