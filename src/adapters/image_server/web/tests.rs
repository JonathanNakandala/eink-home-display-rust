//! Tests of the page and its API.

use chrono::{NaiveTime, TimeZone};
use chrono_tz::Europe::London;

use super::super::testing::{FIRST, publish, publish_layout, start_web, start_with_two_layouts};
use super::pwa::inline_block;
use super::*;
use crate::adapters::clock::{FixedClock, SystemClock};
use crate::domain::models::layout::{LayoutRule, LayoutSchedule, Layouts};
use crate::domain::models::viewer::{ExchangeCode, RefreshToken, ViewerToken};

fn two_layouts() -> Layouts {
    Layouts::new(vec![
        Layout::new(FIRST, "Everything"),
        Layout::new("departures", "Just the trains"),
    ])
    .unwrap()
}

/// The page open to anyone, showing every layout.
fn open_all() -> WebSettings {
    WebSettings {
        layouts: vec![],
        open: true,
        stream: StreamTiming::default(),
        trusted_proxies: Vec::new(),
    }
}

fn only(names: &[&str]) -> WebSettings {
    WebSettings {
        open: true,
        layouts: names
            .iter()
            .map(|n| LayoutName::parse(n).unwrap())
            .collect(),
        stream: StreamTiming::default(),
        trusted_proxies: Vec::new(),
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
    let base = serve(tmp.path(), &open_all()).await;

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
        serve(tmp.path(), &open_all()).await
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
        allowed_layouts(&open_all(), &two_layouts()).unwrap().len(),
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
        &open_all(),
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
        &open_all(),
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
    let base = serve(tmp.path(), &open_all()).await;
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
    let base = serve(tmp.path(), &open_all()).await;
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
    let base = serve(tmp.path(), &open_all()).await;
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
    let base = serve(tmp.path(), &open_all()).await;
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
            text("document.querySelectorAll('#layouts button')[2].getAttribute('aria-pressed')")?
                == "true",
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

// --- who may look: tokens ---------------------------------------------------------------------------------

use super::super::testing::start_web_for;
use crate::adapters::entropy::OsEntropy;
use crate::adapters::viewer_registry::FileViewerRegistry;
use crate::application::viewers::IssueRequest;

fn needing_a_token() -> WebSettings {
    WebSettings {
        layouts: vec![],
        open: false,
        stream: StreamTiming::default(),
        trusted_proxies: Vec::new(),
    }
}

/// A server that wants a viewer token, with the viewers behind it and the clock it reads, over some pictures.
struct Guarded {
    base: String,
    viewers: Arc<Viewers>,
    clock: Arc<FixedClock>,
    handles: crate::adapters::image_server::Handles,
    layouts: Layouts,
    _directory: tempfile::TempDir,
    _viewers: tempfile::TempDir,
}

async fn guarded_by(peer: Option<std::net::SocketAddr>, settings: &WebSettings) -> Guarded {
    guarded_for(peer, settings, None).await
}

/// As `guarded_by`, with the access tokens of refreshing viewers lasting `access` if given.
async fn guarded_for(
    peer: Option<std::net::SocketAddr>,
    settings: &WebSettings,
    access: Option<chrono::Duration>,
) -> Guarded {
    let directory = tempfile::tempdir().unwrap();
    publish_both(directory.path(), FIRST, b"all of it").await;
    publish_both(directory.path(), "departures", b"trains").await;
    let kept = tempfile::tempdir().unwrap();
    let clock = FixedClock::at(at(7, 0));
    let mut made = Viewers::new(
        Arc::new(FileViewerRegistry::open(kept.path()).await.unwrap()),
        clock.clone(),
        Arc::new(OsEntropy),
    );
    if let Some(access) = access {
        made = made.with_access_lifetime(access);
    }
    let viewers = Arc::new(made);
    let layouts = with_a_morning_rule();
    let (base, handles) = start_web_for(
        directory.path().to_path_buf(),
        layouts.clone(),
        settings,
        clock.clone(),
        Some(viewers.clone()),
        peer,
    )
    .await
    .unwrap();
    Guarded {
        base,
        viewers,
        clock,
        handles,
        layouts,
        _directory: directory,
        _viewers: kept,
    }
}

async fn guarded() -> Guarded {
    guarded_by(None, &needing_a_token()).await
}

impl Guarded {
    /// Makes a viewer and signs it in as a browser would, giving the token.
    async fn token(&self, layouts: &[&str]) -> String {
        let issued = self
            .viewers
            .issue(
                IssueRequest {
                    label: "tablet".into(),
                    layouts: layouts.iter().map(|l| (*l).to_owned()).collect(),
                    ..IssueRequest::default()
                },
                &self.layouts,
            )
            .await
            .unwrap();
        self.viewers
            .exchange(issued.code.expose())
            .await
            .unwrap()
            .token
            .expose()
            .to_owned()
    }

    async fn with(&self, path: &str, authorization: Option<&str>) -> reqwest::Response {
        let mut request = reqwest::Client::new().get(format!("{}{path}", self.base));
        if let Some(value) = authorization {
            request = request.header("authorization", value);
        }
        request.send().await.unwrap()
    }

    async fn bearing(&self, path: &str, token: &str) -> reqwest::Response {
        self.with(path, Some(&format!("Bearer {token}"))).await
    }

    async fn post(&self, path: &str, content_type: &str, body: String) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}{path}", self.base))
            .header("content-type", content_type)
            .body(body)
            .send()
            .await
            .unwrap()
    }

    /// Signs in a refreshing viewer over HTTP, as a browser does: its access token and its refresh token.
    async fn pair(&self) -> (String, String) {
        let issued = self
            .viewers
            .issue(
                IssueRequest {
                    label: "refreshing tablet".into(),
                    refreshing: true,
                    ..IssueRequest::default()
                },
                &self.layouts,
            )
            .await
            .unwrap();
        let body: serde_json::Value = self
            .exchange(issued.code.expose())
            .await
            .json()
            .await
            .unwrap();
        (
            body["access_token"].as_str().unwrap().to_owned(),
            body["refresh_token"].as_str().unwrap().to_owned(),
        )
    }

    async fn refresh(&self, refresh_token: &str) -> reqwest::Response {
        self.post(
            "/view/api/token",
            "application/x-www-form-urlencoded",
            format!("grant_type=refresh_token&refresh_token={refresh_token}"),
        )
        .await
    }

    async fn exchange(&self, code: &str) -> reqwest::Response {
        self.post(
            "/view/api/token",
            "application/x-www-form-urlencoded",
            format!("grant_type={GRANT_TYPE}&pre-authorized_code={code}"),
        )
        .await
    }
}

fn challenge_of(response: &reqwest::Response) -> String {
    response.headers()[header::WWW_AUTHENTICATE.as_str()]
        .to_str()
        .unwrap()
        .to_owned()
}

const PROTECTED: [&str; 3] = ["/view/api/layouts", "/view/api/state", "/view/api/image"];

#[tokio::test]
async fn without_a_token_the_page_is_served_and_the_rest_asks_for_one_without_naming_an_error() {
    let g = guarded().await;

    // The page holds nothing private, so it is there to ask from.
    assert_eq!(g.with("/view", None).await.status(), 200);
    for path in PROTECTED {
        let response = g.with(path, None).await;
        assert_eq!(response.status(), 401, "{path}");
        // RFC 6750 section 3.1: no authentication information, so no error code.
        assert_eq!(
            challenge_of(&response),
            "Bearer realm=\"home-display\"",
            "{path}"
        );
        assert!(!response.text().await.unwrap().contains("all of it"));
    }
    // Another scheme is no authentication of ours either.
    let basic = g.with("/view/api/state", Some("Basic dXNlcjpwYXNz")).await;
    assert_eq!(basic.status(), 401);
    assert_eq!(challenge_of(&basic), "Bearer realm=\"home-display\"");
}

#[tokio::test]
async fn a_token_that_is_not_good_is_invalid_token_and_a_header_that_is_not_a_bearer_one_is_invalid_request()
 {
    let g = guarded().await;
    let good = g.token(&[]).await;

    for bad in [
        ViewerToken::generate([1; 32]).expose().to_owned(),
        "hdv1_short".to_owned(),
        format!("{good}x"),
        "x".repeat(300),
    ] {
        let response = g.bearing("/view/api/state", &bad).await;
        assert_eq!(response.status(), 401, "{bad}");
        assert_eq!(
            challenge_of(&response),
            "Bearer realm=\"home-display\", error=\"invalid_token\""
        );
    }
    for malformed in ["Bearer", "Bearer ", "Bearer a b"] {
        let response = g.with("/view/api/state", Some(malformed)).await;
        assert_eq!(response.status(), 400, "{malformed:?}");
        assert!(challenge_of(&response).contains("error=\"invalid_request\""));
    }
    // The scheme is not case sensitive (RFC 9110), and a good token works.
    let lower = g
        .with("/view/api/state", Some(&format!("bearer {good}")))
        .await;
    assert_eq!(lower.status(), 200);
}

#[tokio::test]
async fn the_code_is_traded_for_a_token_in_the_shape_oauth_gives_one_and_the_token_works() {
    let g = guarded().await;
    let issued = g
        .viewers
        .issue(
            IssueRequest {
                label: "tablet".into(),
                ..IssueRequest::default()
            },
            &g.layouts,
        )
        .await
        .unwrap();

    let response = g.exchange(issued.code.expose()).await;

    assert_eq!(response.status(), 200);
    // RFC 6749 section 5.1: nothing of this may be kept.
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["pragma"], "no-cache");
    assert_eq!(response.headers()["content-type"], "application/json");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["scope"], "layout:*");
    assert_eq!(body["expires_in"], 90 * 24 * 3600);
    let token = body["access_token"].as_str().unwrap();
    assert!(token.starts_with("hdv1_"));

    for path in PROTECTED {
        let used = g.bearing(path, token).await;
        assert_eq!(used.status(), 200, "{path}");
        // What is answered depends on who asks, which a cache must know.
        assert_eq!(used.headers()["vary"], "Authorization", "{path}");
    }
    assert_eq!(
        g.bearing("/view/api/image", token)
            .await
            .bytes()
            .await
            .unwrap()
            .as_ref(),
        b"trains"
    );
}

