//! The local socket the admin API is served on, and how to reach it.
//!
//! Nothing on the network can reach it, and who on the machine may use it is decided by the operating system, not by a
//! password. How that is done depends on the system:
//!
//! - **Unix** (`unix`): a socket in the PKI directory, made with mode 0600 in a directory closed to everyone else.
//! - **Windows** (`windows`): a named pipe, named for the socket's path, whose access list names the user the server runs as
//!   and no one else.
//!
//! What is the same on both lives here: the listener `axum` serves on, how to connect, and where the socket is by default.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow};
use interprocess::local_socket::tokio::{Listener, Stream, prelude::*};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use self::unix as platform;
#[cfg(windows)]
use self::windows as platform;

/// The file name of the socket inside the PKI directory.
pub const SOCKET_FILE: &str = "admin.sock";

/// A listener `axum` can serve on.
pub struct LocalListener(Listener);

impl axum::serve::Listener for LocalListener {
    type Io = Stream;
    type Addr = ();

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.0.accept().await {
                Ok(stream) => return (stream, ()),
                Err(e) => {
                    // Whatever went wrong (out of descriptors, say) is not fixed by trying again at once.
                    log::warn!("Failed to accept an admin connection: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(())
    }
}

/// Opens the socket at `path`. Fails, saying why, if another server is already using it, if the path cannot be used
/// for a socket, or if the system would leave it open to other users; a socket left behind by a server that died is
/// replaced.
pub async fn bind(path: &Path) -> anyhow::Result<LocalListener> {
    platform::bind(path).await.map(LocalListener)
}

/// A connection to the admin socket at `path`.
pub async fn connect(path: &Path) -> anyhow::Result<Stream> {
    let name = platform::name(path)?;
    Stream::connect(name)
        .await
        .with_context(|| format!("Failed to connect to {}", path.display()))
}

/// Where the socket is, given the directory the server keeps its authority in.
pub fn default_path(directory: &Path) -> PathBuf {
    directory.join(SOCKET_FILE)
}

/// The error for a path that cannot be turned into a socket's name.
fn unusable(path: &Path, why: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{} is not usable as a socket name: {why}", path.display())
}
