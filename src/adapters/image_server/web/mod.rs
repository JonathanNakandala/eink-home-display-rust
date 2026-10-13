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
//! Who may use the API is the owner's choice (`WebSettings::open`):
//!
//! - **Open:** anyone who can reach the server, as with `/image` over plain HTTP.
//! - **Tokens:** only a browser that shows a bearer token (RFC 6750) from a viewer the owner issued (see
//!   `application::viewers`). The page itself holds nothing private and is served to anyone, so that it can ask for the
//!   token. The token is got by trading a one-time code (`POST /view/api/token`, shaped as an OAuth token request), which is
//!   why no secret has to be in a URL. Answers follow RFC 6750 section 3: `WWW-Authenticate` says what is needed, `401`
//!   for a missing or bad token, `403` and `insufficient_scope` for a layout the token does not allow.
//!
//! These routes make their own decision about who may use them, and so sit outside the gate that the display routes are
//! behind over HTTPS.

mod limiter;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tower_http::trace::TraceLayer;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{IntoParams, Modify, OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use self::limiter::FailureLimiter;
use super::health::image_written_at;
use super::image::{etag_of, names};
use super::{Published, server_error};
use crate::adapters::est_server::Connection;
use crate::application::plan::{Plan, compute};
use crate::application::viewers::{AuthError, ExchangeError, Viewers};
use crate::domain::models::display::ImageFormat;
use crate::domain::models::layout::{Layout, LayoutName};
use crate::domain::models::viewer::LayoutScope;

const PAGE: &str = include_str!("view.html");

/// What the realm of a challenge is called.
const REALM: &str = "home-display";
/// The grant a code is exchanged under, as the OAuth 2.0 "pre-authorized code" grant OpenID for Verifiable Credential
/// Issuance names it: a code made ahead of time by the party that decides (here the owner), traded for a token.
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:pre-authorized_code";
/// The grant a refresh token is used under (RFC 6749 section 6).
const REFRESH_GRANT: &str = "refresh_token";
/// A token request is two short fields.
const MAX_FORM: usize = 2048;

/// Who the page and its API are for.
#[derive(Debug, Clone, Default)]
pub struct WebSettings {
    /// The layouts the page may show, in the order it lists them. Empty means all of them.
    pub layouts: Vec<LayoutName>,
    /// Whether anyone who can reach the server may use it. Otherwise a viewer token is needed.
    pub open: bool,
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

/// Who may use the API.
pub(super) enum Access {
    Open,
    Tokens {
        viewers: Arc<Viewers>,
        failures: FailureLimiter,
    },
}

impl Access {
    /// The access `settings` call for, or why there can't be: tokens need somewhere to keep the viewers.
    pub(super) fn of(
        settings: &WebSettings,
        viewers: Option<Arc<Viewers>>,
    ) -> Result<Self, String> {
        if settings.open {
            return Ok(Self::Open);
        }
        let viewers = viewers.ok_or_else(|| {
            "[server.web] without open = true needs viewer tokens, which need HTTPS and the admin interface to issue them"
                .to_owned()
        })?;
        Ok(Self::Tokens {
            viewers,
            failures: FailureLimiter::default(),
        })
    }
}

pub(super) struct WebState {
    published: Arc<Published>,
    /// What the page may show, in order. Never empty.
    allowed: Vec<Layout>,
    /// The content security policy the page is served with.
    policy: HeaderValue,
    access: Access,
}

/// Who is asking, once they have shown a token: what they may look at. Put on the request by `authenticate`.
#[derive(Clone)]
struct Principal {
    scope: LayoutScope,
}

/// Why a request for a layout is refused.
enum Refusal {
    /// There is no such layout, as far as the asker may know. Says which there are.
    NotFound(Vec<String>),
    /// The layout is there, and the token does not allow it (RFC 6750 section 3.1).
    Insufficient { scope: String },
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        match self {
            // The name is not repeated back: it is whatever the caller sent.
            Self::NotFound(known) => (
                StatusCode::NOT_FOUND,
                format!("There is no such layout. Layouts: {}", known.join(", ")),
            )
                .into_response(),
            Self::Insufficient { scope } => (
                StatusCode::FORBIDDEN,
                [(
                    header::WWW_AUTHENTICATE,
                    challenge(Some("insufficient_scope"), Some(&scope)),
                )],
                "This token does not allow that layout",
            )
                .into_response(),
        }
    }
}

impl WebState {
    pub(super) fn new(published: Arc<Published>, allowed: Vec<Layout>, access: Access) -> Self {
        assert!(!allowed.is_empty(), "the page has a layout to show");
        Self {
            published,
            allowed,
            policy: HeaderValue::from_str(&content_security_policy(PAGE))
                .expect("a policy is a header value"),
            access,
        }
    }

