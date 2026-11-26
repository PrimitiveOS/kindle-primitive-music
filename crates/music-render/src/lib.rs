//! The Music app's screens and touch handling. The app is a remote for the player: the player runs apart from the app
//! (see `music-kindle`), so music keeps playing when the app closes, as on an Apple device. The app sends commands and
//! reads the player's status.
//!
//! The layout follows Apple Music: a large title over a list of tracks, a mini player above a floating tab bar, and a
//! full player with a large artwork square, a thin progress line and round transport buttons. The files carry no
//! artwork, so the square shows a note. Playing needs headphones: the Kindle has no speaker. Sleep pauses playback.
//! Exit leaves the app and music goes on; Stop ends playback.

use std::path::Path;
use std::time::{Duration, Instant};

use music_audio::duration;
use music_core::{format_time, Command, PlayState, Snapshot, Track};
use primitive_app::{App, Change};
use primitive_ui::{
    back_button, full_screen_message, screen_rect, Canvas, Icon, Input, Rect, Tab, TabBar, TextStyle,
    BLACK, CAPTION, HAIRLINE, PAPER,
};

/// Tracks on one library page.
pub const PER_PAGE: usize = 7;
/// How often the status is read (the screen redraws only when it changed).
const WAKE_EVERY: Duration = Duration::from_secs(1);
/// How often the headphones are checked.
const CHECK_EVERY: Duration = Duration::from_secs(5);
/// How often the screen shows a new position while playing, in seconds.
const SHOWN_EVERY: u64 = 5;
/// The grey of the artwork's background and of the note on it.
const ARTWORK: u8 = 235;
const NOTE_INK: u8 = 120;

/// The headphones' name (the Bluetooth device's name), or `None` when none is connected.
pub type Headphones = Box<dyn Fn() -> Option<String>>;

/// The sound level of the Kindle's output: 1 (quietest) to `SOUND_LEVELS`. The device sets it; the app shows it.
pub trait Sound {
    /// The current level, or `None` when it cannot be read.
    fn level(&self) -> Option<u8>;
    fn set_level(&mut self, level: u8) -> Result<(), String>;
}

pub const SOUND_LEVELS: u8 = 8;

/// The player, as the app sees it.
pub trait Remote {
    /// The player's state now (also the saved place when no player runs).
    fn snapshot(&self) -> Snapshot;
    /// Send a command to the player (starting it for `Play`).
    fn send(&mut self, command: Command) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Library,
    Now,
}

const MINUS: Icon = Icon { stroke: &["M5 12 H19"], fill: &[] };
const PLUS: Icon = Icon { stroke: &["M5 12 H19 M12 5 V19"], fill: &[] };
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

fn tab_bar(bounds: Rect, selected: usize) -> TabBar<'static> {
    TabBar {
        bounds,
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
    minus: Rect,
    plus: Rect,
    /// The eight level segments between the two buttons (touch area as well).
    segments: Rect,
}

impl NowLayout {
    fn new(width: i32) -> Self {
        Self {
            artwork: screen_rect(width, 252, 130, 760, 760),
            bar: screen_rect(width, 40, 1058, 1184, 12),
            bar_touch: screen_rect(width, 40, 1030, 1184, 70),
            previous: screen_rect(width, 300, 1180, 140, 140),
            play: screen_rect(width, 532, 1150, 200, 200),
            next: screen_rect(width, 824, 1180, 140, 140),
            minus: screen_rect(width, 40, 1372, 72, 72),
            plus: screen_rect(width, 1152, 1372, 72, 72),
            segments: screen_rect(width, 140, 1372, 972, 72),
        }
    }

    /// The rectangle of level segment `index` (0 to 7), with a small gap between segments.
    fn segment(&self, width: i32, index: u8) -> Rect {
        let gap = screen_rect(width, 0, 0, 8, 0).w;
        let step = self.segments.w / i32::from(SOUND_LEVELS);
        Rect::new(self.segments.x + step * i32::from(index), self.segments.y, step - gap, self.segments.h)
    }
}

