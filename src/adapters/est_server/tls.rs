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
use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::ring as provider;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{ClientHello, ResolvesServerCert, WebPkiClientVerifier};
use rustls::sign::CertifiedKey;
use rustls::{
    DigitallySignedStruct, DistinguishedName, RootCertStore, ServerConfig, SignatureScheme,
};

use crate::adapters::certificate_authority::{PrivateAuthority, ServerIdentity};
use crate::domain::services::certificate_authority::CertificateAuthority;

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
    let ServerIdentity {
        certificate,
        intermediate,
        key,
    } = authority.issue_server(names, now, not_after)?;
    let key = provider::sign::any_ecdsa_type(&key)
        .context("The server's new key is not one rustls can sign with")?;
    Ok(Current {
        key: Arc::new(CertifiedKey::new(vec![certificate, intermediate], key)),
        not_after,
    })
}

/// Checks a display's certificate against the root, with the server's own intermediates added to what
/// the display sent. A display need not keep its intermediate or send it: the one that signed its
/// certificate is found here, and it stays here after being replaced until it ends.
#[derive(Debug)]
struct CompletesTheChain {
    inner: Arc<dyn ClientCertVerifier>,
    intermediates: Vec<CertificateDer<'static>>,
}

impl ClientCertVerifier for CompletesTheChain {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let mut path: Vec<CertificateDer<'_>> = intermediates.to_vec();
        path.extend(self.intermediates.iter().cloned());
        self.inner.verify_client_cert(end_entity, &path, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

pub fn server_config(
    certificate: Arc<RenewingCertificate>,
    authority: &PrivateAuthority,
) -> anyhow::Result<ServerConfig> {
    let provider = Arc::new(provider::default_provider());
    let mut roots = RootCertStore::empty();
    roots
        .add(authority.certificate().to_vec().into())
        .context("The authority certificate can't be used as a trust anchor")?;
    let inner = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .allow_unauthenticated()
        .build()
        .context("Failed to set up checking of display certificates")?;
    let verifier = Arc::new(CompletesTheChain {
        inner,
        intermediates: authority
            .intermediates()
            .into_iter()
            .map(|der| CertificateDer::from(der.to_vec()))
            .collect(),
    });
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
        let config = server_config(certificate, &authority).unwrap();
        assert_eq!(config.alpn_protocols, [b"http/1.1".to_vec()]);
        // The configuration offers no older version: a TLS 1.2-only client is refused (see the
        // end-to-end tests), and no tickets that would allow 0-RTT replay are accepted.
        assert_eq!(config.max_early_data_size, 0);
    }

    /// A display's certificate from `authority`, as a bare leaf with nothing sent beside it.
    fn display_certificate(authority: &PrivateAuthority) -> CertificateDer<'static> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = rcgen::CertificateParams::default();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "kitchen");
        let request = params.serialize_request(&key).unwrap();
        let request = authority.inspect(request.der()).unwrap();
        let now = Utc::now();
        let issued = authority
            .issue(&request, now, now + Duration::days(90))
            .unwrap();
        CertificateDer::from(issued.der)
    }

    fn verifier_for(authority: &PrivateAuthority) -> Arc<dyn ClientCertVerifier> {
        let provider = Arc::new(provider::default_provider());
        let mut roots = RootCertStore::empty();
        roots.add(authority.certificate().to_vec().into()).unwrap();
        let inner = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .unwrap();
        Arc::new(CompletesTheChain {
            inner,
            intermediates: authority
                .intermediates()
                .into_iter()
                .map(|der| CertificateDer::from(der.to_vec()))
                .collect(),
        })
    }

    fn accepts(authority: &PrivateAuthority, leaf: &CertificateDer<'_>) -> bool {
        verifier_for(authority)
            .verify_client_cert(leaf, &[], UnixTime::now())
            .is_ok()
    }

    #[test]
    fn a_bare_certificate_is_checked_against_the_servers_intermediates() {
        let authority = PrivateAuthority::generate(Utc::now()).unwrap().0;
        let leaf = display_certificate(&authority);
        assert!(accepts(&authority, &leaf));
        // A certificate of some other authority's is not.
        let other = PrivateAuthority::generate(Utc::now()).unwrap().0;
        assert!(!accepts(&authority, &display_certificate(&other)));
    }

    #[test]
    fn a_certificate_from_an_intermediate_that_was_replaced_is_still_good_while_it_is_kept() {
        let (first, files) = PrivateAuthority::generate(Utc::now()).unwrap();
        let leaf = display_certificate(&first);
        let (old_certificate, _) = files.intermediate.split_at(
            files
                .intermediate
                .find("-----BEGIN PRIVATE KEY-----")
                .unwrap(),
        );
        let new = crate::adapters::certificate_authority::new_intermediate(
            &files.root_certificate,
            &files.root_key,
            Utc::now(),
        )
        .unwrap();

        let kept = PrivateAuthority::from_pem(
            &files.root_certificate,
            &new,
            &[old_certificate.to_owned()],
            Utc::now(),
        )
        .unwrap();
        assert_ne!(kept.intermediate(), first.intermediate());
        assert!(
            accepts(&kept, &leaf),
            "kept: what the old one signed stays good"
        );

        // Once it is no longer kept, what it signed has no path and is refused (until renewed).
        let forgotten =
            PrivateAuthority::from_pem(&files.root_certificate, &new, &[], Utc::now()).unwrap();
        assert!(!accepts(&forgotten, &leaf));
        // And what the new one signs is good either way.
        assert!(accepts(&forgotten, &display_certificate(&forgotten)));
    }
}
