//! `GET /status`, `/healthz` and `/metrics`: how the service is doing.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::DateTime;
use chrono_tz::Tz;

use super::{Published, metrics, server_error};
use crate::application::devices::ConnectionStatus;
use crate::application::status::{MemberStatus, Status};

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
#[utoipa::path(
    get,
    path = "/status",
    tag = "monitoring",
    responses(
        (
            status = 200,
            description = "The render history, the state of each data source, the displays that have checked in, \
                and where each display stands in the certificate authority (empty over plain HTTP).",
            body = Status,
            content_type = "application/json",
            headers(("cache-control" = String, description = "Always `no-store`."))
        ),
        (status = 500, description = "The server could not work its state out.")
    )
)]
pub(super) async fn status(State(published): State<Arc<Published>>) -> Response {
    match current_status(&published).await {
        Ok(status) => ([(header::CACHE_CONTROL, "no-store")], Json(status)).into_response(),
        Err(e) => server_error("report", e),
    }
}

/// A pass or fail for monitors: 200 unless the image is stale, then 503, with a line saying why.
#[utoipa::path(
    get,
    path = "/healthz",
    tag = "monitoring",
    responses(
        (
            status = 200,
            description = "The service is working. Includes `degraded` (a source is down and an earlier \
                result stands in) and `failing` (renders are failing but the image is not yet stale).",
            body = String,
            content_type = "text/plain",
            example = "ok"
        ),
        (
            status = 503,
            description = "The image is stale, or none has been rendered. The body says which.",
            body = String,
            content_type = "text/plain",
            example = "stale: the image is 3 h 0 min old"
        ),
        (status = 500, description = "The server could not work its state out.")
    )
)]
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
    if let Some(handshakes) = &published.handles.handshakes {
        for device in &mut status.devices {
            device.connection = handshakes
                .device(&device.name)
                .map(|seen| ConnectionStatus::of(&seen, now.timezone()));
        }
    }
    if let Some(enrollment) = &published.handles.members {
        status.members = enrollment
            .pairings()
            .await?
            .iter()
            .map(|pairing| MemberStatus::of(pairing, now, enrollment.certificate_lifetime()))
            .collect();
    }
    Ok(status)
}

/// The same facts as `/status`, in the Prometheus text format, for a scraper.
#[utoipa::path(
    get,
    path = "/metrics",
    tag = "monitoring",
    responses(
        (
            status = 200,
            description = "The service's and the displays' state as Prometheus gauges (text exposition format 0.0.4), \
                for a scraper. Everything in it is derived from the same data as `/status`, so the two agree.",
            body = String,
            content_type = "text/plain; version=0.0.4; charset=utf-8",
            headers(("cache-control" = String, description = "Always `no-store`.")),
            example = json!("# HELP eink_info The running version. Always 1.\n# TYPE eink_info gauge\neink_info{version=\"0.1.0\"} 1\n")
        ),
        (status = 500, description = "The server could not work its state out.")
    )
)]
pub(super) async fn metrics(State(published): State<Arc<Published>>) -> Response {
    match current_status(&published).await {
        Ok(status) => (
            [
                (header::CONTENT_TYPE, metrics::CONTENT_TYPE),
                (header::CACHE_CONTROL, "no-store"),
            ],
            {
                let mut text = metrics::render(&status);
                if let Some(handshakes) = &published.handles.handshakes {
                    metrics::handshakes(&mut text, handshakes.counts());
                }
                text
            },
        )
            .into_response(),
        Err(e) => server_error("report", e),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::testing::{publish, set_age, start, start_full, start_with_status};
    use crate::adapters::clock::SystemClock;
    use crate::adapters::published_images::DirectoryImages;
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

    #[tokio::test]
    async fn status_lists_where_each_display_stands_in_the_authority() {
        use crate::adapters::certificate_authority::{self, Create};
        use crate::adapters::pairing_store::{FilePairingStore, Missing};
        use crate::application::enrollment::{Enrollment, EnrollmentPolicy};
        use crate::domain::models::device_id::DeviceId;
        use crate::domain::models::pairing::{Pairing, PairingState, PublicKey};
        use crate::domain::services::pairing_store::PairingStore;

        let pki = tempfile::tempdir().unwrap();
        let authority =
            Arc::new(certificate_authority::open(pki.path(), Create::IfMissing).unwrap());
        let store = Arc::new(
            FilePairingStore::open(pki.path(), Missing::StartEmpty)
                .await
                .unwrap(),
        );
        let now = chrono::Utc::now();
        for (name, state) in [
            (
                "kitchen",
                PairingState::Enrolled {
                    serial: "01".to_owned(),
                    not_after: now + chrono::Duration::days(20),
                },
            ),
            ("hall", PairingState::Pending),
        ] {
            let device = DeviceId::parse(name).unwrap();
            let key = PublicKey::from_der(name.as_bytes().repeat(8));
            store
                .put(&Pairing::new(device, key, state, now))
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
        let tmp = tempfile::tempdir().unwrap();
        let (base, _) = start_full(
            Arc::new(DirectoryImages::new(tmp.path().to_path_buf())),
            ImageFormat::Bmp,
            Arc::new(SystemClock::new(London)),
            Some(enrollment),
        )
        .await;

        let body: serde_json::Value = reqwest::get(format!("{base}/status"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let members = body["members"].as_array().unwrap();
        assert_eq!(members.len(), 2);
        let hall = members.iter().find(|m| m["name"] == "hall").unwrap();
        assert_eq!(hall["state"], "pending");
        assert!(hall["certificate_not_after"].is_null());
        let kitchen = members.iter().find(|m| m["name"] == "kitchen").unwrap();
        assert_eq!(kitchen["state"], "member");
        assert_eq!(kitchen["certificate_expired"], false);
        let left = kitchen["certificate_expires_in_seconds"].as_i64().unwrap();
        assert!((19 * 86_400..=20 * 86_400).contains(&left), "{left}");

        let metrics = reqwest::get(format!("{base}/metrics"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(metrics.contains("eink_member_state{device=\"kitchen\",state=\"member\"} 1"));
        assert!(
            metrics
                .contains("eink_member_certificate_expiry_timestamp_seconds{device=\"kitchen\"}")
        );
    }

    #[tokio::test]
    async fn over_plain_http_status_has_no_members() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        let body: serde_json::Value = reqwest::get(format!("{base}/status"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body["members"], serde_json::json!([]));
        let metrics = reqwest::get(format!("{base}/metrics"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(!metrics.contains("eink_member"));
    }
}
