//! `GET /plan` and `POST /refresh`: tells a display which render the image is and when to come back.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use super::health::image_written_at;
use super::{server_error, Published};
use crate::application::devices::{RawTelemetry, Telemetry};
use crate::application::plan::compute;
use crate::application::refresh::RefreshOutcome;

/// How long a button press waits for its render: a cold Chrome start can take a while.
const REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);

/// What `/plan` and `/refresh` read from the query besides what the display reports about itself
/// (which `RawTelemetry` reads, from the same query).
#[derive(Deserialize)]
pub(super) struct PlanQuery {
    /// The version of the image the display already shows.
    have: Option<u32>,
}

/// Tells a display which render the image is, whether it is stale, and when to ask again.
pub(super) async fn plan(
    State(published): State<Arc<Published>>,
    Query(query): Query<PlanQuery>,
    Query(telemetry): Query<RawTelemetry>,
) -> Response {
    plan_response(&published, query.have, telemetry.parse()).await
}

/// Renders now if the display's button asked for it (and one hasn't just run), then answers
/// like `/plan` for the image that results. A refused or failed render still answers, with the
/// image there is.
pub(super) async fn refresh_now(
    State(published): State<Arc<Published>>,
    Query(query): Query<PlanQuery>,
    Query(telemetry): Query<RawTelemetry>,
) -> Response {
    match published.handles.refresh.request(REFRESH_TIMEOUT).await {
        RefreshOutcome::Rendered => log::info!("Rendered on request"),
        RefreshOutcome::Throttled => log::info!("Render request ignored: one started recently"),
        RefreshOutcome::TimedOut => log::warn!("Render request timed out after {REFRESH_TIMEOUT:?}"),
    }
    plan_response(&published, query.have, telemetry.parse()).await
}

async fn plan_response(published: &Published, have: Option<u32>, telemetry: Option<Telemetry>) -> Response {
    let rendered_at = match image_written_at(published).await {
        Ok(Some(rendered_at)) => rendered_at,
        Ok(None) => return (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response(),
        Err(e) => return server_error("inspect", e),
    };
    let now = published.clock.now();
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

#[cfg(test)]
mod tests {
    use super::super::testing::{publish, set_age, start};
    use crate::adapters::published_images::DirectoryImages;
    use crate::domain::models::display::ImageFormat;
    use crate::domain::services::published_images::PublishedImages;

    #[tokio::test]
    async fn plan_reports_the_version_and_when_to_come_back() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        assert_eq!(reqwest::get(format!("{base}/plan")).await.unwrap().status(), 404);

        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        set_age(tmp.path(), 100);
        let plan: serde_json::Value = reqwest::get(format!("{base}/plan")).await.unwrap().json().await.unwrap();
        let version = plan["version"].as_u64().unwrap();
        let expected =
            DirectoryImages::new(tmp.path()).published_at(ImageFormat::Bmp).await.unwrap().unwrap().timestamp() as u64;
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
    async fn staleness_follows_the_servers_clock() {
        use crate::adapters::clock::FixedClock;
        use super::super::testing::start_with_clock;

        let tmp = tempfile::tempdir().unwrap();
        let clock = FixedClock::at(chrono::Local::now());
        let (base, _) = start_with_clock(tmp.path().to_path_buf(), ImageFormat::Bmp, clock.clone()).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        let plan = || async { reqwest::get(format!("{base}/plan")).await.unwrap().json::<serde_json::Value>().await.unwrap() };
        assert_eq!(plan().await["stale"], false);

        // Three hours on, with an hourly schedule and no render since, nothing about the file changed.
        clock.set(chrono::Local::now() + chrono::Duration::hours(3));
        let late = plan().await;
        assert_eq!(late["stale"], true);
        assert!(late["age_seconds"].as_u64().unwrap() >= 3 * 3600);
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
