//! The server's own certificate authority, made with `rcgen`. It signs the certificates of the displays
//! that have been approved and the server's own, so nothing here depends on a public authority or a
//! domain name.
//!
//! It is two levels, as a private authority should be:
//!
//! - the **root**: long-lived, signs nothing but intermediates (`pathlen:1`). It is what a display pins
//!   and what a pairing code is worked out from, so it can never change without pairing every display
//!   again. Its key is used only to make an intermediate (at the start, and when one is due), never to
//!   serve a request, so it need not be on the machine that serves, and `PrivateAuthority` never holds it.
//! - an **intermediate**: shorter-lived, signs the server's and the displays' certificates
//!   (`pathlen:0`). If it is lost or leaks, a new one is signed by the root and no display is paired
//!   again.
//!
//! What a certificate says is decided here, never by a request: the display's name and key, a short life,
//! and the one use it is for, proving who is connecting.

mod request;
mod storage;
#[cfg(test)]
mod tests;

use anyhow::{Context, anyhow, bail};
use chrono::{DateTime, Duration, Utc};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PublicKeyData, SanType, SerialNumber,
    SignatureAlgorithm,
};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use time::OffsetDateTime;
use x509_parser::prelude::FromDer;

pub use self::storage::{Create, open, rotate_intermediate};
use crate::domain::models::pairing::Fingerprint;
use crate::domain::services::certificate_authority::{
    CertificateAuthority, CertificateRequest, IssuedCertificate, RequestError,
};

const ROOT_NAME: &str = "E-ink home display root";
const INTERMEDIATE_NAME: &str = "E-ink home display issuing authority";
/// Long enough that it doesn't expire in the life of a display, which can't be told to trust a new root
/// without pairing again.
const ROOT_YEARS: i64 = 20;
/// Short enough to be replaced from time to time, which keeps the means of doing so in working order.
const INTERMEDIATE_YEARS: i64 = 5;
/// An intermediate with less than this left is replaced (see `storage`).
pub const ROTATE_WITHIN: Duration = Duration::days(365);
/// A certificate starts a little before it was made, so a display whose clock is a few minutes behind
/// doesn't find it "not yet valid".
const BACKDATE: Duration = Duration::hours(1);

/// The server's own certificate and key, for it to prove who it is to a display.
pub struct ServerIdentity {
    pub certificate: CertificateDer<'static>,
    /// When the certificate ends: the end asked for, or the intermediate's if that comes first.
    pub not_after: DateTime<Utc>,
    /// The intermediate that signed it, sent after it so a display can build the path to the root it
    /// pins without having kept the intermediate.
    pub intermediate: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
}

/// What a new authority is kept as (see `storage`).
pub struct AuthorityFiles {
    /// The root certificate. Public: it is what displays are given to trust the server by.
    pub root_certificate: String,
    /// The root's key. Used only to make intermediates; keep it somewhere safe, off the server if you can.
    pub root_key: String,
    /// The intermediate's certificate and then its key, in one file so they are always replaced together.
    pub intermediate: String,
}

/// The authority as it is used to serve. It holds the intermediate's key, not the root's.
pub struct PrivateAuthority {
    issuer: Issuer<'static, KeyPair>,
    root: Vec<u8>,
    intermediate: Vec<u8>,
    /// Earlier intermediates that have not ended yet: certificates they signed are still good.
    retired: Vec<Vec<u8>>,
    fingerprint: Fingerprint,
    root_not_after: DateTime<Utc>,
    intermediate_not_after: DateTime<Utc>,
}

/// A display's public key, in the form `rcgen` signs.
struct SubjectKey {
    point: Vec<u8>,
}

impl PublicKeyData for SubjectKey {
    fn der_bytes(&self) -> &[u8] {
        &self.point
    }

    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &PKCS_ECDSA_P256_SHA256
    }
}

/// An authority's certificate: `path_len` more authorities may stand below it.
fn authority_params(
    name: &str,
    now: DateTime<Utc>,
    years: i64,
    path_len: u8,
) -> anyhow::Result<CertificateParams> {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, name);
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(path_len));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = to_time(now - BACKDATE)?;
    params.not_after = to_time(now + Duration::days(365 * years))?;
    params.serial_number = Some(serial()?);
    params.use_authority_key_identifier_extension = true;
    Ok(params)
}

