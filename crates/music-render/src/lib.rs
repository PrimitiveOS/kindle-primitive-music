//! The Music app's screens, touch handling and playback control. Decoding and the player are in `music-audio`; the
//! queue, the time text and the saved place are in `music-core`.
//!
//! The layout follows Apple Music: a large title over a list of tracks, a mini player above a floating tab bar, and a
//! full player with a large artwork square, a thin progress line and round transport buttons. The files carry no
//! artwork, so the square shows a note. Playing needs headphones: the Kindle has no speaker. Sleep pauses playback,
//! and leaving the app stops it.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use music_audio::{duration, Event, Playback};
use music_core::{format_time, PlayState, Queue, Saved, Track};
use primitive_app::{save, App, Change};
use primitive_ui::{
    back_button, full_screen_message, screen_rect, Canvas, Icon, Input, Rect, Tab, TabBar, TextStyle,
    BLACK, CAPTION, HAIRLINE, PAPER,
};

/// Tracks on one library page.
pub const PER_PAGE: usize = 7;
/// How often the screen shows a new position while playing, in seconds.
const SHOWN_EVERY: u64 = 5;
/// How often the headphones are checked.
const CHECK_EVERY: Duration = Duration::from_secs(5);
/// How often the player is polled while a track plays.
const POLL_EVERY: Duration = Duration::from_secs(1);
/// The grey of the artwork's background and of the note on it.
const ARTWORK: u8 = 235;
const NOTE_INK: u8 = 120;

/// The headphones' name (the Bluetooth device's name), or `None` when none is connected.
pub type Headphones = Box<dyn Fn() -> Option<String>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Library,
    Now,
}

const NOTE: Icon = Icon {
    stroke: &["M9 18 V5 L21 3 V16", "M5 18 A3 3 0 1 0 5 18.01", "M17 16 A3 3 0 1 0 17 16.01"],
    fill: &[],
};
const PLAY: Icon = Icon { stroke: &[], fill: &["M7 4 L20 12 L7 20 Z"] };
const PAUSE: Icon = Icon { stroke: &[], fill: &["M6 4 H10 V20 H6 Z M14 4 H18 V20 H14 Z"] };
const NEXT: Icon = Icon { stroke: &[], fill: &["M5 5 L14 12 L5 19 Z M16 5 H19 V19 H16 Z"] };
const PREVIOUS: Icon = Icon { stroke: &[], fill: &["M19 5 L10 12 L19 19 Z M8 5 H5 V19 H8 Z"] };

// Positions are in design units on the 1264-wide screen; `screen_rect` scales them to the actual width.
const LIST_TOP: i32 = 250;
const ROW: i32 = 140;

fn row_rect(width: i32, index: usize) -> Rect {
    screen_rect(width, 40, LIST_TOP + index as i32 * ROW, 1184, ROW)
}

fn pager_previous(width: i32) -> Rect {
    screen_rect(width, 40, 1240, 300, 80)
}

fn pager_next(width: i32) -> Rect {
    screen_rect(width, 924, 1240, 300, 80)
}

fn mini_rect(width: i32) -> Rect {
    screen_rect(width, 40, 1340, 1184, 110)
}

/// The round button at the right end of the mini player.
fn mini_button(width: i32) -> Rect {
    let mini = mini_rect(width);
    let size = screen_rect(width, 0, 0, 80, 80).w;
    let margin = screen_rect(width, 0, 0, 24, 0).w;
    Rect::new(mini.right() - size - margin, mini.y + (mini.h - size) / 2, size, size)
}

fn tabs_rect(width: i32) -> Rect {
    screen_rect(width, 40, 1500, 1184, 120)
}

fn tabs(selected: usize) -> TabBar<'static> {
    TabBar {
        bounds: Rect::new(0, 0, 0, 0),
        tabs: &[Tab { label: "Library", icon: Some(NOTE) }, Tab { label: "Playing", icon: Some(PLAY) }],
        selected,
    }
}

