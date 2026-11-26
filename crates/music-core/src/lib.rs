//! The music app's logic, with no device or audio code: which files are music, the track list, the play queue,
//! time text, and the saved place. Tested on the Mac.

use std::path::{Path, PathBuf};

/// File extensions the decoder can read (see `music-audio`).
pub const EXTENSIONS: &[&str] = &["mp3", "flac", "ogg", "oga", "wav", "m4a", "aac"];

/// Whether a file name is a music file (by its extension, in any case).
pub fn is_music(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// The name shown for a track: its file name without the extension, with underscores as spaces.
pub fn title_from_file_name(name: &str) -> String {
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    stem.replace('_', " ").trim().to_string()
}

/// One music file on the Kindle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
}

/// The library: the music files in a folder, sorted by title (case-insensitive). `names` are the file names found in
/// the folder; their full paths are `dir.join(name)`.
pub fn library(dir: &Path, names: &[String]) -> Vec<Track> {
    let mut tracks: Vec<Track> = names
        .iter()
        .filter(|n| is_music(n))
        .map(|n| Track { path: dir.join(n), title: title_from_file_name(n) })
        .collect();
    sort_tracks(&mut tracks);
    tracks
}

/// Sort tracks by title (case-insensitive), so tracks from several folders list together.
pub fn sort_tracks(tracks: &mut [Track]) {
    tracks.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()).then_with(|| a.title.cmp(&b.title)));
}

/// "3:07", or "1:02:09" from an hour.
pub fn format_time(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Whether the queue is playing, paused, or stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// A command from the app to the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Play track `index` of the library, from `from` seconds.
    Play { index: usize, from: u64 },
    Pause,
    /// End playback and save the place. The player exits.
    Stop,
    Next,
    Previous,
    /// Play when paused, pause when playing (the launcher's media card sends it).
    Toggle,
    /// Move to `seconds` in the current track (while paused, it sets the place to resume from).
    Seek(u64),
}

impl Command {
    /// One line of the control file.
    pub fn to_line(&self) -> String {
        match self {
            Command::Play { index, from } => format!("play {index} {from}"),
            Command::Pause => "pause".into(),
            Command::Stop => "stop".into(),
            Command::Next => "next".into(),
            Command::Previous => "previous".into(),
            Command::Toggle => "toggle".into(),
            Command::Seek(seconds) => format!("seek {seconds}"),
        }
    }

    pub fn from_line(line: &str) -> Option<Self> {
        let mut parts = line.split_whitespace();
        match parts.next()? {
            "play" => Some(Command::Play { index: parts.next()?.parse().ok()?, from: parts.next()?.parse().ok()? }),
            "pause" => Some(Command::Pause),
            "stop" => Some(Command::Stop),
            "next" => Some(Command::Next),
            "previous" => Some(Command::Previous),
            "toggle" => Some(Command::Toggle),
            "seek" => Some(Command::Seek(parts.next()?.parse().ok()?)),
            _ => None,
        }
    }
}

/// The media session the app shares with quick settings (docs/contracts.md §1.2): what the launcher shows while the
/// music plays or is paused. The track title has no line breaks.
pub fn media_text(playing: bool, title: &str, position: u64, length: u64, pid: u32) -> String {
    let state = if playing { "playing" } else { "paused" };
    let title = title.replace('\n', " ");
    format!("app music\nname Music\nstate {state}\ntitle {title}\nposition {position}\nlength {length}\npid {pid}\n")
}

/// What the player reports: its state, the track (its library index), the place in seconds, and a message for the
/// user. Saved as lines of `key value`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub state: PlayState,
    pub index: Option<usize>,
    pub position: u64,
    pub message: Option<String>,
}

impl Snapshot {
    pub fn to_text(&self) -> String {
        let state = match self.state {
            PlayState::Playing => "playing",
            PlayState::Paused => "paused",
            PlayState::Stopped => "stopped",
        };
        let mut out = format!("state {state}\nposition {}\n", self.position);
        if let Some(index) = self.index {
            out.push_str(&format!("index {index}\n"));
        }
        if let Some(message) = &self.message {
            out.push_str(&format!("message {}\n", message.replace('\n', " ")));
        }
        out
    }

