//! `GET /plan` and `POST /refresh`: tells a display which render the image is and when to come back.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use utoipa::{IntoParams, IntoResponses};

use super::health::image_written_at;
use super::identity::{Caller, Named};
use super::{Published, server_error};
use crate::application::devices::{RawTelemetry, Telemetry};
use crate::application::plan::{Plan, compute};
use crate::application::refresh::RefreshOutcome;

/// How long a button press waits for its render: a cold Chrome start can take a while.
const REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);

/// What `/plan` and `/refresh` read from the query besides what the display reports about itself
/// (which `RawTelemetry` reads, from the same query).
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct PlanQuery {
    /// The version of the image the display already shows (the `version` of an earlier plan), so the answer can
    /// say the image is unchanged and the display can skip the download and the refresh.
    #[param(example = 1791463200)]
    have: Option<u32>,
}

/// What `/plan` and `/refresh` answer: the plan, or why there is none.
#[derive(IntoResponses)]
#[allow(dead_code)]
pub(super) enum PlanResponses {
    /// The plan for the image there is.
    #[response(
        status = 200,
        headers(("cache-control" = String, description = "Always `no-store`.")),
        example = json!({
            "version": 1791463200,
            "changed": true,
            "stale": false,
            "pending": false,
            "next_seconds": 600,
            "age_seconds": 100,
            "timezone": "Europe/London",
            "utc_offset_seconds": 3600
        })
    )]
    Plan(#[to_schema] Plan),
    /// Nothing has been rendered yet, so there is no image to plan around.
    #[response(status = 404, content_type = "text/plain", example = json!("No image has been rendered yet"))]
    NoImage(String),
    /// The server could not work the plan out.
    #[response(status = 500)]
    Failed,
}

/// Tells a display which render the image is, whether it is stale, and when to ask again.
#[utoipa::path(
    get,
    path = "/plan",
    tag = "displays",
    params(Named, PlanQuery, RawTelemetry),
    responses(PlanResponses)
)]
pub(super) async fn plan(
    State(published): State<Arc<Published>>,
    Caller(caller): Caller,
    Query(query): Query<PlanQuery>,
    Query(telemetry): Query<RawTelemetry>,
) -> Response {
    plan_response(
        &published,
        query.have,
        caller.map(|device| telemetry.parse(device)),
    )
    .await
}

/// Renders now if the display's button asked for it (and one hasn't just run), then answers
/// like `/plan` for the image that results. A refused or failed render still answers, with the
/// image there is.
#[utoipa::path(
    post,
    path = "/refresh",
    tag = "displays",
    params(Named, PlanQuery, RawTelemetry),
    responses(PlanResponses)
)]
pub(super) async fn refresh_now(
    State(published): State<Arc<Published>>,
    Caller(caller): Caller,
    Query(query): Query<PlanQuery>,
    Query(telemetry): Query<RawTelemetry>,
) -> Response {
    match published.handles.refresh.request(REFRESH_TIMEOUT).await {
        RefreshOutcome::Rendered => log::info!("Rendered on request"),
        RefreshOutcome::Throttled => log::info!("Render request ignored: one started recently"),
        RefreshOutcome::TimedOut => {
            log::warn!("Render request timed out after {REFRESH_TIMEOUT:?}")
        }
    }
    plan_response(
        &published,
        query.have,
        caller.map(|device| telemetry.parse(device)),
    )
    .await
}