struct NowLayout {
    artwork: Rect,
    bar: Rect,
    bar_touch: Rect,
    previous: Rect,
    play: Rect,
    next: Rect,
}

impl NowLayout {
    fn new(width: i32) -> Self {
        Self {
            artwork: screen_rect(width, 252, 130, 760, 760),
            bar: screen_rect(width, 40, 1112, 1184, 12),
            bar_touch: screen_rect(width, 40, 1080, 1184, 70),
            previous: screen_rect(width, 300, 1236, 140, 140),
            play: screen_rect(width, 532, 1206, 200, 200),
            next: screen_rect(width, 824, 1236, 140, 140),
        }
    }
}

pub struct Music {
    queue: Queue,
    playback: Playback,
    headphones: Headphones,
    device: Option<String>,
    checked_at: Instant,
    wake_at: Instant,
    screen: Screen,
    page: usize,
    /// The track's length in seconds, when its header says.
    length: Option<u64>,
    /// Where playback resumes (while stopped or paused).
    resume_at: u64,
    /// The position last shown, in steps of `SHOWN_EVERY` seconds.
    shown: u64,
    message: Option<String>,
    state_file: Option<PathBuf>,
    saved: Saved,
    /// A track has been chosen (or restored from the saved place): the mini player and the Playing tab use it.
    chosen: bool,
}