    /// The layouts the page may show to someone who may look at `scope`, in order.
    fn permitted(&self, scope: &LayoutScope) -> Vec<&Layout> {
        self.allowed
            .iter()
            .filter(|layout| scope.permits(&layout.name))
            .collect()
    }

    /// The layout a request without one gets: the one for this time of day if it may be shown, else the first that may.
    fn automatic(&self, scope: &LayoutScope) -> Result<&Layout, Refusal> {
        let permitted = self.permitted(scope);
        let current = self.published.layouts.current(self.published.clock.now());
        permitted
            .iter()
            .find(|layout| layout.name == current.name)
            .or(permitted.first())
            .copied()
            .ok_or_else(|| Refusal::Insufficient {
                scope: scope.to_scope_string(),
            })
    }

    /// The layout asked for (`auto`, or none, is the automatic one), or why it can't be shown.
    fn resolve(&self, scope: &LayoutScope, wanted: Option<&str>) -> Result<&Layout, Refusal> {
        let Some(name) = wanted.filter(|name| *name != "auto") else {
            return self.automatic(scope);
        };
        let Some(layout) = self
            .allowed
            .iter()
            .find(|layout| layout.name.as_str() == name)
        else {
            return Err(self.not_found(scope));
        };
        if scope.permits(&layout.name) {
            Ok(layout)
        } else {
            Err(Refusal::Insufficient {
                scope: format!("layout:{}", layout.name),
            })
        }
    }

    fn not_found(&self, scope: &LayoutScope) -> Refusal {
        Refusal::NotFound(
            self.permitted(scope)
                .iter()
                .map(|layout| layout.name.to_string())
                .collect(),
        )
    }
}

/// What `request` is allowed to look at: everything the page may show, when it is open.
fn scope_of(principal: Option<Extension<Principal>>) -> LayoutScope {
    principal.map_or(LayoutScope::All, |Extension(principal)| principal.scope)
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

/// A `WWW-Authenticate` challenge (RFC 6750 section 3): what is needed, and if a token was shown, what was wrong with it.
/// Without a token there is no error code.
fn challenge(error: Option<&str>, scope: Option<&str>) -> HeaderValue {
    let mut text = format!("Bearer realm=\"{REALM}\"");
    if let Some(error) = error {
        text.push_str(&format!(", error=\"{error}\""));
    }
    if let Some(scope) = scope {
        text.push_str(&format!(", scope=\"{scope}\""));
    }
    HeaderValue::from_str(&text).expect("a challenge is a header value")
}

/// What an `Authorization` header holds.
enum Presented {
    /// No header, or one for another scheme: no authentication information of ours.
    Nothing,
    /// A bearer header that is not shaped like one.
    Malformed,
    Token(String),
}

fn presented(headers: &HeaderMap) -> Presented {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Presented::Nothing;
    };
    let Ok(text) = value.to_str() else {
        return Presented::Malformed;
    };
    let Some((scheme, rest)) = text.split_once(' ') else {
        return if text.eq_ignore_ascii_case("bearer") {
            Presented::Malformed
        } else {
            Presented::Nothing
        };
    };
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Presented::Nothing;
    }
    // RFC 6750 section 2.1: `Bearer` and one space, then the token (b64token, which has no spaces).
    if rest.is_empty() || rest.contains(' ') {
        return Presented::Malformed;
    }
    Presented::Token(rest.to_owned())
}

/// Where a request came from, for counting what one source does: the peer of the connection, or one shared place for a
/// request that has no peer to tell.
fn source_of_request(connection: &Option<Extension<Connection>>) -> IpAddr {
    connection
        .as_ref()
        .and_then(|Extension(connection)| connection.peer)
        .map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |peer| peer.ip())
}

fn too_many(wait: std::time::Duration) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, wait.as_secs().max(1).to_string())],
        Json(OAuthError::new(
            "temporarily_unavailable",
            "Too many requests that were not good; wait and try again",
        )),
    )
        .into_response()
}

