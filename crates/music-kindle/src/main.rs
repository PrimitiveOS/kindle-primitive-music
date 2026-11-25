//! The Kindle entry point. With no argument it runs the app. With `--player` it runs the player, which keeps playing
//! when the app closes (see `remote` and `service`).

mod remote;
mod service;

use std::{fs, path::{Path, PathBuf}};

use music_core::{library, sort_tracks, Track};
use music_render::Music;
use remote::FilePlayer;

/// The music folder: loose files here, and in `library/` beside it. `MUSIC_FOLDER` overrides it (for tests on a computer).
const MUSIC: &str = "/mnt/us/music";

fn main() -> Result<(), String> {
    let player = std::env::args().nth(1).as_deref() == Some("--player");
    let dir = std::env::var_os("MUSIC_FOLDER").map_or_else(|| PathBuf::from(MUSIC), PathBuf::from);
    let tracks = scan(&dir);
    let place = dir.join("state.txt");
    if player {
        return service::run(tracks, place);
    }
    let app = Music::new(tracks.clone(), Box::new(FilePlayer::new(tracks, place)), Box::new(remote::headphones));
    primitive_app::run(app, &dir)
}

/// The tracks in the music folder and in `library/`, sorted by title.
fn scan(dir: &Path) -> Vec<Track> {
    let mut tracks: Vec<Track> = Vec::new();
    for folder in [dir.to_path_buf(), dir.join("library")] {
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
    tracks
}
