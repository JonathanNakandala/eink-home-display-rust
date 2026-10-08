//! Who may use the display routes (`/image`, `/plan`, ...) over HTTPS, and who they are taken to be.
//!
//! The EST routes sit outside this: a display that has not joined has to be able to reach them. Everything
//! else passes through here. A display shows a certificate during the handshake; TLS has already checked
//! it was issued by the authority and has not expired, and this checks the one thing a certificate can't
//! say, that the display is still a member (it may have been revoked, or replaced by another under the
//! same name). That is asked on every request, so revoking takes effect at once and not when the
//! certificate runs out.
//!
//! A member is put on the request as `AuthenticatedDevice`, which is what the routes use for who is
//! asking. A certificate that is shown and refused is refused whatever the access: showing a bad one is
//! never better than showing none.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};

use super::routes::Connection;
use crate::adapters::authenticated::AuthenticatedDevice;
use crate::application::enrollment::Enrollment;

/// What a request needs to be served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Anyone. A good certificate names the caller; none leaves the caller unnamed (to be named by the
    /// request, as over HTTP).
    Open,
    /// Only a member, shown by a certificate.
    Members,
}

struct Gate {
    enrollment: Arc<Enrollment>,
    access: Access,
}

/// `routes` with the gate in front of all of them.
pub fn guarded(routes: Router, enrollment: Arc<Enrollment>, access: Access) -> Router {
    routes.layer(middleware::from_fn_with_state(
        Arc::new(Gate { enrollment, access }),
        check,
    ))
}

fn refuse(message: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        [(CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!("{message}\n"),
    )
        .into_response()
}

async fn check(
    State(gate): State<Arc<Gate>>,
    axum::Extension(connection): axum::Extension<Connection>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(client) = connection.client else {
        return match gate.access {
            Access::Open => next.run(request).await,
            Access::Members => refuse("A certificate from the authority is needed"),
        };
    };
    match gate
        .enrollment
        .authenticate(&client.device, &client.key)
        .await
    {
        Ok(true) => {
            request
                .extensions_mut()
                .insert(AuthenticatedDevice(client.device));
            next.run(request).await
        }
        Ok(false) => {
            log::warn!(
                "Turned away {} ({}): its certificate is valid but it is not a member",
                client.device,
                connection
                    .peer
                    .map_or_else(|| "an unknown address".to_owned(), |p| p.ip().to_string())
            );
            refuse("This display is not a member")
        }
        Err(e) => {
            log::error!(
                "Failed to check whether {} is a member: {e:#}",
                client.device
            );
            (StatusCode::INTERNAL_SERVER_ERROR, "The server failed\n").into_response()
        }
    }
}
