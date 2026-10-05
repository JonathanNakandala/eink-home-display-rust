//! Why a data source (the weather, a departures board) couldn't give an answer.
//!
//! The kinds are the ones a caller treats differently: a transient failure is worth retrying and
//! will probably clear, while rejected credentials or a request the service refuses won't clear on
//! their own, and say something is wrong with the configuration.

use thiserror::Error;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SourceError {
    /// No answer in time.
    #[error("timed out")]
    Timeout,
    /// The connection couldn't be made, or broke.
    #[error("could not reach the service")]
    Unreachable(#[source] BoxError),
    /// The API key or credentials were refused.
    #[error("credentials rejected (HTTP {status})")]
    Unauthorized { status: u16 },
    #[error("rate limited")]
    RateLimited,
    /// The service itself failed.
    #[error("service error (HTTP {status})")]
    Upstream { status: u16 },
    /// The service refused the request, e.g. a wrong URL or parameters.
    #[error("request rejected (HTTP {status})")]
    Rejected { status: u16 },
    /// The service answered, but not with something usable.
    #[error("unusable response: {detail}")]
    BadResponse {
        detail: String,
        #[source]
        source: Option<BoxError>,
    },
}

impl SourceError {
    pub fn bad_response(detail: impl Into<String>) -> Self {
        Self::BadResponse { detail: detail.into(), source: None }
    }

    pub fn bad_response_from(detail: impl Into<String>, source: impl Into<BoxError>) -> Self {
        Self::BadResponse { detail: detail.into(), source: Some(source.into()) }
    }

    /// Whether trying again shortly may well work.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Timeout | Self::Unreachable(_) | Self::RateLimited | Self::Upstream { .. })
    }

    /// Whether this won't clear by itself, because the key, the URL or the settings are wrong.
    pub fn needs_attention(&self) -> bool {
        matches!(self, Self::Unauthorized { .. } | Self::Rejected { .. })
    }

    /// A few words for the display, e.g. "key rejected".
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Timeout => "timed out",
            Self::Unreachable(_) => "no connection",
            Self::Unauthorized { .. } => "key rejected",
            Self::RateLimited => "rate limited",
            Self::Upstream { .. } => "service error",
            Self::Rejected { .. } => "request rejected",
            Self::BadResponse { .. } => "bad response",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unreachable() -> SourceError {
        SourceError::Unreachable("refused".into())
    }

    #[test]
    fn only_failures_that_may_clear_are_transient() {
        assert!(SourceError::Timeout.is_transient());
        assert!(unreachable().is_transient());
        assert!(SourceError::RateLimited.is_transient());
        assert!(SourceError::Upstream { status: 503 }.is_transient());

        assert!(!SourceError::Unauthorized { status: 401 }.is_transient());
        assert!(!SourceError::Rejected { status: 404 }.is_transient());
        assert!(!SourceError::bad_response("no maximum").is_transient());
    }

    #[test]
    fn a_wrong_key_or_request_needs_attention_and_nothing_else_does() {
        assert!(SourceError::Unauthorized { status: 403 }.needs_attention());
        assert!(SourceError::Rejected { status: 400 }.needs_attention());
        assert!(!SourceError::Timeout.needs_attention());
        assert!(!SourceError::Upstream { status: 500 }.needs_attention());
        assert!(!SourceError::bad_response("x").needs_attention());
    }

    #[test]
    fn the_message_and_the_cause_are_kept() {
        let error = SourceError::bad_response_from("could not read the forecast", std::fmt::Error);
        assert_eq!(error.to_string(), "unusable response: could not read the forecast");
        assert!(std::error::Error::source(&error).is_some());
        assert!(std::error::Error::source(&unreachable()).is_some());
        assert!(std::error::Error::source(&SourceError::Timeout).is_none());
    }

    #[test]
    fn each_kind_has_a_short_reason_for_the_display() {
        assert_eq!(SourceError::Unauthorized { status: 401 }.reason(), "key rejected");
        assert_eq!(SourceError::Timeout.reason(), "timed out");
        assert_eq!(unreachable().reason(), "no connection");
    }
}
