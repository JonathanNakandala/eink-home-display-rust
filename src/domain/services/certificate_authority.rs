//! The private authority that signs the displays' certificates, as the application needs it. What is
//! inside a certificate request, and how a certificate is made, are the adapter's business; the
//! application only asks who a request is from, and for a certificate once the owner has agreed.

use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::domain::models::device_id::DeviceId;
use crate::domain::models::pairing::{Fingerprint, PublicKey};

/// A certificate request that has been read and whose signature has been checked, which shows its
/// sender holds the private key for the public key it asks to have certified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateRequest {
    /// The name the display gave itself, which becomes the certificate's subject.
    pub device: DeviceId,
    pub key: PublicKey,
    /// What the request says about the connection it was sent over (RFC 7030 section 3.5), if it says
    /// anything: it proves the request was signed on this very connection.
    pub channel_binding: Option<Vec<u8>>,
    /// The request as received, for the authority to sign from.
    pub der: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCertificate {
    pub der: Vec<u8>,
    pub serial: String,
    pub not_after: DateTime<Utc>,
}

/// Why a request was not usable. These are the sender's mistakes; the authority failing is an error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RequestError {
    #[error("the request is not a valid certificate request: {0}")]
    Malformed(String),
    #[error("the request's signature does not match its key")]
    BadSignature,
    #[error("the request names no valid device: {0}")]
    BadDevice(String),
    #[error("the request asks for a key type this server does not certify: {0}")]
    UnsupportedKey(String),
}

pub trait CertificateAuthority: Send + Sync {
    /// The authority's own certificate: what a display is given to trust the server by.
    fn certificate(&self) -> &[u8];

    fn fingerprint(&self) -> Fingerprint;

    /// Reads a request and checks its signature, without issuing anything.
    fn inspect(&self, der: &[u8]) -> Result<CertificateRequest, RequestError>;

    /// A certificate for the request's key and name, valid from `now` until `not_after`.
    fn issue(
        &self,
        request: &CertificateRequest,
        now: DateTime<Utc>,
        not_after: DateTime<Utc>,
    ) -> anyhow::Result<IssuedCertificate>;
}
