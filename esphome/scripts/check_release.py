"""Fails if the firmware's release setup is inconsistent, before release-please finds out on GitHub.

The firmware is released by release-please as its own package (.github/release-please/config.json): it changes the version
in esphome/packages/version.yaml, found by the `x-release-please-version` comment on that line, and keeps the version in the
manifest. Checked here: the package is configured and points at that file, the file has the marked line, the version on it
is the manifest's, and the server's package leaves `esphome/` to the firmware's. Run through `make -C esphome lint`; it uses
only the standard library.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CONFIG = ROOT / ".github/release-please/config.json"
MANIFEST = ROOT / ".github/release-please/manifest.json"
VERSION_FILE = "esphome/packages/version.yaml"
FIRMWARE_PATH = "esphome"
MARKED = re.compile(r'^\s*firmware_version:\s*"([^"]+)"\s+#\s*x-release-please-version\s*$', re.MULTILINE)


def problems() -> list[str]:
    found = []
    config = json.loads(CONFIG.read_text())
    manifest = json.loads(MANIFEST.read_text())
    packages = config.get("packages", {})

    firmware = packages.get(FIRMWARE_PATH)
    if firmware is None:
        return [f"{CONFIG.name} has no package for {FIRMWARE_PATH}/"]
    if f"/{VERSION_FILE}" not in firmware.get("extra-files", []):
        found.append(f"the {FIRMWARE_PATH} package does not list /{VERSION_FILE} in extra-files")
    if FIRMWARE_PATH not in packages.get(".", {}).get("exclude-paths", []):
        found.append(f"the server's package (.) does not exclude {FIRMWARE_PATH}, so firmware commits would release it")
    for path, package in packages.items():
        if not package.get("package-name"):
            found.append(f"the package at {path} has no package-name, which names its tags")
    if FIRMWARE_PATH not in manifest:
        found.append(f"{MANIFEST.name} has no version for {FIRMWARE_PATH}")

    marked = MARKED.search((ROOT / VERSION_FILE).read_text())
    if marked is None:
        found.append(f'{VERSION_FILE} has no line `firmware_version: "x.y.z" # x-release-please-version`')
    elif manifest.get(FIRMWARE_PATH) not in (None, marked.group(1)):
        found.append(
            f"{VERSION_FILE} says {marked.group(1)} but the manifest says {manifest[FIRMWARE_PATH]}: release-please keeps "
            "them together, so one was edited by hand"
        )
    return found


def main() -> int:
    found = problems()
    for problem in found:
        print(f"release setup: {problem}", file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
