//! Who is asking. A display names itself with `?device=` on every request it makes (`/plan`,
//! `/refresh` and `/image`), and this reads it once, at the edge, into a validated `DeviceId`. The
//! handlers and everything behind them are given that, never the raw text, and never look for the
//! caller anywhere else (an address, the request before): identity that travels with the request
//! can't be mixed up between two displays asking at the same moment.
//!
//! Over HTTPS a display is named by its certificate instead (`AuthenticatedDevice`, put on the request
//! by the TLS server), and then the query is ignored: a name anyone can write is never preferred to one
//! a certificate proves.
//!
//! Naming is optional and never a reason to refuse: a browser, `curl` or an older display has no
//! name, and one with a bad name is served all the same, just not recorded.

use std::convert::Infallible;

use axum::async_trait;
use axum::extract::{FromRequestParts, Query};
use axum::http::request::Parts;
use serde::Deserialize;

use crate::adapters::authenticated::AuthenticatedDevice;
use crate::domain::models::device_id::DeviceId;

/// The display behind a request, if it said who it is and the name is usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Caller(pub Option<DeviceId>);

#[derive(Deserialize)]
struct Named {
    device: Option<String>,
}

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(AuthenticatedDevice(proved)) = parts.extensions.get::<AuthenticatedDevice>() {
            return Ok(Self(Some(proved.clone())));
        }
        let name = Query::<Named>::try_from_uri(&parts.uri)
            .ok()
            .and_then(|query| query.0.device);
        let Some(name) = name else {
            return Ok(Self(None));
        };
        match DeviceId::parse(&name) {
            Ok(id) => Ok(Self(Some(id))),
            Err(e) => {
                // Not logged above debug: it is whatever a stranger on the network sent.
                log::debug!(
                    "Ignoring a device name that isn't usable ({e}) on {}",
                    parts.uri.path()
                );
                Ok(Self(None))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Request;

    use super::*;

    async fn caller(uri: &str) -> Option<DeviceId> {
        let (mut parts, _) = Request::builder().uri(uri).body(()).unwrap().into_parts();
        Caller::from_request_parts(&mut parts, &()).await.unwrap().0
    }

    fn id(name: &str) -> Option<DeviceId> {
        Some(DeviceId::parse(name).unwrap())
    }

    #[tokio::test]
    async fn a_display_names_itself_on_any_path() {
        assert_eq!(
            caller("/plan?have=1&device=kitchen&battery_mv=3700").await,
            id("kitchen")
        );
        assert_eq!(
            caller("/image?device=reterminal-e1003-a1b2c3").await,
            id("reterminal-e1003-a1b2c3")
        );
        assert_eq!(caller("/refresh?device=%20kitchen%20").await, id("kitchen"));
    }

    #[tokio::test]
    async fn no_name_or_a_bad_one_is_nobody_and_never_an_error() {
        for uri in [
            "/image",
            "/image?have=1",
            "/image?device=",
            "/image?device=has%20space",
            "/image?device=%22quote",
            "/image?device=a%0Ab",
            "/image?device=%FF%FE",
            "/image?device=kitchen&device=again&%",
        ] {
            let found = caller(uri).await;
            // A repeated key is a malformed query, so not a name either.
            assert!(
                found.is_none() || uri.contains("kitchen"),
                "{uri}: {found:?}"
            );
        }
        assert_eq!(caller("/image?device=has%20space").await, None);
        assert_eq!(caller("/image?device=").await, None);
    }

    async fn caller_with(uri: &str, proved: Option<&str>) -> Option<DeviceId> {
        let mut request = Request::builder().uri(uri).body(()).unwrap();
        if let Some(name) = proved {
            request
                .extensions_mut()
                .insert(AuthenticatedDevice(DeviceId::parse(name).unwrap()));
        }
        let (mut parts, _) = request.into_parts();
        Caller::from_request_parts(&mut parts, &()).await.unwrap().0
    }

    #[tokio::test]
    async fn a_name_proved_by_a_certificate_is_the_caller_whatever_the_query_says() {
        assert_eq!(caller_with("/plan", Some("kitchen")).await, id("kitchen"));
        // A display claiming to be another one gets no further than its own certificate.
        assert_eq!(
            caller_with("/plan?device=hall", Some("kitchen")).await,
            id("kitchen")
        );
        assert_eq!(
            caller_with("/plan?device=%20bad%20name%20", Some("kitchen")).await,
            id("kitchen")
        );
    }

    #[tokio::test]
    async fn without_a_certificate_the_query_is_used_as_before() {
        assert_eq!(caller_with("/plan?device=hall", None).await, id("hall"));
        assert_eq!(caller_with("/plan", None).await, None);
    }
}
