//! The authority against real keys and real requests: what it accepts, what it refuses, and exactly
//! what ends up in a certificate.

use chrono::TimeZone;
use rcgen::{
    Attribute, BasicConstraints, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
    PKCS_ECDSA_P384_SHA384, SanType,
};
use x509_parser::prelude::*;

use super::*;
use crate::domain::models::device_id::DeviceId;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 8, 12, 0, 0).unwrap()
}

fn authority() -> PrivateAuthority {
    PrivateAuthority::generate(now()).unwrap().0
}

/// What a display sends: a request for `name`, signed with its own key.
fn display(name: &str) -> (KeyPair, Vec<u8>) {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let request = request_with(&key, params(name), vec![]);
    (key, request)
}

fn params(name: &str) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, name);
    params
}

fn request_with(key: &KeyPair, params: CertificateParams, attributes: Vec<Attribute>) -> Vec<u8> {
    params
        .serialize_request_with_attributes(key, attributes)
        .unwrap()
        .der()
        .to_vec()
}

/// A challenge password as PKCS #9 has it: a UTF8String inside a SET, which `rcgen` takes as DER.
fn challenge_password(text: &str) -> Attribute {
    assert!(text.len() < 100, "short lengths only");
    let mut value = vec![0x31, text.len() as u8 + 2, 0x0c, text.len() as u8];
    value.extend_from_slice(text.as_bytes());
    Attribute {
        oid: &[1, 2, 840, 113549, 1, 9, 7],
        values: value,
    }
}

fn issue(authority: &PrivateAuthority, request: &[u8]) -> IssuedCertificate {
    let request = authority.inspect(request).unwrap();
    authority
        .issue(&request, now(), now() + Duration::days(365))
        .unwrap()
}

#[test]
fn a_display_request_is_read_for_its_name_and_key() {
    let (key, request) = display("reterminal-e1003-a1b2c3");
    let read = authority().inspect(&request).unwrap();
    assert_eq!(
        read.device,
        DeviceId::parse("reterminal-e1003-a1b2c3").unwrap()
    );
    assert_eq!(read.key.as_der(), key.subject_public_key_info().as_slice());
    assert_eq!(read.channel_binding, None);
    assert_eq!(read.der, request);
}

#[test]
fn the_connection_it_was_signed_on_is_read_from_the_challenge_password() {
    // base64 of the 32 bytes 0x00..0x1f, as a display would put its connection's exporter value.
    let binding: Vec<u8> = (0u8..32).collect();
    let text = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &binding);
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let request = request_with(&key, params("kitchen"), vec![challenge_password(&text)]);
    let read = authority().inspect(&request).unwrap();
    assert_eq!(read.channel_binding, Some(binding));
}

#[test]
fn a_challenge_password_that_is_not_base64_is_refused() {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let request = request_with(
        &key,
        params("kitchen"),
        vec![challenge_password("not base64!")],
    );
    assert!(matches!(
        authority().inspect(&request),
        Err(RequestError::Malformed(_))
    ));
}

#[test]
fn a_request_altered_after_signing_is_refused() {
    let (_, mut request) = display("kitchen");
    // The name is inside what the signature covers.
    let at = request.windows(7).position(|w| w == b"kitchen").unwrap();
    request[at] = b'K';
    assert_eq!(
        authority().inspect(&request),
        Err(RequestError::BadSignature)
    );
}

#[test]
fn a_request_for_a_key_its_sender_does_not_hold_is_refused() {
    // One display's request with another's public key put in its place. The same length, so it is still
    // a well-formed request, and only the signature can show it was not made with that key.
    let (a_key, request) = display("kitchen");
    let (b_key, _) = display("kitchen");
    let point = |key: &KeyPair| key.public_key_raw().to_vec();
    let at = request
        .windows(65)
        .position(|w| w == point(&a_key))
        .expect("the key is in the request");
    let mut swapped = request.clone();
    swapped[at..at + 65].copy_from_slice(&point(&b_key));
    assert_eq!(
        authority().inspect(&swapped),
        Err(RequestError::BadSignature)
    );
}

#[test]
fn garbage_and_truncated_requests_are_refused() {
    let (_, request) = display("kitchen");
    for bad in [
        &b""[..],
        b"not a request",
        &request[..request.len() / 2],
        &[request.as_slice(), b"extra"].concat(),
    ] {
        assert!(
            matches!(authority().inspect(bad), Err(RequestError::Malformed(_))),
            "{} bytes",
            bad.len()
        );
    }
}

#[test]
fn only_p256_keys_are_certified() {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).unwrap();
    let request = request_with(&key, params("kitchen"), vec![]);
    assert!(matches!(
        authority().inspect(&request),
        Err(RequestError::UnsupportedKey(_))
    ));
}

