"""Makes the display fetch and draw a fresh picture with nobody at it, and prints what the wake did.

Not part of the build: a helper for trying a change on a display that is plugged in. In order:

1. asks the server to render now (`POST /refresh`, which is what the display's own button asks for), so the wake finds a new
   picture to download;
2. resets the board over its serial cable, by pulsing RTS the way esptool does, which starts a wake;
3. prints the board's log, cut down to the lines that say what happened, until the wake ends.

It is not a press of KEY0. A reset is a power-on boot, so the wake does not draw "Refreshing..." and does not ask the server to
render for itself (step 1 does that); pressing the button wakes the chip from deep sleep, which a serial cable cannot do. What it
does exercise is the download, the decode, the draw and the sleep.

The serial port can be open in one program at a time, so stop `esphome logs` first. The server's plain HTTP side has to be served
(`transport = "http"` or `"prefer-https"`), since the display's routes on the HTTPS side want its certificate.

Run it through `make -C esphome refresh` (extra arguments in `REFRESH_ARGS`, e.g. `REFRESH_ARGS=--all`); it needs pyserial, which
ESPHome installs. Exits 0 if the wake ended in sleep, 2 if it failed or the chip reset itself, 3 if it did not end in time, and 1
if it could not do its job (no server, no port).
"""

import argparse
import re
import sys
import time
import urllib.error
import urllib.request

import serial
from serial.tools import list_ports

# The USB-serial chip on the reTerminal is a WCH CH340, whose vendor id this is: it is how the board's port is found on a
# computer that has more than one.
BRIDGE_VENDOR_ID = 0x1A86

ANSI = re.compile(r"\x1b\[[0-9;]*m")
# The lines worth reading in a wake: what started it, the server, the picture, and how it ended.
INTERESTING = re.compile(
    r"rst:0x|\]\[(W|E)\]|Woken|Found|No image server|No usable|Image (changed|downloaded|unchanged|fully)|Decoding complete|Failure|CRASH|"
    r"Sleeping for|Update took|efreshing|V, \d+%|didn't finish"
)
# The wake is over when the board sleeps, gives up (the backoff line), or the chip resets itself: a second boot, after the
# one the script caused, is a watchdog or a crash. (The crash report is no use for this: it can describe an older crash.)
SLEEPS = re.compile(r"Sleeping for")
FAILED = re.compile(r"Failure \d+ in a row")
BOOT = re.compile(r"rst:0x")
# How long to keep reading after the wake has ended, so the lines just after it are shown.
AFTERWARDS_S = 1.5


def find_port() -> str:
    """The board's serial port: the one USB-serial bridge of its kind that is plugged in."""
    found = [port.device for port in list_ports.comports() if port.vid == BRIDGE_VENDOR_ID]
    if len(found) == 1:
        return found[0]
    if not found:
        sys.exit("No serial port with a CH340 on it was found: is the board plugged in? Or name the port with --port.")
    sys.exit(f"More than one serial port has a CH340 on it ({', '.join(found)}): name the board's with --port.")


def render_now(server: str, device: str | None) -> None:
    """Asks the server to render now and says what it answered. The server is its own judge of whether one just ran."""
    url = f"{server}/refresh" + (f"?device={device}" if device else "")
    started = time.monotonic()
    try:
        with urllib.request.urlopen(urllib.request.Request(url, method="POST"), timeout=60) as reply:
            body = reply.read().decode("utf-8", "replace")
    except urllib.error.URLError as error:
        sys.exit(f"Could not ask the server to render ({url}): {error}")
    print(f"server: answered in {time.monotonic() - started:.1f} s: {body[:160]}")


def open_port(name: str) -> serial.Serial:
    """Opens the port with DTR and RTS held off, so that opening it does not reset the board on its own."""
    port = serial.Serial()
    port.port = name
    port.baudrate = 115200
    port.timeout = 0.25
    port.dtr = False
    port.rts = False
    try:
        port.open()
    except serial.SerialException as error:
        sys.exit(
            f"Could not open {name}: {error}\nIs it the right port, and is `esphome logs` (or anything else) using it?"
        )
    return port


def reset(port: serial.Serial) -> None:
    """Resets the chip as esptool does: RTS pulls EN low, with DTR off so that it boots normally and not into the loader."""
    port.dtr = False
    port.rts = True
    time.sleep(0.1)
    port.rts = False


def follow(port: serial.Serial, show_all: bool, timeout: float) -> int:
    """Prints the log until the wake ends, and returns the exit status (see the top of the file)."""
    started = time.monotonic()
    ended_at: float | None = None
    outcome = 3
    boots = 0
    pending = b""
    while time.monotonic() - started < timeout:
        if ended_at is not None and time.monotonic() > ended_at:
            return outcome
        pending += port.read(4096)
        *lines, pending = pending.split(b"\n")
        for raw in lines:
            line = ANSI.sub("", raw.decode("utf-8", "replace")).rstrip()
            if show_all or INTERESTING.search(line):
                print(f"{time.monotonic() - started:6.1f}s  {line}")
            if BOOT.search(line):
                boots += 1
            # A failure outranks the sleep that follows it: the board sleeps after a failed wake too.
            if FAILED.search(line) or boots > 1:
                outcome = 2
                ended_at = ended_at or time.monotonic() + AFTERWARDS_S
            elif SLEEPS.search(line):
                outcome = 2 if outcome == 2 else 0
                ended_at = ended_at or time.monotonic() + AFTERWARDS_S
    print(f"\nThe wake did not end within {timeout:.0f} s.")
    return outcome


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", help="the board's serial port (default: the one with a CH340 on it)")
    parser.add_argument("--server", default="http://127.0.0.1:8080", help="the server (default %(default)s)")
    parser.add_argument(
        "--device", help="ask for the render as this display (its name, as `displayctl displays list` shows it)"
    )
    parser.add_argument("--no-render", action="store_true", help="do not ask the server to render first")
    parser.add_argument("--all", action="store_true", help="show every line of the log, not just the ones that matter")
    parser.add_argument(
        "--timeout", type=float, default=200, help="seconds to wait for the wake to end (default %(default)s)"
    )
    args = parser.parse_args()

    name = args.port or find_port()
    if not args.no_render:
        render_now(args.server, args.device)
    port = open_port(name)
    try:
        print(f"board: resetting over {name}")
        reset(port)
        sys.exit(follow(port, args.all, args.timeout))
    finally:
        port.close()


if __name__ == "__main__":
    main()