impl Music {
    /// `tracks` is the library in list order. `saved` is the place from the last visit: its track is selected, paused.
    pub fn new(
        tracks: Vec<Track>,
        saved: Saved,
        playback: Playback,
        headphones: Headphones,
        state_file: Option<PathBuf>,
    ) -> Self {
        let queue = Queue::new(tracks);
        let index = saved.index_in(queue.tracks());
        let now = Instant::now();
        let mut app = Self {
            queue,
            playback,
            headphones,
            device: None,
            checked_at: now,
            wake_at: now,
            screen: Screen::Library,
            page: 0,
            length: None,
            resume_at: 0,
            shown: 0,
            message: None,
            state_file,
            saved,
            chosen: false,
        };
        app.device = (app.headphones)();
        if let Some(index) = index {
            app.queue.select(index);
            app.resume_at = app.saved.position;
            app.length = app.current_length();
            app.page = index / PER_PAGE;
            app.chosen = true;
        }
        app
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn position(&self) -> u64 {
        if self.playback.is_playing() {
            self.playback.position()
        } else {
            self.resume_at
        }
    }

    pub fn headphones(&self) -> Option<&str> {
        self.device.as_deref()
    }

    /// Show track `index` paused at `position` seconds, without playing (for previews and tests).
    pub fn show_paused_at(&mut self, index: usize, position: u64) {
        self.queue.select(index);
        self.queue.play(index);
        self.queue.pause();
        self.resume_at = position;
        self.length = self.current_length();
        self.chosen = true;
        self.screen = Screen::Now;
    }

    fn current_length(&self) -> Option<u64> {
        self.queue.current().and_then(|track| duration(&track.path))
    }

    fn page_count(&self) -> usize {
        self.queue.tracks().len().div_ceil(PER_PAGE).max(1)
    }

    /// The tracks on the current page, with their details.
    fn page_rows(&self) -> Vec<(String, String)> {
        let start = self.page * PER_PAGE;
        self.queue
            .tracks()
            .iter()
            .enumerate()
            .skip(start)
            .take(PER_PAGE)
            .map(|(index, track)| (track.title.clone(), self.row_detail(index, track)))
            .collect()
    }

    fn row_detail(&self, index: usize, track: &Track) -> String {
        let format = format_label(track);
        if self.is_current(index) {
            let state = if self.queue.state() == PlayState::Playing { "Playing" } else { "Paused" };
            format!("{format} · {state}")
        } else if self.saved.index_in(self.queue.tracks()) == Some(index) {
            format!("{format} · Last played")
        } else {
            format
        }
    }

    fn is_current(&self, index: usize) -> bool {
        self.queue.current_index() == index && self.queue.state() != PlayState::Stopped
    }

    fn connection_line(&self) -> String {
        match &self.device {
            Some(name) => format!("{name} connected"),
            None => "No headphones connected".into(),
        }
    }

    /// Open a track from the library and start it.
    fn open(&mut self, index: usize) -> Change {
        self.queue.select(index);
        self.chosen = true;
        self.resume_at = match self.saved.index_in(self.queue.tracks()) {
            Some(saved) if saved == index => self.saved.position,
            _ => 0,
        };
        self.length = self.current_length();
        self.screen = Screen::Now;
        self.start(self.resume_at);
        Change::Full
    }

    /// Start the current track at `from` seconds, if headphones are connected.
    fn start(&mut self, from: u64) {
        let Some(track) = self.queue.current().cloned() else {
            return;
        };
        if self.device.is_none() {
            self.queue.stop();
            self.message = Some("Connect headphones in Settings › Bluetooth first.".into());
            return;
        }
        match self.playback.start_at(&track.path, from) {
            Ok(()) => {
                if self.queue.state() != PlayState::Playing {
                    self.queue.play(self.queue.current_index());
                }
                self.resume_at = from;
                self.shown = from / SHOWN_EVERY;
                self.message = None;
                self.wake_at = Instant::now() + POLL_EVERY;
            }
            Err(error) => {
                self.queue.stop();
                self.message = Some(error);
            }
        }
    }

    fn pause(&mut self) {
        if self.playback.is_playing() {
            self.resume_at = self.playback.stop();
        }
        self.queue.pause();
    }

    fn toggle(&mut self) -> Change {
        if self.playback.is_playing() {
            self.pause();
        } else {
            self.start(self.resume_at);
        }
        Change::Partial
    }

    fn move_track(&mut self, forward: bool) -> Change {
        let was_playing = self.playback.is_playing();
        self.playback.stop();
        if forward {
            self.queue.next();
        } else {
            self.queue.previous();
        }
        self.resume_at = 0;
        self.length = self.current_length();
        if was_playing {
            self.start(0);
        } else {
            self.queue.stop();
        }
        Change::Partial
    }

    fn seek(&mut self, fraction: f32) -> Change {
        let Some(length) = self.length else {
            return Change::None;
        };
        let target = (length as f32 * fraction.clamp(0.0, 1.0)) as u64;
        if self.playback.is_playing() {
            self.start(target);
        } else {
            self.resume_at = target;
        }
        Change::Partial
    }

    fn draw_library(&self, canvas: &mut Canvas) {
        let width = canvas.bounds().w;
        canvas.text(screen_rect(width, 40, 50, 1000, 120), "Library", TextStyle::new(84).bold());
        canvas.button(back_button(width), "Exit", true, false);
        canvas.text(screen_rect(width, 40, 176, 1184, 60), &self.connection_line(), CAPTION);

        if self.queue.tracks().is_empty() {
            full_screen_message(
                canvas,
                "No music yet",
                "Copy music files to /mnt/us/music or to /mnt/us/music/library, then open Music again.",
            );
        }
        for (index, (title, detail)) in self.page_rows().iter().enumerate() {
            draw_track_row(canvas, row_rect(width, index), title, detail, self.is_current(self.page * PER_PAGE + index));
        }
        if self.page_count() > 1 {
            self.draw_pager(canvas, width);
        }
        if self.chosen {
            if let Some(track) = self.queue.current() {
                self.draw_mini_player(canvas, width, &track.title);
            }
        }
        draw_tabs(canvas, width, 0);
    }

    fn draw_pager(&self, canvas: &mut Canvas, width: i32) {
        if self.page > 0 {
            text_control(canvas, pager_previous(width), "Previous", TextStyle::new(38).bold());
        }
        if self.page + 1 < self.page_count() {
            text_control(canvas, pager_next(width), "Next", TextStyle::new(38).bold().right());
        }
        let middle = screen_rect(width, 432, 1262, 400, 40);
        canvas.text(middle, &format!("{} of {}", self.page + 1, self.page_count()), CAPTION.center());
    }

    fn draw_mini_player(&self, canvas: &mut Canvas, width: i32, title: &str) {
        let rect = mini_rect(width);
        canvas.panel(rect, 28, 2, PAPER);
        let pad = canvas.px(32);
        let text_width = rect.w - pad - canvas.px(140);
        canvas.text(
            Rect::new(rect.x + pad, rect.y + canvas.px(18), text_width, canvas.px(56)),
            title,
            TextStyle::new(38).bold(),
        );
        canvas.text(
            Rect::new(rect.x + pad, rect.y + canvas.px(70), text_width, canvas.px(36)),
            &self.connection_line(),
            CAPTION,
        );
        let symbol = if self.playback.is_playing() { PAUSE } else { PLAY };
        round_control(canvas, mini_button(width), symbol, true);
    }

    fn draw_now(&self, canvas: &mut Canvas) {
        let width = canvas.bounds().w;
        let layout = NowLayout::new(width);
        let Some(track) = self.queue.current().filter(|_| self.chosen) else {
            full_screen_message(canvas, "Nothing playing", "Choose a track in the library.");
            draw_tabs(canvas, width, 1);
            return;
        };

        canvas.panel(layout.artwork, 24, 3, ARTWORK);
        canvas.icon(layout.artwork.inset(layout.artwork.w / 3), NOTE, NOTE_INK);

        let position = self.position();
        let shown = self.length.map_or(position, |length| position.min(length));
        canvas.text(screen_rect(width, 40, 950, 1184, 100), &track.title, TextStyle::new(58).bold().lines(1));
        canvas.text(
            screen_rect(width, 40, 1036, 1184, 50),
            &format!(
                "{} · {} of {}",
                format_label(track),
                self.queue.current_index() + 1,
                self.queue.tracks().len()
            ),
            CAPTION,
        );

        canvas.fill(layout.bar, HAIRLINE);
        if let Some(length) = self.length {
            let fraction = if length == 0 { 0.0 } else { shown as f32 / length as f32 };
            let filled = (layout.bar.w as f32 * fraction) as i32;
            canvas.fill(Rect::new(layout.bar.x, layout.bar.y, filled, layout.bar.h), BLACK);
        }
        canvas.text(screen_rect(width, 40, 1134, 400, 50), &format_time(shown), CAPTION);
        if let Some(length) = self.length {
            let remaining = format!("-{}", format_time(length.saturating_sub(shown)));
            canvas.text(screen_rect(width, 824, 1134, 400, 50), &remaining, CAPTION.right());
        }

        round_control(canvas, layout.previous, PREVIOUS, false);
        let playing = self.playback.is_playing();
        round_control(canvas, layout.play, if playing { PAUSE } else { PLAY }, true);
        round_control(canvas, layout.next, NEXT, false);

        let detail = self.message.clone().unwrap_or_else(|| match self.queue.state() {
            PlayState::Playing => self.connection_line(),
            PlayState::Paused => "Paused".into(),
            PlayState::Stopped => "Stopped".into(),
        });
        canvas.text(screen_rect(width, 40, 1424, 1184, 60), &detail, CAPTION.center());
        draw_tabs(canvas, width, 1);
    }

    fn library_hit(&mut self, x: f32, y: f32, width: i32) -> Change {
        if back_button(width).contains(x, y) {
            return Change::Exit;
        }
        if self.chosen && mini_rect(width).contains(x, y) {
            if mini_button(width).contains(x, y) {
                return self.toggle();
            }
            self.screen = Screen::Now;
            return Change::Full;
        }
        if let Some(index) = (0..self.page_rows().len()).find(|&index| row_rect(width, index).contains(x, y)) {
            return self.open(self.page * PER_PAGE + index);
        }
        if self.page > 0 && pager_previous(width).contains(x, y) {
            self.page -= 1;
            return Change::Partial;
        }
        if self.page + 1 < self.page_count() && pager_next(width).contains(x, y) {
            self.page += 1;
            return Change::Partial;
        }
        self.tab_hit(x, y, width)
    }

    fn now_hit(&mut self, x: f32, y: f32, width: i32) -> Change {
        let layout = NowLayout::new(width);
        if layout.previous.contains(x, y) {
            return self.move_track(false);
        }
        if layout.play.contains(x, y) {
            return self.toggle();
        }
        if layout.next.contains(x, y) {
            return self.move_track(true);
        }
        if layout.bar_touch.contains(x, y) {
            let fraction = (x - layout.bar.x as f32) / layout.bar.w as f32;
            return self.seek(fraction);
        }
        self.tab_hit(x, y, width)
    }

    /// A tab bar tap: Library goes to the list, Playing to the player (only once a track is chosen).
    fn tab_hit(&mut self, x: f32, y: f32, width: i32) -> Change {
        let bar = TabBar { bounds: tabs_rect(width), ..tabs(0) };
        match bar.hit(x, y) {
            Some(0) if self.screen != Screen::Library => {
                self.screen = Screen::Library;
                Change::Full
            }
            Some(1) if self.screen != Screen::Now && self.chosen => {
                self.screen = Screen::Now;
                Change::Full
            }
            _ => Change::None,
        }
    }

    fn tick_playback(&mut self) -> Change {
        if !self.playback.is_playing() {
            return Change::None;
        }
        match self.playback.poll() {
            Event::Playing => {
                let step = self.position() / SHOWN_EVERY;
                if step != self.shown {
                    self.shown = step;
                    Change::Partial
                } else {
                    Change::None
                }
            }
            Event::Ended => {
                // The track finished: the next one plays, and the list repeats.
                self.queue.finished();
                self.resume_at = 0;
                self.length = self.current_length();
                self.start(0);
                Change::Partial
            }
            Event::Failed(error) => {
                self.playback.stop();
                self.queue.stop();
                self.message = Some(error);
                Change::Partial
            }
        }
    }
}

fn format_label(track: &Track) -> String {
    track
        .path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_uppercase())
        .unwrap_or_default()
}