#[tokio::test]
async fn a_code_works_once_and_what_is_wrong_with_a_request_is_said_as_oauth_says_it() {
    let g = guarded().await;
    let issued = g
        .viewers
        .issue(
            IssueRequest {
                label: "tablet".into(),
                ..IssueRequest::default()
            },
            &g.layouts,
        )
        .await
        .unwrap();
    let code = issued.code.expose().to_owned();
    assert_eq!(g.exchange(&code).await.status(), 200);

    let error = |response: reqwest::Response, status: u16, name: &'static str| async move {
        assert_eq!(response.status(), status, "{name}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"], name);
        assert!(body["error_description"].as_str().unwrap().len() > 5);
    };
    // The same code again, one that never was, and nonsense: all the same, so which codes there are can't be found out.
    error(g.exchange(&code).await, 400, "invalid_grant").await;
    error(
        g.exchange(ExchangeCode::generate([9; 32]).expose()).await,
        400,
        "invalid_grant",
    )
    .await;
    error(g.exchange("hdc1_nope").await, 400, "invalid_grant").await;
    // The request itself is wrong.
    error(
        g.post(
            "/view/api/token",
            "application/x-www-form-urlencoded",
            format!("grant_type=password&pre-authorized_code={code}"),
        )
        .await,
        400,
        "unsupported_grant_type",
    )
    .await;
    error(
        g.post(
            "/view/api/token",
            "application/x-www-form-urlencoded",
            "pre-authorized_code=x".into(),
        )
        .await,
        400,
        "invalid_request",
    )
    .await;
    error(
        g.post(
            "/view/api/token",
            "application/x-www-form-urlencoded",
            format!("grant_type={GRANT_TYPE}"),
        )
        .await,
        400,
        "invalid_request",
    )
    .await;
    error(
        g.post(
            "/view/api/token",
            "application/json",
            format!("{{\"pre-authorized_code\":\"{code}\"}}"),
        )
        .await,
        400,
        "invalid_request",
    )
    .await;
    // A request is a couple of short fields, so a large one is refused unread.
    let big = g
        .post(
            "/view/api/token",
            "application/x-www-form-urlencoded",
            "x".repeat(10_000),
        )
        .await;
    assert_eq!(big.status(), 413);
    // And only a POST is a token request.
    assert_eq!(g.with("/view/api/token", None).await.status(), 405);
}

#[tokio::test]
async fn a_token_for_some_layouts_sees_only_those_and_is_told_so_for_the_rest() {
    let g = guarded().await;
    let token = g.token(&["departures"]).await;

    // The listing has only what it may see, and `auto` stays within it (at 07:00 the schedule says departures anyway).
    let list: serde_json::Value = g
        .bearing("/view/api/layouts", &token)
        .await
        .json()
        .await
        .unwrap();
    let names: Vec<_> = list["layouts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["departures"]);
    // Out of the schedule's window `auto` would be the dashboard; for this viewer it is what it may see.
    g.clock.set(at(12, 0));
    let state: serde_json::Value = g
        .bearing("/view/api/state", &token)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(state["layout"], "departures");

    // A layout there is but the token does not allow: 403 and `insufficient_scope` naming what would be needed.
    for path in [
        "/view/api/state?layout=dashboard",
        "/view/api/image?layout=dashboard",
    ] {
        let refused = g.bearing(path, &token).await;
        assert_eq!(refused.status(), 403, "{path}");
        assert_eq!(
            challenge_of(&refused),
            "Bearer realm=\"home-display\", error=\"insufficient_scope\", scope=\"layout:dashboard\""
        );
    }
    // One there is not is not found, whatever the token.
    assert_eq!(
        g.bearing("/view/api/state?layout=nonesuch", &token)
            .await
            .status(),
        404
    );
    // And the one it may see is served.
    assert_eq!(
        g.bearing("/view/api/state?layout=departures", &token)
            .await
            .status(),
        200
    );
    // A token for everything sees everything.
    let all = g.token(&[]).await;
    assert_eq!(
        g.bearing("/view/api/state?layout=dashboard", &all)
            .await
            .status(),
        200
    );
}

#[tokio::test]
async fn the_settings_still_limit_what_a_token_for_everything_may_see() {
    let g = guarded_by(
        None,
        &WebSettings {
            layouts: vec![LayoutName::parse("departures").unwrap()],
            open: false,
            stream: StreamTiming::default(),
            trusted_proxies: Vec::new(),
        },
    )
    .await;
    let token = g.token(&[]).await;

    let list: serde_json::Value = g
        .bearing("/view/api/layouts", &token)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(list["layouts"].as_array().unwrap().len(), 1);
    // The page may not show it, so to this viewer it is not there.
    assert_eq!(
        g.bearing("/view/api/state?layout=dashboard", &token)
            .await
            .status(),
        404
    );
    // A token for a layout the page may not show allows nothing the page can give.
    let useless = g.token(&["dashboard"]).await;
    let refused = g.bearing("/view/api/layouts", &useless).await;
    assert_eq!(refused.status(), 403);
    assert!(challenge_of(&refused).contains("insufficient_scope"));
}

#[tokio::test]
async fn a_token_stops_at_once_when_revoked_when_given_up_and_when_it_runs_out() {
    let g = guarded().await;
    let (revoked, given_up, runs_out) =
        (g.token(&[]).await, g.token(&[]).await, g.token(&[]).await);
    let ok = |token: String| {
        let g = &g;
        async move { g.bearing("/view/api/state", &token).await.status() }
    };

    // The owner revokes one, by its id.
    let id = g.viewers.authenticate(&revoked).await.unwrap().viewer;
    assert_eq!(ok(revoked.clone()).await, 200);
    g.viewers.revoke(&id).await.unwrap();
    assert_eq!(ok(revoked).await, 401);

    // The holder gives one up (RFC 7009); asking about others, or nonsense, is the same quiet success.
    assert_eq!(ok(given_up.clone()).await, 200);
    for token in [
        given_up.clone(),
        given_up.clone(),
        "hdv1_unknown".to_owned(),
        "garbage".to_owned(),
    ] {
        let response = g
            .post(
                "/view/api/revoke",
                "application/x-www-form-urlencoded",
                format!("token={token}"),
            )
            .await;
        assert_eq!(response.status(), 200, "{token}");
    }
    assert_eq!(ok(given_up).await, 401);
    assert_eq!(
        ok(runs_out.clone()).await,
        200,
        "and the other is untouched"
    );

    // It runs out on the day.
    g.clock.set(at(7, 0) + chrono::Duration::days(90));
    assert_eq!(ok(runs_out).await, 401);
    // Giving up needs a form.
    assert_eq!(
        g.post("/view/api/revoke", "application/json", "{}".into())
            .await
            .status(),
        400
    );
}

#[tokio::test]
async fn the_page_for_a_token_records_nobody_among_the_displays() {
    let g = guarded().await;
    let token = g.token(&[]).await;

    g.bearing("/view/api/state?device=kitchen", &token).await;

    assert!(
        g.handles
            .devices
            .snapshot(chrono::Utc::now().with_timezone(&London))
            .is_empty()
    );
}

#[tokio::test]
async fn a_source_that_keeps_showing_bad_tokens_is_slowed_down_and_a_good_one_clears_it() {
    let peer = Some("203.0.113.9:5000".parse().unwrap());
    let g = guarded_by(peer, &needing_a_token()).await;
    let good = g.token(&[]).await;

    for _ in 0..10 {
        assert_eq!(
            g.bearing("/view/api/state", "hdv1_wrong").await.status(),
            401
        );
    }

    // Turned away before the token is looked at, whichever it is, and told how long (RFC 6585).
    for token in ["hdv1_wrong", good.as_str()] {
        let response = g.bearing("/view/api/state", token).await;
        assert_eq!(response.status(), 429);
        let wait: u64 = response.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=600).contains(&wait), "{wait}");
    }
    // A wrong code counts the same, and is turned away the same.
    let code = g.exchange(ExchangeCode::generate([4; 32]).expose()).await;
    assert_eq!(code.status(), 429);
    let body: serde_json::Value = code.json().await.unwrap();
    assert_eq!(body["error"], "temporarily_unavailable");
}

#[tokio::test]
async fn bad_codes_count_against_the_source_too_so_a_script_trying_them_is_stopped() {
    let peer = Some("203.0.113.10:5000".parse().unwrap());
    let g = guarded_by(peer, &needing_a_token()).await;

    for i in 0..10u8 {
        assert_eq!(
            g.exchange(ExchangeCode::generate([i; 32]).expose())
                .await
                .status(),
            400
        );
    }

    assert_eq!(
        g.exchange(ExchangeCode::generate([99; 32]).expose())
            .await
            .status(),
        429
    );
    assert_eq!(g.bearing("/view/api/state", "hdv1_x").await.status(), 429);
}

#[tokio::test]
async fn a_good_request_forgives_what_the_source_got_wrong_before() {
    let peer = Some("203.0.113.11:5000".parse().unwrap());
    let g = guarded_by(peer, &needing_a_token()).await;
    let good = g.token(&[]).await;

    for _ in 0..9 {
        g.bearing("/view/api/state", "hdv1_wrong").await;
    }
    assert_eq!(g.bearing("/view/api/state", &good).await.status(), 200);
    // Nine more are allowed again: the count started over.
    for _ in 0..9 {
        assert_eq!(
            g.bearing("/view/api/state", "hdv1_wrong").await.status(),
            401
        );
    }
    assert_eq!(g.bearing("/view/api/state", &good).await.status(), 200);
}

