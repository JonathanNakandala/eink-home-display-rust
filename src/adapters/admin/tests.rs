//! The admin API through a real local socket, the way `displayctl` reaches it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Duration, Utc};
use chrono_tz::Tz;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tempfile::TempDir;
use tokio::task::JoinHandle;

use super::*;
use crate::adapters::api_doc_testing as doc;
use crate::adapters::certificate_authority::{self, Create};
use crate::adapters::pairing_store::{FilePairingStore, Missing};
use crate::application::enrollment::{Enrollment, EnrollmentPolicy};

/// A temporary directory only its owner can use, as the PKI directory is: the admin socket refuses to live in
/// any other (this system's temporary directories are open to others).
fn private_directory() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    directory
}

/// A clock that stands still until it is moved, so a day can pass in a test.
struct TestClock(std::sync::Mutex<chrono::DateTime<Utc>>);

impl TestClock {
    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }
}

impl crate::domain::services::clock::Clock for TestClock {
    fn now(&self) -> chrono::DateTime<Tz> {
        self.0.lock().unwrap().with_timezone(&Tz::UTC)
    }
}

struct Fixture {
    _directory: TempDir,
    socket: PathBuf,
    enrollment: Arc<Enrollment>,
    authority: Arc<certificate_authority::PrivateAuthority>,
    clock: Arc<TestClock>,
    client: AdminClient,
    server: JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn enrollment_in(
    directory: &Path,
) -> (
    Arc<Enrollment>,
    Arc<certificate_authority::PrivateAuthority>,
    Arc<TestClock>,
) {
    let authority = Arc::new(
        certificate_authority::open(&directory.join("authority"), Create::IfMissing).unwrap(),
    );
    let store = FilePairingStore::open(&directory.join("pairings"), Missing::StartEmpty)
        .await
        .unwrap();
    let clock = Arc::new(TestClock(std::sync::Mutex::new(Utc::now())));
    let enrollment = Arc::new(Enrollment::new(
        authority.clone(),
        Arc::new(store),
        clock.clone(),
        EnrollmentPolicy {
            certificate_lifetime: Duration::days(90),
            retry_after: Duration::minutes(5),
            require_channel_binding: false,
        },
    ));
    (enrollment, authority, clock)
}

async fn start() -> Fixture {
    let directory = private_directory();
    let (enrollment, authority, clock) = enrollment_in(directory.path()).await;
    let socket = default_path(directory.path());
    let listener = bind(&socket).await.unwrap();
    let app = router(enrollment.clone());
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Fixture {
        client: AdminClient::new(&socket),
        _directory: directory,
        socket,
        enrollment,
        authority,
        clock,
        server,
    }
}

/// What a display holds: a name and a key of its own.
struct Display {
    name: String,
    key: rcgen::KeyPair,
}

impl Display {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            key: rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap(),
        }
    }

    /// The request it sends to join.
    fn request(&self) -> Vec<u8> {
        let mut params = rcgen::CertificateParams::default();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, self.name.as_str());
        params.serialize_request(&self.key).unwrap().der().to_vec()
    }

    /// What its own panel shows: worked out from what the display itself holds and saw.
    fn code(&self, f: &Fixture) -> String {
        use crate::domain::models::device_id::DeviceId;
        use crate::domain::models::pairing::{PairingCode, PublicKey};
        use crate::domain::services::certificate_authority::CertificateAuthority;
        use rcgen::PublicKeyData;
        PairingCode::derive(
            &f.authority.fingerprint(),
            &DeviceId::parse(&self.name).unwrap(),
            &PublicKey::from_der(self.key.subject_public_key_info()),
        )
        .to_string()
    }

    /// Asks to join, as it does on each wake.
    async fn asks(&self, f: &Fixture) -> Result<crate::application::enrollment::Outcome, String> {
        f.enrollment
            .enroll(&self.request(), None)
            .await
            .map_err(|e| e.to_string())
    }
}

