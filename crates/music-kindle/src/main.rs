//! The Kindle entry point: scan the music folders, read the saved place, and run the app on the device.

use std::{fs, path::PathBuf, process::Command};

use music_audio::Playback;
use music_core::{library, sort_tracks, Saved, Track};
use music_render::Music;

/// The music folder: loose files here, and in `library/` beside it.
const MUSIC: &str = "/mnt/us/music";
/// Decoded segments are written here (RAM, so the flash storage is not worn).
const SEGMENTS: &str = "/var/tmp/music-segments";
/// The stock player. It plays a WAV file through the audio manager, to the connected Bluetooth headphones.
const PLAYER: &str = "/usr/bin/mplay";

fn main() -> Result<(), String> {
    let dir = PathBuf::from(MUSIC);
    let mut tracks: Vec<Track> = Vec::new();
    for folder in [dir.clone(), dir.join("library")] {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        let names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        tracks.extend(library(&folder, &names));
    }
    sort_tracks(&mut tracks);

    let state = dir.join("state.txt");
    let saved = Saved::from_text(&fs::read_to_string(&state).unwrap_or_default());
    let app = Music::new(
        tracks,
        saved,
        Playback::new(PLAYER, SEGMENTS),
        Box::new(headphones),
        Some(state),
    );
    primitive_app::run(app, &dir)
}

/// The name of the connected Bluetooth audio device, from the stock Bluetooth stack.
fn headphones() -> Option<String> {
    let output = Command::new("lipc-get-prop")
        .args(["com.lab126.btfd", "BTconnectedDevName"])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !name.is_empty()).then_some(name)
}