    pub fn from_text(text: &str) -> Self {
        let mut snapshot = Snapshot::default();
        for line in text.lines() {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            match key {
                "state" => {
                    snapshot.state = match value {
                        "playing" => PlayState::Playing,
                        "paused" => PlayState::Paused,
                        _ => PlayState::Stopped,
                    }
                }
                "position" => snapshot.position = value.parse().unwrap_or(0),
                "index" => snapshot.index = value.parse().ok(),
                "message" => snapshot.message = (!value.is_empty()).then(|| value.to_string()),
                _ => {}
            }
        }
        snapshot
    }
}

/// The play queue: the tracks in order, the current one, and the state. Playing the last track wraps to the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queue {
    tracks: Vec<Track>,
    current: usize,
    state: PlayState,
}

impl Queue {
    pub fn new(tracks: Vec<Track>) -> Self {
        Self { tracks, current: 0, state: PlayState::Stopped }
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn current_index(&self) -> usize {
        self.current
    }

    pub fn current(&self) -> Option<&Track> {
        self.tracks.get(self.current)
    }

    pub fn state(&self) -> PlayState {
        self.state
    }

    /// Choose track `index` without starting it (wraps past the end; an empty queue does nothing).
    pub fn select(&mut self, index: usize) {
        if !self.tracks.is_empty() {
            self.current = index % self.tracks.len();
            self.state = PlayState::Stopped;
        }
    }

    /// Start track `index` (wraps past the end; an empty queue does nothing).
    pub fn play(&mut self, index: usize) {
        if self.tracks.is_empty() {
            return;
        }
        self.current = index % self.tracks.len();
        self.state = PlayState::Playing;
    }

    /// Play or pause the current track; from stopped, starts the first track.
    pub fn toggle(&mut self) {
        match self.state {
            PlayState::Playing => self.state = PlayState::Paused,
            PlayState::Paused => self.state = PlayState::Playing,
            PlayState::Stopped => self.play(self.current),
        }
    }

    /// Pause the current track (a stopped or paused queue stays as it is).
    pub fn pause(&mut self) {
        if self.state == PlayState::Playing {
            self.state = PlayState::Paused;
        }
    }

    pub fn stop(&mut self) {
        self.state = PlayState::Stopped;
    }

    /// The next track, wrapping to the first after the last.
    pub fn next(&mut self) {
        if !self.tracks.is_empty() {
            self.play(self.current + 1);
        }
    }

    /// The previous track, wrapping to the last before the first.
    pub fn previous(&mut self) {
        if !self.tracks.is_empty() {
            self.play(self.current + self.tracks.len() - 1);
        }
    }

    /// Called when a track finishes on its own: the next one plays.
    pub fn finished(&mut self) {
        self.next();
    }
}

/// The place to resume: the track's path and the position in seconds. Saved as two lines.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Saved {
    pub path: Option<PathBuf>,
    pub position: u64,
}

impl Saved {
    pub fn to_text(&self) -> String {
        match &self.path {
            Some(path) => format!("{}\n{}\n", path.display(), self.position),
            None => String::new(),
        }
    }

    pub fn from_text(text: &str) -> Self {
        let mut lines = text.lines();
        let path = lines.next().filter(|l| !l.is_empty()).map(PathBuf::from);
        let position = lines.next().and_then(|l| l.trim().parse().ok()).unwrap_or(0);
        Self { path, position }
    }

