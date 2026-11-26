<div align="center">

<img src="assets/music-icon.png" alt="Music icon" width="100">

# Music

**Play the music on your Kindle through Bluetooth headphones.**

[Install guide](INSTALL.md) · [Device notes](docs/device-notes.md) · [Changelog](CHANGELOG.md)

</div>

Music is a standalone app for PrimitiveOS. It plays the music files in your Kindle's user storage through the
Bluetooth headphones connected in Settings › Bluetooth. The Kindle has no speaker, so Music needs headphones to play.

## Using Music

- **Library.** Tracks are listed by title, seven to a page. Copy music files to `/mnt/us/music` (or to
  `/mnt/us/music/library`). Tap a track to play it.
- **Player.** Pause and resume, previous and next, and tap the progress line to seek. The eight segments under the
  controls set the sound level, which is the same level as quick settings.
- **Background play.** Exit leaves the app and the music keeps playing. Open Music again to see what is playing.
  Pausing stops playback and keeps the place.
- **Quick settings.** While music plays, quick settings shows a card with the track and the controls, so you can
  pause or skip without opening Music.
- **Sleep.** Sleeping the Kindle pauses playback. The place is kept.

Music reads MP3, FLAC, Ogg Vorbis, WAV, M4A and AAC files. Titles come from the file names.

## How it works

The player is a separate process, so the music keeps playing when the app closes. The app is a remote for it.
Playback uses the Kindle's own player (`mplay`): tracks are decoded in 30-second segments, and the stock audio manager
sends them to the headphones. The media card follows the media session contract in
[docs/contracts.md](https://github.com/PrimitiveOS/kindle-primitive/blob/feature/system-architecture/docs/contracts.md) §1.2, so other apps
that play audio can show their controls the same way.

## Build and preview

Keep `doannt-kindle-tools` beside this checkout (Music builds against it by path).

```sh
cargo test --offline --workspace
cargo run --offline -p music-desktop -- build/previews
cargo run --offline -p music-desktop -- listen <file> --pause-after 35
cargo zigbuild --offline --release -p music-kindle --target armv7-unknown-linux-musleabihf
sh scripts/stage.sh armhf
```

The desktop target writes the screen previews to `build/previews/`. `listen` plays a file through the Mac's `afplay`
with the same segments and pause logic as the Kindle. The end-to-end player test
(`crates/music-kindle/tests/player.rs`, macOS only) runs the real player against silent audio, and checks play, pause,
seek, headphones leaving, stop, and the media session.

## Status

Music 0.1.0 is released. The tests above pass on a computer. On the Kindle Oasis 3 (firmware 5.18.2), Music
opens from the launcher, its library lists the copied tracks, and playback through the AirPods Pro was heard. Still to
check on the device: the sound row, the media card, and sound during a segment join. The
[device notes](docs/device-notes.md) list the checks.

## Licence

Copyright (C) 2026 Doan Nguyen.

Code is licensed under [PolyForm Noncommercial 1.0.0](LICENSE). Original documentation and pictures are licensed under
[CC BY-NC-SA 4.0](LICENSE-DOCS). Decoding uses [Symphonia](https://github.com/pdeljanov/Symphonia), licensed under
MPL-2.0.
