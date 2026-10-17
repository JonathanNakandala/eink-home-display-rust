//! The stream of changes (`/view/api/events`): server-sent events that tell the page of a new picture as it is drawn.

use super::*;

/// What the page needs to know has changed: which layout it is looking at, which render of it, and whether it is late or due.
/// A stream says so when this is not what it said last.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    layout: String,
    version: u32,
    stale: bool,
    pending: bool,
}

impl Fingerprint {
    fn of(view: &ViewState) -> Self {
        Self {
            layout: view.layout.clone(),
            version: view.plan.version,
            stale: view.plan.stale,
            pending: view.plan.pending,
        }
    }
}

/// What one event says: the same facts as `/view/api/state`, less what changes with the time.
fn event_of(view: &ViewState) -> String {
    let data = serde_json::json!({
        "layout": view.layout,
        "version": view.plan.version,
        "stale": view.plan.stale,
        "pending": view.plan.pending,
        "next_seconds": view.plan.next_seconds,
    });
    format!("event: state\ndata: {data}\n\n")
}

/// Tells the page when the picture it is looking at changes, as it happens, so that it need not ask on a timer.
///
/// `text/event-stream`: an event is sent at the start, and again whenever a render finishes that changes the picture, the late
/// or due state, or what `auto` means. Between, a comment line is sent now and then so that nothing on the way closes it. It
/// ends after a minute and a half (`event: bye`), and the page opens another at once; a token that has been revoked or has run
/// out in the meantime ends it at the next look. The page still asks now and then, so a stream that fails costs only speed.
#[utoipa::path(
    get,
    path = "/view/api/events",
    tag = "web",
    security((), ("bearerAuth" = [])),
    params(WhichLayout),
    responses(
        (
            status = 200,
            description = "A stream of events: `state` with the layout, the render's `version`, `stale`, `pending` and `next_seconds` whenever any of the first four changes, `: keepalive` comments, and a last `bye` when the stream ends and should be opened again. Send the token as `Authorization`, which a browser's `EventSource` cannot, so read it with `fetch`.",
            content_type = "text/event-stream",
            body = String,
            headers(("cache-control" = String, description = "Always `no-store`."))
        ),
        (status = 401, description = "A viewer token is needed, or the one shown is not good.", body = String, content_type = "text/plain"),
        (status = 403, description = "The token does not allow that layout (`insufficient_scope`).", body = String, content_type = "text/plain"),
        (status = 404, description = "Nothing has been rendered yet, or there is no such layout.", body = String, content_type = "text/plain"),
        (status = 429, description = "Too many requests that were not good from this source.", body = OAuthError),
        (status = 503, description = "As many streams are open as there may be, in all or from this source; `Retry-After` says when to ask again. Everything else is still answered, so a page carries on asking for the state.", body = String, content_type = "text/plain")
    )
)]
pub(super) async fn events(
    State(state): State<Arc<WebState>>,
    principal: Option<Extension<Principal>>,
    peer: Option<Extension<PeerAddress>>,
    headers: HeaderMap,
    Query(which): Query<WhichLayout>,
) -> Response {
    let (scope, token) = match principal {
        Some(Extension(principal)) => (principal.scope, Some(principal.token)),
        None => (LayoutScope::All, None),
    };
    // Refused as `state` would, before anything is streamed: a stream that can't start says why in a status.
    let first = match view_state(&state, &scope, which.layout.as_deref(), None).await {
        Ok(first) => first,
        Err(refusal) => return *refusal,
    };
    // A place for it, or not: a stream holds one of the connections the server has, and a source has only so many. The page
    // carries on asking on its own without one, and tries again after a wait.
    let Some(permit) = state
        .streams
        .acquire(client_of(&peer, &headers, &state.proxies))
    else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "30")],
            "There are as many streams open as there may be; ask again later",
        )
            .into_response();
    };
    let wanted = which.layout;
    let (sender, receiver) =
        tokio::sync::mpsc::channel::<Result<Bytes, std::convert::Infallible>>(4);
    tokio::spawn(stream_changes(
        state, scope, token, wanted, first, sender, permit,
    ));
    let body = axum::body::Body::from_stream(futures_util::stream::unfold(
        receiver,
        |mut receiver| async move { receiver.recv().await.map(|item| (item, receiver)) },
    ));
    (
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-store"),
            // Not held up by a proxy that waits to fill a buffer (nginx honours this).
            (HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        body,
    )
        .into_response()
}

/// Sends events down `sender` until the stream's time is up, the page has gone, or the token is no longer good.
async fn stream_changes(
    state: Arc<WebState>,
    scope: LayoutScope,
    token: Option<String>,
    wanted: Option<String>,
    first: ViewState,
    sender: tokio::sync::mpsc::Sender<Result<Bytes, std::convert::Infallible>>,
    // Held for as long as the stream goes on, and given back when this returns, however it does.
    _permit: limiter::StreamPermit,
) {
    let mut finishes = state.published.handles.refresh.finishes();
    let ends = tokio::time::Instant::now() + state.stream.lifetime;
    let mut last = Fingerprint::of(&first);
    let mut view = first;
    // `retry` is how soon a browser's own reconnecting would try again, which the page does itself but a plain client may rely on.
    let opening = format!("retry: 2000\n\n{}", event_of(&view));
    if sender.send(Ok(Bytes::from(opening))).await.is_err() {
        return;
    }
    loop {
        // When to look again: when told a render finished, or when the picture is due or the schedule changes what auto is,
        // or after a tick, whichever is first, and never past the stream's end.
        let mut wait = state.stream.tick;
        wait = wait.min(std::time::Duration::from_secs(
            view.plan.next_seconds.saturating_add(1),
        ));
        if let Some(changes) = view.auto_changes_in_seconds {
            wait = wait.min(std::time::Duration::from_secs(changes.saturating_add(1)));
        }
        let wake = (tokio::time::Instant::now() + wait).min(ends);
        tokio::select! {
            _ = finishes.changed() => {}
            _ = tokio::time::sleep_until(wake) => {}
            _ = sender.closed() => return,
        }
        if tokio::time::Instant::now() >= ends {
            let _ = sender
                .send(Ok(Bytes::from("event: bye\ndata: {}\n\n")))
                .await;
            return;
        }
        // Still good? A revoked token, or one that has run out, ends the stream, and the page finds out when it opens another.
        if let (Some(token), Access::Tokens { viewers, .. }) = (&token, &state.access)
            && viewers.authenticate(token).await.is_err()
        {
            let _ = sender
                .send(Ok(Bytes::from("event: bye\ndata: {}\n\n")))
                .await;
            return;
        }
        let message = match view_state(&state, &scope, wanted.as_deref(), None).await {
            Ok(now) => {
                let fingerprint = Fingerprint::of(&now);
                view = now;
                if fingerprint != last {
                    last = fingerprint;
                    event_of(&view)
                } else {
                    ": keepalive\n\n".to_owned()
                }
            }
            // Gone since (the picture, the layout, the permission): the page is told to start again and find out why.
            Err(_) => "event: bye\ndata: {}\n\n".to_owned(),
        };
        let done = message.starts_with("event: bye");
        if sender.send(Ok(Bytes::from(message))).await.is_err() || done {
            return;
        }
    }
}