#[tokio::test]
async fn open_has_no_token_routes_and_ignores_a_token() {
    let tmp = tempfile::tempdir().unwrap();
    let base = serve(tmp.path(), &open_all()).await;
    publish_both(tmp.path(), FIRST, b"a").await;

    // Nothing to trade a code for, or to give up.
    for path in ["/view/api/token", "/view/api/revoke"] {
        let response = reqwest::Client::new()
            .post(format!("{base}{path}"))
            .header("content-type", "application/x-www-form-urlencoded")
            .body("token=x")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404, "{path}");
    }
    // And what is shown to anyone is shown to one with a (wrong) token, with no gate to refuse it.
    let with_one = reqwest::Client::new()
        .get(format!("{base}/view/api/state"))
        .header("authorization", "Bearer hdv1_whatever")
        .send()
        .await
        .unwrap();
    assert_eq!(with_one.status(), 200);
}

#[test]
fn wanting_tokens_with_nowhere_to_keep_the_viewers_is_an_error_that_says_so() {
    let error = Access::of(&needing_a_token(), None).err().unwrap();
    assert!(
        error.contains("HTTPS") && error.contains("admin"),
        "{error}"
    );
    assert!(matches!(Access::of(&open_all(), None), Ok(Access::Open)));
}

#[tokio::test]
async fn the_description_names_the_bearer_scheme_and_what_each_protected_route_answers() {
    let (_, api) = routes().split_for_parts();
    let json = serde_json::to_value(&api).unwrap();

    let scheme = &json["components"]["securitySchemes"]["bearerAuth"];
    assert_eq!(scheme["type"], "http");
    assert_eq!(scheme["scheme"], "bearer");
    for path in PROTECTED {
        let get = &json["paths"][path]["get"];
        // Optional: none is needed when the page is open.
        let security = get["security"].as_array().unwrap();
        assert!(
            security.iter().any(|s| s.as_object().unwrap().is_empty()),
            "{path}"
        );
        assert!(
            security.iter().any(|s| s.get("bearerAuth").is_some()),
            "{path}"
        );
        assert!(get["responses"].get("401").is_some(), "{path}");
    }
    assert!(
        json["paths"]["/view/api/token"]["post"]["requestBody"]["content"]
            .get("application/x-www-form-urlencoded")
            .is_some()
    );
}

/// Needs a real Chrome, so run with `--ignored`. The whole of signing in as a browser does it: the link's code in the
/// fragment is traded for a token, the token is kept and the code is gone from the address, a reload needs no link, a
/// revoked token ends it, and a link used twice says so.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn signing_in_from_a_link_works_in_a_browser_under_its_own_policy() {
    let g = guarded().await;
    publish_picture(g._directory.path(), FIRST, 40).await;
    publish_picture(g._directory.path(), "departures", 20).await;
    let issue = |label: &'static str| {
        let (viewers, layouts) = (g.viewers.clone(), g.layouts.clone());
        async move {
            viewers
                .issue(
                    IssueRequest {
                        label: label.into(),
                        ..IssueRequest::default()
                    },
                    &layouts,
                )
                .await
                .unwrap()
        }
    };
    let first = issue("first").await;
    let second = issue("second").await;
    let (base, viewers) = (g.base.clone(), g.viewers.clone());
    let (first_link, second_code) = (
        format!("{base}/view#code={}", first.code.expose()),
        second.code.expose().to_owned(),
    );
    let first_id = first.viewer.id.clone();

    let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let browser = headless_chrome::Browser::new(
            headless_chrome::LaunchOptions::default_builder()
                .headless(true)
                .path(headless_chrome::browser::default_executable().ok())
                .build()?,
        )?;
        let tab = browser.new_tab()?;
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
        let waits = |script: &str, wanted: f64| -> anyhow::Result<()> {
            for _ in 0..100 {
                if number(script)? >= wanted {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            anyhow::bail!("{script} never reached {wanted}")
        };
        // As a link opened in a new tab is: a whole load, not a change of fragment on the page already there (which
        // does not finish loading, and which the page handles on its own).
        let open = |url: &str| -> anyhow::Result<()> {
            tab.navigate_to(&format!("{base}/view?away"))
                .and_then(|tab| tab.wait_until_navigated())
                .map_err(|e| anyhow::anyhow!("opening the page to leave from: {e}"))?;
            tab.navigate_to(url)
                .and_then(|tab| tab.wait_until_navigated())
                .map_err(|e| anyhow::anyhow!("opening {url}: {e}"))?;
            Ok(())
        };
        let picture_width = "document.getElementById('picture').naturalWidth";
        let panel_shown = "document.getElementById('signin').hidden ? 0 : 1";

        // Without a link, there is nothing to look at and the page says what to do.
        open(&format!("{base}/view"))?;
        waits(panel_shown, 1.0)?;
        anyhow::ensure!(
            text("document.getElementById('signin-title').textContent")?.contains("needs a link"),
            "no sign-in prompt"
        );

        // The link signs in: the picture the schedule says (departures at seven) is there, the token is kept, and the
        // code is gone from the address.
        open(&first_link)?;
        waits(picture_width, 20.0)?;
        anyhow::ensure!(
            !text("location.hash")?.contains("code="),
            "the code is still in the address"
        );
        anyhow::ensure!(
            text("localStorage.getItem('hd.token')")?.starts_with("hdv1_"),
            "no token kept"
        );
        anyhow::ensure!(number(panel_shown)? == 0.0, "the prompt stayed");

        // A reload with no link needs none.
        open(&format!("{base}/view"))?;
        waits(picture_width, 20.0)?;

        // The same link again does not work, and says so, in a browser with no token.
        tab.evaluate("localStorage.clear()", false)?;
        open(&first_link)?;
        waits(panel_shown, 1.0)?;
        anyhow::ensure!(
            text("document.getElementById('signin-title').textContent")?.contains("does not work"),
            "a used link was accepted"
        );

        // Another viewer signs in, then is revoked by the owner: the next look ends it.
        open(&format!("{base}/view#code={second_code}"))?;
        waits(picture_width, 20.0)?;
        let who = tokio::runtime::Handle::current();
        let all = who.block_on(viewers.list())?;
        let second_id = all
            .iter()
            .find(|v| v.label.as_str() == "second")
            .unwrap()
            .id
            .clone();
        who.block_on(viewers.revoke(&second_id))?;
        // Straight to a new load of the page (leaving by a page of its own would use the token, and see it refused,
        // before the load being checked).
        tab.navigate_to(&format!("{base}/view?again"))?
            .wait_until_navigated()?;
        waits(panel_shown, 1.0)?;
        let title = text("document.getElementById('signin-title').textContent")?;
        anyhow::ensure!(
            title.contains("access has ended"),
            "a revoked token was accepted: the page says {title:?}"
        );
        anyhow::ensure!(
            text("localStorage.getItem('hd.token')")?.is_empty(),
            "the token was kept"
        );
        let _ = first_id;
        Ok(())
    })
    .await
    .unwrap();

    outcome.unwrap();
}

#[tokio::test]
async fn a_refreshing_viewer_is_given_a_refresh_token_and_a_plain_one_is_not() {
    let g = guarded().await;

    let (access, refresh) = g.pair().await;
    assert!(refresh.starts_with("hdr1_"));
    let issued = g
        .viewers
        .issue(
            IssueRequest {
                label: "plain".into(),
                ..IssueRequest::default()
            },
            &g.layouts,
        )
        .await
        .unwrap();
    let plain: serde_json::Value = g.exchange(issued.code.expose()).await.json().await.unwrap();

    assert!(plain.get("refresh_token").is_none(), "{plain}");
    assert_eq!(plain["expires_in"], 90 * 24 * 3600);
    // Its access token works as one does.
    assert_eq!(g.bearing("/view/api/state", &access).await.status(), 200);
}

#[tokio::test]
async fn a_refresh_token_is_traded_for_a_new_pair_in_the_shape_oauth_gives_one_and_the_old_pair_stops()
 {
    let g = guarded().await;
    let (access, refresh) = g.pair().await;

    let response = g.refresh(&refresh).await;

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["pragma"], "no-cache");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["scope"], "layout:*");
    assert_eq!(body["expires_in"], 3600);
    let (new_access, new_refresh) = (
        body["access_token"].as_str().unwrap(),
        body["refresh_token"].as_str().unwrap(),
    );
    assert!(new_access != access && new_refresh != refresh);
    // The new pair works and the old access token does not.
    assert_eq!(g.bearing("/view/api/state", new_access).await.status(), 200);
    assert_eq!(g.bearing("/view/api/state", &access).await.status(), 401);
    // A refresh token is not an access token.
    let as_access = g.bearing("/view/api/state", new_refresh).await;
    assert_eq!(as_access.status(), 401);
    assert!(challenge_of(&as_access).contains("invalid_token"));
    // And it goes on from the new one.
    assert_eq!(g.refresh(new_refresh).await.status(), 200);
}

