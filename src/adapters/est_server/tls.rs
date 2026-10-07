//! The server's TLS: version 1.3 only, with the server's certificate renewed as it ages.
//!
//! TLS 1.3 is the only version offered. Nothing here has to talk to old clients: the displays are
//! flashed with this server in mind. It also gives the one connection value EST needs to tie a request
//! to its connection (see `Connection`).
//!
//! A client may present a certificate, and if it does it is checked against the authority; one that
//! doesn't is let through to the endpoints that need no identity (a display joining has none yet).
//! Whether a client certificate is required is each endpoint's decision.

use std::sync::Arc;

use anyhow::Context;
use arc_swap::ArcSwap;
use chrono::{DateTime, Duration, Utc};
use rustls::RootCertStore;
use rustls::ServerConfig;
use rustls::crypto::ring as provider;
use rustls::server::{ClientHello, ResolvesServerCert, WebPkiClientVerifier};
use rustls::sign::CertifiedKey;

use crate::adapters::certificate_authority::{PrivateAuthority, ServerIdentity};

/// The server's current certificate, replaced as it nears its end without any connection noticing.
pub struct RenewingCertificate {
    current: ArcSwap<Current>,
}

struct Current {
    key: Arc<CertifiedKey>,
    not_after: DateTime<Utc>,
}

impl std::fmt::Debug for RenewingCertificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenewingCertificate")
            .field("not_after", &self.not_after())
            .finish()
    }
}

impl RenewingCertificate {
    pub fn new(
        authority: &PrivateAuthority,
        names: &[String],
        now: DateTime<Utc>,
        lifetime: Duration,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            current: ArcSwap::from_pointee(issue(authority, names, now, lifetime)?),
        })
    }

    pub fn not_after(&self) -> DateTime<Utc> {
        self.current.load().not_after
    }

    /// Replaces the certificate with a new one, if the one in use has less than a third of its life left.
    /// Returns whether it did.
    pub fn renew_if_due(
        &self,
        authority: &PrivateAuthority,
        names: &[String],
        now: DateTime<Utc>,
        lifetime: Duration,
    ) -> anyhow::Result<bool> {
        if !is_due(self.not_after(), now, lifetime) {
            return Ok(false);
        }
        self.current
            .store(Arc::new(issue(authority, names, now, lifetime)?));
        Ok(true)
    }
}

impl ResolvesServerCert for RenewingCertificate {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current.load().key.clone())
    }
}

/// Whether a certificate that ends at `not_after` should be replaced: once a third of its life is left,
/// which leaves ample time to retry if the authority or the clock is briefly wrong.
fn is_due(not_after: DateTime<Utc>, now: DateTime<Utc>, lifetime: Duration) -> bool {
    not_after - now < lifetime / 3
}

fn issue(
    authority: &PrivateAuthority,
    names: &[String],
    now: DateTime<Utc>,
    lifetime: Duration,
) -> anyhow::Result<Current> {
    let not_after = now + lifetime;
    let ServerIdentity { certificate, key } = authority.issue_server(names, now, not_after)?;
    let key = provider::sign::any_ecdsa_type(&key)
        .context("The server's new key is not one rustls can sign with")?;
    Ok(Current {
        key: Arc::new(CertifiedKey::new(vec![certificate], key)),
        not_after,
    })
}

pub fn server_config(
    certificate: Arc<RenewingCertificate>,
    authority_certificate: &[u8],
) -> anyhow::Result<ServerConfig> {
    let provider = Arc::new(provider::default_provider());
    let mut roots = RootCertStore::empty();
    roots
        .add(authority_certificate.to_vec().into())
        .context("The authority certificate can't be used as a trust anchor")?;
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .allow_unauthenticated()
        .build()
        .context("Failed to set up checking of display certificates")?;
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("TLS 1.3 is not available")?
        .with_client_cert_verifier(verifier)
        .with_cert_resolver(certificate);
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::domain::services::certificate_authority::CertificateAuthority;

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, 0, 0, 0).unwrap()
    }

    #[test]
    fn a_certificate_is_renewed_once_a_third_of_its_life_is_left() {
        let lifetime = Duration::days(9);
        let not_after = at(10);
        // 4 days left of 9: more than a third, so not yet. 3 days is exactly a third, still not due.
        assert!(!is_due(not_after, at(6), lifetime));
        assert!(!is_due(not_after, at(7), lifetime));
        assert!(is_due(not_after, at(8), lifetime));
        // And one that has already ended is certainly due.
        assert!(is_due(not_after, at(20), lifetime));
    }

    fn authority() -> PrivateAuthority {
        PrivateAuthority::generate(at(1)).unwrap().0
    }

    fn names() -> Vec<String> {
        vec!["localhost".to_owned()]
    }

    #[test]
    fn the_certificate_in_use_is_swapped_when_it_is_due_and_not_before() {
        let authority = authority();
        let lifetime = Duration::days(9);
        let certificate = RenewingCertificate::new(&authority, &names(), at(1), lifetime).unwrap();
        assert_eq!(certificate.not_after(), at(10));
        let before = certificate.current.load().key.cert[0].clone();

        assert!(
            !certificate
                .renew_if_due(&authority, &names(), at(5), lifetime)
                .unwrap()
        );
        assert_eq!(certificate.current.load().key.cert[0], before);

        assert!(
            certificate
                .renew_if_due(&authority, &names(), at(8), lifetime)
                .unwrap()
        );
        assert_eq!(certificate.not_after(), at(17));
        assert_ne!(certificate.current.load().key.cert[0], before);
    }

    #[test]
    fn only_tls_1_3_is_configured() {
        let authority = authority();
        let certificate = Arc::new(
            RenewingCertificate::new(&authority, &names(), at(1), Duration::days(9)).unwrap(),
        );
        let config = server_config(
            certificate,
            rustls::pki_types::CertificateDer::from(authority.certificate().to_vec()).as_ref(),
        )
        .unwrap();
        assert_eq!(config.alpn_protocols, [b"http/1.1".to_vec()]);
        // The configuration offers no older version: a TLS 1.2-only client is refused (see the
        // end-to-end tests), and no tickets that would allow 0-RTT replay are accepted.
        assert_eq!(config.max_early_data_size, 0);
    }
}
