//! What the server knows about a display that is joining, or has joined, its private certificate
//! authority: who it is, which key it holds, and how far it has got.
//!
//! A display proves it holds a key by signing a certificate request, but that proves nothing about who
//! it is, and the first connection can't be authenticated (the display has no reason yet to trust the
//! server's certificate). So the owner confirms it by hand: the display shows a `PairingCode` on its
//! panel, the owner types it in at the server, and the server accepts only if it computed the same code.
//! The code covers the authority's certificate as the display saw it and the display's public key as the
//! server saw it, so someone in the middle, who must show each side a different one, makes the two
//! codes differ and the approval fails.

use std::fmt;

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::domain::models::device_id::DeviceId;

/// SHA-256 of a certificate's DER encoding: what is shown to compare two certificates by eye.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn of(der: &[u8]) -> Self {
        Self(Sha256::digest(der).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// `AB:CD:..`, the usual way to write one.
impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

/// A public key as `SubjectPublicKeyInfo` DER, the form it has in a certificate request.
#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey(Vec<u8>);

impl PublicKey {
    pub fn from_der(der: Vec<u8>) -> Self {
        Self(der)
    }

    pub fn as_der(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", Fingerprint::of(&self.0))
    }
}

/// How many characters a code has: twelve of Crockford's Base32 (5 bits each) is 60 bits.
///
/// The code is worked out from things an attacker in the middle can see and choose (the root they show,
/// the name, the key), and nothing in it is random or secret, so they can search offline for values that
/// give the code the owner will see. At 40 bits that is minutes to hours on one graphics card; at 60 it
/// is not worth attempting. (A commitment step or a password-authenticated exchange would let the code be
/// shorter; see the notes with the pairing protocol. This is the simple way to be safe.)
const LENGTH: usize = 12;
/// How many bits of the hash the code holds.
const BITS: u32 = 5 * LENGTH as u32;
const GROUP: usize = 4;

/// Crockford's Base32 (<https://www.crockford.com/base32.html>): digits and letters without `I`, `L`,
/// `O` and `U`, so nothing read off a small panel can be taken for something else.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// `characters` (all from `ALPHABET`, `LENGTH` of them) split into dash-separated groups.
fn group(characters: &str) -> String {
    (0..LENGTH)
        .step_by(GROUP)
        .map(|start| &characters[start..start + GROUP])
        .collect::<Vec<_>>()
        .join("-")
}

/// The value of a typed character, reading it as Crockford says to: any case, `I` and `L` as `1`, `O`
/// as `0`. `U` is not a character of the alphabet and is refused.
fn value_of(c: char) -> Option<u8> {
    let c = match c.to_ascii_uppercase() {
        'I' | 'L' => '1',
        'O' => '0',
        other => other,
    };
    let c = u8::try_from(c).ok()?;
    ALPHABET.iter().position(|&a| a == c).map(|v| v as u8)
}

/// The code a display shows and the owner types in, as `JGWP-14YW-3BT0`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PairingCode(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("a pairing code is {LENGTH} letters and digits, like JGWP-14YW-3BT0")]
pub struct InvalidPairingCode;

impl PairingCode {
    /// What the display and the server each compute. It covers the authority certificate, the name the
    /// display claims and its public key, with each part's length so no two combinations give the same
    /// input; the label keeps it from being reused as some other hash. The code is the first 60 bits.
    pub fn derive(authority: &Fingerprint, device: &DeviceId, key: &PublicKey) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"home-display pairing code v1");
        hash.update(authority.as_bytes());
        for part in [device.as_str().as_bytes(), key.as_der()] {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part);
        }
        let digest = hash.finalize();
        let first = u64::from_be_bytes(digest[..8].try_into().expect("a digest has 32 bytes"));
        Self::from_number(first >> (64 - BITS))
    }

    /// The low `BITS` bits of `number` as characters, most significant first, in groups.
    fn from_number(number: u64) -> Self {
        let characters: String = (0..LENGTH)
            .map(|i| ALPHABET[((number >> (5 * (LENGTH - 1 - i))) & 31) as usize] as char)
            .collect();
        Self(group(&characters))
    }

    /// A code as typed: dashes and spaces between the characters or none, any case, and the look-alikes
    /// (`I`, `L`, `O`) read as the digits they resemble.
    pub fn parse(text: &str) -> Result<Self, InvalidPairingCode> {
        let values: Option<Vec<u8>> = text
            .chars()
            .filter(|c| !matches!(c, '-' | ' '))
            .map(value_of)
            .collect();
        let values = values
            .filter(|v| v.len() == LENGTH)
            .ok_or(InvalidPairingCode)?;
        let characters: String = values
            .iter()
            .map(|&v| ALPHABET[v as usize] as char)
            .collect();
        Ok(Self(group(&characters)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Never printed by `{:?}`. The code is meant to be read off the display's own panel and nowhere else, and a
/// derived `Debug` on anything that holds one (a record, an answer, a log line of either) would print it. `Display`
/// still shows it, for the few places that mean to: the tests, and the example for whoever writes the firmware.
impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingCode(<hidden>)")
    }
}

/// Where a display is in joining. Each step is the owner's or the display's, never both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingState {
    /// Asked to join; waiting for the owner to type in the code from its panel.
    Pending,
    /// The owner approved it; its next request collects its certificate.
    Approved,
    /// Holds a certificate.
    Enrolled {
        serial: String,
        not_after: DateTime<Utc>,
    },
    /// The owner turned it down. Stays refused until the owner removes it.
    Rejected,
    /// Was a member and no longer is: its certificate is still valid for a while, but the server no
    /// longer accepts it.
    Revoked,
}

