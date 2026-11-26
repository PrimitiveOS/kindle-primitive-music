//! The player, end to end, on a computer: the real `music-kindle --player` process reads the control file, plays
//! through `afplay` (silence, so nothing is heard), and reports through the status file. The headphones are a fake
//! `lipc-hash-prop` on the PATH. Runs on macOS only (it needs `afplay`).

#![cfg(target_os = "macos")]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use music_audio::Pcm;
use music_core::{PlayState, Saved, Snapshot};

const CONTROL: &str = "/var/tmp/music-control";
const STATUS: &str = "/var/tmp/music-status";
const PID: &str = "/var/tmp/music-player.pid";
const MEDIA_NOW: &str = "/var/tmp/media/music.now";
const MEDIA_CMD: &str = "/var/tmp/media/music.cmd";

/// Kills the player when the test ends, also when it fails: a player left behind keeps writing the shared files.
struct Guard(Child);

impl Guard {
    fn id(&self) -> u32 {
        self.0.id()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.0.try_wait()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn send(line: &str) {
    let mut file = OpenOptions::new().create(true).append(true).open(CONTROL).unwrap();
    writeln!(file, "{line}").unwrap();
}

fn status() -> Snapshot {
    Snapshot::from_text(&fs::read_to_string(STATUS).unwrap_or_default())
}

/// Wait until the status satisfies `done`, or fail after `within`.
fn until(within: Duration, what: &str, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let start = Instant::now();
    loop {
        let now = status();
        if done(&now) {
            return now;
        }
        assert!(start.elapsed() < within, "timed out waiting for: {what}; status was {now:?}");
        thread::sleep(Duration::from_millis(200));
    }
}

fn set_headphones(shim: &Path, connected: bool) {
    let body = if connected {
        "printf 'Hash Index: 0\\n\\tbd_name = \"AirPods Pro\"\\n\\tis_connected = (1)\\n'\n"
    } else {
        "exit 0\n"
    };
    fs::write(shim, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(shim, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

#[test]
fn play_pause_seek_disconnect_and_stop_through_the_control_file() {
    for path in [CONTROL, STATUS, PID, MEDIA_CMD, MEDIA_NOW] {
        let _ = fs::remove_file(path);
    }
    let root = std::env::temp_dir().join(format!("music-player-test-{}", std::process::id()));
    let music = root.join("music");
    let bin = root.join("bin");
    fs::create_dir_all(&music).unwrap();
    fs::create_dir_all(&bin).unwrap();
    // Two tracks of 40 seconds of silence (22 kHz mono), so a segment boundary is crossed.
    let silence = Pcm { rate: 22_050, channels: 1, samples: vec![0; 22_050 * 40] };
    fs::write(music.join("Alpha.wav"), silence.wav_bytes()).unwrap();
    fs::write(music.join("Beta.wav"), silence.wav_bytes()).unwrap();
    let shim = bin.join("lipc-hash-prop");
    set_headphones(&shim, true);
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());

    let mut player = Guard(Command::new(env!("CARGO_BIN_EXE_music-kindle"))
        .arg("--player")
        .env("MUSIC_FOLDER", &music)
        .env("MUSIC_PLAYER", "afplay")
        .env("PATH", &path)
        .spawn()
        .expect("the player starts"));
    thread::sleep(Duration::from_millis(300));

    // Play the first track from the start.
    send("play 0 0");
    until(Duration::from_secs(15), "playing", |s| s.state == PlayState::Playing && s.index == Some(0));
    thread::sleep(Duration::from_secs(3));
    let playing = until(Duration::from_secs(5), "a position", |s| s.position >= 2);
    assert_eq!(playing.state, PlayState::Playing);

    // The media session (for quick settings) says what plays, and the player's process id.
    let now = fs::read_to_string(MEDIA_NOW).expect("the media session is written while playing");
    assert!(now.contains("state playing") && now.contains("title Alpha"), "{now}");
    assert!(now.contains(&format!("pid {}", player.id())), "the session names the player: {now}");

    // A toggle from the media card (its command file) pauses; the session says paused.
    let mut card = OpenOptions::new().create(true).append(true).open(MEDIA_CMD).unwrap();
    writeln!(card, "toggle").unwrap();
    drop(card);
    let paused = until(Duration::from_secs(5), "paused by the card", |s| s.state == PlayState::Paused);
    assert!(paused.position >= 2, "the place is kept: {paused:?}");
    let now = fs::read_to_string(MEDIA_NOW).expect("the session stays while paused");
    assert!(now.contains("state paused"), "{now}");

    // Pause keeps the place; resume from it.
    send(&format!("play 0 {}", paused.position));
    until(Duration::from_secs(10), "playing again", |s| s.state == PlayState::Playing);
    // Toggle again from the card: pause, and then the card's play resumes from the same place.
    let mut card = OpenOptions::new().create(true).append(true).open(MEDIA_CMD).unwrap();
    writeln!(card, "toggle").unwrap();
    drop(card);
    until(Duration::from_secs(5), "paused again", |s| s.state == PlayState::Paused);
    let mut card = OpenOptions::new().create(true).append(true).open(MEDIA_CMD).unwrap();
    writeln!(card, "toggle").unwrap();
    drop(card);
    until(Duration::from_secs(10), "playing from the card", |s| s.state == PlayState::Playing);

    // Next and previous move through the list.
    send("next");
    until(Duration::from_secs(10), "the second track", |s| s.index == Some(1) && s.state == PlayState::Playing);
    send("previous");
    until(Duration::from_secs(10), "back to the first", |s| s.index == Some(0) && s.state == PlayState::Playing);

    // Seek moves the place: it restarts the track there.
    send("seek 25");
    until(Duration::from_secs(10), "seek to 25 s", |s| s.position >= 25 && s.state == PlayState::Playing);

    // Headphones leave: playback pauses with a message, and the player keeps running.
    set_headphones(&shim, false);
    let lost = until(Duration::from_secs(15), "paused by the disconnect", |s| {
        s.state == PlayState::Paused && s.message.is_some()
    });
    assert!(lost.message.unwrap().contains("disconnected"));
    assert!(player.try_wait().unwrap().is_none(), "the player waits, it does not exit");
    set_headphones(&shim, true);

    // Stop ends the player, saves the place, and clears the process id.
    send("stop");
    let deadline = Instant::now() + Duration::from_secs(15);
    let exit = loop {
        if let Some(status) = player.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the player did not stop");
        thread::sleep(Duration::from_millis(200));
    };
    assert!(exit.success(), "the player exits cleanly: {exit:?}");
    let stopped = status();
    assert_eq!(stopped.state, PlayState::Stopped);
    assert_eq!(stopped.index, Some(0));
    assert!(!Path::new(PID).exists(), "the process id is removed");
    let saved = Saved::from_text(&fs::read_to_string(music.join("state.txt")).unwrap());
    assert_eq!(saved.path.as_deref(), Some(music.join("Alpha.wav").as_path()));
    assert!(saved.position >= 25, "the place is saved: {saved:?}");
    assert!(!Path::new(MEDIA_NOW).exists(), "the session is removed when the player stops");

    for path in [CONTROL, STATUS, PID, MEDIA_CMD] {
        let _ = fs::remove_file(path);
    }
    let _ = fs::remove_dir_all(&root);
}
