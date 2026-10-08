//! Hands the latest rendered image to displays that fetch it over HTTP.
//! The display adapter publishes into a directory; this serves what is there.

mod advertise;
mod health;
mod identity;
mod image;
mod metrics;
mod negotiate;
mod plan;
#[cfg(test)]
mod testing;
mod transfer;

use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub use self::advertise::{Advertisement, SecureOffer, mdns_host_name};
use crate::adapters::listen::{self, Bound};
use crate::application::devices::{DeviceBoard, TelemetryLimits, telemetry_limits};
use crate::application::plan::PlanTiming;
use crate::application::refresh::RefreshControl;
use crate::application::status::StatusBoard;
use crate::domain::models::display::ImageFormat;
use crate::domain::models::schedule::Schedule;
use crate::domain::services::clock::Clock;
use crate::domain::services::published_images::PublishedImages;

pub(super) struct Published {
    pub(super) images: Arc<dyn PublishedImages>,
    pub(super) format: ImageFormat,
    pub(super) schedule: Schedule,
    pub(super) timing: PlanTiming,
    pub(super) handles: Handles,
    pub(super) clock: Arc<dyn Clock>,
}

/// How the server listens and announces itself.
#[derive(Debug, Clone)]
pub struct ServerSettings {
    /// Address to listen on.
    pub bind: std::net::SocketAddr,
    /// Announce the server over mDNS / DNS-SD.
    pub advertise: bool,
    /// The name shown for the service in a scan.
    pub instance_name: String,
    /// What `/plan` tells a display about when to come back.
    pub timing: PlanTiming,
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
    /// Where each display stands in the certificate authority, for `/status` and `/metrics`, when HTTPS
    /// is served.
    pub members: Option<Arc<crate::application::enrollment::Enrollment>>,
}

/// What the server says about itself in its API description.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "E-ink home display server",
        description = "What displays and monitors ask of the server that serves the dashboard image."
    ),
    tags(
        (name = "displays", description = "What a display asks of the server on each wake."),
        (name = "monitoring", description = "How the service is doing, for monitors and for the owner.")
    )
)]
struct ApiDoc;

/// The routes and what is known about them, in one place, so the API description cannot list a path the
/// server doesn't serve. A route is added with `routes!` once it is described (`#[utoipa::path]` on its
/// handler), and with `route` until then.
fn routes() -> OpenApiRouter<Arc<Published>> {
    let mut description = ApiDoc::openapi();
    // The package has no licence, and the derive would otherwise publish an empty one.
    description.info.license = None;
    OpenApiRouter::with_openapi(description)
        .routes(routes!(image::image))
        .routes(routes!(plan::plan))
        .routes(routes!(plan::refresh_now))
        .routes(routes!(health::healthz))
        .routes(routes!(health::status))
        .routes(routes!(health::metrics))
}

/// The API description (OpenAPI 3.1), as served at `/openapi.json` and committed as `config/openapi.json`.
pub fn openapi_json() -> String {
    let (_, mut api) = routes().split_for_parts();
    state_telemetry_limits(&mut api, &telemetry_limits());
    api.to_pretty_json()
        .expect("the API description serializes")
        + "\n"
}

/// Says in each telemetry parameter's description what the server accepts, from the limits the code itself
/// applies (see `telemetry_limits`), so the description cannot fall out of step with them. It is done here, after
/// the routes are collected, because the limits are values and an attribute can only hold text.
fn state_telemetry_limits(api: &mut utoipa::openapi::OpenApi, limits: &TelemetryLimits) {
    let range =
        |low: i64, high: i64| format!("Ignored unless a whole number from {low} to {high}.");
    let notes = [
        (
            "battery_mv",
            range(
                (*limits.battery_millivolts.start()).into(),
                (*limits.battery_millivolts.end()).into(),
            ),
        ),
        (
            "battery_pct",
            range(
                (*limits.battery_percent.start()).into(),
                (*limits.battery_percent.end()).into(),
            ),
        ),
        (
            "rssi",
            range(
                (*limits.wifi_rssi_dbm.start()).into(),
                (*limits.wifi_rssi_dbm.end()).into(),
            ),
        ),
        (
            "last_wake_s",
            range(
                (*limits.wake_seconds.start()).into(),
                (*limits.wake_seconds.end()).into(),
            ),
        ),
    ];
    for item in api.paths.paths.values_mut() {
        for operation in [&mut item.get, &mut item.post].into_iter().flatten() {
            for parameter in operation.parameters.iter_mut().flatten() {
                let utoipa::openapi::RefOr::T(parameter) = parameter else {
                    continue;
                };
                let Some((_, note)) = notes.iter().find(|(name, _)| *name == parameter.name) else {
                    continue;
                };
                parameter.description = Some(match parameter.description.take() {
                    Some(text) => format!("{text} {note}"),
                    None => note.clone(),
                });
            }
        }
    }
}

