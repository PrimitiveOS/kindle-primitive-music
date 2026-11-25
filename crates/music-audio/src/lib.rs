//! Decoding music files and playing them through the Kindle's own player.
//!
//! The Kindle has no sound card. Its `mplay` command plays a WAV file through the stock audio manager, which sends it to
//! connected Bluetooth headphones. `mplay` cannot read from a pipe, so a track is decoded in segments of
//! [`SEGMENT_SECS`] seconds, each written to a small WAV file in a scratch folder. The next segment is decoded while
//! the current one plays, so the gap between segments is only the time `mplay` takes to start.
//!
//! The same [`Playback`] runs on a computer with `afplay` for previews and listening tests.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

/// Seconds of audio in one WAV file handed to the player.
pub const SEGMENT_SECS: u64 = 30;

/// Decoded audio: 16-bit interleaved samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

impl Pcm {
    /// The length of the audio in whole seconds.
    pub fn seconds(&self) -> u64 {
        let per_second = u64::from(self.rate) * u64::from(self.channels);
        if per_second == 0 {
            0
        } else {
            self.samples.len() as u64 / per_second
        }
    }

    /// The audio as a 16-bit PCM WAV file.
    pub fn wav_bytes(&self) -> Vec<u8> {
        let data_len = (self.samples.len() * 2) as u32;
        let block = u32::from(self.channels) * 2;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&self.channels.to_le_bytes());
        out.extend_from_slice(&self.rate.to_le_bytes());
        out.extend_from_slice(&(self.rate * block).to_le_bytes());
        out.extend_from_slice(&(block as u16).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for sample in &self.samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }
}

fn open(path: &Path) -> Result<Box<dyn FormatReader>, String> {
    let file = File::open(path).map_err(|e| format!("could not open the file: {e}"))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    symphonia::default::get_probe()
        .probe(&hint, stream, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("not a supported music file: {e}"))
}

/// The length of a file in whole seconds, read from its header. `None` when the header does not say.
pub fn duration(path: &Path) -> Option<u64> {
    let format = open(path).ok()?;
    let track = format.default_track(TrackType::Audio)?;
    let frames = track.num_frames?;
    let time = track.time_base?.calc_time(symphonia::core::units::Timestamp::new(frames as i64))?;
    Some(time.as_secs().max(0) as u64)
}

/// Decode `secs` seconds of a file from `start` (in seconds). `Ok(None)` when there is no audio at that point.
pub fn decode(path: &Path, start: u64, secs: u64) -> Result<Option<Pcm>, String> {
    let mut format = open(path)?;
    let (track_id, params) = {
        let track = format.default_track(TrackType::Audio).ok_or("no audio track in this file")?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio().cloned())
            .ok_or("the audio format is not supported")?;
        (track.id, params)
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| format!("the audio format is not supported: {e}"))?;
    if start > 0 {
        let time = Time::try_from_secs_f64(start as f64).ok_or("the position is out of range")?;
        // A seek past the end is an error in the decoder: there is no audio left at that point.
        if format.seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(track_id) }).is_err() {
            return Ok(None);
        }
    }

    let mut pcm = Pcm { rate: 0, channels: 0, samples: Vec::new() };
    let mut chunk: Vec<i16> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) | Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("could not read the file: {e}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(Error::DecodeError(_)) | Err(Error::IoError(_)) => continue,
            Err(e) => return Err(format!("could not decode the file: {e}")),
        };
        let spec = decoded.spec();
        pcm.rate = spec.rate();
        pcm.channels = spec.channels().count() as u16;
        decoded.copy_to_vec_interleaved(&mut chunk);
        pcm.samples.extend_from_slice(&chunk);
        if pcm.samples.len() as u64 >= secs * u64::from(pcm.rate) * u64::from(pcm.channels) {
            break;
        }
    }
    let wanted = secs * u64::from(pcm.rate) * u64::from(pcm.channels);
    pcm.samples.truncate(wanted as usize);
    Ok((!pcm.samples.is_empty()).then_some(pcm))
}

/// What the player reports on each poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Audio is playing (or the player has just moved to the next segment).
    Playing,
    /// The track has no more audio: the last segment has finished.
    Ended,
    /// A segment could not be started.
    Failed(String),
}

/// Plays one track at a time: segment after segment through `program` (`mplay` on the Kindle, `afplay` on a computer).
pub struct Playback {
    program: PathBuf,
    dir: PathBuf,
    track: Option<PathBuf>,
    child: Option<Child>,
    started: Option<Instant>,
    /// Where the playing segment starts, in seconds.
    start: u64,
    /// The next segment, decoded ahead while the current one plays.
    queued: Option<(u64, Pcm)>,
    /// Which of the two segment file names is next (the one before may still be read by the player).
    file: usize,
}

impl Playback {
    /// `dir` is a scratch folder for segment files. It is created if missing.
    pub fn new(program: impl Into<PathBuf>, dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let _ = fs::create_dir_all(&dir);
        Self { program: program.into(), dir, track: None, child: None, started: None, start: 0, queued: None, file: 0 }
    }

    pub fn is_playing(&self) -> bool {
        self.child.is_some()
    }

    pub fn track(&self) -> Option<&Path> {
        self.track.as_deref()
    }

    /// The position in seconds: where the playing segment began, plus the time it has run.
    pub fn position(&self) -> u64 {
        self.start + self.started.map_or(0, |t| t.elapsed().as_secs())
    }

