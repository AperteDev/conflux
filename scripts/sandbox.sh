#!/bin/sh
# Run scratch daemons and the app on your screen, apart from your own config,
# daemons and open app:
#
#   scripts/sandbox.sh [--isolated]
#
# Everything lives in target/sandbox: a user config and daemon, and a scratch
# "system" (system/: config, daemon, and the helper in its test mode, so Unlock
# needs no root). Starter configs syncing a/ with b/ are written the first time;
# edit them freely, delete the dir to start over. The app runs as
# dev.aperte.Conflux.Test.
#
# Plain, the app still sees your real systemd units: leave the switches of
# conflux* units alone. --isolated runs it all in bubblewrap instead: only
# target/sandbox writable, HOME in it, a private session bus and no system bus
# (so no systemd, logind or pkexec), the network shared. Ctrl+C, or closing the
# app, stops it all.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
dir=$root/target/sandbox
sys=$dir/system

if [ -z "${SANDBOX_INNER:-}" ]; then
  cargo build --quiet --manifest-path "$root/Cargo.toml" -p conflux -p conflux-helper -p conflux-gtk
  mkdir -p "$dir/home"
  # An absolute display: the scratch XDG_RUNTIME_DIR would hide the real socket.
  case ${WAYLAND_DISPLAY:-} in
    /* | '') display=${WAYLAND_DISPLAY:-} ;;
    *) display=$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY ;;
  esac
  if [ "${1:-}" != --isolated ]; then
    SANDBOX_INNER=1 WAYLAND_DISPLAY=$display exec "$0"
  fi
  [ -S "$display" ] || { echo "--isolated needs a Wayland session" >&2; exit 1; }
  # /run and /tmp empty: no buses, no systemd, no X11; the display and DNS back in.
  set -- bwrap --ro-bind / / --dev /dev --dev-bind-try /dev/dri /dev/dri --proc /proc \
    --tmpfs /run --tmpfs /tmp \
    --bind "$dir" "$dir" --bind "$display" "$display" \
    --unshare-all --share-net --die-with-parent
  if [ -d /run/systemd/resolve ]; then
    set -- "$@" --ro-bind /run/systemd/resolve /run/systemd/resolve
  fi
  exec "$@" env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_CACHE_HOME -u XDG_STATE_HOME \
    -u DISPLAY SANDBOX_INNER=1 HOME="$dir/home" WAYLAND_DISPLAY="$display" \
    GSETTINGS_BACKEND=memory GDK_DEBUG=no-portals dbus-run-session -- "$0"
fi

# The scratch system in the helper's test layout (config.toml, run/, state/),
# kept by a user-mode daemon whose runtime and state dirs lead there.
mkdir -p "$dir/a" "$dir/b" "$dir/run" "$dir/state" \
  "$sys/a" "$sys/b" "$sys/run" "$sys/state" "$sys/xdg-run" "$sys/xdg-state"
chmod 700 "$dir/run" "$sys/xdg-run"
ln -sfn ../run "$sys/xdg-run/conflux"
ln -sfn ../state "$sys/xdg-state/conflux"
for d in "$dir" "$sys"; do
  [ -e "$d/config.toml" ] || cat > "$d/config.toml" <<EOF
[[sync]]
remote_fs = "$d/b"
local = "$d/a"
scope = "mirror"
trigger = "watch-both"
EOF
done

CONFLUX_CONFIG=$sys/config.toml XDG_RUNTIME_DIR=$sys/xdg-run XDG_STATE_HOME=$sys/xdg-state \
  "$root/target/debug/conflux" daemon &
system=$!
export CONFLUX_CONFIG="$dir/config.toml" XDG_RUNTIME_DIR="$dir/run" XDG_STATE_HOME="$dir/state"
"$root/target/debug/conflux" daemon &
user=$!
trap 'kill $user $system 2>/dev/null' EXIT INT TERM
CONFLUX_HELPER_COMMAND=$root/target/debug/conflux-helper CONFLUX_TEST_SYSTEM_ROOT=$sys \
  "$root/target/debug/conflux-gtk" --gapplication-app-id=dev.aperte.Conflux.Test