/// The level a touch at `x` picks on the segments (1 to `SOUND_LEVELS`), or `None` beside them.
fn level_at(layout: &NowLayout, x: f32) -> Option<u8> {
    let left = layout.segments.x as f32;
    let width = layout.segments.w as f32;
    if x < left || x >= left + width {
        return None;
    }
    let level = ((x - left) / width * f32::from(SOUND_LEVELS)).floor() as u8 + 1;
    Some(level.min(SOUND_LEVELS))
}

/// What the screen shows: when it changes, the screen redraws.
#[derive(Debug, PartialEq, Eq)]
struct View {
    state: PlayState,
    index: Option<usize>,
    shown: u64,
    message: Option<String>,
    device: Option<String>,
    sound: Option<u8>,
}

pub struct Music {
    tracks: Vec<Track>,
    remote: Box<dyn Remote>,
    sound: Box<dyn Sound>,
    /// The sound level when headphones are connected (read every few seconds).
    sound_level: Option<u8>,
    snapshot: Snapshot,
    /// The current track's length in seconds, when its header says.
    length: Option<u64>,
    length_of: Option<usize>,
    headphones: Headphones,
    device: Option<String>,
    checked_at: Instant,
    wake_at: Instant,
    screen: Screen,
    page: usize,
    /// A message from the app itself, such as "Connect headphones first." (the player's messages come in the status).
    notice: Option<String>,
}

