//! Writing a file so that it is never seen half written.

use std::path::Path;

use anyhow::Context;

/// Writes `bytes` to `path` so that a reader, or a crash, sees the old contents or the new, not a mix.
pub(super) async fn write_atomically(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;

    let directory = path.parent().context("The file has no directory")?;
    let temporary = path.with_extension("json.tmp");
    let mut file = tokio::fs::File::create(&temporary)
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
