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

mod icons;
mod limiter;
mod pwa;
mod stream;
mod token;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
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
use self::pwa::{content_security_policy, manifest_json, shell_fingerprint};
use self::token::OAuthError;
use super::health::image_written_at;
use super::image::{etag_of, names};
use super::{Published, server_error};
use crate::adapters::source::PeerAddress;
use crate::application::plan::{Plan, PlanTiming, compute};
use crate::application::viewers::{AuthError, ExchangeError, Viewers};
use crate::domain::models::display::ImageFormat;
use crate::domain::models::layout::{Layout, LayoutName};
use crate::domain::models::viewer::LayoutScope;

const PAGE: &str = include_str!("view.html");
/// The service worker that keeps the page's shell.
const WORKER: &str = include_str!("sw.js");

/// What the realm of a challenge is called.
const REALM: &str = "home-display";
/// The grant a code is exchanged under, as the OAuth 2.0 "pre-authorized code" grant OpenID for Verifiable Credential
/// Issuance names it: a code made ahead of time by the party that decides (here the owner), traded for a token.
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:pre-authorized_code";
/// The grant a refresh token is used under (RFC 6749 section 6).
const REFRESH_GRANT: &str = "refresh_token";
/// How long after a render is due the page looks again for it, and how often while it is not there yet. A display is told to
/// come later (`wake_delay`) because it has a radio to start and a panel to refresh; a browser is already awake, so it need
/// not, and a picture then shows a few seconds after it is drawn and not half a minute.
const WEB_WAKE_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
/// A token request is two short fields.
const MAX_FORM: usize = 2048;

/// How long a stream of changes (`/view/api/events`) goes on, and how often it looks again when nothing has told it to. A
/// stream is ended after a while and the page opens another at once, as server-sent events are meant to be used: no proxy, and
/// no connection limit of the server's own (the HTTPS side ends a connection after two minutes), has to be waited out, and a
/// token that has since been revoked or has run out is found out within a moment.
#[derive(Debug, Clone, Copy)]
pub struct StreamTiming {
    pub lifetime: std::time::Duration,
    pub tick: std::time::Duration,
    /// How many streams may be open at once, in all.
    pub most: usize,
    /// How many may be open from one source (as the HTTPS side counts one: a whole IPv6 /64). A stream holds one of the
    /// connections a source may have, so this leaves the others for everything else it asks for.
    pub most_per_source: usize,
}

impl Default for StreamTiming {
    fn default() -> Self {
        Self {
            lifetime: std::time::Duration::from_secs(90),
            tick: std::time::Duration::from_secs(20),
            most: 8,
            most_per_source: 2,
        }
    }
}

/// Who the page and its API are for.
#[derive(Debug, Clone, Default)]
pub struct WebSettings {
    /// The layouts the page may show, in the order it lists them. Empty means all of them.
    pub layouts: Vec<LayoutName>,
    /// Whether anyone who can reach the server may use it. Otherwise a viewer token is needed.
    pub open: bool,
    pub stream: StreamTiming,
    /// Proxies in front of the server whose `X-Forwarded-For` is believed, for telling clients apart: failed sign-ins are
    /// counted, and streams limited, by client. Without one the address that connected is the client, which behind a proxy is
    /// the proxy, for everyone.
    pub trusted_proxies: Vec<IpAddr>,
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
    /// The icon in each size it is served in.
    icons: Vec<(u32, Vec<u8>)>,
    /// The service worker, its cache named for what it keeps (see `shell_fingerprint`).
    worker: String,
    stream: StreamTiming,
    /// How many streams are open, in all and from each source.
    streams: Arc<limiter::StreamBudget>,
    access: Access,
    proxies: Vec<IpAddr>,
}

