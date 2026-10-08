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
use rustls::crypto::aws_lc_rs as provider;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{ClientHello, ResolvesServerCert, WebPkiClientVerifier};
use rustls::sign::CertifiedKey;
use rustls::{
    DigitallySignedStruct, DistinguishedName, RootCertStore, ServerConfig, SignatureScheme,
};

use crate::adapters::certificate_authority::{PrivateAuthority, ServerIdentity};
use crate::domain::services::certificate_authority::CertificateAuthority;
use crate::domain::services::clock::{earliest_plausible, implausible};

/// The server's current certificate, replaced as it nears its end without any connection noticing.
pub struct RenewingCertificate {
    current: ArcSwap<Current>,
}

struct Current {
    key: Arc<CertifiedKey>,
    /// The time the certificate was made at, by the clock that made it.
    issued_at: DateTime<Utc>,
    /// When it really ends: the end asked for, or the intermediate's if that comes first.
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

    /// Replaces the certificate with a new one, if the one in use is due (see `is_due`). Returns whether it did.
    /// If a new one can't be made (the clock is not set) the one in use stays.
    pub fn renew_if_due(
        &self,
        authority: &PrivateAuthority,
        names: &[String],
        now: DateTime<Utc>,
        lifetime: Duration,
    ) -> anyhow::Result<bool> {
        let current = self.current.load();
        if !is_due(current.issued_at, current.not_after, now) {
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

/// Whether a certificate made at `issued_at` that ends at `not_after` should be replaced:
/// - once a third of the life it really has is left, which leaves ample time to retry. (Its real life, not the one
///   asked for: a certificate cut short by its intermediate's end would otherwise be due the moment it was made.)
/// - or at once if the clock now reads earlier than when it was made. The clock has been set back, perhaps from a
///   wrong guess to the right time, and a certificate dated from the wrong one is not valid yet.
///
/// A clock that has jumped forward needs no case of its own: the certificate is then past its end, or nearly.
fn is_due(issued_at: DateTime<Utc>, not_after: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now < issued_at || not_after - now < (not_after - issued_at) / 3
}

fn issue(
    authority: &PrivateAuthority,
    names: &[String],
    now: DateTime<Utc>,
    lifetime: Duration,
) -> anyhow::Result<Current> {
    // A server certificate dated from a clock that has not been set would be valid in 1970 and no later: the
    // displays would refuse it as soon as their clocks were right, or the server's was corrected.
    if now < earliest_plausible() {
        anyhow::bail!(implausible(now));
    }
    let not_after = now.checked_add_signed(lifetime).with_context(|| {
        format!(
            "a certificate lifetime of {} days is too long to add to {now}",
            lifetime.num_days()
        )
    })?;
    let ServerIdentity {
        certificate,
        not_after,
        intermediate,
        key,
    } = authority.issue_server(names, now, not_after)?;
    let key = provider::sign::any_ecdsa_type(&key)
        .context("The server's new key is not one rustls can sign with")?;
    Ok(Current {
        key: Arc::new(CertifiedKey::new(vec![certificate, intermediate], key)),
        issued_at: now,
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
        // Made on the 1st, ending on the 10th: nine days, so a third is three.
        let (issued, not_after) = (at(1), at(10));
        // 4 days left of 9: more than a third, so not yet. 3 days is exactly a third, still not due.
        assert!(!is_due(issued, not_after, at(6)));
        assert!(!is_due(issued, not_after, at(7)));
        assert!(is_due(issued, not_after, at(8)));
        // And one that has already ended is certainly due.
        assert!(is_due(issued, not_after, at(20)));
    }

    #[test]
    fn a_certificate_is_due_at_once_if_the_clock_has_been_set_back_past_when_it_was_made() {
        // Made at a time the clock later turned out to have wrong: it is dated from the future, so not valid yet.
        assert!(is_due(at(20), at(28), at(5)));
        // The same instant is not set back.
        assert!(!is_due(at(5), at(28), at(5)));
    }

    #[test]
    fn a_certificate_is_due_by_the_life_it_really_has() {
        // Cut short to ten days. Judged by the 90 asked for it would be due the moment it was made, and
        // remade every check; by its real ten, it is due with three days left.
        let (issued, not_after) = (at(1), at(11));
        assert!(!is_due(issued, not_after, at(1)));
        assert!(!is_due(issued, not_after, at(7)));
        assert!(is_due(issued, not_after, at(9)));
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

    #[test]
    fn no_server_certificate_is_made_from_a_clock_that_has_not_been_set() {
        let authority = authority();
        let unset = Utc.with_ymd_and_hms(1970, 1, 1, 0, 5, 0).unwrap();
        let error = RenewingCertificate::new(&authority, &names(), unset, Duration::days(90))
            .unwrap_err()
            .to_string();
        assert!(error.contains("can't be right"), "{error}");
    }

    #[test]
    fn when_the_clock_is_found_wrong_the_certificate_in_use_stays() {
        let authority = authority();
        let certificate =
            RenewingCertificate::new(&authority, &names(), at(1), Duration::days(9)).unwrap();
        let before = certificate.current.load().key.cert[0].clone();
        // Due, but the clock now reads 1970: nothing is made, and what there is carries on.
        let unset = Utc.with_ymd_and_hms(1970, 1, 1, 0, 5, 0).unwrap();
        assert!(
            certificate
                .renew_if_due(&authority, &names(), unset, Duration::days(9))
                .is_err()
        );
        assert_eq!(certificate.current.load().key.cert[0], before);
    }

    #[test]
    fn a_clock_set_back_to_the_right_time_gets_a_certificate_that_is_valid_at_it() {
        let authority = authority();
        // Made on the 20th by a clock that was wrong; the real time is the 5th.
        let certificate =
            RenewingCertificate::new(&authority, &names(), at(20), Duration::days(9)).unwrap();
        let before = certificate.current.load().key.cert[0].clone();
        assert!(
            certificate
                .renew_if_due(&authority, &names(), at(5), Duration::days(9))
                .unwrap()
        );
        assert_ne!(certificate.current.load().key.cert[0], before);
        assert_eq!(certificate.not_after(), at(14));
    }

    #[test]
    fn a_lifetime_too_long_to_add_to_a_date_is_an_error_and_not_a_panic() {
        let authority = authority();
        for days in [4_000_000_000, i64::from(i32::MAX)] {
            let result =
                RenewingCertificate::new(&authority, &names(), at(1), Duration::days(days));
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains("too long") || error.contains("out of range"),
                "{days}: {error}"
            );
        }
    }

    #[test]
    fn a_certificate_cannot_outlast_its_intermediate_and_is_judged_by_the_life_it_has() {
        use crate::adapters::certificate_authority::new_intermediate;
        // An intermediate with ten days left: made five years less ten days ago.
        let (_, files) = PrivateAuthority::generate(Utc::now()).unwrap();
        let now = Utc::now();
        let intermediate = new_intermediate(
            &files.root_certificate,
            &files.root_key,
            now - Duration::days(365 * 5 - 10),
        )
        .unwrap();
        let authority =
            PrivateAuthority::from_pem(&files.root_certificate, &intermediate, &[], now).unwrap();
        let certificate =
            RenewingCertificate::new(&authority, &names(), now, Duration::days(90)).unwrap();
        let end = certificate.not_after();
        assert_eq!(end, authority.intermediate_not_after());
        assert!(end - now < Duration::days(11), "{end}");
        // Not remade every check, as it would be if judged by the 90 days asked for.
        assert!(
            !certificate
                .renew_if_due(
                    &authority,
                    &names(),
                    now + Duration::hours(1),
                    Duration::days(90)
                )
                .unwrap()
        );
    }
}