/// For the routes that give the picture away: lets through a request that shows a good token (or any, when open), and answers
/// the rest as RFC 6750 says.
async fn authenticate(
    State(state): State<Arc<WebState>>,
    connection: Option<Extension<Connection>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Access::Tokens { viewers, failures } = &state.access else {
        return next.run(request).await;
    };
    let source = source_of_request(&connection);
    if let Some(wait) = failures.wait(source) {
        return too_many(wait);
    }
    let token = match presented(request.headers()) {
        Presented::Token(token) => token,
        Presented::Nothing => {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, challenge(None, None))],
                "A viewer token is needed",
            )
                .into_response();
        }
        Presented::Malformed => {
            failures.failed(source);
            return (
                StatusCode::BAD_REQUEST,
                [(
                    header::WWW_AUTHENTICATE,
                    challenge(Some("invalid_request"), None),
                )],
                "The Authorization header is not `Bearer` and a token",
            )
                .into_response();
        }
    };
    match viewers.authenticate(&token).await {
        Ok(who) => {
            failures.succeeded(source);
            request
                .extensions_mut()
                .insert(Principal { scope: who.scope });
            let mut response = next.run(request).await;
            // What is answered depends on who asks.
            response
                .headers_mut()
                .append(header::VARY, HeaderValue::from_static("Authorization"));
            response
        }
        Err(AuthError::InvalidToken) => {
            failures.failed(source);
            (
                StatusCode::UNAUTHORIZED,
                [(
                    header::WWW_AUTHENTICATE,
                    challenge(Some("invalid_token"), None),
                )],
                "The token is not good: it may have run out or been revoked",
            )
                .into_response()
        }
        Err(AuthError::Failed(e)) => server_error("check the viewer token of", e),
    }
}

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearerAuth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some(
                        "A viewer token (RFC 6750), from `POST /view/api/token`. Needed unless the server is configured with \
                         `[server.web] open = true`, when none is.",
                    ))
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    modifiers(&BearerAuth),
    tags((
        name = "web",
        description = "A page for a web browser to look at the picture, and what it asks of the server. Served only when [server.web] is enabled."
    ))
)]
struct WebDoc;

/// The page.
fn open_routes() -> OpenApiRouter<Arc<WebState>> {
    OpenApiRouter::with_openapi(WebDoc::openapi()).routes(routes!(page))
}

/// What gives the picture away: behind `authenticate`.
fn protected_routes() -> OpenApiRouter<Arc<WebState>> {
    OpenApiRouter::new()
        .routes(routes!(layouts))
        .routes(routes!(state))
        .routes(routes!(image))
}

/// Getting a token, and giving it up: there only when tokens are how the page is let in.
fn token_routes() -> OpenApiRouter<Arc<WebState>> {
    OpenApiRouter::new()
        .routes(routes!(token_endpoint))
        .routes(routes!(revoke_endpoint))
}

/// Every route, for the API description.
pub(super) fn routes() -> OpenApiRouter<Arc<WebState>> {
    open_routes()
        .merge(protected_routes())
        .merge(token_routes())
}