#[test]
fn a_request_needs_exactly_one_valid_name() {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    for name in ["", "has space", "x".repeat(40).as_str()] {
        let request = request_with(&key, params(name), vec![]);
        assert!(
            matches!(
                authority().inspect(&request),
                Err(RequestError::BadDevice(_))
            ),
            "{name:?}"
        );
    }
    let mut none = params("kitchen");
    none.distinguished_name = DistinguishedName::new();
    assert!(matches!(
        authority().inspect(&request_with(&key, none, vec![])),
        Err(RequestError::BadDevice(_))
    ));
    let mut two = params("kitchen");
    // `rcgen` keeps one value per name type, so the second goes in under the same OID spelled out.
    two.distinguished_name
        .push(DnType::CustomDnType(vec![2, 5, 4, 3]), "hall");
    assert!(matches!(
        authority().inspect(&request_with(&key, two, vec![])),
        Err(RequestError::BadDevice(_))
    ));
}

fn parse(certificate: &IssuedCertificate) -> X509Certificate<'_> {
    X509Certificate::from_der(&certificate.der).unwrap().1
}

#[test]
fn a_certificate_names_the_display_and_is_valid_for_the_time_asked() {
    let authority = authority();
    let (key, request) = display("kitchen");
    let issued = issue(&authority, &request);
    let cert = parse(&issued);
    assert_eq!(
        cert.subject()
            .iter_common_name()
            .next()
            .unwrap()
            .as_str()
            .unwrap(),
        "kitchen"
    );
    assert_eq!(
        cert.issuer()
            .iter_common_name()
            .next()
            .unwrap()
            .as_str()
            .unwrap(),
        NAME
    );
    assert_eq!(
        cert.public_key().raw,
        key.subject_public_key_info().as_slice()
    );
    assert_eq!(
        cert.validity().not_after.timestamp(),
        (now() + Duration::days(365)).timestamp()
    );
    // Starts a little early to allow for a display clock that is behind.
    assert_eq!(
        cert.validity().not_before.timestamp(),
        (now() - BACKDATE).timestamp()
    );
    assert_eq!(issued.not_after, now() + Duration::days(365));
}

#[test]
fn a_certificate_is_signed_by_the_authority() {
    let authority = authority();
    let (_, request) = display("kitchen");
    let issued = issue(&authority, &request);
    let (_, ca) = X509Certificate::from_der(authority.certificate()).unwrap();
    parse(&issued)
        .verify_signature(Some(ca.public_key()))
        .unwrap();
    // And by nothing else.
    let other = self::authority();
    let (_, other_ca) = X509Certificate::from_der(other.certificate()).unwrap();
    assert!(
        parse(&issued)
            .verify_signature(Some(other_ca.public_key()))
            .is_err()
    );
}

#[test]
fn a_certificate_is_for_proving_who_connects_and_nothing_more() {
    let authority = authority();
    let (_, request) = display("kitchen");
    let issued = issue(&authority, &request);
    let cert = parse(&issued);
    assert!(!cert.is_ca());
    let usage = cert.key_usage().unwrap().unwrap().value;
    assert!(usage.digital_signature());
    assert!(!usage.key_cert_sign() && !usage.key_encipherment());
    let eku = cert.extended_key_usage().unwrap().unwrap().value;
    assert!(eku.client_auth && !eku.server_auth && !eku.any && eku.other.is_empty());
    assert!(cert.subject_alternative_name().unwrap().is_none());
}

#[test]
fn what_a_request_asks_for_beyond_its_name_and_key_is_not_given() {
    // A hostile display asking to be an authority, a server, and any name it likes.
    let authority = authority();
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut greedy = params("kitchen");
    greedy.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    greedy.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    greedy.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    greedy.subject_alt_names = vec![SanType::DnsName("bank.example".try_into().unwrap())];
    let issued = issue(&authority, &request_with(&key, greedy, vec![]));
    let cert = parse(&issued);
    assert!(!cert.is_ca());
    assert!(cert.subject_alternative_name().unwrap().is_none());
    let eku = cert.extended_key_usage().unwrap().unwrap().value;
    assert!(eku.client_auth && !eku.server_auth);
    assert!(!cert.key_usage().unwrap().unwrap().value.key_cert_sign());
}

#[test]
fn issuing_goes_by_the_request_not_by_what_the_caller_says_about_it() {
    let authority = authority();
    let (_, request) = display("kitchen");
    let mut claimed = authority.inspect(&request).unwrap();
    claimed.device = DeviceId::parse("somebody-else").unwrap();
    let issued = authority
        .issue(&claimed, now(), now() + Duration::days(1))
        .unwrap();
    let cert = parse(&issued);
    assert_eq!(
        cert.subject()
            .iter_common_name()
            .next()
            .unwrap()
            .as_str()
            .unwrap(),
        "kitchen"
    );
}

