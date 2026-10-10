//! The server's half of the contract with the display: `testdata/contract/` holds what both must agree on, and the display's
//! tests (`esphome/tests/core/contract_test.cpp`) read the same files. Here, what the server derives and reads is what the
//! vectors say.
//!
//! Each side is tested alone, so a change to one that the other does not follow would pass both suites; these are the check
//! that they agree. The files and their format are described in `testdata/contract/README.md`.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::adapters::certificate_authority::request;
use crate::adapters::est_server::{BINDING_LABEL, BINDING_LEN, SERVER_NAME};
use crate::application::devices::{BatteryState, FIRMWARE_CHARS, FailureReason, RawTelemetry};
use crate::domain::models::device_id::DeviceId;
use crate::domain::models::pairing::{
    ALPHABET, Fingerprint, GROUP, HASH_LABEL, LENGTH, PairingCode, PublicKey,
};

/// One case of a vector file: its name, and its `key: value` lines in order.
struct Case {
    name: String,
    fields: Vec<(String, String)>,
}

impl Case {
    fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn need(&self, key: &str) -> &str {
        self.get(key)
            .unwrap_or_else(|| panic!("case {:?} has no {key}", self.name))
    }
}

/// The cases in `text`, which is a whole vector file. A file with none is a mistake, not a pass.
fn parse(text: &str, file: &str) -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            cases.push(Case {
                name: name.to_owned(),
                fields: Vec::new(),
            });
        } else {
            let (key, value) = line
                .split_once(':')
                .unwrap_or_else(|| panic!("{file}: cannot read the line {line:?}"));
            cases
                .last_mut()
                .unwrap_or_else(|| panic!("{file}: a line before any case"))
                .fields
                .push((key.trim().to_owned(), value.trim().to_owned()));
        }
    }
    assert!(!cases.is_empty(), "{file} has no case");
    cases
}

macro_rules! vectors {
    ($file:literal) => {
        parse(
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/testdata/contract/",
                $file
            )),
            $file,
        )
    };
}

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_pairing_code_is_what_the_vectors_say_for_every_case() {
    for case in vectors!("pairing_code.vectors") {
        let device = DeviceId::parse(case.need("name")).unwrap();
        let key = PublicKey::from_der(from_hex(case.need("spki")));
        let authority = Fingerprint::of(&from_hex(case.need("root")));
        assert_eq!(
            PairingCode::derive(&authority, &device, &key).as_str(),
            case.need("code"),
            "case {}",
            case.name
        );
    }
}

#[test]
fn base64_is_what_the_vectors_say_both_ways() {
    for case in vectors!("base64.vectors") {
        let bytes = from_hex(case.need("bytes"));
        assert_eq!(
            STANDARD.encode(&bytes),
            case.need("text"),
            "case {}",
            case.name
        );
        assert_eq!(
            STANDARD.decode(case.need("text")).unwrap(),
            bytes,
            "case {}",
            case.name
        );
    }
}

#[test]
fn the_certificate_request_the_display_builds_is_read_and_its_signature_accepted() {
    for case in vectors!("csr.vectors") {
        let der = from_hex(case.need("request"));
        let read = request::read(&der)
            .unwrap_or_else(|e| panic!("case {}: the server refused it: {e}", case.name));
        assert_eq!(read.device.as_str(), case.need("name"), "{}", case.name);
        assert_eq!(
            to_hex(read.key.as_der()),
            case.need("spki"),
            "case {}",
            case.name
        );
        let binding = read.channel_binding.as_deref().map(to_hex);
        let expected = match case.need("binding") {
            "none" => None,
            hex => Some(hex.to_owned()),
        };
        assert_eq!(binding, expected, "case {}", case.name);
    }
}

#[test]
fn a_request_changed_after_it_was_signed_is_refused_so_the_vectors_prove_the_signature_is_checked()
{
    for case in vectors!("csr.vectors") {
        let tbs = from_hex(case.need("tbs"));
        let mut der = from_hex(case.need("request"));
        // A byte of the signed part, at the name's last letter: still well formed, no longer what was signed.
        let at = der
            .windows(tbs.len())
            .position(|w| w == tbs.as_slice())
            .expect("the signed part is in the request");
        let name_end = at
            + tbs
                .windows(case.need("name").len())
                .position(|w| w == case.need("name").as_bytes())
                .expect("the name is in the signed part")
            + case.need("name").len()
            - 1;
        der[name_end] = if der[name_end] == b'z' { b'y' } else { b'z' };
        assert!(
            request::read(&der).is_err(),
            "case {}: a changed request was accepted",
            case.name
        );
    }
}

