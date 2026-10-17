//! The page itself, and what a browser needs to put it on a home screen: manifest, icons, service worker, and the policy that lets the page run.

use super::*;

/// The page.
#[utoipa::path(
    get,
    path = "/view",
    tag = "web",
    responses((
        status = 200,
        description = "A web page that shows the picture and keeps it up to date. It holds nothing private, so it is served to anyone, and asks for a token itself.",
        content_type = "text/html",
        body = String
    ))
)]
pub(super) async fn page(State(state): State<Arc<WebState>>) -> Response {
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

/// The web app manifest: what makes the page installable, so that a tablet can put it on its home screen and open it full
/// screen, with no address bar, like an app.
#[utoipa::path(
    get,
    path = "/view/manifest.webmanifest",
    tag = "web",
    responses((
        status = 200,
        description = "The web app manifest, as `application/manifest+json`.",
        content_type = "application/manifest+json",
        body = String
    ))
)]
pub(super) async fn manifest() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/manifest+json"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        manifest_json(),
    )
        .into_response()
}

/// What the service worker keeps, named: a short fingerprint of the page, the manifest, the icons and the worker's own script. A
/// worker kept by name alone would serve an old manifest or icon for ever after an upgrade, since it answers those from its
/// cache; named for what is in it, any change makes a new worker (a browser checks the script's bytes) with a new cache, and
/// the old is deleted when it takes over.
pub(super) fn shell_fingerprint(
    page: &str,
    manifest: &str,
    icons: &[(u32, Vec<u8>)],
    svg: &str,
    worker: &str,
) -> String {
    let mut hash = Sha256::new();
    let mut part = |bytes: &[u8]| {
        // With each part's length, so that no two arrangements of the same bytes give the same input.
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    };
    part(page.as_bytes());
    part(manifest.as_bytes());
    for (size, png) in icons {
        part(&size.to_be_bytes());
        part(png);
    }
    part(svg.as_bytes());
    part(worker.as_bytes());
    let digest = hash.finalize();
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The manifest, as JSON.
pub(super) fn manifest_json() -> String {
    let icons: Vec<_> = icons::SIZES
        .iter()
        .filter(|size| **size != 180)
        .map(|size| {
            serde_json::json!({
                "src": format!("/view/icons/{size}.png"),
                "sizes": format!("{size}x{size}"),
                "type": "image/png",
                "purpose": "any",
            })
        })
        .chain([serde_json::json!({
            "src": "/view/icon.svg",
            "sizes": "any",
            "type": "image/svg+xml",
        })])
        .collect();
    let manifest = serde_json::json!({
        "name": "Home display",
        "short_name": "Display",
        "description": "What the home display shows, kept up to date",
        // Opens as the kiosk: the picture alone, filling the screen.
        "start_url": "/view#kiosk",
        "scope": "/view",
        "display": "fullscreen",
        "display_override": ["fullscreen", "standalone"],
        "orientation": "any",
        "background_color": "#ffffff",
        "theme_color": "#111111",
        "icons": icons,
    });
    manifest.to_string()
}

/// The icon, as a vector.
#[utoipa::path(
    get,
    path = "/view/icon.svg",
    tag = "web",
    responses((
        status = 200,
        description = "The page's icon.",
        content_type = "image/svg+xml",
        body = String
    ))
)]
pub(super) async fn icon_svg() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        icons::SVG,
    )
        .into_response()
}

/// The service worker, which keeps the shell of the page so that it opens with the network down. It keeps nothing private.
#[utoipa::path(
    get,
    path = "/view/sw.js",
    tag = "web",
    responses((
        status = 200,
        description = "The service worker script. Its scope is the page (`/view`), which `Service-Worker-Allowed` says may be wider than the script's own folder.",
        content_type = "text/javascript",
        body = String
    ))
)]
pub(super) async fn worker(State(state): State<Arc<WebState>>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            // Always checked, so that a changed worker is found at once.
            (header::CACHE_CONTROL, "no-cache"),
            (HeaderName::from_static("service-worker-allowed"), "/view"),
        ],
        state.worker.clone(),
    )
        .into_response()
}

/// Which size of the icon.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(super) struct IconFile {
    /// The size in pixels and `.png`: `180.png` (what an Apple device looks for), `192.png` or `512.png`.
    #[param(example = "192.png")]
    file: String,
}

/// The icon, as a PNG of one of its sizes.
#[utoipa::path(
    get,
    path = "/view/icons/{file}",
    tag = "web",
    params(IconFile),
    responses(
        (status = 200, description = "The page's icon.", content_type = "image/png", body = Vec<u8>),
        (status = 404, description = "There is no icon of that size.", body = String, content_type = "text/plain")
    )
)]
pub(super) async fn icon_png(
    State(state): State<Arc<WebState>>,
    Path(IconFile { file }): Path<IconFile>,
) -> Response {
    let size = file
        .strip_suffix(".png")
        .and_then(|size| size.parse::<u32>().ok());
    match state.icons.iter().find(|(known, _)| Some(*known) == size) {
        Some((_, png)) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            png.clone(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "There is no icon of that size").into_response(),
    }
}

/// The text between the first `<tag>` and its `</tag>`, which is exactly what a browser hashes for a policy.
pub(super) fn inline_block<'a>(html: &'a str, tag: &str) -> &'a str {
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
pub(super) fn content_security_policy(page: &str) -> String {
    format!(
        "default-src 'none'; script-src {}; style-src {}; img-src blob: 'self'; connect-src 'self'; manifest-src 'self'; worker-src 'self'; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        hash_source(inline_block(page, "script")),
        hash_source(inline_block(page, "style")),
    )
}
