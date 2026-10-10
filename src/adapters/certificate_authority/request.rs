//! Reads a certificate request (PKCS #10, RFC 2986) from a display, and checks it.
//!
//! The request is the display's, so nothing in it is taken on trust beyond what it can prove: that its
//! sender holds the private key (the signature), and the name and key it asks to be certified. Whatever
//! else a request asks for (extensions such as `CA:TRUE`, a server's key usage, alternative names) is
//! ignored, never copied into the certificate, so a request can't ask for more than a display is given.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use x509_parser::certification_request::X509CertificationRequest;
use x509_parser::cri_attributes::ParsedCriAttribute;
use x509_parser::prelude::FromDer;

use crate::domain::models::device_id::DeviceId;
use crate::domain::models::pairing::PublicKey;
use crate::domain::models::profile::DeviceProfile;
use crate::domain::services::certificate_authority::{CertificateRequest, RequestError};

/// `id-ecPublicKey`, `prime256v1` (P-256), and `ecdsa-with-SHA256`: the one kind of key and signature
/// the displays are asked to use (RFC 9908 lets the server say so). One kind is enough, and every
/// extra one is more code that reads what the network sends. The signature check that follows also
/// refuses a point that isn't a well-formed uncompressed P-256 one, so there is no length check here.
const EC_PUBLIC_KEY: &str = "1.2.840.10045.2.1";
const P256: &str = "1.2.840.10045.3.1.7";
const ECDSA_SHA256: &str = "1.2.840.10045.4.3.2";

pub fn read(der: &[u8]) -> Result<CertificateRequest, RequestError> {
    let (rest, request) = X509CertificationRequest::from_der(der)
        .map_err(|e| RequestError::Malformed(e.to_string()))?;
    if !rest.is_empty() {
        return Err(RequestError::Malformed("data after the request".to_owned()));
    }
    let info = &request.certification_request_info;
    let key = &info.subject_pki;

    // RFC 2986 section 4.1: the version "shall be 0" (v1). Anything else is a request this was not written for,
    // and it is refused before anything in it is looked at.
    if info.version.0 != 0 {
        return Err(RequestError::Malformed(format!(
            "the request is version {}, and only version 0 is defined",
            info.version.0
        )));
    }

    // Only the kind of key and signature that is asked for. Checked before the signature, which is
    // what would otherwise pick the algorithm from whatever the request says.
    if key.algorithm.algorithm.to_id_string() != EC_PUBLIC_KEY
        || key
            .algorithm
            .parameters
            .as_ref()
            .and_then(|p| p.as_oid().ok())
            .map(|oid| oid.to_id_string())
            .as_deref()
            != Some(P256)
    {
        return Err(RequestError::UnsupportedKey(
            "only ECDSA keys on the P-256 curve are certified".to_owned(),
        ));
    }
    if request.signature_algorithm.algorithm.to_id_string() != ECDSA_SHA256 {
        return Err(RequestError::UnsupportedKey(
            "the request must be signed with ECDSA and SHA-256".to_owned(),
        ));
    }
    request
        .verify_signature()
        .map_err(|_| RequestError::BadSignature)?;

    let mut names = info.subject.iter_common_name();
    let name = names
        .next()
        .ok_or_else(|| RequestError::BadDevice("no common name".to_owned()))?;
    if names.next().is_some() {
        return Err(RequestError::BadDevice(
            "more than one common name".to_owned(),
        ));
    }
    let name = name
        .as_str()
        .map_err(|_| RequestError::BadDevice("the name is not text".to_owned()))?;
    let device = DeviceId::parse(name).map_err(|e| RequestError::BadDevice(e.to_string()))?;

    let profile = profile(&request, &device);
    Ok(CertificateRequest {
        device,
        key: PublicKey::from_der(key.raw.to_vec()),
        channel_binding: channel_binding(&request)?,
        profile,
        der: der.to_vec(),
    })
}

/// The extension in which a display says what it is: a private OID (an RFC 4122 UUID under `2.25`, which needs no
/// registration), in the request's `extensionRequest` attribute (RFC 2985) like any extension a request asks for. The
/// display's copy is `home_display/core/wire.h`.
#[cfg(test)]
pub(crate) const PROFILE_OID: &str = "2.25.110978727574289354506863863824604692215";

