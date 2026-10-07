//! The server's own certificate authority, made with `rcgen`. It signs the certificates of the displays
//! that have been approved (and, later, the server's own), so nothing here depends on a public
//! authority or a domain name.
//!
//! The authority is a P-256 key and a long-lived certificate kept in two files (see `storage`). What a
//! certificate says is decided here, never by a request: the display's name and key, a short life, and
//! the one use it is for, proving who is connecting.

mod request;
mod storage;
#[cfg(test)]
mod tests;

use anyhow::{Context, anyhow};
use chrono::{DateTime, Duration, Utc};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PublicKeyData, SerialNumber,
    SignatureAlgorithm,
};
use time::OffsetDateTime;
use x509_parser::prelude::FromDer;

pub use self::storage::open;
use crate::domain::models::pairing::Fingerprint;
use crate::domain::services::certificate_authority::{
    CertificateAuthority, CertificateRequest, IssuedCertificate, RequestError,
};

/// The authority's name, as it shows in a certificate viewer.
const NAME: &str = "E-ink home display authority";
/// Long enough that it doesn't expire in the life of a display, which can't be told to trust a new one
/// without pairing again. Its key never leaves this machine, so a long life costs little.
const AUTHORITY_YEARS: i64 = 20;
/// A certificate starts a little before it was made, so a display whose clock is a few minutes behind
/// doesn't find it "not yet valid".
const BACKDATE: Duration = Duration::hours(1);

pub struct PrivateAuthority {
    issuer: Issuer<'static, KeyPair>,
    certificate: Vec<u8>,
    fingerprint: Fingerprint,
    not_after: DateTime<Utc>,
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

impl PrivateAuthority {
    /// A new authority, and the two files' contents (certificate, then key) to keep it by.
    pub fn generate(now: DateTime<Utc>) -> anyhow::Result<(Self, String, String)> {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .context("Failed to generate the authority's key")?;
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, NAME);
        // It may sign certificates for displays and for nothing beneath them: not for another authority.
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.not_before = to_time(now - BACKDATE)?;
        params.not_after = to_time(now + Duration::days(365 * AUTHORITY_YEARS))?;
        params.serial_number = Some(serial()?);
        let certificate = params
            .self_signed(&key)
            .context("Failed to sign the authority's certificate")?;
        let (certificate_pem, key_pem) = (certificate.pem(), key.serialize_pem());
        let authority = Self::from_pem(&certificate_pem, &key_pem)?;
        Ok((authority, certificate_pem, key_pem))
    }

    /// An authority from its saved files, checked to belong together and to be an authority.
    pub fn from_pem(certificate_pem: &str, key_pem: &str) -> anyhow::Result<Self> {
        use rustls_pki_types::CertificateDer;
        use rustls_pki_types::pem::PemObject;

        let certificate = CertificateDer::from_pem_slice(certificate_pem.as_bytes())
            .map_err(|e| anyhow!("The authority certificate is not valid PEM: {e}"))?;
        let key = KeyPair::from_pem(key_pem).context("The authority key is not a valid key")?;

        let (_, parsed) = x509_parser::parse_x509_certificate(&certificate)
            .map_err(|e| anyhow!("The authority certificate can't be read: {e}"))?;
        if !parsed.is_ca() {
            return Err(anyhow!("The authority certificate is not a CA certificate"));
        }
        if parsed.public_key().raw != key.subject_public_key_info().as_slice() {
            return Err(anyhow!(
                "The authority key does not belong to the authority certificate"
            ));
        }
        let not_after = DateTime::from_timestamp(parsed.validity().not_after.timestamp(), 0)
            .context("The authority certificate has an impossible end date")?;
        let issuer = Issuer::from_ca_cert_der(&certificate, key)
            .context("Failed to use the authority certificate to sign with")?;
        Ok(Self {
            issuer,
            fingerprint: Fingerprint::of(&certificate),
            certificate: certificate.to_vec(),
            not_after,
        })
    }

    /// When the authority's certificate ends, after which nothing it signed is trusted.
    pub fn not_after(&self) -> DateTime<Utc> {
        self.not_after
    }
}

impl CertificateAuthority for PrivateAuthority {
    fn certificate(&self) -> &[u8] {
        &self.certificate
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
