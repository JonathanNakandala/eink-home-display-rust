//! The other end of the admin socket: what the command line uses to talk to a running server.

use std::path::{Path, PathBuf};

use anyhow::Context;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::api::{ApiError, OpenWindow, WindowState};
use super::transport;

/// Why a call failed: the server could not be reached, or it answered that the request was no good.
#[derive(Debug)]
pub enum CallError {
    /// No answer: the server is not running, or not offering the admin interface, or this user may not use it.
    Unreachable(anyhow::Error),
    /// The server answered with an error.
    Refused { status: StatusCode, error: ApiError },
    /// The server answered with something this client does not understand.
    Unexpected(anyhow::Error),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(e) => write!(f, "could not reach the server: {e:#}"),
            Self::Refused { error, .. } => write!(f, "{}", error.error.message),
            Self::Unexpected(e) => write!(f, "unexpected answer: {e:#}"),
        }
    }
}

impl std::error::Error for CallError {}

pub struct AdminClient {
    socket: PathBuf,
}

impl AdminClient {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// One request and its answer. A new connection each time: the calls are rare, and it keeps this simple.
    async fn call<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, CallError> {
        let stream = transport::connect(&self.socket)
            .await
            .map_err(CallError::Unreachable)?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .context("The handshake failed")
            .map_err(CallError::Unreachable)?;
        let driver = tokio::spawn(connection);
        let payload = match body {
            Some(body) => serde_json::to_vec(body)
                .context("Failed to encode the request")
                .map_err(CallError::Unexpected)?,
            None => Vec::new(),
        };
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, "localhost")
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(payload)))
            .context("Failed to build the request")
            .map_err(CallError::Unexpected)?;
        let result = async {
            let response = sender.send_request(request).await?;
            let status = response.status();
            let bytes = response.into_body().collect().await?.to_bytes();
            anyhow::Ok((status, bytes))
        }
        .await;
        driver.abort();
        let (status, bytes) = result
            .context("The request failed")
            .map_err(CallError::Unreachable)?;
        if status.is_success() {
            return serde_json::from_slice(&bytes)
                .context("The answer is not what was expected")
                .map_err(CallError::Unexpected);
        }
        match serde_json::from_slice::<ApiError>(&bytes) {
            Ok(error) => Err(CallError::Refused { status, error }),
            Err(_) => Err(CallError::Unexpected(anyhow::anyhow!(
                "{status} with {}",
                String::from_utf8_lossy(&bytes)
            ))),
        }
    }

    pub async fn window(&self) -> Result<WindowState, CallError> {
        self.call(Method::GET, "/v1/window", None::<&()>).await
    }

    pub async fn open_window(&self, minutes: u32) -> Result<WindowState, CallError> {
        self.call(Method::PUT, "/v1/window", Some(&OpenWindow { minutes }))
            .await
    }

    pub async fn close_window(&self) -> Result<WindowState, CallError> {
        self.call(Method::DELETE, "/v1/window", None::<&()>).await
    }
}

/// The state of the window in a sentence for a person, with times in `zone`.
pub fn describe_window<Z: chrono::TimeZone>(
    state: &WindowState,
    now: chrono::DateTime<chrono::Utc>,
    zone: &Z,
) -> String
where
    Z::Offset: std::fmt::Display,
{
    match state.closes_at {
        Some(closes) if state.open => {
            let left = (closes - now).num_seconds().max(0);
            let (minutes, seconds) = (left / 60, left % 60);
            format!(
                "The pairing window is open until {} ({minutes} min {seconds:02} s from now). A display that has not joined can ask now.",
                closes.with_timezone(zone).format("%H:%M:%S")
            )
        }
        _ => "The pairing window is closed. Open it with `displayctl window open` to let a display ask to join."
            .to_owned(),
    }
}

/// For a caller that wants a plain failure with the status in the message.
pub fn describe(error: &CallError) -> String {
    match error {
        CallError::Refused { status, error } => format!("{status}: {}", error.error.message),
        other => other.to_string(),
    }
}
