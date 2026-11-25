#!/bin/sh
# stage.sh TARGET [STAGE_DIR]
set -eu
cd "$(dirname "$0")/.."
case "${1:-armhf}" in
    armhf|armv7-unknown-linux-musleabihf) target=armv7-unknown-linux-musleabihf ;;
    *) echo "Unsupported package architecture: $1" >&2; exit 1 ;;
esac
stage=${2:-build/stage}
target_dir=${CARGO_TARGET_DIR:-target}
bin=$target_dir/$target/release/music-kindle
[ -x "$bin" ] || { echo "Build $bin before staging." >&2; exit 1; }
install -d "$stage/music/bin" "$stage/documents"
install -m 755 "$bin" "$stage/music/bin/music-kindle"
install -m 755 deploy/Music.sh "$stage/documents/Music.sh"
