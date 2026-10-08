//! The local socket the admin API is served on, and how to reach it.
//!
//! A Unix domain socket in the PKI directory. Who may use it is decided by the operating system, not by a
//! password. Two things keep other users out, and the second is the one that works everywhere:
//!
//! - the socket is created with mode 0600, set before it is bound so there is no moment it is open to others.
//!   Linux honours that; macOS does not apply a mode to a socket at all (the call is refused as unsupported), so
//!   there the mode is set after, as a label only;
//! - the directory it is in must not be open to anyone else (no group or other permissions), which is how the PKI
//!   directory is made. `bind` refuses a directory that is, because then the socket's own mode would be all there is.
//!
//! Nothing on the network can reach it.
//!
//! On Windows this would be a named pipe, but whether the pipe's default access keeps other users out has not
//! been checked, and that is the whole protection, so the interface is not offered there yet (`bind` says so).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use interprocess::local_socket::tokio::{Listener, Stream, prelude::*};
use interprocess::local_socket::{GenericFilePath, ListenerOptions};

/// The longest socket path that is certain to fit in a Unix socket address (104 bytes on macOS, 108 on Linux,
/// less the terminator and some room).
const MAX_PATH_BYTES: usize = 100;

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

/// Opens the socket at `path`. Fails, saying why, if another server is already using it, if the path is too
/// long for a socket, or if something that is not a socket is in the way; a socket left behind by a server that
/// died is replaced.
pub async fn bind(path: &Path) -> anyhow::Result<LocalListener> {
    #[cfg(not(unix))]
    {
        let _ = path;
        bail!("the admin interface is not available on this platform yet");
    }
    #[cfg(unix)]
    {
        use interprocess::os::unix::local_socket::ListenerOptionsExt;

        let bytes = path.as_os_str().len();
        if bytes > MAX_PATH_BYTES {
            bail!(
                "the admin socket path is {bytes} bytes, more than the {MAX_PATH_BYTES} a socket address can hold: {}. \
                 Shorten [server.tls] directory, or set [server.admin] socket to a shorter path",
                path.display()
            );
        }
        require_private_directory(path)?;
        clear_stale(path).await?;
        let options = || {
            path.to_fs_name::<GenericFilePath>()
                .with_context(|| format!("{} is not usable as a socket name", path.display()))
                .map(|name| ListenerOptions::new().name(name))
        };
        let listener = match options()?.mode(0o600).create_tokio() {
            Ok(listener) => listener,
            // This system does not apply a mode to a socket (macOS); the directory is what keeps others out.
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                let listener = options()?.create_tokio().with_context(|| {
                    format!("Failed to open the admin socket {}", path.display())
                })?;
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                    .with_context(|| format!("Failed to restrict {}", path.display()))?;
                listener
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("Failed to open the admin socket {}", path.display())
                });
            }
        };
        Ok(LocalListener(listener))
    }
}

/// The directory the socket is in must be closed to everyone but its owner, or the socket's own mode (which some
/// systems ignore) would be the only thing keeping other users out.
#[cfg(unix)]
fn require_private_directory(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let directory = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mode = std::fs::metadata(directory)
        .with_context(|| format!("Failed to look at {}", directory.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        bail!(
            "{} can be used by other users (mode {:o}), and the admin socket in it would be open to them. \
             Run `chmod 700 {}`, or set [server.admin] socket to a path in a directory only you can use",
            directory.display(),
            mode & 0o777,
            directory.display()
        );
    }
    Ok(())
}

/// Removes a socket file nothing is listening on, so a new one can be made where it was. Refuses to touch a
/// socket another server is using, or a file that is not a socket at all.
#[cfg(unix)]
async fn clear_stale(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::FileTypeExt;

    let kind = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata.file_type(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("Failed to look at {}", path.display())),
    };
    if !kind.is_socket() {
        bail!(
            "{} is there and is not a socket; not removing it. Move it, or set [server.admin] socket elsewhere",
            path.display()
        );
    }
    if connect(path).await.is_ok() {
        bail!(
            "another server is already listening on {}: two servers must not share a directory",
            path.display()
        );
    }
    std::fs::remove_file(path)
        .with_context(|| format!("Failed to remove the old socket {}", path.display()))?;
    log::info!("Replaced a socket left behind at {}", path.display());
    Ok(())
}

/// A connection to the admin socket at `path`.
pub async fn connect(path: &Path) -> anyhow::Result<Stream> {
    let name = path
        .to_fs_name::<GenericFilePath>()
        .map_err(|e| anyhow!("{} is not usable as a socket name: {e}", path.display()))?;
    Stream::connect(name)
        .await
        .with_context(|| format!("Failed to connect to {}", path.display()))
}

/// Where the socket is, given the directory the server keeps its authority in.
pub fn default_path(directory: &Path) -> PathBuf {
    directory.join(SOCKET_FILE)
}
