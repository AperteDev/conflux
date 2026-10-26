#!/bin/sh
# Show the GTK app in a headless nested GNOME Shell, for testing it without a
# screen: `scripts/gui-drive.py` clicks, types and takes screenshots there.
#
#   scripts/gui-shell.sh [scratch dir]
#
# The shell runs on its own session bus, which only gui-drive.py talks to; the
# app stays on the host's bus, so it sees the user's real systemd units: test
# with a scratch config, and leave the switches of real units alone. With a
# scratch dir (holding config.toml, run/ and state/, as for a scratch daemon),
# the app reads that config and those daemons. Ctrl+C stops it all.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
work=$root/target/gui-shell
display=conflux-gui-shell

if [ -z "${GUI_SHELL_BUS:-}" ]; then
  mkdir -p "$work"
  HOST_BUS=$DBUS_SESSION_BUS_ADDRESS GUI_SHELL_BUS=1 exec dbus-run-session -- "$0" "$@"
fi
# Where gui-drive.py finds the shell.
echo "$DBUS_SESSION_BUS_ADDRESS" > "$work/bus"

# Settings in memory: a fresh GNOME each run, nothing written to the user's dconf.
export GSETTINGS_BACKEND=memory
rm -f "$XDG_RUNTIME_DIR/$display" "$XDG_RUNTIME_DIR/$display.lock"
gnome-shell --headless --unsafe-mode --wayland --wayland-display=$display \
  --virtual-monitor 1400x1100 > "$work/shell.log" 2>&1 &
shell=$!
trap 'kill $shell 2>/dev/null' EXIT INT TERM
gdbus wait --session --timeout 60 org.gnome.Shell.Screenshot

cargo build --quiet --manifest-path "$root/Cargo.toml" -p conflux-gtk
# An absolute display: a scratch XDG_RUNTIME_DIR would hide the shell's socket.
# No accessibility bus here, so none for the app either.
set -- "${1:-}"
if [ -n "$1" ]; then
  scratch=$(cd "$1" && pwd)
  set -- env CONFLUX_CONFIG="$scratch/config.toml" XDG_RUNTIME_DIR="$scratch/run" \
    XDG_STATE_HOME="$scratch/state"
else
  set -- env
fi
"$@" WAYLAND_DISPLAY="$XDG_RUNTIME_DIR/$display" GDK_BACKEND=wayland GTK_A11Y=none \
  DBUS_SESSION_BUS_ADDRESS="$HOST_BUS" "$root/target/debug/conflux-gtk" \
  > "$work/app.log" 2>&1 &

# The app maximized, the overview closed, and no hot corner for the pointer to
# hit on its way in from the top left.
sleep 2
"$root/scripts/gui-drive.py" eval '
  Main.layoutManager.hotCorners.forEach(h => h?.destroy());
  Main.layoutManager.hotCorners = [];
  Main.overview.hide();
  const w = global.get_window_actors().map(a => a.meta_window)
    .find(w => w.get_wm_class()?.startsWith("dev.aperte.Conflux"));
  if (w) { try { w.maximize(); } catch { w.maximize(imports.gi.Meta.MaximizeFlags.BOTH); } }
  !!w' > /dev/null
echo "Ready: scripts/gui-drive.py shot target/gui-shell/shot.png"
wait $shell
