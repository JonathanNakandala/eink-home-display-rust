//! Keeps the authority in files, and makes it the first time.
//!
//! - `root.pem`: the root certificate. Public: it is what displays are given to trust the server by.
//! - `root.key`: the root's key, readable only by the server's user. It is used for one thing, signing an
//!   intermediate, and the server never loads it to serve. It may be moved off the machine: the server then
//!   runs as before, and an intermediate that is due to be replaced waits for the key to be put back (or
//!   for `rotate_intermediate` to be given its path).
//! - `intermediate.pem`: the intermediate's certificate and key together, so the two are always replaced
//!   as one and can't be found not to match.
//! - `retired.pem`: earlier intermediates, kept until they end, because certificates they signed are still
//!   good and a display need not send its intermediate.
//!
//! If the files that make up the authority are only partly there, something has been deleted or moved, and
//! making a new authority would quietly strand every paired display, so it is an error and not a fresh
//! start. The exception is a root with its key and no intermediate, which is how an intermediate is lost
//! and replaced.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, anyhow};
use chrono::{DateTime, Utc};

use super::{PrivateAuthority, ROTATE_WITHIN, new_intermediate};
use crate::domain::services::clock::{earliest_plausible, implausible};

const ROOT_CERTIFICATE: &str = "root.pem";
const ROOT_KEY: &str = "root.key";
const INTERMEDIATE: &str = "intermediate.pem";
const RETIRED: &str = "retired.pem";
/// What an earlier version of this program called its single-level authority.
const OLD_CERTIFICATE: &str = "authority.pem";
const OLD_KEY: &str = "authority.key";

/// Whether to make an authority when there is none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Create {
    IfMissing,
    /// Open only what is there. A directory with no authority in it is an error: it is as likely to be
    /// a volume that was not mounted as a first start, and a new authority would lock out every display.
    Never,
}

/// The authority kept in `directory`, made there first if there is none and `create` allows it. An
/// intermediate that is near its end is replaced when the root's key is there to do it.
pub fn open(directory: &Path, create: Create) -> anyhow::Result<PrivateAuthority> {
    for old in [OLD_CERTIFICATE, OLD_KEY] {
        if directory.join(old).exists() {
            return Err(anyhow!(
                "{} is from an earlier version that had one level of authority, which is no longer read. \
                 Nothing it signed can be used; remove it and start again",
                directory.join(old).display()
            ));
        }
    }
    let root = directory.join(ROOT_CERTIFICATE);
    let root_key = directory.join(ROOT_KEY);
    let intermediate = directory.join(INTERMEDIATE);
    match (root.exists(), intermediate.exists()) {
        (true, true) => {
            let authority = load(directory)?;
            let left = authority.intermediate_not_after() - Utc::now();
            if left >= ROTATE_WITHIN {
                return Ok(authority);
            }
            if root_key.exists() {
                log::info!(
                    "The intermediate ends in {} days; replacing it",
                    left.num_days()
                );
                rotate_intermediate(directory, None)?;
                return load(directory);
            }
            log::warn!(
                "The intermediate certificate ends in {} days and {} is not here to replace it. Put the \
                 root's key back, or run the rotation with its path, before then: after it ends no display \
                 can be served",
                left.num_days().max(0),
                root_key.display()
            );
            Ok(authority)
        }
        (true, false) if root_key.exists() => {
            // The intermediate is gone but the root and its key are not: the way an intermediate is replaced.
            log::warn!(
                "{} is missing; making a new intermediate from the root",
                intermediate.display()
            );
            rotate_intermediate(directory, None)?;
            load(directory)
        }
        (false, false) if !root_key.exists() => match create {
            Create::Never => Err(anyhow!(
                "There is no certificate authority in {}",
                directory.display()
            )),
            Create::IfMissing => create_all(directory, Utc::now()),
        },
        _ => Err(anyhow!(
            "{} is only partly there (it needs {ROOT_CERTIFICATE} and {INTERMEDIATE}, and {ROOT_KEY} to replace \
             the intermediate); not making a new authority, which would strand the paired displays",
            directory.display()
        )),
    }
}

fn load(directory: &Path) -> anyhow::Result<PrivateAuthority> {
    let read = |name: &str| {
        let path = directory.join(name);
        fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))
    };
    let retired = match read(RETIRED) {
        Ok(text) => split_certificates(&text),
        Err(_) if !directory.join(RETIRED).exists() => Vec::new(),
        Err(e) => return Err(e),
    };
    PrivateAuthority::from_pem(
        &read(ROOT_CERTIFICATE)?,
        &read(INTERMEDIATE)?,
        &retired,
        Utc::now(),
    )
    .with_context(|| format!("The authority in {} is not usable", directory.display()))
}

