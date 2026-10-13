//! `/view`: a page for a web browser to look at the picture, and the small API behind it.
//!
//! The page is one embedded HTML file with its script and style inline, which is what lets it be served with a content
//! security policy that names those two blocks by hash and allows nothing else (no `unsafe-inline`, no other
//! scripts, no network but this server). It asks `/view/api/layouts` what there is to show, `/view/api/state` which render
//! the picture is and when to look again (the same facts `/plan` gives a display, so the page comes back when the server
//! says and not on a timer of its own), and `/view/api/image` for the picture.
//!
//! None of this is a display: it never records telemetry, so a browser doesn't turn up among the displays, and it asks for
//! the PNG, which is a fraction of the BMP a display with no preference is sent.
//!
//! These routes make their own decision about who may use them (see `Access`), and so sit outside the gate that the
//! display routes are behind over HTTPS.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tower_http::trace::TraceLayer;
use utoipa::{IntoParams, OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::health::image_written_at;
use super::image::{etag_of, names};
use super::{Published, server_error};
use crate::application::plan::{Plan, compute};
use crate::domain::models::display::ImageFormat;
use crate::domain::models::layout::{Layout, LayoutName};

const PAGE: &str = include_str!("view.html");

/// Who the page and its API are for.
#[derive(Debug, Clone, Default)]
pub struct WebSettings {
    /// The layouts the page may show, in the order it lists them. Empty means all of them.
    pub layouts: Vec<LayoutName>,
}

/// The layouts `settings` allows out of those there are, or the name of one that isn't among them.
pub fn allowed_layouts(
    settings: &WebSettings,
    layouts: &crate::domain::models::layout::Layouts,
) -> Result<Vec<Layout>, String> {
    if settings.layouts.is_empty() {
        return Ok(layouts.all().to_vec());
    }
    settings
        .layouts
        .iter()
        .map(|name| {
            layouts.find(name.as_str()).cloned().ok_or_else(|| {
                format!(
                    "[server.web] layouts names {name}, which there is not (there are: {})",
                    layouts
                        .all()
                        .iter()
                        .map(|layout| layout.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        })
        .collect()
}

pub(super) struct WebState {
    published: Arc<Published>,
    /// What the page may show, in order. Never empty.
    allowed: Vec<Layout>,
    /// The content security policy the page is served with.
    policy: HeaderValue,
}

impl WebState {
    pub(super) fn new(published: Arc<Published>, allowed: Vec<Layout>) -> Self {
        assert!(!allowed.is_empty(), "the page has a layout to show");
        Self {
            published,
            allowed,
            policy: HeaderValue::from_str(&content_security_policy(PAGE))
                .expect("a policy is a header value"),
        }
    }

    /// The layout a request without one gets: the one for this time of day if the page may show it, else the first it may.
    fn automatic(&self) -> &Layout {
        let current = self.published.layouts.current(self.published.clock.now());
        self.allowed
            .iter()
            .find(|layout| layout.name == current.name)
            .unwrap_or(&self.allowed[0])
    }

    /// The layout asked for (`auto`, or none, is the automatic one), or the answer to give if it is not one the page may
    /// show.
    fn resolve(&self, wanted: Option<&str>) -> Result<&Layout, (StatusCode, String)> {
        match wanted {
            None | Some("auto") => Ok(self.automatic()),
            Some(name) => self
                .allowed
                .iter()
                .find(|layout| layout.name.as_str() == name)
                .ok_or_else(|| {
                    let known: Vec<_> = self
                        .allowed
                        .iter()
                        .map(|layout| layout.name.as_str())
                        .collect();
                    // The name is not repeated back: it is whatever the caller sent.
                    (
                        StatusCode::NOT_FOUND,
                        format!("There is no such layout. Layouts: {}", known.join(", ")),
                    )
                }),
        }
    }
}

/// The text between the first `<tag>` and its `</tag>`, which is exactly what a browser hashes for a policy.
fn inline_block<'a>(html: &'a str, tag: &str) -> &'a str {
    let open = format!("<{tag}>");
    let start = html.find(&open).expect("the page has the block") + open.len();
    let end = start
        + html[start..]
            .find(&format!("</{tag}>"))
            .expect("the block ends");
    &html[start..end]
}

fn hash_source(text: &str) -> String {
    format!(
        "'sha256-{}'",
        STANDARD.encode(Sha256::digest(text.as_bytes()))
    )
}

/// A policy that lets the page run its own script and apply its own style, show pictures it made from what it fetched, and
/// talk to this server: nothing else.
fn content_security_policy(page: &str) -> String {
    format!(
        "default-src 'none'; script-src {}; style-src {}; img-src blob:; connect-src 'self'; base-uri 'none'; \
         form-action 'none'; frame-ancestors 'none'",
        hash_source(inline_block(page, "script")),
        hash_source(inline_block(page, "style")),
    )
}

#[derive(OpenApi)]
#[openapi(tags((
    name = "web",
    description = "A page for a web browser to look at the picture, and what it asks of the server. Served only when [server.web] is enabled."
)))]
struct WebDoc;