#[tokio::test]
async fn a_refresh_request_that_is_wrong_is_answered_as_oauth_answers_it() {
    let g = guarded().await;
    let (_, refresh) = g.pair().await;
    let error = |response: reqwest::Response, name: &'static str| async move {
        assert_eq!(response.status(), 400, "{name}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"], name);
    };

    // No token with it, a made-up one, a malformed one, and one of another kind: the last three are the same.
    error(
        g.post(
            "/view/api/token",
            "application/x-www-form-urlencoded",
            "grant_type=refresh_token".into(),
        )
        .await,
        "invalid_request",
    )
    .await;
    for bad in [
        RefreshToken::generate([5; 32]).expose().to_owned(),
        "hdr1_short".to_owned(),
        ViewerToken::generate([5; 32]).expose().to_owned(),
    ] {
        error(g.refresh(&bad).await, "invalid_grant").await;
    }
    // A code is not a refresh token, nor the reverse.
    error(
        g.refresh(ExchangeCode::generate([5; 32]).expose()).await,
        "invalid_grant",
    )
    .await;
    error(
        g.post(
            "/view/api/token",
            "application/x-www-form-urlencoded",
            format!("grant_type={GRANT_TYPE}&pre-authorized_code={refresh}"),
        )
        .await,
        "invalid_grant",
    )
    .await;
    // The one issued is untouched by all of that.
    assert_eq!(g.refresh(&refresh).await.status(), 200);
}

#[tokio::test]
async fn a_refresh_token_used_again_after_it_was_replaced_ends_the_session_for_whoever_holds_it() {
    let g = guarded().await;
    let (_, first) = g.pair().await;
    let second: serde_json::Value = g.refresh(&first).await.json().await.unwrap();
    let (second_access, second_refresh) = (
        second["access_token"].as_str().unwrap().to_owned(),
        second["refresh_token"].as_str().unwrap().to_owned(),
    );
    assert_eq!(
        g.bearing("/view/api/state", &second_access).await.status(),
        200
    );

    // A copy of the first one is shown, long after: refused, and the session is over for the real holder too.
    g.clock.set(at(7, 5));
    let copied = g.refresh(&first).await;
    assert_eq!(copied.status(), 400);
    assert_eq!(
        copied.json::<serde_json::Value>().await.unwrap()["error"],
        "invalid_grant"
    );
    assert_eq!(
        g.bearing("/view/api/state", &second_access).await.status(),
        401
    );
    assert_eq!(g.refresh(&second_refresh).await.status(), 400);
}

#[tokio::test]
async fn a_refresh_token_that_is_not_good_counts_against_the_source_like_a_bad_code() {
    let peer = Some("203.0.113.12:5000".parse().unwrap());
    let g = guarded_by(peer, &needing_a_token()).await;

    for i in 0..10u8 {
        assert_eq!(
            g.refresh(RefreshToken::generate([i; 32]).expose())
                .await
                .status(),
            400
        );
    }

    assert_eq!(
        g.refresh(RefreshToken::generate([99; 32]).expose())
            .await
            .status(),
        429
    );
}

#[tokio::test]
async fn the_description_has_the_refresh_token_in_a_token_response_and_the_grant_in_a_request() {
    let (_, api) = routes().split_for_parts();
    let json = serde_json::to_value(&api).unwrap();

    let response = &json["components"]["schemas"]["TokenResponse"]["properties"];
    assert!(response.get("refresh_token").is_some());
    let request = &json["components"]["schemas"]["TokenRequest"]["properties"];
    assert!(request.get("refresh_token").is_some());
    assert!(request.get("grant_type").is_some());
}

/// Needs a real Chrome, so run with `--ignored`. A viewer whose access tokens last two seconds: the page renews its
/// token with the refresh token by itself, keeps showing the picture, and has a new refresh token each time; two tabs
/// asking at once between them renew once, and a viewer revoked while it is signed in is out at its next look.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_refreshing_viewers_page_renews_its_token_by_itself_in_a_browser() {
    let g = guarded_for(None, &needing_a_token(), Some(chrono::Duration::seconds(2))).await;
    publish_picture(g._directory.path(), FIRST, 40).await;
    publish_picture(g._directory.path(), "departures", 20).await;
    let issued = g
        .viewers
        .issue(
            IssueRequest {
                label: "wall tablet".into(),
                refreshing: true,
                ..IssueRequest::default()
            },
            &g.layouts,
        )
        .await
        .unwrap();
    let (base, viewers) = (g.base.clone(), g.viewers.clone());
    let link = format!("{base}/view#code={}", issued.code.expose());
    let id = issued.viewer.id.clone();

    let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let browser = headless_chrome::Browser::new(
            headless_chrome::LaunchOptions::default_builder()
                .headless(true)
                .path(headless_chrome::browser::default_executable().ok())
                .build()?,
        )?;
        let tab = browser.new_tab()?;
        let text = |script: &str| -> anyhow::Result<String> {
            Ok(tab
                .evaluate(script, false)?
                .value
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default())
        };
        let number = |script: &str| -> anyhow::Result<f64> {
            Ok(tab
                .evaluate(script, false)?
                .value
                .and_then(|v| v.as_f64())
                .unwrap_or(-1.0))
        };
        let waits = |script: &str, wanted: f64| -> anyhow::Result<()> {
            for _ in 0..100 {
                if number(script)? >= wanted {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            anyhow::bail!("{script} never reached {wanted}")
        };
        let picture_width = "document.getElementById('picture').naturalWidth";
        let look_again = "document.dispatchEvent(new Event('visibilitychange'))";

        tab.navigate_to(&format!("{base}/view?away"))?
            .wait_until_navigated()?;
        tab.navigate_to(&link)?.wait_until_navigated()?;
        waits(picture_width, 20.0)?;
        let refresh_at_first = text("localStorage.getItem('hd.refresh')")?;
        anyhow::ensure!(
            refresh_at_first.starts_with("hdr1_"),
            "no refresh token kept"
        );
        let access_at_first = text("localStorage.getItem('hd.token')")?;

        // The access token runs out; looking again renews it, and the picture is still there, with no sign-in prompt.
        std::thread::sleep(std::time::Duration::from_millis(2500));
        tab.evaluate(look_again, false)?;
        for _ in 0..100 {
            if text("localStorage.getItem('hd.refresh')")? != refresh_at_first {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        anyhow::ensure!(
            text("localStorage.getItem('hd.refresh')")? != refresh_at_first,
            "the refresh token was not replaced"
        );
        anyhow::ensure!(
            text("localStorage.getItem('hd.token')")? != access_at_first,
            "the access token was not replaced"
        );
        anyhow::ensure!(
            number("document.getElementById('signin').hidden ? 0 : 1")? == 0.0,
            "the prompt came up"
        );
        anyhow::ensure!(number(picture_width)? == 20.0, "the picture went");

        // Many looks at once (as many tabs would make) renew once between them and none is taken for a copy.
        std::thread::sleep(std::time::Duration::from_millis(2500));
        tab.evaluate(
            &format!("for (var i = 0; i < 5; i++) {{ {look_again}; }}"),
            false,
        )?;
        std::thread::sleep(std::time::Duration::from_millis(1500));
        anyhow::ensure!(
            number("document.getElementById('signin').hidden ? 0 : 1")? == 0.0,
            "a burst of looks ended the session"
        );
        anyhow::ensure!(
            number(picture_width)? == 20.0,
            "the picture went after a burst"
        );

        // The owner revokes the viewer: at its next look the page says its access has ended.
        let handle = tokio::runtime::Handle::current();
        handle.block_on(viewers.revoke(&id))?;
        std::thread::sleep(std::time::Duration::from_millis(2500));
        tab.evaluate(look_again, false)?;
        waits("document.getElementById('signin').hidden ? 0 : 1", 1.0)?;
        let title = text("document.getElementById('signin-title').textContent")?;
        anyhow::ensure!(
            title.contains("access has ended"),
            "after a revoke the page says {title:?}"
        );
        anyhow::ensure!(
            text("localStorage.getItem('hd.refresh')")?.is_empty(),
            "a refresh token was kept"
        );
        Ok(())
    })
    .await
    .unwrap();

    outcome.unwrap();
}

// --- for a screen on a wall ----------------------------------------------------------------------------

#[tokio::test]
async fn the_page_looks_again_a_moment_after_a_picture_is_due_not_half_a_minute() {
    let tmp = tempfile::tempdir().unwrap();
    let base = serve(tmp.path(), &open_all()).await;
    publish_both(tmp.path(), FIRST, b"a").await;
    // An hourly schedule, the picture a minute old: it is due in 59 minutes, and the page is to come back two seconds after,
    // where a display is told to come thirty (its radio, its panel).
    crate::adapters::image_server::testing::set_age(tmp.path(), 60);

    let state = json(format!("{base}/view/api/state")).await;

    let next = state["next_seconds"].as_u64().unwrap();
    assert!((3535..=3545).contains(&next), "{next}");
}

#[tokio::test]
async fn while_a_render_is_due_but_not_there_the_page_is_told_to_look_again_in_a_couple_of_seconds()
{
    let tmp = tempfile::tempdir().unwrap();
    let base = serve(tmp.path(), &open_all()).await;
    publish_both(tmp.path(), FIRST, b"a").await;
    // An hour and a few seconds on from the render, so the next is due.
    crate::adapters::image_server::testing::set_age(tmp.path(), 3600 + 5);

    let state = json(format!("{base}/view/api/state")).await;

    assert_eq!(state["pending"], true);
    let next = state["next_seconds"].as_u64().unwrap();
    assert!((1..=3).contains(&next), "{next}");
}

#[tokio::test]
async fn on_auto_the_page_is_told_when_the_schedule_will_change_what_auto_means() {
    let tmp = tempfile::tempdir().unwrap();
    publish_both(tmp.path(), FIRST, b"all").await;
    publish_both(tmp.path(), "departures", b"trains").await;
    let clock = FixedClock::at(at(6, 0));
    let (base, _) = start_web(
        tmp.path().to_path_buf(),
        with_a_morning_rule(),
        &open_all(),
        clock.clone(),
    )
    .await
    .unwrap();
    let changes = |query: &'static str| {
        let url = format!("{base}/view/api/state{query}");
        async move { json(url).await["auto_changes_in_seconds"].clone() }
    };

    // At 06:00 the dashboard is the layout, and departures starts at 06:30: in half an hour, then at 09:30 it is back.
    assert_eq!(changes("").await, 30 * 60);
    assert_eq!(changes("?layout=auto").await, 30 * 60);
    clock.set(at(7, 0));
    assert_eq!(changes("").await, 2 * 3600 + 30 * 60);
    // Asked for a layout by name there is nothing to follow.
    assert!(changes("?layout=dashboard").await.is_null());
    // With no change in the week the field is left out.
    let still = start_web(tmp.path().to_path_buf(), two_layouts(), &open_all(), clock)
        .await
        .unwrap()
        .0;
    assert!(
        json(format!("{still}/view/api/state"))
            .await
            .get("auto_changes_in_seconds")
            .is_none()
    );
}