/// Who is asking, once they have shown a token: what they may look at. Put on the request by `authenticate`.
#[derive(Clone)]
struct Principal {
    scope: LayoutScope,
    /// What was shown, so that a stream that goes on for a while can check it is still good.
    token: String,
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
    pub(super) fn new(
        published: Arc<Published>,
        allowed: Vec<Layout>,
        stream: StreamTiming,
        access: Access,
        proxies: Vec<IpAddr>,
    ) -> Self {
        assert!(!allowed.is_empty(), "the page has a layout to show");
        let icons: Vec<(u32, Vec<u8>)> = icons::SIZES
            .iter()
            .map(|size| (*size, icons::png(*size)))
            .collect();
        let fingerprint = shell_fingerprint(PAGE, &manifest_json(), &icons, icons::SVG, WORKER);
        Self {
            published,
            allowed,
            policy: HeaderValue::from_str(&content_security_policy(PAGE))
                .expect("a policy is a header value"),
            worker: WORKER.replace("__SHELL__", &fingerprint),
            icons,
            stream,
            streams: limiter::StreamBudget::new(stream.most, stream.most_per_source),
            access,
            proxies,
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
        self.automatic_at(scope, self.published.clock.now())
    }

    /// What `automatic` is, or will be, at `at`.
    fn automatic_at(
        &self,
        scope: &LayoutScope,
        at: chrono::DateTime<chrono_tz::Tz>,
    ) -> Result<&Layout, Refusal> {
        let permitted = self.permitted(scope);
        let current = self.published.layouts.current(at);
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

    /// How long until `automatic` is another layout than `now`'s, if the schedule has it change within a week.
    fn automatic_changes_in(&self, scope: &LayoutScope) -> Option<u64> {
        let now = self.published.clock.now();
        let current = self.automatic_at(scope, now).ok()?.name.clone();
        self.published
            .layouts
            .schedule()
            .boundaries(now, 8)
            .into_iter()
            .find(|moment| {
                self.automatic_at(scope, *moment)
                    .is_ok_and(|layout| layout.name != current)
            })
            .map(|moment| u64::try_from((moment - now).num_seconds()).unwrap_or(0))
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
fn source_of_request(
    peer: &Option<Extension<PeerAddress>>,
    headers: &HeaderMap,
    proxies: &[IpAddr],
) -> IpAddr {
    client_of(peer, headers, proxies).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

/// Where a request came from, if the connection says: the peer of the connection, or, when that is a proxy the owner trusts
/// (`WebSettings::trusted_proxies`), the client it says it is passing on (see `forwarded_for`). Over plain HTTP nothing says.
fn client_of(
    peer: &Option<Extension<PeerAddress>>,
    headers: &HeaderMap,
    proxies: &[IpAddr],
) -> Option<IpAddr> {
    let peer = peer.as_ref().map(|Extension(peer)| peer.0.ip())?;
    Some(forwarded_for(peer, headers, proxies))
}

/// The client behind `peer`. A peer that isn't a trusted proxy is the client, whatever it claims in `X-Forwarded-For` (anyone
/// can write one). A trusted proxy appends the address it was reached from to the header, so the list is read from the end,
/// past the proxies that are trusted, and the first address that isn't one is the client; the entries before it were written
/// by whoever it was and are not believed. A header that is not made of addresses is not believed at all.
fn forwarded_for(peer: IpAddr, headers: &HeaderMap, proxies: &[IpAddr]) -> IpAddr {
    let trusted = |address: IpAddr| {
        let address = limiter::exact(address);
        proxies
            .iter()
            .any(|proxy| limiter::exact(*proxy) == address)
    };
    if !trusted(peer) {
        return peer;
    }
    let entries: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect();
    for entry in entries.into_iter().rev() {
        match entry.parse::<IpAddr>() {
            Ok(address) if trusted(address) => continue,
            Ok(address) => return address,
            Err(_) => return peer,
        }
    }
    peer
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
    peer: Option<Extension<PeerAddress>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Access::Tokens { viewers, failures } = &state.access else {
        return next.run(request).await;
    };
    let source = source_of_request(&peer, request.headers(), &state.proxies);
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
            request.extensions_mut().insert(Principal {
                scope: who.scope,
                token,
            });
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

/// The page, and what a browser needs to put it on a home screen. None of it is private, so it is for anyone, and a browser
/// asks for it without the token.
fn open_routes() -> OpenApiRouter<Arc<WebState>> {
    OpenApiRouter::with_openapi(WebDoc::openapi())
        .routes(routes!(pwa::page))
        .routes(routes!(pwa::manifest))
        .routes(routes!(pwa::icon_svg))
        .routes(routes!(pwa::icon_png))
        .routes(routes!(pwa::worker))
}

/// What gives the picture away: behind `authenticate`.
fn protected_routes() -> OpenApiRouter<Arc<WebState>> {
    OpenApiRouter::new()
        .routes(routes!(layouts))
        .routes(routes!(state))
        .routes(routes!(stream::events))
        .routes(routes!(image))
}

/// Getting a token, and giving it up: there only when tokens are how the page is let in.
fn token_routes() -> OpenApiRouter<Arc<WebState>> {
    OpenApiRouter::new()
        .routes(routes!(token::token_endpoint))
        .routes(routes!(token::revoke_endpoint))
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
    /// For a request that was for `auto` (or for nothing): how many seconds until the layout `auto` means changes, because the
    /// schedule says so, if it does within a week. Absent for a layout asked for by name. The page looks again then, so that a
    /// change of layout at 06:30 is on screen at 06:30 and not whenever it next happens to ask.
    #[schema(examples(3600))]
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_changes_in_seconds: Option<u64>,
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
    match view_state(
        &state,
        &scope_of(principal),
        which.layout.as_deref(),
        have.have,
    )
    .await
    {
        Ok(view) => ([(header::CACHE_CONTROL, "no-store")], Json(view)).into_response(),
        Err(refusal) => *refusal,
    }
}

/// The state of the picture of the layout asked for (`auto`, or none, is the one for now), or the answer to give if there is
/// none to say anything of.
async fn view_state(
    state: &WebState,
    scope: &LayoutScope,
    wanted: Option<&str>,
    have: Option<u32>,
) -> Result<ViewState, Box<Response>> {
    let layout = state
        .resolve(scope, wanted)
        .map_err(IntoResponse::into_response)?;
    let automatic = wanted.is_none_or(|name| name == "auto");
    let published = &state.published;
    let rendered_at = match image_written_at(published, layout).await {
        Ok(Some(rendered_at)) => rendered_at,
        Ok(None) => {
            return Err(Box::new(
                (StatusCode::NOT_FOUND, "No image has been rendered yet").into_response(),
            ));
        }
        Err(e) => return Err(Box::new(server_error("inspect", e))),
    };
    let plan = compute(
        published.clock.now(),
        rendered_at,
        &published.schedule,
        PlanTiming {
            wake_delay: WEB_WAKE_DELAY,
            ..published.timing
        },
        have,
    )
    .map_err(|e| Box::new(server_error("plan", e)))?;
    Ok(ViewState {
        layout: layout.name.to_string(),
        auto_changes_in_seconds: if automatic {
            state.automatic_changes_in(scope)
        } else {
            None
        },
        plan,
    })
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

#[cfg(test)]
mod tests;
