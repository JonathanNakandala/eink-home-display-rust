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

use super::api::{ApiError, ApproveRequest, DisplayEntry, DisplayList, OpenWindow, WindowState};
use super::transport;
use crate::domain::models::device_id::DeviceId;

/// Why a call failed: the server could not be reached, or it answered that the request was no good.
#[derive(Debug)]
pub enum CallError {
    /// No answer: the server is not running, or not offering the admin interface, or this user may not use it.
    Unreachable(anyhow::Error),
    /// The server answered with an error.
    Refused { status: StatusCode, error: ApiError },
    /// The server answered with something this client does not understand.
    Unexpected(anyhow::Error),
    /// The request was not sent, because what it was given can't be right (a name that can't be a display's).
    Invalid(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(e) => write!(f, "could not reach the server: {e:#}"),
            Self::Refused { error, .. } => write!(f, "{}", error.error.message),
            Self::Unexpected(e) => write!(f, "unexpected answer: {e:#}"),
            Self::Invalid(message) => write!(f, "{message}"),
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

    /// The path of a display, after checking `name` can be one (so a typo is caught here, with a clear message).
    fn display_path(name: &str, tail: &str) -> Result<String, CallError> {
        let device = DeviceId::parse(name)
            .map_err(|e| CallError::Invalid(format!("{name:?} is not a display name: {e}")))?;
        Ok(format!("/v1/displays/{device}{tail}"))
    }

    pub async fn displays(&self) -> Result<DisplayList, CallError> {
        self.call(Method::GET, "/v1/displays", None::<&()>).await
    }

    pub async fn display(&self, name: &str) -> Result<DisplayEntry, CallError> {
        self.call(Method::GET, &Self::display_path(name, "")?, None::<&()>)
            .await
    }

    /// Approves `name` with the code the owner read off its own panel.
    pub async fn approve(&self, name: &str, code: &str) -> Result<DisplayEntry, CallError> {
        let body = ApproveRequest {
            code: code.to_owned(),
        };
        self.call(
            Method::POST,
            &Self::display_path(name, "/approve")?,
            Some(&body),
        )
        .await
    }

    pub async fn reject(&self, name: &str) -> Result<DisplayEntry, CallError> {
        self.call(
            Method::POST,
            &Self::display_path(name, "/reject")?,
            None::<&()>,
        )
        .await
    }

    pub async fn revoke(&self, name: &str) -> Result<DisplayEntry, CallError> {
        self.call(
            Method::POST,
            &Self::display_path(name, "/revoke")?,
            None::<&()>,
        )
        .await
    }

    pub async fn forget(&self, name: &str) -> Result<DisplayEntry, CallError> {
        self.call(Method::DELETE, &Self::display_path(name, "")?, None::<&()>)
            .await
    }
}

/// For a caller that wants a plain failure with the status in the message.
pub fn describe(error: &CallError) -> String {
    match error {
        CallError::Refused { status, error } => format!("{status}: {}", error.error.message),
        other => other.to_string(),
    }
}