#[tokio::test]
async fn a_change_the_viewer_cannot_see_is_not_a_change_for_it() {
    let tmp = tempfile::tempdir().unwrap();
    publish_both(tmp.path(), FIRST, b"all").await;
    publish_both(tmp.path(), "departures", b"trains").await;
    let clock = FixedClock::at(at(6, 0));
    // This page may show only the dashboard, so the schedule's move to departures never changes what `auto` is for it.
    let (base, _) = start_web(
        tmp.path().to_path_buf(),
        with_a_morning_rule(),
        &only(&[FIRST]),
        clock,
    )
    .await
    .unwrap();

    let state = json(format!("{base}/view/api/state")).await;

    assert_eq!(state["layout"], FIRST);
    assert!(state.get("auto_changes_in_seconds").is_none(), "{state}");
}

#[tokio::test]
async fn the_page_can_be_put_on_a_home_screen_a_manifest_and_icons_for_anyone() {
    let g = guarded().await;

    // No token for any of it: nothing in it is private, and a browser fetches it without credentials.
    let manifest = g.with("/view/manifest.webmanifest", None).await;
    assert_eq!(manifest.status(), 200);
    assert_eq!(
        manifest.headers()["content-type"],
        "application/manifest+json"
    );
    let manifest: serde_json::Value = manifest.json().await.unwrap();
    assert_eq!(manifest["display"], "fullscreen");
    assert_eq!(manifest["start_url"], "/view#kiosk");
    assert_eq!(manifest["scope"], "/view");
    // Every icon it names is served, as what it says it is.
    let icons = manifest["icons"].as_array().unwrap();
    assert!(
        icons.iter().any(|i| i["sizes"] == "192x192")
            && icons.iter().any(|i| i["sizes"] == "512x512")
    );
    for icon in icons {
        let response = g.with(icon["src"].as_str().unwrap(), None).await;
        assert_eq!(response.status(), 200, "{icon}");
        assert_eq!(
            response.headers()["content-type"],
            icon["type"].as_str().unwrap()
        );
    }
    // The one an Apple device looks for, which a manifest does not name, and sizes there are not.
    assert_eq!(g.with("/view/icons/180.png", None).await.status(), 200);
    for missing in [
        "/view/icons/181.png",
        "/view/icons/192",
        "/view/icons/x.png",
    ] {
        assert_eq!(g.with(missing, None).await.status(), 404, "{missing}");
    }
    // The page points at them.
    let page = g.with("/view", None).await.text().await.unwrap();
    for link in [
        "rel=\"manifest\" href=\"/view/manifest.webmanifest\"",
        "rel=\"apple-touch-icon\" href=\"/view/icons/180.png\"",
        "rel=\"icon\" href=\"/view/icon.svg\"",
        "apple-mobile-web-app-capable",
    ] {
        assert!(page.contains(link), "{link}");
    }
}

#[tokio::test]
async fn the_policy_lets_the_manifest_icons_and_worker_load_and_nothing_more() {
    let tmp = tempfile::tempdir().unwrap();
    let base = serve(tmp.path(), &open_all()).await;

    let policy = reqwest::get(format!("{base}/view"))
        .await
        .unwrap()
        .headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();

    assert!(policy.contains("manifest-src 'self'"), "{policy}");
    assert!(policy.contains("img-src blob: 'self'"), "{policy}");
    // Still nothing from elsewhere and no running of inline scripts but its own by hash.
    assert!(
        policy.contains("default-src 'none'") && policy.contains("connect-src 'self'"),
        "{policy}"
    );
    assert!(policy.contains("worker-src 'self'"), "{policy}");
    assert!(
        !policy.contains("unsafe") && !policy.contains("script-src 'self'"),
        "{policy}"
    );
}

#[test]
fn the_page_can_keep_the_screen_on_and_be_locked_and_does_so_only_where_the_browser_can() {
    // The script: asks for a wake lock only if the browser has them, again whenever the page is shown, and lets a kiosk
    // start it on its own unless told otherwise.
    assert!(PAGE.contains("'wakeLock' in navigator"));
    assert!(PAGE.contains("navigator.wakeLock.request('screen')"));
    assert!(PAGE.contains("kiosk && remembered !== '0'"));
    // And `lock` stops layouts being changed by a touch.
    assert!(PAGE.contains("hash.has('lock')"));
    assert!(PAGE.contains("layouts.length < 2 || locked"));
    // The button starts hidden, so a browser with no wake lock never shows one that does nothing.
    assert!(
        PAGE.contains("id=\"awake\"") && PAGE.contains("title=\"Stop the screen from sleeping")
    );
    let button = &PAGE[PAGE.find("id=\"awake\"").unwrap()..];
    assert!(button[..button.find('>').unwrap()].contains("hidden"));
}

/// Needs a real Chrome, so run with `--ignored`. A page set up as a wall display: the bar is gone, a layout can't be changed
/// by key or touch, the screen is asked to stay on, and the manifest is there for the browser under the page's policy.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_locked_kiosk_page_keeps_its_layout_and_asks_for_the_screen_in_a_browser() {
    let tmp = tempfile::tempdir().unwrap();
    let base = serve(tmp.path(), &open_all()).await;
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
        let number = |script: &str| -> anyhow::Result<f64> {
            Ok(tab.evaluate(script, false)?.value.and_then(|v| v.as_f64()).unwrap_or(-1.0))
        };
        let text = |script: &str| -> anyhow::Result<String> {
            Ok(tab.evaluate(script, false)?.value.and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default())
        };
        let waits = |script: &str, wanted: f64| -> anyhow::Result<()> {
            for _ in 0..100 {
                if number(script)? >= wanted {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            anyhow::bail!("{script} never reached {wanted}")
        };
        let picture_width = "document.getElementById('picture').naturalWidth";

        tab.navigate_to(&format!("{base}/view?layout=dashboard#kiosk&lock"))?.wait_until_navigated()?;
        waits(picture_width, 40.0)?;

        // Locked: no buttons, and neither the arrow keys nor a swipe move it off its layout.
        anyhow::ensure!(number("document.querySelectorAll('#layouts button').length")? == 0.0, "buttons in a locked page");
        tab.evaluate(
            "document.dispatchEvent(new KeyboardEvent('keydown', {key: 'ArrowRight'}))",
            false,
        )?;
        std::thread::sleep(std::time::Duration::from_millis(800));
        anyhow::ensure!(number(picture_width)? == 40.0, "an arrow key changed a locked page");
        // The same page unlocked does change.
        tab.navigate_to(&format!("{base}/view?layout=dashboard&away=1#kiosk"))?.wait_until_navigated()?;
        waits(picture_width, 40.0)?;
        tab.evaluate("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'ArrowRight'}))", false)?;
        waits("document.getElementById('picture').naturalWidth == 20 ? 1 : 0", 1.0)?;

        // A kiosk asks for the screen to stay on, where the browser has wake locks (the button shows only then).
        if number("document.getElementById('awake').hidden ? 0 : 1")? == 1.0 {
            anyhow::ensure!(
                text("document.getElementById('awake').getAttribute('aria-pressed')")? == "true",
                "a kiosk did not ask for the screen"
            );
        }
        // The manifest is reachable by the page under its own policy, and says what it should.
        let manifest = tab
            .evaluate(
                "fetch(document.querySelector('link[rel=manifest]').href).then(r => r.json()).then(j => j.display + ' ' + j.start_url)",
                true,
            )?
            .value
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        anyhow::ensure!(manifest == "fullscreen /view#kiosk", "the manifest says {manifest:?}");
        Ok(())
    })
    .await
    .unwrap();

    outcome.unwrap();
}

// --- a stream of changes ---------------------------------------------------------------------------------

use crate::domain::services::render_observer::RenderObserver;

/// Settings with a stream that is quick about everything, so a test need not wait a minute.
fn quick(open: bool) -> WebSettings {
    WebSettings {
        layouts: vec![],
        open,
        stream: StreamTiming {
            lifetime: std::time::Duration::from_millis(1500),
            tick: std::time::Duration::from_millis(100),
            ..StreamTiming::default()
        },
        trusted_proxies: Vec::new(),
    }
}

/// Reads from `response` until `done` says the text so far is enough, or a few seconds pass.
async fn read_until(response: &mut reqwest::Response, done: impl Fn(&str) -> bool) -> String {
    let mut text = String::new();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !done(&text) {
            match response.chunk().await.unwrap() {
                Some(bytes) => text.push_str(&String::from_utf8_lossy(&bytes)),
                None => break,
            }
        }
    })
    .await;
    assert!(outcome.is_ok(), "timed out with {text:?}");
    text
}

fn state_events(text: &str) -> Vec<serde_json::Value> {
    text.split("\n\n")
        .filter_map(|block| {
            block
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .filter(|_| block.contains("event: state"))
        })
        .map(|data| serde_json::from_str(data).unwrap())
        .collect()
}

/// Finishes a render as the render loop would, which is what tells a stream to look.
fn render_finished(handles: &crate::adapters::image_server::Handles) {
    handles.refresh.render_succeeded(
        chrono::Utc::now().with_timezone(&London),
        &crate::domain::models::render_report::RenderReport::default(),
    );
}

