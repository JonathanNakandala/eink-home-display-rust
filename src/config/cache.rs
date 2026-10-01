use std::path::{Path, PathBuf};

use anyhow::Context;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const DEFAULT_CACHE_DIR: &str = "cache";

/// Where files that can be re-created (a downloaded Chrome, for one) are kept between runs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CacheConfig {
    /// Relative paths are resolved against the working directory.
    #[serde(default = "default_directory")]
    pub directory: PathBuf,
}

fn default_directory() -> PathBuf {
    PathBuf::from(DEFAULT_CACHE_DIR)
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self { directory: default_directory() }
    }
}

/// The layout inside the cache directory, so no one else hard-codes sub-folder names.
#[derive(Debug, Clone)]
pub struct CachePaths {
    root: PathBuf,
}

impl CachePaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a downloaded Chrome is extracted.
    pub fn chrome(&self) -> PathBuf {
        self.root.join("chrome")
    }

    /// Creates the cache directory and its sub-folders if missing.
    pub fn ensure_exists(&self) -> anyhow::Result<()> {
        for dir in [self.chrome()] {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create cache directory {}", dir.display()))?;
        }
        Ok(())
    }
}

impl From<&CacheConfig> for CachePaths {
    fn from(config: &CacheConfig) -> Self {
        Self::new(config.directory.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_a_cache_folder() {
        assert_eq!(CacheConfig::default().directory, PathBuf::from("cache"));
    }

    #[test]
    fn ensure_exists_creates_the_chrome_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = CachePaths::new(tmp.path().join("cache"));

        paths.ensure_exists().unwrap();
        paths.ensure_exists().unwrap(); // idempotent

        assert!(paths.chrome().is_dir());
    }
}
