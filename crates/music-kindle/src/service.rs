//! The player: a separate process that plays the library and keeps playing when the app closes. It reads commands from
//! the control file, writes its status every second, and exits when playback stops (or when no command arrives at
//! start).

use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use music_audio::{Event, Playback};
use music_core::{Command, PlayState, Queue, Saved, Snapshot, Track};

use std::path::PathBuf;

use crate::remote::{self, CONTROL, MEDIA_CMD, MEDIA_NOW, PLAYER_PID, STATUS};

/// Decoded segments are written here (RAM, so the flash storage is not worn).
pub const SEGMENTS: &str = "/var/tmp/music-segments";
/// The stock player, which plays a WAV file through the audio manager to the connected Bluetooth headphones.
/// `MUSIC_PLAYER` overrides it (for tests on a computer, with `afplay`).
pub fn player_program() -> String {
    std::env::var("MUSIC_PLAYER").unwrap_or_else(|_| "/usr/bin/mplay".into())
}
/// How long the player waits for its first command before it gives up.
const FIRST_COMMAND_WITHIN: Duration = Duration::from_secs(5);
/// How often the headphones are checked while music plays.
const CHECK_EVERY: Duration = Duration::from_secs(5);

pub fn run(tracks: Vec<Track>, place: PathBuf) -> Result<(), String> {
    fs::write(PLAYER_PID, std::process::id().to_string())
        .map_err(|e| format!("could not start the player: {e}"))?;
    let _ = fs::create_dir_all(Path::new(MEDIA_NOW).parent().unwrap_or(Path::new("/var/tmp/media")));
    let mut service = Service::new(tracks, place);
    service.serve();
    service.finish();
    Ok(())
}

/// Takes the commands written to `file` since the last read. The file is moved first, so the writer can append meanwhile.
fn read_commands(file: &str) -> Vec<Command> {
    if !Path::new(file).exists() {
        return Vec::new();
    }
    let work = format!("{file}.work");
    if fs::rename(file, &work).is_err() {
        return Vec::new();
    }
    let text = fs::read_to_string(&work).unwrap_or_default();
    let _ = fs::remove_file(&work);
    text.lines().filter_map(Command::from_line).collect()
}

struct Service {
    place_file: PathBuf,
    queue: Queue,
    playback: Playback,
    /// Whether a track has been chosen; the status then names it.
    chosen: bool,
    /// Whether any command has arrived.
    started: bool,
    state: PlayState,
    /// The place while stopped or paused, in seconds.
    place: u64,
    /// The current track's length in seconds (0 when unknown).
    length: u64,
    message: Option<String>,
    started_at: Instant,
    device_checked: Instant,
    status_written: Instant,
}

impl Service {
    fn new(tracks: Vec<Track>, place_file: PathBuf) -> Self {
        let now = Instant::now();
        Self {
            place_file,
            queue: Queue::new(tracks),
            playback: Playback::new(player_program(), SEGMENTS),
            chosen: false,
            started: false,
            state: PlayState::Stopped,
            place: 0,
            length: 0,
            message: None,
            started_at: now,
            device_checked: now,
            status_written: now - Duration::from_secs(1),
        }
    }

    fn position(&self) -> u64 {
        if self.state == PlayState::Playing {
            self.playback.position()
        } else {
            self.place
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            state: self.state,
            index: self.chosen.then(|| self.queue.current_index()),
            position: self.position(),
            message: self.message.clone(),
        }
    }

    /// Apply one command. Returns false when the player must stop.
    fn apply(&mut self, command: Command) -> bool {
        self.started = true;
        match command {
            Command::Play { index, from } => self.start(index, from),
            Command::Pause => self.pause(),
            Command::Toggle => {
                if self.state == PlayState::Playing {
                    self.pause();
                } else if self.chosen {
                    let index = self.queue.current_index();
                    self.start(index, self.place);
                }
            }
            Command::Stop => {
                self.stop();
                return false;
            }
            Command::Next | Command::Previous => {
                let was_playing = self.state == PlayState::Playing;
                self.playback.stop();
                if command == Command::Next {
                    self.queue.next();
                } else {
                    self.queue.previous();
                }
                self.chosen = true;
                self.place = 0;
                if was_playing {
                    let index = self.queue.current_index();
                    self.start(index, 0);
                } else {
                    self.save_place();
                }
            }
            Command::Seek(seconds) => {
                if self.state == PlayState::Playing {
                    let index = self.queue.current_index();
                    self.start(index, seconds);
                } else {
                    self.place = seconds;
                    self.save_place();
                }
            }
        }
        true
    }

