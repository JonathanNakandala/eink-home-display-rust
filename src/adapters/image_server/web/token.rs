//! Getting a token (RFC 6749) and giving it up (RFC 7009).

use super::*;

/// An error from the token endpoint, as RFC 6749 section 5.2 has it.
#[derive(Serialize, ToSchema)]
pub(super) struct OAuthError {
    /// `invalid_request`, `invalid_grant`, `unsupported_grant_type` or `temporarily_unavailable`.
    #[schema(examples("invalid_grant"))]
    pub(super) error: &'static str,
    pub(super) error_description: &'static str,
}

impl OAuthError {
    pub(super) fn new(error: &'static str, error_description: &'static str) -> Self {
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
pub(super) async fn token_endpoint(
    State(state): State<Arc<WebState>>,
    peer: Option<Extension<PeerAddress>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Access::Tokens { viewers, failures } = &state.access else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let source = source_of_request(&peer, &headers, &state.proxies);
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
pub(super) async fn revoke_endpoint(
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
