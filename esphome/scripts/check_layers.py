"""Fails if a header under home_display/ includes from a layer it must not know about.

The layers (see the README): core/ is pure logic and the interfaces it needs, tls/ is what is built on mbedTLS and
sockets, esp/ is what needs ESPHome or ESP-IDF. Dependencies point inward only: core includes core, tls includes core and
tls, esp includes all three. core is also held to the platform: nothing from mbedTLS, sockets, ESPHome or ESP-IDF, which is
what lets it be built and tested with a bare C++ compiler. Run through `make -C esphome lint`; it uses only the standard
library.
"""

import re
import sys
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parents[1] / "home_display"

MAY_INCLUDE = {"core": {"core"}, "tls": {"core", "tls"}, "esp": {"core", "tls", "esp"}}

# What the platform's headers look like, for the layers that may not use them.
PLATFORM = {
    "core": re.compile(r'#include [<"](mbedtls/|esphome/|esp_|lwip/|nvs|sys/|netinet/|arpa/|fcntl\.h|unistd\.h)'),
    "tls": re.compile(r'#include [<"](esphome/|esp_|lwip/|nvs)'),
}
INCLUDE = re.compile(r'#include "home_display/([a-z]+)/')


def main() -> int:
    problems = []
    for layer, allowed in MAY_INCLUDE.items():
        for header in sorted((ROOT_DIR / layer).glob("*.h")):
            for number, line in enumerate(header.read_text().splitlines(), 1):
                found = INCLUDE.search(line)
                if found and found.group(1) not in allowed:
                    problems.append(
                        f"{header.relative_to(ROOT_DIR.parent)}:{number}: {layer}/ must not include {found.group(1)}/"
                    )
                if layer in PLATFORM and PLATFORM[layer].search(line):
                    problems.append(
                        f"{header.relative_to(ROOT_DIR.parent)}:{number}: {layer}/ must not use the platform: {line.strip()}"
                    )
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
