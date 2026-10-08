//! The admin API: how the owner of a running server looks at it and changes it, from the same machine.
//!
//! It is HTTP and JSON over a local socket (see `transport`), so anything that can speak HTTP to a socket can use
//! it, and `displayctl` is the one that comes with the program. It is described in `config/admin-openapi.json`,
//! which is made from the code, checked by a test, and also served at `/openapi.json` on the socket.
//!
//! Nothing here can be reached from the network, and no endpoint returns a secret: a pairing code, a key or a
//! certificate never comes back, so nothing copied from here can stand in for what the display itself shows.

mod api;
mod client;
mod routes;
#[cfg(test)]
mod tests;
mod transport;

pub use self::api::{ApiError, ErrorBody, ErrorCode, MAX_WINDOW_MINUTES, OpenWindow, WindowState};
pub use self::client::{AdminClient, CallError, describe, describe_window};
pub use self::routes::{openapi_json, router};
pub use self::transport::{LocalListener, SOCKET_FILE, bind, connect, default_path};
