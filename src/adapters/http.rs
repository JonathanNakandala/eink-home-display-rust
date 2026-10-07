//! The HTTP client every provider adapter uses, so a slow or flaky upstream can't hang or
//! sink a render: requests time out, and transient failures are retried with a short backoff.

use std::time::{Duration, SystemTime};

use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;

use crate::domain::models::source_error::SourceError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// For the whole request, including reading the body.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Extra attempts after the first.
    pub retries: u32,
    /// Wait before the first retry; doubled for each one after.
    pub backoff: Duration,
    /// The longest a server's `Retry-After` is waited out. Asked to wait longer than this, the request
    /// fails at once: a render can't afford to sleep through it, and the next one will try again.
    pub max_retry_after: Duration,
}

pub const DEFAULT_RETRY: RetryPolicy = RetryPolicy {
    retries: 2,
    backoff: Duration::from_millis(500),
    max_retry_after: Duration::from_secs(10),
};

pub fn client() -> Client {
    client_with_timeouts(CONNECT_TIMEOUT, REQUEST_TIMEOUT)
}

fn client_with_timeouts(connect: Duration, request: Duration) -> Client {
    Client::builder()
        .connect_timeout(connect)
        .timeout(request)
        .build()
        .expect("an HTTP client with only timeouts set builds")
}

/// Sends the request, retrying on a transient failure (see `SourceError::is_transient`): a timeout,
/// a connection error, a 408, 429 or 5xx answer. A retry waits for the backoff, or for the server's
/// `Retry-After` if that is longer (and not longer than the policy allows). Any other failing status
/// (a bad key, a wrong URL) is not retried. Returns the response only if its status is a success. Errors have the query string
/// removed, since that is where keys go.
pub async fn send(request: RequestBuilder) -> Result<Response, SourceError> {
    send_with(request, DEFAULT_RETRY).await
}

pub async fn send_with(
    request: RequestBuilder,
    policy: RetryPolicy,
) -> Result<Response, SourceError> {
    let mut attempt = 0;
    loop {
        // Every request made here is a plain GET, which can always be copied.
        let this_try = request
            .try_clone()
            .expect("provider requests have no streaming body");
        let (error, retry_after) = match this_try.send().await {
            Ok(response) if !is_failure(response.status()) => return Ok(response),
            Ok(response) => (
                from_status(response.status()),
                retry_after(response.headers(), SystemTime::now()),
            ),
            Err(error) => (classify(error), None),
        };
        if attempt >= policy.retries || !error.is_transient() {
            return Err(error);
        }
        if let Some(asked) = retry_after
            && asked > policy.max_retry_after
        {
            log::warn!(
                "Request failed ({error}); the server asked for {asked:?}, too long to wait"
            );
            return Err(error);
        }
        let backoff = policy.backoff * 2u32.pow(attempt);
        let wait = retry_after.map_or(backoff, |asked| asked.max(backoff));
        attempt += 1;
        log::warn!(
            "Request failed ({error}), retrying in {wait:?} ({attempt}/{})",
            policy.retries
        );
        tokio::time::sleep(wait).await;
    }
}

/// A client or server error answer; anything else (including a redirect, which reqwest follows) is a success.
fn is_failure(status: StatusCode) -> bool {
    status.is_client_error() || status.is_server_error()
}

/// How long the server asked us to wait: `Retry-After` as a number of seconds or an HTTP date.
fn retry_after(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    Some(date.duration_since(now).unwrap_or_default())
}

/// Reads the body of a successful response as JSON. `what` names it for the error, e.g. "forecast".
pub async fn json<T: DeserializeOwned>(response: Response, what: &str) -> Result<T, SourceError> {
    response.json().await.map_err(|error| {
        if error.is_decode() {
            SourceError::bad_response_from(format!("could not read the {what}"), strip_query(error))
        } else {
            classify(error)
        }
    })
}

fn classify(error: reqwest::Error) -> SourceError {
    let error = strip_query(error);
    if error.is_timeout() {
        return SourceError::Timeout;
    }
    if let Some(status) = error.status() {
        return from_status(status);
    }
    if error.is_decode() {
        return SourceError::bad_response_from("could not read the response", error);
    }
    SourceError::Unreachable(Box::new(error))
}

fn from_status(status: StatusCode) -> SourceError {
    let code = status.as_u16();
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            SourceError::Unauthorized { status: code }
        }
        StatusCode::TOO_MANY_REQUESTS => SourceError::RateLimited,
        StatusCode::REQUEST_TIMEOUT => SourceError::Timeout,
        _ if status.is_server_error() => SourceError::Upstream { status: code },
        _ => SourceError::Rejected { status: code },
    }
}