pub(super) fn routes() -> OpenApiRouter<Arc<WebState>> {
    OpenApiRouter::with_openapi(WebDoc::openapi())
        .routes(routes!(page))
        .routes(routes!(layouts))
        .routes(routes!(state))
        .routes(routes!(image))
}

/// The routes, with the page's own security headers, for `router` to serve.
pub(super) fn router(state: WebState) -> Router {
    let (router, _) = routes().split_for_parts();
    crate::adapters::response_headers::hardened(router.layer(TraceLayer::new_for_http()))
        .with_state(Arc::new(state))
}

/// The page.
#[utoipa::path(
    get,
    path = "/view",
    tag = "web",
    responses((
        status = 200,
        description = "A web page that shows the picture and keeps it up to date.",
        content_type = "text/html",
        body = String
    ))
)]
async fn page(State(state): State<Arc<WebState>>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        // The policy is a header the response sets itself, so the hardening default does not replace it.
        [(header::CONTENT_SECURITY_POLICY, state.policy.clone())],
        PAGE,
    )
        .into_response()
}

/// Which layout a request is about. Optional: without it, or with `auto`, the one for this time of day.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct WhichLayout {
    /// One of the names `/view/api/layouts` lists, or `auto`.
    #[param(value_type = Option<String>, example = "departures")]
    layout: Option<String>,
}

#[derive(Serialize, ToSchema)]
struct LayoutItem {
    /// What to put after `layout=`.
    #[schema(examples("departures"))]
    name: String,
    /// What the layout shows.
    description: String,
    /// Whether this is the one `auto` means now.
    current: bool,
}

#[derive(Serialize, ToSchema)]
struct LayoutItems {
    /// The layouts the page may show, the first being what it falls back to.
    layouts: Vec<LayoutItem>,
}

/// The layouts the page may show.
#[utoipa::path(
    get,
    path = "/view/api/layouts",
    tag = "web",
    responses((
        status = 200,
        description = "The layouts, and which one `auto` means now.",
        body = LayoutItems,
        content_type = "application/json",
        headers(("cache-control" = String, description = "Always `no-store`."))
    ))
)]
async fn layouts(State(state): State<Arc<WebState>>) -> Response {
    let automatic = state.automatic().name.clone();
    let items = state
        .allowed
        .iter()
        .map(|layout| LayoutItem {
            name: layout.name.to_string(),
            description: layout.description.clone(),
            current: layout.name == automatic,
        })
        .collect();
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(LayoutItems { layouts: items }),
    )
        .into_response()
}

/// Which render the picture is, how old, and when to look again.
#[derive(Serialize, ToSchema)]
struct ViewState {
    /// The layout this is the state of: what `auto` came to.
    #[schema(examples("dashboard"))]
    layout: String,
    #[serde(flatten)]
    plan: Plan,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct Have {
    /// The version the page shows, so the answer can say whether it has changed.
    #[param(example = 1791463200)]
    have: Option<u32>,
}

/// The state of a layout's picture: the same facts `/plan` gives a display, without recording anyone.
#[utoipa::path(
    get,
    path = "/view/api/state",
    tag = "web",
    params(WhichLayout, Have),
    responses(
        (
            status = 200,
            description = "The picture's version, its age, whether it is stale, and how long to wait before asking again.",
            body = ViewState,
            content_type = "application/json",
            headers(("cache-control" = String, description = "Always `no-store`."))
        ),
        (status = 404, description = "Nothing has been rendered yet, or there is no such layout (the body lists the ones there are).", body = String, content_type = "text/plain"),
        (status = 500, description = "The server could not work the state out.")
    )
)]
async fn state(
    State(state): State<Arc<WebState>>,
    Query(which): Query<WhichLayout>,
    Query(have): Query<Have>,
) -> Response {
    let layout = match state.resolve(which.layout.as_deref()) {
        Ok(layout) => layout,
        Err(refusal) => return refusal.into_response(),
    };
    let published = &state.published;
    let rendered_at = match image_written_at(published, layout).await {
        Ok(Some(rendered_at)) => rendered_at,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response();
        }
        Err(e) => return server_error("inspect", e),
    };
    match compute(
        published.clock.now(),
        rendered_at,
        &published.schedule,
        published.timing,
        have.have,
    ) {
        Ok(plan) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(ViewState {
                layout: layout.name.to_string(),
                plan,
            }),
        )
            .into_response(),
        Err(e) => server_error("plan", e),
    }
}

