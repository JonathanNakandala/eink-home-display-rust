"""Writes the shared test vectors that can be worked out without either implementation (testdata/contract/).

The display (C++) and the server (Rust) each have to read what the other writes, and each is tested alone, so a change to one
that the other does not follow would pass both test suites. These vectors are the common ground: both sides' tests read the
same files. The pairing codes and the base64 are computed here from the specification, a third implementation that is
neither of the two, so a mistake the two share cannot hide.

  python3 esphome/scripts/make_vectors.py

The report, constants and request vectors are written by hand (see testdata/contract/README.md); this only writes the ones a
program can work out. Uses only the standard library.
"""

import base64
import hashlib
from pathlib import Path

OUT = Path(__file__).resolve().parents[2] / "testdata/contract"
ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
LABEL = b"home-display pairing code v1"


def pairing_code(root: bytes, name: str, spki: bytes) -> str:
    """The code the display shows and the server compares: README 'Secure transport', 'The code'."""
    hash_ = hashlib.sha256()
    hash_.update(LABEL)
    hash_.update(hashlib.sha256(root).digest())
    for part in (name.encode(), spki):
        hash_.update(len(part).to_bytes(8, "big"))
        hash_.update(part)
    number = int.from_bytes(hash_.digest()[:8], "big") >> (64 - 60)
    characters = "".join(ALPHABET[(number >> (5 * (11 - i))) & 31] for i in range(12))
    return "-".join(characters[i : i + 4] for i in range(0, 12, 4))


def pattern(n: int, step: int = 1) -> bytes:
    return bytes((i * step) % 256 for i in range(n))


def write_pairing() -> None:
    cases = [
        (
            "the_example_in_the_documentation",
            bytes([0x30, 0x03, 0x02, 0x01, 0x01]),
            "reterminal-e1003-a1b2c3",
            pattern(91),
        ),
        ("a_root_with_nothing_in_it", b"", "kitchen", pattern(91)),
        ("the_shortest_name", pattern(40), "a", pattern(91)),
        ("the_longest_name", pattern(40), "n" * 32, pattern(91)),
        ("dots_hyphens_and_underscores", pattern(40), "hall_2.upstairs-east", pattern(91)),
        ("the_same_name_in_another_case_is_another_code", pattern(40), "Kitchen", pattern(91)),
        ("the_same_name_in_lower_case", pattern(40), "kitchen", pattern(91)),
        ("a_big_root", pattern(1500, 7), "reterminal-e1003-a1b2c3", pattern(91, 3)),
        ("a_key_of_all_ones", pattern(40), "reterminal-e1003-a1b2c3", b"\xff" * 91),
        ("a_key_with_nothing_in_it", pattern(40), "reterminal-e1003-a1b2c3", b""),
    ]
    lines = [
        "# The pairing code (README 'Secure transport'): worked out by esphome/scripts/make_vectors.py from the specification.",
        "# root: the authority certificate's DER (any bytes: only its hash is used). spki: the display's public key DER.",
        "# The display (core/pairing.h) and the server (domain/models/pairing.rs) must each give `code`.",
        "",
    ]
    for name, root, device, spki in cases:
        lines += [
            f"[{name}]",
            f"root: {root.hex()}",
            f"name: {device}",
            f"spki: {spki.hex()}",
            f"code: {pairing_code(root, device, spki)}",
            "",
        ]
    (OUT / "pairing_code.vectors").write_text("\n".join(lines))


def write_base64() -> None:
    samples = [
        b"",
        b"\x00",
        b"\xff",
        b"\x00\x00",
        b"Ma",
        b"Man",
        b"\xfb\xff\xbf",
        pattern(91),
        pattern(256),
        pattern(1000, 5),
    ]
    lines = [
        "# Base64 (RFC 4648, standard alphabet with padding) as EST puts it on the wire. Worked out by make_vectors.py.",
        "# Both sides encode `bytes` to `text` and decode `text` back to `bytes`.",
        "",
    ]
    for index, sample in enumerate(samples):
        lines += [
            f"[{index}_{len(sample)}_bytes]",
            f"bytes: {sample.hex()}",
            f"text: {base64.b64encode(sample).decode()}",
            "",
        ]
    (OUT / "base64.vectors").write_text("\n".join(lines))


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    write_pairing()
    write_base64()


if __name__ == "__main__":
    main()
