//! Hands the latest rendered image to displays that fetch it over HTTP.
//! The display adapter publishes into a directory; this serves what is there.

mod advertise;
mod devices;
mod metrics;
mod plan;
mod refresh;
mod status;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Local};
use serde::Deserialize;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

use self::advertise::Advertisement;
pub use self::plan::{render_due, PlanTiming};
pub use self::devices::DeviceBoard;
pub use self::refresh::RefreshControl;
pub use self::status::StatusBoard;
use self::devices::{RawTelemetry, Telemetry};
use self::refresh::RefreshOutcome;
use self::plan::compute;
use crate::config::server::ServerConfig;
use crate::domain::models::display::ImageFormat;
use crate::scheduler::Schedule;

fn published_path(directory: &Path, format: ImageFormat) -> PathBuf {
    directory.join(format!("image.{}", format.extension()))
}

/// Makes `bytes` the image that is served. Written beside the target and renamed into
/// place, so a download in progress never sees half a file.
pub async fn publish(directory: &Path, format: ImageFormat, bytes: &[u8]) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(directory)
        .await
        .with_context(|| format!("Failed to create {}", directory.display()))?;
    let target = published_path(directory, format);
    let staging = target.with_extension(format!("{}.tmp", format.extension()));
    tokio::fs::write(&staging, bytes)
        .await
        .with_context(|| format!("Failed to write {}", staging.display()))?;
    tokio::fs::rename(&staging, &target)
        .await
        .with_context(|| format!("Failed to move the image into {}", target.display()))?;
    Ok(())
}

struct Published {
    directory: PathBuf,
    format: ImageFormat,
    schedule: Schedule,
    timing: PlanTiming,
    handles: Handles,
}

/// What the render loop shares with the server.
#[derive(Clone)]
pub struct Handles {
    /// Lets a display's button trigger a render.
    pub refresh: Arc<RefreshControl>,
    /// The render history, for `/status` and `/healthz`.
    pub status: Arc<StatusBoard>,
    /// What the displays report about themselves, for `/status` and `/metrics`.
    pub devices: Arc<DeviceBoard>,
}

/// How long a button press waits for its render: a cold Chrome start can take a while.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(40);

/// When the image being served was rendered, or None if there isn't one yet.
pub fn rendered_at(directory: &Path, format: ImageFormat) -> Option<DateTime<Local>> {
    let modified = std::fs::metadata(published_path(directory, format)).ok()?.modified().ok()?;
    Some(DateTime::<Local>::from(modified))
}

pub fn router(
    directory: PathBuf,
    format: ImageFormat,
    schedule: Schedule,
    timing: PlanTiming,
    handles: Handles,
) -> Router {
    Router::new()
        .route("/image", get(image))
        .route("/plan", get(plan))
        .route("/refresh", post(refresh_now))
        .route("/status", get(status))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .layer(TraceLayer::new_for_http())
        .with_state(Arc::new(Published { directory, format, schedule, timing, handles }))
}

#[derive(Deserialize)]
struct PlanQuery {
    /// The version of the image the display already shows.
    have: Option<u32>,
    // What the display reports about itself. Text, so a malformed value can't fail the request;
    // see `RawTelemetry`.
    device: Option<String>,
    battery_mv: Option<String>,
    battery_pct: Option<String>,
    battery_state: Option<String>,
    failed_wakes: Option<String>,
}

impl PlanQuery {
    fn telemetry(&self) -> Option<Telemetry> {
        RawTelemetry {
            device: self.device.clone(),
            battery_mv: self.battery_mv.clone(),
            battery_pct: self.battery_pct.clone(),
            battery_state: self.battery_state.clone(),
            failed_wakes: self.failed_wakes.clone(),
        }
        .parse()
    }
}

/// Tells a display which render the image is, whether it is stale, and when to ask again.
async fn plan(State(published): State<Arc<Published>>, Query(query): Query<PlanQuery>) -> Response {
    plan_response(&published, query.have, query.telemetry()).await
}

/// Renders now if the display's button asked for it (and one hasn't just run), then answers
/// like `/plan` for the image that results. A refused or failed render still answers, with the
/// image there is.
async fn refresh_now(State(published): State<Arc<Published>>, Query(query): Query<PlanQuery>) -> Response {
    match published.handles.refresh.request(REFRESH_TIMEOUT).await {
        RefreshOutcome::Rendered => log::info!("Rendered on request"),
        RefreshOutcome::Throttled => log::info!("Render request ignored: one started recently"),
        RefreshOutcome::TimedOut => log::warn!("Render request timed out after {REFRESH_TIMEOUT:?}"),
    }
    plan_response(&published, query.have, query.telemetry()).await
}

