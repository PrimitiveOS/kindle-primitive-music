# Music

Plays the music files on your Kindle through Bluetooth headphones. It is a native PrimitiveOS app: a library screen
with a mini player, and a player screen styled after Apple Music.

Music reads `/mnt/us/music` and `/mnt/us/music/library`. It plays MP3, FLAC, Ogg Vorbis, WAV, M4A and AAC files.
Playing needs headphones connected in Settings › Bluetooth, because the Kindle has no speaker.

## How it plays

The Kindle's stock player, `mplay`, plays WAV files through the audio manager to the connected headphones. Music
decodes each track in 30-second segments into a WAV file in `/var/tmp` (RAM), and the next segment is decoded while the
current one plays. Pausing stops the player and resumes from the saved position. Sleep pauses playback.

Decoding uses [Symphonia](https://github.com/pdeljanov/Symphonia) (MPL-2.0), a pure Rust library.

## Layout

- `crates/music-core`: the library (which files are music, the sort order), the queue, time text and the saved place.
- `crates/music-audio`: decoding (Symphonia) and the segment player (`mplay` on the Kindle, `afplay` on a computer).
- `crates/music-render`: the screens, touch handling and playback control.
- `crates/music-kindle`: the device entry point.
- `crates/music-desktop`: screen previews (`cargo run -p music-desktop`) and a listening test on a computer
  (`cargo run -p music-desktop -- listen <file> [--pause-after SECS]`).

## Build and test

    cargo test --workspace --offline
    cargo run -p music-desktop --offline -- build/previews
    cargo zigbuild --release --offline --target armv7-unknown-linux-musleabihf -p music-kindle

The app builds against the PrimitiveOS kit by path (`../doannt-kindle-tools`).

## Licence

Code: PolyForm Noncommercial 1.0.0 (see `LICENSE`). Documentation: see `LICENSE-DOCS`.
