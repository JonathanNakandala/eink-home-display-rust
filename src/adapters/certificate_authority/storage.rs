//! Keeps the authority in two files, and makes it the first time.
//!
//! `authority.pem` is the certificate (public: it is what displays are given) and `authority.key` is its
//! private key, readable only by the server's user. If only one of the two is there, something has
//! been deleted or moved, and making a new authority would quietly strand every paired display, so it
//! is an error and not a fresh start.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, anyhow};
use chrono::Utc;

use super::PrivateAuthority;

const CERTIFICATE: &str = "authority.pem";
const KEY: &str = "authority.key";

/// The authority kept in `directory`, made there first if there is none.
pub fn open(directory: &Path) -> anyhow::Result<PrivateAuthority> {
    let certificate_path = directory.join(CERTIFICATE);
    let key_path = directory.join(KEY);
    match (certificate_path.exists(), key_path.exists()) {
        (true, true) => {
            let certificate = fs::read_to_string(&certificate_path)
                .with_context(|| format!("Failed to read {}", certificate_path.display()))?;
            let key = fs::read_to_string(&key_path)
                .with_context(|| format!("Failed to read {}", key_path.display()))?;
            PrivateAuthority::from_pem(&certificate, &key)
                .with_context(|| format!("The authority in {} is not usable", directory.display()))
        }
        (false, false) => {
            fs::create_dir_all(directory)
                .with_context(|| format!("Failed to create {}", directory.display()))?;
            // Holds the authority's key, so no one else needs to see in.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                    .with_context(|| format!("Failed to restrict {}", directory.display()))?;
            }
            let (authority, certificate, key) = PrivateAuthority::generate(Utc::now())?;
            // The key first: with a certificate and no key, a crash in between would leave the
            // half-made state that is refused above, not a usable one.
            write_private(&key_path, &key)?;
            fs::write(&certificate_path, certificate)
                .with_context(|| format!("Failed to write {}", certificate_path.display()))?;
            log::info!(
                "Made a new certificate authority in {}",
                directory.display()
            );
            Ok(authority)
        }
        (true, false) => Err(anyhow!(
            "{} is there but {} is not; not making a new authority, which would strand the paired displays",
            certificate_path.display(),
            key_path.display()
        )),
        (false, true) => Err(anyhow!(
            "{} is there but {} is not; not making a new authority, which would strand the paired displays",
            key_path.display(),
            certificate_path.display()
        )),
    }
}

/// Writes a file only its owner can read, from the moment it exists (not made open and then
/// narrowed), and never over one that is already there.
fn write_private(path: &Path, contents: &str) -> anyhow::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("Failed to create {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("Failed to write {}", path.display()))
}
