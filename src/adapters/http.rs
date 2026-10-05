//! The HTTP client every provider adapter uses, so a slow or flaky upstream can't hang or
//! sink a render: requests time out, and transient failures are retried with a short backoff.

use std::time::Duration;

use anyhow::Context;
use reqwest::{Client, RequestBuilder, Response, StatusCode};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// For the whole request, including reading the body.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Extra attempts after the first.
    pub retries: u32,
    /// Wait before the first retry; doubled for each one after.
    pub backoff: Duration,
}

pub const DEFAULT_RETRY: RetryPolicy = RetryPolicy { retries: 2, backoff: Duration::from_millis(500) };

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

/// Sends the request, retrying on a timeout, a connection error, or a 408, 429 or 5xx answer.
/// Any other failing status (a bad key, a wrong URL) is not retried. Returns the response only if
/// its status is a success. Errors have the query string removed, since that is where keys go.
pub async fn send(request: RequestBuilder) -> anyhow::Result<Response> {
    send_with(request, DEFAULT_RETRY).await
}

pub async fn send_with(request: RequestBuilder, policy: RetryPolicy) -> anyhow::Result<Response> {
    let mut attempt = 0;
    loop {
        let this_try = request.try_clone().context("A request with a streaming body can't be retried")?;
        let outcome = this_try.send().await.and_then(Response::error_for_status);
        let error = match outcome {
            Ok(response) => return Ok(response),
            Err(error) => error,
        };
        if attempt >= policy.retries || !is_transient(&error) {
            return Err(strip_query(error).into());
        }
        let wait = policy.backoff * 2u32.pow(attempt);
        attempt += 1;
        log::warn!("Request failed ({}), retrying in {wait:?} ({attempt}/{})", strip_query(error), policy.retries);
        tokio::time::sleep(wait).await;
    }
}

fn is_transient(error: &reqwest::Error) -> bool {
    if error.is_timeout() || error.is_connect() {
        return true;
    }
    matches!(
        error.status(),
        Some(status) if status.is_server_error()
            || status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::REQUEST_TIMEOUT
    )
}

fn strip_query(mut error: reqwest::Error) -> reqwest::Error {
    if let Some(url) = error.url_mut() {
        url.set_query(None);
    }
    error
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;

    use super::*;

    const FAST: RetryPolicy = RetryPolicy { retries: 2, backoff: Duration::from_millis(5) };

    /// Answers with each status in turn (the last one repeats), counting the requests.
    async fn server(statuses: Vec<u16>) -> (String, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let state = (Arc::new(statuses), Arc::clone(&hits));
        let app = Router::new()
            .route(
                "/",
                get(|State((statuses, hits)): State<(Arc<Vec<u16>>, Arc<AtomicUsize>)>| async move {
                    let n = hits.fetch_add(1, Ordering::SeqCst);
                    let status = statuses[n.min(statuses.len() - 1)];
                    (StatusCode::from_u16(status).unwrap(), "body")
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
        let message = format!("{error:#}");
        assert!(message.contains("500"), "{message}");
        assert!(!message.contains("secret"), "{message}");
    }

    #[tokio::test]
    async fn a_client_error_is_not_retried() {
        let (url, hits) = server(vec![401]).await;
        assert!(send_with(Client::new().get(&url), FAST).await.is_err());
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
        let error = send_with(client.get(format!("http://{address}/")), FAST).await.unwrap_err();

        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        assert_eq!(accepted.load(Ordering::SeqCst), 3);
        assert!(format!("{error:#}").to_lowercase().contains("timed out"), "{error:#}");
    }

    #[tokio::test]
    async fn a_refused_connection_is_retried() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let started = std::time::Instant::now();
        assert!(send_with(Client::new().get(format!("http://{address}/")), FAST).await.is_err());
        // Two backoffs of 5ms and 10ms happened.
        assert!(started.elapsed() >= Duration::from_millis(15));
    }
}