#[tokio::test]
async fn a_stream_starts_with_the_state_and_says_what_it_is() {
    let tmp = tempfile::tempdir().unwrap();
    publish_both(tmp.path(), FIRST, b"a").await;
    let (base, _) = start_web(
        tmp.path().to_path_buf(),
        two_layouts(),
        &quick(true),
        Arc::new(SystemClock::new(London)),
    )
    .await
    .unwrap();

    let mut response = reqwest::get(format!("{base}/view/api/events"))
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let text = read_until(&mut response, |t| t.contains("event: state")).await;
    assert!(text.starts_with("retry: 2000\n\n"), "{text:?}");
    let first = &state_events(&text)[0];
    assert_eq!(first["layout"], FIRST);
    assert!(first["version"].as_u64().unwrap() > 0);
    assert_eq!(first["stale"], false);
}

#[tokio::test]
async fn a_new_picture_is_sent_as_soon_as_the_render_that_made_it_finishes() {
    let tmp = tempfile::tempdir().unwrap();
    publish_both(tmp.path(), FIRST, b"old").await;
    crate::adapters::image_server::testing::set_age(tmp.path(), 120);
    // A long tick, so what is seen here is the render's finishing and not a look on a timer.
    let settings = WebSettings {
        stream: StreamTiming {
            lifetime: std::time::Duration::from_secs(30),
            tick: std::time::Duration::from_secs(20),
            ..StreamTiming::default()
        },
        ..open_all()
    };
    let (base, handles) = start_web(
        tmp.path().to_path_buf(),
        two_layouts(),
        &settings,
        Arc::new(SystemClock::new(London)),
    )
    .await
    .unwrap();
    let mut response = reqwest::get(format!("{base}/view/api/events"))
        .await
        .unwrap();
    let text = read_until(&mut response, |t| t.contains("event: state")).await;
    let before = state_events(&text)[0]["version"].as_u64().unwrap();

    // A render ends having drawn a new picture.
    publish_both(tmp.path(), FIRST, b"new").await;
    let started = std::time::Instant::now();
    render_finished(&handles);

    let text = read_until(&mut response, |t| t.contains("event: state")).await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let after = state_events(&text)[0]["version"].as_u64().unwrap();
    assert!(after > before, "{after} after {before}");
}

#[tokio::test]
async fn nothing_changing_is_a_keepalive_and_not_an_event() {
    let tmp = tempfile::tempdir().unwrap();
    publish_both(tmp.path(), FIRST, b"a").await;
    let (base, _) = start_web(
        tmp.path().to_path_buf(),
        two_layouts(),
        &quick(true),
        Arc::new(SystemClock::new(London)),
    )
    .await
    .unwrap();
    let mut response = reqwest::get(format!("{base}/view/api/events"))
        .await
        .unwrap();

    let text = read_until(&mut response, |t| t.matches(": keepalive").count() >= 2).await;

    assert_eq!(state_events(&text).len(), 1, "{text:?}");
}

#[tokio::test]
async fn a_stream_ends_with_a_bye_after_its_time_so_the_page_opens_another() {
    let tmp = tempfile::tempdir().unwrap();
    publish_both(tmp.path(), FIRST, b"a").await;
    let (base, _) = start_web(
        tmp.path().to_path_buf(),
        two_layouts(),
        &quick(true),
        Arc::new(SystemClock::new(London)),
    )
    .await
    .unwrap();
    let started = std::time::Instant::now();
    let mut response = reqwest::get(format!("{base}/view/api/events"))
        .await
        .unwrap();

    let text = read_until(&mut response, |_| false).await;

    assert!(text.ends_with("event: bye\ndata: {}\n\n"), "{text:?}");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_millis(1400)
            && elapsed < std::time::Duration::from_secs(4),
        "{elapsed:?}"
    );
}

#[tokio::test]
async fn on_auto_a_stream_says_when_the_schedule_changes_the_layout() {
    let tmp = tempfile::tempdir().unwrap();
    publish_both(tmp.path(), FIRST, b"all").await;
    publish_both(tmp.path(), "departures", b"trains").await;
    let clock = FixedClock::at(at(6, 0));
    let (base, _) = start_web(
        tmp.path().to_path_buf(),
        with_a_morning_rule(),
        &quick(true),
        clock.clone(),
    )
    .await
    .unwrap();
    let mut response = reqwest::get(format!("{base}/view/api/events"))
        .await
        .unwrap();
    let text = read_until(&mut response, |t| t.contains("event: state")).await;
    assert_eq!(state_events(&text)[0]["layout"], FIRST);

    // The morning window opens; a stream on auto moves to departures without being asked.
    clock.set(at(6, 31));

    let text = read_until(&mut response, |t| t.contains("event: state")).await;
    assert_eq!(state_events(&text)[0]["layout"], "departures");
}

#[tokio::test]
async fn a_stream_for_a_layout_by_name_stays_on_it_whatever_the_schedule_does() {
    let tmp = tempfile::tempdir().unwrap();
    publish_both(tmp.path(), FIRST, b"all").await;
    publish_both(tmp.path(), "departures", b"trains").await;
    let clock = FixedClock::at(at(6, 0));
    let (base, _) = start_web(
        tmp.path().to_path_buf(),
        with_a_morning_rule(),
        &quick(true),
        clock.clone(),
    )
    .await
    .unwrap();
    let mut response = reqwest::get(format!("{base}/view/api/events?layout=dashboard"))
        .await
        .unwrap();
    read_until(&mut response, |t| t.contains("event: state")).await;

    clock.set(at(6, 31));
    let text = read_until(&mut response, |t| t.contains("event: bye")).await;

    assert!(state_events(&text).is_empty(), "{text:?}");
}

