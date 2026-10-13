//! The admin API: how the owner of a running server looks at it and changes it, from the same machine.
//!
//! It is HTTP and JSON over a local socket (see `transport`), so anything that can speak HTTP to a socket can use
//! it, and `displayctl` is the one that comes with the program. It is described in `config/admin-openapi.json`,
//! which is made from the code, checked by a test, and also served at `/openapi.json` on the socket.
//!
//! Nothing here can be reached from the network, and no endpoint returns a secret that a display's membership turns on: a
//! pairing code, a key or a certificate never comes back, so nothing copied from here can stand in for what the display itself
//! shows. There is one deliberate exception, which is not that: `POST /v1/viewers` returns the one-time code of the viewer it
//! made, in the link for a browser, once and never again. It is what the owner asked for, to carry to the browser, and it
//! works once, for a short time. What is kept of it, and of the token it is traded for, is only a hash.

mod api;
mod client;
mod present;
mod routes;
#[cfg(test)]
mod tests;
mod transport;

pub use self::api::{
    ApiError, ApproveRequest, ChooseLayout, DisplayEntry, DisplayList, DisplayState, ErrorBody,
    ErrorCode, IssueViewer, LayoutEntry, LayoutList, MAX_WINDOW_MINUTES, OpenWindow, ViewerEntry,
    ViewerIssued, ViewerList, ViewerStatus, WindowState,
};
pub use self::client::{AdminClient, CallError, describe};
pub use self::present::{
    age, approved, describe_entry, describe_layouts, describe_list, describe_viewers,
    describe_window, forgotten, label, layout_changed, qr_for_terminal, rejected, revoked,
    viewer_changed, viewer_issued,
};
pub use self::routes::{openapi_json, router};
pub use self::transport::{LocalListener, SOCKET_FILE, bind, connect, default_path};
