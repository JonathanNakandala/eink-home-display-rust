//! The whole thing over real TLS 1.3 connections on a local port, with a client that does what a display
//! does: take the authority from an unverified connection, work out the pairing code from what it saw,
//! ask to join, wait, collect its certificate, and then renew with it.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::Duration as ChronoDuration;
use chrono_tz::Tz;
use cms::cert::x509::der::Decode;
use cms::content_info::ContentInfo;
use cms::signed_data::SignedData;
use rcgen::{
    Attribute, CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256,
    PublicKeyData,
};
use rustls::ClientConfig;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::ring as provider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use super::*;
use crate::adapters::clock::SystemClock;
use crate::adapters::pairing_store::FilePairingStore;
use crate::application::enrollment::EnrollmentPolicy;
use crate::domain::models::pairing::{Fingerprint, PairingCode, PublicKey};

struct Harness {
    address: SocketAddr,
    enrollment: Arc<Enrollment>,
    authority: Arc<PrivateAuthority>,
    server: JoinHandle<anyhow::Result<()>>,
    _directory: TempDir,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn start() -> Harness {
    start_with(false, |_| {}).await
}

async fn start_with(require_channel_binding: bool, tune: impl FnOnce(&mut EstSettings)) -> Harness {
    let directory = tempfile::tempdir().unwrap();
    let authority = Arc::new(
        crate::adapters::certificate_authority::open(&directory.path().join("authority")).unwrap(),
    );
    let store = FilePairingStore::open(&directory.path().join("pairings"))
        .await
        .unwrap();
    let enrollment = Arc::new(Enrollment::new(
        authority.clone(),
        Arc::new(store),
        Arc::new(SystemClock::new(Tz::UTC)),
        EnrollmentPolicy {
            certificate_lifetime: ChronoDuration::days(365),
            retry_after: ChronoDuration::minutes(5),
            require_channel_binding,
        },
    ));
    let mut settings = EstSettings {
        bind: "127.0.0.1:0".parse().unwrap(),
        names: vec!["localhost".to_owned(), "127.0.0.1".to_owned()],
        ..EstSettings::default()
    };
    tune(&mut settings);
    let server = EstServer::bind(settings, authority.clone(), enrollment.clone()).unwrap();
    let address = server.local_addr().unwrap();
    Harness {
        address,
        enrollment,
        authority,
        server: tokio::spawn(server.run()),
        _directory: directory,
    }
}

/// Accepts any server certificate: what a display with no trust yet does to fetch the authority.
#[derive(Debug)]
struct TrustAnything;

impl ServerCertVerifier for TrustAnything {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        // Willing to speak TLS 1.2, so that a refusal can only be the server's.
        rustls::crypto::verify_tls12_signature(
            message,
            certificate,
            signature,
            &provider::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            certificate,
            signature,
            &provider::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

enum Trust<'a> {
    Anything,
    Authority(&'a [u8]),
}

struct Identity {
    certificate: Vec<u8>,
    key: KeyPair,
}

fn client_config(
    trust: Trust<'_>,
    identity: Option<&Identity>,
    versions: &[&'static rustls::SupportedProtocolVersion],
) -> ClientConfig {
    let builder = ClientConfig::builder_with_provider(Arc::new(provider::default_provider()))
        .with_protocol_versions(versions)
        .unwrap();
    let builder = match trust {
        Trust::Anything => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(TrustAnything)),
        Trust::Authority(der) => {
            let mut roots = RootCertStore::empty();
            roots.add(CertificateDer::from(der.to_vec())).unwrap();
            builder.with_root_certificates(roots)
        }
    };
    let mut config = match identity {
        Some(identity) => builder
            .with_client_auth_cert(
                vec![CertificateDer::from(identity.certificate.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.key.serialize_der())),
            )
            .unwrap(),
        None => builder.with_no_client_auth(),
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config
}

async fn connect_with(
    harness: &Harness,
    config: ClientConfig,
) -> std::io::Result<TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(harness.address).await?;
    TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
}

async fn connect(
    harness: &Harness,
    trust: Trust<'_>,
    identity: Option<&Identity>,
) -> TlsStream<TcpStream> {
    connect_with(
        harness,
        client_config(trust, identity, &[&rustls::version::TLS13]),
    )
    .await
    .expect("the connection should be made")
}

/// What this connection's channel binding is, as the client computes it.
fn binding(stream: &TlsStream<TcpStream>) -> Vec<u8> {
    stream
        .get_ref()
        .1
        .export_keying_material([0u8; 32], b"EXPORTER-Channel-Binding", Some(&[]))
        .unwrap()
        .to_vec()
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The certificates in a certs-only response, as DER.
    fn certificates(&self) -> Vec<Vec<u8>> {
        assert_eq!(self.status, 200, "{}", self.text());
        assert_eq!(
            self.header("content-type"),
            Some("application/pkcs7-mime; smime-type=certs-only")
        );
        let der = wire::decode_body(&self.body).expect("a base64 body");
        let info = ContentInfo::from_der(&der).unwrap();
        let signed: SignedData = info.content.decode_as().unwrap();
        signed
            .certificates
            .unwrap()
            .0
            .iter()
            .map(|c| match c {
                cms::cert::CertificateChoices::Certificate(c) => {
                    cms::cert::x509::der::Encode::to_der(c).unwrap()
                }
                other => panic!("{other:?}"),
            })
            .collect()
    }
}

async fn send(
    stream: &mut TlsStream<TcpStream>,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Response {
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    if let Some(content_type) = content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    // The server may close without a TLS close_notify; what arrived before that is the response.
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw).await;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no response head in {} bytes", raw.len()));
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Response {
        status,
        headers,
        body: dechunk(&raw[split + 4..]),
    }
}

/// The body of a response, undoing chunked encoding if the server used it.
fn dechunk(raw: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(raw);
    let Some((size, rest)) = text.split_once("\r\n") else {
        return raw.to_vec();
    };
    if usize::from_str_radix(size.trim(), 16).is_err() {
        return raw.to_vec();
    }
    let mut body = Vec::new();
    let mut rest = rest;
    let mut size = size;
    loop {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 {
            return body;
        }
        body.extend_from_slice(&rest.as_bytes()[..n]);
        let Some((next, after)) = rest[n..].trim_start_matches("\r\n").split_once("\r\n") else {
            return body;
        };
        size = next;
        rest = after;
    }
}

const ENROLL: &str = "/.well-known/est/simpleenroll";
const REENROLL: &str = "/.well-known/est/simplereenroll";
const CACERTS: &str = "/.well-known/est/cacerts";
const CSRATTRS: &str = "/.well-known/est/csrattrs";
const PKCS10: &str = "application/pkcs10";

fn challenge_password(text: &str) -> Attribute {
    let mut value = vec![0x31, text.len() as u8 + 2, 0x0c, text.len() as u8];
    value.extend_from_slice(text.as_bytes());
    Attribute {
        oid: &[1, 2, 840, 113549, 1, 9, 7],
        values: value,
    }
}

/// A request for `name` with `key`, tied to `connection` if one is given, as the text sent in a body.
fn request(name: &str, key: &KeyPair, connection: Option<&[u8]>) -> String {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, name);
    let attributes = connection
        .map(|binding| vec![challenge_password(&STANDARD.encode(binding))])
        .unwrap_or_default();
    let csr = params
        .serialize_request_with_attributes(key, attributes)
        .unwrap();
    STANDARD.encode(csr.der().as_ref())
}

fn new_key() -> KeyPair {
    KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap()
}

fn device(name: &str) -> DeviceId {
    DeviceId::parse(name).unwrap()
}

/// Everything a display does up to asking to join: gets the authority from an unverified connection,
/// and works out the code its panel would show from what it saw.
async fn first_contact(harness: &Harness, name: &str, key: &KeyPair) -> (Vec<u8>, PairingCode) {
    let mut stream = connect(harness, Trust::Anything, None).await;
    let response = send(&mut stream, "GET", CACERTS, None, b"").await;
    let authority = response.certificates().remove(0);
    let code = PairingCode::derive(
        &Fingerprint::of(&authority),
        &device(name),
        &PublicKey::from_der(key.subject_public_key_info()),
    );
    (authority, code)
}

/// One request on a connection of its own, tied to that connection like a display's.
async fn enroll(harness: &Harness, trust: Trust<'_>, name: &str, key: &KeyPair) -> Response {
    let mut stream = connect(harness, trust, None).await;
    let body = request(name, key, Some(&binding(&stream)));
    send(&mut stream, "POST", ENROLL, Some(PKCS10), body.as_bytes()).await
}

#[tokio::test]
async fn a_display_joins_waits_for_the_owner_collects_its_certificate_and_renews() {
    let harness = start().await;
    let key = new_key();
    let name = "reterminal-e1003-a1b2c3";

    // 1. It has no trust yet. It takes the authority from an unverified connection, and works out the
    //    code its panel shows from the authority it was given.
    let (authority, panel_code) = first_contact(&harness, name, &key).await;
    assert_eq!(authority, harness.authority.certificate());

    // 2. Asking while the owner has not opened the window is refused.
    let refused = enroll(&harness, Trust::Authority(&authority), name, &key).await;
    assert_eq!(refused.status, 403, "{}", refused.text());

    // 3. With the window open it is told to wait, and not told the code.
    harness.enrollment.open_window(ChronoDuration::minutes(10));
    let waiting = enroll(&harness, Trust::Authority(&authority), name, &key).await;
    assert_eq!(waiting.status, 202, "{}", waiting.text());
    assert_eq!(waiting.header("retry-after"), Some("300"));
    assert!(!waiting.text().contains(&panel_code.to_string()));
    assert!(
        !waiting
            .text()
            .contains(&panel_code.as_str().replace('-', ""))
    );

    // 4. The server holds what the display worked out for itself: same code, so the owner can approve.
    let pairings = harness.enrollment.pairings().await.unwrap();
    assert_eq!(pairings.len(), 1);
    assert_eq!(pairings[0].code, panel_code);
    harness
        .enrollment
        .approve(&device(name), &panel_code)
        .await
        .unwrap();

    // 5. Its next request gets the certificate.
    let granted = enroll(&harness, Trust::Authority(&authority), name, &key).await;
    let certificates = granted.certificates();
    assert_eq!(certificates.len(), 1);
    let certificate = certificates[0].clone();
    let (_, parsed) = x509_parser::parse_x509_certificate(&certificate).unwrap();
    assert_eq!(
        parsed
            .subject()
            .iter_common_name()
            .next()
            .unwrap()
            .as_str()
            .unwrap(),
        name
    );
    assert_eq!(
        parsed.public_key().raw,
        key.subject_public_key_info().as_slice()
    );

    // 6. It is now known by that certificate, and renews with it, with no one's help.
    let identity = Identity { certificate, key };
    let mut stream = connect(&harness, Trust::Authority(&authority), Some(&identity)).await;
    let body = request(name, &identity.key, Some(&binding(&stream)));
    let renewed = send(&mut stream, "POST", REENROLL, Some(PKCS10), body.as_bytes()).await;
    assert_eq!(renewed.certificates().len(), 1);

    // 7. Until the owner revokes it.
    harness.enrollment.revoke(&device(name)).await.unwrap();
    let mut stream = connect(&harness, Trust::Authority(&authority), Some(&identity)).await;
    let body = request(name, &identity.key, Some(&binding(&stream)));
    let after = send(&mut stream, "POST", REENROLL, Some(PKCS10), body.as_bytes()).await;
    assert_eq!(after.status, 403, "{}", after.text());
}

#[tokio::test]
async fn someone_in_the_middle_gives_the_display_a_different_code_than_the_server_holds() {
    // The display is shown an impostor's authority; the server knows its own. The two codes differ, so
    // the owner, typing what the panel shows, is refused.
    let harness = start().await;
    let impostor = start().await;
    let key = new_key();
    let (_, shown_by_display) = first_contact(&impostor, "kitchen", &key).await;

    harness.enrollment.open_window(ChronoDuration::minutes(10));
    let (genuine, _) = first_contact(&harness, "kitchen", &key).await;
    let waiting = enroll(&harness, Trust::Authority(&genuine), "kitchen", &key).await;
    assert_eq!(waiting.status, 202);

    let result = harness
        .enrollment
        .approve(&device("kitchen"), &shown_by_display)
        .await;
    assert!(
        matches!(
            result,
            Err(crate::application::enrollment::ApproveError::WrongCode(_))
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn the_authority_can_be_fetched_by_anyone_and_parses_as_a_certs_only_message() {
    let harness = start().await;
    let mut stream = connect(&harness, Trust::Anything, None).await;
    let response = send(&mut stream, "GET", CACERTS, None, b"").await;
    assert_eq!(
        response.certificates(),
        [harness.authority.certificate().to_vec()]
    );
}

#[tokio::test]
async fn the_server_proves_itself_to_a_display_that_trusts_the_authority() {
    let harness = start().await;
    let config = client_config(
        Trust::Authority(harness.authority.certificate()),
        None,
        &[&rustls::version::TLS13],
    );
    let mut stream = connect_with(&harness, config).await.unwrap();
    let response = send(&mut stream, "GET", CSRATTRS, None, b"").await;
    assert_eq!(response.status, 200);
}

#[tokio::test]
async fn a_display_that_trusts_a_different_authority_does_not_trust_this_server() {
    let harness = start().await;
    let other = start().await;
    let config = client_config(
        Trust::Authority(other.authority.certificate()),
        None,
        &[&rustls::version::TLS13],
    );
    assert!(connect_with(&harness, config).await.is_err());
}

#[tokio::test]
async fn a_server_certificate_for_another_name_is_not_accepted() {
    // The certificate names "localhost" and 127.0.0.1; a client that expects another name must refuse it.
    let harness = start().await;
    let config = client_config(
        Trust::Authority(harness.authority.certificate()),
        None,
        &[&rustls::version::TLS13],
    );
    let tcp = TcpStream::connect(harness.address).await.unwrap();
    let result = TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from("elsewhere.example").unwrap(), tcp)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn tls_1_2_is_refused() {
    let harness = start().await;
    let config = ClientConfig::builder_with_provider(Arc::new(provider::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS12])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(TrustAnything))
        .with_no_client_auth();
    assert!(connect_with(&harness, config).await.is_err());
}

#[tokio::test]
async fn what_the_server_asks_of_a_request_is_the_ecdsa_p256_attribute_list() {
    let harness = start().await;
    let mut stream = connect(&harness, Trust::Anything, None).await;
    let response = send(&mut stream, "GET", CSRATTRS, None, b"").await;
    assert_eq!(
        response.header("content-type"),
        Some("application/csrattrs")
    );
    assert_eq!(
        wire::decode_body(&response.body).unwrap(),
        wire::csr_attributes(false)
    );

    let strict = start_with(true, |_| {}).await;
    let mut stream = connect(&strict, Trust::Anything, None).await;
    let response = send(&mut stream, "GET", CSRATTRS, None, b"").await;
    assert_eq!(
        wire::decode_body(&response.body).unwrap(),
        wire::csr_attributes(true)
    );
}

#[tokio::test]
async fn a_request_signed_on_another_connection_is_refused() {
    // A request captured from one connection and replayed on another, which the binding catches.
    let harness = start().await;
    harness.enrollment.open_window(ChronoDuration::minutes(10));
    let key = new_key();
    let mut other = connect(&harness, Trust::Anything, None).await;
    let tied_elsewhere = request("kitchen", &key, Some(&binding(&other)));
    let _ = other.shutdown().await;

    let mut stream = connect(&harness, Trust::Anything, None).await;
    let response = send(
        &mut stream,
        "POST",
        ENROLL,
        Some(PKCS10),
        tied_elsewhere.as_bytes(),
    )
    .await;
    assert_eq!(response.status, 400, "{}", response.text());
    assert!(harness.enrollment.pairings().await.unwrap().is_empty());
}

#[tokio::test]
async fn when_required_a_request_must_carry_the_binding() {
    let harness = start_with(true, |_| {}).await;
    harness.enrollment.open_window(ChronoDuration::minutes(10));
    let key = new_key();
    let mut stream = connect(&harness, Trust::Anything, None).await;
    let untied = request("kitchen", &key, None);
    let response = send(&mut stream, "POST", ENROLL, Some(PKCS10), untied.as_bytes()).await;
    assert_eq!(response.status, 400, "{}", response.text());

    let tied = enroll(&harness, Trust::Anything, "kitchen", &key).await;
    assert_eq!(tied.status, 202);
}

#[tokio::test]
async fn renewing_needs_a_certificate_and_a_certificate_from_elsewhere_is_no_good() {
    let harness = start().await;
    let key = new_key();
    let mut stream = connect(&harness, Trust::Anything, None).await;
    let body = request("kitchen", &key, Some(&binding(&stream)));
    let anonymous = send(&mut stream, "POST", REENROLL, Some(PKCS10), body.as_bytes()).await;
    assert_eq!(anonymous.status, 401);

    // A certificate from some other authority is refused at the handshake.
    let (other_authority, _, _) =
        crate::adapters::certificate_authority::PrivateAuthority::generate(chrono::Utc::now())
            .unwrap();
    let their_key = new_key();
    let csr = request("kitchen", &their_key, None);
    let der = STANDARD.decode(csr).unwrap();
    let request_read = other_authority.inspect(&der).unwrap();
    let issued = other_authority
        .issue(
            &request_read,
            chrono::Utc::now(),
            chrono::Utc::now() + ChronoDuration::days(1),
        )
        .unwrap();
    let stranger = Identity {
        certificate: issued.der,
        key: their_key,
    };
    let config = client_config(Trust::Anything, Some(&stranger), &[&rustls::version::TLS13]);
    let outcome = async {
        let mut stream = connect_with(&harness, config).await?;
        // In TLS 1.3 the server's rejection of the client's certificate arrives after the client
        // considers the handshake done, so it shows up on the first use of the connection.
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await?;
        let mut buffer = Vec::new();
        stream.read_to_end(&mut buffer).await?;
        Ok::<_, std::io::Error>(buffer)
    }
    .await;
    assert!(outcome.is_err() || outcome.unwrap().is_empty());
}

#[tokio::test]
async fn badly_formed_requests_get_the_right_answers() {
    let harness = start().await;
    harness.enrollment.open_window(ChronoDuration::minutes(10));
    let key = new_key();
    let good = request("kitchen", &key, None);

    let mut stream = connect(&harness, Trust::Anything, None).await;
    let wrong_type = send(
        &mut stream,
        "POST",
        ENROLL,
        Some("text/plain"),
        good.as_bytes(),
    )
    .await;
    assert_eq!(wrong_type.status, 415);

    let mut stream = connect(&harness, Trust::Anything, None).await;
    let no_type = send(&mut stream, "POST", ENROLL, None, good.as_bytes()).await;
    assert_eq!(no_type.status, 415);

    let mut stream = connect(&harness, Trust::Anything, None).await;
    let not_base64 = send(&mut stream, "POST", ENROLL, Some(PKCS10), b"!!!!").await;
    assert_eq!(not_base64.status, 400);

    let mut stream = connect(&harness, Trust::Anything, None).await;
    let not_a_request = send(
        &mut stream,
        "POST",
        ENROLL,
        Some(PKCS10),
        STANDARD.encode(b"hello").as_bytes(),
    )
    .await;
    assert_eq!(not_a_request.status, 400);

    let mut stream = connect(&harness, Trust::Anything, None).await;
    let huge = vec![b'A'; 64 * 1024];
    let too_big = send(&mut stream, "POST", ENROLL, Some(PKCS10), &huge).await;
    assert_eq!(too_big.status, 413);

    // A content type with a parameter or other case is still the right type.
    let mut stream = connect(&harness, Trust::Anything, None).await;
    let mixed = send(
        &mut stream,
        "POST",
        ENROLL,
        Some("Application/PKCS10; charset=binary"),
        good.as_bytes(),
    )
    .await;
    assert_eq!(mixed.status, 202, "{}", mixed.text());

    assert_eq!(harness.enrollment.pairings().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_wrapped_request_body_is_accepted() {
    // MIME-style base64 has line breaks (RFC 8951 section 3.1 asks receivers to allow them).
    let harness = start().await;
    harness.enrollment.open_window(ChronoDuration::minutes(10));
    let key = new_key();
    let wrapped = request("kitchen", &key, None)
        .as_bytes()
        .chunks(64)
        .map(|c| String::from_utf8(c.to_vec()).unwrap())
        .collect::<Vec<_>>()
        .join("\r\n");
    let mut stream = connect(&harness, Trust::Anything, None).await;
    let response = send(
        &mut stream,
        "POST",
        ENROLL,
        Some(PKCS10),
        wrapped.as_bytes(),
    )
    .await;
    assert_eq!(response.status, 202, "{}", response.text());
}

#[tokio::test]
async fn other_paths_and_methods_are_not_served() {
    let harness = start().await;
    for (method, path) in [
        ("GET", "/"),
        ("GET", "/image"),
        ("GET", ENROLL),
        ("POST", CACERTS),
        ("GET", "/.well-known/est/serverkeygen"),
    ] {
        let mut stream = connect(&harness, Trust::Anything, None).await;
        let response = send(&mut stream, method, path, None, b"").await;
        assert!(
            matches!(response.status, 404 | 405),
            "{method} {path}: {}",
            response.status
        );
    }
}

#[tokio::test]
async fn a_client_that_never_finishes_the_handshake_is_dropped() {
    let harness = start_with(false, |s| s.handshake_timeout = Duration::from_millis(200)).await;
    let mut tcp = TcpStream::connect(harness.address).await.unwrap();
    let mut buffer = [0u8; 16];
    let read = tokio::time::timeout(Duration::from_secs(3), tcp.read(&mut buffer))
        .await
        .expect("the server should have given up by now");
    assert!(matches!(read, Ok(0) | Err(_)));
}

#[tokio::test]
async fn garbage_instead_of_a_handshake_does_no_harm() {
    let harness = start().await;
    let mut tcp = TcpStream::connect(harness.address).await.unwrap();
    tcp.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
    let mut buffer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), tcp.read_to_end(&mut buffer)).await;
    // And the server still serves.
    let mut stream = connect(&harness, Trust::Anything, None).await;
    assert_eq!(
        send(&mut stream, "GET", CSRATTRS, None, b"").await.status,
        200
    );
}

#[tokio::test]
async fn only_so_many_connections_are_held_at_once() {
    let harness = start_with(false, |s| s.max_connections = 2).await;
    // Two idle connections, handshake done and no request sent, take both places.
    let a = connect(&harness, Trust::Anything, None).await;
    let b = connect(&harness, Trust::Anything, None).await;
    // A third is dropped by the server before the handshake.
    let tcp = TcpStream::connect(harness.address).await.unwrap();
    let third = TlsConnector::from(Arc::new(client_config(
        Trust::Anything,
        None,
        &[&rustls::version::TLS13],
    )))
    .connect(ServerName::try_from("localhost").unwrap(), tcp)
    .await;
    assert!(third.is_err());
    // When one place is freed, a new connection is served.
    drop(a);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut again = connect(&harness, Trust::Anything, None).await;
    assert_eq!(
        send(&mut again, "GET", CSRATTRS, None, b"").await.status,
        200
    );
    drop(b);
}

#[tokio::test]
async fn a_connection_that_goes_on_too_long_is_ended() {
    let harness = start_with(false, |s| {
        s.connection_lifetime = Duration::from_millis(300)
    })
    .await;
    let mut stream = connect(&harness, Trust::Anything, None).await;
    // No request: the server ends the connection when the lifetime is up.
    let mut buffer = Vec::new();
    let finished =
        tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut buffer)).await;
    assert!(
        finished.is_ok(),
        "the server should have closed the connection"
    );
}