#[tokio::test]
async fn a_stream_that_cannot_be_given_is_refused_in_a_status_as_the_state_is() {
    let tmp = tempfile::tempdir().unwrap();
    let (base, _) = start_web(
        tmp.path().to_path_buf(),
        two_layouts(),
        &quick(true),
        Arc::new(SystemClock::new(London)),
    )
    .await
    .unwrap();

    // Nothing drawn yet; then a layout there is not.
    assert_eq!(
        reqwest::get(format!("{base}/view/api/events"))
            .await
            .unwrap()
            .status(),
        404
    );
    publish_both(tmp.path(), FIRST, b"a").await;
    let missing = reqwest::get(format!("{base}/view/api/events?layout=nonesuch"))
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    assert_eq!(
        reqwest::get(format!("{base}/view/api/events"))
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn a_stream_wants_a_token_when_the_page_does_and_ends_when_the_token_is_revoked() {
    let g = guarded_for_streams().await;
    let token = g.token(&[]).await;

    assert_eq!(g.with("/view/api/events", None).await.status(), 401);
    let mut response = g.bearing("/view/api/events", &token).await;
    assert_eq!(response.status(), 200);
    read_until(&mut response, |t| t.contains("event: state")).await;

    // The owner revokes the viewer: within a tick the stream says bye, and the page's next try is refused.
    let id = g.viewers.authenticate(&token).await.unwrap().viewer;
    g.viewers.revoke(&id).await.unwrap();
    let text = read_until(&mut response, |t| t.contains("event: bye")).await;
    assert!(text.contains("event: bye"), "{text:?}");
    assert_eq!(g.bearing("/view/api/events", &token).await.status(), 401);
}

#[tokio::test]
async fn a_stream_is_for_the_layouts_the_token_allows() {
    let g = guarded_for_streams().await;
    let token = g.token(&["departures"]).await;

    let forbidden = g.bearing("/view/api/events?layout=dashboard", &token).await;
    assert_eq!(forbidden.status(), 403);
    assert!(challenge_of(&forbidden).contains("insufficient_scope"));
    // On auto it is the one it may see.
    let mut allowed = g.bearing("/view/api/events", &token).await;
    let text = read_until(&mut allowed, |t| t.contains("event: state")).await;
    assert_eq!(state_events(&text)[0]["layout"], "departures");
}

/// A server that wants a token, with a stream that is quick about everything.
async fn guarded_for_streams() -> Guarded {
    guarded_by(None, &quick(false)).await
}

#[tokio::test]
async fn the_service_worker_is_served_to_anyone_with_the_scope_the_page_needs_and_keeps_nothing_private()
 {
    let g = guarded().await;

    let response = g.with("/view/sw.js", None).await;

    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "text/javascript; charset=utf-8"
    );
    // A page at `/view` is outside the folder the script is in (`/view/`), so the scope is allowed to reach it.
    assert_eq!(response.headers()["service-worker-allowed"], "/view");
    assert_eq!(response.headers()["cache-control"], "no-cache");
    let script = response.text().await.unwrap();
    // It keeps the shell, and answers nothing from the API: that is the viewer's, and a revoked token must be refused.
    for kept in [
        "'/view'",
        "'/view/manifest.webmanifest'",
        "'/view/icons/192.png'",
    ] {
        assert!(script.contains(kept), "{kept}");
    }
    assert!(
        !script.contains("/view/api"),
        "the worker must leave the API to the network"
    );
    // And the page registers it with that scope.
    let page = g.with("/view", None).await.text().await.unwrap();
    assert!(page.contains("register('/view/sw.js', { scope: '/view' })"));
}

#[test]
fn the_page_asks_for_a_stream_keeps_its_last_picture_and_forgets_it_on_leaving() {
    // The stream: read with fetch (a token can't go in an EventSource), reopened when it ends, and not held open while hidden.
    assert!(PAGE.contains("api('events?layout='"));
    assert!(PAGE.contains("response.body.getReader()"));
    assert!(PAGE.contains("if (document.hidden) closeStream()"));
    assert!(!PAGE.contains("new EventSource"));
    // The last picture: remembered per layout choice, shown at once on load, forgotten on sign-out and when access ends.
    assert!(PAGE.contains("indexedDB.open('hd', 1)"));
    assert!(PAGE.contains("showRemembered().then(refresh)"));
    let forgets = PAGE.matches("forgetPicture()").count();
    assert!(forgets >= 3, "{forgets}");
    assert!(PAGE.contains("indexedDB.deleteDatabase('hd')"));
}

/// Needs a real Chrome, so run with `--ignored`. A page left open on a wall: a new picture is on screen within seconds of the
/// render that made it finishing (pushed by the stream: the page's own timer is an hour away), and with the network gone the
/// page, reloaded, still opens and shows the last picture it saw and says how old it is.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_page_on_a_wall_is_pushed_new_pictures_and_survives_the_network_going_in_a_browser() {
    let tmp = tempfile::tempdir().unwrap();
    publish_picture(tmp.path(), FIRST, 40).await;
    crate::adapters::image_server::testing::set_age(tmp.path(), 120);
    let (base, handles) = start_web(
        tmp.path().to_path_buf(),
        two_layouts(),
        &open_all(),
        Arc::new(SystemClock::new(London)),
    )
    .await
    .unwrap();
    let directory = tmp.path().to_path_buf();
    let notified = handles.clone();

    let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let browser = headless_chrome::Browser::new(
            headless_chrome::LaunchOptions::default_builder()
                .headless(true)
                .path(headless_chrome::browser::default_executable().ok())
                .build()?,
        )?;
        let tab = browser.new_tab()?;
        let number = |script: &str| -> anyhow::Result<f64> {
            Ok(tab.evaluate(script, false)?.value.and_then(|v| v.as_f64()).unwrap_or(-1.0))
        };
        let text = |script: &str, promise: bool| -> anyhow::Result<String> {
            Ok(tab
                .evaluate(script, promise)?
                .value
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default())
        };
        let waits = |script: &str, wanted: f64, seconds: u32| -> anyhow::Result<()> {
            for _ in 0..(seconds * 10) {
                if number(script)? >= wanted {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            anyhow::bail!("{script} never reached {wanted}")
        };
        let width = "document.getElementById('picture').naturalWidth";
        let handle = tokio::runtime::Handle::current();

        tab.navigate_to(&format!("{base}/view?layout=dashboard"))?.wait_until_navigated()?;
        waits(width, 40.0, 10)?;
        // The shell is kept by the worker, and the picture by the page.
        let mut kept = String::new();
        for _ in 0..50 {
            kept = text(
                "caches.keys().then(async (names) => { for (const n of names) { if ((await (await caches.open(n)).keys()).length) return 'yes'; } return 'no'; })",
                true,
            )?;
            if kept == "yes" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        anyhow::ensure!(kept == "yes", "the worker kept no shell");
        std::thread::sleep(std::time::Duration::from_millis(1500));

        // A render ends with a new picture. Nothing asks: the stream says so, and the picture is there in a moment.
        handle.block_on(publish_picture(&directory, FIRST, 30));
        let started = std::time::Instant::now();
        notified.refresh.render_succeeded(
            chrono::Utc::now().with_timezone(&London),
            &crate::domain::models::render_report::RenderReport::default(),
        );
        waits("document.getElementById('picture').naturalWidth == 30 ? 1 : 0", 1.0, 8)?;
        anyhow::ensure!(started.elapsed() < std::time::Duration::from_secs(8), "pushed too slowly");

        // The network goes. The page is reloaded: the shell comes from the worker, and the last picture from the page's own
        // memory, with no word from the server.
        tab.call_method(headless_chrome::protocol::cdp::Network::Enable {
            max_total_buffer_size: None,
            max_resource_buffer_size: None,
            max_post_data_size: None,
            report_direct_socket_traffic: None,
            enable_durable_messages: None,
        })?;
        tab.call_method(headless_chrome::protocol::cdp::Network::EmulateNetworkConditions {
            offline: true,
            latency: 0.0,
            download_throughput: -1.0,
            upload_throughput: -1.0,
            connection_Type: None,
            packet_loss: None,
            packet_queue_length: None,
            packet_reordering: None,
        })?;
        // An ordinary reload: a hard one (ignoring caches) skips the worker by design.
        tab.reload(false, None)?;
        std::thread::sleep(std::time::Duration::from_millis(2500));
        if number(width)? != 30.0 {
            let seen = text(
                "location.href + ' | ' + document.title + ' | controller=' + !!(navigator.serviceWorker && navigator.serviceWorker.controller) + ' | status=' + (document.getElementById('status') ? document.getElementById('status').textContent : 'none') + ' | width=' + (document.getElementById('picture') ? document.getElementById('picture').naturalWidth : 'none')",
                false,
            )?;
            anyhow::bail!("the last picture was not shown with the network down: {seen}");
        }
        let said = text("document.getElementById('status').textContent", false)?;
        anyhow::ensure!(said.contains("Cannot reach the server"), "the page says {said:?}");
        anyhow::ensure!(said.contains("Showing the picture from"), "the page does not say how old it is: {said:?}");
        Ok(())
    })
    .await
    .unwrap();

    outcome.unwrap();
}

/// Needs a real Chrome, so run with `--ignored`. The server fails to renew a token twice running (as it would for a
/// moment of bad Wi-Fi, a `5xx` or a `429`) just as the access token runs out: that is not the end of the viewer's access,
/// so the page keeps its tokens and its picture, tries again, and renews.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn a_renewal_that_fails_does_not_sign_a_refreshing_viewer_out_and_is_tried_again() {
    let g = guarded_for(None, &needing_a_token(), Some(chrono::Duration::seconds(2))).await;
    publish_picture(g._directory.path(), FIRST, 40).await;
    publish_picture(g._directory.path(), "departures", 20).await;
    let issued = g
        .viewers
        .issue(
            IssueRequest {
                label: "wall tablet".into(),
                refreshing: true,
                ..IssueRequest::default()
            },
            &g.layouts,
        )
        .await
        .unwrap();
    let link = format!("{}/view#code={}", g.base, issued.code.expose());
    let (base, clock) = (g.base.clone(), g.clock.clone());

    let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let browser = headless_chrome::Browser::new(
            headless_chrome::LaunchOptions::default_builder()
                .headless(true)
                .path(headless_chrome::browser::default_executable().ok())
                .build()?,
        )?;
        let tab = browser.new_tab()?;
        let text = |script: &str| -> anyhow::Result<String> {
            Ok(tab
                .evaluate(script, false)?
                .value
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default())
        };
        let number = |script: &str| -> anyhow::Result<f64> {
            Ok(tab
                .evaluate(script, false)?
                .value
                .and_then(|v| v.as_f64())
                .unwrap_or(-1.0))
        };
        let waits = |script: &str, wanted: f64, tries: u32| -> anyhow::Result<()> {
            for _ in 0..tries {
                if number(script)? >= wanted {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            anyhow::bail!("{script} never reached {wanted}")
        };
        let picture_width = "document.getElementById('picture').naturalWidth";
        let prompt = "document.getElementById('signin').hidden ? 0 : 1";

        tab.navigate_to(&format!("{base}/view?away"))?
            .wait_until_navigated()?;
        tab.navigate_to(&link)?.wait_until_navigated()?;
        waits(picture_width, 20.0, 100)?;
        let refresh_before = text("localStorage.getItem('hd.refresh')")?;
        anyhow::ensure!(refresh_before.starts_with("hdr1_"), "no refresh token kept");

        // The next two requests for a token fail at the server, as a hiccup would; others go through.
        tab.evaluate(
            "window.__fails = 2; var real = window.fetch; window.fetch = function (url, options) { \
               if (String(url).indexOf('/view/api/token') >= 0 && window.__fails > 0) { \
                 window.__fails -= 1; return Promise.resolve(new Response('', { status: 500 })); } \
               return real.apply(this, arguments); };",
            false,
        )?;
        // The access token runs out (by the server's clock, which a test moves on), and the page looks again.
        clock.set(at(7, 0) + chrono::Duration::seconds(5));
        std::thread::sleep(std::time::Duration::from_millis(2500));
        tab.evaluate(
            "document.dispatchEvent(new Event('visibilitychange'))",
            false,
        )?;
        std::thread::sleep(std::time::Duration::from_millis(1500));

        // Not signed out: the prompt is not up, and the tokens and the picture are as they were.
        anyhow::ensure!(
            number(prompt)? == 0.0,
            "the page said access had ended after a failed renewal"
        );
        anyhow::ensure!(
            text("localStorage.getItem('hd.refresh')")? == refresh_before,
            "the refresh token was dropped"
        );
        anyhow::ensure!(number(picture_width)? == 20.0, "the picture went");

        // And it is tried again, and works: a new refresh token, still the picture, no prompt.
        for _ in 0..250 {
            if text("localStorage.getItem('hd.refresh')")? != refresh_before {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        anyhow::ensure!(
            text("localStorage.getItem('hd.refresh')")? != refresh_before,
            "it never tried again: status {:?}, title {:?}, token {:?}, fails left {}",
            text("document.getElementById('status').textContent")?,
            text("document.getElementById('signin-title').textContent")?,
            text("localStorage.getItem('hd.token')")?,
            number("window.__fails")?
        );
        anyhow::ensure!(number(prompt)? == 0.0, "the prompt came up");
        anyhow::ensure!(
            number(picture_width)? == 20.0,
            "the picture went after the retry"
        );
        Ok(())
    })
    .await
    .unwrap();

    outcome.unwrap();
}

// --- the service worker's cache is named for what is in it ----------------------------------------------

fn the_shell() -> (String, Vec<(u32, Vec<u8>)>) {
    (
        manifest_json(),
        icons::SIZES.iter().map(|s| (*s, icons::png(*s))).collect(),
    )
}

#[test]
fn the_fingerprint_is_short_stable_and_changes_with_each_thing_the_worker_keeps() {
    let (manifest, icons) = the_shell();
    let base = shell_fingerprint(PAGE, &manifest, &icons, icons::SVG, WORKER);

    assert_eq!(base.len(), 12);
    assert!(base.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(
        base,
        shell_fingerprint(PAGE, &manifest, &icons, icons::SVG, WORKER)
    );
    // Each of them alone is enough to make a new worker, and so a new cache.
    let mut icon_changed = icons.clone();
    icon_changed[0].1.push(0);
    for (what, changed) in [
        (
            "page",
            shell_fingerprint("<p>other</p>", &manifest, &icons, icons::SVG, WORKER),
        ),
        (
            "manifest",
            shell_fingerprint(PAGE, "{}", &icons, icons::SVG, WORKER),
        ),
        (
            "icon",
            shell_fingerprint(PAGE, &manifest, &icon_changed, icons::SVG, WORKER),
        ),
        (
            "svg",
            shell_fingerprint(PAGE, &manifest, &icons, "<svg/>", WORKER),
        ),
        (
            "worker",
            shell_fingerprint(PAGE, &manifest, &icons, icons::SVG, "// other"),
        ),
    ] {
        assert_ne!(changed, base, "{what}");
    }
    // Parts are told apart by length, so moving a byte from one to the next is not the same input.
    assert_ne!(
        shell_fingerprint("ab", "c", &[], "", ""),
        shell_fingerprint("a", "bc", &[], "", "")
    );
}

#[tokio::test]
async fn the_worker_is_served_with_its_cache_named_for_what_it_keeps_and_no_placeholder_left() {
    let tmp = tempfile::tempdir().unwrap();
    let base = serve(tmp.path(), &open_all()).await;

    let response = reqwest::get(format!("{base}/view/sw.js")).await.unwrap();
    assert_eq!(response.status(), 200);
    let script = response.text().await.unwrap();

    let (manifest, icons) = the_shell();
    let expected = shell_fingerprint(PAGE, &manifest, &icons, icons::SVG, WORKER);
    assert!(
        script.contains(&format!("const CACHE = 'hd-shell-{expected}';")),
        "{script}"
    );
    assert!(!script.contains("__SHELL__"));
    // The same on the next request, and from a second server: the name is of the content, not of the process.
    let again = serve(tmp.path(), &open_all()).await;
    assert_eq!(
        reqwest::get(format!("{again}/view/sw.js"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        script
    );
}

#[test]
fn the_workers_source_has_the_placeholder_once_in_the_cache_name() {
    assert_eq!(WORKER.matches("__SHELL__").count(), 1);
    assert!(WORKER.contains("const CACHE = 'hd-shell-__SHELL__';"));
}

// --- streams are a share of the server's connections, not all of them ---------------------------------

async fn watching(base: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{base}/view/api/events"))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_source_holds_only_its_share_of_streams_and_the_rest_of_the_page_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    publish_both(tmp.path(), FIRST, b"a").await;
    let settings = WebSettings {
        stream: StreamTiming {
            most_per_source: 1,
            ..StreamTiming::default()
        },
        ..open_all()
    };
    let (base, _) = start_web_for(
        tmp.path().to_path_buf(),
        two_layouts(),
        &settings,
        Arc::new(SystemClock::new(London)),
        None,
        Some("192.168.1.20:5000".parse().unwrap()),
    )
    .await
    .unwrap();

    let first = watching(&base).await;
    assert_eq!(first.status(), 200);
    // A second from the same place is told so, and when to ask again, without its connection being the worse for it.
    let second = watching(&base).await;
    assert_eq!(second.status(), 503);
    assert_eq!(second.headers()["retry-after"], "30");
    // The page's other requests, which it carries on with, are answered.
    assert_eq!(get(format!("{base}/view/api/state")).await.0, 200);
    assert_eq!(get(format!("{base}/view/api/layouts")).await.0, 200);

    // Given up, the place is there again.
    drop(first);
    let mut again = 0;
    for _ in 0..40 {
        again = watching(&base).await.status().as_u16();
        if again == 200 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(again, 200, "the place was never given back");
}

fn forwarded(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.append("x-forwarded-for", HeaderValue::from_str(value).unwrap());
    headers
}

#[test]
fn a_peer_that_is_not_a_trusted_proxy_is_the_client_whatever_it_claims() {
    let proxies = ["192.168.1.2".parse().unwrap()];
    let peer = "192.168.1.50".parse().unwrap();
    assert_eq!(forwarded_for(peer, &forwarded("10.0.0.9"), &proxies), peer);
    assert_eq!(forwarded_for(peer, &forwarded("10.0.0.9"), &[]), peer);
}

#[test]
fn a_trusted_proxy_passes_on_the_client_it_was_reached_from() {
    let proxies: Vec<IpAddr> = vec![
        "192.168.1.2".parse().unwrap(),
        "192.168.1.3".parse().unwrap(),
    ];
    let proxy = proxies[0];
    let client: IpAddr = "10.0.0.9".parse().unwrap();
    assert_eq!(
        forwarded_for(proxy, &forwarded("10.0.0.9"), &proxies),
        client
    );
    // Through a second trusted proxy, the client is the first address that isn't one.
    assert_eq!(
        forwarded_for(proxy, &forwarded("10.0.0.9, 192.168.1.3"), &proxies),
        client
    );
    // What the client itself wrote in front of that is not believed.
    assert_eq!(
        forwarded_for(proxy, &forwarded("6.6.6.6, 10.0.0.9"), &proxies),
        client
    );
    // The same when the header is sent more than once.
    let mut headers = forwarded("6.6.6.6");
    headers.append("x-forwarded-for", HeaderValue::from_static("10.0.0.9"));
    assert_eq!(forwarded_for(proxy, &headers, &proxies), client);
}

#[test]
fn a_header_that_cannot_be_read_as_addresses_leaves_the_proxy_as_the_client() {
    let proxies: Vec<IpAddr> = vec!["192.168.1.2".parse().unwrap()];
    let proxy = proxies[0];
    assert_eq!(forwarded_for(proxy, &HeaderMap::new(), &proxies), proxy);
    assert_eq!(
        forwarded_for(proxy, &forwarded("not-an-address"), &proxies),
        proxy
    );
    assert_eq!(
        forwarded_for(proxy, &forwarded("10.0.0.9, unknown"), &proxies),
        proxy
    );
    // Every entry a trusted proxy: nobody else to name.
    assert_eq!(
        forwarded_for(proxy, &forwarded("192.168.1.2"), &proxies),
        proxy
    );
}

#[test]
fn an_ipv4_proxy_is_recognised_when_it_arrives_written_as_ipv6() {
    let proxies: Vec<IpAddr> = vec!["192.168.1.2".parse().unwrap()];
    let mapped: IpAddr = "::ffff:192.168.1.2".parse().unwrap();
    assert_eq!(
        forwarded_for(mapped, &forwarded("10.0.0.9"), &proxies),
        "10.0.0.9".parse::<IpAddr>().unwrap()
    );
}

#[tokio::test]
async fn behind_a_trusted_proxy_each_client_has_its_own_share_of_streams() {
    let tmp = tempfile::tempdir().unwrap();
    publish_both(tmp.path(), FIRST, b"a").await;
    let settings = WebSettings {
        stream: StreamTiming {
            most_per_source: 1,
            ..StreamTiming::default()
        },
        trusted_proxies: vec!["192.168.1.20".parse().unwrap()],
        ..open_all()
    };
    let (base, _) = start_web_for(
        tmp.path().to_path_buf(),
        two_layouts(),
        &settings,
        Arc::new(SystemClock::new(London)),
        None,
        Some("192.168.1.20:5000".parse().unwrap()),
    )
    .await
    .unwrap();
    let watching_as = |client: &'static str| {
        let url = format!("{base}/view/api/events");
        async move {
            reqwest::Client::new()
                .get(url)
                .header("x-forwarded-for", client)
                .send()
                .await
                .unwrap()
        }
    };
    let one = watching_as("10.0.0.1").await;
    assert_eq!(one.status(), 200);
    // Another client of the same proxy has a place; the first, a second, has not.
    let other = watching_as("10.0.0.2").await;
    assert_eq!(other.status(), 200);
    assert_eq!(watching_as("10.0.0.1").await.status(), 503);
}

#[tokio::test]
async fn the_total_is_bounded_even_where_no_source_is_known() {
    // Plain HTTP, as open mode over it is: no peer to count, so only the total can be kept to.
    let tmp = tempfile::tempdir().unwrap();
    publish_both(tmp.path(), FIRST, b"a").await;
    let settings = WebSettings {
        stream: StreamTiming {
            most: 2,
            ..StreamTiming::default()
        },
        ..open_all()
    };
    let (base, _) = start_web(
        tmp.path().to_path_buf(),
        two_layouts(),
        &settings,
        Arc::new(SystemClock::new(London)),
    )
    .await
    .unwrap();

    let _one = watching(&base).await;
    let _two = watching(&base).await;
    let third = watching(&base).await;

    assert_eq!(third.status(), 503);
    assert_eq!(get(format!("{base}/view/api/state")).await.0, 200);
}

#[tokio::test]
async fn a_stream_refused_for_want_of_a_place_says_so_before_the_picture_is_looked_for() {
    // Nothing drawn yet: a stream that is wanted but not there is 404 as for the state, and not 503, which is only for a
    // stream that would have been given.
    let tmp = tempfile::tempdir().unwrap();
    let (base, _) = start_web(
        tmp.path().to_path_buf(),
        two_layouts(),
        &WebSettings {
            stream: StreamTiming {
                most: 0,
                ..StreamTiming::default()
            },
            ..open_all()
        },
        Arc::new(SystemClock::new(London)),
    )
    .await
    .unwrap();

    assert_eq!(watching(&base).await.status(), 404);
}