impl Music {
    pub fn new(tracks: Vec<Track>, remote: Box<dyn Remote>, sound: Box<dyn Sound>, headphones: Headphones) -> Self {
        let now = Instant::now();
        let snapshot = remote.snapshot();
        let mut app = Self {
            tracks,
            remote,
            sound,
            sound_level: None,
            snapshot,
            length: None,
            length_of: None,
            headphones,
            device: None,
            checked_at: now,
            wake_at: now,
            screen: Screen::Library,
            page: 0,
            notice: None,
        };
        app.device = (app.headphones)();
        if let Some(index) = app.snapshot.index {
            app.page = index / PER_PAGE;
        }
        app.refresh_length();
        app.refresh_sound();
        app
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn show_now(&mut self) {
        self.screen = Screen::Now;
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn headphones(&self) -> Option<&str> {
        self.device.as_deref()
    }

    fn refresh(&mut self) {
        self.snapshot = self.remote.snapshot();
        self.refresh_length();
    }

    /// The sound level is shown only while headphones are connected.
    fn refresh_sound(&mut self) {
        self.sound_level = if self.device.is_some() { self.sound.level() } else { None };
    }

    fn set_sound(&mut self, level: u8) -> Change {
        let level = level.clamp(1, SOUND_LEVELS);
        match self.sound.set_level(level) {
            Ok(()) => {
                self.sound_level = Some(level);
                self.notice = None;
            }
            Err(error) => self.notice = Some(error),
        }
        Change::Partial
    }

    fn refresh_length(&mut self) {
        let index = self.snapshot.index.filter(|&i| i < self.tracks.len());
        if self.length_of != index {
            self.length_of = index;
            self.length = index.and_then(|i| duration(&self.tracks[i].path));
        }
    }

    fn current(&self) -> Option<&Track> {
        self.snapshot.index.and_then(|index| self.tracks.get(index))
    }

    fn playing(&self) -> bool {
        self.snapshot.state == PlayState::Playing
    }

    /// The mini player shows while something plays or is paused.
    fn shows_mini(&self) -> bool {
        self.snapshot.state != PlayState::Stopped && self.current().is_some()
    }

    fn page_count(&self) -> usize {
        self.tracks.len().div_ceil(PER_PAGE).max(1)
    }

    fn is_current(&self, index: usize) -> bool {
        self.snapshot.index == Some(index) && self.snapshot.state != PlayState::Stopped
    }

    fn row_detail(&self, index: usize, track: &Track) -> String {
        let format = format_label(track);
        if self.is_current(index) {
            let state = if self.playing() { "Playing" } else { "Paused" };
            format!("{format} · {state}")
        } else if self.snapshot.index == Some(index) {
            format!("{format} · Last played")
        } else {
            format
        }
    }

    fn connection_line(&self) -> String {
        match &self.device {
            Some(name) => format!("{name} connected"),
            None => "No headphones connected".into(),
        }
    }

    fn message(&self) -> Option<String> {
        self.notice.clone().or_else(|| self.snapshot.message.clone())
    }

    fn send(&mut self, command: Command) -> Change {
        self.notice = self.remote.send(command).err();
        self.refresh();
        Change::Partial
    }

    /// Play track `index` from `from` seconds, if headphones are connected.
    fn play_track(&mut self, index: usize, from: u64) -> Change {
        if self.device.is_none() {
            self.notice = Some("Connect headphones in Settings › Bluetooth first.".into());
            return Change::Partial;
        }
        self.send(Command::Play { index, from })
    }

    /// Pause, or resume the chosen track from its place.
    fn toggle(&mut self) -> Change {
        if self.playing() {
            self.send(Command::Pause)
        } else if let Some(index) = self.snapshot.index {
            self.play_track(index, self.snapshot.position)
        } else {
            Change::None
        }
    }

    /// A row tap: the current track pauses or resumes; another track plays (a stopped last-played track resumes from its
    /// place).
    fn open_row(&mut self, index: usize) -> Change {
        self.screen = Screen::Now;
        if self.is_current(index) {
            return self.toggle();
        }
        let from = if self.snapshot.index == Some(index) { self.snapshot.position } else { 0 };
        self.play_track(index, from);
        Change::Full
    }

    fn seek(&mut self, fraction: f32) -> Change {
        let Some(length) = self.length else {
            return Change::None;
        };
        let target = (length as f32 * fraction.clamp(0.0, 1.0)) as u64;
        self.send(Command::Seek(target))
    }

    fn view(&self) -> View {
        View {
            state: self.snapshot.state,
            index: self.snapshot.index,
            shown: self.snapshot.position / SHOWN_EVERY,
            message: self.message(),
            device: self.device.clone(),
            sound: self.sound_level,
        }
    }

    fn draw_library(&self, canvas: &mut Canvas) {
        let width = canvas.bounds().w;
        canvas.text(screen_rect(width, 40, 50, 1000, 120), "Library", TextStyle::new(84).bold());
        canvas.button(back_button(width), "Exit", true, false);
        canvas.text(screen_rect(width, 40, 176, 1184, 60), &self.connection_line(), CAPTION);

        if self.tracks.is_empty() {
            full_screen_message(
                canvas,
                "No music yet",
                "Copy music files to /mnt/us/music or to /mnt/us/music/library, then open Music again.",
            );
        }
        let start = self.page * PER_PAGE;
        let end = self.tracks.len().min(start + PER_PAGE);
        for (position, index) in (start..end).enumerate() {
            let track = &self.tracks[index];
            let detail = self.row_detail(index, track);
            draw_track_row(canvas, row_rect(width, position), &track.title, &detail, self.is_current(index));
        }
        if self.page_count() > 1 {
            self.draw_pager(canvas, width);
        }
        if self.shows_mini() {
            if let Some(track) = self.current() {
                self.draw_mini_player(canvas, width, &track.title);
            }
        }
        tab_bar(tabs_rect(width), 0).draw(canvas, None);
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
        let symbol = if self.playing() { PAUSE } else { PLAY };
        round_control(canvas, mini_button(width), symbol, true);
    }

    fn draw_now(&self, canvas: &mut Canvas) {
        let width = canvas.bounds().w;
        let layout = NowLayout::new(width);
        let Some(track) = self.current() else {
            full_screen_message(canvas, "Nothing playing", "Choose a track in the library.");
            tab_bar(tabs_rect(width), 1).draw(canvas, None);
            return;
        };

        canvas.panel(layout.artwork, 24, 3, ARTWORK);
        canvas.icon(layout.artwork.inset(layout.artwork.w / 3), NOTE, NOTE_INK);

        let position = self.snapshot.position;
        let shown = self.length.map_or(position, |length| position.min(length));
        canvas.text(screen_rect(width, 40, 912, 1184, 80), &track.title, TextStyle::new(58).bold().lines(1));
        let index = self.snapshot.index.unwrap_or(0) + 1;
        canvas.text(
            screen_rect(width, 40, 996, 1184, 50),
            &format!("{} · {} of {}", format_label(track), index, self.tracks.len()),
            CAPTION,
        );

        canvas.fill(layout.bar, HAIRLINE);
        if let Some(length) = self.length {
            let fraction = if length == 0 { 0.0 } else { shown as f32 / length as f32 };
            let filled = (layout.bar.w as f32 * fraction) as i32;
            canvas.fill(Rect::new(layout.bar.x, layout.bar.y, filled, layout.bar.h), BLACK);
        }
        canvas.text(screen_rect(width, 40, 1078, 400, 50), &format_time(shown), CAPTION);
        if let Some(length) = self.length {
            let remaining = format!("-{}", format_time(length.saturating_sub(shown)));
            canvas.text(screen_rect(width, 824, 1078, 400, 50), &remaining, CAPTION.right());
        }

        round_control(canvas, layout.previous, PREVIOUS, false);
        let symbol = if self.playing() { PAUSE } else { PLAY };
        round_control(canvas, layout.play, symbol, true);
        round_control(canvas, layout.next, NEXT, false);

        if let Some(level) = self.sound_level {
            round_control(canvas, layout.minus, MINUS, false);
            round_control(canvas, layout.plus, PLUS, false);
            for index in 0..SOUND_LEVELS {
                let rect = layout.segment(width, index);
                if index < level {
                    canvas.panel(rect, 8, 2, BLACK);
                } else {
                    canvas.panel(rect, 8, 2, PAPER);
                }
            }
        }

        let detail = self.message().unwrap_or_else(|| match self.snapshot.state {
            PlayState::Playing => self.connection_line(),
            PlayState::Paused => "Paused".into(),
            PlayState::Stopped => "Stopped".into(),
        });
        canvas.text(screen_rect(width, 40, 1456, 1184, 40), &detail, CAPTION.center());
        tab_bar(tabs_rect(width), 1).draw(canvas, None);
    }

    fn library_hit(&mut self, x: f32, y: f32, width: i32) -> Change {
        if back_button(width).contains(x, y) {
            // Leaving keeps the music going: the player is separate from this screen.
            return Change::Exit;
        }
        if self.shows_mini() && mini_rect(width).contains(x, y) {
            if mini_button(width).contains(x, y) {
                return self.toggle();
            }
            self.screen = Screen::Now;
            return Change::Full;
        }
        let start = self.page * PER_PAGE;
        let visible = self.tracks.len().saturating_sub(start).min(PER_PAGE);
        if let Some(position) = (0..visible).find(|&position| row_rect(width, position).contains(x, y)) {
            return self.open_row(start + position);
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
            return self.send(Command::Previous);
        }
        if layout.play.contains(x, y) {
            return self.toggle();
        }
        if layout.next.contains(x, y) {
            return self.send(Command::Next);
        }
        if layout.bar_touch.contains(x, y) {
            let fraction = (x - layout.bar.x as f32) / layout.bar.w as f32;
            return self.seek(fraction);
        }
        if self.sound_level.is_some() {
            // The level is read now, not from the last tick: the quick settings may have changed it since.
            let current = self.sound.level().or(self.sound_level).unwrap_or(1);
            if layout.minus.contains(x, y) {
                return self.set_sound(current.saturating_sub(1));
            }
            if layout.plus.contains(x, y) {
                return self.set_sound(current + 1);
            }
            if layout.segments.contains(x, y) {
                if let Some(target) = level_at(&layout, x) {
                    return self.set_sound(target);
                }
            }
        }
        self.tab_hit(x, y, width)
    }

    /// A tab bar tap: Library goes to the list, Playing to the player (once a track is chosen).
    fn tab_hit(&mut self, x: f32, y: f32, width: i32) -> Change {
        match tab_bar(tabs_rect(width), 0).hit(x, y) {
            Some(0) if self.screen != Screen::Library => {
                self.screen = Screen::Library;
                Change::Full
            }
            Some(1) if self.screen != Screen::Now && self.current().is_some() => {
                self.screen = Screen::Now;
                Change::Full
            }
            _ => Change::None,
        }
    }
}

fn format_label(track: &Track) -> String {
    Path::new(&track.path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_uppercase())
        .unwrap_or_default()
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

/// A text control with no box, such as Previous, Next and Stop. It fills while held.
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
                // The Kindle's sleep cuts the sound, so playback pauses; the place is kept.
                if self.playing() {
                    let _ = self.remote.send(Command::Pause);
                    self.refresh();
                }
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
        let before = self.view();
        self.refresh();
        if now.duration_since(self.checked_at) >= CHECK_EVERY {
            self.checked_at = now;
            self.device = (self.headphones)();
        }
        // The level is read every tick (the quick settings can change it too).
        self.refresh_sound();
        self.wake_at = now + WAKE_EVERY;
        if self.view() != before {
            Change::Partial
        } else {
            Change::None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    use super::*;

    const WIDTH: i32 = primitive_app::WIDTH as i32;

    /// A player that keeps its state in memory and records what it was sent.
    #[derive(Default)]
    struct Fake {
        snapshot: Snapshot,
        sent: Vec<Command>,
    }

    struct Shared(Rc<RefCell<Fake>>);

    impl Remote for Shared {
        fn snapshot(&self) -> Snapshot {
            self.0.borrow().snapshot.clone()
        }

        fn send(&mut self, command: Command) -> Result<(), String> {
            let mut fake = self.0.borrow_mut();
            match &command {
                Command::Play { index, from } => {
                    fake.snapshot.state = PlayState::Playing;
                    fake.snapshot.index = Some(*index);
                    fake.snapshot.position = *from;
                }
                Command::Pause => {
                    if fake.snapshot.state == PlayState::Playing {
                        fake.snapshot.state = PlayState::Paused;
                    }
                }
                Command::Stop => fake.snapshot.state = PlayState::Stopped,
                _ => {}
            }
            fake.sent.push(command);
            Ok(())
        }
    }

    fn tracks(count: usize) -> Vec<Track> {
        (0..count)
            .map(|i| Track {
                path: PathBuf::from(format!("/no/such/music/{i:02}.mp3")),
                title: format!("Track {i:02}"),
            })
            .collect()
    }

    /// A sound that keeps its level in memory.
    struct FakeSound(Rc<RefCell<u8>>);

    impl Sound for FakeSound {
        fn level(&self) -> Option<u8> {
            Some(*self.0.borrow())
        }

        fn set_level(&mut self, level: u8) -> Result<(), String> {
            *self.0.borrow_mut() = level;
            Ok(())
        }
    }

    fn app(count: usize, device: Option<&str>, snapshot: Snapshot) -> (Music, Rc<RefCell<Fake>>) {
        let (music, fake, _) = app_with_sound(count, device, snapshot, 4);
        (music, fake)
    }

    fn app_with_sound(
        count: usize,
        device: Option<&str>,
        snapshot: Snapshot,
        level: u8,
    ) -> (Music, Rc<RefCell<Fake>>, Rc<RefCell<u8>>) {
        let fake = Rc::new(RefCell::new(Fake { snapshot, sent: Vec::new() }));
        let sound = Rc::new(RefCell::new(level));
        let device = device.map(str::to_string);
        let mut music = Music::new(
            tracks(count),
            Box::new(Shared(fake.clone())),
            Box::new(FakeSound(sound.clone())),
            Box::new(move || device.clone()),
        );
        music.tick(Instant::now());
        (music, fake, sound)
    }

    fn center(rect: Rect) -> Input {
        Input::Tap((rect.x + rect.w / 2) as f32, (rect.y + rect.h / 2) as f32)
    }

    fn sent(fake: &Rc<RefCell<Fake>>) -> Vec<Command> {
        fake.borrow().sent.clone()
    }

    #[test]
    fn a_row_plays_the_track_and_opens_the_player() {
        let (mut music, fake) = app(9, Some("AirPods Pro"), Snapshot::default());
        assert_eq!(music.input(center(row_rect(WIDTH, 0))), Change::Full);
        assert_eq!(music.screen(), Screen::Now);
        assert_eq!(sent(&fake), vec![Command::Play { index: 0, from: 0 }]);
    }

    #[test]
    fn playing_needs_headphones_and_says_so() {
        let (mut music, fake) = app(3, None, Snapshot::default());
        music.input(center(row_rect(WIDTH, 0)));
        assert!(sent(&fake).is_empty());
        assert!(music.message().unwrap().contains("Connect headphones"));
    }

    #[test]
    fn exit_leaves_the_app_and_the_music_goes_on() {
        let playing = Snapshot { state: PlayState::Playing, index: Some(2), position: 40, message: None };
        let (mut music, fake) = app(9, Some("AirPods Pro"), playing);
        assert_eq!(music.input(center(back_button(WIDTH))), Change::Exit);
        assert!(sent(&fake).is_empty(), "leaving sends nothing: the player keeps playing");
    }

    #[test]
    fn the_player_has_no_stop_button_and_the_top_left_does_nothing() {
        let playing = Snapshot { state: PlayState::Playing, index: Some(2), position: 40, message: None };
        let (mut music, fake) = app(9, Some("AirPods Pro"), playing);
        music.show_now();
        let top_left = Rect::new(40, 40, 240, 80);
        assert_eq!(music.input(center(top_left)), Change::None);
        assert!(sent(&fake).is_empty());
    }

    #[test]
    fn the_sound_row_shows_only_with_headphones_and_moves_the_level() {
        let playing = Snapshot { state: PlayState::Playing, index: Some(2), position: 40, message: None };
        let (mut music, _, sound) = app_with_sound(9, Some("AirPods Pro"), playing.clone(), 4);
        music.show_now();
        let layout = NowLayout::new(WIDTH);
        assert_eq!(music.input(center(layout.plus)), Change::Partial);
        assert_eq!(*sound.borrow(), 5);
        music.input(center(layout.minus));
        music.input(center(layout.minus));
        assert_eq!(*sound.borrow(), 3);
        // A tap on the fifth segment sets level 5 (its own segment).
        let fifth = layout.segment(WIDTH, 4);
        music.input(center(fifth));
        assert_eq!(*sound.borrow(), 5);
        // The ends stop at the limits.
        for _ in 0..10 {
            music.input(center(layout.plus));
        }
        assert_eq!(*sound.borrow(), SOUND_LEVELS);
        for _ in 0..10 {
            music.input(center(layout.minus));
        }
        assert_eq!(*sound.borrow(), 1);

        let (mut without, _, _) = app_with_sound(9, None, playing, 4);
        without.show_now();
        assert_eq!(without.sound_level, None, "no headphones, no sound row");
        assert_eq!(without.input(center(NowLayout::new(WIDTH).plus)), Change::None);
    }

    #[test]
    fn a_touch_beside_the_segments_picks_no_level() {
        let layout = NowLayout::new(WIDTH);
        assert_eq!(level_at(&layout, layout.segments.x as f32 - 5.0), None);
        assert_eq!(level_at(&layout, layout.segments.x as f32 + 1.0), Some(1));
        assert_eq!(level_at(&layout, layout.segments.right() as f32 - 1.0), Some(SOUND_LEVELS));
    }

    #[test]
    fn the_play_button_pauses_then_resumes_from_the_place() {
        let playing = Snapshot { state: PlayState::Playing, index: Some(2), position: 40, message: None };
        let (mut music, fake) = app(9, Some("AirPods Pro"), playing);
        music.show_now();
        let layout = NowLayout::new(WIDTH);
        music.input(center(layout.play));
        assert_eq!(sent(&fake).last(), Some(&Command::Pause));
        music.input(center(layout.play));
        assert_eq!(sent(&fake).last(), Some(&Command::Play { index: 2, from: 40 }));
    }

    #[test]
    fn a_stopped_last_track_resumes_from_its_place() {
        let stopped = Snapshot { state: PlayState::Stopped, index: Some(4), position: 42, message: None };
        let (mut music, fake) = app(9, Some("AirPods Pro"), stopped);
        assert_eq!(music.page(), 0);
        music.show_now();
        music.input(center(NowLayout::new(WIDTH).play));
        assert_eq!(sent(&fake), vec![Command::Play { index: 4, from: 42 }]);
    }

    #[test]
    fn the_mini_player_shows_only_while_music_is_playing_or_paused() {
        let (music, _) = app(3, Some("AirPods Pro"), Snapshot::default());
        assert!(!music.shows_mini());
        let paused = Snapshot { state: PlayState::Paused, index: Some(1), position: 5, message: None };
        let (music, _) = app(3, Some("AirPods Pro"), paused);
        assert!(music.shows_mini());
    }

    #[test]
    fn the_pager_turns_pages_and_stops_at_the_ends() {
        let (mut music, _) = app(9, Some("AirPods Pro"), Snapshot::default());
        assert_eq!(music.input(Input::PageBack), Change::None);
        assert_eq!(music.input(center(pager_next(WIDTH))), Change::Partial);
        assert_eq!(music.page(), 1);
        assert_eq!(music.input(Input::PageForward), Change::None, "already on the last page");
        assert_eq!(music.input(center(pager_previous(WIDTH))), Change::Partial);
        assert_eq!(music.page(), 0);
    }

    #[test]
    fn the_tab_bar_switches_between_list_and_player() {
        let (mut music, _) = app(3, Some("AirPods Pro"), Snapshot::default());
        let bar = tabs_rect(WIDTH);
        let playing_tab = Rect::new(bar.x + bar.w * 3 / 4, bar.y, bar.w / 4, bar.h);
        assert_eq!(music.input(center(playing_tab)), Change::None, "no track chosen yet");
        let (mut music, _) = app(3, Some("AirPods Pro"), Snapshot { index: Some(0), ..Snapshot::default() });
        assert_eq!(music.input(center(playing_tab)), Change::Full);
        assert_eq!(music.screen(), Screen::Now);
    }

    #[test]
    fn sleep_pauses_playing_music() {
        let playing = Snapshot { state: PlayState::Playing, index: Some(1), position: 9, message: None };
        let (mut music, fake) = app(3, Some("AirPods Pro"), playing);
        assert_eq!(music.input(Input::Sleep), Change::None);
        assert_eq!(sent(&fake), vec![Command::Pause]);
    }

    #[test]
    fn the_status_message_reaches_the_screen() {
        let failed = Snapshot {
            state: PlayState::Paused,
            index: Some(1),
            position: 9,
            message: Some("The headphones disconnected, so playback paused.".into()),
        };
        let (music, _) = app(3, Some("AirPods Pro"), failed);
        assert!(music.message().unwrap().contains("disconnected"));
    }

    #[test]
    fn a_level_changed_outside_the_app_shows_on_the_next_tick_and_the_buttons_start_from_it() {
        let playing = Snapshot { state: PlayState::Playing, index: Some(2), position: 40, message: None };
        let (mut music, _, sound) = app_with_sound(9, Some("AirPods Pro"), playing, 4);
        music.show_now();
        assert_eq!(music.sound_level, Some(4));
        // The quick settings set the level to the maximum.
        *sound.borrow_mut() = 8;
        music.tick(Instant::now());
        assert_eq!(music.sound_level, Some(8), "the next tick shows it");
        // "−" starts from 8, not from the 4 the app showed before.
        let layout = NowLayout::new(WIDTH);
        music.input(center(layout.minus));
        assert_eq!(*sound.borrow(), 7);
    }
}