    /// The saved track's index in the queue, if it is still there.
    pub fn index_in(&self, tracks: &[Track]) -> Option<usize> {
        let path = self.path.as_ref()?;
        tracks.iter().position(|t| &t.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        ["zebra.mp3", "Apple_Song.FLAC", "notes.txt", "middle.ogg", "cover.png", "bird.wav"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_music_files_are_listed_sorted_by_title() {
        let lib = library(Path::new("/mnt/us/music"), &names());
        let titles: Vec<&str> = lib.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Apple Song", "bird", "middle", "zebra"]);
        assert_eq!(lib[0].path, Path::new("/mnt/us/music/Apple_Song.FLAC"));
    }

    #[test]
    fn extensions_are_matched_in_any_case() {
        assert!(is_music("A.Mp3"));
        assert!(!is_music("readme"));
        assert!(!is_music("photo.jpg"));
    }

    #[test]
    fn times_read_as_minutes_and_hours() {
        assert_eq!(format_time(0), "0:00");
        assert_eq!(format_time(187), "3:07");
        assert_eq!(format_time(3729), "1:02:09");
    }

    #[test]
    fn the_queue_moves_and_wraps() {
        let mut q = Queue::new(library(Path::new("/m"), &names()));
        assert_eq!(q.state(), PlayState::Stopped);
        q.toggle();
        assert_eq!((q.current_index(), q.state()), (0, PlayState::Playing));
        q.toggle();
        assert_eq!(q.state(), PlayState::Paused);
        q.toggle();
        assert_eq!(q.state(), PlayState::Playing);
        q.previous();
        assert_eq!(q.current_index(), 3, "before the first wraps to the last");
        q.next();
        assert_eq!(q.current_index(), 0, "after the last wraps to the first");
        q.finished();
        assert_eq!(q.current_index(), 1, "a finished track plays the next");
        q.stop();
        assert_eq!(q.state(), PlayState::Stopped);
    }

    #[test]
    fn an_empty_queue_does_nothing() {
        let mut q = Queue::new(Vec::new());
        q.toggle();
        q.next();
        q.previous();
        assert_eq!(q.state(), PlayState::Stopped);
        assert!(q.current().is_none());
    }

    #[test]
    fn the_saved_place_round_trips_and_finds_its_track() {
        let lib = library(Path::new("/mnt/us/music"), &names());
        let saved = Saved { path: Some(PathBuf::from("/mnt/us/music/middle.ogg")), position: 42 };
        let back = Saved::from_text(&saved.to_text());
        assert_eq!(back, saved);
        // Sorted by title: Apple Song, bird, middle, zebra.
        assert_eq!(back.index_in(&lib), Some(2));
        assert_eq!(Saved::from_text("").path, None);
        assert_eq!(Saved::from_text("/gone.mp3\n5").index_in(&lib), None, "a track that is gone is not found");
    }

    #[test]
    fn commands_round_trip_through_the_control_file() {
        for command in [
            Command::Play { index: 4, from: 73 },
            Command::Pause,
            Command::Stop,
            Command::Next,
            Command::Previous,
            Command::Toggle,
            Command::Seek(12),
        ] {
            assert_eq!(Command::from_line(&command.to_line()), Some(command.clone()));
        }
        assert_eq!(Command::from_line("jump 3"), None);
        assert_eq!(Command::from_line(""), None);
    }

    #[test]
    fn the_status_round_trips_and_tolerates_missing_parts() {
        let snapshot = Snapshot {
            state: PlayState::Paused,
            index: Some(7),
            position: 95,
            message: Some("The headphones disconnected.".into()),
        };
        assert_eq!(Snapshot::from_text(&snapshot.to_text()), snapshot);
        let empty = Snapshot::from_text("");
        assert_eq!(empty, Snapshot::default());
        assert_eq!(empty.state, PlayState::Stopped);
    }

    #[test]
    fn the_media_session_is_written_in_the_contract_format() {
        let text = media_text(true, "Evening Bells", 83, 203, 20561);
        assert_eq!(text, "app music\nname Music\nstate playing\ntitle Evening Bells\nposition 83\nlength 203\npid 20561\n");
        assert!(media_text(false, "a\nb", 0, 0, 1).contains("state paused\ntitle a b\n"));
    }
}