impl PairingState {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Enrolled { .. } => "enrolled",
            Self::Rejected => "rejected",
            Self::Revoked => "revoked",
        }
    }
}

/// A key a member is changing to. The certificate for it has been issued; the old key stays a member
/// beside it until the new one is first used, which shows the display got the certificate.
///
/// There is no time limit. The key was put forward by the member itself, over a connection that showed its
/// own certificate, so it is no less the display's than the old one, and a display that is off for
/// months between being given the certificate and using it must still find the way open. It ends when
/// the new key is used, or another change replaces it, or the owner revokes or forgets the display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rollover {
    pub key: PublicKey,
    /// When the change was made, for whoever is looking at why a display has two keys.
    pub since: DateTime<Utc>,
}

/// A different key asking to take over a member's name, as when the display was reflashed and lost its
/// key. The member carries on with its own key until the owner approves this one by its code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    pub key: PublicKey,
    pub approved: bool,
    pub requested_at: DateTime<Utc>,
}

/// One display and where it has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub device: DeviceId,
    pub key: PublicKey,
    pub state: PairingState,
    pub requested_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// A key being changed to, while both are members.
    pub rollover: Option<Rollover>,
    /// A key waiting for the owner to approve taking over the name.
    pub replacement: Option<Replacement>,
}

impl Pairing {
    pub fn new(device: DeviceId, key: PublicKey, state: PairingState, now: DateTime<Utc>) -> Self {
        Self {
            device,
            key,
            state,
            requested_at: now,
            updated_at: now,
            rollover: None,
            replacement: None,
        }
    }
}