fn strip_query(mut error: reqwest::Error) -> reqwest::Error {
    if let Some(url) = error.url_mut() {
        url.set_query(None);
    }
    error
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::get;

    use super::*;

    const FAST: RetryPolicy = RetryPolicy {
        retries: 2,
        backoff: Duration::from_millis(5),
        max_retry_after: Duration::from_secs(10),
    };

    struct Canned {
        statuses: Vec<u16>,
        hits: Arc<AtomicUsize>,
        header: Option<&'static str>,
    }

    /// Answers with each status in turn (the last one repeats), counting the requests.
    async fn server(statuses: Vec<u16>) -> (String, Arc<AtomicUsize>) {
        server_with_retry_after(statuses, None).await
    }

    /// Like `server`, with a `Retry-After` header on every failing answer.
    async fn server_with_retry_after(
        statuses: Vec<u16>,
        header: Option<&'static str>,
    ) -> (String, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(Canned {
            statuses,
            hits: Arc::clone(&hits),
            header,
        });
        let app = Router::new()
            .route(
                "/",
                get(|State(canned): State<Arc<Canned>>| async move {
                    let n = canned.hits.fetch_add(1, Ordering::SeqCst);
                    let status = canned.statuses[n.min(canned.statuses.len() - 1)];
                    let mut headers = axum::http::HeaderMap::new();
                    if let (Some(value), true) = (canned.header, status >= 400) {
                        headers.insert(RETRY_AFTER, value.parse().unwrap());
                    }
                    (StatusCode::from_u16(status).unwrap(), headers, "body")
                }),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{address}/?appid=secret"), hits)
    }

    #[tokio::test]
    async fn a_transient_failure_is_retried_until_it_works() {
        let (url, hits) = server(vec![503, 429, 200]).await;
        let response = send_with(Client::new().get(&url), FAST).await.unwrap();
        assert_eq!(response.text().await.unwrap(), "body");
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn gives_up_after_the_retries_and_hides_the_key() {
        let (url, hits) = server(vec![500]).await;
        let error = send_with(Client::new().get(&url), FAST).await.unwrap_err();
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        assert!(
            matches!(error, SourceError::Upstream { status: 500 }),
            "{error:?}"
        );
        assert!(!format!("{error:?}").contains("secret"), "{error:?}");
    }

    #[tokio::test]
    async fn a_client_error_is_not_retried() {
        let (url, hits) = server(vec![401]).await;
        let error = send_with(Client::new().get(&url), FAST).await.unwrap_err();
        assert!(
            matches!(error, SourceError::Unauthorized { status: 401 }),
            "{error:?}"
        );
        assert!(error.needs_attention());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_server_that_never_answers_times_out_and_is_retried() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&accepted);
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                held.push(socket); // accept and say nothing
            }
        });

        let client = client_with_timeouts(Duration::from_secs(1), Duration::from_millis(100));
        let started = std::time::Instant::now();
        let error = send_with(client.get(format!("http://{address}/")), FAST)
            .await
            .unwrap_err();

        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(accepted.load(Ordering::SeqCst), 3);
        assert!(matches!(error, SourceError::Timeout), "{error:?}");
    }

    #[tokio::test]
    async fn a_refused_connection_is_retried() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let started = std::time::Instant::now();
        let error = send_with(Client::new().get(format!("http://{address}/")), FAST)
            .await
            .unwrap_err();
        assert!(matches!(error, SourceError::Unreachable(_)), "{error:?}");
        // Two backoffs of 5ms and 10ms happened.
        assert!(started.elapsed() >= Duration::from_millis(15));
    }

    #[tokio::test]
    async fn statuses_are_classified_by_what_a_caller_can_do_about_them() {
        for (status, retried, expected) in [
            (403, false, "key rejected"),
            (404, false, "request rejected"),
            (429, true, "rate limited"),
            (502, true, "service error"),
        ] {
            let (url, hits) = server(vec![status]).await;
            let error = send_with(Client::new().get(&url), FAST).await.unwrap_err();
            assert_eq!(error.reason(), expected, "{status}");
            assert_eq!(
                hits.load(Ordering::SeqCst),
                if retried { 3 } else { 1 },
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn a_body_that_is_not_the_expected_json_is_a_bad_response_and_is_not_retried() {
        let (url, hits) = server(vec![200]).await; // answers "body", which isn't JSON
        let response = send_with(Client::new().get(&url), FAST).await.unwrap();
        let error = json::<serde_json::Value>(response, "forecast")
            .await
            .unwrap_err();
        assert!(
            matches!(error, SourceError::BadResponse { .. }),
            "{error:?}"
        );
        assert!(
            error.to_string().contains("could not read the forecast"),
            "{error}"
        );
        assert!(!error.is_transient());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_retry_waits_for_the_servers_retry_after() {
        let (url, hits) = server_with_retry_after(vec![429, 200], Some("1")).await;
        let started = std::time::Instant::now();
        send_with(Client::new().get(&url), FAST).await.unwrap();
        // The normal backoff here is 5ms; the server asked for a second.
        assert!(
            started.elapsed() >= Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_retry_after_longer_than_allowed_fails_at_once() {
        let (url, hits) = server_with_retry_after(vec![429, 200], Some("3600")).await;
        let started = std::time::Instant::now();
        let error = send_with(Client::new().get(&url), FAST).await.unwrap_err();
        assert!(matches!(error, SourceError::RateLimited), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn retry_after_reads_seconds_and_http_dates() {
        let now = httpdate::parse_http_date("Wed, 21 Oct 2026 07:28:00 GMT").unwrap();
        let header = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(RETRY_AFTER, value.parse().unwrap());
            retry_after(&headers, now)
        };
        assert_eq!(header("120"), Some(Duration::from_secs(120)));
        assert_eq!(header(" 3 "), Some(Duration::from_secs(3)));
        assert_eq!(
            header("Wed, 21 Oct 2026 07:28:30 GMT"),
            Some(Duration::from_secs(30))
        );
        // A date already past means no wait.
        assert_eq!(
            header("Wed, 21 Oct 2026 07:00:00 GMT"),
            Some(Duration::ZERO)
        );
        assert_eq!(header("soon"), None);
        assert_eq!(retry_after(&HeaderMap::new(), now), None);
    }
}