    /// Start `track` at `from` seconds, stopping whatever was playing before.
    pub fn start_at(&mut self, track: &Path, from: u64) -> Result<(), String> {
        self.halt();
        self.track = Some(track.to_path_buf());
        let first = decode(track, from, SEGMENT_SECS)?.ok_or("there is no audio at this point")?;
        self.launch(from, &first)?;
        self.prefetch();
        Ok(())
    }

    /// Stop playing and return the position to resume from.
    pub fn stop(&mut self) -> u64 {
        let position = self.position();
        self.halt();
        position
    }

    /// Check on the player. Call it often (about once a second) while playing.
    pub fn poll(&mut self) -> Event {
        let Some(child) = self.child.as_mut() else {
            return Event::Ended;
        };
        match child.try_wait() {
            Ok(None) => Event::Playing,
            Ok(Some(_)) => {
                self.child = None;
                match self.queued.take() {
                    Some((start, pcm)) => match self.launch(start, &pcm) {
                        Ok(()) => {
                            self.prefetch();
                            Event::Playing
                        }
                        Err(error) => Event::Failed(error),
                    },
                    None => {
                        self.started = None;
                        Event::Ended
                    }
                }
            }
            Err(e) => Event::Failed(format!("the player stopped unexpectedly: {e}")),
        }
    }

    fn launch(&mut self, start: u64, pcm: &Pcm) -> Result<(), String> {
        self.halt_child();
        let path = self.dir.join(format!("segment-{}.wav", self.file));
        self.file ^= 1;
        fs::write(&path, pcm.wav_bytes()).map_err(|e| format!("could not write the audio: {e}"))?;
        let child = Command::new(&self.program)
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start the player: {e}"))?;
        self.child = Some(child);
        self.started = Some(Instant::now());
        self.start = start;
        Ok(())
    }

    /// Decode the segment after the one playing, so it is ready when the current one ends.
    fn prefetch(&mut self) {
        let (Some(track), true) = (self.track.clone(), self.child.is_some()) else {
            return;
        };
        let next = self.start + SEGMENT_SECS;
        self.queued = match decode(&track, next, SEGMENT_SECS) {
            Ok(Some(pcm)) => Some((next, pcm)),
            Ok(None) | Err(_) => None,
        };
    }

    fn halt(&mut self) {
        self.halt_child();
        self.queued = None;
        self.started = None;
        self.track = None;
        self.start = 0;
    }

    fn halt_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Ask politely first: the player should release the audio stream when it gets TERM.
            terminate(&child);
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.halt_child();
        for file in 0..2 {
            let _ = fs::remove_file(self.dir.join(format!("segment-{file}.wav")));
        }
    }
}

#[cfg(unix)]
fn terminate(child: &Child) {
    // SAFETY: kill only sends a signal to the child process this program started.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
}

#[cfg(not(unix))]
fn terminate(_child: &Child) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_file_has_a_44_byte_header_and_the_samples() {
        let pcm = Pcm { rate: 16_000, channels: 2, samples: vec![1, -1, 300, -300] };
        let bytes = pcm.wav_bytes();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 8);
        assert_eq!(bytes.len(), 52);
        assert_eq!(pcm.seconds(), 0);
    }

    #[test]
    fn a_wav_file_decodes_back_to_its_samples() {
        let dir = std::env::temp_dir().join(format!("music-audio-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        // One second of a 440 Hz tone, stereo, 8 kHz.
        let samples: Vec<i16> = (0..8_000)
            .flat_map(|i| {
                let v = ((i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 8_000.0).sin() * 8_000.0) as i16;
                [v, v]
            })
            .collect();
        let pcm = Pcm { rate: 8_000, channels: 2, samples: samples.clone() };
        fs::write(&path, pcm.wav_bytes()).unwrap();

        assert_eq!(duration(&path), Some(1));
        let back = decode(&path, 0, 30).unwrap().expect("audio");
        assert_eq!((back.rate, back.channels), (8_000, 2));
        assert_eq!(back.samples.len(), samples.len());
        assert_eq!(back.samples, samples);
        assert_eq!(decode(&path, 5, 1).unwrap(), None, "past the end there is no audio");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_listed_formats_decode_to_their_length() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        for name in ["tone.mp3", "tone.flac", "tone.ogg", "tone.m4a"] {
            let path = fixtures.join(name);
            let pcm = decode(&path, 0, SEGMENT_SECS)
                .unwrap_or_else(|e| panic!("{name}: {e}"))
                .unwrap_or_else(|| panic!("{name}: no audio"));
            assert_eq!(pcm.rate, 22_050, "{name}");
            assert_eq!(pcm.channels, 1, "{name}");
            let seconds = pcm.samples.len() as f64 / 22_050.0;
            assert!((0.9..=1.1).contains(&seconds), "{name} decoded {seconds} s");
            if let Some(length) = duration(&path) {
                assert_eq!(length, 1, "{name}");
            }
        }
    }

    #[test]
    fn a_segment_past_the_end_is_empty() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone.flac");
        assert_eq!(decode(&path, 1, SEGMENT_SECS).unwrap(), None);
        assert!(decode(&path, 0, SEGMENT_SECS).unwrap().is_some());
        assert!(decode(Path::new("/nonexistent/tone.mp3"), 0, 1).is_err());
    }
}
