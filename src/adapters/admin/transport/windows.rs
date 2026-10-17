//! The admin socket on Windows: a named pipe, named for the socket's path so that the server and `displayctl`, given the
//! same path, find the same pipe, and two servers with different directories do not meet.
//!
//! A pipe has no file to put in a private directory, so what keeps other users out is its security descriptor, and the
//! default one is not relied on (Microsoft documents it as giving read access to Everyone and the anonymous account, and
//! full control to the administrators and the system account). The pipe is made with one of its own:
//!
//! - its **access list** names one user, the one the server runs as, and nobody else, not even the administrators group,
//!   and is marked protected, so nothing is inherited into it;
//! - its **owner** is that same user, whatever the server's token would make its owner by default (an elevated process
//!   would make it the administrators group).
//!
//! Other machines are refused by the pipe itself (`interprocess` makes it with `PIPE_REJECT_REMOTE_CLIENTS`). The first
//! pipe of a name is made with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a pipe someone else already made under it is an error
//! for the server, not a server that is listening on a pipe it does not control.
//!
//! The pipe's name can be guessed, so the same is checked from the other side: `connect` looks at who owns the pipe it
//! has connected to, and gives it up before a byte is sent if that is not the user it is running as. Otherwise another
//! user could make a pipe of that name first, and be handed what `displayctl` sends (a pairing code, say) and be able to
//! act as the user who sent it.

mod user;

use std::os::windows::io::AsHandle;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use interprocess::local_socket::tokio::{Listener, prelude::*};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Name};
use interprocess::os::windows::local_socket::ListenerOptionsExt;
use interprocess::os::windows::named_pipe::pipe_mode::Bytes;
use interprocess::os::windows::named_pipe::tokio::DuplexPipeStream;
use interprocess::os::windows::security_descriptor::SecurityDescriptor;
use sha2::{Digest, Sha256};
use widestring::U16CString;

use super::unusable;

/// A connection to the pipe.
pub type Connection = DuplexPipeStream<Bytes>;

/// The start of the pipe's name, before what tells one socket path from another.
const PIPE_PREFIX: &str = "home-display-admin-";

/// The pipe's name: for the path as an absolute path, ignoring case as Windows does, so `displayctl` run from another
/// directory with the same absolute path, or from the same one with a relative path, reaches the same pipe.
pub fn name(path: &Path) -> anyhow::Result<Name<'static>> {
    pipe_name(path)?
        .to_ns_name::<GenericNamespaced>()
        .map_err(|e| unusable(path, e))
}

fn pipe_name(path: &Path) -> anyhow::Result<String> {
    let absolute: PathBuf = std::path::absolute(path).map_err(|e| unusable(path, e))?;
    let digest = Sha256::digest(absolute.to_string_lossy().to_lowercase().as_bytes());
    let tag: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(format!("{PIPE_PREFIX}{tag}"))
}

pub async fn bind(path: &Path) -> anyhow::Result<Listener> {
    let sid = user::current_user_sid().context("Failed to find which user the server runs as")?;
    ListenerOptions::new()
        .name(name(path)?)
        .security_descriptor(only(&sid)?)
        .create_tokio()
        .with_context(|| {
            format!(
                "Failed to open the admin pipe for {}. If another server is using the same path, give this one its own \
                 [server.tls] directory or [server.admin] socket",
                path.display()
            )
        })
}

/// A connection to the pipe, made only if the pipe belongs to the user this process runs as.
pub async fn connect(path: &Path) -> anyhow::Result<Connection> {
    let pipe = format!(r"\\.\pipe\{}", pipe_name(path)?);
    let stream = DuplexPipeStream::<Bytes>::connect_by_path(pipe.as_str())
        .await
        .with_context(|| format!("Failed to connect to {}", path.display()))?;
    let me = user::current_user_sid().context("Failed to find which user this is")?;
    let owner = user::owner_sid(stream.as_handle())
        .context("Failed to find who owns the admin pipe it connected to")?;
    if owner != me {
        bail!(
            "the admin pipe for {} belongs to {owner}, not to the user running this ({me}), so it is not the server; \
             nothing was sent to it",
            path.display()
        );
    }
    Ok(stream)
}

/// A security descriptor that makes the user with this SID the owner, gives all access to that user and none to anyone
/// else, and is protected from inheriting more.
fn only(sid: &str) -> anyhow::Result<SecurityDescriptor> {
    let sddl = U16CString::from_str(format!("O:{sid}D:P(A;;GA;;;{sid})"))
        .map_err(|e| anyhow!("The access list for the admin pipe could not be written: {e}"))?;
    SecurityDescriptor::deserialize(&sddl)
        .context("Failed to make the access list for the admin pipe")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_the_same_pipe_however_it_is_spelled() {
        let one = pipe_name(Path::new(r"C:\Server\pki\admin.sock")).unwrap();
        let other = pipe_name(Path::new(r"c:\server\PKI\admin.sock")).unwrap();

        assert_eq!(one, other);
    }

    #[test]
    fn different_paths_are_different_pipes() {
        let one = pipe_name(Path::new(r"C:\a\admin.sock")).unwrap();
        let other = pipe_name(Path::new(r"C:\b\admin.sock")).unwrap();

        assert_ne!(one, other);
        assert!(one.starts_with(PIPE_PREFIX));
    }

    #[test]
    fn the_access_list_is_one_that_can_be_made_only_from_a_sid() {
        let sid = user::current_user_sid().unwrap();

        assert!(only(&sid).is_ok());
        assert!(only("not a sid").is_err());
    }

    /// What the system holds for the pipe, read back from a connection to it, is the owner and the one entry asked for,
    /// and nothing for Everyone, the system or the administrators.
    #[tokio::test]
    async fn the_pipe_that_is_made_is_owned_by_the_user_and_open_to_no_one_else() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("admin.sock");
        let _listener = bind(&path).await.unwrap();
        let sid = user::current_user_sid().unwrap();

        let stream = connect(&path).await.unwrap();

        let held = user::descriptor_text(stream.as_handle()).unwrap();
        assert_eq!(held, format!("O:{sid}D:P(A;;FA;;;{sid})"));
    }

    /// Another user's pipe cannot be tried, so a pipe that is owned by someone else is stood in for by the owner check
    /// itself: the owner of a pipe is looked up, and is compared with the user, not assumed.
    #[tokio::test]
    async fn the_owner_of_a_connected_pipe_is_what_connect_compares() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("admin.sock");
        let _listener = bind(&path).await.unwrap();
        let stream = DuplexPipeStream::<Bytes>::connect_by_path(
            format!(r"\\.\pipe\{}", pipe_name(&path).unwrap()).as_str(),
        )
        .await
        .unwrap();

        let owner = user::owner_sid(stream.as_handle()).unwrap();

        assert_eq!(owner, user::current_user_sid().unwrap());
    }
}
