//! Computer tools for the Music app. Without arguments, it writes screen previews as PNG files (`build/previews`).
//! `listen <file> [--pause-after SECS]` plays a file here with `afplay`, through the same segments and pause logic the
//! Kindle uses, so the segment joins and the pause can be heard on a computer first.

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use music_audio::{Event, Playback};
use music_core::{Command, PlayState, Snapshot, Track};
use music_render::{Music, Remote};
use primitive_app::{font_paths, frame, write_png, App, HEIGHT, WIDTH};
use primitive_ui::{Canvas, Input};

/// The player on a Mac.
const PLAYER: &str = "afplay";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("listen") => listen(&args[1..]),
        _ => previews(args.first().map_or_else(|| PathBuf::from("build/previews"), PathBuf::from)),
    }
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("music-desktop-{name}-{}", std::process::id()))
}

fn sample_tracks() -> Vec<Track> {
    [
        "Clouds Over the Lake",
        "Dance in Autumn",
        "Evening Bells",
        "First Light",
        "Hallway Waltz",
        "Ice on the River",
        "Jasmine",
        "Kettle Song",
        "Long Way Home",
        "Morning Rain",
    ]
    .iter()
    .map(|title| Track {
        path: PathBuf::from(format!("/mnt/us/music/{title}.mp3")),
        title: (*title).to_string(),
    })
    .collect()
}

/// A player that shows a fixed status (for previews).
struct Still(Snapshot);

impl Remote for Still {
    fn snapshot(&self) -> Snapshot {
        self.0.clone()
    }

    fn send(&mut self, _command: Command) -> Result<(), String> {
        Ok(())
    }
}

fn previews(output: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(&output)?;
    let fonts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../doannt-kindle-launcher/assets/fonts");
    let mut canvas = Canvas::new(WIDTH, HEIGHT, &font_paths(&fonts))?;
    let headphones = || Box::new(|| Some("AirPods Pro".to_string())) as Box<dyn Fn() -> Option<String>>;
    let make = |status: Snapshot| Music::new(sample_tracks(), Box::new(Still(status)), headphones());
    let playing = |index: usize, position: u64| Snapshot {
        state: PlayState::Playing,
        index: Some(index),
        position,
        message: None,
    };
    let paused = Snapshot { state: PlayState::Paused, ..playing(2, 83) };

    let library = make(Snapshot::default());
    write_png(&output.join("library.png"), WIDTH, HEIGHT, &frame(&library, &mut canvas, false))?;
    write_png(&output.join("library-dark.png"), WIDTH, HEIGHT, &frame(&library, &mut canvas, true))?;

    let mut second_page = make(Snapshot::default());
    second_page.input(Input::PageForward);
    write_png(&output.join("library-page-2.png"), WIDTH, HEIGHT, &frame(&second_page, &mut canvas, false))?;

    let mut with_mini = make(playing(2, 83));
    write_png(&output.join("library-mini-player.png"), WIDTH, HEIGHT, &frame(&with_mini, &mut canvas, false))?;
    with_mini.input(Input::PageBack);

    let mut now_playing = make(playing(2, 83));
    now_playing.show_now();
    write_png(&output.join("now-playing.png"), WIDTH, HEIGHT, &frame(&now_playing, &mut canvas, false))?;

    let mut now_paused = make(paused.clone());
    now_paused.show_now();
    write_png(&output.join("now-paused.png"), WIDTH, HEIGHT, &frame(&now_paused, &mut canvas, false))?;
    write_png(&output.join("now-paused-dark.png"), WIDTH, HEIGHT, &frame(&now_paused, &mut canvas, true))?;

    let mut now_stopped = make(Snapshot { state: PlayState::Stopped, index: Some(4), position: 42, message: None });
    now_stopped.show_now();
    write_png(&output.join("now-stopped.png"), WIDTH, HEIGHT, &frame(&now_stopped, &mut canvas, false))?;

    let mut without = Music::new(sample_tracks(), Box::new(Still(Snapshot::default())), Box::new(|| None));
    without.show_now();
    write_png(&output.join("now-no-headphones.png"), WIDTH, HEIGHT, &frame(&without, &mut canvas, false))?;
    println!("wrote previews to {}", output.display());
    Ok(())
}

/// Play `file` here, from the start. With `--pause-after SECS`, pause after that many seconds, wait three seconds,
/// and resume from the place it paused at.
fn listen(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let file = PathBuf::from(args.first().ok_or("usage: music-desktop listen <file> [--pause-after SECS]")?);
    let pause_after = args
        .iter()
        .position(|a| a == "--pause-after")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.parse::<u64>())
        .transpose()?;

    let mut playback = Playback::new(PLAYER, scratch("listen"));
    playback.start_at(&file, 0)?;
    let began = Instant::now();
    let mut paused = false;
    let mut last_shown = u64::MAX;
    loop {
        match playback.poll() {
            Event::Playing => {}
            Event::Ended => {
                println!("ended at {} s", playback.position());
                break;
            }
            Event::Failed(error) => return Err(error.into()),
        }
        if let Some(seconds) = pause_after {
            if !paused && began.elapsed() >= Duration::from_secs(seconds) {
                let at = playback.stop();
                println!("paused at {at} s");
                thread::sleep(Duration::from_secs(3));
                playback.start_at(&file, at)?;
                println!("resumed at {at} s");
                paused = true;
            }
        }
        let shown = playback.position() / 5;
        if shown != last_shown {
            last_shown = shown;
            println!("playing at {} s", playback.position());
        }
        thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}
