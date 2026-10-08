//! The real `displayctl` program against a real server socket.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use chrono::Duration;
use chrono_tz::Tz;
use eink_home_display_rust::adapters::admin;
use eink_home_display_rust::adapters::certificate_authority::{self, Create};
use eink_home_display_rust::adapters::clock::SystemClock;
use eink_home_display_rust::adapters::pairing_store::{FilePairingStore, Missing};
use eink_home_display_rust::application::enrollment::{Enrollment, EnrollmentPolicy};
use eink_home_display_rust::config::application::ApplicationConfig;
use std::os::unix::fs::PermissionsExt;

struct Server {
    directory: tempfile::TempDir,
    socket: PathBuf,
    enrollment: Arc<Enrollment>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start() -> Server {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let authority = Arc::new(
        certificate_authority::open(&directory.path().join("authority"), Create::IfMissing)
            .unwrap(),
    );
    let store = FilePairingStore::open(&directory.path().join("pairings"), Missing::StartEmpty)
        .await
        .unwrap();
    let enrollment = Arc::new(Enrollment::new(
        authority,
        Arc::new(store),
        Arc::new(SystemClock::new(Tz::UTC)),
        EnrollmentPolicy {
            certificate_lifetime: Duration::days(90),
            retry_after: Duration::minutes(5),
            require_channel_binding: false,
        },
    ));
    let socket = admin::default_path(directory.path());
    let listener = admin::bind(&socket).await.unwrap();
    let app = admin::router(enrollment.clone());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Server {
        directory,
        socket,
        enrollment,
        task,
    }
}

async fn displayctl(arguments: &[&str]) -> Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_displayctl"))
        .args(arguments)
        .output()
        .await
        .unwrap()
}

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn err(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[tokio::test]
async fn the_command_shows_opens_and_closes_the_window_on_a_running_server() {
    let server = start().await;
    let socket = path(&server.socket);

    let shown = displayctl(&["--socket", socket, "window", "show"]).await;
    assert!(shown.status.success(), "{}", err(&shown));
    assert!(out(&shown).contains("closed"), "{}", out(&shown));

    let opened = displayctl(&["--socket", socket, "window", "open", "20"]).await;
    assert!(opened.status.success(), "{}", err(&opened));
    assert!(out(&opened).contains("is open until"), "{}", out(&opened));
    // The server it talked to really has the window open.
    assert!(server.enrollment.window_closes_at().is_some());

    let shown = displayctl(&["--socket", socket, "window", "show"]).await;
    assert!(out(&shown).contains("is open until"), "{}", out(&shown));

    let closed = displayctl(&["--socket", socket, "window", "close"]).await;
    assert!(closed.status.success(), "{}", err(&closed));
    assert!(out(&closed).contains("closed"), "{}", out(&closed));
    assert_eq!(server.enrollment.window_closes_at(), None);
}

#[tokio::test]
async fn it_opens_for_fifteen_minutes_when_not_told() {
    let server = start().await;
    let opened = displayctl(&["--socket", path(&server.socket), "window", "open"]).await;
    assert!(opened.status.success(), "{}", err(&opened));
    let closes = server.enrollment.window_closes_at().unwrap();
    let minutes = (closes - chrono::Utc::now()).num_minutes();
    assert!((14..=15).contains(&minutes), "{minutes}");
}

#[tokio::test]
async fn a_refusal_is_a_failure_with_the_servers_words_and_changes_nothing() {
    let server = start().await;
    let refused = displayctl(&["--socket", path(&server.socket), "window", "open", "0"]).await;
    assert!(!refused.status.success());
    assert!(
        err(&refused).contains("minutes must be a whole number from 1 to 240"),
        "{}",
        err(&refused)
    );
    assert_eq!(server.enrollment.window_closes_at(), None);
}

#[tokio::test]
async fn it_finds_the_socket_from_the_servers_own_configuration_file() {
    let server = start().await;
    let mut config = ApplicationConfig::example();
    config.server.tls.directory = server.directory.path().to_owned();
    let file = server.directory.path().join("config.toml");
    std::fs::write(&file, toml::to_string(&config).unwrap()).unwrap();

    let shown = displayctl(&["--config-file", path(&file), "window", "show"]).await;
    assert!(shown.status.success(), "{}", err(&shown));
    assert!(out(&shown).contains("closed"), "{}", out(&shown));

    // And a different `[server.admin] socket` is followed.
    let elsewhere = server.directory.path().join("other.sock");
    config.server.admin.socket = Some(elsewhere.clone());
    std::fs::write(&file, toml::to_string(&config).unwrap()).unwrap();
    let missing = displayctl(&["--config-file", path(&file), "window", "show"]).await;
    assert!(!missing.status.success());
    assert!(err(&missing).contains("other.sock"), "{}", err(&missing));
}

#[tokio::test]
async fn with_no_server_it_says_where_it_looked_and_what_to_check() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("admin.sock");
    let output = displayctl(&["--socket", path(&socket), "window", "show"]).await;
    assert!(!output.status.success());
    let message = err(&output);
    assert!(message.contains(path(&socket)), "{message}");
    assert!(
        message.contains("prefer-https") && message.contains("[server.admin]"),
        "{message}"
    );
}

#[tokio::test]
async fn it_needs_to_be_told_where_the_server_is() {
    let output = displayctl(&["window", "show"]).await;
    assert!(!output.status.success());
    assert!(
        err(&output).contains("--config-file") || err(&output).contains("--socket"),
        "{}",
        err(&output)
    );
}