fn draw_tabs(canvas: &mut Canvas, width: i32, selected: usize) {
    TabBar { bounds: tabs_rect(width), ..tabs(selected) }.draw(canvas, None);
}

/// A track row: a square with a note, the title and its detail, and a hairline below. A row under the finger turns grey.
fn draw_track_row(canvas: &mut Canvas, rect: Rect, title: &str, detail: &str, current: bool) {
    if canvas.control_pressed(rect, false) {
        canvas.fill(rect, HAIRLINE);
    }
    let size = rect.h - canvas.px(40);
    let thumb = Rect::new(rect.x + canvas.px(12), rect.y + canvas.px(20), size, size);
    canvas.panel(thumb, 12, 2, ARTWORK);
    canvas.icon(thumb.inset(thumb.w / 4), NOTE, if current { BLACK } else { NOTE_INK });
    let text_x = thumb.right() + canvas.px(28);
    let text_width = rect.right() - text_x - canvas.px(16);
    canvas.text(
        Rect::new(text_x, rect.y + canvas.px(24), text_width, canvas.px(56)),
        title,
        TextStyle::new(38).bold(),
    );
    canvas.text(Rect::new(text_x, rect.y + canvas.px(80), text_width, canvas.px(40)), detail, CAPTION);
    canvas.fill(Rect::new(text_x, rect.bottom() - 2, rect.right() - text_x, 2), HAIRLINE);
}