/// The routes, with the page's own security headers, for `router` to serve.
pub(super) fn router(state: WebState) -> Router {
    let state = Arc::new(state);
    let (open, _) = open_routes().split_for_parts();
    let (protected, _) = protected_routes().split_for_parts();
    let mut app = open
        .merge(protected.route_layer(middleware::from_fn_with_state(state.clone(), authenticate)));
    if matches!(state.access, Access::Tokens { .. }) {
        let (tokens, _) = token_routes().split_for_parts();
        app = app.merge(tokens.layer(DefaultBodyLimit::max(MAX_FORM)));
    }
    crate::adapters::response_headers::hardened(app.layer(TraceLayer::new_for_http()))
        .with_state(state)
}

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
    security((), ("bearerAuth" = [])),
    responses(
        (
            status = 200,
            description = "The layouts, and which one `auto` means now.",
            body = LayoutItems,
            content_type = "application/json",
            headers(("cache-control" = String, description = "Always `no-store`."))
        ),
        (status = 401, description = "A viewer token is needed, or the one shown is not good. `WWW-Authenticate` says which (RFC 6750).", body = String, content_type = "text/plain"),
        (status = 403, description = "The token allows none of the layouts the page may show.", body = String, content_type = "text/plain"),
        (status = 429, description = "Too many requests that were not good from this source; `Retry-After` says how long to wait.", body = OAuthError)
    )
)]
async fn layouts(
    State(state): State<Arc<WebState>>,
    principal: Option<Extension<Principal>>,
) -> Response {
    let scope = scope_of(principal);
    let automatic = match state.automatic(&scope) {
        Ok(layout) => layout.name.clone(),
        Err(refusal) => return refusal.into_response(),
    };
    let items = state
        .permitted(&scope)
        .into_iter()
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
    security((), ("bearerAuth" = [])),
    params(WhichLayout, Have),
    responses(
        (
            status = 200,
            description = "The picture's version, its age, whether it is stale, and how long to wait before asking again.",
            body = ViewState,
            content_type = "application/json",
            headers(("cache-control" = String, description = "Always `no-store`."))
        ),
        (status = 401, description = "A viewer token is needed, or the one shown is not good.", body = String, content_type = "text/plain"),
        (status = 403, description = "The token does not allow that layout (`insufficient_scope`).", body = String, content_type = "text/plain"),
        (status = 404, description = "Nothing has been rendered yet, or there is no such layout (the body lists the ones there are).", body = String, content_type = "text/plain"),
        (status = 429, description = "Too many requests that were not good from this source.", body = OAuthError),
        (status = 500, description = "The server could not work the state out.")
    )
)]
async fn state(
    State(state): State<Arc<WebState>>,
    principal: Option<Extension<Principal>>,
    Query(which): Query<WhichLayout>,
    Query(have): Query<Have>,
) -> Response {
    let layout = match state.resolve(&scope_of(principal), which.layout.as_deref()) {
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
    security((), ("bearerAuth" = [])),
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
        (status = 401, description = "A viewer token is needed, or the one shown is not good.", body = String, content_type = "text/plain"),
        (status = 403, description = "The token does not allow that layout (`insufficient_scope`).", body = String, content_type = "text/plain"),
        (status = 404, description = "Nothing has been rendered yet, or there is no such layout.", body = String, content_type = "text/plain"),
        (status = 429, description = "Too many requests that were not good from this source.", body = OAuthError),
        (status = 500, description = "An image exists but could not be read.")
    )
)]
async fn image(
    State(state): State<Arc<WebState>>,
    principal: Option<Extension<Principal>>,
    Query(which): Query<WhichLayout>,
    headers: HeaderMap,
) -> Response {
    let layout = match state.resolve(&scope_of(principal), which.layout.as_deref()) {
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

/// An error from the token endpoint, as RFC 6749 section 5.2 has it.
#[derive(Serialize, ToSchema)]
struct OAuthError {
    /// `invalid_request`, `invalid_grant`, `unsupported_grant_type` or `temporarily_unavailable`.
    #[schema(examples("invalid_grant"))]
    error: &'static str,
    error_description: &'static str,
}

impl OAuthError {
    fn new(error: &'static str, error_description: &'static str) -> Self {
        Self {
            error,
            error_description,
        }
    }
}

/// What a token request is: the grant, and the code or the refresh token.
#[derive(Deserialize, ToSchema)]
#[allow(dead_code)]
struct TokenRequest {
    /// `urn:ietf:params:oauth:grant-type:pre-authorized_code` to trade the code in a link, or `refresh_token` to trade a
    /// refresh token.
    grant_type: String,
    /// With the code grant: the one-time code from the link `displayctl viewer create` printed.
    #[serde(rename = "pre-authorized_code")]
    #[schema(rename = "pre-authorized_code", examples("hdc1_…"))]
    pre_authorized_code: Option<String>,
    /// With the refresh grant: the refresh token the last answer gave, which works once.
    #[schema(examples("hdr1_…"))]
    refresh_token: Option<String>,
}

/// A token, as RFC 6749 section 5.1 has it.
#[derive(Serialize, ToSchema)]
struct TokenResponse {
    /// The bearer token, shown here and nowhere else, once.
    access_token: String,
    /// For a viewer made with `--refresh`: the token to ask for the next access token with. It works once, and each answer
    /// brings a new one, which replaces it (RFC 6749 section 6, with the rotation of RFC 9700 section 4.14).
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    /// Always `Bearer`.
    token_type: &'static str,
    /// Seconds until it runs out.
    expires_in: i64,
    /// What it allows: `layout:*`, or `layout:NAME` for each layout.
    scope: String,
}

fn oauth_error(status: StatusCode, error: &'static str, description: &'static str) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        Json(OAuthError::new(error, description)),
    )
        .into_response()
}

/// The fields of an `application/x-www-form-urlencoded` body.
fn form_fields(headers: &HeaderMap, body: &Bytes) -> Option<Vec<(String, String)>> {
    let kind = headers.get(header::CONTENT_TYPE)?.to_str().ok()?;
    let essence = kind.split(';').next()?.trim();
    if !essence.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        return None;
    }
    Some(
        url::form_urlencoded::parse(body)
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect(),
    )
}

fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
}