async fn plan_response(
    published: &Published,
    have: Option<u32>,
    telemetry: Option<Telemetry>,
) -> Response {
    let rendered_at = match image_written_at(published).await {
        Ok(Some(rendered_at)) => rendered_at,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response();
        }
        Err(e) => return server_error("inspect", e),
    };
    let now = published.clock.now();
    match compute(
        now,
        rendered_at,
        &published.schedule,
        published.timing,
        have,
    ) {
        Ok(plan) => {
            if let Some(telemetry) = telemetry {
                published
                    .handles
                    .devices
                    .record(now, telemetry, plan.next_seconds);
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
    use chrono_tz::Europe::London;

    #[tokio::test]
    async fn plan_reports_the_version_and_when_to_come_back() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        assert_eq!(
            reqwest::get(format!("{base}/plan")).await.unwrap().status(),
            404
        );

        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        set_age(tmp.path(), 100);
        let plan: serde_json::Value = reqwest::get(format!("{base}/plan"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let version = plan["version"].as_u64().unwrap();
        let expected = DirectoryImages::new(tmp.path())
            .published_at(ImageFormat::Bmp)
            .await
            .unwrap()
            .unwrap()
            .timestamp() as u64;
        assert_eq!(version, expected);
        assert_eq!(plan["changed"], true);
        assert_eq!(plan["stale"], false);
        // The server's zone and its offset now, for a display that has no timezone database.
        let zone: chrono_tz::Tz = plan["timezone"].as_str().unwrap().parse().unwrap();
        let offset =
            chrono::Offset::fix(chrono::Utc::now().with_timezone(&zone).offset()).local_minus_utc();
        assert_eq!(plan["utc_offset_seconds"], offset);
        // An hour after the render, less the 100 seconds already gone, plus the 30 second delay.
        let next = plan["next_seconds"].as_u64().unwrap();
        assert!((3525..=3535).contains(&next), "{next}");

        let same: serde_json::Value = reqwest::get(format!("{base}/plan?have={version}"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(same["changed"], false);
        let other: serde_json::Value = reqwest::get(format!("{base}/plan?have=1"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(other["changed"], true);

        // A newer render is a larger version.
        publish(tmp.path(), ImageFormat::Bmp, b"b").await.unwrap();
        let newer: serde_json::Value = reqwest::get(format!("{base}/plan"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(newer["version"].as_u64().unwrap() >= version + 99);
    }

    #[tokio::test]
    async fn an_old_image_is_reported_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        set_age(tmp.path(), 3 * 3600);

        let plan: serde_json::Value = reqwest::get(format!("{base}/plan"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(plan["stale"], true);
        assert!(plan["age_seconds"].as_u64().unwrap() >= 3 * 3600);
    }

    #[tokio::test]
    async fn staleness_follows_the_servers_clock() {
        use super::super::testing::start_with_clock;
        use crate::adapters::clock::FixedClock;

        let tmp = tempfile::tempdir().unwrap();
        let clock = FixedClock::at(chrono::Utc::now().with_timezone(&London));
        let (base, _) =
            start_with_clock(tmp.path().to_path_buf(), ImageFormat::Bmp, clock.clone()).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        let plan = || async {
            reqwest::get(format!("{base}/plan"))
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap()
        };
        assert_eq!(plan().await["stale"], false);

        // Three hours on, with an hourly schedule and no render since, nothing about the file changed.
        clock.set(chrono::Utc::now().with_timezone(&London) + chrono::Duration::hours(3));
        let late = plan().await;
        assert_eq!(late["stale"], true);
        assert!(late["age_seconds"].as_u64().unwrap() >= 3 * 3600);
    }

    /// The query string the firmware builds, character for character (esphome/tests/report_test.cpp,
    /// `a_full_report_has_every_field_in_a_fixed_order`). The two halves are written and tested apart, so this
    /// is what stops a renamed field or a changed range on one side going unnoticed on the other.
    const FIRMWARE_REPORT: &str = "&device=reterminal-e1003-a1b2c3&failed_wakes=2&battery_mv=3712&battery_pct=47\
        &battery_state=ok&rssi=-67&last_failure=download&last_wake_s=24&last_tls_ms=1100&last_heap_min=61440&fw=0.1.0";

    #[tokio::test]
    async fn the_exact_report_the_firmware_builds_is_understood() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();

        assert_eq!(
            reqwest::get(format!("{base}/plan?have=1{FIRMWARE_REPORT}"))
                .await
                .unwrap()
                .status(),
            200
        );

        let status = reqwest::get(format!("{base}/status"))
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        let device = &status["devices"][0];
        assert_eq!(device["name"], "reterminal-e1003-a1b2c3");
        assert_eq!(device["failed_wakes"], 2);
        assert_eq!(device["battery_millivolts"], 3712);
        assert_eq!(device["battery_percent"], 47);
        assert_eq!(device["battery_state"], "ok");
        assert_eq!(device["wifi_rssi_dbm"], -67);
        assert_eq!(device["last_failure"], "download");
        assert_eq!(device["last_wake_seconds"], 24);
        assert_eq!(device["last_tls_milliseconds"], 1100);
        assert_eq!(device["last_heap_min_bytes"], 61440);
        assert_eq!(device["firmware"], "0.1.0");
    }

    #[tokio::test]
    async fn bad_telemetry_never_stops_the_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();

        let url =
            format!("{base}/plan?device=%22bad%0A&battery_mv=lots&battery_pct=-1&failed_wakes=x");
        assert_eq!(reqwest::get(url).await.unwrap().status(), 200);
        let devices = reqwest::get(format!("{base}/status"))
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(devices["devices"], serde_json::json!([]));
    }
}
