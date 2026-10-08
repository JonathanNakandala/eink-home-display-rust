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
use crate::adapters::clock::SystemClock;
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

struct Fixture {
    _directory: TempDir,
    socket: PathBuf,
    enrollment: Arc<Enrollment>,
    client: AdminClient,
    server: JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn enrollment_in(directory: &Path) -> Arc<Enrollment> {
    let authority = Arc::new(
        certificate_authority::open(&directory.join("authority"), Create::IfMissing).unwrap(),
    );
    let store = FilePairingStore::open(&directory.join("pairings"), Missing::StartEmpty)
        .await
        .unwrap();
    Arc::new(Enrollment::new(
        authority,
        Arc::new(store),
        Arc::new(SystemClock::new(Tz::UTC)),
        EnrollmentPolicy {
            certificate_lifetime: Duration::days(90),
            retry_after: Duration::minutes(5),
            require_channel_binding: false,
        },
    ))
}

async fn start() -> Fixture {
    let directory = private_directory();
    let enrollment = enrollment_in(directory.path()).await;
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
        server,
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
    let routes: Vec<(String, Vec<String>)> = api["paths"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(path, item)| {
            (
                path.clone(),
                item.as_object().unwrap().keys().cloned().collect(),
            )
        })
        .collect();
    assert_eq!(routes.len(), 1, "{routes:?}");
    assert_eq!(routes[0].0, "/v1/window");
    assert_eq!(routes[0].1, ["delete", "get", "put"]);
    // The statuses an endpoint can answer with: 400 only where there is input to get wrong.
    let statuses = |method: &str| -> Vec<String> {
        api["paths"]["/v1/window"][method]["responses"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    };
    assert_eq!(statuses("get"), ["200", "500"]);
    assert_eq!(statuses("delete"), ["200", "500"]);
    assert_eq!(statuses("put"), ["200", "400", "500"]);
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
        let enrollment = enrollment_in(directory.path()).await;
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
