//! Helpers for the route tests: a server on a free port over a temporary directory.

use chrono_tz::Europe::London;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;

use super::{Handles, router};
use crate::adapters::certificate_authority::{self, Create};
use crate::adapters::clock::SystemClock;
use crate::adapters::layout_choices::FileLayoutChoices;
use crate::adapters::pairing_store::{FilePairingStore, Missing};
use crate::adapters::published_images::DirectoryImages;
use crate::application::devices::DeviceBoard;
use crate::application::enrollment::{Enrollment, EnrollmentPolicy};
use crate::application::plan::PlanTiming;
use crate::application::refresh::RefreshControl;
use crate::application::status::StatusBoard;
use crate::domain::models::device_id::DeviceId;
use crate::domain::models::display::ImageFormat;
use crate::domain::models::layout::{Layout, LayoutName, Layouts};
use crate::domain::models::pairing::{Pairing, PairingState, PublicKey};
use crate::domain::models::schedule::Schedule;
use crate::domain::services::clock::Clock;
use crate::domain::services::layout_choices::LayoutChoices;
use crate::domain::services::pairing_store::PairingStore;
use crate::domain::services::published_images::PublishedImages;

/// What the first of the test servers' layouts is called.
pub(super) const FIRST: &str = "dashboard";

/// The test servers' layouts when a test has no need of more than one.
pub(super) fn one_layout() -> Layouts {
    Layouts::new(vec![Layout::new(FIRST, "Everything")]).unwrap()
}

pub(super) async fn publish(
    directory: &Path,
    format: ImageFormat,
    bytes: &[u8],
) -> anyhow::Result<()> {
    publish_layout(directory, FIRST, format, bytes).await
}

/// Publishes the image of the layout called `layout`.
pub(super) async fn publish_layout(
    directory: &Path,
    layout: &str,
    format: ImageFormat,
    bytes: &[u8],
) -> anyhow::Result<()> {
    DirectoryImages::new(directory)
        .publish(&LayoutName::parse(layout).unwrap(), format, bytes)
        .await
}

/// A server that has two layouts, the default and `departures`.
pub(super) async fn start_with_two_layouts(directory: PathBuf, format: ImageFormat) -> String {
    let layouts = Layouts::new(vec![
        Layout::new(FIRST, "Everything"),
        Layout::new("departures", "Just the trains"),
    ])
    .unwrap();
    start_layouts(
        Arc::new(DirectoryImages::new(directory)),
        format,
        Arc::new(SystemClock::new(London)),
        None,
        None,
        layouts,
    )
    .await
    .0
}

pub(super) async fn start(directory: PathBuf, format: ImageFormat) -> String {
    start_with_status(directory, format).await.0
}

/// Also hands back the status board, to play the render loop's part.
pub(super) async fn start_with_status(
    directory: PathBuf,
    format: ImageFormat,
) -> (String, Arc<StatusBoard>) {
    start_with_clock(directory, format, Arc::new(SystemClock::new(London))).await
}

/// Like `start_with_status`, with the server reading the time from `clock`.
pub(super) async fn start_with_clock(
    directory: PathBuf,
    format: ImageFormat,
    clock: Arc<dyn Clock>,
) -> (String, Arc<StatusBoard>) {
    start_with(Arc::new(DirectoryImages::new(directory)), format, clock).await
}

/// Like `start_with_clock`, over any store of images (to make one misbehave).
pub(super) async fn start_with(
    images: Arc<dyn PublishedImages>,
    format: ImageFormat,
    clock: Arc<dyn Clock>,
) -> (String, Arc<StatusBoard>) {
    start_full(images, format, clock, None).await
}

/// Like `start_with`, reporting on the displays in the certificate authority too.
pub(super) async fn start_full(
    images: Arc<dyn PublishedImages>,
    format: ImageFormat,
    clock: Arc<dyn Clock>,
    members: Option<Arc<crate::application::enrollment::Enrollment>>,
) -> (String, Arc<StatusBoard>) {
    start_layouts(images, format, clock, members, None, one_layout()).await
}

/// Like `start_full`, with the layouts there are and the choices of layout made for the displays.
pub(super) async fn start_layouts(
    images: Arc<dyn PublishedImages>,
    format: ImageFormat,
    clock: Arc<dyn Clock>,
    members: Option<Arc<crate::application::enrollment::Enrollment>>,
    layout_choices: Option<Arc<dyn LayoutChoices>>,
    layouts: Layouts,
) -> (String, Arc<StatusBoard>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let timing = PlanTiming {
        wake_delay: Duration::from_secs(30),
        stale_grace: Duration::from_secs(300),
    };
    let schedule = Schedule::parse_every("1h").unwrap();
    let status = StatusBoard::new(chrono::Utc::now().with_timezone(&London));
    let handles = Handles {
        layouts,
        layout_choices,
        viewers: None,
        refresh: RefreshControl::new(Duration::from_secs(30)),
        status: Arc::clone(&status),
        devices: DeviceBoard::new(Duration::from_secs(900)),
        members,
        handshakes: None,
    };
    tokio::spawn(async move {
        axum::serve(
            listener,
            router(images, format, schedule, timing, handles, clock),
        )
        .await
    });
    (format!("http://{address}"), status)
}

