//! `GET /status`, `/healthz` and `/metrics`: how the service is doing.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::DateTime;
use chrono_tz::Tz;

use super::{Published, metrics, server_error};
use crate::application::status::Status;

/// When the served image was written; None before the first render.
pub(super) async fn image_written_at(
    published: &Published,
) -> anyhow::Result<Option<DateTime<Tz>>> {
    let zone = published.clock.now().timezone();
    Ok(published
        .images
        .published_at(published.format)
        .await?
        .map(|at| at.with_timezone(&zone)))
}

/// How the service is doing: the render history, the sources' state, and the image's age.
pub(super) async fn status(State(published): State<Arc<Published>>) -> Response {
    match current_status(&published).await {
        Ok(status) => ([(header::CACHE_CONTROL, "no-store")], Json(status)).into_response(),
        Err(e) => server_error("report", e),
    }
}

/// A pass or fail for monitors: 200 unless the image is stale, then 503, with a line saying why.
pub(super) async fn healthz(State(published): State<Arc<Published>>) -> Response {
    match current_status(&published).await {
        Ok(status) => {
            let code = if status.is_healthy() {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            (
                code,
                [(header::CACHE_CONTROL, "no-store")],
                status.summary(),
            )
                .into_response()
        }
        Err(e) => server_error("check", e),
    }
}

async fn current_status(published: &Published) -> anyhow::Result<Status> {
    let rendered_at = image_written_at(published).await?;
    let now = published.clock.now();
    let mut status =
        published
            .handles
            .status
            .status(now, rendered_at, &published.schedule, published.timing)?;
    status.devices = published.handles.devices.snapshot(now);
    Ok(status)
}

/// The same facts as `/status`, in the Prometheus text format, for a scraper.
pub(super) async fn metrics(State(published): State<Arc<Published>>) -> Response {
    match current_status(&published).await {
        Ok(status) => (
            [
                (header::CONTENT_TYPE, metrics::CONTENT_TYPE),
                (header::CACHE_CONTROL, "no-store"),
            ],
            metrics::render(&status),
        )
            .into_response(),
        Err(e) => server_error("report", e),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{publish, set_age, start, start_with_status};
    use crate::domain::models::display::ImageFormat;
    use crate::domain::services::render_observer::RenderObserver;
    use chrono_tz::Europe::London;

    #[tokio::test]
    async fn healthz_answers_without_an_image() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Png).await;
        assert_eq!(
            reqwest::get(format!("{base}/healthz"))
                .await
                .unwrap()
                .status(),
            200
        );
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
        assert_eq!(
            (code, text.as_str()),
            (503, "stale: the image is 3 h 0 min old")
        );

        publish(tmp.path(), ImageFormat::Bmp, b"b").await.unwrap();
        status.render_succeeded(
            chrono::Utc::now().with_timezone(&London),
            &Default::default(),
        );
        assert_eq!(healthz(&base).await, (200, "ok".to_owned()));
    }

    #[tokio::test]
    async fn a_source_that_is_down_is_degraded_but_the_check_still_passes() {
        use crate::domain::models::render_report::{RenderReport, SourceReport, SourceState};

        let tmp = tempfile::tempdir().unwrap();
        let (base, status) = start_with_status(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        status.render_succeeded(
            chrono::Utc::now().with_timezone(&London),
            &RenderReport {
                sources: vec![SourceReport {
                    name: "weather".into(),
                    state: SourceState::Unavailable {
                        reason: "key rejected".into(),
                    },
                }],
            },
        );

        assert_eq!(healthz(&base).await, (200, "degraded".to_owned()));
        let body: serde_json::Value = reqwest::get(format!("{base}/status"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body["state"], "degraded");
        assert_eq!(body["sources"][0]["name"], "weather");
        assert_eq!(body["sources"][0]["state"], "unavailable");
        assert_eq!(body["sources"][0]["reason"], "key rejected");
    }

    #[tokio::test]
    async fn status_reports_the_history_the_image_and_the_next_render() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, status) = start_with_status(tmp.path().to_path_buf(), ImageFormat::Bmp).await;

        let before: serde_json::Value = reqwest::get(format!("{base}/status"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(before["state"], "starting");
        assert!(before["image"].is_null());
        assert!(before["last_success"].is_null());
        assert_eq!(before["consecutive_failures"], 0);

        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        status.render_started();
        status.render_failed(
            chrono::Utc::now().with_timezone(&London),
            &anyhow::anyhow!("Chrome did not start"),
        );
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

    #[tokio::test]
    async fn a_check_in_shows_up_in_status_and_metrics() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();

        let status = || async {
            reqwest::get(format!("{base}/status"))
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap()
        };
        assert_eq!(status().await["devices"], serde_json::json!([]));

        let url = format!(
            "{base}/plan?have=1&device=kitchen&battery_mv=3350&battery_pct=8&battery_state=low&failed_wakes=2&rssi=-71&last_failure=download&last_wake_s=24"
        );
        assert_eq!(reqwest::get(url).await.unwrap().status(), 200);

        let devices = status().await["devices"].clone();
        assert_eq!(devices[0]["name"], "kitchen");
        assert_eq!(devices[0]["battery_millivolts"], 3350);
        assert_eq!(devices[0]["battery_state"], "low");
        assert_eq!(devices[0]["overdue"], false);
        assert_eq!(devices[0]["wifi_rssi_dbm"], -71);
        assert_eq!(devices[0]["last_failure"], "download");
        assert_eq!(devices[0]["last_wake_seconds"], 24);

        let response = reqwest::get(format!("{base}/metrics")).await.unwrap();
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/plain; version=0.0.4")
        );
        let text = response.text().await.unwrap();
        assert!(
            text.contains("eink_device_battery_volts{device=\"kitchen\"} 3.35"),
            "{text}"
        );
        assert!(
            text.contains("eink_device_battery_state{device=\"kitchen\",state=\"low\"} 1"),
            "{text}"
        );
        assert!(
            text.contains("eink_device_wifi_rssi_dbm{device=\"kitchen\"} -71"),
            "{text}"
        );
        assert!(
            text.contains("eink_device_last_failure{device=\"kitchen\",reason=\"download\"} 1"),
            "{text}"
        );
    }
}