/// Trades the one-time code from the owner's link for a token, once; or, for a viewer made with `--refresh`, trades its
/// refresh token for a new access token and a new refresh token (which replaces it).
#[utoipa::path(
    post,
    path = "/view/api/token",
    tag = "web",
    request_body(
        content = TokenRequest,
        content_type = "application/x-www-form-urlencoded",
        description = "An OAuth token request: the pre-authorized code grant for the code in a link, or the refresh token grant."
    ),
    responses(
        (
            status = 200,
            description = "The token. It is not shown again.",
            body = TokenResponse,
            content_type = "application/json",
            headers(
                ("cache-control" = String, description = "Always `no-store`."),
                ("pragma" = String, description = "Always `no-cache`.")
            )
        ),
        (status = 400, description = "`invalid_request` (not a form, or a field missing), `unsupported_grant_type`, or `invalid_grant`: the code or refresh token is not one there is, has been used, or has run out or been revoked, which are not told apart. A refresh token used again after it was replaced ends the session.", body = OAuthError),
        (status = 429, description = "Too many requests that were not good from this source; `Retry-After` says how long to wait.", body = OAuthError),
        (status = 500, description = "The server could not do it.")
    )
)]
async fn token_endpoint(
    State(state): State<Arc<WebState>>,
    connection: Option<Extension<Connection>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Access::Tokens { viewers, failures } = &state.access else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let source = source_of_request(&connection);
    if let Some(wait) = failures.wait(source) {
        return too_many(wait);
    }
    let Some(fields) = form_fields(&headers, &body) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Send application/x-www-form-urlencoded fields",
        );
    };
    let exchanged = match field(&fields, "grant_type") {
        Some(GRANT_TYPE) => {
            let Some(code) = field(&fields, "pre-authorized_code") else {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "pre-authorized_code is missing",
                );
            };
            viewers.exchange(code).await
        }
        Some(REFRESH_GRANT) => {
            let Some(token) = field(&fields, "refresh_token") else {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "refresh_token is missing",
                );
            };
            viewers.refresh(token).await
        }
        Some(_) => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "unsupported_grant_type",
                "Only the pre-authorized code grant and the refresh token grant are supported",
            );
        }
        None => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "grant_type is missing",
            );
        }
    };
    match exchanged {
        Ok(grant) => {
            failures.succeeded(source);
            (
                [
                    (header::CACHE_CONTROL, "no-store"),
                    (header::PRAGMA, "no-cache"),
                ],
                Json(TokenResponse {
                    access_token: grant.token.expose().to_owned(),
                    refresh_token: grant.refresh_token.map(|token| token.expose().to_owned()),
                    token_type: "Bearer",
                    expires_in: grant.expires_in.num_seconds(),
                    scope: grant.scope.to_scope_string(),
                }),
            )
                .into_response()
        }
        Err(ExchangeError::InvalidGrant) => {
            failures.failed(source);
            oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_grant",
                "The code or refresh token is not good: it may have been used, or have run out",
            )
        }
        Err(ExchangeError::Failed(e)) => server_error("exchange the code of", e),
    }
}

/// Gives a token up, as the holder asks (RFC 7009).
#[utoipa::path(
    post,
    path = "/view/api/revoke",
    tag = "web",
    request_body(
        content = String,
        content_type = "application/x-www-form-urlencoded",
        description = "`token=` and the token to give up."
    ),
    responses(
        (status = 200, description = "Done, or there was nothing to do: a token that is not good, or not there, is answered the same, so that this can't be used to find out which exist."),
        (status = 400, description = "Not a form.", body = OAuthError)
    )
)]
async fn revoke_endpoint(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Access::Tokens { viewers, .. } = &state.access else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(fields) = form_fields(&headers, &body) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Send application/x-www-form-urlencoded fields",
        );
    };
    if let Some(token) = field(&fields, "token")
        && let Err(e) = viewers.revoke_token(token).await
    {
        return server_error("give up the token of", e);
    }
    (StatusCode::OK, [(header::CACHE_CONTROL, "no-store")]).into_response()
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
        }
    }

    fn only(names: &[&str]) -> WebSettings {
        WebSettings {
            open: true,
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

    // --- who may look: tokens ---------------------------------------------------------------------------------

    use super::super::testing::start_web_for;
    use crate::adapters::entropy::OsEntropy;
    use crate::adapters::viewer_registry::FileViewerRegistry;
    use crate::application::viewers::IssueRequest;

    fn needing_a_token() -> WebSettings {
        WebSettings {
            layouts: vec![],
            open: false,
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
    async fn without_a_token_the_page_is_served_and_the_rest_asks_for_one_without_naming_an_error()
    {
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
                text("document.getElementById('signin-title').textContent")?
                    .contains("needs a link"),
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
                text("document.getElementById('signin-title').textContent")?
                    .contains("does not work"),
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
    async fn a_refresh_token_used_again_after_it_was_replaced_ends_the_session_for_whoever_holds_it()
     {
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
    async fn the_description_has_the_refresh_token_in_a_token_response_and_the_grant_in_a_request()
    {
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
}
