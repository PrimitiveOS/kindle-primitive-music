//! The player: a separate process that plays the library and keeps playing when the app closes. It reads commands from
//! the control file, writes its status every second, and exits when playback stops (or when no command arrives at
//! start).

use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use music_audio::{Event, Playback};
use music_core::{Command, PlayState, Queue, Saved, Snapshot, Track};

use crate::remote::{self, CONTROL, PLACE, PLAYER_PID, STATUS};

/// Decoded segments are written here (RAM, so the flash storage is not worn).
pub const SEGMENTS: &str = "/var/tmp/music-segments";
/// The stock player. It plays a WAV file through the audio manager, to the connected Bluetooth headphones.
pub const PLAYER: &str = "/usr/bin/mplay";
/// How long the player waits for its first command before it gives up.
const FIRST_COMMAND_WITHIN: Duration = Duration::from_secs(5);
/// How often the headphones are checked while music plays.
const CHECK_EVERY: Duration = Duration::from_secs(5);

pub fn run(tracks: Vec<Track>) -> Result<(), String> {
    fs::write(PLAYER_PID, std::process::id().to_string())
        .map_err(|e| format!("could not start the player: {e}"))?;
    let mut service = Service::new(tracks);
    service.serve();
    service.finish();
    Ok(())
}

/// Takes the commands the app wrote since the last read. The file is moved first, so the app can append meanwhile.
fn read_commands() -> Vec<Command> {
    if !Path::new(CONTROL).exists() {
        return Vec::new();
    }
    let work = format!("{CONTROL}.work");
    if fs::rename(CONTROL, &work).is_err() {
        return Vec::new();
    }
    let text = fs::read_to_string(&work).unwrap_or_default();
    let _ = fs::remove_file(&work);
    text.lines().filter_map(Command::from_line).collect()
}

struct Service {
    queue: Queue,
    playback: Playback,
    /// Whether a track has been chosen; the status then names it.
    chosen: bool,
    /// Whether any command has arrived.
    started: bool,
    state: PlayState,
    /// The place while stopped or paused, in seconds.
    place: u64,
    message: Option<String>,
    started_at: Instant,
    device_checked: Instant,
    status_written: Instant,
}

impl Service {
    fn new(tracks: Vec<Track>) -> Self {
        let now = Instant::now();
        Self {
            queue: Queue::new(tracks),
            playback: Playback::new(PLAYER, SEGMENTS),
            chosen: false,
            started: false,
            state: PlayState::Stopped,
            place: 0,
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
            for command in read_commands() {
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
    }

    fn save_place(&self) {
        if !self.chosen {
            return;
        }
        let saved = Saved {
            path: self.queue.current().map(|track| track.path.clone()),
            position: self.place,
        };
        let _ = fs::write(PLACE, saved.to_text());
    }

    fn finish(&mut self) {
        let _ = fs::remove_file(PLAYER_PID);
        self.write_status();
    }
}