/// Refuses a time that is before this program existed. The authority is made from the system clock, and a machine
/// with no real-time clock reads 1970 until a time service sets it. A root dated from then would end in 1990 and
/// every display that pinned it would refuse it once its own clock was right, with pairing them all again the only
/// way out; an intermediate dated from then would be refused the same way. So nothing is made until the clock is set
/// (under systemd the start is tried again after a pause).
fn require_plausible(now: DateTime<Utc>) -> anyhow::Result<()> {
    if now < earliest_plausible() {
        return Err(anyhow!(implausible(now)));
    }
    Ok(())
}

pub(super) fn create_all(directory: &Path, now: DateTime<Utc>) -> anyhow::Result<PrivateAuthority> {
    // Before anything is made, so a refusal leaves no directory behind.
    require_plausible(now)?;
    fs::create_dir_all(directory)
        .with_context(|| format!("Failed to create {}", directory.display()))?;
    // Holds the keys, so no one else needs to see in.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("Failed to restrict {}", directory.display()))?;
    }
    let (authority, files) = PrivateAuthority::generate(now)?;
    // The root's key first and the intermediate last. Without the intermediate nothing has been signed,
    // so a crash before it is a start that can be repeated; and with it, the authority is whole.
    write_atomically(&directory.join(ROOT_KEY), &files.root_key, true)?;
    write_atomically(
        &directory.join(ROOT_CERTIFICATE),
        &files.root_certificate,
        false,
    )?;
    write_atomically(&directory.join(INTERMEDIATE), &files.intermediate, true)?;
    log::info!(
        "Made a new certificate authority in {}. {ROOT_KEY} there is used only to replace the intermediate \
         every few years; back it up, and keep it off this machine if you can",
        directory.display()
    );
    Ok(authority)
}

/// Replaces the intermediate with a new one signed by the root, keeping the old one until it ends so
/// that what it signed stays good. The root's key is `root_key`, or `root.key` in `directory`.
///
/// The two files change one after the other, each whole or not at all: the retired list first, so that a
/// crash between them leaves the old intermediate still in use and also kept, which is harmless.
pub fn rotate_intermediate(directory: &Path, root_key: Option<&Path>) -> anyhow::Result<()> {
    rotate_intermediate_at(directory, root_key, Utc::now())
}

pub(super) fn rotate_intermediate_at(
    directory: &Path,
    root_key: Option<&Path>,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    require_plausible(now)?;
    let key_path = root_key
        .map(Path::to_path_buf)
        .unwrap_or_else(|| directory.join(ROOT_KEY));
    let read = |path: &Path| {
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))
    };
    let root_certificate = read(&directory.join(ROOT_CERTIFICATE))?;
    let new = new_intermediate(&root_certificate, &read(&key_path)?, now)?;

    let intermediate_path = directory.join(INTERMEDIATE);
    if intermediate_path.exists() {
        let old = read(&intermediate_path)?;
        let certificate = split_certificates(&old)
            .into_iter()
            .next()
            .context("The intermediate being replaced has no certificate in it")?;
        let retired_path = directory.join(RETIRED);
        let mut retired = if retired_path.exists() {
            read(&retired_path)?
        } else {
            String::new()
        };
        if !retired.is_empty() && !retired.ends_with('\n') {
            retired.push('\n');
        }
        retired.push_str(&certificate);
        write_atomically(&retired_path, &retired, false)?;
    }
    write_atomically(&intermediate_path, &new, true)?;
    log::info!("Replaced the intermediate certificate");
    Ok(())
}

/// The certificates in `pem`, each as its own PEM.
fn split_certificates(pem: &str) -> Vec<String> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut found = Vec::new();
    let mut rest = pem;
    while let Some(start) = rest.find(BEGIN) {
        let Some(end) = rest[start..].find(END) else {
            break;
        };
        let stop = start + end + END.len();
        found.push(format!("{}\n", &rest[start..stop]));
        rest = &rest[stop..];
    }
    found
}

/// Writes a file whole or not at all: to a new file beside it, then renamed over. A `private` one can be
/// read only by its owner from the moment it exists (not made open and then narrowed).
fn write_atomically(path: &Path, contents: &str, private: bool) -> anyhow::Result<()> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("A file has no name")?;
    let temporary = path.with_file_name(format!(".{name}.new"));
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to clear {}", temporary.display()));
        }
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if private { 0o600 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("Failed to create {}", temporary.display()))?;
    file.write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("Failed to write {}", temporary.display()))?;
    fs::rename(&temporary, path)
        .with_context(|| format!("Failed to put {} in place", path.display()))?;
    // The rename is only durable once the directory entry is, which on Unix needs the directory flushed. Without
    // it a crash just after could lose the new file, and a start after that would find the authority only
    // partly there. (The list of displays is written the same way, in `pairing_store`.)
    #[cfg(unix)]
    if let Some(directory) = path.parent() {
        fs::File::open(directory)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("Failed to flush {}", directory.display()))?;
    }
    Ok(())
}
