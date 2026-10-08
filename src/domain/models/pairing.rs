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

use chrono::{DateTime, Utc};
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

/// How many decimal digits a code has. Twelve is about 40 bits: a short code can be matched by
/// someone in the middle who generates keys until one gives the code they need, and at eight digits
/// that is minutes of work for a laptop, at twelve it is not worth attempting.
const DIGITS: usize = 12;
const GROUP: usize = 4;

/// `digits` (all ASCII, `DIGITS` of them) split into dash-separated groups.
fn group(digits: &str) -> String {
    (0..DIGITS)
        .step_by(GROUP)
        .map(|start| &digits[start..start + GROUP])
        .collect::<Vec<_>>()
        .join("-")
}

/// The code a display shows and the owner types in, as `1234-5678-9012`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PairingCode(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("a pairing code is {DIGITS} digits, like 1234-5678-9012")]
pub struct InvalidPairingCode;

impl PairingCode {
    /// What the display and the server each compute. It covers the authority certificate, the name the
    /// display claims and its public key, with each part's length so no two combinations give the same
    /// input; the label keeps it from being reused as some other hash.
    pub fn derive(authority: &Fingerprint, device: &DeviceId, key: &PublicKey) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"eink-home-display pairing code v1");
        hash.update(authority.as_bytes());
        for part in [device.as_str().as_bytes(), key.as_der()] {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part);
        }
        let digest = hash.finalize();
        let number = u64::from_be_bytes(digest[..8].try_into().expect("a digest has 32 bytes"));
        Self::from_number(number % 10u64.pow(DIGITS as u32))
    }

    /// The digits of `number`, padded with zeros to the full length, in groups.
    fn from_number(number: u64) -> Self {
        let digits = format!("{number:0DIGITS$}");
        Self(group(&digits))
    }

    /// A code as typed: the digits, with dashes or spaces between them or none at all.
    pub fn parse(text: &str) -> Result<Self, InvalidPairingCode> {
        let digits: String = text.chars().filter(|c| !matches!(c, '-' | ' ')).collect();
        if digits.len() != DIGITS || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(InvalidPairingCode);
        }
        Ok(Self(group(&digits)))
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

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PairingCode({self})")
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

/// One display and where it has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub device: DeviceId,
    pub key: PublicKey,
    /// What the display shows; kept so the owner can be told which request is which.
    pub code: PairingCode,
    pub state: PairingState,
    pub requested_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
    fn a_code_is_twelve_digits_in_three_groups() {
        let code = PairingCode::derive(&authority(1), &device("kitchen"), &key(2));
        let text = code.as_str();
        assert_eq!(text.len(), 14, "{text}");
        let groups: Vec<&str> = text.split('-').collect();
        assert_eq!(groups.len(), 3, "{text}");
        assert!(
            groups
                .iter()
                .all(|g| g.len() == 4 && g.chars().all(|c| c.is_ascii_digit()))
        );
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
    fn a_code_typed_with_dashes_spaces_or_neither_is_the_same_code() {
        let code = PairingCode::derive(&authority(1), &device("kitchen"), &key(2));
        let digits: String = code.as_str().replace('-', "");
        for typed in [
            code.as_str().to_owned(),
            digits.clone(),
            format!(" {} {} {} ", &digits[..4], &digits[4..8], &digits[8..]),
        ] {
            assert_eq!(PairingCode::parse(&typed).unwrap(), code, "{typed:?}");
        }
    }

    #[test]
    fn a_code_that_is_not_twelve_digits_is_refused() {
        for typed in [
            "",
            "1234",
            "1234-5678-901",
            "1234-5678-9012-3",
            "1234-5678-90ab",
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
    fn a_small_number_keeps_its_leading_zeros() {
        assert_eq!(PairingCode::from_number(42).as_str(), "0000-0000-0042");
        assert_eq!(PairingCode::from_number(0).as_str(), "0000-0000-0000");
        assert_eq!(
            PairingCode::from_number(999_999_999_999).as_str(),
            "9999-9999-9999"
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
        assert_eq!(code.as_str(), "5404-2991-0703");
    }
}