/// The same OID's value octets (without the tag and length). Compared instead of the dotted text: the library cannot
/// write an OID with an arc this large (a UUID is 128 bits) as text, and gives its bytes.
pub(crate) const PROFILE_OID_VALUE: [u8; 20] = [
    0x69, 0x81, 0xA6, 0xFD, 0xDC, 0xED, 0xFD, 0x99, 0xC2, 0x81, 0xB7, 0x88, 0x9C, 0xC9, 0x8D, 0xBB,
    0xAE, 0x9D, 0xFD, 0x77,
];

/// The version of the profile this reads. A newer one is left out, not guessed at.
pub(crate) const PROFILE_VERSION: u32 = 1;

/// What the display says it is, if it said so and the whole of it is usable. Never an error: a display whose profile
/// cannot be read is still a display, and a firmware mistake must not lock it out. Like every other extension a request
/// asks for, it is read and not copied into the certificate.
fn profile(request: &X509CertificationRequest<'_>, device: &DeviceId) -> Option<DeviceProfile> {
    let mut found: Option<&[u8]> = None;
    for attribute in request.certification_request_info.attributes() {
        let ParsedCriAttribute::ExtensionRequest(requested) = attribute.parsed_attribute() else {
            continue;
        };
        for extension in &requested.extensions {
            if extension.oid.as_bytes() != PROFILE_OID_VALUE {
                continue;
            }
            if found.replace(extension.value).is_some() {
                log::warn!("{device}: ignoring its profile, which it gave twice");
                return None;
            }
        }
    }
    match read_profile(found?) {
        Ok(profile) => Some(profile),
        Err(why) => {
            log::warn!("{device}: ignoring its profile: {why}");
            None
        }
    }
}

/// `SEQUENCE { INTEGER version, UTF8String model, INTEGER width, INTEGER height, INTEGER levels,
/// SEQUENCE OF UTF8String formats, UTF8String firmware }`.
fn read_profile(der: &[u8]) -> Result<DeviceProfile, String> {
    use x509_parser::der_parser::der::parse_der;
    let (rest, object) = parse_der(der).map_err(|e| format!("it is not DER: {e}"))?;
    if !rest.is_empty() {
        return Err("data after the profile".to_owned());
    }
    let items = object
        .as_sequence()
        .map_err(|_| "it is not a sequence".to_owned())?;
    let [version, model, width, height, levels, formats, firmware] = items.as_slice() else {
        return Err(format!("it has {} parts, and a profile has 7", items.len()));
    };
    let number = |object: &x509_parser::der_parser::der::DerObject<'_>, what: &str| {
        object
            .as_u32()
            .map_err(|_| format!("the {what} is not a number"))
    };
    let text = |object: &x509_parser::der_parser::der::DerObject<'_>, what: &str| {
        object
            .as_str()
            .map(str::to_owned)
            .map_err(|_| format!("the {what} is not text"))
    };
    let version = number(version, "version")?;
    if version != PROFILE_VERSION {
        return Err(format!(
            "it is version {version}, and this reads version {PROFILE_VERSION}"
        ));
    }
    let formats = formats
        .as_sequence()
        .map_err(|_| "the formats are not a sequence".to_owned())?
        .iter()
        .map(|format| text(format, "format"))
        .collect::<Result<Vec<_>, _>>()?;
    DeviceProfile::new(
        &text(model, "model")?,
        &text(firmware, "firmware")?,
        number(width, "width")?,
        number(height, "height")?,
        number(levels, "levels")?,
        formats,
    )
    .map_err(|e| e.to_string())
}

/// The proof that the request was signed on one particular connection: RFC 7030 section 3.5 puts it
/// in the request's challenge-password, as base64.
fn channel_binding(
    request: &X509CertificationRequest<'_>,
) -> Result<Option<Vec<u8>>, RequestError> {
    let mut found = request
        .certification_request_info
        .attributes()
        .iter()
        .filter_map(|attribute| match attribute.parsed_attribute() {
            ParsedCriAttribute::ChallengePassword(password) => Some(&password.0),
            _ => None,
        });
    let Some(text) = found.next() else {
        return Ok(None);
    };
    if found.next().is_some() {
        return Err(RequestError::Malformed(
            "more than one challenge password".to_owned(),
        ));
    }
    STANDARD
        .decode(text)
        .map(Some)
        .map_err(|_| RequestError::Malformed("the challenge password is not base64".to_owned()))
}