/// The value the server keeps, as the vectors write it: the text, or `none`.
fn shown<T: ToString>(value: Option<T>) -> String {
    value.map_or_else(|| "none".to_owned(), |v| v.to_string())
}

#[test]
fn the_check_in_the_display_writes_is_read_as_the_vectors_say() {
    for case in vectors!("report.vectors") {
        let query = case.need("query");
        let mut raw = RawTelemetry::default();
        let mut device = None;
        for (key, value) in url::form_urlencoded::parse(query.trim_start_matches('&').as_bytes()) {
            let value = value.into_owned();
            match key.as_ref() {
                "device" => device = Some(value),
                "battery_mv" => raw.battery_mv = Some(value),
                "battery_pct" => raw.battery_pct = Some(value),
                "battery_state" => raw.battery_state = Some(value),
                "failed_wakes" => raw.failed_wakes = Some(value),
                "rssi" => raw.rssi = Some(value),
                "last_failure" => raw.last_failure = Some(value),
                "last_wake_s" => raw.last_wake_s = Some(value),
                "last_tls_ms" => raw.last_tls_ms = Some(value),
                "last_heap_min" => raw.last_heap_min = Some(value),
                "fw" => raw.fw = Some(value),
                other => panic!(
                    "case {}: the display wrote {other}, which the server does not read",
                    case.name
                ),
            }
        }
        let device = DeviceId::parse(&device.expect("a report names its display")).unwrap();
        assert_eq!(device.as_str(), case.need("device"), "case {}", case.name);
        let read = raw.parse(device);

        let expect = |key: &str| {
            case.get(&format!("expect_{key}"))
                .unwrap_or("none")
                .to_owned()
        };
        let name = &case.name;
        assert_eq!(shown(read.failed_wakes), expect("failed_wakes"), "{name}");
        assert_eq!(
            shown(read.battery_millivolts),
            expect("battery_mv"),
            "{name}"
        );
        assert_eq!(shown(read.battery_percent), expect("battery_pct"), "{name}");
        assert_eq!(
            shown(read.battery_state.map(BatteryState::as_str)),
            expect("battery_state"),
            "{name}"
        );
        assert_eq!(shown(read.wifi_rssi_dbm), expect("rssi"), "{name}");
        assert_eq!(
            shown(read.last_failure.map(FailureReason::as_str)),
            expect("failure"),
            "{name}"
        );
        assert_eq!(shown(read.last_wake_seconds), expect("wake_s"), "{name}");
        assert_eq!(
            shown(read.last_tls_milliseconds),
            expect("tls_ms"),
            "{name}"
        );
        assert_eq!(
            shown(read.last_heap_min_bytes),
            expect("heap_min"),
            "{name}"
        );
        assert_eq!(shown(read.firmware), expect("firmware"), "{name}");
    }
}

#[test]
fn the_names_and_numbers_both_sides_share_are_what_the_vectors_say() {
    let cases = vectors!("constants.vectors");
    let wire = &cases[0];
    assert_eq!(SERVER_NAME, wire.need("server_name"));
    assert_eq!(BINDING_LABEL, wire.need("binding_label").as_bytes());
    assert_eq!(BINDING_LEN.to_string(), wire.need("binding_length"));

    let code = &cases[1];
    assert_eq!(ALPHABET.as_slice(), code.need("alphabet").as_bytes());
    assert_eq!(LENGTH.to_string(), code.need("characters"));
    assert_eq!(GROUP.to_string(), code.need("group"));
    assert_eq!(HASH_LABEL, code.need("hash_label").as_bytes());

    let report = &cases[2];
    // The failures the server knows, in order, are the ones the display can name, and no other.
    let failures: Vec<&str> = FailureReason::ALL.iter().map(|f| f.as_str()).collect();
    assert_eq!(failures.join(","), report.need("failure_names"));
    let states: Vec<&str> = BatteryState::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(states.join(","), report.need("battery_states"));
    assert_eq!(
        FIRMWARE_CHARS.to_string(),
        report.need("firmware_version_longest")
    );
}