/// One request exactly as given, whatever it is, and what came back.
async fn raw(
    socket: &Path,
    method: Method,
    path: &str,
    content_type: Option<&str>,
    body: &str,
) -> (StatusCode, String) {
    let stream = connect(socket).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    let driver = tokio::spawn(connection);
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(HOST, "localhost");
    if let Some(content_type) = content_type {
        request = request.header(CONTENT_TYPE, content_type);
    }
    let response = sender
        .send_request(
            request
                .body(Full::new(Bytes::from(body.to_owned())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let text = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    driver.abort();
    (status, text)
}

fn described() -> Value {
    serde_json::from_str(&openapi_json()).unwrap()
}

// --- the window -------------------------------------------------------------------------------------

#[tokio::test]
async fn the_window_starts_closed_and_opens_and_closes_on_request() {
    let f = start().await;
    assert_eq!(
        f.client.window().await.unwrap(),
        WindowState {
            open: false,
            closes_at: None
        }
    );

    let opened = f.client.open_window(15).await.unwrap();
    assert!(opened.open);
    let closes = opened.closes_at.unwrap();
    let expected = Utc::now() + Duration::minutes(15);
    assert!((closes - expected).num_seconds().abs() < 5, "{closes}");
    // It is the real window that opened, not only a report of one.
    assert_eq!(f.enrollment.window_closes_at(), Some(closes));
    assert_eq!(f.client.window().await.unwrap(), opened);

    let closed = f.client.close_window().await.unwrap();
    assert_eq!(
        closed,
        WindowState {
            open: false,
            closes_at: None
        }
    );
    assert_eq!(f.enrollment.window_closes_at(), None);
    // Closing a closed window is not an error.
    assert!(!f.client.close_window().await.unwrap().open);
}

#[tokio::test]
async fn opening_it_again_sets_the_time_afresh() {
    let f = start().await;
    let short = f.client.open_window(5).await.unwrap().closes_at.unwrap();
    let long = f.client.open_window(60).await.unwrap().closes_at.unwrap();
    assert!(long - short > Duration::minutes(50), "{short} {long}");
    let shorter = f.client.open_window(10).await.unwrap().closes_at.unwrap();
    assert!(shorter < long, "a shorter window shortens it");
}

#[tokio::test]
async fn the_longest_and_shortest_windows_are_allowed() {
    let f = start().await;
    assert!(f.client.open_window(1).await.unwrap().open);
    assert!(f.client.open_window(MAX_WINDOW_MINUTES).await.unwrap().open);
}

#[tokio::test]
async fn minutes_outside_the_range_are_refused_with_a_code_and_change_nothing() {
    let f = start().await;
    let before = f.client.open_window(20).await.unwrap();
    for minutes in [0, MAX_WINDOW_MINUTES + 1, u32::MAX] {
        match f.client.open_window(minutes).await {
            Err(CallError::Refused { status, error }) => {
                assert_eq!(status, 400, "{minutes}");
                assert_eq!(error.error.code, ErrorCode::InvalidMinutes, "{minutes}");
                assert!(
                    error.error.message.contains("240"),
                    "{}",
                    error.error.message
                );
            }
            other => panic!("{minutes}: {other:?}"),
        }
    }
    assert_eq!(
        f.client.window().await.unwrap(),
        before,
        "a refusal changes nothing"
    );
}

#[tokio::test]
async fn a_request_that_is_not_understood_gets_the_same_kind_of_answer() {
    let f = start().await;
    let bodies = [
        ("", Some("application/json")),
        ("not json", Some("application/json")),
        ("{}", Some("application/json")),
        (r#"{"minutes": "ten"}"#, Some("application/json")),
        (r#"{"minutes": -5}"#, Some("application/json")),
        (r#"{"minutes": 1.5}"#, Some("application/json")),
        (r#"{"minutes": 15}"#, None),
        (r#"{"minutes": 15}"#, Some("text/plain")),
    ];
    for (body, content_type) in bodies {
        let (status, text) = raw(&f.socket, Method::PUT, "/v1/window", content_type, body).await;
        assert_eq!(status, 400, "{body:?} as {content_type:?}: {text}");
        let error: ApiError = serde_json::from_str(&text).unwrap_or_else(|e| {
            panic!("{body:?} as {content_type:?}: {text} is not an error body: {e}")
        });
        assert_eq!(error.error.code, ErrorCode::InvalidRequest, "{body:?}");
    }
    // A body bigger than any request needs is turned away the same way.
    let huge = format!(r#"{{"minutes": 15, "padding": "{}"}}"#, "x".repeat(10_000));
    let (status, text) = raw(
        &f.socket,
        Method::PUT,
        "/v1/window",
        Some("application/json"),
        &huge,
    )
    .await;
    assert_eq!(status, 400, "{text}");
    assert!(
        !f.client.window().await.unwrap().open,
        "none of these opened it"
    );
}

#[tokio::test]
async fn an_unknown_path_and_a_wrong_method_answer_in_the_same_form() {
    let f = start().await;
    let (status, text) = raw(&f.socket, Method::GET, "/v1/nothing", None, "").await;
    assert_eq!(status, 404);
    assert_eq!(
        serde_json::from_str::<ApiError>(&text).unwrap().error.code,
        ErrorCode::NotFound
    );
    let (status, text) = raw(&f.socket, Method::POST, "/v1/window", None, "").await;
    assert_eq!(status, 405);
    assert_eq!(
        serde_json::from_str::<ApiError>(&text).unwrap().error.code,
        ErrorCode::MethodNotAllowed
    );
}

// --- the description ----------------------------------------------------------------------------------

#[test]
fn the_committed_description_is_current() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/admin-openapi.json");
    assert_eq!(
        std::fs::read_to_string(path).unwrap_or_default(),
        openapi_json(),
        "config/admin-openapi.json is stale: run `cargo run --bin gen_config -- --write`"
    );
}

#[test]
fn every_reference_in_the_description_points_at_something() {
    assert_eq!(doc::dangling_references(&described()), Vec::<String>::new());
}

#[test]
fn every_example_in_the_description_satisfies_its_own_schema() {
    let all = doc::examples(&described());
    assert!(all.len() >= 3, "only {} examples found", all.len());
    for (place, schema, example) in all {
        assert_eq!(
            doc::check(&described(), &schema, &example),
            Vec::<String>::new(),
            "{place}: {example}"
        );
    }
}

#[test]
fn the_description_lists_exactly_the_routes_there_are() {
    let api = described();
    let mut routes: Vec<(String, String, Vec<String>)> = Vec::new();
    for (path, item) in api["paths"].as_object().unwrap() {
        for (method, operation) in item.as_object().unwrap() {
            routes.push((
                method.to_uppercase(),
                path.clone(),
                operation["responses"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect(),
            ));
        }
    }
    routes.sort();
    // The statuses each can answer with: 400 only where there is input to get wrong, 404 and 409 only where a
    // display is named, 403 only where a code is checked.
    let expected: [(&str, &str, &[&str]); 9] = [
        (
            "DELETE",
            "/v1/displays/{name}",
            &["200", "400", "404", "500"],
        ),
        ("DELETE", "/v1/window", &["200", "500"]),
        ("GET", "/v1/displays", &["200", "500"]),
        ("GET", "/v1/displays/{name}", &["200", "400", "404", "500"]),
        ("GET", "/v1/window", &["200", "500"]),
        (
            "POST",
            "/v1/displays/{name}/approve",
            &["200", "400", "403", "404", "409", "429", "500"],
        ),
        (
            "POST",
            "/v1/displays/{name}/reject",
            &["200", "400", "404", "409", "500"],
        ),
        (
            "POST",
            "/v1/displays/{name}/revoke",
            &["200", "400", "404", "409", "500"],
        ),
        ("PUT", "/v1/window", &["200", "400", "500"]),
    ];
    assert_eq!(routes.len(), expected.len(), "{routes:?}");
    for (route, (method, path, statuses)) in routes.iter().zip(expected) {
        assert_eq!((route.0.as_str(), route.1.as_str()), (method, path));
        assert_eq!(route.2, statuses, "{method} {path}");
    }
}

#[test]
fn the_longest_window_in_the_description_is_the_one_the_code_allows() {
    let schema = &described()["components"]["schemas"]["OpenWindow"]["properties"]["minutes"];
    assert_eq!(schema["maximum"], MAX_WINDOW_MINUTES);
    assert_eq!(schema["minimum"], 1);
}

#[tokio::test]
async fn what_the_server_really_sends_matches_the_description() {
    let f = start().await;
    let api = described();
    let declared = |method: &str, status: StatusCode| {
        api["paths"]["/v1/window"][method]["responses"]
            .get(status.as_str())
            .cloned()
    };
    let body_schema = |method: &str, status: StatusCode| {
        declared(method, status).unwrap_or_else(|| panic!("{method} {status} is not described"))["content"]
            ["application/json"]["schema"]
            .clone()
    };

    for (method, body) in [
        (Method::GET, ""),
        (Method::PUT, r#"{"minutes": 15}"#),
        (Method::DELETE, ""),
    ] {
        let (status, text) = raw(
            &f.socket,
            method.clone(),
            "/v1/window",
            Some("application/json"),
            body,
        )
        .await;
        let json: Value = serde_json::from_str(&text).unwrap();
        let schema = body_schema(&method.as_str().to_lowercase(), status);
        assert_eq!(
            doc::check(&api, &schema, &json),
            Vec::<String>::new(),
            "{method}: {text}"
        );
    }
    for body in ["", r#"{"minutes": 0}"#, r#"{"minutes": 9999}"#] {
        let (status, text) = raw(
            &f.socket,
            Method::PUT,
            "/v1/window",
            Some("application/json"),
            body,
        )
        .await;
        let json: Value = serde_json::from_str(&text).unwrap();
        let schema = body_schema("put", status);
        assert_eq!(
            doc::check(&api, &schema, &json),
            Vec::<String>::new(),
            "{body:?}: {text}"
        );
    }
}

#[tokio::test]
async fn the_description_is_served_on_the_socket_as_committed() {
    let f = start().await;
    let (status, text) = raw(&f.socket, Method::GET, "/openapi.json", None, "").await;
    assert_eq!(status, 200);
    assert_eq!(text, openapi_json());
}

// --- the socket -----------------------------------------------------------------------------------------

#[cfg(unix)]
mod socket {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    use super::*;

    #[tokio::test]
    async fn only_its_owner_can_use_the_socket() {
        let f = start().await;
        let metadata = std::fs::metadata(&f.socket).unwrap();
        assert!(metadata.file_type().is_socket());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }

    #[tokio::test]
    async fn the_socket_goes_when_the_server_does() {
        let directory = private_directory();
        let path = default_path(directory.path());
        let listener = bind(&path).await.unwrap();
        assert!(path.exists());
        drop(listener);
        assert!(!path.exists(), "a socket nobody listens on is left behind");
    }

    #[tokio::test]
    async fn a_second_server_in_the_same_directory_is_refused_and_the_first_is_untouched() {
        let f = start().await;
        let error = bind(&f.socket)
            .await
            .err()
            .expect("should refuse")
            .to_string();
        assert!(error.contains("already listening"), "{error}");
        assert!(
            f.client.window().await.is_ok(),
            "the first server still answers"
        );
    }

    #[tokio::test]
    async fn a_socket_left_by_a_server_that_died_is_replaced() {
        let directory = private_directory();
        let path = default_path(directory.path());
        {
            // A listener that exits without cleaning up, as a crash does: the file stays, nothing listens.
            let dead = std::os::unix::net::UnixListener::bind(&path).unwrap();
            drop(dead);
        }
        assert!(path.exists());
        let listener = bind(&path).await.unwrap();
        // And it works.
        let (enrollment, _, _) = enrollment_in(directory.path()).await;
        let app = router(enrollment);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        assert!(AdminClient::new(&path).window().await.is_ok());
        server.abort();
    }

    #[tokio::test]
    async fn something_that_is_not_a_socket_is_never_removed() {
        let directory = private_directory();
        let path = default_path(directory.path());
        std::fs::write(&path, "precious").unwrap();
        let error = bind(&path).await.err().expect("should refuse").to_string();
        assert!(error.contains("not a socket"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "precious");
    }

    #[tokio::test]
    async fn a_directory_other_users_can_use_is_refused() {
        // The mode of a socket is ignored on some systems, so the directory is what must keep others out.
        for open in [0o755, 0o750, 0o701, 0o770] {
            let directory = private_directory();
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(open))
                .unwrap();
            let path = default_path(directory.path());
            let error = bind(&path).await.err().expect("should refuse").to_string();
            assert!(error.contains("chmod 700"), "{open:o}: {error}");
            assert!(!path.exists(), "{open:o}: nothing was made");
        }
        // Closed to everyone else, it is fine, whatever the owner's own bits are.
        let directory = private_directory();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let path = default_path(directory.path());
        let refused = bind(&path)
            .await
            .err()
            .expect("a directory it cannot write in")
            .to_string();
        assert!(!refused.contains("chmod 700"), "{refused}");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[tokio::test]
    async fn a_path_too_long_for_a_socket_says_so_and_what_to_do() {
        let directory = private_directory();
        let long = directory.path().join("x".repeat(120)).join("admin.sock");
        let error = bind(&long).await.err().expect("should refuse").to_string();
        assert!(
            error.contains("bytes") && error.contains("[server.admin] socket"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn with_no_server_the_client_says_it_could_not_be_reached() {
    let directory = private_directory();
    let client = AdminClient::new(default_path(directory.path()));
    match client.window().await {
        Err(CallError::Unreachable(e)) => {
            assert!(format!("{e:#}").contains("admin.sock"), "{e:#}");
        }
        other => panic!("{other:?}"),
    }
}

// --- what a person reads ----------------------------------------------------------------------------------

#[test]
fn the_window_is_described_in_a_sentence_with_the_time_in_the_zone_asked_for() {
    let now = Utc::now();
    let open = WindowState {
        open: true,
        closes_at: Some(now + Duration::minutes(14) + Duration::seconds(5)),
    };
    let utc = describe_window(&open, now, &Utc);
    assert!(utc.contains("is open until"), "{utc}");
    assert!(utc.contains("14 min 05 s from now"), "{utc}");
    assert!(
        utc.contains(&open.closes_at.unwrap().format("%H:%M:%S").to_string()),
        "{utc}"
    );
    // In another zone the clock time is that zone's.
    let plus_five = chrono::FixedOffset::east_opt(5 * 3600).unwrap();
    let there = describe_window(&open, now, &plus_five);
    let expected = open
        .closes_at
        .unwrap()
        .with_timezone(&plus_five)
        .format("%H:%M:%S")
        .to_string();
    assert!(there.contains(&expected), "{there}");

    let closed = describe_window(
        &WindowState {
            open: false,
            closes_at: None,
        },
        now,
        &Utc,
    );
    assert!(
        closed.contains("closed") && closed.contains("displayctl window open"),
        "{closed}"
    );
    // A time in the past is never shown as negative.
    let late = WindowState {
        open: true,
        closes_at: Some(now - Duration::seconds(30)),
    };
    assert!(describe_window(&late, now, &Utc).contains("0 min 00 s"));
}

// --- displays ---------------------------------------------------------------------------------------------

/// A display that is a member: it asked, was approved, and collected its certificate.
async fn member(f: &Fixture, name: &str) -> Display {
    let display = Display::new(name);
    f.enrollment.open_window(Duration::minutes(10));
    display.asks(f).await.unwrap();
    f.client.approve(name, &display.code(f)).await.unwrap();
    assert!(matches!(
        display.asks(f).await.unwrap(),
        crate::application::enrollment::Outcome::Issued(_)
    ));
    display
}

async fn waiting(f: &Fixture, name: &str) -> Display {
    let display = Display::new(name);
    f.enrollment.open_window(Duration::minutes(10));
    display.asks(f).await.unwrap();
    display
}

fn code_of(error: CallError) -> ErrorCode {
    match error {
        CallError::Refused { error, .. } => error.error.code,
        other => panic!("not a refusal: {other:?}"),
    }
}

#[tokio::test]
async fn no_display_is_listed_until_one_asks() {
    let f = start().await;
    assert_eq!(f.client.displays().await.unwrap().displays, []);
    waiting(&f, "kitchen").await;
    let list = f.client.displays().await.unwrap();
    assert_eq!(list.displays.len(), 1);
    let entry = &list.displays[0];
    assert_eq!(entry.name, "kitchen");
    assert_eq!(entry.state, DisplayState::Waiting);
    let asked = entry.waiting_since.unwrap();
    assert!((Utc::now() - asked).num_seconds().abs() < 5, "{asked}");
    assert_eq!(f.client.display("kitchen").await.unwrap(), *entry);
}

#[tokio::test]
async fn nothing_that_comes_back_from_the_api_is_a_code_a_key_or_a_certificate() {
    // The owner reads the code off the display's panel. If the server gave it, it would be copied from here, and
    // the check that panel and server agree would never be made.
    let f = start().await;
    let display = waiting(&f, "kitchen").await;
    let code = display.code(&f);
    let plain = code.replace('-', "");
    let mut everything = Vec::new();
    for path in [
        "/v1/displays",
        "/v1/displays/kitchen",
        "/v1/window",
        "/openapi.json",
    ] {
        everything.push(raw(&f.socket, Method::GET, path, None, "").await.1);
    }
    // A wrong code is answered without repeating either code.
    let (_, refused) = raw(
        &f.socket,
        Method::POST,
        "/v1/displays/kitchen/approve",
        Some("application/json"),
        r#"{"code": "0000-0000-0000"}"#,
    )
    .await;
    everything.push(refused);
    for text in everything {
        assert!(!text.contains(&code) && !text.contains(&plain), "{text}");
        let lower = text.to_lowercase();
        assert!(
            !lower.contains("begin certificate") && !lower.contains("private key"),
            "{text}"
        );
    }
    // And no field of a display's entry is one that could hold them.
    let entry: Value = serde_json::from_str(
        &raw(&f.socket, Method::GET, "/v1/displays/kitchen", None, "")
            .await
            .1,
    )
    .unwrap();
    let mut fields: Vec<&str> = entry
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort();
    assert_eq!(
        fields,
        [
            "certificate_expired",
            "certificate_not_after",
            "changing_keys",
            "name",
            "renewal_overdue",
            "replacement_waiting",
            "state",
            "waiting_since"
        ]
    );
}

#[tokio::test]
async fn the_right_code_approves_and_the_display_then_collects_its_certificate() {
    let f = start().await;
    let kitchen = waiting(&f, "kitchen").await;
    let approved = f
        .client
        .approve("kitchen", &kitchen.code(&f))
        .await
        .unwrap();
    assert_eq!(approved.state, DisplayState::Approved);
    // The real thing happened: the next ask is granted a certificate.
    assert!(matches!(
        kitchen.asks(&f).await.unwrap(),
        crate::application::enrollment::Outcome::Issued(_)
    ));
    assert_eq!(
        f.client.display("kitchen").await.unwrap().state,
        DisplayState::Member
    );
}

#[tokio::test]
async fn wrong_codes_in_a_row_are_refused_with_429_and_say_for_how_long() {
    let f = start().await;
    let kitchen = waiting(&f, "kitchen").await;
    let wrong = Display::new("kitchen").code(&f);
    for _ in 1..crate::application::enrollment::MAX_WRONG_CODES {
        match f.client.approve("kitchen", &wrong).await {
            Err(CallError::Refused { status, error }) => {
                assert_eq!(status, 403);
                assert_eq!(error.error.code, ErrorCode::WrongCode);
            }
            other => panic!("{other:?}"),
        }
    }
    // The one that makes the run: refused for good measure, and told why and for how long.
    match f.client.approve("kitchen", &wrong).await {
        Err(CallError::Refused { status, error }) => {
            assert_eq!(status, 429);
            assert_eq!(error.error.code, ErrorCode::TooManyAttempts);
            assert!(
                error.error.message.contains("15 more minute"),
                "{}",
                error.error.message
            );
        }
        other => panic!("{other:?}"),
    }
    // Locked: even the right code waits.
    match f.client.approve("kitchen", &kitchen.code(&f)).await {
        Err(CallError::Refused { status, error }) => {
            assert_eq!(status, 429);
            assert_eq!(error.error.code, ErrorCode::TooManyAttempts);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        f.client.display("kitchen").await.unwrap().state,
        DisplayState::Waiting
    );
}

#[tokio::test]
async fn a_code_typed_the_way_people_type_it_is_accepted() {
    let f = start().await;
    for (name, mangle) in [
        ("lower", (|c: &str| c.to_lowercase()) as fn(&str) -> String),
        ("no-dashes", |c| c.replace('-', "")),
        ("spaces", |c| c.replace('-', " ")),
        ("look-alikes", |c| c.replace('1', "l").replace('0', "o")),
    ] {
        let display = waiting(&f, name).await;
        let typed = mangle(&display.code(&f));
        let result = f.client.approve(name, &typed).await;
        assert!(result.is_ok(), "{name}: {typed}: {result:?}");
    }
}

#[tokio::test]
async fn a_wrong_code_is_refused_says_what_to_check_and_changes_nothing() {
    let f = start().await;
    let kitchen = waiting(&f, "kitchen").await;
    let other = Display::new("kitchen"); // same name, a different key: its panel would show another code
    let wrong = other.code(&f);
    assert_ne!(wrong, kitchen.code(&f));
    match f.client.approve("kitchen", &wrong).await {
        Err(CallError::Refused { status, error }) => {
            assert_eq!(status, 403);
            assert_eq!(error.error.code, ErrorCode::WrongCode);
            assert!(
                error.error.message.contains("panel"),
                "{}",
                error.error.message
            );
            assert!(
                !error.error.message.contains(&wrong),
                "the wrong code is not repeated"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        f.client.display("kitchen").await.unwrap().state,
        DisplayState::Waiting
    );
    // The right one still works afterwards.
    f.client
        .approve("kitchen", &kitchen.code(&f))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_code_that_is_not_a_code_is_refused_before_any_is_tried() {
    let f = start().await;
    waiting(&f, "kitchen").await;
    for typed in [
        "",
        "abc",
        "B0AJ-QTW6",
        "JGWP-14YW-3BT0-X",
        "B0AJ-QTW6-Y8SU",
        "not a code at all",
    ] {
        let error = f.client.approve("kitchen", typed).await.unwrap_err();
        assert_eq!(code_of(error), ErrorCode::InvalidCode, "{typed:?}");
    }
}

#[tokio::test]
async fn a_name_that_cannot_be_a_displays_is_refused_and_one_that_is_not_known_is_not_found() {
    let f = start().await;
    for (method, path, body) in [
        (Method::GET, "/v1/displays/has%20space", ""),
        (Method::DELETE, "/v1/displays/%3Cscript%3E", ""),
        (Method::POST, "/v1/displays/a%20b/revoke", ""),
        (
            Method::POST,
            "/v1/displays/a%20b/approve",
            r#"{"code": "JGWP-14YW-3BT0"}"#,
        ),
        (Method::GET, &format!("/v1/displays/{}", "x".repeat(33)), ""),
    ] {
        let (status, text) = raw(
            &f.socket,
            method.clone(),
            path,
            Some("application/json"),
            body,
        )
        .await;
        assert_eq!(status, 400, "{method} {path}: {text}");
        assert_eq!(
            serde_json::from_str::<ApiError>(&text).unwrap().error.code,
            ErrorCode::InvalidName
        );
    }
    // Names of only dots are not names (and so are never taken for "here" or "up" anywhere): `.` and `..`.
    for dots in ["%2e", "%2e%2e", "...."] {
        let (status, text) = raw(
            &f.socket,
            Method::GET,
            &format!("/v1/displays/{dots}"),
            None,
            "",
        )
        .await;
        assert_eq!(status, 400, "{dots}: {text}");
        assert_eq!(
            serde_json::from_str::<ApiError>(&text).unwrap().error.code,
            ErrorCode::InvalidName,
            "{dots}"
        );
    }
    assert_eq!(
        code_of(f.client.display("nobody").await.unwrap_err()),
        ErrorCode::UnknownDisplay
    );
    assert_eq!(
        code_of(
            f.client
                .approve("nobody", "JGWP-14YW-3BT0")
                .await
                .unwrap_err()
        ),
        ErrorCode::UnknownDisplay
    );
    assert_eq!(
        code_of(f.client.reject("nobody").await.unwrap_err()),
        ErrorCode::UnknownDisplay
    );
    assert_eq!(
        code_of(f.client.revoke("nobody").await.unwrap_err()),
        ErrorCode::UnknownDisplay
    );
    assert_eq!(
        code_of(f.client.forget("nobody").await.unwrap_err()),
        ErrorCode::UnknownDisplay
    );
    // The client catches a name that can't be one without asking.
    assert!(matches!(
        f.client.display("has space").await,
        Err(CallError::Invalid(_))
    ));
}

#[tokio::test]
async fn only_a_waiting_display_can_be_approved_or_turned_down_and_only_a_member_revoked() {
    let f = start().await;
    let kitchen = member(&f, "kitchen").await;
    let hall = waiting(&f, "hall").await;
    assert_eq!(
        code_of(
            f.client
                .approve("kitchen", &kitchen.code(&f))
                .await
                .unwrap_err()
        ),
        ErrorCode::NotWaiting
    );
    assert_eq!(
        code_of(f.client.reject("kitchen").await.unwrap_err()),
        ErrorCode::NotWaiting
    );
    assert_eq!(
        code_of(f.client.revoke("hall").await.unwrap_err()),
        ErrorCode::NotAMember
    );
    let _ = hall;
    // Nothing changed.
    assert_eq!(
        f.client.display("kitchen").await.unwrap().state,
        DisplayState::Member
    );
    assert_eq!(
        f.client.display("hall").await.unwrap().state,
        DisplayState::Waiting
    );
}

#[tokio::test]
async fn turning_a_waiting_display_down_keeps_it_out_until_it_is_forgotten() {
    let f = start().await;
    let hall = waiting(&f, "hall").await;
    assert_eq!(
        f.client.reject("hall").await.unwrap().state,
        DisplayState::Rejected
    );
    f.enrollment.open_window(Duration::minutes(10));
    assert!(
        hall.asks(&f).await.is_err(),
        "a display turned down is refused when it asks again"
    );
    f.client.forget("hall").await.unwrap();
    assert!(matches!(
        hall.asks(&f).await.unwrap(),
        crate::application::enrollment::Outcome::Pending { .. }
    ));
}

#[tokio::test]
async fn turning_down_another_key_for_a_members_name_leaves_the_member_as_it_was() {
    let f = start().await;
    let kitchen = member(&f, "kitchen").await;
    let impostor = Display::new("kitchen");
    f.enrollment.open_window(Duration::minutes(10));
    impostor.asks(&f).await.unwrap();
    let before = f.client.display("kitchen").await.unwrap();
    assert!(before.replacement_waiting);
    assert_eq!(before.state, DisplayState::Member);
    assert!(before.waiting_since.is_some());

    let after = f.client.reject("kitchen").await.unwrap();
    assert_eq!(after.state, DisplayState::Member);
    assert!(!after.replacement_waiting);
    // The real member still works.
    assert!(
        f.enrollment
            .authenticate(
                &crate::domain::models::device_id::DeviceId::parse("kitchen").unwrap(),
                &crate::domain::models::pairing::PublicKey::from_der({
                    use rcgen::PublicKeyData;
                    kitchen.key.subject_public_key_info()
                })
            )
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn revoking_ends_membership_at_once() {
    use crate::domain::models::device_id::DeviceId;
    use crate::domain::models::pairing::PublicKey;
    use rcgen::PublicKeyData;
    let f = start().await;
    let kitchen = member(&f, "kitchen").await;
    let device = DeviceId::parse("kitchen").unwrap();
    let key = PublicKey::from_der(kitchen.key.subject_public_key_info());
    assert!(f.enrollment.authenticate(&device, &key).await.unwrap());

    assert_eq!(
        f.client.revoke("kitchen").await.unwrap().state,
        DisplayState::Revoked
    );
    assert!(!f.enrollment.authenticate(&device, &key).await.unwrap());
    // Revoking twice is not a member to revoke.
    assert_eq!(
        code_of(f.client.revoke("kitchen").await.unwrap_err()),
        ErrorCode::NotAMember
    );
}

#[tokio::test]
async fn forgetting_removes_a_display_and_says_what_it_was() {
    let f = start().await;
    member(&f, "kitchen").await;
    let forgotten = f.client.forget("kitchen").await.unwrap();
    assert_eq!(forgotten.name, "kitchen");
    assert_eq!(forgotten.state, DisplayState::Member);
    assert_eq!(
        code_of(f.client.display("kitchen").await.unwrap_err()),
        ErrorCode::UnknownDisplay
    );
    assert_eq!(f.client.displays().await.unwrap().displays, []);
}

#[tokio::test]
async fn displays_are_listed_by_name_with_each_ones_standing() {
    let f = start().await;
    member(&f, "kitchen").await;
    waiting(&f, "attic").await;
    member(&f, "hall").await;
    f.client.revoke("hall").await.unwrap();
    let list = f.client.displays().await.unwrap();
    let seen: Vec<(&str, DisplayState)> = list
        .displays
        .iter()
        .map(|e| (e.name.as_str(), e.state))
        .collect();
    assert_eq!(
        seen,
        [
            ("attic", DisplayState::Waiting),
            ("hall", DisplayState::Revoked),
            ("kitchen", DisplayState::Member)
        ]
    );
    let kitchen = list.displays.iter().find(|e| e.name == "kitchen").unwrap();
    let ends = kitchen.certificate_not_after.unwrap();
    assert!(
        (ends - (Utc::now() + Duration::days(90)))
            .num_minutes()
            .abs()
            < 5,
        "{ends}"
    );
    assert!(!kitchen.certificate_expired);
}

#[tokio::test]
async fn a_member_that_has_not_renewed_when_it_should_have_is_listed_as_overdue_until_its_certificate_ends()
 {
    let f = start().await;
    member(&f, "kitchen").await;
    // The display renews with 30 of the 90 days left, so a member 50 days in is fine, and 70 days in it has not.
    f.clock.advance(Duration::days(50));
    assert!(!f.client.display("kitchen").await.unwrap().renewal_overdue);
    f.clock.advance(Duration::days(20));
    let late = f.client.display("kitchen").await.unwrap();
    assert!(late.renewal_overdue);
    assert!(!late.certificate_expired, "still has a certificate");
    // Out: the louder state replaces it.
    f.clock.advance(Duration::days(21));
    let out = f.client.display("kitchen").await.unwrap();
    assert!(out.certificate_expired);
    assert!(!out.renewal_overdue);
}

#[tokio::test]
async fn a_member_whose_certificate_has_run_out_is_listed_as_one_that_has() {
    let f = start().await;
    member(&f, "kitchen").await;
    f.clock.advance(Duration::days(91));
    let entry = f.client.display("kitchen").await.unwrap();
    assert_eq!(
        entry.state,
        DisplayState::Member,
        "running out is not being unpaired"
    );
    assert!(entry.certificate_expired);
}

#[tokio::test]
async fn a_request_nobody_answers_lapses_and_disappears_from_the_list() {
    let f = start().await;
    let kitchen = waiting(&f, "kitchen").await;
    f.clock
        .advance(crate::application::enrollment::REQUEST_LIFETIME - Duration::minutes(1));
    assert_eq!(f.client.displays().await.unwrap().displays.len(), 1);
    f.clock.advance(Duration::minutes(2));
    assert_eq!(f.client.displays().await.unwrap().displays, []);
    assert_eq!(
        code_of(
            f.client
                .approve("kitchen", &kitchen.code(&f))
                .await
                .unwrap_err()
        ),
        ErrorCode::UnknownDisplay
    );
}

// --- the descriptions of the display endpoints ----------------------------------------------------------------

#[tokio::test]
async fn every_answer_the_display_endpoints_give_is_one_the_description_declares() {
    let f = start().await;
    let api = described();
    let kitchen = member(&f, "kitchen").await;
    let hall = waiting(&f, "hall").await;
    let impostor = Display::new("kitchen");
    f.enrollment.open_window(Duration::minutes(10));
    impostor.asks(&f).await.unwrap();
    let right = hall.code(&f);
    let _ = kitchen;

    let cases: Vec<(Method, String, &str)> = vec![
        (Method::GET, "/v1/displays".into(), ""),
        (Method::GET, "/v1/displays/kitchen".into(), ""),
        (Method::GET, "/v1/displays/nobody".into(), ""),
        (Method::GET, "/v1/displays/has%20space".into(), ""),
        (
            Method::POST,
            "/v1/displays/hall/approve".into(),
            r#"{"code": "0000-0000-0000"}"#,
        ),
        (
            Method::POST,
            "/v1/displays/hall/approve".into(),
            r#"{"code": "nope"}"#,
        ),
        (Method::POST, "/v1/displays/hall/approve".into(), "not json"),
        (
            Method::POST,
            "/v1/displays/nobody/approve".into(),
            r#"{"code": "JGWP-14YW-3BT0"}"#,
        ),
        (Method::POST, "/v1/displays/kitchen/revoke".into(), ""),
        (Method::POST, "/v1/displays/kitchen/revoke".into(), ""),
        (Method::POST, "/v1/displays/hall/revoke".into(), ""),
        (Method::POST, "/v1/displays/nobody/revoke".into(), ""),
        (Method::POST, "/v1/displays/hall/reject".into(), ""),
        (Method::POST, "/v1/displays/hall/reject".into(), ""),
        (Method::POST, "/v1/displays/nobody/reject".into(), ""),
        (Method::DELETE, "/v1/displays/hall".into(), ""),
        (Method::DELETE, "/v1/displays/hall".into(), ""),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for (method, path, body) in cases {
        let (status, text) = raw(
            &f.socket,
            method.clone(),
            &path,
            Some("application/json"),
            body,
        )
        .await;
        let template = path
            .replacen("/kitchen", "/{name}", 1)
            .replacen("/hall", "/{name}", 1)
            .replacen("/nobody", "/{name}", 1)
            .replacen("/has%20space", "/{name}", 1);
        let operation = &api["paths"][template.as_str()][method.as_str().to_lowercase()];
        let declared = operation["responses"]
            .get(status.as_str())
            .unwrap_or_else(|| {
                panic!("{method} {path} answered {status}, which is not declared: {text}")
            });
        seen.insert((method.to_string(), template.clone(), status.as_u16()));
        let schema = &declared["content"]["application/json"]["schema"];
        assert_eq!(
            doc::check(&api, schema, &serde_json::from_str(&text).unwrap()),
            Vec::<String>::new(),
            "{method} {path} {status}: {text}"
        );
    }
    // The cases reached the answers that matter, not only the easy ones.
    for expected in [
        ("GET", "/v1/displays", 200),
        ("GET", "/v1/displays/{name}", 404),
        ("GET", "/v1/displays/{name}", 400),
        ("POST", "/v1/displays/{name}/approve", 403),
        ("POST", "/v1/displays/{name}/approve", 400),
        ("POST", "/v1/displays/{name}/approve", 404),
        ("POST", "/v1/displays/{name}/revoke", 200),
        ("POST", "/v1/displays/{name}/revoke", 409),
        ("POST", "/v1/displays/{name}/reject", 200),
        ("DELETE", "/v1/displays/{name}", 200),
    ] {
        assert!(
            seen.contains(&(expected.0.to_owned(), expected.1.to_owned(), expected.2)),
            "{expected:?} was never reached: {seen:?}"
        );
    }
    // And the one answer that approves, last, so the others were all made while hall was still waiting.
    let (status, text) = raw(
        &f.socket,
        Method::POST,
        "/v1/displays/hall/approve",
        Some("application/json"),
        &format!(r#"{{"code": "{right}"}}"#),
    )
    .await;
    assert_eq!(status, 404, "hall was forgotten above: {text}");
}

#[test]
fn every_display_state_is_labelled_as_the_description_names_it() {
    let schema = &described()["components"]["schemas"]["DisplayState"];
    let named: Vec<&str> = schema["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for state in [
        DisplayState::Waiting,
        DisplayState::Approved,
        DisplayState::Member,
        DisplayState::Rejected,
        DisplayState::Revoked,
    ] {
        assert!(
            named.contains(&label(state)),
            "{state:?} is labelled {}, not one of {named:?}",
            label(state)
        );
        assert_eq!(serde_json::to_value(state).unwrap(), label(state));
    }
    assert_eq!(named.len(), 5);
}

// --- what is said ----------------------------------------------------------------------------------------------

fn entry(state: DisplayState) -> DisplayEntry {
    DisplayEntry {
        name: "kitchen".to_owned(),
        state,
        waiting_since: None,
        certificate_not_after: None,
        certificate_expired: false,
        renewal_overdue: false,
        changing_keys: false,
        replacement_waiting: false,
    }
}

#[test]
fn spans_of_time_are_written_in_the_units_that_matter() {
    for (seconds, expected) in [
        (0, "less than a minute"),
        (59, "less than a minute"),
        (60, "1 min"),
        (3 * 60 + 30, "3 min"),
        (3600, "1 h"),
        (2 * 3600 + 5 * 60, "2 h 5 min"),
        (86_400, "1 day"),
        (86_400 + 3 * 3600, "1 day 3 h"),
        (3 * 86_400, "3 days"),
        (3 * 86_400 + 5 * 3600, "3 days 5 h"),
        (-100, "less than a minute"),
    ] {
        assert_eq!(age(Duration::seconds(seconds)), expected, "{seconds}");
    }
}

#[test]
fn each_state_says_what_to_do_about_it() {
    let now = Utc::now();
    let say = |e: &DisplayEntry| describe_entry(e, now, &Utc);
    let mut waiting = entry(DisplayState::Waiting);
    waiting.waiting_since = Some(now - Duration::minutes(3));
    let text = say(&waiting);
    assert!(
        text.contains("asked 3 min ago") && text.contains("code on its own panel"),
        "{text}"
    );

    assert!(say(&entry(DisplayState::Approved)).contains("next time it asks"));
    assert!(say(&entry(DisplayState::Rejected)).contains("Forget it"));
    assert!(say(&entry(DisplayState::Revoked)).contains("Forget it"));

    let mut member = entry(DisplayState::Member);
    member.certificate_not_after = Some(now + Duration::days(20) + Duration::hours(3));
    let text = say(&member);
    assert!(
        text.contains("certificate until") && text.contains("20 days 3 h left"),
        "{text}"
    );

    member.certificate_not_after = Some(now + Duration::days(20));
    member.renewal_overdue = true;
    let text = say(&member);
    assert!(
        text.contains("20 days") && text.contains("should have been renewed by now"),
        "{text}"
    );
    member.renewal_overdue = false;

    member.certificate_not_after = Some(now - Duration::days(2));
    member.certificate_expired = true;
    let text = say(&member);
    assert!(
        text.contains("ran out 2 days ago") && text.contains("by itself"),
        "{text}"
    );

    member.changing_keys = true;
    member.replacement_waiting = true;
    member.waiting_since = Some(now - Duration::minutes(10));
    let text = say(&member);
    assert!(text.contains("changing to a new key"), "{text}");
    assert!(
        text.contains("another key asked 10 min ago") && text.contains("reject"),
        "{text}"
    );
}

#[test]
fn the_list_is_a_table_and_an_empty_one_says_how_to_get_a_display_in() {
    let now = Utc::now();
    let mut kitchen = entry(DisplayState::Member);
    kitchen.certificate_not_after = Some(now + Duration::days(30));
    let mut long = entry(DisplayState::Waiting);
    long.name = "reterminal-e1003-a1b2c3".to_owned();
    long.waiting_since = Some(now - Duration::minutes(1));
    let table = describe_list(
        &DisplayList {
            displays: vec![long, kitchen],
        },
        now,
        &Utc,
    );
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(lines.len(), 3, "{table}");
    assert!(
        lines[0].starts_with("NAME") && lines[0].contains("STATE") && lines[0].contains("DETAIL")
    );
    // The columns line up whatever the names' lengths.
    let state_at = lines[0].find("STATE").unwrap();
    assert!(lines[1][state_at..].starts_with("waiting"), "{table}");
    assert!(lines[2][state_at..].starts_with("member"), "{table}");

    let empty = describe_list(&DisplayList { displays: vec![] }, now, &Utc);
    assert!(
        empty.contains("No display has asked") && empty.contains("window open"),
        "{empty}"
    );
}

#[test]
fn what_was_done_is_said_in_a_sentence_that_depends_on_what_it_was() {
    assert!(approved(&entry(DisplayState::Approved)).contains("becomes a member"));
    assert!(
        approved(&entry(DisplayState::Member)).contains("new key"),
        "approving a replacement says so"
    );
    assert!(rejected(&entry(DisplayState::Rejected)).contains("until it is forgotten"));
    assert!(rejected(&entry(DisplayState::Member)).contains("carries on"));
    assert!(revoked(&entry(DisplayState::Revoked)).contains("very next request"));
    assert!(forgotten(&entry(DisplayState::Member)).contains("pairing window"));
}