/// A round control with an icon. It reverses while held, as the kit's buttons do: a solid face turns into a ring,
/// and an empty face fills. `solid` is the normal face (the primary control is solid).
fn round_control(canvas: &mut Canvas, rect: Rect, symbol: Icon, solid: bool) {
    let pressed = canvas.control_pressed(rect, false);
    let filled = solid != pressed;
    canvas.circle(rect, BLACK);
    if !filled {
        canvas.circle(rect.inset(canvas.px(4)), PAPER);
    }
    let ink = if filled { PAPER } else { BLACK };
    canvas.icon(rect.inset(rect.w / 4), symbol, ink);
}

/// A text control with no box, such as the pager's Previous and Next. It fills while held.
fn text_control(canvas: &mut Canvas, rect: Rect, label: &str, style: TextStyle) {
    let pressed = canvas.control_pressed(rect, false);
    if pressed {
        canvas.fill(rect, BLACK);
    }
    canvas.text(rect, label, if pressed { style.ink(PAPER) } else { style });
}

impl App for Music {
    fn draw(&self, canvas: &mut Canvas) {
        match self.screen {
            Screen::Library => self.draw_library(canvas),
            Screen::Now => self.draw_now(canvas),
        }
    }

    fn input(&mut self, input: Input) -> Change {
        let width = primitive_app::WIDTH as i32;
        match input {
            Input::Sleep => {
                // The player stops at sleep: the saved place is the position it stopped at.
                self.pause();
                Change::None
            }
            Input::Wake => Change::None,
            Input::PageForward if self.screen == Screen::Library && self.page + 1 < self.page_count() => {
                self.page += 1;
                Change::Partial
            }
            Input::PageBack if self.screen == Screen::Library && self.page > 0 => {
                self.page -= 1;
                Change::Partial
            }
            Input::PageBack if self.screen == Screen::Now => {
                self.screen = Screen::Library;
                Change::Full
            }
            Input::PageBack => Change::None,
            Input::Tap(x, y) => match self.screen {
                Screen::Library => self.library_hit(x, y, width),
                Screen::Now => self.now_hit(x, y, width),
            },
            _ => Change::None,
        }
    }

