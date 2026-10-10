# Contract vectors

What the display (C++, `esphome/home_display/`) and the server (Rust, `src/`) must agree on, as data both sides' tests read.

Each side is tested alone, so a change to one that the other does not follow would pass both suites. These files are the common
ground: the C++ tests (`esphome/tests/core/contract_test.cpp`) and the Rust tests (`src/contract_tests.rs`) read the same
bytes, and a vector that one side reads differently fails there.

| File | What it pins | Made by |
|---|---|---|
| `constants.vectors` | the server's certificate name, the RFC 9266 label and length, the pairing code's alphabet and label, the failure names | by hand |
| `pairing_code.vectors` | root, name, key -> the code the display shows and the server compares | `esphome/scripts/make_vectors.py`, from the specification |
| `base64.vectors` | bytes <-> the base64 EST puts on the wire | `make_vectors.py` |
| `report.vectors` | what the display writes in its check-in, and what the server must read from it | by hand |
| `csr.vectors` | the certificate request the display builds (with the profile it sends: model, panel, formats, firmware), and that the server reads and verifies, including profiles it must leave out | `make_vectors.py --csr`, below |

## Format

```
# a comment
[the name of a case]
key: value
```

One line per key, values on one line, bytes as lower-case hex. A key can repeat. Nothing else: both sides have a parser of a
few lines (`esphome/tests/support/vectors.h`, `src/contract_tests.rs`).

## Adding a case

Add it to the file; both sides' tests pick it up. For the generated files, add it to `make_vectors.py` and run it. If the two
sides disagree about a case, one of them is wrong (or the specification is ambiguous): work out which, and fix that side, not
the vector, unless the vector is what is wrong.

## The request vectors

`python3 esphome/scripts/make_vectors.py --csr` makes them, with a DER encoder of its own (neither the firmware's nor the server's)
and a real ECDSA P-256 / SHA-256 signature from OpenSSL, on a key made for the run and thrown away (nothing is protected by it).
It writes new signatures each time, so the file changes whenever it is run: do it when a case is added or the format changes.

Eleven cases. Five are requests the display builds itself (`display_builds: yes`), with and without the channel binding and with and
without a profile: the display's tests build them from the same inputs and must get the same bytes. Six carry a profile the server
must leave out (given twice, a newer version, not DER, text it will not show, a part missing, an impossible panel): only the server's
tests read those, and must still accept the request and its signature, with no profile.
