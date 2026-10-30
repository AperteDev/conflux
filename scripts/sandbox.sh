#!/bin/sh
# Run a scratch daemon and the app on your screen, apart from your own config,
# daemons and open app: everything lives in target/sandbox (a starter config
# syncing a/ with b/ is written the first time; edit it freely, delete the dir
# to start over). The app runs as dev.aperte.Conflux.Test. It still sees your
# real systemd units: leave the switches of conflux* units alone. Ctrl+C, or
# closing the app, stops both.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
dir=$root/target/sandbox
mkdir -p "$dir/a" "$dir/b" "$dir/run" "$dir/state"
chmod 700 "$dir/run"
[ -e "$dir/config.toml" ] || cat > "$dir/config.toml" <<EOF
[[sync]]
remote_fs = "$dir/b"
local = "$dir/a"
scope = "mirror"
trigger = "watch-both"
EOF

cargo build --quiet --manifest-path "$root/Cargo.toml" -p conflux -p conflux-helper -p conflux-gtk
# An absolute display: the scratch XDG_RUNTIME_DIR would hide the real socket.
case ${WAYLAND_DISPLAY:-} in
  /* | '') display=${WAYLAND_DISPLAY:-} ;;
  *) display=$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY ;;
esac
export CONFLUX_CONFIG="$dir/config.toml" XDG_RUNTIME_DIR="$dir/run" XDG_STATE_HOME="$dir/state"

"$root/target/debug/conflux" daemon &
daemon=$!
trap 'kill $daemon 2>/dev/null' EXIT INT TERM
WAYLAND_DISPLAY=$display "$root/target/debug/conflux-gtk" --gapplication-app-id=dev.aperte.Conflux.Test
