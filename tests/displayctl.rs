//! The real `displayctl` program against a real server socket.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use chrono::Duration;
use chrono_tz::Tz;
use home_display_server::adapters::admin;
use home_display_server::adapters::certificate_authority::{self, Create};
use home_display_server::adapters::clock::SystemClock;
use home_display_server::adapters::pairing_store::{FilePairingStore, Missing};
use home_display_server::application::enrollment::Outcome;
use home_display_server::application::enrollment::{Enrollment, EnrollmentPolicy};
use home_display_server::config::application::ApplicationConfig;
use home_display_server::domain::models::device_id::DeviceId;
use home_display_server::domain::models::pairing::{PairingCode, PublicKey};
use home_display_server::domain::services::certificate_authority::CertificateAuthority;
use rcgen::PublicKeyData;
use std::os::unix::fs::PermissionsExt;

struct Server {
    directory: tempfile::TempDir,
    socket: PathBuf,
    enrollment: Arc<Enrollment>,
    authority: Arc<certificate_authority::PrivateAuthority>,
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
        authority.clone(),
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
        authority,
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

/// `displayctl --socket SOCKET ARGUMENTS`.
async fn ctl(socket: &str, arguments: &[&str]) -> Output {
    let mut all = vec!["--socket", socket];
    all.extend_from_slice(arguments);
    displayctl(&all).await
}

/// What a display holds, and does.
struct Display {
    name: &'static str,
    key: rcgen::KeyPair,
}

impl Display {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            key: rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap(),
        }
    }

    async fn asks(&self, server: &Server) -> Outcome {
        let mut params = rcgen::CertificateParams::default();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, self.name);
        let request = params.serialize_request(&self.key).unwrap();
        server.enrollment.enroll(request.der(), None).await.unwrap()
    }

    /// What its own panel shows.
    fn code(&self, server: &Server) -> String {
        PairingCode::derive(
            &server.authority.fingerprint(),
            &DeviceId::parse(self.name).unwrap(),
            &PublicKey::from_der(self.key.subject_public_key_info()),
        )
        .to_string()
    }
}

#[tokio::test]
async fn a_display_is_paired_from_start_to_finish_with_the_real_command() {
    let server = start().await;
    let socket = path(&server.socket);
    let kitchen = Display::new("kitchen");

    // Nothing is known yet, and the empty list says how to get a display in.
    let empty = ctl(socket, &["displays", "list"]).await;
    assert!(
        out(&empty).contains("No display has asked"),
        "{}",
        out(&empty)
    );

    // The window is opened, and the display asks.
    assert!(ctl(socket, &["window", "open"]).await.status.success());
    assert!(matches!(
        kitchen.asks(&server).await,
        Outcome::Pending { .. }
    ));
    let listed = ctl(socket, &["displays", "list"]).await;
    assert!(listed.status.success(), "{}", err(&listed));
    let table = out(&listed);
    assert!(
        table.contains("kitchen") && table.contains("waiting") && table.contains("own panel"),
        "{table}"
    );
    let code = kitchen.code(&server);
    assert!(
        !table.contains(&code) && !table.contains(&code.replace('-', "")),
        "the list shows the code: {table}"
    );

    // A wrong code does nothing, and says so.
    let wrong = ctl(socket, &["approve", "kitchen", "0000-0000-0000"]).await;
    assert!(!wrong.status.success());
    assert!(err(&wrong).contains("not the code"), "{}", err(&wrong));
    assert!(out(&ctl(socket, &["displays", "show", "kitchen"]).await).contains("asked"));

    // The code as a person types it from the panel: lower case, no dashes.
    let typed = code.to_lowercase().replace('-', "");
    let approved = ctl(socket, &["approve", "kitchen", &typed]).await;
    assert!(approved.status.success(), "{}", err(&approved));
    assert!(
        out(&approved).contains("Approved kitchen"),
        "{}",
        out(&approved)
    );

    // The display's next ask is granted a certificate, and it is a member.
    assert!(matches!(kitchen.asks(&server).await, Outcome::Issued(_)));
    let shown = out(&ctl(socket, &["displays", "show", "kitchen"]).await);
    assert!(
        shown.contains("kitchen") && shown.contains("certificate until"),
        "{shown}"
    );

    // Revoked, and then forgotten.
    let revoked = ctl(socket, &["revoke", "kitchen"]).await;
    assert!(revoked.status.success(), "{}", err(&revoked));
    assert!(out(&ctl(socket, &["displays", "list"]).await).contains("revoked"));
    let forgotten = ctl(socket, &["forget", "kitchen"]).await;
    assert!(forgotten.status.success(), "{}", err(&forgotten));
    assert!(out(&ctl(socket, &["displays", "list"]).await).contains("No display has asked"));
}

#[tokio::test]
async fn naming_a_display_that_is_not_known_or_cannot_be_one_fails_with_a_reason() {
    let server = start().await;
    let socket = path(&server.socket);
    let unknown = displayctl(&["--socket", socket, "approve", "ghost", "JGWP-14YW-3BT0"]).await;
    assert!(!unknown.status.success());
    assert!(err(&unknown).contains("ghost"), "{}", err(&unknown));
    let silly = displayctl(&["--socket", socket, "revoke", "has space"]).await;
    assert!(!silly.status.success());
    assert!(
        err(&silly).contains("not a display name"),
        "{}",
        err(&silly)
    );
    let notcode = displayctl(&["--socket", socket, "approve", "ghost", "abc"]).await;
    assert!(!notcode.status.success());
}