fn key_from_pem(pem: &str, what: &str) -> anyhow::Result<KeyPair> {
    let der = PrivatePkcs8KeyDer::from_pem_slice(pem.as_bytes())
        .map_err(|e| anyhow!("The {what} key is not a valid PEM private key: {e}"))?;
    KeyPair::from_pkcs8_der_and_sign_algo(&der, &PKCS_ECDSA_P256_SHA256)
        .with_context(|| format!("The {what} key is not a P-256 key"))
}

fn certificate_from_pem(pem: &str, what: &str) -> anyhow::Result<CertificateDer<'static>> {
    CertificateDer::from_pem_slice(pem.as_bytes())
        .map_err(|e| anyhow!("The {what} certificate is not valid PEM: {e}"))
}

fn end_of(parsed: &x509_parser::certificate::X509Certificate<'_>) -> anyhow::Result<DateTime<Utc>> {
    DateTime::from_timestamp(parsed.validity().not_after.timestamp(), 0)
        .context("A certificate has an impossible end date")
}

/// A new intermediate signed by the root, as the contents of the intermediate's file: its certificate and
/// then its key. This is the one thing the root's key is for.
pub fn new_intermediate(
    root_certificate_pem: &str,
    root_key_pem: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<String> {
    let root_certificate = certificate_from_pem(root_certificate_pem, "root")?;
    let root_key = key_from_pem(root_key_pem, "root")?;
    let (_, root) = x509_parser::parse_x509_certificate(&root_certificate)
        .map_err(|e| anyhow!("The root certificate can't be read: {e}"))?;
    if root.public_key().raw != root_key.subject_public_key_info().as_slice() {
        return Err(anyhow!(
            "The root key does not belong to the root certificate"
        ));
    }
    let root_end = end_of(&root)?;
    let root_issuer = Issuer::from_ca_cert_der(&root_certificate, root_key)
        .context("Failed to use the root certificate to sign with")?;

    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .context("Failed to generate the intermediate's key")?;
    let mut params = authority_params(INTERMEDIATE_NAME, now, INTERMEDIATE_YEARS, 0)?;
    // Never outlive the root: nothing below an ended authority is trusted.
    let end = (now + Duration::days(365 * INTERMEDIATE_YEARS)).min(root_end);
    params.not_after = to_time(end)?;
    let certificate = params
        .signed_by(&key, &root_issuer)
        .context("Failed to sign the intermediate's certificate")?;
    Ok(format!("{}{}", certificate.pem(), key.serialize_pem()))
}

impl PrivateAuthority {
    /// A new root and intermediate, and the three files' contents to keep them by.
    pub fn generate(now: DateTime<Utc>) -> anyhow::Result<(Self, AuthorityFiles)> {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .context("Failed to generate the root's key")?;
        let params = authority_params(ROOT_NAME, now, ROOT_YEARS, 1)?;
        let certificate = params
            .self_signed(&key)
            .context("Failed to sign the root's certificate")?;
        let (root_certificate, root_key) = (certificate.pem(), key.serialize_pem());
        let intermediate = new_intermediate(&root_certificate, &root_key, now)?;
        let authority = Self::from_pem(&root_certificate, &intermediate, &[], now)?;
        Ok((
            authority,
            AuthorityFiles {
                root_certificate,
                root_key,
                intermediate,
            },
        ))
    }

    /// An authority from its saved files, checked to belong together. `intermediate_pem` is the
    /// intermediate's certificate and key; `retired` are the PEM of earlier intermediates. Ones that
    /// have ended by `now` are dropped.
    pub fn from_pem(
        root_pem: &str,
        intermediate_pem: &str,
        retired: &[String],
        now: DateTime<Utc>,
    ) -> anyhow::Result<Self> {
        let root = certificate_from_pem(root_pem, "root")?;
        let (_, root_parsed) = x509_parser::parse_x509_certificate(&root)
            .map_err(|e| anyhow!("The root certificate can't be read: {e}"))?;
        if !root_parsed.is_ca() {
            return Err(anyhow!("The root certificate is not a CA certificate"));
        }
        root_parsed
            .verify_signature(None)
            .map_err(|_| anyhow!("The root certificate is not self-signed"))?;

        let intermediate = certificate_from_pem(intermediate_pem, "intermediate")?;
        let key = key_from_pem(intermediate_pem, "intermediate")?;
        let (_, parsed) = x509_parser::parse_x509_certificate(&intermediate)
            .map_err(|e| anyhow!("The intermediate certificate can't be read: {e}"))?;
        check_intermediate(&parsed, &root_parsed)?;
        if parsed.public_key().raw != key.subject_public_key_info().as_slice() {
            return Err(anyhow!(
                "The intermediate key does not belong to the intermediate certificate"
            ));
        }
        let mut earlier = Vec::new();
        for pem in retired {
            let der = certificate_from_pem(pem, "retired intermediate")?;
            let (_, old) = x509_parser::parse_x509_certificate(&der)
                .map_err(|e| anyhow!("A retired intermediate can't be read: {e}"))?;
            check_intermediate(&old, &root_parsed)?;
            if end_of(&old)? > now && der.as_ref() != intermediate.as_ref() {
                earlier.push(der.to_vec());
            }
        }
        let issuer = Issuer::from_ca_cert_der(&intermediate, key)
            .context("Failed to use the intermediate certificate to sign with")?;
        Ok(Self {
            issuer,
            fingerprint: Fingerprint::of(&root),
            root_not_after: end_of(&root_parsed)?,
            intermediate_not_after: end_of(&parsed)?,
            root: root.to_vec(),
            intermediate: intermediate.to_vec(),
            retired: earlier,
        })
    }

    /// A certificate for the server, under a new key, for `names` (host names or IP addresses). It
    /// is for proving the server's identity and nothing else, and short-lived: the key is made
    /// fresh each time and kept only in memory, so there is nothing to protect on disk.
    pub fn issue_server(
        &self,
        names: &[String],
        now: DateTime<Utc>,
        not_after: DateTime<Utc>,
    ) -> anyhow::Result<ServerIdentity> {
        if names.is_empty() {
            return Err(anyhow!("A server certificate needs at least one name"));
        }
        let not_after = self.limit_to_intermediate(now, not_after)?;
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .context("Failed to generate the server's key")?;
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, "E-ink home display server");
        params.subject_alt_names = names
            .iter()
            .map(|name| match name.parse::<std::net::IpAddr>() {
                Ok(address) => Ok(SanType::IpAddress(address)),
                Err(_) if is_host_name(name) => name
                    .as_str()
                    .try_into()
                    .map(SanType::DnsName)
                    .map_err(|e| anyhow!("{name:?} is not a usable host name: {e}")),
                Err(_) => Err(anyhow!("{name:?} is not a usable host name")),
            })
            .collect::<anyhow::Result<_>>()?;
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.not_before = to_time(now - BACKDATE)?;
        params.not_after = to_time(not_after)?;
        params.serial_number = Some(serial()?);
        params.use_authority_key_identifier_extension = true;
        let certificate = params
            .signed_by(&key, &self.issuer)
            .context("Failed to sign the server's certificate")?;
        Ok(ServerIdentity {
            certificate: certificate.der().clone(),
            not_after,
            intermediate: CertificateDer::from(self.intermediate.clone()),
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        })
    }

    /// The end a certificate made at `now` can have: the one asked for, or the intermediate's if that comes
    /// first. A certificate is only as good as the intermediate that signed it, so one that claimed longer would be
    /// refused when the intermediate ended, which looks like a fault on every display at once. Once the
    /// intermediate has ended nothing can be made under it at all, and the way out is to replace it.
    fn limit_to_intermediate(
        &self,
        now: DateTime<Utc>,
        not_after: DateTime<Utc>,
    ) -> anyhow::Result<DateTime<Utc>> {
        if self.intermediate_not_after <= now {
            bail!(
                "the intermediate certificate ended on {}; nothing can be signed under it. Put the root's key back \
                 and restart, or run the rotation, to replace it",
                self.intermediate_not_after.format("%Y-%m-%d")
            );
        }
        if not_after > self.intermediate_not_after {
            log::warn!(
                "A certificate asked to last until {} is cut short to {}, when the intermediate ends",
                not_after.format("%Y-%m-%d"),
                self.intermediate_not_after.format("%Y-%m-%d")
            );
        }
        Ok(not_after.min(self.intermediate_not_after))
    }

    /// When the root ends, after which nothing is trusted and every display has to be paired again.
    pub fn not_after(&self) -> DateTime<Utc> {
        self.root_not_after
    }

    /// When the intermediate in use ends. Before then it is replaced (see `rotate_intermediate`).
    pub fn intermediate_not_after(&self) -> DateTime<Utc> {
        self.intermediate_not_after
    }

    /// Every intermediate whose certificates are still good: the one in use, then earlier ones. A display
    /// need not send its intermediate; the server completes the path from these.
    pub fn intermediates(&self) -> Vec<&[u8]> {
        std::iter::once(&self.intermediate)
            .chain(&self.retired)
            .map(Vec::as_slice)
            .collect()
    }
}