#[test]
fn a_request_that_does_not_check_out_is_not_issued_even_if_it_was_passed_on() {
    let authority = authority();
    let (_, mut request) = display("kitchen");
    let claimed = authority.inspect(&request).unwrap();
    let at = request.windows(7).position(|w| w == b"kitchen").unwrap();
    request[at] = b'K';
    let tampered = CertificateRequest {
        der: request,
        ..claimed
    };
    assert!(
        authority
            .issue(&tampered, now(), now() + Duration::days(1))
            .is_err()
    );
}

#[test]
fn no_two_certificates_share_a_serial_number() {
    let authority = authority();
    let (_, request) = display("kitchen");
    let serials: std::collections::HashSet<String> = (0..50)
        .map(|_| issue(&authority, &request).serial)
        .collect();
    assert_eq!(serials.len(), 50);
    for serial in &serials {
        assert_eq!(serial.len(), 32);
        assert!(
            serial.starts_with(['4', '5', '6', '7']),
            "positive and not short: {serial}"
        );
    }
}

#[test]
fn the_serial_in_the_certificate_is_the_one_reported() {
    let authority = authority();
    let (_, request) = display("kitchen");
    let issued = issue(&authority, &request);
    assert_eq!(
        parse(&issued).raw_serial_as_string().replace(':', ""),
        issued.serial
    );
}

#[test]
fn the_authority_may_sign_displays_but_not_further_authorities() {
    let authority = authority();
    let (_, ca) = X509Certificate::from_der(authority.certificate()).unwrap();
    let constraints = ca.basic_constraints().unwrap().unwrap().value;
    assert!(constraints.ca);
    assert_eq!(constraints.path_len_constraint, Some(0));
    let usage = ca.key_usage().unwrap().unwrap().value;
    assert!(usage.key_cert_sign() && !usage.digital_signature());
    assert_eq!(
        authority.not_after(),
        now() + Duration::days(365 * AUTHORITY_YEARS)
    );
}

fn names() -> Vec<String> {
    ["eink.local", "192.168.1.5", "::1"]
        .map(str::to_owned)
        .to_vec()
}