/// The routes for a web browser alone, on a free port, over a directory of images. Hands back the handles too, to see what
/// the page did and did not record.
pub(super) async fn start_web(
    directory: PathBuf,
    layouts: Layouts,
    settings: &super::WebSettings,
    clock: Arc<dyn Clock>,
) -> Result<(String, Handles), String> {
    start_web_for(directory, layouts, settings, clock, None, None).await
}

/// As `start_web`, with viewers to check tokens against and, if given, the address every request is taken to come from (as
/// the TLS server tells the routes), which is what failures are counted against.
pub(super) async fn start_web_for(
    directory: PathBuf,
    layouts: Layouts,
    settings: &super::WebSettings,
    clock: Arc<dyn Clock>,
    viewers: Option<Arc<crate::application::viewers::Viewers>>,
    peer: Option<std::net::SocketAddr>,
) -> Result<(String, Handles), String> {
    let handles = Handles {
        layouts,
        layout_choices: None,
        viewers,
        refresh: RefreshControl::new(Duration::from_secs(30)),
        status: StatusBoard::new(chrono::Utc::now().with_timezone(&London)),
        devices: DeviceBoard::new(Duration::from_secs(900)),
        members: None,
        handshakes: None,
    };
    let timing = PlanTiming {
        wake_delay: Duration::from_secs(30),
        stale_grace: Duration::from_secs(300),
    };
    let mut app = super::web_router(
        settings,
        Arc::new(DirectoryImages::new(directory)),
        ImageFormat::Bmp,
        Schedule::parse_every("1h").unwrap(),
        timing,
        handles.clone(),
        clock,
    )?;
    if let Some(peer) = peer {
        app = app.layer(axum::Extension(crate::adapters::source::PeerAddress(peer)));
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    Ok((format!("http://{address}"), handles))
}

pub(super) fn set_age(directory: &Path, seconds: u64) {
    let file = std::fs::File::options()
        .write(true)
        .open(directory.join(FIRST).join("image.bmp"))
        .unwrap();
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(seconds))
        .unwrap();
}

/// A directory of images where one format fails to be inspected, and one fails to be read.
pub(super) struct Faulty {
    pub(super) inner: DirectoryImages,
    pub(super) cannot_inspect: Option<ImageFormat>,
    pub(super) cannot_read: Option<ImageFormat>,
}

#[async_trait::async_trait]
impl PublishedImages for Faulty {
    async fn publish(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        self.inner.publish(layout, format, bytes).await
    }

    async fn published_at(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
    ) -> anyhow::Result<Option<chrono::DateTime<chrono::Utc>>> {
        anyhow::ensure!(self.cannot_inspect != Some(format), "permission denied");
        self.inner.published_at(layout, format).await
    }

    async fn read(
        &self,
        layout: &LayoutName,
        format: ImageFormat,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        anyhow::ensure!(self.cannot_read != Some(format), "input/output error");
        self.inner.read(layout, format).await
    }
}

/// Displays in a certificate authority and a list, with a place to keep which layout is chosen for each, in a
/// directory that lasts as long as this does.
pub(super) struct Members {
    pub(super) enrollment: Arc<Enrollment>,
    pub(super) choices: Arc<FileLayoutChoices>,
    _directory: tempfile::TempDir,
}

/// These displays, each in the state given, known to a new authority.
pub(super) async fn members(displays: &[(&str, PairingState)]) -> Members {
    let directory = tempfile::tempdir().unwrap();
    let authority =
        Arc::new(certificate_authority::open(directory.path(), Create::IfMissing).unwrap());
    let store = Arc::new(
        FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap(),
    );
    let now = chrono::Utc::now();
    for (name, state) in displays {
        store
            .put(&Pairing::new(
                DeviceId::parse(name).unwrap(),
                PublicKey::from_der(name.as_bytes().repeat(8)),
                state.clone(),
                now,
            ))
            .await
            .unwrap();
    }
    let enrollment = Arc::new(Enrollment::new(
        authority,
        store,
        Arc::new(SystemClock::new(London)),
        EnrollmentPolicy {
            certificate_lifetime: chrono::Duration::days(90),
            retry_after: chrono::Duration::minutes(5),
            require_channel_binding: false,
        },
    ));
    let choices = Arc::new(FileLayoutChoices::open(directory.path()).await.unwrap());
    Members {
        enrollment,
        choices,
        _directory: directory,
    }
}

/// A state for a display that has joined and has a certificate for another twenty days.
pub(super) fn enrolled() -> PairingState {
    PairingState::Enrolled {
        serial: "01".to_owned(),
        not_after: chrono::Utc::now() + chrono::Duration::days(20),
    }
}
