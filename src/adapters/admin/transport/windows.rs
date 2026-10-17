//! The admin socket on Windows: a named pipe, named for the socket's path so that the server and `displayctl`, given the
//! same path, find the same pipe, and two servers with different directories do not meet.
//!
//! A pipe has no file to put in a private directory, so what keeps other users out is its access list, and the default one
//! is not enough (it lets every user read, and the pipe is open to write by more than its owner's account on some setups).
//! The pipe is made with a list of its own that names one user, the one the server runs as, and nobody else, not even the
//! administrators group; the list is marked protected, so nothing is inherited into it. Other machines are refused by the
//! pipe itself (`interprocess` makes it with `PIPE_REJECT_REMOTE_CLIENTS`). The first pipe of a name is made with
//! `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a pipe someone else already made under it is an error, not a server that is
//! listening on a pipe it does not control.

mod user;

use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use interprocess::local_socket::tokio::{Listener, prelude::*};
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Name};
use interprocess::os::windows::local_socket::ListenerOptionsExt;
use interprocess::os::windows::security_descriptor::SecurityDescriptor;
use sha2::{Digest, Sha256};
use widestring::U16CString;

use super::unusable;

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

/// A security descriptor that gives all access to the user with this SID and none to anyone else, and is protected from
/// inheriting more.
fn only(sid: &str) -> anyhow::Result<SecurityDescriptor> {
    let sddl = U16CString::from_str(format!("D:P(A;;GA;;;{sid})"))
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
    fn the_access_list_names_the_user_and_no_one_else() {
        let sid = user::current_user_sid().unwrap();

        assert!(only(&sid).is_ok());
        assert!(only("not a sid").is_err());
    }
}
