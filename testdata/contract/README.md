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
| `csr.vectors` | the certificate request the display builds, and that the server reads and verifies | OpenSSL, below |

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

A real signature needs a key; this one was made once and thrown away (nothing is protected by it):

```sh
openssl ecparam -name prime256v1 -genkey -noout -out key.pem
openssl ec -in key.pem -pubout -outform DER -out spki.der            # 91 bytes: the `spki`
# build `tbs` for each case with home_display_der::request_info (core/der.h), then sign it:
openssl dgst -sha256 -sign key.pem -out case.sig case.tbs
# and assemble: home_display_der::request(info, signature)
```

The `tbs` and `request` the firmware builds today are what is in the file, so the firmware test is a regression test of its
encoder; the server's test is the independent half: its X.509 parser and signature check read what the firmware wrote.