pub fn router(
    images: Arc<dyn PublishedImages>,
    format: ImageFormat,
    schedule: Schedule,
    timing: PlanTiming,
    handles: Handles,
    clock: Arc<dyn Clock>,
) -> Router {
    let (router, _) = routes().split_for_parts();
    let description: Arc<str> = openapi_json().into();
    router
        .route(
            "/openapi.json",
            get(move || {
                let description = description.clone();
                async move {
                    (
                        [(CONTENT_TYPE, "application/json")],
                        description.to_string(),
                    )
                }
            }),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(Arc::new(Published {
            images,
            format,
            schedule,
            timing,
            handles,
            clock,
        }))
}

pub(super) fn server_error(action: &str, e: impl std::fmt::Display) -> Response {
    log::error!("Failed to {action} the published image: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// Serves until the future is dropped, or fails at once if the address can't be bound.
pub async fn serve(
    settings: &ServerSettings,
    images: Arc<dyn PublishedImages>,
    format: ImageFormat,
    schedule: Schedule,
    handles: Handles,
    clock: Arc<dyn Clock>,
) -> anyhow::Result<()> {
    let app = router(images, format, schedule, settings.timing, handles, clock);
    serve_router(settings, app, format, None).await
}

/// Starts announcing the server over mDNS, if `settings` ask for it. Discovery is a convenience, so a
/// failure is a warning and the server runs without it. Held until serving ends, which is when the
/// goodbye goes out.
pub fn announce(
    settings: &ServerSettings,
    port: u16,
    format: ImageFormat,
    families: listen::Families,
    secure: Option<SecureOffer>,
) -> Option<Advertisement> {
    settings
        .advertise
        .then(|| Advertisement::start(settings, port, format, families, secure))
        .transpose()
        .unwrap_or_else(|e| {
            log::warn!("Not advertising over mDNS: {e:#}");
            None
        })
}

/// Serves `app` over plain HTTP at `settings.bind` and announces it, with the HTTPS on offer if any.
pub async fn serve_router(
    settings: &ServerSettings,
    app: Router,
    format: ImageFormat,
    secure: Option<SecureOffer>,
) -> anyhow::Result<()> {
    bind_http(settings)?
        .serve(settings, app, format, secure)
        .await
}

/// The plain-HTTP socket, open and not yet serving, so its address can be read (the port may have
/// been left to the system to choose) before it is.
pub struct HttpListener {
    bound: Bound,
}

pub fn bind_http(settings: &ServerSettings) -> anyhow::Result<HttpListener> {
    Ok(HttpListener {
        bound: listen::bind(settings.bind)?,
    })
}

impl HttpListener {
    pub fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.bound.listener.local_addr()
    }

    pub async fn serve(
        self,
        settings: &ServerSettings,
        app: Router,
        format: ImageFormat,
        secure: Option<SecureOffer>,
    ) -> anyhow::Result<()> {
        let Bound { listener, families } = self.bound;
        let port = listener.local_addr()?.port();
        log::info!(
            "Serving the display image at http://{}/image ({families})",
            settings.bind
        );
        // It announces the IP versions the socket really accepts, which isn't always what the
        // configured address says (see `listen`).
        let _advertisement = announce(settings, port, format, families, secure);
        axum::serve(listener, app)
            .await
            .context("Image server stopped")
    }
}

#[cfg(test)]
mod api_description {
    use std::path::Path;

    use serde_json::Value;

    use super::testing::{publish, set_age, start, start_with_status};
    use super::*;
    use crate::application::status::Status;
    use crate::domain::services::render_observer::RenderObserver;

    fn described() -> Value {
        serde_json::from_str(&openapi_json()).unwrap()
    }

    #[test]
    fn the_committed_description_is_current() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/openapi.json");
        assert_eq!(
            std::fs::read_to_string(path).unwrap_or_default(),
            openapi_json(),
            "config/openapi.json is stale: run `cargo run --bin gen_config -- --write`"
        );
    }

    #[test]
    fn every_reference_in_the_description_points_at_something() {
        assert_eq!(
            crate::adapters::api_doc_testing::dangling_references(&described()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_description_is_openapi_3_1_and_claims_no_licence() {
        let api = described();
        assert_eq!(api["openapi"], "3.1.0");
        assert!(api["info"].get("license").is_none(), "{}", api["info"]);
        assert_eq!(api["info"]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn healthz_is_described_as_a_get_and_nothing_else() {
        let api = described();
        let item = api["paths"]["/healthz"].as_object().unwrap();
        assert_eq!(item.keys().collect::<Vec<_>>(), ["get"]);
        let statuses: Vec<&str> = item["get"]["responses"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(statuses, ["200", "500", "503"]);
    }

    #[tokio::test]
    async fn healthz_answers_only_with_what_the_description_says_and_its_examples_are_real() {
        let responses = described()["paths"]["/healthz"]["get"]["responses"].clone();
        let declared = |status: u16| responses.get(status.to_string()).cloned();
        let example = |status: u16| {
            declared(status).unwrap()["content"]["text/plain"]["example"]
                .as_str()
                .unwrap()
                .to_owned()
        };

        let tmp = tempfile::tempdir().unwrap();
        let (base, _) = start_with_status(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        let ask = || async {
            let response = reqwest::get(format!("{base}/healthz")).await.unwrap();
            let status = response.status().as_u16();
            let kind = response.headers()["content-type"]
                .to_str()
                .unwrap()
                .to_owned();
            (status, kind, response.text().await.unwrap())
        };

        // Working: just started, then with a fresh image.
        let (status, kind, body) = ask().await;
        assert!(declared(status).is_some(), "{status} is not described");
        assert!(kind.starts_with("text/plain"), "{kind}");
        assert_eq!((status, body.as_str()), (200, "starting"));
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        let (status, _, body) = ask().await;
        assert!(declared(status).is_some(), "{status} is not described");
        assert_eq!((status, body), (200, example(200)));

        // Stale: the example in the description is what the server really says.
        set_age(tmp.path(), 3 * 3600);
        let (status, kind, body) = ask().await;
        assert!(declared(status).is_some(), "{status} is not described");
        assert!(kind.starts_with("text/plain"), "{kind}");
        assert_eq!((status, body), (503, example(503)));
    }

    fn check(schema: &Value, instance: &Value) -> Vec<String> {
        crate::adapters::api_doc_testing::check(&described(), schema, instance)
    }

    fn problems(name: &str, instance: &Value) -> Vec<String> {
        crate::adapters::api_doc_testing::problems(&described(), name, instance)
    }

    fn at(hour: u32) -> chrono::DateTime<chrono_tz::Tz> {
        use chrono::TimeZone;
        chrono_tz::Europe::London
            .with_ymd_and_hms(2026, 10, 8, hour, 0, 0)
            .unwrap()
    }

    /// A status with everything filled in: every optional field present, every kind of source, a display that
    /// has failed, one that has been sent an image, and members in different states.
    fn full_status() -> Status {
        use crate::application::devices::{
            BatteryState, DeliveryStatus, DeviceStatus, FailureReason,
        };
        use crate::application::status::{
            FailureStatus, Health, ImageStatus, MemberStatus, Moment,
        };
        use crate::domain::models::device_id::DeviceId;
        use crate::domain::models::render_report::{SourceReport, SourceState};

        let source = |name: &str, state| SourceReport {
            name: name.to_owned(),
            state,
        };
        Status {
            state: Health::Degraded,
            rendering: true,
            image: Some(ImageStatus {
                rendered_at: at(11),
                age_seconds: 600,
                version: 7,
            }),
            last_success: Some(Moment {
                at: at(11),
                age_seconds: 600,
            }),
            last_failure: Some(FailureStatus {
                at: at(9),
                age_seconds: 7_800,
                error: "Chrome did not start".to_owned(),
            }),
            consecutive_failures: 0,
            sources: vec![
                source("weather", SourceState::Fresh),
                source("NORTHBOUND", SourceState::Stale { age_seconds: 420 }),
                source(
                    "TURNPIKE LANE",
                    SourceState::Unavailable {
                        reason: "key rejected".to_owned(),
                    },
                ),
            ],
            devices: ImageFormat::ALL
                .into_iter()
                .map(|format| DeviceStatus {
                    name: DeviceId::parse(&format!("reterminal-{}", format.extension())).unwrap(),
                    last_seen: at(11),
                    age_seconds: 30,
                    expected_by: at(12),
                    overdue: false,
                    battery_millivolts: Some(3712),
                    battery_percent: Some(47),
                    battery_state: Some(BatteryState::Low),
                    failed_wakes: Some(2),
                    wifi_rssi_dbm: Some(-71),
                    last_failure: Some(FailureReason::Download),
                    last_wake_seconds: Some(24),
                    last_image: Some(DeliveryStatus {
                        format,
                        bytes: 2_592_054,
                        at: at(11),
                        age_seconds: 30,
                    }),
                })
                .collect(),
            members: vec![
                MemberStatus {
                    name: "kitchen".to_owned(),
                    state: "member",
                    certificate_not_after: Some(at(12)),
                    certificate_expires_in_seconds: Some(-3_600),
                    certificate_expired: true,
                    changing_keys: true,
                    replacement_waiting: true,
                },
                MemberStatus {
                    name: "hall".to_owned(),
                    state: "pending",
                    certificate_not_after: None,
                    certificate_expires_in_seconds: None,
                    certificate_expired: false,
                    changing_keys: false,
                    replacement_waiting: false,
                },
            ],
            next_render: Some(at(12)),
            schedule: vec!["At 00:00".to_owned()],
            uptime_seconds: 86_400,
            version: "0.1.0",
        }
    }

    #[test]
    fn a_status_with_everything_in_it_matches_its_description() {
        let json = serde_json::to_value(full_status()).unwrap();
        assert_eq!(problems("Status", &json), Vec::<String>::new());
    }

    #[test]
    fn a_status_with_almost_nothing_in_it_matches_its_description() {
        let status = Status {
            state: crate::application::status::Health::Starting,
            rendering: false,
            image: None,
            last_success: None,
            last_failure: None,
            consecutive_failures: 0,
            sources: vec![],
            devices: vec![],
            members: vec![],
            next_render: None,
            schedule: vec![],
            uptime_seconds: 0,
            version: "0.1.0",
        };
        let json = serde_json::to_value(status).unwrap();
        assert_eq!(problems("Status", &json), Vec::<String>::new());
    }

    #[test]
    fn the_check_does_notice_a_response_that_is_wrong() {
        // Without this the two tests above could pass for no reason.
        let mut json = serde_json::to_value(full_status()).unwrap();
        json["uptime_seconds"] = "a day".into();
        json["devices"][0]["last_image"]["format"] = "gif".into();
        json["sources"][1]["state"] = "half-stale".into();
        json["state"] = "fine".into();
        json.as_object_mut().unwrap().remove("members");
        let found = problems("Status", &json);
        // The validator names the offending value, or the missing property, not the field.
        for expected in [
            "a day",
            "gif",
            "half-stale",
            "\"fine\"",
            "\"members\" is a required",
        ] {
            assert!(
                found.iter().any(|problem| problem.contains(expected)),
                "{expected:?} not noticed in {found:?}"
            );
        }
    }

    #[tokio::test]
    async fn what_the_server_really_sends_at_status_matches_its_description() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, status) = start_with_status(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        status.render_succeeded(
            chrono::Utc::now().with_timezone(&chrono_tz::Europe::London),
            &crate::domain::models::render_report::RenderReport {
                sources: vec![crate::domain::models::render_report::SourceReport {
                    name: "weather".to_owned(),
                    state: crate::domain::models::render_report::SourceState::Unavailable {
                        reason: "key rejected".to_owned(),
                    },
                }],
            },
        );
        let response = reqwest::get(format!("{base}/status")).await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        let json: Value = response.json().await.unwrap();
        assert_eq!(problems("Status", &json), Vec::<String>::new());
    }

    fn examples() -> Vec<(String, Value, Value)> {
        crate::adapters::api_doc_testing::examples(&described())
    }

    #[test]
    fn every_example_in_the_description_satisfies_its_own_schema() {
        let all = examples();
        // Guards against a walk that finds nothing and so passes for no reason.
        assert!(all.len() >= 12, "only {} examples found", all.len());
        for (place, schema, example) in all {
            assert_eq!(
                check(&schema, &example),
                Vec::<String>::new(),
                "{place}: {example}"
            );
        }
    }

    #[test]
    fn the_described_routes_are_the_ones_the_server_has() {
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
        let expected = [
            ("/healthz", "get"),
            ("/image", "get"),
            ("/metrics", "get"),
            ("/plan", "get"),
            ("/refresh", "post"),
            ("/status", "get"),
        ];
        assert_eq!(routes.len(), expected.len(), "{routes:?}");
        for (path, method) in expected {
            let item = routes
                .iter()
                .find(|(p, _)| p == path)
                .unwrap_or_else(|| panic!("{path} missing"));
            assert_eq!(item.1, [method], "{path}");
        }
        // A button press is answered like a plan request, so the two describe the same answers.
        let statuses = |path: &str, method: &str| -> Vec<String> {
            api["paths"][path][method]["responses"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        };
        assert_eq!(statuses("/plan", "get"), statuses("/refresh", "post"));
    }

    #[tokio::test]
    async fn plan_answers_with_what_the_description_says() {
        let responses = described()["paths"]["/plan"]["get"]["responses"].clone();
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;

        // Before any render: the documented 404, with the documented words.
        let response = reqwest::get(format!("{base}/plan?device=kitchen"))
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        assert!(responses.get("404").is_some());
        let words = responses["404"]["content"]["text/plain"]["example"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(response.text().await.unwrap(), words);

        // With an image: the documented 200, which satisfies its schema and says it is not to be cached.
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        set_age(tmp.path(), 100);
        let response = reqwest::get(format!("{base}/plan?device=kitchen&have=1"))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        let plan: Value = response.json().await.unwrap();
        let schema = &responses["200"]["content"]["application/json"]["schema"];
        assert_eq!(check(schema, &plan), Vec::<String>::new(), "{plan}");
    }

    #[tokio::test]
    async fn a_reading_the_server_ignores_does_not_fail_the_request() {
        // Why the description says "ignored" and gives no minimum or maximum: a client that sends a reading
        // outside the range is not sending a bad request.
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        let limits = crate::application::devices::telemetry_limits();
        let top = *limits.battery_millivolts.end();
        let ask = |query: String| {
            let url = format!("{base}/plan?{query}");
            async move { reqwest::get(url).await.unwrap().status() }
        };
        assert_eq!(
            ask(format!(
                "device=hall&battery_mv={}&rssi=abc&battery_pct=101&battery_state=on%20fire",
                top + 1
            ))
            .await,
            200
        );
        assert_eq!(
            ask(format!("device=kitchen&battery_mv={top}&rssi=-71")).await,
            200
        );

        let status: Value = reqwest::get(format!("{base}/status"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let device = |name: &str| {
            status["devices"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["name"] == name)
                .unwrap()
                .clone()
        };
        // The top of the range is kept, one above it is dropped, and the display is still known either way.
        assert_eq!(device("kitchen")["battery_millivolts"], top);
        assert_eq!(device("kitchen")["wifi_rssi_dbm"], -71);
        assert!(device("hall")["battery_millivolts"].is_null());
        assert!(device("hall")["wifi_rssi_dbm"].is_null());
        assert!(device("hall")["battery_percent"].is_null());
        assert!(device("hall")["battery_state"].is_null());
    }

    #[test]
    fn the_ranges_in_the_description_are_the_ones_the_code_applies() {
        let limits = crate::application::devices::telemetry_limits();
        let api = described();
        let note = |name: &str| -> String {
            api["paths"]["/plan"]["get"]["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["name"] == name)
                .unwrap_or_else(|| panic!("{name} is not described"))["description"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        for (name, low, high) in [
            (
                "battery_mv",
                i64::from(*limits.battery_millivolts.start()),
                i64::from(*limits.battery_millivolts.end()),
            ),
            (
                "battery_pct",
                i64::from(*limits.battery_percent.start()),
                i64::from(*limits.battery_percent.end()),
            ),
            (
                "rssi",
                i64::from(*limits.wifi_rssi_dbm.start()),
                i64::from(*limits.wifi_rssi_dbm.end()),
            ),
            (
                "last_wake_s",
                i64::from(*limits.wake_seconds.start()),
                i64::from(*limits.wake_seconds.end()),
            ),
        ] {
            let text = note(name);
            assert!(
                text.contains(&format!(
                    "Ignored unless a whole number from {low} to {high}."
                )),
                "{name}: {text}"
            );
        }
        // /refresh takes the same readings and says the same of them.
        for parameter in api["paths"]["/refresh"]["post"]["parameters"]
            .as_array()
            .unwrap()
        {
            if parameter["name"] == "battery_mv" {
                assert_eq!(
                    parameter["description"],
                    api["paths"]["/plan"]["get"]["parameters"][2]["description"]
                );
            }
        }
    }

    #[tokio::test]
    async fn image_answers_with_what_the_description_says() {
        let responses = described()["paths"]["/image"]["get"]["responses"].clone();
        let declared = |status: u16| responses.get(status.to_string()).cloned();
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        let client = reqwest::Client::new();

        let nothing = client.get(format!("{base}/image")).send().await.unwrap();
        assert_eq!(nothing.status(), 404);
        assert!(declared(404).is_some());

        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        let image = client.get(format!("{base}/image")).send().await.unwrap();
        assert_eq!(image.status(), 200);
        let kind = image.headers()["content-type"].to_str().unwrap().to_owned();
        assert!(
            responses["200"]["content"].get(&kind).is_some(),
            "{kind} is not described"
        );
        for header in responses["200"]["headers"].as_object().unwrap().keys() {
            assert!(
                image.headers().get(header.as_str()).is_some(),
                "{header} is described but not sent"
            );
        }
        assert_eq!(image.headers()["cache-control"], "no-store");
        assert_eq!(image.headers()["vary"], "Accept");

        let refused = client
            .get(format!("{base}/image"))
            .header("accept", "text/html")
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 406);
        assert!(declared(406).is_some());
        assert_eq!(refused.headers()["vary"], "Accept");
        // And every format it can send is one the description lists.
        for format in ImageFormat::ALL {
            assert!(
                responses["200"]["content"]
                    .get(format.content_type())
                    .is_some(),
                "{} is not described",
                format.content_type()
            );
        }
    }

    #[tokio::test]
    async fn metrics_answers_with_what_the_description_says() {
        let api = described();
        let response = &api["paths"]["/metrics"]["get"]["responses"]["200"];
        let kind = response["content"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        let example = response["content"][&kind]["example"]
            .as_str()
            .unwrap()
            .to_owned();
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        let real = reqwest::get(format!("{base}/metrics")).await.unwrap();
        assert_eq!(real.status(), 200);
        assert_eq!(real.headers()["content-type"], kind.as_str());
        assert_eq!(real.headers()["cache-control"], "no-store");
        let body = real.text().await.unwrap();
        // The example is the start of a real answer, apart from the version.
        let first = example.lines().next().unwrap();
        assert!(
            body.starts_with(first),
            "{first:?} is not how {body:?} starts"
        );
    }

    #[tokio::test]
    async fn the_description_is_served_as_it_is_committed() {
        let tmp = tempfile::tempdir().unwrap();
        let base = start(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        let response = reqwest::get(format!("{base}/openapi.json")).await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(response.text().await.unwrap(), openapi_json());
    }
}