    /// Play track `index` from `from` seconds, if headphones are connected.
    fn start(&mut self, index: usize, from: u64) {
        let Some(path) = self.queue.tracks().get(index).map(|t| t.path.clone()) else {
            return;
        };
        self.length = music_audio::duration(&path).unwrap_or(0);
        if remote::headphones().is_none() {
            self.message = Some("Connect headphones in Settings › Bluetooth first.".into());
            self.pause();
            return;
        }
        self.queue.select(index);
        self.chosen = true;
        match self.playback.start_at(&path, from) {
            Ok(()) => {
                self.state = PlayState::Playing;
                self.place = from;
                self.message = None;
            }
            Err(error) => {
                self.state = PlayState::Stopped;
                self.place = from;
                self.message = Some(error);
                self.save_place();
            }
        }
    }

    fn pause(&mut self) {
        if self.state == PlayState::Playing {
            self.place = self.playback.stop();
            self.state = PlayState::Paused;
            self.save_place();
        }
    }

    fn stop(&mut self) {
        if self.state == PlayState::Playing {
            self.place = self.playback.stop();
        }
        self.state = PlayState::Stopped;
        self.save_place();
    }

    /// The next track on the list when the current one ends. The list repeats.
    fn poll(&mut self) {
        if self.state != PlayState::Playing {
            return;
        }
        match self.playback.poll() {
            Event::Playing => {}
            Event::Ended => {
                self.queue.finished();
                let index = self.queue.current_index();
                self.start(index, 0);
            }
            Event::Failed(error) => {
                self.state = PlayState::Stopped;
                self.place = 0;
                self.message = Some(error);
                self.save_place();
            }
        }
    }

    fn check_headphones(&mut self) {
        if self.state == PlayState::Playing && self.device_checked.elapsed() >= CHECK_EVERY {
            self.device_checked = Instant::now();
            if remote::headphones().is_none() {
                self.message = Some("The headphones disconnected, so playback paused.".into());
                self.pause();
            }
        }
    }

    fn serve(&mut self) {
        loop {
            let mut keep_going = true;
            let commands = read_commands(CONTROL).into_iter().chain(read_commands(MEDIA_CMD));
            for command in commands {
                keep_going = self.apply(command);
                if !keep_going {
                    break;
                }
            }
            if !keep_going {
                break;
            }
            self.poll();
            self.check_headphones();
            // Stopped after a command ends the player. Before any command, the player waits briefly for one.
            if self.started && self.state == PlayState::Stopped {
                break;
            }
            if !self.started && self.started_at.elapsed() >= FIRST_COMMAND_WITHIN {
                break;
            }
            if self.status_written.elapsed() >= Duration::from_secs(1) {
                self.write_status();
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    fn write_status(&mut self) {
        self.status_written = Instant::now();
        let temporary = format!("{STATUS}.tmp");
        if fs::write(&temporary, self.snapshot().to_text()).is_ok() {
            let _ = fs::rename(&temporary, STATUS);
        }
        self.write_media();
    }

    /// The media session while the music plays or is paused; removed when it stops.
    fn write_media(&self) {
        if self.state == PlayState::Stopped {
            let _ = fs::remove_file(MEDIA_NOW);
            return;
        }
        let title = self.queue.current().map(|t| t.title.clone()).unwrap_or_default();
        let text = music_core::media_text(
            self.state == PlayState::Playing,
            &title,
            self.position(),
            self.length,
            std::process::id(),
        );
        let temporary = format!("{MEDIA_NOW}.tmp");
        if fs::write(&temporary, text).is_ok() {
            let _ = fs::rename(&temporary, MEDIA_NOW);
        }
    }

    fn save_place(&self) {
        if !self.chosen {
            return;
        }
        let saved = Saved {
            path: self.queue.current().map(|track| track.path.clone()),
            position: self.place,
        };
        let _ = fs::write(&self.place_file, saved.to_text());
    }

    fn finish(&mut self) {
        let _ = fs::remove_file(MEDIA_NOW);
        let _ = fs::remove_file(PLAYER_PID);
        self.write_status();
    }
}