/// A layout's picture, as a PNG (a BMP if there is no PNG).
#[utoipa::path(
    get,
    path = "/view/api/image",
    tag = "web",
    params(WhichLayout),
    responses(
        (
            status = 200,
            description = "The picture. Always a PNG when there is one, whatever the request accepts: this is for a browser, which can show it.",
            content(("image/png"), ("image/bmp")),
            headers(
                ("etag" = String, description = "Identifies this file's bytes."),
                ("cache-control" = String, description = "Always `no-store`.")
            )
        ),
        (status = 304, description = "The client's `If-None-Match` names this picture. No body."),
        (status = 404, description = "Nothing has been rendered yet, or there is no such layout.", body = String, content_type = "text/plain"),
        (status = 500, description = "An image exists but could not be read.")
    )
)]
async fn image(
    State(state): State<Arc<WebState>>,
    Query(which): Query<WhichLayout>,
    headers: HeaderMap,
) -> Response {
    let layout = match state.resolve(which.layout.as_deref()) {
        Ok(layout) => layout,
        Err(refusal) => return refusal.into_response(),
    };
    let images = &state.published.images;
    for format in [ImageFormat::Png, ImageFormat::Bmp] {
        match images.read(&layout.name, format).await {
            Ok(Some(bytes)) => {
                let etag = etag_of(&bytes);
                let cached = headers
                    .get(header::IF_NONE_MATCH)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| names(value, &etag));
                if cached {
                    return (
                        StatusCode::NOT_MODIFIED,
                        [
                            (header::ETAG, etag),
                            (header::CACHE_CONTROL, "no-store".to_owned()),
                        ],
                    )
                        .into_response();
                }
                return (
                    [
                        (header::ETAG, etag),
                        (header::CONTENT_TYPE, format.content_type().to_owned()),
                        (header::CACHE_CONTROL, "no-store".to_owned()),
                    ],
                    bytes,
                )
                    .into_response();
            }
            Ok(None) => continue,
            Err(e) => return server_error("read", e),
        }
    }
    (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response()
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveTime, TimeZone};
    use chrono_tz::Europe::London;

    use super::super::testing::{
        FIRST, publish, publish_layout, start_web, start_with_two_layouts,
    };
    use super::*;
    use crate::adapters::clock::{FixedClock, SystemClock};
    use crate::domain::models::layout::{LayoutRule, LayoutSchedule, Layouts};

    fn two_layouts() -> Layouts {
        Layouts::new(vec![
            Layout::new(FIRST, "Everything"),
            Layout::new("departures", "Just the trains"),
        ])
        .unwrap()
    }

    fn only(names: &[&str]) -> WebSettings {
        WebSettings {
            layouts: names
                .iter()
                .map(|n| LayoutName::parse(n).unwrap())
                .collect(),
        }
    }

    async fn serve(directory: &std::path::Path, settings: &WebSettings) -> String {
        start_web(
            directory.to_path_buf(),
            two_layouts(),
            settings,
            Arc::new(SystemClock::new(London)),
        )
        .await
        .unwrap()
        .0
    }

    /// Publishes `layout`'s picture as the server does: every format, the preferred (BMP here) last, so its date is the
    /// render's version.
    async fn publish_both(directory: &std::path::Path, layout: &str, bytes: &[u8]) {
        for format in [ImageFormat::Png, ImageFormat::Bmp] {
            publish_layout(directory, layout, format, bytes)
                .await
                .unwrap();
        }
    }

    async fn get(url: String) -> (u16, String) {
        let response = reqwest::get(url).await.unwrap();
        (response.status().as_u16(), response.text().await.unwrap())
    }

    async fn json(url: String) -> serde_json::Value {
        serde_json::from_str(&get(url).await.1).unwrap()
    }

    // --- the page -------------------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_page_is_html_with_a_policy_that_names_its_own_script_and_style_and_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        let base = serve(tmp.path(), &WebSettings::default()).await;

        let response = reqwest::get(format!("{base}/view")).await.unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        let policy = response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .to_owned();
        let body = response.text().await.unwrap();
        // The hashes are those of the blocks the browser will find, worked out here from the page as it was sent.
        let hash = |tag: &str| {
            let block = inline_block(&body, tag);
            format!(
                "'sha256-{}'",
                STANDARD.encode(Sha256::digest(block.as_bytes()))
            )
        };
        assert!(
            policy.contains(&format!("script-src {}", hash("script"))),
            "{policy}"
        );
        assert!(
            policy.contains(&format!("style-src {}", hash("style"))),
            "{policy}"
        );
        for rule in [
            "default-src 'none'",
            "connect-src 'self'",
            "img-src blob:",
            "base-uri 'none'",
            "frame-ancestors 'none'",
        ] {
            assert!(policy.contains(rule), "{rule} in {policy}");
        }
        assert!(
            !policy.contains("unsafe-inline") && !policy.contains("unsafe-eval"),
            "{policy}"
        );
    }

    #[test]
    fn the_page_has_one_script_one_style_and_nothing_a_policy_like_that_would_block() {
        assert_eq!(PAGE.matches("<script").count(), 1);
        assert_eq!(PAGE.matches("<style").count(), 1);
        // Nothing fetched from anywhere else, no handler written into the markup, nothing that runs from a link.
        assert!(
            !PAGE.contains("src=\"http"),
            "a script or image from elsewhere"
        );
        assert!(!PAGE.contains("href=\"http"));
        assert!(!PAGE.contains("javascript:"));
        for handler in [" onclick=", " onload=", " onerror=", " onchange="] {
            assert!(!PAGE.contains(handler), "{handler}");
        }
        // The script talks to this server at one place, by an address that does not depend on how the page was reached.
        assert!(PAGE.contains("'/view/api/'"));
    }

    // --- what it may show -----------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_layouts_listed_are_those_the_settings_allow_in_the_order_given() {
        let tmp = tempfile::tempdir().unwrap();

        let all = json(format!(
            "{}/view/api/layouts",
            serve(tmp.path(), &WebSettings::default()).await
        ))
        .await;
        let some = json(format!(
            "{}/view/api/layouts",
            serve(tmp.path(), &only(&["departures"])).await
        ))
        .await;
        let reversed = json(format!(
            "{}/view/api/layouts",
            serve(tmp.path(), &only(&["departures", FIRST])).await
        ))
        .await;

        let names = |list: &serde_json::Value| {
            list["layouts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l["name"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&all), [FIRST, "departures"]);
        assert_eq!(names(&some), ["departures"]);
        assert_eq!(names(&reversed), ["departures", FIRST]);
        assert_eq!(all["layouts"][0]["description"], "Everything");
    }

    #[test]
    fn settings_naming_a_layout_there_is_not_are_an_error_that_says_what_there_is() {
        let error = allowed_layouts(&only(&["nonesuch"]), &two_layouts()).unwrap_err();
        assert!(
            error.contains("nonesuch") && error.contains("dashboard, departures"),
            "{error}"
        );
        assert_eq!(
            allowed_layouts(&WebSettings::default(), &two_layouts())
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn a_layout_there_is_but_the_page_may_not_show_is_not_found_like_one_there_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let base = serve(tmp.path(), &only(&["departures"])).await;
        publish_both(tmp.path(), FIRST, b"all").await;
        publish_both(tmp.path(), "departures", b"trains").await;

        for path in ["state", "image"] {
            let forbidden = get(format!("{base}/view/api/{path}?layout={FIRST}")).await;
            let missing = get(format!("{base}/view/api/{path}?layout=nonesuch")).await;
            assert_eq!(forbidden.0, 404, "{path}");
            // The answer is the same, so it does not say which layouts exist beyond those it may show.
            assert_eq!(forbidden.1, missing.1, "{path}");
            assert!(
                forbidden.1.contains("departures"),
                "{path}: {}",
                forbidden.1
            );
            assert!(!forbidden.1.contains("nonesuch") && !forbidden.1.contains(FIRST));
        }
    }

    // --- automatic ------------------------------------------------------------------------------------------

    fn with_a_morning_rule() -> Layouts {
        let morning = LayoutRule::new(
            LayoutName::parse("departures").unwrap(),
            vec![],
            NaiveTime::from_hms_opt(6, 30, 0).unwrap(),
            NaiveTime::from_hms_opt(9, 30, 0).unwrap(),
        )
        .unwrap();
        two_layouts()
            .with_schedule(LayoutSchedule::new(vec![morning]))
            .unwrap()
    }

    fn at(hour: u32, minute: u32) -> chrono::DateTime<chrono_tz::Tz> {
        London
            .with_ymd_and_hms(2026, 10, 12, hour, minute, 0)
            .unwrap()
    }

    #[tokio::test]
    async fn auto_follows_the_schedule_among_the_layouts_the_page_may_show() {
        let tmp = tempfile::tempdir().unwrap();
        publish_both(tmp.path(), FIRST, b"all").await;
        publish_both(tmp.path(), "departures", b"trains").await;
        let clock = FixedClock::at(at(7, 0));
        let (everything, _) = start_web(
            tmp.path().to_path_buf(),
            with_a_morning_rule(),
            &WebSettings::default(),
            clock.clone(),
        )
        .await
        .unwrap();
        let (dashboard_only, _) = start_web(
            tmp.path().to_path_buf(),
            with_a_morning_rule(),
            &only(&[FIRST]),
            clock.clone(),
        )
        .await
        .unwrap();
        let layout_of = |base: String, query: &'static str| async move {
            json(format!("{base}/view/api/state{query}")).await["layout"]
                .as_str()
                .unwrap()
                .to_owned()
        };

        // In the morning window the schedule says departures; asked for nothing or `auto`, that is what comes.
        assert_eq!(layout_of(everything.clone(), "").await, "departures");
        assert_eq!(
            layout_of(everything.clone(), "?layout=auto").await,
            "departures"
        );
        // A layout asked for by name is that one.
        assert_eq!(
            layout_of(everything.clone(), "?layout=dashboard").await,
            FIRST
        );
        // A page that may not show departures falls back to what it may.
        assert_eq!(layout_of(dashboard_only, "").await, FIRST);

        clock.set(at(12, 0));
        assert_eq!(layout_of(everything.clone(), "").await, FIRST);
        // The listing marks the one `auto` means.
        let list = json(format!("{everything}/view/api/layouts")).await;
        let current: Vec<_> = list["layouts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["current"] == true)
            .collect();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0]["name"], FIRST);
    }

    // --- state ---------------------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_state_says_which_render_it_is_and_when_to_look_again_and_records_nobody() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, handles) = start_web(
            tmp.path().to_path_buf(),
            two_layouts(),
            &WebSettings::default(),
            Arc::new(SystemClock::new(London)),
        )
        .await
        .unwrap();
        // Nothing drawn yet.
        let nothing = get(format!("{base}/view/api/state")).await;
        assert_eq!(nothing, (404, "No image has been rendered yet".to_owned()));

        publish_both(tmp.path(), FIRST, b"a").await;
        let response = reqwest::get(format!(
            "{base}/view/api/state?device=kitchen&battery_pct=50"
        ))
        .await
        .unwrap();
        assert_eq!(response.headers()["cache-control"], "no-store");
        let state: serde_json::Value = response.json().await.unwrap();

        assert_eq!(state["layout"], FIRST);
        assert!(state["version"].as_u64().unwrap() > 0);
        assert_eq!(state["stale"], false);
        assert!(state["next_seconds"].as_u64().unwrap() > 0);
        assert!(state["age_seconds"].as_u64().unwrap() < 60);
        assert_eq!(state["timezone"], "Europe/London");
        // The page is not a display: whatever it sends about itself, it does not turn up among them.
        assert!(
            handles
                .devices
                .snapshot(chrono::Utc::now().with_timezone(&London))
                .is_empty()
        );

        // And what it already has is judged against the version.
        let version = state["version"].as_u64().unwrap();
        let same = json(format!("{base}/view/api/state?have={version}")).await;
        assert_eq!(same["changed"], false);
        let other = json(format!("{base}/view/api/state?have=1")).await;
        assert_eq!(other["changed"], true);
    }

    // --- the picture -----------------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_picture_is_the_png_whatever_is_accepted_with_the_bmp_only_if_there_is_no_png() {
        let tmp = tempfile::tempdir().unwrap();
        let base = serve(tmp.path(), &WebSettings::default()).await;
        assert_eq!(get(format!("{base}/view/api/image")).await.0, 404);

        publish(tmp.path(), ImageFormat::Bmp, b"the bmp")
            .await
            .unwrap();
        let only_bmp = reqwest::get(format!("{base}/view/api/image"))
            .await
            .unwrap();
        assert_eq!(only_bmp.headers()["content-type"], "image/bmp");

        publish(tmp.path(), ImageFormat::Png, b"the png")
            .await
            .unwrap();
        let client = reqwest::Client::new();
        for accept in ["*/*", "image/bmp", "image/avif,image/webp,*/*"] {
            let response = client
                .get(format!("{base}/view/api/image"))
                .header("accept", accept)
                .send()
                .await
                .unwrap();
            assert_eq!(response.headers()["content-type"], "image/png", "{accept}");
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert_eq!(response.bytes().await.unwrap().as_ref(), b"the png");
        }
    }

    #[tokio::test]
    async fn the_picture_follows_the_layout_and_answers_304_to_one_it_already_has() {
        let tmp = tempfile::tempdir().unwrap();
        let base = serve(tmp.path(), &WebSettings::default()).await;
        publish(tmp.path(), ImageFormat::Png, b"all").await.unwrap();
        publish_layout(tmp.path(), "departures", ImageFormat::Png, b"trains")
            .await
            .unwrap();

        assert_eq!(get(format!("{base}/view/api/image")).await.1, "all");
        assert_eq!(
            get(format!("{base}/view/api/image?layout=departures"))
                .await
                .1,
            "trains"
        );

        let first = reqwest::get(format!("{base}/view/api/image?layout=departures"))
            .await
            .unwrap();
        let etag = first.headers()["etag"].to_str().unwrap().to_owned();
        let again = reqwest::Client::new()
            .get(format!("{base}/view/api/image?layout=departures"))
            .header("if-none-match", &etag)
            .send()
            .await
            .unwrap();
        assert_eq!(again.status(), 304);
        assert_eq!(again.headers()["etag"], etag.as_str());
        // The same name for another layout's picture is not a match.
        let other = reqwest::Client::new()
            .get(format!("{base}/view/api/image"))
            .header("if-none-match", &etag)
            .send()
            .await
            .unwrap();
        assert_eq!(other.status(), 200);
    }

    #[tokio::test]
    async fn the_routes_carry_the_hardening_headers_and_nothing_else_is_served() {
        let tmp = tempfile::tempdir().unwrap();
        let base = serve(tmp.path(), &WebSettings::default()).await;
        publish_both(tmp.path(), FIRST, b"a").await;

        let response = reqwest::get(format!("{base}/view/api/layouts"))
            .await
            .unwrap();
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            response.headers()["cross-origin-resource-policy"],
            "same-origin"
        );
        // Only the web routes: the display routes are not served by this router.
        for path in ["/image", "/plan", "/status", "/metrics", "/healthz"] {
            assert_eq!(get(format!("{base}{path}")).await.0, 404, "{path}");
        }
        // The web routes are the ones the API description lists for them.
        let described = openapi_paths();
        for path in [
            "/view",
            "/view/api/layouts",
            "/view/api/state",
            "/view/api/image",
        ] {
            assert!(described.contains(&path.to_owned()), "{path}");
            assert_ne!(get(format!("{base}{path}")).await.0, 404, "{path}");
        }
    }

    /// The paths the web routes are described under.
    fn openapi_paths() -> Vec<String> {
        let (_, api) = routes().split_for_parts();
        api.paths.paths.keys().cloned().collect()
    }

    #[tokio::test]
    async fn the_display_routes_server_is_unaffected_by_there_being_a_page() {
        // The two-layout display server of the other tests still answers /image as before.
        let tmp = tempfile::tempdir().unwrap();
        let base = start_with_two_layouts(tmp.path().to_path_buf(), ImageFormat::Bmp).await;
        publish(tmp.path(), ImageFormat::Bmp, b"a").await.unwrap();
        assert_eq!(get(format!("{base}/image")).await.1, "a");
        assert_eq!(get(format!("{base}/view")).await.0, 404);
    }

    // --- in a real browser -----------------------------------------------------------------------------------

    /// A PNG and a BMP of one colour and `width` x 10, so the page's picture can be told apart by its size.
    fn picture(width: u32, format: image::ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::GrayImage::from_pixel(width, 10, image::Luma([128]))
            .write_to(&mut out, format)
            .unwrap();
        out.into_inner()
    }

    async fn publish_picture(directory: &std::path::Path, layout: &str, width: u32) {
        publish_layout(
            directory,
            layout,
            ImageFormat::Png,
            &picture(width, image::ImageFormat::Png),
        )
        .await
        .unwrap();
        publish_layout(
            directory,
            layout,
            ImageFormat::Bmp,
            &picture(width, image::ImageFormat::Bmp),
        )
        .await
        .unwrap();
    }

    /// Needs a real Chrome, so run with `--ignored`. Loads the page under the policy it is served with, and checks that
    /// what the script does works there: shows the picture, lists the layouts, and changes layout when asked.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn the_page_works_in_a_browser_under_its_own_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let base = serve(tmp.path(), &WebSettings::default()).await;
        publish_picture(tmp.path(), FIRST, 40).await;
        publish_picture(tmp.path(), "departures", 20).await;

        let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let browser = headless_chrome::Browser::new(
                headless_chrome::LaunchOptions::default_builder()
                    .headless(true)
                    .path(headless_chrome::browser::default_executable().ok())
                    .build()?,
            )?;
            let tab = browser.new_tab()?;
            tab.navigate_to(&format!("{base}/view"))?
                .wait_until_navigated()?;
            let number = |script: &str| -> anyhow::Result<f64> {
                Ok(tab
                    .evaluate(script, false)?
                    .value
                    .and_then(|v| v.as_f64())
                    .unwrap_or(-1.0))
            };
            let text = |script: &str| -> anyhow::Result<String> {
                Ok(tab
                    .evaluate(script, false)?
                    .value
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default())
            };
            // Waits for `script` to be at least `wanted`, as the page does its fetching.
            let waits = |script: &str, wanted: f64| -> anyhow::Result<()> {
                for _ in 0..100 {
                    if number(script)? >= wanted {
                        return Ok(());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                anyhow::bail!("{script} never reached {wanted}")
            };

            // The picture of the layout the schedule says (here the default) is shown, and the status line says how old.
            waits("document.getElementById('picture').naturalWidth", 40.0)?;
            anyhow::ensure!(
                text("document.getElementById('status').textContent")?.contains("updated"),
                "no age shown"
            );
            // Auto, and the two layouts, are offered.
            anyhow::ensure!(
                number("document.querySelectorAll('#layouts button').length")? == 3.0,
                "the layouts were not offered"
            );
            // Choosing one shows its picture, and remembers it in the address.
            tab.evaluate(
                "document.querySelectorAll('#layouts button')[2].click()",
                false,
            )?;
            waits("document.getElementById('picture').naturalWidth", 20.0)?;
            anyhow::ensure!(
                text("location.search")?.contains("layout=departures"),
                "the choice is not in the address"
            );
            anyhow::ensure!(
                text(
                    "document.querySelectorAll('#layouts button')[2].getAttribute('aria-pressed')"
                )? == "true",
                "the choice is not shown as chosen"
            );
            // For looking at: where VIEW_SCREENSHOT names a file, the page as it stands.
            if let Ok(path) = std::env::var("VIEW_SCREENSHOT") {
                let png = tab.capture_screenshot(
                    headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
                    None,
                    None,
                    true,
                )?;
                std::fs::write(path, png)?;
            }
            // Kiosk mode hides the bar.
            tab.navigate_to(&format!("{base}/view#kiosk"))?
                .wait_until_navigated()?;
            waits("document.getElementById('picture').naturalWidth", 1.0)?;
            anyhow::ensure!(
                text("getComputedStyle(document.querySelector('header')).display")? == "none",
                "kiosk mode left the bar"
            );
            Ok(())
        })
        .await
        .unwrap();

        outcome.unwrap();
    }
}