#[test]
fn the_servers_certificate_names_the_server_and_is_for_proving_it_to_a_display() {
    let authority = authority();
    let identity = authority
        .issue_server(&names(), now(), now() + Duration::days(90))
        .unwrap();
    let (_, cert) = X509Certificate::from_der(&identity.certificate).unwrap();
    assert!(!cert.is_ca());
    let alt = cert.subject_alternative_name().unwrap().unwrap().value;
    let found: Vec<String> = alt
        .general_names
        .iter()
        .map(|n| match n {
            GeneralName::DNSName(name) => name.to_string(),
            GeneralName::IPAddress(bytes) => format!("{bytes:?}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(found[0], "eink.local");
    assert_eq!(found[1], "[192, 168, 1, 5]");
    assert_eq!(found.len(), 3, "{found:?}");
    let eku = cert.extended_key_usage().unwrap().unwrap().value;
    assert!(eku.server_auth && !eku.client_auth && !eku.any);
    let usage = cert.key_usage().unwrap().unwrap().value;
    assert!(usage.digital_signature() && !usage.key_cert_sign());
    assert_eq!(
        cert.validity().not_after.timestamp(),
        (now() + Duration::days(90)).timestamp()
    );
}

#[test]
fn the_servers_certificate_is_signed_by_the_authority_and_matches_its_key() {
    let authority = authority();
    let identity = authority
        .issue_server(&names(), now(), now() + Duration::days(90))
        .unwrap();
    let (_, ca) = X509Certificate::from_der(authority.certificate()).unwrap();
    let (_, cert) = X509Certificate::from_der(&identity.certificate).unwrap();
    cert.verify_signature(Some(ca.public_key())).unwrap();

    let rustls_pki_types::PrivateKeyDer::Pkcs8(pkcs8) = &identity.key else {
        panic!("expected a PKCS #8 key");
    };
    let key = KeyPair::from_pkcs8_der_and_sign_algo(pkcs8, &PKCS_ECDSA_P256_SHA256).unwrap();
    assert_eq!(
        cert.public_key().raw,
        key.subject_public_key_info().as_slice()
    );
}

#[test]
fn each_server_certificate_has_a_key_of_its_own() {
    let authority = authority();
    let one = authority
        .issue_server(&names(), now(), now() + Duration::days(90))
        .unwrap();
    let two = authority
        .issue_server(&names(), now(), now() + Duration::days(90))
        .unwrap();
    assert_ne!(one.certificate.as_ref(), two.certificate.as_ref());
}

#[test]
fn a_server_certificate_needs_usable_names() {
    let authority = authority();
    let long = "a".repeat(64);
    for bad in [
        vec![],
        vec!["has space".to_owned()],
        vec!["bad\u{e9}name".to_owned()],
        vec!["*.example.com".to_owned()],
        vec!["-leading.example".to_owned()],
        vec!["trailing-.example".to_owned()],
        vec!["double..dot".to_owned()],
        vec![format!("{long}.example")],
        // One good name does not excuse a bad one.
        vec!["fine.local".to_owned(), "not fine".to_owned()],
    ] {
        assert!(
            authority
                .issue_server(&bad, now(), now() + Duration::days(90))
                .is_err(),
            "{bad:?}"
        );
    }
    for good in [
        "eink",
        "eink.local",
        "my-host.example.com",
        "_svc.local",
        "localhost",
    ] {
        assert!(
            authority
                .issue_server(&[good.to_owned()], now(), now() + Duration::days(90))
                .is_ok(),
            "{good}"
        );
    }
}

mod saved {
    use super::*;

    #[test]
    fn never_means_an_empty_directory_is_an_error_and_is_left_empty() {
        let parent = tempfile::tempdir().unwrap();
        let directory = parent.path().join("pki");
        let error = open(&directory, Create::Never)
            .err()
            .expect("should refuse")
            .to_string();
        assert!(error.contains("no certificate authority"), "{error}");
        assert!(!directory.exists(), "nothing was made");
        // An authority that is there is opened all the same.
        open(&directory, Create::IfMissing).unwrap();
        open(&directory, Create::Never).unwrap();
    }

    #[test]
    fn a_new_authority_is_made_once_and_found_again() {
        let directory = tempfile::tempdir().unwrap();
        let first = open(directory.path(), Create::IfMissing).unwrap();
        let again = open(directory.path(), Create::IfMissing).unwrap();
        assert_eq!(first.fingerprint(), again.fingerprint());
        assert_eq!(first.certificate(), again.certificate());
        // The reopened one signs for the same authority.
        let (_, request) = display("kitchen");
        let issued = issue(&again, &request);
        let (_, ca) = X509Certificate::from_der(first.certificate()).unwrap();
        parse(&issued)
            .verify_signature(Some(ca.public_key()))
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_key_can_be_read_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        open(directory.path(), Create::IfMissing).unwrap();
        let mode = |name: &str| {
            std::fs::metadata(directory.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("authority.key"), 0o600);
    }

    #[test]
    fn a_missing_half_is_an_error_not_a_fresh_start() {
        for missing in ["authority.key", "authority.pem"] {
            let directory = tempfile::tempdir().unwrap();
            let original = open(directory.path(), Create::IfMissing).unwrap();
            std::fs::remove_file(directory.path().join(missing)).unwrap();
            let error = open(directory.path(), Create::IfMissing)
                .err()
                .expect("should refuse")
                .to_string();
            assert!(error.contains("not making a new authority"), "{error}");
            // Nothing was made or overwritten.
            assert_eq!(
                directory.path().join("authority.pem").exists(),
                missing != "authority.pem"
            );
            drop(original);
        }
    }

    #[test]
    fn a_key_that_belongs_to_another_authority_is_refused() {
        let (_, certificate, _) = PrivateAuthority::generate(now()).unwrap();
        let (_, _, other_key) = PrivateAuthority::generate(now()).unwrap();
        let error = PrivateAuthority::from_pem(&certificate, &other_key)
            .err()
            .expect("should refuse")
            .to_string();
        assert!(error.contains("does not belong"), "{error}");
    }

    #[test]
    fn a_certificate_that_is_not_an_authority_is_refused() {
        // A display's certificate, with its key, is not an authority.
        let authority = authority();
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let request = request_with(&key, params("kitchen"), vec![]);
        let issued = issue(&authority, &request);
        let pem = pem_of(&issued.der);
        let error = PrivateAuthority::from_pem(&pem, &key.serialize_pem())
            .err()
            .expect("should refuse")
            .to_string();
        assert!(error.contains("not a CA"), "{error}");
    }

    fn pem_of(der: &[u8]) -> String {
        let body = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, der);
        let lines: Vec<&str> = body
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            lines.join("\n")
        )
    }

    #[test]
    fn garbage_in_the_files_is_an_error_naming_the_directory() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("authority.pem"), "nonsense").unwrap();
        std::fs::write(directory.path().join("authority.key"), "nonsense").unwrap();
        let error = format!(
            "{:#}",
            open(directory.path(), Create::IfMissing)
                .err()
                .expect("should refuse")
        );
        assert!(error.contains("not usable"), "{error}");
    }
}
