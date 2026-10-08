"""Writes .vscode/c_cpp_properties.json so VS Code's C/C++ extension can read the firmware's headers.

Not part of the build. The headers include ESPHome's and ESP-IDF's, which are fetched into the build directory and
a cache outside the repository when the firmware is compiled, so their paths are on the machine and not in the
repository. This reads them from the compiler command ESPHome's build recorded for main.cpp, so what the editor
sees is what the compiler saw. Run it through `make -C esphome vscode` after a compile; it uses only the standard
library.
"""

import json
import shlex
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ESPHOME = ROOT / "esphome"
COMMANDS = ESPHOME / ".esphome/build/reterminal-e1003/build/compile_commands.json"
OUTPUT = ROOT / ".vscode/c_cpp_properties.json"


def main_command() -> list[str]:
    if not COMMANDS.exists():
        sys.exit(f"{COMMANDS.relative_to(ROOT)} not found: run `make -C esphome compile` first.")
    for entry in json.loads(COMMANDS.read_text()):
        if entry["file"].endswith("src/main.cpp"):
            return shlex.split(entry["command"])
    sys.exit("main.cpp is not in compile_commands.json")


def flag_values(command: list[str], prefix: str) -> list[str]:
    return [arg[len(prefix) :] for arg in command if arg.startswith(prefix)]


def portable(path: str) -> str:
    """`path` with the repository and the home directory written as VS Code variables, which it expands itself,
    so the file does not hold this machine's user name or where the repository is checked out."""
    for prefix, variable in ((str(ROOT), "${workspaceFolder}"), (str(Path.home()), "${env:HOME}")):
        if path == prefix or path.startswith(prefix + "/"):
            return variable + path[len(prefix) :]
    return path


def standard(command: list[str], default: str) -> str:
    for arg in command:
        if arg.startswith("-std="):
            return arg[len("-std=") :].replace("gnu++", "c++").replace("gnu", "c")
    return default


def main() -> None:
    command = main_command()
    compiler = portable(command[0])
    # A compiler ignores an -I directory that is not there, and ESP-IDF names a few that do not exist for this
    # chip; VS Code warns "Cannot find" for each, so they are left out.
    includes = [portable(path) for path in flag_values(command, "-I") if Path(path).is_dir()]
    defines = flag_values(command, "-D")
    firmware = {
        "name": "ESPHome firmware",
        # The headers in esphome/ are copied into the build's src/ when compiling, so the workspace copies get the
        # same include paths, plus their own directory.
        "includePath": ["${workspaceFolder}/esphome"] + includes,
        "defines": defines,
        "compilerPath": compiler,
        "cStandard": "c17",
        "cppStandard": standard(command, "c++20"),
        "intelliSenseMode": "linux-gcc-x86",
    }
    host = {
        "name": "Host tests",
        # tests/ and the pure headers build with the computer's own compiler (see esphome/Makefile).
        "includePath": ["${workspaceFolder}/esphome"],
        "defines": [],
        "compilerPath": "/usr/bin/clang",
        "cStandard": "c17",
        "cppStandard": "c++17",
        "intelliSenseMode": "macos-clang-arm64",
    }
    OUTPUT.parent.mkdir(exist_ok=True)
    OUTPUT.write_text(json.dumps({"configurations": [firmware, host], "version": 4}, indent=4) + "\n")
    print(f"Wrote {OUTPUT.relative_to(ROOT)}: {len(includes)} include paths, compiler {compiler}")


if __name__ == "__main__":
    main()
