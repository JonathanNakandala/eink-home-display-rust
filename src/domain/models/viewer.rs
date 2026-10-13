//! Who may look at the picture in a web browser when it isn't open to everyone: a viewer, with a secret to prove it.
//!
//! The owner makes a viewer and is given a one-time **code** (shown once, in a link). The browser exchanges the code for an
//! access **token** (also shown once, to the browser alone) and sends that with every request, as a bearer token (RFC 6750).
//! Neither secret is kept: only a SHA-256 hash of each, which is enough to recognise one and useless to anyone who reads
//! the file, as both are 256 random bits and so can't be guessed or looked up in a list of likely ones.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;

use super::layout::LayoutName;

/// What a viewer is called to the owner, to tell it from the others: "kitchen tablet".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerLabel(String);

const LONGEST_LABEL: usize = 40;

impl ViewerLabel {
    pub fn parse(text: &str) -> Result<Self, ViewerError> {
        let text = text.trim();
        if text.is_empty()
            || text.chars().count() > LONGEST_LABEL
            || text.chars().any(char::is_control)
        {
            return Err(ViewerError::InvalidLabel);
        }
        Ok(Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ViewerLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How the owner names a viewer to change it: eight lower-case letters and digits, made at random.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewerId(String);

const ID_LENGTH: usize = 8;
const ID_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

impl ViewerId {
    /// An id from random bytes (the first five of them).
    pub fn from_random(bytes: &[u8]) -> Self {
        let mut number = 0u64;
        for byte in bytes.iter().take(5) {
            number = (number << 8) | u64::from(*byte);
        }
        let text: String = (0..ID_LENGTH)
            .map(|i| ID_ALPHABET[((number >> (5 * (ID_LENGTH - 1 - i))) & 31) as usize] as char)
            .collect();
        Self(text)
    }

    pub fn parse(text: &str) -> Result<Self, ViewerError> {
        if text.len() == ID_LENGTH && text.bytes().all(|b| ID_ALPHABET.contains(&b)) {
            Ok(Self(text.to_owned()))
        } else {
            Err(ViewerError::InvalidId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ViewerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which layouts a viewer may look at. Written as OAuth scopes (RFC 6749 section 3.3): `layout:*`, or `layout:NAME` for each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutScope {
    All,
    /// Just these, in the order they are shown.
    Only(Vec<LayoutName>),
}

impl LayoutScope {
    /// The scope for `layouts`: none means all of them.
    pub fn of(layouts: Vec<LayoutName>) -> Self {
        if layouts.is_empty() {
            Self::All
        } else {
            Self::Only(layouts)
        }
    }

    pub fn permits(&self, layout: &LayoutName) -> bool {
        match self {
            Self::All => true,
            Self::Only(layouts) => layouts.contains(layout),
        }
    }

    /// As the `scope` of a token response and of `WWW-Authenticate`: space-separated.
    pub fn to_scope_string(&self) -> String {
        match self {
            Self::All => "layout:*".to_owned(),
            Self::Only(layouts) => layouts
                .iter()
                .map(|layout| format!("layout:{layout}"))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    /// Read back from `to_scope_string`'s form.
    pub fn from_scope_string(text: &str) -> Result<Self, ViewerError> {
        let mut layouts = Vec::new();
        for item in text.split_whitespace() {
            match item.strip_prefix("layout:") {
                Some("*") => return Ok(Self::All),
                Some(name) => {
                    layouts.push(LayoutName::parse(name).map_err(|_| ViewerError::InvalidScope)?)
                }
                None => return Err(ViewerError::InvalidScope),
            }
        }
        if layouts.is_empty() {
            return Err(ViewerError::InvalidScope);
        }
        Ok(Self::Only(layouts))
    }
}

/// What a secret hashes to. Compared in constant time, and never shown.
#[derive(Clone, Copy)]
pub struct SecretHash([u8; 32]);

impl SecretHash {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl PartialEq for SecretHash {
    fn eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl Eq for SecretHash {}

impl fmt::Debug for SecretHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretHash(..)")
    }
}

/// A secret is a prefix that says what it is (and lets a scanner for leaked secrets find one, as RFC 8959 intends), then 256
/// random bits in base64url.
fn make(prefix: &str, random: [u8; 32]) -> String {
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(random))
}

fn well_formed(prefix: &str, text: &str) -> bool {
    text.strip_prefix(prefix).is_some_and(|rest| {
        rest.len() == 43
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

fn hash_of(text: &str) -> SecretHash {
    SecretHash(Sha256::digest(text.as_bytes()).into())
}

macro_rules! secret {
    ($name:ident, $prefix:literal, $what:literal) => {
        #[doc = $what]
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            /// From 256 random bits.
            pub fn generate(random: [u8; 32]) -> Self {
                Self(make($prefix, random))
            }

            /// What was sent, if it is shaped like one. Anything else can't be one, so is never looked for.
            pub fn parse(text: &str) -> Result<Self, ViewerError> {
                if well_formed($prefix, text) {
                    Ok(Self(text.to_owned()))
                } else {
                    Err(ViewerError::NotASecret)
                }
            }

            /// The secret itself. Only for handing to whoever it is for, once.
            pub fn expose(&self) -> &str {
                &self.0
            }

            pub fn hash(&self) -> SecretHash {
                hash_of(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "(..)"))
            }
        }
    };
}

secret!(
    ViewerToken,
    "hdv1_",
    "The access token a browser sends: a bearer token (RFC 6750)."
);
secret!(
    ExchangeCode,
    "hdc1_",
    "The one-time code in the link the owner is given, which a browser exchanges for a token."
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewerState {
    /// Made, and the link not used yet. The code works until `code_expires_at`, once.
    Pending {
        code: SecretHash,
        code_expires_at: DateTime<Utc>,
    },
    /// The code was exchanged. The token works until `expires_at`.
    Active {
        token: SecretHash,
        expires_at: DateTime<Utc>,
        /// When it was last used, to within a few minutes (it is not written on every request).
        last_used_at: Option<DateTime<Utc>>,
    },
    Revoked {
        at: DateTime<Utc>,
    },
}

/// Where a viewer stands, in words for the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerStanding {
    /// The link has not been used, and still can be.
    Waiting,
    /// The link was not used in time.
    LinkExpired,
    Active,
    /// The token ran out.
    Expired,
    Revoked,
}

impl ViewerStanding {
    pub fn name(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::LinkExpired => "link_expired",
            Self::Active => "active",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    pub id: ViewerId,
    pub label: ViewerLabel,
    pub scope: LayoutScope,
    /// How long the token lasts once the link has been used.
    pub lifetime: Duration,
    pub created_at: DateTime<Utc>,
    pub state: ViewerState,
}

impl Viewer {
    pub fn standing(&self, now: DateTime<Utc>) -> ViewerStanding {
        match &self.state {
            ViewerState::Pending {
                code_expires_at, ..
            } if now < *code_expires_at => ViewerStanding::Waiting,
            ViewerState::Pending { .. } => ViewerStanding::LinkExpired,
            ViewerState::Active { expires_at, .. } if now < *expires_at => ViewerStanding::Active,
            ViewerState::Active { .. } => ViewerStanding::Expired,
            ViewerState::Revoked { .. } => ViewerStanding::Revoked,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ViewerError {
    #[error("A label is from 1 to {LONGEST_LABEL} characters, with no control characters")]
    InvalidLabel,
    #[error("A viewer's id is {ID_LENGTH} lower-case letters and digits")]
    InvalidId,
    #[error("A scope is `layout:*` or `layout:` and a layout name, separated by spaces")]
    InvalidScope,
    #[error("That is not shaped like a secret of ours")]
    NotASecret,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(name: &str) -> LayoutName {
        LayoutName::parse(name).unwrap()
    }

    #[test]
    fn a_label_is_tidied_and_kept_short_and_plain() {
        assert_eq!(
            ViewerLabel::parse("  kitchen tablet ").unwrap().as_str(),
            "kitchen tablet"
        );
        for bad in ["", "   ", "a\nb", "tab\there", &"x".repeat(41)] {
            assert_eq!(
                ViewerLabel::parse(bad),
                Err(ViewerError::InvalidLabel),
                "{bad:?}"
            );
        }
        assert!(ViewerLabel::parse(&"x".repeat(40)).is_ok());
    }

    #[test]
    fn an_id_is_eight_letters_and_digits_made_from_random_bytes() {
        let id = ViewerId::from_random(&[1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(id.as_str().len(), 8);
        assert_eq!(ViewerId::parse(id.as_str()), Ok(id.clone()));
        assert_ne!(ViewerId::from_random(&[9, 2, 3, 4, 5]), id);
        for bad in [
            "",
            "short",
            "UPPERCASE",
            "has-dash!",
            "toolongtoolong",
            "abcdefg1",
        ] {
            assert_eq!(ViewerId::parse(bad), Err(ViewerError::InvalidId), "{bad:?}");
        }
    }

    #[test]
    fn a_scope_is_written_and_read_back_as_oauth_scopes() {
        assert_eq!(LayoutScope::All.to_scope_string(), "layout:*");
        let some = LayoutScope::Only(vec![layout("dashboard"), layout("departures")]);
        assert_eq!(some.to_scope_string(), "layout:dashboard layout:departures");
        assert_eq!(
            LayoutScope::from_scope_string(&some.to_scope_string()),
            Ok(some.clone())
        );
        assert_eq!(
            LayoutScope::from_scope_string("layout:*"),
            Ok(LayoutScope::All)
        );
        for bad in [
            "",
            "layout:",
            "layout:Bad Name",
            "view",
            "layout:a layout:../b",
        ] {
            assert_eq!(
                LayoutScope::from_scope_string(bad),
                Err(ViewerError::InvalidScope),
                "{bad:?}"
            );
        }
        // None named is all.
        assert_eq!(LayoutScope::of(vec![]), LayoutScope::All);
        assert!(some.permits(&layout("departures")) && !some.permits(&layout("clock")));
        assert!(LayoutScope::All.permits(&layout("clock")));
    }

    #[test]
    fn a_secret_has_a_prefix_that_says_what_it_is_and_256_bits_in_base64url() {
        let token = ViewerToken::generate([7; 32]);
        let code = ExchangeCode::generate([7; 32]);

        assert!(token.expose().starts_with("hdv1_") && code.expose().starts_with("hdc1_"));
        assert_eq!(token.expose().len(), 5 + 43);
        // Only characters a bearer token may have (RFC 6750 section 2.1: b64token).
        assert!(
            token.expose()[5..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        // Different bytes, different secret; the same bytes in a different kind are not the same thing.
        assert_ne!(token, ViewerToken::generate([8; 32]));
        assert_ne!(token.hash(), code.hash());
    }

    #[test]
    fn only_something_shaped_like_a_secret_is_read_as_one() {
        let token = ViewerToken::generate([1; 32]);
        assert_eq!(ViewerToken::parse(token.expose()), Ok(token.clone()));
        // The other kind, a changed length, a stray character, nothing.
        assert!(ViewerToken::parse(ExchangeCode::generate([1; 32]).expose()).is_err());
        assert!(ViewerToken::parse(&token.expose()[..40]).is_err());
        assert!(ViewerToken::parse(&format!("{} ", token.expose())).is_err());
        assert!(
            ViewerToken::parse(&format!("{}!", &token.expose()[..token.expose().len() - 1]))
                .is_err()
        );
        assert!(ViewerToken::parse("").is_err());
    }

    #[test]
    fn a_secret_is_never_shown_by_debug_or_by_its_hash() {
        let token = ViewerToken::generate([3; 32]);
        let shown = format!("{token:?} {:?}", token.hash());
        assert!(!shown.contains(&token.expose()[5..]), "{shown}");
        assert_eq!(
            token.hash(),
            ViewerToken::parse(token.expose()).unwrap().hash()
        );
    }

    #[test]
    fn where_a_viewer_stands_follows_its_state_and_the_time() {
        let at = |minutes: i64| Utc::now() + Duration::minutes(minutes);
        let now = at(0);
        let viewer = |state| Viewer {
            id: ViewerId::from_random(&[1; 5]),
            label: ViewerLabel::parse("tablet").unwrap(),
            scope: LayoutScope::All,
            lifetime: Duration::days(90),
            created_at: now,
            state,
        };
        let hash = ViewerToken::generate([1; 32]).hash();

        let waiting = viewer(ViewerState::Pending {
            code: hash,
            code_expires_at: at(10),
        });
        assert_eq!(waiting.standing(now), ViewerStanding::Waiting);
        assert_eq!(waiting.standing(at(10)), ViewerStanding::LinkExpired);
        let active = viewer(ViewerState::Active {
            token: hash,
            expires_at: at(60),
            last_used_at: None,
        });
        assert_eq!(active.standing(now), ViewerStanding::Active);
        assert_eq!(active.standing(at(60)), ViewerStanding::Expired);
        assert_eq!(
            viewer(ViewerState::Revoked { at: now }).standing(now),
            ViewerStanding::Revoked
        );
    }
}
