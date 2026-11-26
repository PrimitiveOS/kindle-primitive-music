# Changelog

## 0.1.1

- The release package builds against the tools commit it names in full (the first 0.1.0 release run could not check it out, so it published nothing).

## 0.1.0

- First version. Library of music files in `/mnt/us/music` and `/mnt/us/music/library`, seven tracks a page.
- Player: MP3, FLAC, Ogg Vorbis, WAV, M4A and AAC through the Kindle's `mplay`, to connected Bluetooth headphones.
- The player runs apart from the app: Exit leaves the app and the music keeps playing. Reopening Music shows what is
  playing. Stop ends playback and saves the place.
- Pause, resume, previous, next, and seeking by tapping the progress line. Sleep pauses while the app is open.
- A sound row on the player (− and +, or a tap on the eight segments) while headphones are connected. It sets the Kindle's sound level, the same as quick settings.
- The app icon has its own light and dark forms, without a border of its own (the launcher draws the outline).
- Headphones are read from the Bluetooth connected list; when they leave, playback pauses with a message.
