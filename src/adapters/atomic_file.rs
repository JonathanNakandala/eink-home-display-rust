//! Writing a file so that it is never seen half written.

use std::path::Path;

use anyhow::Context;

/// Writes `bytes` to `path` so that a reader, or a crash, sees the old contents or the new, not a mix.
pub(super) async fn write_atomically(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write(path, bytes, false).await
}

/// As `write_atomically`, for a file that holds a secret: on Unix it is made readable by its owner alone from the moment it
/// exists (the temporary file too, so there is no instant it is open to others). Elsewhere it has the permissions its
/// directory gives it, as the rest of what the server keeps does.
pub(super) async fn write_privately(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write(path, bytes, true).await
}

async fn write(path: &Path, bytes: &[u8], private: bool) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;

    let directory = path.parent().context("The file has no directory")?;
    let temporary = path.with_extension("json.tmp");
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let mut file = options
        .open(&temporary)
        .await
        .with_context(|| format!("Failed to create {}", temporary.display()))?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&temporary, path).await?;
    // The rename is only durable once the directory entry is, which on Unix needs the directory flushed.
    #[cfg(unix)]
    tokio::fs::File::open(directory).await?.sync_all().await?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}