/// Checks `intermediate` is an authority that may sign certificates but not further authorities, and that
/// `root` signed it.
fn check_intermediate(
    intermediate: &x509_parser::certificate::X509Certificate<'_>,
    root: &x509_parser::certificate::X509Certificate<'_>,
) -> anyhow::Result<()> {
    if !intermediate.is_ca() {
        return Err(anyhow!("The intermediate is not a CA certificate"));
    }
    let constraints = intermediate
        .basic_constraints()
        .ok()
        .flatten()
        .map(|c| c.value.path_len_constraint);
    if constraints != Some(Some(0)) {
        return Err(anyhow!(
            "The intermediate may sign more than certificates (its path length is not 0)"
        ));
    }
    intermediate
        .verify_signature(Some(root.public_key()))
        .map_err(|_| anyhow!("The intermediate was not signed by this root"))
}

impl CertificateAuthority for PrivateAuthority {
    fn certificate(&self) -> &[u8] {
        &self.root
    }

    fn intermediate(&self) -> Option<&[u8]> {
        Some(&self.intermediate)
    }

    fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    fn inspect(&self, der: &[u8]) -> Result<CertificateRequest, RequestError> {
        request::read(der)
    }

    fn issue(
        &self,
        request: &CertificateRequest,
        now: DateTime<Utc>,
        not_after: DateTime<Utc>,
    ) -> anyhow::Result<IssuedCertificate> {
        // Read again from the request itself: what is signed is what it says, not what the caller
        // passed alongside.
        let request = request::read(&request.der).context("Refused to certify the request")?;
        let not_after = self.limit_to_intermediate(now, not_after)?;
        let (_, spki) = x509_parser::x509::SubjectPublicKeyInfo::from_der(request.key.as_der())
            .map_err(|e| anyhow!("The request's key can't be read: {e}"))?;

        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, request.device.as_str());
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        params.not_before = to_time(now - BACKDATE)?;
        // A certificate holds whole seconds.
        let not_after = DateTime::from_timestamp(not_after.timestamp(), 0)
            .context("The certificate's end is out of range")?;
        params.not_after = to_time(not_after)?;
        let serial = serial()?;
        let serial_text = hex(&serial.to_bytes());
        params.serial_number = Some(serial);
        params.use_authority_key_identifier_extension = true;

        let certificate = params
            .signed_by(
                &SubjectKey {
                    point: spki.subject_public_key.data.to_vec(),
                },
                &self.issuer,
            )
            .context("Failed to sign the certificate")?;
        Ok(IssuedCertificate {
            der: certificate.der().to_vec(),
            serial: serial_text,
            not_after,
        })
    }
}

/// A plain host name: dot-separated labels of letters, digits and hyphens (and underscores, which
/// service names use), none empty, none starting or ending with a hyphen, no wildcard. The certificate
/// library takes anything that is ASCII, a space included.
fn is_host_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        })
}

fn to_time(at: DateTime<Utc>) -> anyhow::Result<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(at.timestamp()).context("A date is out of range")
}

/// 128 random bits with the top bit clear, so it is a positive number (RFC 5280 asks for at most 20
/// bytes and unique per authority; randomness gives the second).
fn serial() -> anyhow::Result<SerialNumber> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("No randomness for a serial number: {e}"))?;
    bytes[0] = (bytes[0] & 0x7f) | 0x40;
    Ok(SerialNumber::from_slice(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
