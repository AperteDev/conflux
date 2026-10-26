#!/usr/bin/env python3
"""Drive the app in the nested shell of scripts/gui-shell.sh, through the shell's
own bus (Eval needs its --unsafe-mode):

    gui-drive.py click X Y        a left click at a point of the 1400x1100 screen
    gui-drive.py scroll X Y N     N wheel steps there; negative scrolls up
    gui-drive.py type TEXT        type ASCII text
    gui-drive.py key NAME         Return, Escape, Tab or ctrl+a
    gui-drive.py shot FILE        a screenshot, as PNG
    gui-drive.py eval JS          run JavaScript in the shell
"""

import pathlib
import subprocess
import sys

BUS = (pathlib.Path(__file__).resolve().parent.parent / "target/gui-shell/bus").read_text().strip()

# A virtual pointer and keyboard, made once per shell.
SETUP = """
const Clutter = imports.gi.Clutter;
const seat = global.stage.context.get_backend().get_default_seat();
global._vp ??= seat.create_virtual_device(Clutter.InputDeviceType.POINTER_DEVICE);
global._vk ??= seat.create_virtual_device(Clutter.InputDeviceType.KEYBOARD_DEVICE);
const t = () => global.get_current_time() * 1000;
const key = k => {
  global._vk.notify_keyval(t(), k, Clutter.KeyState.PRESSED);
  global._vk.notify_keyval(t(), k, Clutter.KeyState.RELEASED);
};
"""

KEYS = {"Return": 0xFF0D, "Escape": 0xFF1B, "Tab": 0xFF09}


def call(dest, path, method, *args):
    out = subprocess.run(
        ["gdbus", "call", "--address", BUS, "--dest", dest, "--object-path", path,
         "--method", method, *args],
        capture_output=True, text=True,
    )
    return out.stdout.strip() or out.stderr.strip()


def run(js):
    return call("org.gnome.Shell", "/org/gnome/Shell", "org.gnome.Shell.Eval", SETUP + js)


def main():
    cmd, args = sys.argv[1], sys.argv[2:]
    if cmd == "click":
        x, y = args
        print(run(f"""global._vp.notify_absolute_motion(t(), {x}, {y});
            global._vp.notify_button(t(), Clutter.BUTTON_PRIMARY, Clutter.ButtonState.PRESSED);
            global._vp.notify_button(t(), Clutter.BUTTON_PRIMARY, Clutter.ButtonState.RELEASED);
            true"""))
    elif cmd == "scroll":
        x, y, n = args[0], args[1], int(args[2])
        step = "DOWN" if n > 0 else "UP"
        print(run(f"global._vp.notify_absolute_motion(t(), {x}, {y});" + "".join(
            f"global._vp.notify_discrete_scroll(t(), Clutter.ScrollDirection.{step},"
            " Clutter.ScrollSource.WHEEL);" for _ in range(abs(n))) + "true"))
    elif cmd == "type":
        print(run("".join(f"key({ord(c)});" for c in args[0]) + "true"))
    elif cmd == "key" and args[0] == "ctrl+a":
        print(run("""global._vk.notify_keyval(t(), 0xffe3, Clutter.KeyState.PRESSED);
            key(0x61);
            global._vk.notify_keyval(t(), 0xffe3, Clutter.KeyState.RELEASED); true"""))
    elif cmd == "key":
        print(run(f"key({KEYS[args[0]]}); true"))
    elif cmd == "shot":
        print(call("org.gnome.Shell.Screenshot", "/org/gnome/Shell/Screenshot",
                   "org.gnome.Shell.Screenshot.Screenshot", "false", "false",
                   str(pathlib.Path(args[0]).resolve())))
    elif cmd == "eval":
        print(run(args[0]))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
