//! The EST endpoints (RFC 7030 section 3.2.2), under `/.well-known/est`.
//!
//! - `GET cacerts`: the authority's certificate. Anyone may ask: it is what a display is given to
//!   trust the server by, and is public. A display that has no trust yet takes it from an unverified
//!   connection and has the owner confirm it by the pairing code, never by trusting this response.
//! - `GET csrattrs`: what a request should contain.
//! - `POST simpleenroll`: a display asks to join, or collects its approved certificate. Not
//!   authenticated; the owner's approval is what lets it through.
//! - `POST simplereenroll`: a member renews, authenticated by its current certificate.
//!
//! Responses to a request that is waiting never carry the pairing code. The display works that out
//! itself from what it saw; if the server told it, someone in the middle could tell it the same.

use std::sync::Arc;

use axum::Extension;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, RETRY_AFTER};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tower_http::timeout::TimeoutLayer;

use super::wire;
use crate::application::enrollment::{EnrollError, Enrollment, Outcome, Refusal};
use crate::domain::models::device_id::DeviceId;
use crate::domain::models::pairing::PublicKey;
use crate::domain::services::certificate_authority::{CertificateAuthority, IssuedCertificate};

const BASE: &str = "/.well-known/est";
/// A certificate request is a few hundred bytes; this leaves room for any reasonable one and no more.
const MAX_BODY: usize = 8 * 1024;

/// Who a client certificate says its holder is: the name in it and the key it certifies. TLS has
/// already checked it against the authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentity {
    pub device: DeviceId,
    pub key: PublicKey,
}

/// What the server knows about the connection a request arrived on, from the TLS layer.
#[derive(Debug, Clone)]
pub struct Connection {
    /// The connection's channel binding (RFC 9266 `tls-exporter`), which a request can carry to prove
    /// it was signed on this very connection.
    pub binding: Option<Vec<u8>>,
    /// The display the client certificate names, if the client showed one.
    pub client: Option<ClientIdentity>,
}

struct Est {
    enrollment: Arc<Enrollment>,
    authority: Arc<dyn CertificateAuthority>,
}

pub fn router(
    enrollment: Arc<Enrollment>,
    authority: Arc<dyn CertificateAuthority>,
    request_timeout: std::time::Duration,
) -> Router {
    Router::new()
        .route(&format!("{BASE}/cacerts"), get(cacerts))
        .route(&format!("{BASE}/csrattrs"), get(csrattrs))
        .route(&format!("{BASE}/simpleenroll"), post(simple_enroll))
        .route(&format!("{BASE}/simplereenroll"), post(simple_reenroll))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            request_timeout,
        ))
        .with_state(Arc::new(Est {
            enrollment,
            authority,
        }))
}

fn text(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!("{message}\n"),
    )
        .into_response()
}

fn certificates(chain: &[&[u8]]) -> Response {
    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, wire::CERTS_ONLY),
            (CACHE_CONTROL, "no-store"),
        ],
        wire::encode_body(&wire::certs_only(chain)),
    )
        .into_response()
}

/// The root, which a display pins, and the intermediate that signs under it (RFC 7030 section 4.1.3: the
/// certificates needed to build a path from an issued certificate to the root). The root is the one that
/// is self-signed.
async fn cacerts(State(est): State<Arc<Est>>) -> Response {
    let mut chain = vec![est.authority.certificate()];
    chain.extend(est.authority.intermediate());
    certificates(&chain)
}

async fn csrattrs(State(est): State<Arc<Est>>) -> Response {
    (
        StatusCode::OK,
        [(CONTENT_TYPE, wire::CSR_ATTRIBUTES)],
        wire::encode_body(&wire::csr_attributes(
            est.enrollment.requires_channel_binding(),
        )),
    )
        .into_response()
}

/// The DER of the request in a body, or the status and message to answer with instead.
fn request_der(headers: &HeaderMap, body: &Bytes) -> Result<Vec<u8>, (StatusCode, &'static str)> {
    let media_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        });
    if media_type.as_deref() != Some(wire::PKCS10) {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "The request must be application/pkcs10",
        ));
    }
    wire::decode_body(body)
        .filter(|der| !der.is_empty())
        .ok_or((StatusCode::BAD_REQUEST, "The request body is not base64"))
}

async fn simple_enroll(
    State(est): State<Arc<Est>>,
    Extension(connection): Extension<Connection>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let der = match request_der(&headers, &body) {
        Ok(der) => der,
        Err((status, message)) => return text(status, message),
    };
    match est
        .enrollment
        .enroll(&der, connection.binding.as_deref())
        .await
    {
        Ok(Outcome::Issued(certificate)) => issued(&certificate),
        Ok(Outcome::Pending { retry_after, .. }) => (
            StatusCode::ACCEPTED,
            [
                (RETRY_AFTER, retry_after.num_seconds().max(1).to_string()),
                (CONTENT_TYPE, "text/plain; charset=utf-8".to_owned()),
            ],
            "Waiting for the owner to approve this display. Ask again later.\n",
        )
            .into_response(),
        Err(e) => failure(e),
    }
}

async fn simple_reenroll(
    State(est): State<Arc<Est>>,
    Extension(connection): Extension<Connection>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Not 401, which must come with a `WWW-Authenticate` challenge (RFC 9110 section 15.5.2), and there
    // is no HTTP scheme for "show a TLS client certificate".
    let Some(caller) = &connection.client else {
        return text(
            StatusCode::FORBIDDEN,
            "Renewing needs the display's current certificate",
        );
    };
    let der = match request_der(&headers, &body) {
        Ok(der) => der,
        Err((status, message)) => return text(status, message),
    };
    match est
        .enrollment
        .renew(
            &caller.device,
            &caller.key,
            &der,
            connection.binding.as_deref(),
        )
        .await
    {
        Ok(certificate) => issued(&certificate),
        Err(e) => failure(e),
    }
}

fn issued(certificate: &IssuedCertificate) -> Response {
    certificates(&[certificate.der.as_slice()])
}

/// How a refusal reads to the display. The text says why, for whoever reads the display's log; the
/// status says whether to try again (a 4xx means the request itself must change).
fn failure(error: EnrollError) -> Response {
    match error {
        EnrollError::Refused(refusal) => {
            log::warn!("EST request refused: {refusal}");
            let status = match &refusal {
                Refusal::BadRequest(_)
                | Refusal::ChannelBindingMissing
                | Refusal::ChannelBindingMismatch => StatusCode::BAD_REQUEST,
                Refusal::NotEnrolled
                | Refusal::NotAccepting
                | Refusal::Rejected
                | Refusal::Revoked
                | Refusal::KeyMismatch
                | Refusal::CertificateSuperseded
                | Refusal::WrongDevice => StatusCode::FORBIDDEN,
                Refusal::TooManyPending => StatusCode::SERVICE_UNAVAILABLE,
            };
            let mut response = text(status, &refusal.to_string());
            if status == StatusCode::SERVICE_UNAVAILABLE {
                response.headers_mut().insert(
                    RETRY_AFTER,
                    "600".parse().expect("a number is a header value"),
                );
            }
            response
        }
        EnrollError::Failed(e) => {
            log::error!("EST request failed: {e:#}");
            text(StatusCode::INTERNAL_SERVER_ERROR, "The server failed")
        }
    }
}