/// When the served image was written; None before the first render.
async fn image_written_at(published: &Published) -> std::io::Result<Option<DateTime<Local>>> {
    let path = published_path(&published.directory, published.format);
    match tokio::fs::metadata(&path).await.and_then(|metadata| metadata.modified()) {
        Ok(modified) => Ok(Some(DateTime::<Local>::from(modified))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// How the service is doing: the render history, the sources' state, and the image's age.
async fn status(State(published): State<Arc<Published>>) -> Response {
    match current_status(&published).await {
        Ok(status) => ([(header::CACHE_CONTROL, "no-store")], Json(status)).into_response(),
        Err(e) => server_error("report", e),
    }
}

/// A pass or fail for monitors: 200 unless the image is stale, then 503, with a line saying why.
async fn healthz(State(published): State<Arc<Published>>) -> Response {
    match current_status(&published).await {
        Ok(status) => {
            let code = if status.is_healthy() { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
            (code, [(header::CACHE_CONTROL, "no-store")], status.summary()).into_response()
        }
        Err(e) => server_error("check", e),
    }
}

async fn current_status(published: &Published) -> anyhow::Result<status::Status> {
    let rendered_at = image_written_at(published).await?;
    let now = Local::now();
    let mut status = published.handles.status.status(now, rendered_at, &published.schedule, published.timing)?;
    status.devices = published.handles.devices.snapshot(now);
    Ok(status)
}

/// The same facts as `/status`, in the Prometheus text format, for a scraper.
async fn metrics(State(published): State<Arc<Published>>) -> Response {
    match current_status(&published).await {
        Ok(status) => (
            [(header::CONTENT_TYPE, metrics::CONTENT_TYPE), (header::CACHE_CONTROL, "no-store")],
            metrics::render(&status),
        )
            .into_response(),
        Err(e) => server_error("report", e),
    }
}

async fn plan_response(published: &Published, have: Option<u32>, telemetry: Option<Telemetry>) -> Response {
    let rendered_at = match image_written_at(published).await {
        Ok(Some(rendered_at)) => rendered_at,
        Ok(None) => return (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response(),
        Err(e) => return server_error("inspect", e),
    };
    let now = Local::now();
    match compute(now, rendered_at, &published.schedule, published.timing, have) {
        Ok(plan) => {
            if let Some(telemetry) = telemetry {
                published.handles.devices.record(now, telemetry, plan.next_seconds);
            }
            ([(header::CACHE_CONTROL, "no-store")], Json(plan)).into_response()
        }
        Err(e) => server_error("plan", e),
    }
}

fn server_error(action: &str, e: impl std::fmt::Display) -> Response {
    log::error!("Failed to {action} the published image: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

async fn image(State(published): State<Arc<Published>>) -> impl IntoResponse {
    match tokio::fs::read(published_path(&published.directory, published.format)).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, published.format.content_type()),
                // The picture changes every refresh, so nothing may reuse an old one.
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response()
        }
        Err(e) => {
            log::error!("Failed to read the published image: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Serves until the future is dropped, or fails at once if the address can't be bound.
pub async fn serve(
    config: &ServerConfig,
    format: ImageFormat,
    schedule: Schedule,
    handles: Handles,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("Failed to listen on {}", config.bind))?;
    log::info!("Serving the display image at http://{}/image", config.bind);
    // Discovery is a convenience, so the server runs without it. Held until serving ends,
    // which is when the goodbye goes out.
    let _advertisement = config
        .advertise
        .then(|| Advertisement::start(config, listener.local_addr()?.port(), format))
        .transpose()
        .unwrap_or_else(|e| {
            log::warn!("Not advertising over mDNS: {e:#}");
            None
        });
    axum::serve(listener, router(config.directory.clone(), format, schedule, PlanTiming::from(config), handles))
        .await
        .context("Image server stopped")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn start(directory: PathBuf, format: ImageFormat) -> String {
        start_with_status(directory, format).await.0
    }

    /// Also hands back the status board, to play the render loop's part.
    async fn start_with_status(directory: PathBuf, format: ImageFormat) -> (String, Arc<StatusBoard>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let timing = PlanTiming::from(&ServerConfig::default());
        let schedule = Schedule::parse_every("1h").unwrap();
        let status = StatusBoard::new(Local::now());
        let handles = Handles { refresh: RefreshControl::new(Duration::from_secs(30)), status: Arc::clone(&status), devices: DeviceBoard::new(Duration::from_secs(900)) };
        tokio::spawn(async move { axum::serve(listener, router(directory, format, schedule, timing, handles)).await });
        (format!("http://{address}"), status)
    }

    #[tokio::test]
    async fn serves_the_published_image_with_its_content_type() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;

        assert_eq!(reqwest::get(format!("{base}/image")).await.unwrap().status(), 404);

        publish(tmp.path(), ImageFormat::Bmp, &[1, 2, 3]).await.unwrap();
        let response = reqwest::get(format!("{base}/image")).await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "image/bmp");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.bytes().await.unwrap().as_ref(), [1, 2, 3]);

        // A new publish replaces it and leaves no staging file behind.
        publish(tmp.path(), ImageFormat::Bmp, &[9]).await.unwrap();
        let response = reqwest::get(format!("{base}/image")).await.unwrap();
        assert_eq!(response.bytes().await.unwrap().as_ref(), [9]);
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn healthz_answers_without_an_image() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Png).await;
        assert_eq!(reqwest::get(format!("{base}/healthz")).await.unwrap().status(), 200);
    }

    async fn healthz(base: &str) -> (u16, String) {
        let response = reqwest::get(format!("{base}/healthz")).await.unwrap();
        assert_eq!(response.headers()["cache-control"], "no-store");
        (response.status().as_u16(), response.text().await.unwrap())
    }

    #[tokio::test]
    async fn healthz_fails_when_the_image_is_stale_and_recovers_with_a_render() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, status) = start_with_status(tmp.path().to_path_buf(), ImageFormat::Bmp).await;

        // Just started, nothing rendered yet: not a failure.
        assert_eq!(healthz(&base).await, (200, "starting".to_owned()));

        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        assert_eq!(healthz(&base).await, (200, "ok".to_owned()));

        // An hourly schedule, and the image is three hours old: two renders are missing.
        set_age(tmp.path(), 3 * 3600);
        let (code, text) = healthz(&base).await;
        assert_eq!((code, text.as_str()), (503, "stale: the image is 3 h 0 min old"));

        publish(tmp.path(), ImageFormat::Bmp, b"b").await.unwrap();
        status.render_succeeded(Local::now(), Default::default());
        assert_eq!(healthz(&base).await, (200, "ok".to_owned()));
    }

    #[tokio::test]
    async fn a_source_that_is_down_is_degraded_but_the_check_still_passes() {
        use crate::domain::models::render_report::{RenderReport, SourceReport, SourceState};

        let tmp = tempfile::tempdir().unwrap();
        let (base, status) = start_with_status(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        status.render_succeeded(
            Local::now(),
            RenderReport {
                sources: vec![SourceReport {
                    name: "weather".into(),
                    state: SourceState::Unavailable { reason: "key rejected".into() },
                }],
            },
        );

        assert_eq!(healthz(&base).await, (200, "degraded".to_owned()));
        let body: serde_json::Value = reqwest::get(format!("{base}/status")).await.unwrap().json().await.unwrap();
        assert_eq!(body["state"], "degraded");
        assert_eq!(body["sources"][0]["name"], "weather");
        assert_eq!(body["sources"][0]["state"], "unavailable");
        assert_eq!(body["sources"][0]["reason"], "key rejected");
    }

    #[tokio::test]
    async fn status_reports_the_history_the_image_and_the_next_render() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, status) = start_with_status(tmp.path().to_path_buf(), ImageFormat::Bmp).await;

        let before: serde_json::Value = reqwest::get(format!("{base}/status")).await.unwrap().json().await.unwrap();
        assert_eq!(before["state"], "starting");
        assert!(before["image"].is_null());
        assert!(before["last_success"].is_null());
        assert_eq!(before["consecutive_failures"], 0);

        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        status.render_started();
        status.render_failed(Local::now(), &anyhow::anyhow!("Chrome did not start"));
        let after = reqwest::get(format!("{base}/status")).await.unwrap();
        assert_eq!(after.headers()["cache-control"], "no-store");
        let after: serde_json::Value = after.json().await.unwrap();

        assert_eq!(after["state"], "failing");
        assert_eq!(after["rendering"], false);
        assert_eq!(after["consecutive_failures"], 1);
        assert_eq!(after["last_failure"]["error"], "Chrome did not start");
        assert!(after["image"]["age_seconds"].as_u64().unwrap() < 5);
        assert!(after["image"]["version"].as_u64().unwrap() > 0);
        assert!(after["next_render"].as_str().unwrap().contains('T'));
        assert_eq!(after["version"], env!("CARGO_PKG_VERSION"));
    }

    fn set_age(directory: &Path, seconds: u64) {
        let file = std::fs::File::options().write(true).open(directory.join("image.bmp")).unwrap();
        file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(seconds)).unwrap();
    }

    #[tokio::test]
    async fn plan_reports_the_version_and_when_to_come_back() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        assert_eq!(reqwest::get(format!("{base}/plan")).await.unwrap().status(), 404);

        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        set_age(tmp.path(), 100);
        let plan: serde_json::Value = reqwest::get(format!("{base}/plan")).await.unwrap().json().await.unwrap();
        let version = plan["version"].as_u64().unwrap();
        let expected = rendered_at(tmp.path(), ImageFormat::Bmp).unwrap().timestamp() as u64;
        assert_eq!(version, expected);
        assert_eq!(plan["changed"], true);
        assert_eq!(plan["stale"], false);
        // An hour after the render, less the 100 seconds already gone, plus the 30 second delay.
        let next = plan["next_seconds"].as_u64().unwrap();
        assert!((3525..=3535).contains(&next), "{next}");

        let same: serde_json::Value =
            reqwest::get(format!("{base}/plan?have={version}")).await.unwrap().json().await.unwrap();
        assert_eq!(same["changed"], false);
        let other: serde_json::Value = reqwest::get(format!("{base}/plan?have=1")).await.unwrap().json().await.unwrap();
        assert_eq!(other["changed"], true);

        // A newer render is a larger version.
        publish(tmp.path(), ImageFormat::Bmp, b"b").await.unwrap();
        let newer: serde_json::Value = reqwest::get(format!("{base}/plan")).await.unwrap().json().await.unwrap();
        assert!(newer["version"].as_u64().unwrap() >= version + 99);
    }

    #[tokio::test]
    async fn an_old_image_is_reported_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        set_age(tmp.path(), 3 * 3600);

        let plan: serde_json::Value = reqwest::get(format!("{base}/plan")).await.unwrap().json().await.unwrap();
        assert_eq!(plan["stale"], true);
        assert!(plan["age_seconds"].as_u64().unwrap() >= 3 * 3600);
    }

    #[tokio::test]
    async fn a_check_in_shows_up_in_status_and_metrics() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();

        let status = || async {
            reqwest::get(format!("{base}/status")).await.unwrap().json::<serde_json::Value>().await.unwrap()
        };
        assert_eq!(status().await["devices"], serde_json::json!([]));

        let url = format!(
            "{base}/plan?have=1&device=kitchen&battery_mv=3350&battery_pct=8&battery_state=low&failed_wakes=0"
        );
        assert_eq!(reqwest::get(url).await.unwrap().status(), 200);

        let devices = status().await["devices"].clone();
        assert_eq!(devices[0]["name"], "kitchen");
        assert_eq!(devices[0]["battery_millivolts"], 3350);
        assert_eq!(devices[0]["battery_state"], "low");
        assert_eq!(devices[0]["overdue"], false);

        let response = reqwest::get(format!("{base}/metrics")).await.unwrap();
        assert!(response.headers()["content-type"].to_str().unwrap().starts_with("text/plain; version=0.0.4"));
        let text = response.text().await.unwrap();
        assert!(text.contains("eink_device_battery_volts{device=\"kitchen\"} 3.35"), "{text}");
        assert!(text.contains("eink_device_battery_state{device=\"kitchen\",state=\"low\"} 1"), "{text}");
    }

    #[tokio::test]
    async fn bad_telemetry_never_stops_the_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();

        let url = format!("{base}/plan?device=%22bad%0A&battery_mv=lots&battery_pct=-1&failed_wakes=x");
        assert_eq!(reqwest::get(url).await.unwrap().status(), 200);
        let devices = reqwest::get(format!("{base}/status")).await.unwrap().json::<serde_json::Value>().await.unwrap();
        assert_eq!(devices["devices"], serde_json::json!([]));
    }
}