/// Whether a member's certificate should have been renewed by now and has not been.
///
/// A display renews with a third of its life left, at its next wake, and a display wakes at least once a day, so a
/// healthy one has a new certificate within a day or so of that point. One that still has a certificate with less
/// than a third of its life left, less a few days of allowance, has stopped renewing: it has gone quiet, or it cannot
/// reach the server, or something is wrong with its clock or its flash. With 90-day certificates that is about 63
/// days after it was issued.
///
/// False once the certificate has ended (`remaining` is not positive): that is its own, louder state, and a display
/// that is switched off would otherwise be reported for good. A display that is switched on gets a new certificate
/// by itself.
pub fn renewal_overdue(remaining: Duration, lifetime: Duration) -> bool {
    // Long enough for a display that sleeps its longest and backs off, but never most of a short certificate's life.
    let allowance = Duration::days(3).min(lifetime / 6);
    remaining > Duration::zero() && remaining < lifetime / 3 - allowance
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str) -> DeviceId {
        DeviceId::parse(name).unwrap()
    }

    fn key(byte: u8) -> PublicKey {
        PublicKey::from_der(vec![byte; 91])
    }

    fn authority(byte: u8) -> Fingerprint {
        Fingerprint::of(&[byte; 300])
    }

    #[test]
    fn a_fingerprint_is_written_as_colon_separated_hex() {
        // SHA-256 of nothing, a published value.
        let shown = Fingerprint::of(b"").to_string();
        assert_eq!(
            shown,
            "E3:B0:C4:42:98:FC:1C:14:9A:FB:F4:C8:99:6F:B9:24:27:AE:41:E4:64:9B:93:4C:A4:95:99:1B:78:52:B8:55"
        );
    }

    #[test]
    fn a_code_is_twelve_characters_in_three_groups() {
        let code = PairingCode::derive(&authority(1), &device("kitchen"), &key(2));
        let groups: Vec<&str> = code.as_str().split('-').collect();
        assert_eq!(groups.len(), 3, "{code}");
        assert!(groups.iter().all(|g| g.len() == 4), "{code}");
    }

    #[test]
    fn both_sides_get_the_same_code_from_the_same_things() {
        let a = PairingCode::derive(&authority(1), &device("kitchen"), &key(2));
        let b = PairingCode::derive(&authority(1), &device("kitchen"), &key(2));
        assert_eq!(a, b);
    }

    #[test]
    fn changing_any_one_thing_changes_the_code() {
        // The attack the code is for: each side is shown something different, so each part has to count.
        let base = PairingCode::derive(&authority(1), &device("kitchen"), &key(2));
        assert_ne!(
            base,
            PairingCode::derive(&authority(9), &device("kitchen"), &key(2))
        );
        assert_ne!(
            base,
            PairingCode::derive(&authority(1), &device("hall"), &key(2))
        );
        assert_ne!(
            base,
            PairingCode::derive(&authority(1), &device("kitchen"), &key(9))
        );
    }

    #[test]
    fn the_parts_are_not_run_together() {
        // A name and a key that together spell the same bytes as another pair must not collide.
        let a = PairingCode::derive(
            &authority(1),
            &device("ab"),
            &PublicKey::from_der(b"cd".to_vec()),
        );
        let b = PairingCode::derive(
            &authority(1),
            &device("abc"),
            &PublicKey::from_der(b"d".to_vec()),
        );
        assert_ne!(a, b);
    }

    #[test]
    fn a_code_typed_with_dashes_spaces_any_case_or_neither_is_the_same_code() {
        let code = PairingCode::derive(&authority(1), &device("kitchen"), &key(2));
        let plain: String = code.as_str().replace('-', "");
        for typed in [
            code.as_str().to_owned(),
            plain.clone(),
            plain.to_lowercase(),
            format!(" {} {} {} ", &plain[..4], &plain[4..8], &plain[8..]).to_lowercase(),
        ] {
            assert_eq!(PairingCode::parse(&typed).unwrap(), code, "{typed:?}");
        }
    }

    #[test]
    fn look_alikes_are_read_as_the_characters_they_resemble() {
        let code = PairingCode::parse("0011-0011-0011").unwrap();
        for typed in ["OOIl-OOLi-OOIl", "ooil-ooli-ooil", "00Il-oO1L-0oiL"] {
            assert_eq!(PairingCode::parse(typed).unwrap(), code, "{typed:?}");
        }
        // And what is shown never contains the ones that could be mistaken.
        assert_eq!(code.as_str(), "0011-0011-0011");
    }

    #[test]
    fn a_code_that_is_not_twelve_characters_of_the_alphabet_is_refused() {
        for typed in [
            "",
            "B0AJ",
            "B0AJ-QTW6", // the old eight-character code is not enough
            "B0AJ-QTW6-Y8S",
            "JGWP-14YW-3BT0A",
            "B0AJ-QTW6-Y8SU", // U is not in the alphabet
            "B0AJ-QTW6-Y8!A",
            "B0AJ-QTW6-Y8S\u{0666}",
            "١٢٣٤-٥٦٧٨-٩٠١٢",
        ] {
            assert_eq!(
                PairingCode::parse(typed),
                Err(InvalidPairingCode),
                "{typed:?}"
            );
        }
    }

    #[test]
    fn every_code_that_is_made_is_in_the_alphabet_and_parses_back_to_itself() {
        for i in 0..2000u32 {
            let code =
                PairingCode::derive(&authority(1), &device("kitchen"), &key((i % 251) as u8));
            let chars = code.as_str().replace('-', "");
            assert_eq!(chars.len(), 12);
            assert!(chars.bytes().all(|b| ALPHABET.contains(&b)), "{code}");
            assert!(!chars.contains(['I', 'L', 'O', 'U']), "{code}");
            assert_eq!(PairingCode::parse(code.as_str()).unwrap(), code);
        }
    }

    #[test]
    fn numbers_become_characters_five_bits_at_a_time() {
        assert_eq!(PairingCode::from_number(0).as_str(), "0000-0000-0000");
        assert_eq!(PairingCode::from_number(31).as_str(), "0000-0000-000Z");
        assert_eq!(PairingCode::from_number(32).as_str(), "0000-0000-0010");
        assert_eq!(
            PairingCode::from_number((1 << 60) - 1).as_str(),
            "ZZZZ-ZZZZ-ZZZZ"
        );
    }

    #[test]
    fn a_fixed_example_gives_the_code_the_firmware_documentation_shows() {
        // The expected code was computed by a separate implementation of the formula (Python), and is
        // the example in esphome/README.md for whoever writes the display's side of it.
        let authority = Fingerprint::of(&[0x30, 0x03, 0x02, 0x01, 0x01]);
        assert_eq!(
            authority.to_string().replace(':', "").to_lowercase(),
            "1b65f68a522c858715f5dd951cd0402dc16691778814bf0759822b7a257421d0"
        );
        let key = PublicKey::from_der((0u8..91).collect());
        let code = PairingCode::derive(&authority, &device("reterminal-e1003-a1b2c3"), &key);
        assert_eq!(code.as_str(), "JGWP-14YW-3BT0");
    }

    #[test]
    fn debug_never_prints_a_code_so_a_log_of_anything_holding_one_cannot_leak_it() {
        let code = PairingCode::derive(&authority(1), &device("kitchen"), &key(2));
        let plain = code.as_str().replace('-', "");
        for shown in [
            format!("{code:?}"),
            format!("{code:#?}"),
            format!("{:?}", Some(code.clone())),
        ] {
            assert!(
                !shown.contains(code.as_str()) && !shown.contains(&plain),
                "{shown}"
            );
            assert!(shown.contains("hidden"), "{shown}");
        }
        // Display is still the way to mean to show one.
        assert_eq!(code.to_string(), code.as_str());
    }

    #[test]
    fn a_certificate_is_overdue_for_renewal_a_few_days_after_the_third_of_its_life_that_is_left() {
        let life = Duration::days(90);
        // Fresh, halfway, and at the point where a display renews: not overdue, a display does that on its next wake.
        assert!(!renewal_overdue(Duration::days(90), life));
        assert!(!renewal_overdue(Duration::days(45), life));
        assert!(!renewal_overdue(Duration::days(30), life));
        // The allowance for a display that sleeps for a day and backs off: still not.
        assert!(!renewal_overdue(Duration::days(27), life));
        // Past it: it has stopped renewing.
        assert!(renewal_overdue(
            Duration::days(27) - Duration::seconds(1),
            life
        ));
        assert!(renewal_overdue(Duration::days(5), life));
        assert!(renewal_overdue(Duration::seconds(1), life));
    }

    #[test]
    fn a_certificate_that_has_ended_is_expired_and_not_overdue() {
        let life = Duration::days(90);
        assert!(!renewal_overdue(Duration::zero(), life));
        assert!(!renewal_overdue(-Duration::days(2), life));
    }

    #[test]
    fn a_short_lived_certificate_keeps_most_of_its_life_before_it_is_overdue() {
        // The allowance is a sixth of the life at most, so one day's certificate is not overdue at once.
        let life = Duration::days(1);
        assert!(!renewal_overdue(life / 3 - life / 6, life));
        assert!(renewal_overdue(
            life / 3 - life / 6 - Duration::seconds(1),
            life
        ));
        assert!(!renewal_overdue(Duration::hours(20), life));
    }
}
