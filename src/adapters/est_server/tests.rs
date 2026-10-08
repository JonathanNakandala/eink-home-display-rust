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
use rustls::crypto::aws_lc_rs as provider;
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
use crate::adapters::pairing_store::{FilePairingStore, Missing};
use crate::application::enrollment::EnrollmentPolicy;
use crate::domain::models::pairing::{Fingerprint, PairingCode, PublicKey};

pub(crate) struct Harness {
    pub(crate) address: SocketAddr,
    pub(crate) enrollment: Arc<Enrollment>,
    pub(crate) authority: Arc<PrivateAuthority>,
    pub(crate) server: JoinHandle<anyhow::Result<()>>,
    pub(crate) _directory: TempDir,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub(crate) async fn start() -> Harness {
    start_with(false, |_| {}).await
}

pub(crate) async fn start_with(
    require_channel_binding: bool,
    tune: impl FnOnce(&mut EstSettings),
) -> Harness {
    start_full(require_channel_binding, tune, None).await
}

/// A server with a stand-in for the display routes behind a gate of the given kind.
pub(crate) async fn start_guarded(access: Access) -> Harness {
    start_full(false, |_| {}, Some(access)).await
}

/// `/image` says it was served; `/who` says who the request is from, as the routes would see it.
pub(crate) fn stand_in_display() -> axum::Router {
    use crate::adapters::authenticated::AuthenticatedDevice;
    use axum::routing::get;
    axum::Router::new()
        .route("/image", get(|| async { "image-bytes" }))
        .route(
            "/who",
            get(
                |who: Option<axum::Extension<AuthenticatedDevice>>| async move {
                    who.map(|w| w.0.0.to_string())
                        .unwrap_or_else(|| "anonymous".to_owned())
                },
            ),
        )
}

pub(crate) async fn start_full(
    require_channel_binding: bool,
    tune: impl FnOnce(&mut EstSettings),
    guard: Option<Access>,
) -> Harness {
    let directory = tempfile::tempdir().unwrap();
    let authority = Arc::new(
        crate::adapters::certificate_authority::open(
            &directory.path().join("authority"),
            crate::adapters::certificate_authority::Create::IfMissing,
        )
        .unwrap(),
    );
    let store = FilePairingStore::open(&directory.path().join("pairings"), Missing::StartEmpty)
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
    let display = guard.map(|access| guarded(stand_in_display(), enrollment.clone(), access));
    let server = EstServer::bind(settings, authority.clone(), enrollment.clone(), display).unwrap();
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
pub(crate) struct TrustAnything;

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

pub(crate) enum Trust<'a> {
    Anything,
    Authority(&'a [u8]),
}

pub(crate) struct Identity {
    pub(crate) certificate: Vec<u8>,
    pub(crate) key: KeyPair,
}

pub(crate) fn client_config(
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

pub(crate) async fn connect_with(
    harness: &Harness,
    config: ClientConfig,
) -> std::io::Result<TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(harness.address).await?;
    TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
}

pub(crate) async fn connect(
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
pub(crate) fn binding(stream: &TlsStream<TcpStream>) -> Vec<u8> {
    stream
        .get_ref()
        .1
        .export_keying_material([0u8; 32], b"EXPORTER-Channel-Binding", Some(&[]))
        .unwrap()
        .to_vec()
}

pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl Response {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The certificates in a certs-only response, as DER.
    pub(crate) fn certificates(&self) -> Vec<Vec<u8>> {
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

pub(crate) async fn send(
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
pub(crate) fn dechunk(raw: &[u8]) -> Vec<u8> {
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

pub(crate) const ENROLL: &str = "/.well-known/est/simpleenroll";
pub(crate) const REENROLL: &str = "/.well-known/est/simplereenroll";
pub(crate) const CACERTS: &str = "/.well-known/est/cacerts";
pub(crate) const CSRATTRS: &str = "/.well-known/est/csrattrs";
pub(crate) const PKCS10: &str = "application/pkcs10";

pub(crate) fn challenge_password(text: &str) -> Attribute {
    let mut value = vec![0x31, text.len() as u8 + 2, 0x0c, text.len() as u8];
    value.extend_from_slice(text.as_bytes());
    Attribute {
        oid: &[1, 2, 840, 113549, 1, 9, 7],
        values: value,
    }
}

/// A request for `name` with `key`, tied to `connection` if one is given, as the text sent in a body.
pub(crate) fn request(name: &str, key: &KeyPair, connection: Option<&[u8]>) -> String {
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

pub(crate) fn new_key() -> KeyPair {
    KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap()
}

pub(crate) fn device(name: &str) -> DeviceId {
    DeviceId::parse(name).unwrap()
}

/// Everything a display does up to asking to join: gets the authority from an unverified connection,
/// and works out the code its panel would show from what it saw.
pub(crate) async fn first_contact(
    harness: &Harness,
    name: &str,
    key: &KeyPair,
) -> (Vec<u8>, PairingCode) {
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
pub(crate) async fn enroll(
    harness: &Harness,
    trust: Trust<'_>,
    name: &str,
    key: &KeyPair,
) -> Response {
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

    // 4. The server works out the same code from what it was sent, so the owner, typing what the panel shows,
    // can approve. (It keeps no copy of the code: it is worked out when one is typed.)
    let pairings = harness.enrollment.pairings().await.unwrap();
    assert_eq!(pairings.len(), 1);
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
    // The root, which a display pins, and then the intermediate it needs to build a path to it.
    let certificates = response.certificates();
    assert_eq!(certificates.len(), 2);
    assert_eq!(certificates[0], harness.authority.certificate());
    assert_eq!(certificates[1], harness.authority.intermediate().unwrap());
    let (_, root) = x509_parser::parse_x509_certificate(&certificates[0]).unwrap();
    assert_eq!(
        root.subject(),
        root.issuer(),
        "the root is the self-signed one"
    );
    root.verify_signature(None).unwrap();
}

#[tokio::test]
async fn the_server_sends_its_intermediate_with_its_certificate_so_a_display_needs_only_the_root() {
    let harness = start().await;
    let stream = connect(
        &harness,
        Trust::Authority(harness.authority.certificate()),
        None,
    )
    .await;
    let sent = stream
        .get_ref()
        .1
        .peer_certificates()
        .expect("a chain")
        .to_vec();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].as_ref(), harness.authority.intermediate().unwrap());
}

#[tokio::test]
async fn a_display_that_sends_only_its_own_certificate_is_still_known() {
    // The server completes the path from the intermediates it holds, so the display need not keep or send
    // its intermediate. `join` and `get_as` show a bare certificate, with nothing else.
    let harness = start_guarded(Access::Members).await;
    let kitchen = join(&harness, "kitchen", new_key()).await;
    assert_eq!(
        get_as(&harness, "/who", Some(&kitchen)).await.text().trim(),
        "kitchen"
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

/// Whether the server answers a request on a connection made with `config`. A client certificate that
/// the server rejects shows up not at the client's handshake (in TLS 1.3 that is already done) but as
/// an alert instead of an answer, so the question is whether any HTTP came back. The connection is read
/// until it ends, however it ends: the server closes without a TLS close_notify.
pub(crate) async fn gets_an_answer(harness: &Harness, config: ClientConfig) -> bool {
    let Ok(mut stream) = connect_with(harness, config).await else {
        return false;
    };
    let request =
        format!("GET {CSRATTRS} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut received = Vec::new();
    let mut chunk = [0u8; 1024];
    while let Ok(n) = stream.read(&mut chunk).await {
        if n == 0 {
            break;
        }
        received.extend_from_slice(&chunk[..n]);
    }
    received.starts_with(b"HTTP/1.1 ")
}

#[tokio::test]
async fn renewing_needs_a_certificate_and_a_certificate_from_elsewhere_is_no_good() {
    let harness = start().await;
    let key = new_key();
    let mut stream = connect(&harness, Trust::Anything, None).await;
    let body = request("kitchen", &key, Some(&binding(&stream)));
    let anonymous = send(&mut stream, "POST", REENROLL, Some(PKCS10), body.as_bytes()).await;
    assert_eq!(anonymous.status, 403, "{}", anonymous.text());
    assert!(anonymous.header("www-authenticate").is_none());

    // A certificate from some other authority is refused at the handshake.
    let (other_authority, _) =
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
    assert!(
        !gets_an_answer(&harness, config).await,
        "a certificate from another authority should be turned away"
    );
    // The same helper does see an answer when the certificate is good, so "no answer" means something.
    let member = join(&harness, "kitchen", new_key()).await;
    let config = client_config(
        Trust::Authority(harness.authority.certificate()),
        Some(&member),
        &[&rustls::version::TLS13],
    );
    assert!(gets_an_answer(&harness, config).await);
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

/// A display joining, start to finish, and holding its certificate.
pub(crate) async fn join(harness: &Harness, name: &str, key: KeyPair) -> Identity {
    let (authority, code) = first_contact(harness, name, &key).await;
    harness.enrollment.open_window(ChronoDuration::minutes(10));
    let waiting = enroll(harness, Trust::Authority(&authority), name, &key).await;
    assert_eq!(waiting.status, 202, "{}", waiting.text());
    harness
        .enrollment
        .approve(&device(name), &code)
        .await
        .unwrap();
    let granted = enroll(harness, Trust::Authority(&authority), name, &key).await;
    Identity {
        certificate: granted.certificates().remove(0),
        key,
    }
}

/// Renews as `identity`, over a verified connection, and says what the server answered.
pub(crate) async fn renew_as(harness: &Harness, identity: &Identity, name: &str) -> Response {
    let mut stream = connect(
        harness,
        Trust::Authority(harness.authority.certificate()),
        Some(identity),
    )
    .await;
    let body = request(name, &identity.key, Some(&binding(&stream)));
    send(&mut stream, "POST", REENROLL, Some(PKCS10), body.as_bytes()).await
}

#[tokio::test]
async fn a_certificate_of_a_forgotten_display_can_not_renew_once_another_has_taken_the_name() {
    let harness = start().await;
    let first = join(&harness, "kitchen", new_key()).await;
    assert_eq!(renew_as(&harness, &first, "kitchen").await.status, 200);

    // The owner forgets it (say, it was lost) and a new display takes the name.
    harness.enrollment.forget(&device("kitchen")).await.unwrap();
    let second = join(&harness, "kitchen", new_key()).await;

    // The first one's certificate is still valid as far as TLS can tell, and says "kitchen".
    let stale = renew_as(&harness, &first, "kitchen").await;
    assert_eq!(stale.status, 403, "{}", stale.text());
    // The one now enrolled is unaffected.
    assert_eq!(renew_as(&harness, &second, "kitchen").await.status, 200);
}

#[tokio::test]
async fn the_servers_own_certificate_is_not_accepted_as_a_display() {
    // It is signed by the same authority, but it is for proving the server, not a display.
    let harness = start().await;
    let server = harness
        .authority
        .issue_server(
            &["localhost".to_owned()],
            chrono::Utc::now(),
            chrono::Utc::now() + ChronoDuration::days(1),
        )
        .unwrap();
    let PrivateKeyDer::Pkcs8(pkcs8) = &server.key else {
        panic!("expected a PKCS #8 key");
    };
    let impostor = Identity {
        certificate: server.certificate.to_vec(),
        key: KeyPair::from_pkcs8_der_and_sign_algo(pkcs8, &PKCS_ECDSA_P256_SHA256).unwrap(),
    };
    let config = client_config(
        Trust::Authority(harness.authority.certificate()),
        Some(&impostor),
        &[&rustls::version::TLS13],
    );
    assert!(
        !gets_an_answer(&harness, config).await,
        "the server's certificate should not be taken for a display's"
    );
}

#[tokio::test]
async fn a_client_that_sends_its_body_too_slowly_is_told_to_stop() {
    let harness = start_with(false, |s| s.request_timeout = Duration::from_millis(300)).await;
    let mut stream = connect(&harness, Trust::Anything, None).await;
    stream
        .write_all(
            format!(
                "POST {ENROLL} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: {PKCS10}\r\nContent-Length: 100\r\n\r\nAAAA"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut raw = Vec::new();
    let finished = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut raw)).await;
    assert!(
        finished.is_ok(),
        "the server should have answered, not waited"
    );
    assert!(
        String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 408"),
        "{}",
        String::from_utf8_lossy(&raw)
    );
}

/// What the stand-in display routes answer to `path`, from a client showing `identity` (or nothing).
pub(crate) async fn get_as(harness: &Harness, path: &str, identity: Option<&Identity>) -> Response {
    let mut stream = connect(
        harness,
        Trust::Authority(harness.authority.certificate()),
        identity,
    )
    .await;
    send(&mut stream, "GET", path, None, b"").await
}

#[tokio::test]
async fn with_open_access_anyone_is_served_and_a_member_is_named_by_its_certificate() {
    let harness = start_guarded(Access::Open).await;
    let anonymous = get_as(&harness, "/who", None).await;
    assert_eq!(
        (anonymous.status, anonymous.text().trim()),
        (200, "anonymous")
    );
    assert_eq!(
        get_as(&harness, "/image", None).await.text().trim(),
        "image-bytes"
    );

    let kitchen = join(&harness, "kitchen", new_key()).await;
    let named = get_as(&harness, "/who", Some(&kitchen)).await;
    assert_eq!((named.status, named.text().trim()), (200, "kitchen"));
}

#[tokio::test]
async fn with_open_access_a_certificate_that_is_shown_and_refused_is_refused() {
    // Showing a bad certificate is not better than showing none.
    let harness = start_guarded(Access::Open).await;
    let kitchen = join(&harness, "kitchen", new_key()).await;
    harness.enrollment.revoke(&device("kitchen")).await.unwrap();
    assert_eq!(get_as(&harness, "/image", Some(&kitchen)).await.status, 403);
    assert_eq!(get_as(&harness, "/who", Some(&kitchen)).await.status, 403);
    // While without it, the same routes are open.
    assert_eq!(get_as(&harness, "/image", None).await.status, 200);
}

#[tokio::test]
async fn a_certificate_of_a_replaced_display_is_turned_away_from_the_display_routes() {
    let harness = start_guarded(Access::Open).await;
    let first = join(&harness, "kitchen", new_key()).await;
    harness.enrollment.forget(&device("kitchen")).await.unwrap();
    let second = join(&harness, "kitchen", new_key()).await;
    assert_eq!(get_as(&harness, "/who", Some(&first)).await.status, 403);
    assert_eq!(
        get_as(&harness, "/who", Some(&second)).await.text().trim(),
        "kitchen"
    );
}

#[tokio::test]
async fn with_members_only_a_display_without_a_certificate_gets_nothing() {
    let harness = start_guarded(Access::Members).await;
    for path in ["/image", "/who"] {
        let refused = get_as(&harness, path, None).await;
        assert_eq!(refused.status, 403, "{path}: {}", refused.text());
        assert!(!refused.text().contains("image-bytes"));
    }
    let kitchen = join(&harness, "kitchen", new_key()).await;
    assert_eq!(
        get_as(&harness, "/image", Some(&kitchen))
            .await
            .text()
            .trim(),
        "image-bytes"
    );
    assert_eq!(
        get_as(&harness, "/who", Some(&kitchen)).await.text().trim(),
        "kitchen"
    );

    harness.enrollment.revoke(&device("kitchen")).await.unwrap();
    assert_eq!(get_as(&harness, "/image", Some(&kitchen)).await.status, 403);
}

#[tokio::test]
async fn with_members_only_a_new_display_can_still_reach_the_way_in() {
    // Otherwise nothing could ever join.
    let harness = start_guarded(Access::Members).await;
    assert_eq!(get_as(&harness, CACERTS, None).await.status, 200);
    assert_eq!(get_as(&harness, CSRATTRS, None).await.status, 200);
    let kitchen = join(&harness, "kitchen", new_key()).await;
    assert_eq!(get_as(&harness, "/image", Some(&kitchen)).await.status, 200);
}

/// The names in the server's certificate, as a client sees it.
async fn names_in_the_servers_certificate(harness: &Harness) -> Vec<String> {
    let stream = connect(harness, Trust::Anything, None).await;
    let chain = stream
        .get_ref()
        .1
        .peer_certificates()
        .expect("a certificate")
        .to_vec();
    let (_, certificate) = x509_parser::parse_x509_certificate(chain[0].as_ref()).unwrap();
    let alt = certificate
        .subject_alternative_name()
        .unwrap()
        .unwrap()
        .value;
    alt.general_names
        .iter()
        .filter_map(|name| match name {
            x509_parser::extensions::GeneralName::DNSName(name) => Some(name.to_string()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_servers_certificate_always_has_the_fixed_name_whatever_is_configured() {
    for configured in [
        vec![],
        vec!["localhost".to_owned()],
        vec!["other.example".to_owned(), "localhost".to_owned()],
        // Already there, in another case: not added twice.
        vec![SERVER_NAME.to_uppercase(), "localhost".to_owned()],
    ] {
        let harness = start_with(false, |s| s.names = configured.clone()).await;
        let names = names_in_the_servers_certificate(&harness).await;
        assert!(
            names.iter().any(|n| n.eq_ignore_ascii_case(SERVER_NAME)),
            "{configured:?}: {names:?}"
        );
        assert_eq!(
            names
                .iter()
                .filter(|n| n.eq_ignore_ascii_case(SERVER_NAME))
                .count(),
            1,
            "{names:?}"
        );
    }
}

#[tokio::test]
async fn a_display_that_checks_the_fixed_name_trusts_the_server_whatever_its_own_names_are() {
    // The names the owner chose are not the display's business: it checks the fixed one against the
    // authority it pinned, and connects by whatever address mDNS gave it.
    let harness = start_with(false, |s| {
        s.names = vec!["renamed-later.example".to_owned()]
    })
    .await;
    let config = client_config(
        Trust::Authority(harness.authority.certificate()),
        None,
        &[&rustls::version::TLS13],
    );
    let tcp = TcpStream::connect(harness.address).await.unwrap();
    let mut stream = TlsConnector::from(Arc::new(config))
        .connect(ServerName::try_from(SERVER_NAME).unwrap(), tcp)
        .await
        .expect("the fixed name should verify");
    let response = send(&mut stream, "GET", CSRATTRS, None, b"").await;
    assert_eq!(response.status, 200);
}

#[tokio::test]
async fn a_display_certificate_is_not_taken_for_the_server() {
    // A display's certificate has no name at all, so it can't pass as the server to another display
    // that does not check key usage.
    let harness = start().await;
    let member = join(&harness, "kitchen", new_key()).await;
    let (_, parsed) = x509_parser::parse_x509_certificate(&member.certificate).unwrap();
    assert!(parsed.subject_alternative_name().unwrap().is_none());
}

#[tokio::test]
async fn a_refused_request_is_logged_with_where_it_came_from() {
    crate::captured_log::install();
    let harness = start().await;
    // The window is shut, so this is refused, and the warning says who asked.
    let key = new_key();
    let response = enroll(
        &harness,
        Trust::Authority(harness.authority.certificate()),
        "logsource-display",
        &key,
    )
    .await;
    assert_eq!(response.status, 403);
    let lines = crate::captured_log::lines_containing("EST request from 127.0.0.1 refused");
    assert!(
        lines.iter().any(|line| line.starts_with("WARN")),
        "{lines:?}"
    );
}

/// A clock fixed at one moment.
struct FixedClock(chrono::DateTime<chrono::Utc>);

impl crate::domain::services::clock::Clock for FixedClock {
    fn now(&self) -> chrono::DateTime<chrono_tz::Tz> {
        self.0.with_timezone(&chrono_tz::Tz::UTC)
    }
}

#[tokio::test]
async fn the_server_does_not_start_while_its_clock_has_not_been_set() {
    use chrono::TimeZone;
    let directory = tempfile::tempdir().unwrap();
    let authority = Arc::new(
        crate::adapters::certificate_authority::open(
            &directory.path().join("authority"),
            crate::adapters::certificate_authority::Create::IfMissing,
        )
        .unwrap(),
    );
    let enrolment_at = |clock: FixedClock, name: &'static str| {
        let directory = directory.path().to_owned();
        let authority = authority.clone();
        async move {
            let store = FilePairingStore::open(&directory.join(name), Missing::StartEmpty)
                .await
                .unwrap();
            Arc::new(Enrollment::new(
                authority,
                Arc::new(store),
                Arc::new(clock),
                EnrollmentPolicy {
                    certificate_lifetime: ChronoDuration::days(90),
                    retry_after: ChronoDuration::minutes(5),
                    require_channel_binding: false,
                },
            ))
        }
    };
    let settings = || EstSettings {
        bind: "127.0.0.1:0".parse().unwrap(),
        names: vec!["localhost".to_owned()],
        ..EstSettings::default()
    };

    // A machine with no real-time clock reads 1970 until a time service sets it. The server will not come up
    // with a certificate dated from that, which no display with the right time would accept.
    let unset = FixedClock(chrono::Utc.with_ymd_and_hms(1970, 1, 1, 0, 3, 0).unwrap());
    let error = EstServer::bind(
        settings(),
        authority.clone(),
        enrolment_at(unset, "unset").await,
        None,
    )
    .err()
    .expect("should refuse")
    .to_string();
    assert!(error.contains("can't be right"), "{error}");

    // The same server, once the clock is right.
    let set = FixedClock(chrono::Utc::now());
    assert!(
        EstServer::bind(
            settings(),
            authority.clone(),
            enrolment_at(set, "set").await,
            None
        )
        .is_ok()
    );
}

#[test]
fn the_servers_own_certificate_is_looked_at_at_least_once_a_minute() {
    // A clock corrected after start-up leaves the server presenting a certificate no display accepts until the
    // next look, so the look must be frequent. It is only a comparison of two dates.
    assert!(
        super::RENEWAL_CHECK <= Duration::from_secs(60),
        "{:?}",
        super::RENEWAL_CHECK
    );
}

// --- who a certificate names ---------------------------------------------------------------------------------

fn self_signed(common_name: Option<&str>) -> rustls::pki_types::CertificateDer<'static> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = rcgen::CertificateParams::default();
    params.distinguished_name = rcgen::DistinguishedName::new();
    if let Some(name) = common_name {
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, name);
    }
    params.self_signed(&key).unwrap().der().clone()
}

#[test]
fn a_client_with_no_certificate_is_anonymous_and_one_with_a_good_certificate_is_named() {
    assert_eq!(super::identity_of(None).unwrap(), None);
    assert_eq!(super::identity_of(Some(&[])).unwrap(), None);
    let good = self_signed(Some("kitchen"));
    let identity = super::identity_of(Some(std::slice::from_ref(&good)))
        .unwrap()
        .expect("named");
    assert_eq!(identity.device, device("kitchen"));
    // The key is the certificate's own, not anything the client says.
    let (_, parsed) = x509_parser::parse_x509_certificate(good.as_ref()).unwrap();
    assert_eq!(identity.key.as_der(), parsed.public_key().raw);
}

#[test]
fn a_certificate_that_names_no_usable_display_is_refused_not_served_as_anonymous() {
    for bad in [
        self_signed(Some("has a space")),
        self_signed(Some("")),
        self_signed(Some(&"x".repeat(64))),
        self_signed(None),
        rustls::pki_types::CertificateDer::from(vec![0x30, 0x03, 0x02, 0x01, 0x01]),
    ] {
        let error = super::identity_of(Some(&[bad])).unwrap_err().to_string();
        assert!(error.contains("does not name a display"), "{error}");
    }
}

// --- the size of a request's head ------------------------------------------------------------------------------

/// What the server answers to `head` sent as a request's line and headers, as its status (0 if it just closed).
async fn answer_to_head(harness: &Harness, head: String) -> u16 {
    let mut stream = connect(harness, Trust::Anything, None).await;
    let _ = stream.write_all(head.as_bytes()).await;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 2048];
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while let Ok(n) = stream.read(&mut chunk).await {
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..n]);
            if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
    })
    .await;
    String::from_utf8_lossy(&raw)
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[tokio::test]
async fn a_request_head_that_is_far_bigger_than_any_display_sends_is_refused() {
    let harness = start().await;
    let line = format!("GET {CSRATTRS} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    // A normal head is served.
    assert_eq!(answer_to_head(&harness, format!("{line}\r\n")).await, 200);
    // One header of 40 KB is not, and neither is a head of 60 small ones (hyper's own limit is 100).
    let big = format!("{line}X-Padding: {}\r\n\r\n", "a".repeat(40_000));
    let refused = answer_to_head(&harness, big).await;
    assert!(refused == 0 || refused >= 400, "answered {refused}");
    let many: String = (0..60).map(|i| format!("X-{i}: v\r\n")).collect();
    let refused = answer_to_head(&harness, format!("{line}{many}\r\n")).await;
    assert!(refused == 0 || refused >= 400, "answered {refused}");
}

#[tokio::test]
async fn a_head_with_a_sensible_number_of_headers_of_a_sensible_size_is_still_served() {
    let harness = start().await;
    let line = format!("GET {CSRATTRS} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    let some: String = (0..20)
        .map(|i| format!("X-{i}: {}\r\n", "v".repeat(200)))
        .collect();
    assert_eq!(
        answer_to_head(&harness, format!("{line}{some}\r\n")).await,
        200
    );
}

#[tokio::test]
async fn one_source_can_not_hold_all_the_connections() {
    let harness = start_with(false, |s| s.max_connections_per_source = 2).await;
    // Everything here comes from one address. Two idle connections are all it may have, though the server has room.
    let a = connect(&harness, Trust::Anything, None).await;
    let b = connect(&harness, Trust::Anything, None).await;
    let tcp = TcpStream::connect(harness.address).await.unwrap();
    let third = TlsConnector::from(Arc::new(client_config(
        Trust::Anything,
        None,
        &[&rustls::version::TLS13],
    )))
    .connect(ServerName::try_from("localhost").unwrap(), tcp)
    .await;
    assert!(third.is_err(), "a third from the same source is dropped");
    // A place given back is a place the source may use again.
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
async fn one_source_can_not_fill_the_places_for_waiting_displays() {
    let harness = start_with(false, |s| s.max_names_per_source = 2).await;
    harness.enrollment.open_window(ChronoDuration::minutes(10));
    let authority = harness.authority.certificate().to_vec();
    let trust = || Trust::Authority(&authority);
    let kitchen = new_key();

    assert_eq!(
        enroll(&harness, trust(), "kitchen", &kitchen).await.status,
        202
    );
    assert_eq!(
        enroll(&harness, trust(), "hall", &new_key()).await.status,
        202
    );
    // A third name from the same source is one too many, and says to wait and how long.
    let refused = enroll(&harness, trust(), "study", &new_key()).await;
    assert_eq!(refused.status, 429, "{}", refused.text());
    assert_eq!(refused.header("retry-after"), Some("600"));
    // It made nothing wait: the places are for displays, not for what is turned away.
    let waiting: Vec<_> = harness
        .enrollment
        .pairings()
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.device.to_string())
        .collect();
    assert_eq!(waiting.len(), 2, "{waiting:?}");
    assert!(!waiting.contains(&"study".to_owned()));
    // A display already waiting asks again as often as it likes.
    for _ in 0..5 {
        assert_eq!(
            enroll(&harness, trust(), "kitchen", &kitchen).await.status,
            202
        );
    }
}

#[tokio::test]
async fn a_request_that_is_not_signed_does_not_count_against_the_names_a_source_may_ask_as() {
    let harness = start_with(false, |s| s.max_names_per_source = 1).await;
    harness.enrollment.open_window(ChronoDuration::minutes(10));
    let authority = harness.authority.certificate().to_vec();
    // Nonsense in the right wrapper is refused for what it is, however many times.
    for _ in 0..3 {
        let mut stream = connect(&harness, Trust::Authority(&authority), None).await;
        let junk = send(
            &mut stream,
            "POST",
            ENROLL,
            Some(PKCS10),
            STANDARD.encode(b"hello").as_bytes(),
        )
        .await;
        assert_eq!(junk.status, 400);
    }
    assert_eq!(
        enroll(
            &harness,
            Trust::Authority(&authority),
            "kitchen",
            &new_key()
        )
        .await
        .status,
        202
    );
}

#[tokio::test]
async fn someone_asking_to_join_is_not_told_which_names_the_server_knows() {
    let harness = start().await;
    let authority = harness.authority.certificate().to_vec();
    let trust = || Trust::Authority(&authority);

    // Window closed: nobody is accepted.
    let closed = enroll(&harness, trust(), "garage", &new_key()).await;
    // A display the owner turned down.
    harness.enrollment.open_window(ChronoDuration::minutes(10));
    let turned_down_key = new_key();
    enroll(&harness, trust(), "attic", &turned_down_key).await;
    harness.enrollment.reject(&device("attic")).await.unwrap();
    let rejected = enroll(&harness, trust(), "attic", &turned_down_key).await;
    // A member that was revoked.
    let revoked_key = new_key();
    let code = PairingCode::derive(
        &Fingerprint::of(&authority),
        &device("cellar"),
        &PublicKey::from_der(revoked_key.subject_public_key_info()),
    );
    enroll(&harness, trust(), "cellar", &revoked_key).await;
    harness
        .enrollment
        .approve(&device("cellar"), &code)
        .await
        .unwrap();
    let granted = enroll(&harness, trust(), "cellar", &revoked_key).await;
    assert_eq!(granted.status, 200);
    harness.enrollment.revoke(&device("cellar")).await.unwrap();
    let revoked = enroll(&harness, trust(), "cellar", &revoked_key).await;

    for refusal in [&closed, &rejected, &revoked] {
        assert_eq!(refusal.status, 403, "{}", refusal.text());
        assert_eq!(refusal.text(), "The request was refused\n");
    }
}

#[tokio::test]
async fn a_display_that_showed_its_certificate_is_told_why_it_was_refused() {
    let harness = start().await;
    harness.enrollment.open_window(ChronoDuration::minutes(10));
    let key = new_key();
    let identity = join(&harness, "kitchen", key).await;
    harness.enrollment.revoke(&device("kitchen")).await.unwrap();
    let after = renew_as(&harness, &identity, "kitchen").await;
    assert_eq!(after.status, 403);
    assert_ne!(
        after.text(),
        "The request was refused\n",
        "{}",
        after.text()
    );
}
