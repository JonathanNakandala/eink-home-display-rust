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
        INTERMEDIATE_NAME
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
fn a_certificate_is_signed_by_the_intermediate_and_by_nothing_else() {
    let authority = authority();
    let (_, request) = display("kitchen");
    let issued = issue(&authority, &request);
    let (_, intermediate) = X509Certificate::from_der(authority.intermediate().unwrap()).unwrap();
    parse(&issued)
        .verify_signature(Some(intermediate.public_key()))
        .unwrap();
    // Not by the root directly: the root signs intermediates only.
    let (_, root) = X509Certificate::from_der(authority.certificate()).unwrap();
    assert!(
        parse(&issued)
            .verify_signature(Some(root.public_key()))
            .is_err()
    );
    // And not by another authority's intermediate.
    let other = self::authority();
    let (_, other_intermediate) = X509Certificate::from_der(other.intermediate().unwrap()).unwrap();
    assert!(
        parse(&issued)
            .verify_signature(Some(other_intermediate.public_key()))
            .is_err()
    );
    // It says which key signed it, so a path can be found.
    let authority_key = parse(&issued)
        .get_extension_unique(&oid_registry::OID_X509_EXT_AUTHORITY_KEY_IDENTIFIER)
        .unwrap()
        .expect("an authority key identifier")
        .value
        .to_vec();
    let subject_key = intermediate
        .get_extension_unique(&oid_registry::OID_X509_EXT_SUBJECT_KEY_IDENTIFIER)
        .unwrap()
        .expect("a subject key identifier")
        .value
        .to_vec();
    assert!(
        authority_key
            .windows(subject_key.len() - 2)
            .any(|w| w == &subject_key[2..]),
        "the leaf's authority key identifier should be the intermediate's subject key identifier"
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
fn the_root_signs_one_level_of_authority_and_the_intermediate_none() {
    let authority = authority();
    let (_, root) = X509Certificate::from_der(authority.certificate()).unwrap();
    let constraints = root.basic_constraints().unwrap().unwrap().value;
    assert!(constraints.ca);
    assert_eq!(constraints.path_len_constraint, Some(1));
    let usage = root.key_usage().unwrap().unwrap().value;
    assert!(usage.key_cert_sign() && !usage.digital_signature());
    root.verify_signature(None)
        .expect("the root is self-signed");
    assert_eq!(
        authority.not_after(),
        now() + Duration::days(365 * ROOT_YEARS)
    );

    let (_, intermediate) = X509Certificate::from_der(authority.intermediate().unwrap()).unwrap();
    let constraints = intermediate.basic_constraints().unwrap().unwrap().value;
    assert!(constraints.ca);
    assert_eq!(constraints.path_len_constraint, Some(0));
    let usage = intermediate.key_usage().unwrap().unwrap().value;
    assert!(usage.key_cert_sign() && !usage.digital_signature());
    intermediate
        .verify_signature(Some(root.public_key()))
        .expect("the root signed it");
    assert_eq!(
        authority.intermediate_not_after(),
        now() + Duration::days(365 * INTERMEDIATE_YEARS)
    );
}

#[test]
fn the_fingerprint_a_pairing_code_is_made_from_is_the_roots() {
    let authority = authority();
    assert_eq!(
        authority.fingerprint(),
        Fingerprint::of(authority.certificate())
    );
    assert_ne!(
        authority.fingerprint(),
        Fingerprint::of(authority.intermediate().unwrap())
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
fn the_servers_certificate_comes_with_its_intermediate_and_the_path_to_the_root_checks_out() {
    let authority = authority();
    let identity = authority
        .issue_server(&names(), now(), now() + Duration::days(90))
        .unwrap();
    let (_, root) = X509Certificate::from_der(authority.certificate()).unwrap();
    let (_, intermediate) = X509Certificate::from_der(&identity.intermediate).unwrap();
    let (_, cert) = X509Certificate::from_der(&identity.certificate).unwrap();
    assert_eq!(
        identity.intermediate.as_ref(),
        authority.intermediate().unwrap()
    );
    cert.verify_signature(Some(intermediate.public_key()))
        .unwrap();
    intermediate
        .verify_signature(Some(root.public_key()))
        .unwrap();

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

    fn files(directory: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

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
        assert_eq!(
            files(directory.path()),
            ["intermediate.pem", "root.key", "root.pem"]
        );
        let again = open(directory.path(), Create::IfMissing).unwrap();
        assert_eq!(first.fingerprint(), again.fingerprint());
        assert_eq!(first.certificate(), again.certificate());
        assert_eq!(first.intermediate(), again.intermediate());
        // The reopened one signs under the same root.
        let (_, request) = display("kitchen");
        let issued = issue(&again, &request);
        let (_, intermediate) = X509Certificate::from_der(first.intermediate().unwrap()).unwrap();
        parse(&issued)
            .verify_signature(Some(intermediate.public_key()))
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_keys_can_be_read_only_by_their_owner() {
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
        assert_eq!(mode("root.key"), 0o600);
        assert_eq!(mode("intermediate.pem"), 0o600);
        assert_eq!(mode("root.pem"), 0o644);
    }

    #[test]
    fn serving_does_not_need_the_roots_key() {
        let directory = tempfile::tempdir().unwrap();
        let made = open(directory.path(), Create::IfMissing).unwrap();
        std::fs::remove_file(directory.path().join("root.key")).unwrap();
        let again = open(directory.path(), Create::Never).unwrap();
        assert_eq!(made.fingerprint(), again.fingerprint());
        let (_, request) = display("kitchen");
        issue(&again, &request);
        again
            .issue_server(&names(), now(), now() + Duration::days(90))
            .unwrap();
        // And nothing was made in its place.
        assert_eq!(files(directory.path()), ["intermediate.pem", "root.pem"]);
    }

    #[test]
    fn a_missing_part_is_an_error_not_a_fresh_start() {
        for missing in ["root.pem", "intermediate.pem"] {
            let directory = tempfile::tempdir().unwrap();
            open(directory.path(), Create::IfMissing).unwrap();
            // Without the root's key there is no way to replace the intermediate, so it is an error.
            std::fs::remove_file(directory.path().join("root.key")).unwrap();
            std::fs::remove_file(directory.path().join(missing)).unwrap();
            let error = open(directory.path(), Create::IfMissing)
                .err()
                .expect("should refuse")
                .to_string();
            assert!(error.contains("not making a new authority"), "{error}");
            assert_eq!(
                directory.path().join("root.pem").exists(),
                missing != "root.pem",
                "nothing was made or overwritten"
            );
        }
        // A root's key alone is not an authority either.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("root.key"), "x").unwrap();
        let error = open(directory.path(), Create::IfMissing)
            .err()
            .expect("should refuse")
            .to_string();
        assert!(error.contains("not making a new authority"), "{error}");
        assert_eq!(files(directory.path()), ["root.key"]);
    }

    #[test]
    fn a_lost_intermediate_is_replaced_from_the_root_and_displays_are_not_paired_again() {
        let directory = tempfile::tempdir().unwrap();
        let before = open(directory.path(), Create::IfMissing).unwrap();
        let (_, request) = display("kitchen");
        let old = issue(&before, &request);
        std::fs::remove_file(directory.path().join("intermediate.pem")).unwrap();

        let after = open(directory.path(), Create::Never).unwrap();
        // The same root, so the same pin and the same codes.
        assert_eq!(before.fingerprint(), after.fingerprint());
        assert_ne!(before.intermediate(), after.intermediate());
        // A certificate the lost intermediate signed no longer has an intermediate to check against, and
        // that is all: the display renews and gets one under the new intermediate.
        let new = issue(&after, &request);
        let (_, intermediate) = X509Certificate::from_der(after.intermediate().unwrap()).unwrap();
        parse(&new)
            .verify_signature(Some(intermediate.public_key()))
            .unwrap();
        assert!(
            parse(&old)
                .verify_signature(Some(intermediate.public_key()))
                .is_err()
        );
    }

    #[test]
    fn an_old_single_level_authority_is_refused_with_the_reason() {
        for old in ["authority.pem", "authority.key"] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join(old), "x").unwrap();
            let error = open(directory.path(), Create::IfMissing)
                .err()
                .expect("should refuse")
                .to_string();
            assert!(error.contains("one level"), "{error}");
        }
    }

    /// Replaces the intermediate in `directory` with one made `age` ago, as if the authority were that old.
    fn age_the_intermediate(directory: &std::path::Path, age: Duration) {
        let root = std::fs::read_to_string(directory.join("root.pem")).unwrap();
        let key = std::fs::read_to_string(directory.join("root.key")).unwrap();
        let aged = new_intermediate(&root, &key, Utc::now() - age).unwrap();
        std::fs::write(directory.join("intermediate.pem"), aged).unwrap();
    }

    #[test]
    fn an_intermediate_near_its_end_is_replaced_when_the_roots_key_is_there() {
        let directory = tempfile::tempdir().unwrap();
        let first = open(directory.path(), Create::IfMissing).unwrap();
        age_the_intermediate(directory.path(), Duration::days(365 * 4 + 200));
        let aged = open(directory.path(), Create::Never).unwrap();
        // Opening replaced it, so what is in use now has years left.
        assert!(aged.intermediate_not_after() - Utc::now() > Duration::days(365 * 4));
        assert_eq!(first.fingerprint(), aged.fingerprint());
        // The one it replaced is kept, with what it signed still good.
        assert!(directory.path().join("retired.pem").exists());
        assert_eq!(aged.intermediates().len(), 2);
    }

    #[test]
    fn an_intermediate_with_plenty_of_time_left_is_left_alone() {
        let directory = tempfile::tempdir().unwrap();
        open(directory.path(), Create::IfMissing).unwrap();
        age_the_intermediate(directory.path(), Duration::days(365 * 3));
        let before = std::fs::read(directory.path().join("intermediate.pem")).unwrap();
        open(directory.path(), Create::Never).unwrap();
        assert_eq!(
            std::fs::read(directory.path().join("intermediate.pem")).unwrap(),
            before
        );
        assert!(!directory.path().join("retired.pem").exists());
    }

    #[test]
    fn without_the_roots_key_an_intermediate_near_its_end_is_still_served_with_a_warning() {
        let directory = tempfile::tempdir().unwrap();
        open(directory.path(), Create::IfMissing).unwrap();
        age_the_intermediate(directory.path(), Duration::days(365 * 4 + 200));
        let aged_file = std::fs::read(directory.path().join("intermediate.pem")).unwrap();
        std::fs::remove_file(directory.path().join("root.key")).unwrap();
        let authority = open(directory.path(), Create::Never).unwrap();
        // Still in use and untouched; replacing it waits for the key.
        assert!(authority.intermediate_not_after() - Utc::now() < Duration::days(365));
        assert_eq!(
            std::fs::read(directory.path().join("intermediate.pem")).unwrap(),
            aged_file
        );
    }

    #[test]
    fn rotating_with_the_key_kept_elsewhere_and_what_the_old_intermediate_signed_stays_good() {
        let directory = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let before = open(directory.path(), Create::IfMissing).unwrap();
        let moved = elsewhere.path().join("root.key");
        std::fs::rename(directory.path().join("root.key"), &moved).unwrap();
        let (_, request) = display("kitchen");
        let old = issue(&before, &request);

        rotate_intermediate(directory.path(), Some(&moved)).unwrap();
        let after = open(directory.path(), Create::Never).unwrap();
        assert_ne!(before.intermediate(), after.intermediate());
        assert_eq!(before.fingerprint(), after.fingerprint());
        // Both are known to the server, so a certificate from the old one is still completed to the root.
        let known = after.intermediates();
        assert_eq!(known.len(), 2);
        assert!(known.contains(&before.intermediate().unwrap()));
        let (_, kept) = X509Certificate::from_der(before.intermediate().unwrap()).unwrap();
        parse(&old)
            .verify_signature(Some(kept.public_key()))
            .unwrap();
        // New certificates come from the new one.
        let new = issue(&after, &request);
        let (_, current) = X509Certificate::from_der(after.intermediate().unwrap()).unwrap();
        parse(&new)
            .verify_signature(Some(current.public_key()))
            .unwrap();
        // The key was where it had been put and is not in the directory.
        assert!(!directory.path().join("root.key").exists());
    }

    #[test]
    fn a_retired_intermediate_that_has_ended_is_dropped() {
        let directory = tempfile::tempdir().unwrap();
        open(directory.path(), Create::IfMissing).unwrap();
        age_the_intermediate(directory.path(), Duration::days(365 * 6));
        // It ended a year ago; rotating retires it, and the next open no longer carries it.
        rotate_intermediate(directory.path(), None).unwrap();
        let authority = open(directory.path(), Create::Never).unwrap();
        assert_eq!(authority.intermediates().len(), 1);
    }

    #[test]
    fn a_root_key_that_belongs_to_another_root_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        open(directory.path(), Create::IfMissing).unwrap();
        let (_, other) = PrivateAuthority::generate(now()).unwrap();
        let root = std::fs::read_to_string(directory.path().join("root.pem")).unwrap();
        let error = new_intermediate(&root, &other.root_key, now())
            .expect_err("should refuse")
            .to_string();
        assert!(error.contains("does not belong"), "{error}");
    }

    #[test]
    fn an_intermediate_signed_by_another_root_is_refused() {
        let (_, ours) = PrivateAuthority::generate(now()).unwrap();
        let (_, theirs) = PrivateAuthority::generate(now()).unwrap();
        let error =
            PrivateAuthority::from_pem(&ours.root_certificate, &theirs.intermediate, &[], now())
                .err()
                .expect("should refuse")
                .to_string();
        assert!(error.contains("not signed by this root"), "{error}");
    }

    #[test]
    fn an_intermediate_whose_key_is_another_ones_is_refused() {
        let (_, one) = PrivateAuthority::generate(now()).unwrap();
        let root_key = key_from_pem(&one.root_key, "root").unwrap();
        // Same root, a second intermediate: take its certificate with the first one's key.
        let second = new_intermediate(&one.root_certificate, &one.root_key, now()).unwrap();
        let mixed = format!(
            "{}{}",
            split_pem_blocks(&second).0,
            split_pem_blocks(&one.intermediate).1
        );
        let error = PrivateAuthority::from_pem(&one.root_certificate, &mixed, &[], now())
            .err()
            .expect("should refuse")
            .to_string();
        assert!(error.contains("does not belong"), "{error}");
        drop(root_key);
    }

    /// An intermediate file's certificate and its key.
    fn split_pem_blocks(pem: &str) -> (String, String) {
        let at = pem.find("-----BEGIN PRIVATE KEY-----").unwrap();
        (pem[..at].to_owned(), pem[at..].to_owned())
    }

    #[test]
    fn an_intermediate_that_may_sign_further_authorities_is_refused() {
        // Signed by the right root, but allowed one more level beneath it.
        let (_, ours) = PrivateAuthority::generate(now()).unwrap();
        let root_key = key_from_pem(&ours.root_key, "root").unwrap();
        let root_der = certificate_from_pem(&ours.root_certificate, "root").unwrap();
        let issuer = Issuer::from_ca_cert_der(&root_der, root_key).unwrap();
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let params = authority_params("too much", now(), 1, 1).unwrap();
        let certificate = params.signed_by(&key, &issuer).unwrap();
        let pem = format!("{}{}", certificate.pem(), key.serialize_pem());
        let error = PrivateAuthority::from_pem(&ours.root_certificate, &pem, &[], now())
            .err()
            .expect("should refuse")
            .to_string();
        assert!(error.contains("path length"), "{error}");
    }

    #[test]
    fn something_that_is_not_a_root_is_refused_as_one() {
        let (_, ours) = PrivateAuthority::generate(now()).unwrap();
        // An intermediate offered as the root: it is an authority, but not self-signed.
        let (certificate, _) = split_pem_blocks(&ours.intermediate);
        let error = PrivateAuthority::from_pem(&certificate, &ours.intermediate, &[], now())
            .err()
            .expect("should refuse")
            .to_string();
        assert!(error.contains("not self-signed"), "{error}");

        // A display's certificate is not an authority at all.
        let authority = authority();
        let (_, request) = display("kitchen");
        let issued = issue(&authority, &request);
        let error =
            PrivateAuthority::from_pem(&pem_of(&issued.der), &ours.intermediate, &[], now())
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
        std::fs::write(directory.path().join("root.pem"), "nonsense").unwrap();
        std::fs::write(directory.path().join("intermediate.pem"), "nonsense").unwrap();
        let error = format!(
            "{:#}",
            open(directory.path(), Create::IfMissing)
                .err()
                .expect("should refuse")
        );
        assert!(error.contains("not usable"), "{error}");
    }

    #[test]
    fn an_intermediate_never_outlives_the_root() {
        // The root was made 19 years ago, so it has a year left. An intermediate made now would run for
        // five, but it ends with the root.
        let (_, files) = PrivateAuthority::generate(Utc::now() - Duration::days(365 * 19)).unwrap();
        let intermediate =
            new_intermediate(&files.root_certificate, &files.root_key, Utc::now()).unwrap();
        let authority =
            PrivateAuthority::from_pem(&files.root_certificate, &intermediate, &[], Utc::now())
                .unwrap();
        assert!(authority.not_after() - Utc::now() < Duration::days(366));
        assert_eq!(authority.intermediate_not_after(), authority.not_after());
    }
}