    fn deadline(&self) -> Option<Instant> {
        Some(self.wake_at)
    }

    fn tick(&mut self, now: Instant) -> Change {
        let mut change = self.tick_playback();
        if now.duration_since(self.checked_at) >= CHECK_EVERY {
            self.checked_at = now;
            let device = (self.headphones)();
            if device != self.device {
                self.device = device;
                change = Change::Partial;
                if self.device.is_none() && self.playback.is_playing() {
                    self.pause();
                    self.message = Some("The headphones disconnected, so playback paused.".into());
                }
            }
        }
        self.wake_at = now + if self.playback.is_playing() { POLL_EVERY } else { CHECK_EVERY };
        change
    }

    fn save(&mut self) -> io::Result<()> {
        let Some(path) = &self.state_file else {
            return Ok(());
        };
        let saved = Saved {
            path: self.queue.current().map(|track| track.path.clone()),
            position: self.position(),
        };
        save(path, saved.to_text().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const WIDTH: i32 = primitive_app::WIDTH as i32;

    fn tracks(count: usize) -> Vec<Track> {
        (0..count)
            .map(|i| Track {
                path: PathBuf::from(format!("/no/such/music/{i:02}.mp3")),
                title: format!("Track {i:02}"),
            })
            .collect()
    }

    fn app(count: usize, device: Option<&str>, saved: Saved) -> Music {
        let device = device.map(str::to_string);
        Music::new(
            tracks(count),
            saved,
            Playback::new("/bin/cat", std::env::temp_dir().join(format!("music-render-test-{}", std::process::id()))),
            Box::new(move || device.clone()),
            None,
        )
    }

    fn center(rect: Rect) -> Input {
        Input::Tap((rect.x + rect.w / 2) as f32, (rect.y + rect.h / 2) as f32)
    }

    #[test]
    fn a_row_opens_the_track_and_done_leaves() {
        let mut music = app(9, Some("AirPods Pro"), Saved::default());
        assert_eq!(music.input(center(row_rect(WIDTH, 0))), Change::Full);
        assert_eq!(music.screen(), Screen::Now);
        assert_eq!(music.queue().current_index(), 0);
        assert_eq!(music.input(Input::PageBack), Change::Full, "the hardware back returns to the list");
        assert_eq!(music.screen(), Screen::Library);
        assert_eq!(music.input(center(back_button(WIDTH))), Change::Exit, "Exit leaves the app");
    }

    #[test]
    fn the_pager_turns_pages_and_stops_at_the_ends() {
        let mut music = app(9, Some("AirPods Pro"), Saved::default());
        assert_eq!(music.input(Input::PageBack), Change::None);
        assert_eq!(music.input(center(pager_next(WIDTH))), Change::Partial);
        assert_eq!(music.page(), 1);
        assert_eq!(music.input(Input::PageForward), Change::None, "already on the last page");
        assert_eq!(music.input(center(pager_previous(WIDTH))), Change::Partial);
        assert_eq!(music.page(), 0);
    }

    #[test]
    fn the_tab_bar_switches_between_list_and_player() {
        let mut music = app(3, Some("AirPods Pro"), Saved::default());
        let bar = tabs_rect(WIDTH);
        let playing_tab = Rect::new(bar.x + bar.w * 3 / 4, bar.y, bar.w / 4, bar.h);
        assert_eq!(music.input(center(playing_tab)), Change::None, "no track chosen yet");
        music.input(center(row_rect(WIDTH, 0)));
        music.input(Input::PageBack);
        assert_eq!(music.input(center(playing_tab)), Change::Full);
        assert_eq!(music.screen(), Screen::Now);
    }

    #[test]
    fn the_mini_player_opens_the_player_and_its_button_toggles() {
        let mut music = app(3, Some("AirPods Pro"), Saved::default());
        music.show_paused_at(1, 10);
        music.input(Input::PageBack);
        assert_eq!(music.screen(), Screen::Library);
        assert_eq!(music.input(center(mini_button(WIDTH))), Change::Partial, "the button toggles");
        assert_eq!(music.input(center(mini_rect(WIDTH))), Change::Full, "the rest of the bar opens the player");
        assert_eq!(music.screen(), Screen::Now);
    }

    #[test]
    fn playing_needs_headphones() {
        let mut music = app(3, None, Saved::default());
        music.input(center(row_rect(WIDTH, 0)));
        assert_eq!(music.queue().state(), PlayState::Stopped);
        assert!(music.message.as_deref().unwrap().contains("Connect headphones"));
        assert!(!music.playback.is_playing());
    }

    #[test]
    fn a_file_that_cannot_be_read_stops_with_a_message() {
        let mut music = app(3, Some("AirPods Pro"), Saved::default());
        music.input(center(row_rect(WIDTH, 0)));
        assert_eq!(music.queue().state(), PlayState::Stopped);
        assert!(music.message.as_deref().unwrap().contains("could not open"));
    }

    #[test]
    fn the_saved_place_selects_its_track_without_playing() {
        let saved = Saved { path: Some(PathBuf::from("/no/such/music/04.mp3")), position: 42 };
        let music = app(9, Some("AirPods Pro"), saved);
        assert_eq!(music.queue().current_index(), 4);
        assert_eq!(music.queue().state(), PlayState::Stopped);
        assert_eq!(music.position(), 42);
        assert_eq!(music.page(), 0);
    }

    #[test]
    fn sleep_pauses_and_the_place_is_saved() {
        let dir = std::env::temp_dir().join(format!("music-render-save-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state.txt");
        let mut music = Music::new(
            tracks(2),
            Saved::default(),
            Playback::new("/bin/cat", dir.join("segments")),
            Box::new(|| Some("AirPods Pro".into())),
            Some(state.clone()),
        );
        music.show_paused_at(1, 75);
        music.input(Input::Sleep);
        music.save().unwrap();
        let saved = Saved::from_text(&fs::read_to_string(&state).unwrap());
        assert_eq!(saved, Saved { path: Some(PathBuf::from("/no/such/music/01.mp3")), position: 75 });
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_library_shows_seven_tracks_a_page() {
        let music = app(9, Some("AirPods Pro"), Saved::default());
        assert_eq!(music.page_rows().len(), PER_PAGE);
        assert_eq!(music.page_count(), 2);
    }
}
