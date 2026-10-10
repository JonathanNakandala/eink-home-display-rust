"""Writes the shared test vectors that can be worked out without either implementation (testdata/contract/).

The display (C++) and the server (Rust) each have to read what the other writes, and each is tested alone, so a change to one
that the other does not follow would pass both test suites. These vectors are the common ground: both sides' tests read the
same files. The pairing codes and the base64 are computed here from the specification, a third implementation that is
neither of the two, so a mistake the two share cannot hide.

  python3 esphome/scripts/make_vectors.py          # the pairing codes and the base64: deterministic
  python3 esphome/scripts/make_vectors.py --csr    # the certificate requests: needs `openssl`, and new signatures each time

The report and constants vectors are written by hand (see testdata/contract/README.md). The requests are built here too, by a
DER encoder written for the purpose: a third implementation, neither the firmware's nor the server's, and signed with OpenSSL.
Uses only the standard library, and the `openssl` program for --csr.
"""

import base64
import hashlib
import subprocess
import sys
import tempfile
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


# ---- certificate requests ------------------------------------------------------------------------------------------------


def length(n: int) -> bytes:
    if n < 128:
        return bytes([n])
    body = n.to_bytes((n.bit_length() + 7) // 8, "big")
    return bytes([0x80 | len(body)]) + body


def tlv(tag: int, content: bytes) -> bytes:
    return bytes([tag]) + length(len(content)) + content


def integer(n: int) -> bytes:
    body = n.to_bytes(max(1, (n.bit_length() + 7) // 8), "big")
    return tlv(0x02, (b"\x00" + body) if body[0] >= 0x80 else body)


def utf8(text: str) -> bytes:
    return tlv(0x0C, text.encode())


OID_COMMON_NAME = bytes.fromhex("0603550403")
OID_CHALLENGE_PASSWORD = bytes.fromhex("06092a864886f70d010907")
OID_EXTENSION_REQUEST = bytes.fromhex("06092a864886f70d01090e")
OID_PROFILE = bytes.fromhex(
    "06146981a6fddcedfd99c281b7889cc98dbbae9dfd77"
)  # 2.25.110978727574289354506863863824604692215
ECDSA_SHA256 = bytes.fromhex("300a06082a8648ce3d040302")


def profile_der(
    model: str, firmware: str, width: int, height: int, levels: int, formats: list[str], version: int = 1
) -> bytes:
    """ProfileV1: SEQUENCE { version, model, width, height, levels, SEQUENCE OF UTF8String formats, firmware }."""
    return tlv(
        0x30,
        integer(version)
        + utf8(model)
        + integer(width)
        + integer(height)
        + integer(levels)
        + tlv(0x30, b"".join(utf8(f) for f in formats))
        + utf8(firmware),
    )


def extension_request(extensions: list[bytes]) -> bytes:
    """extensionRequest { SEQUENCE OF Extension { SEQUENCE { extnID, extnValue OCTET STRING } } }, each value given as DER."""
    return tlv(
        0x30,
        OID_EXTENSION_REQUEST
        + tlv(0x31, tlv(0x30, b"".join(tlv(0x30, OID_PROFILE + tlv(0x04, v)) for v in extensions))),
    )


def request_info(name: str, spki: bytes, challenge: str, extensions: list[bytes]) -> bytes:
    attributes = b""
    if challenge:
        attributes += tlv(0x30, OID_CHALLENGE_PASSWORD + tlv(0x31, utf8(challenge)))
    if extensions:
        attributes += extension_request(extensions)
    subject = tlv(0x30, tlv(0x31, tlv(0x30, OID_COMMON_NAME + utf8(name))))
    return tlv(0x30, integer(0) + subject + spki + tlv(0xA0, attributes))


def write_csr() -> None:
    reterminal = ("reTerminal E1003", "0.2.0", 1872, 1404, 16, ["bmp", "png", "qoi"])
    waveshare = ("Waveshare 7.5", "0.1.0", 800, 480, 16, ["png"])
    binding = base64.b64encode(bytes(0xA0 + i for i in range(32))).decode()
    other = base64.b64encode(b"\x5a" * 32).decode()
    # (id, name, challenge, extension values, profile it carries as the server must read it or None, the display builds it)
    cases = [
        ("with_binding", "reterminal-e1003-a1b2c3", binding, [], None, True),
        ("without_binding", "kitchen", "", [], None, True),
        ("shortest_name", "a", other, [], None, True),
        ("with_binding_and_profile", "reterminal-e1003-a1b2c3", binding, [profile_der(*reterminal)], reterminal, True),
        ("profile_without_binding", "kitchen", "", [profile_der(*waveshare)], waveshare, True),
        # What a display must never be kept from joining by: a profile the server cannot use is left out, and the request is read.
        ("profile_given_twice", "kitchen", "", [profile_der(*waveshare)] * 2, None, False),
        ("profile_of_a_newer_version", "kitchen", "", [profile_der(*waveshare, version=2)], None, False),
        ("profile_that_is_not_der", "kitchen", "", [b"not der at all"], None, False),
        (
            "profile_with_unusable_text",
            "kitchen",
            "",
            [profile_der("<script>", "0.1.0", 800, 480, 16, ["png"])],
            None,
            False,
        ),
        ("profile_with_a_part_missing", "kitchen", "", [tlv(0x30, integer(1) + utf8("m") + integer(800))], None, False),
        (
            "profile_of_an_impossible_panel",
            "kitchen",
            "",
            [profile_der("m", "0.1.0", 0, 480, 16, ["png"])],
            None,
            False,
        ),
    ]
    with tempfile.TemporaryDirectory() as work:
        key = Path(work) / "key.pem"
        subprocess.run(
            ["openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", str(key)],
            check=True,
            capture_output=True,
        )
        spki = subprocess.run(
            ["openssl", "ec", "-in", str(key), "-pubout", "-outform", "DER"], check=True, capture_output=True
        ).stdout
        lines = [
            "# The certificate request (core/der.h builds it, adapters/certificate_authority/request.rs reads it). Made by",
            "# esphome/scripts/make_vectors.py --csr, with a DER encoder of its own and a real ECDSA P-256 / SHA-256 signature (OpenSSL)",
            "# from a key made for the run and thrown away; nothing is protected by it.",
            "#   tbs: the part that is signed. request: the finished request. spki: the key. challenge: the channel binding as base64.",
            "#   profile_der: what the display says it is, as DER (core/profile.h). profile: what the server must read from it, as",
            "#     model|firmware|width|height|levels|formats, or none (it was not sent, or the server cannot use it).",
            "#   display_builds: yes if the display produces this request itself; no for a request only the server need read.",
            "# The display must build `profile_der`, `tbs` and `request` byte for byte (display_builds: yes). The server must read the name,",
            "# the key, the binding and the profile as given, and accept the signature, for every case.",
            "",
        ]
        for case_id, name, challenge, extensions, profile, display_builds in cases:
            info = request_info(name, spki, challenge, extensions)
            sig = subprocess.run(
                ["openssl", "dgst", "-sha256", "-sign", str(key)], input=info, check=True, capture_output=True
            ).stdout
            request = tlv(0x30, info + ECDSA_SHA256 + tlv(0x03, b"\x00" + sig))
            shown = (
                "none"
                if profile is None
                else "|".join(
                    [profile[0], profile[1], str(profile[2]), str(profile[3]), str(profile[4]), ",".join(profile[5])]
                )
            )
            lines += [
                f"[{case_id}]",
                f"name: {name}",
                f"spki: {spki.hex()}",
                f"challenge: {challenge or 'none'}",
                f"profile_der: {extensions[0].hex() if display_builds and extensions else 'none'}",
                f"display_builds: {'yes' if display_builds else 'no'}",
                f"tbs: {info.hex()}",
                f"signature: {sig.hex()}",
                f"request: {request.hex()}",
                f"binding: {base64.b64decode(challenge).hex() if challenge else 'none'}",
                f"profile: {shown}",
                "",
            ]
    (OUT / "csr.vectors").write_text("\n".join(lines))


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    write_pairing()
    write_base64()
    if "--csr" in sys.argv[1:]:
        write_csr()


if __name__ == "__main__":
    main()
