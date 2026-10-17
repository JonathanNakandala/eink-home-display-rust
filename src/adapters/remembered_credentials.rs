//! Keeps the keys the owner asked to be remembered in one small JSON file, `credentials.json`.
//!
//! This is a file with secrets in it, and the only one the server writes, so it is held to more than the others:
//!
//! - **Private.** On Unix it is made readable by its owner alone (mode 0600), the temporary file it is written through
//!   too, from the moment each exists. Elsewhere it has what its directory gives it, as the authority's key does.
//! - **Plain.** The keys are stored as they are. A key encrypted with another key kept beside it would be no safer, so
//!   this does not pretend to be; what protects them is who can read the file. Keys the owner did not ask to remember are
//!   never written, and the file is not made until one is.
//! - **Strict.** A file that is not usable is an error, never an empty start, so that a key isn't quietly forgotten. A key
//!   in it that is not one this server knows is an error as well, since it would otherwise be written out without it.
//!
//! The whole file is held in memory and written whole through a temporary file renamed over the real one, and memory only
//! changes once the write has worked.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::atomic_file::write_privately;
use crate::domain::models::credential::CredentialName;
use crate::domain::services::remembered_credentials::RememberedCredentials;

const FILE: &str = "credentials.json";
/// Bumped when the layout of the file changes, so an older server refuses a file it would misread.
const VERSION: u32 = 1;

pub struct FileRememberedCredentials {
    path: PathBuf,
    keys: Mutex<BTreeMap<CredentialName, SecretString>>,
}

/// The file as it is written: names as text, kept apart from the domain types.
#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    keys: BTreeMap<String, String>,
}

impl FileRememberedCredentials {
    /// The keys kept in `directory`; none if there is no file yet. Nothing is created until a key is kept.
    pub async fn open(directory: &Path) -> anyhow::Result<Self> {
        let path = directory.join(FILE);
        let keys = match tokio::fs::read(&path).await {
            Ok(bytes) => parse(&bytes)
                .with_context(|| format!("{} is not usable; fix or remove it", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
        };
        Ok(Self {
            path,
            keys: Mutex::new(keys),
        })
    }

    async fn save(&self, keys: &BTreeMap<CredentialName, SecretString>) -> anyhow::Result<()> {
        if keys.is_empty() {
            // Nothing left to keep: the file goes, rather than lingering as an empty one.
            return match tokio::fs::remove_file(&self.path).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => {
                    Err(e).with_context(|| format!("Failed to remove {}", self.path.display()))
                }
            };
        }
        let file = File {
            version: VERSION,
            keys: keys
                .iter()
                .map(|(name, key)| (name.as_str().to_owned(), key.expose_secret().to_owned()))
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&file)?;
        write_privately(&self.path, &bytes)
            .await
            .with_context(|| format!("Failed to save {}", self.path.display()))
    }
}

fn parse(bytes: &[u8]) -> anyhow::Result<BTreeMap<CredentialName, SecretString>> {
    // The error from a file of the wrong shape names where, never what: serde does not echo a string it can't use, but
    // the text is not kept in the message either way.
    let file: File = serde_json::from_slice(bytes).context("It is not the expected JSON")?;
    anyhow::ensure!(
        file.version == VERSION,
        "It is version {}, and this server reads version {VERSION}",
        file.version
    );
    let mut keys = BTreeMap::new();
    for (name, key) in file.keys {
        let known = CredentialName::parse(&name)
            .map_err(|e| anyhow::anyhow!("It has a key this server does not know: {e}"))?;
        known
            .check(&key)
            .map_err(|e| anyhow::anyhow!("The key for {known} is not usable: {e}"))?;
        keys.insert(known, SecretString::from(key));
    }
    Ok(keys)
}

#[async_trait::async_trait]
impl RememberedCredentials for FileRememberedCredentials {
    async fn load(&self) -> anyhow::Result<BTreeMap<CredentialName, SecretString>> {
        Ok(self.keys.lock().await.clone())
    }

    async fn remember(&self, name: CredentialName, key: &SecretString) -> anyhow::Result<()> {
        let mut held = self.keys.lock().await;
        let mut changed = held.clone();
        changed.insert(name, key.clone());
        self.save(&changed).await?;
        *held = changed;
        Ok(())
    }

    async fn forget(&self, name: CredentialName) -> anyhow::Result<()> {
        let mut held = self.keys.lock().await;
        if !held.contains_key(&name) {
            return Ok(());
        }
        let mut changed = held.clone();
        changed.remove(&name);
        self.save(&changed).await?;
        *held = changed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEATHER: &str = "0123456789abcdef0123456789abcdef";

    fn secret(key: &str) -> SecretString {
        SecretString::from(key.to_owned())
    }

    fn text(keys: &BTreeMap<CredentialName, SecretString>, name: CredentialName) -> Option<String> {
        keys.get(&name).map(|key| key.expose_secret().to_owned())
    }

    #[tokio::test]
    async fn nothing_is_made_until_a_key_is_kept() {
        let directory = tempfile::tempdir().unwrap();

        let store = FileRememberedCredentials::open(directory.path())
            .await
            .unwrap();

        assert!(store.load().await.unwrap().is_empty());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_key_is_kept_replaced_and_forgotten_and_survives_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let store = FileRememberedCredentials::open(directory.path())
            .await
            .unwrap();
        store
            .remember(CredentialName::OpenWeather, &secret(WEATHER))
            .await
            .unwrap();
        store
            .remember(CredentialName::Tfl, &secret("old"))
            .await
            .unwrap();
        store
            .remember(CredentialName::Tfl, &secret("new"))
            .await
            .unwrap();

        let again = FileRememberedCredentials::open(directory.path())
            .await
            .unwrap();
        let keys = again.load().await.unwrap();
        assert_eq!(
            text(&keys, CredentialName::OpenWeather).as_deref(),
            Some(WEATHER)
        );
        assert_eq!(text(&keys, CredentialName::Tfl).as_deref(), Some("new"));

        again.forget(CredentialName::Tfl).await.unwrap();
        let after = FileRememberedCredentials::open(directory.path())
            .await
            .unwrap();
        assert!(
            !after
                .load()
                .await
                .unwrap()
                .contains_key(&CredentialName::Tfl)
        );
        // Forgetting what was never kept is fine.
        after.forget(CredentialName::OpenLdbws).await.unwrap();
    }

    #[tokio::test]
    async fn forgetting_the_last_key_removes_the_file_and_leaves_no_temporary_one() {
        let directory = tempfile::tempdir().unwrap();
        let store = FileRememberedCredentials::open(directory.path())
            .await
            .unwrap();
        store
            .remember(CredentialName::Tfl, &secret("key"))
            .await
            .unwrap();

        store.forget(CredentialName::Tfl).await.unwrap();

        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_file_and_the_one_it_is_written_through_are_private_from_the_start() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let store = FileRememberedCredentials::open(directory.path())
            .await
            .unwrap();

        store
            .remember(CredentialName::Tfl, &secret("key"))
            .await
            .unwrap();

        let mode = std::fs::metadata(directory.path().join(FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        // The write goes through a temporary file that is renamed; a leftover one would have the same mode, so look at the
        // first write's one more way: nothing but the real file is left.
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn a_file_that_is_not_usable_is_an_error_naming_it_and_never_an_empty_start() {
        for (what, content) in [
            ("not json", "nope".to_owned()),
            (
                "another version",
                r#"{"version": 9, "keys": {}}"#.to_owned(),
            ),
            (
                "a key nobody knows",
                r#"{"version": 1, "keys": {"mystery": "abc"}}"#.to_owned(),
            ),
            (
                "a key that cannot be one",
                r#"{"version": 1, "keys": {"open-weather": "short"}}"#.to_owned(),
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join(FILE), &content).unwrap();

            let error = match FileRememberedCredentials::open(directory.path()).await {
                Ok(_) => panic!("{what} should be refused"),
                Err(e) => format!("{e:#}"),
            };

            assert!(error.contains("credentials.json"), "{what}: {error}");
            assert!(!error.contains("short"), "{what}: {error}");
        }
    }

    #[tokio::test]
    async fn a_key_that_cannot_be_saved_is_not_kept() {
        let directory = tempfile::tempdir().unwrap();
        let store = FileRememberedCredentials::open(directory.path())
            .await
            .unwrap();
        store
            .remember(CredentialName::Tfl, &secret("first"))
            .await
            .unwrap();
        // Something that is not a file is where the temporary one goes.
        std::fs::create_dir(directory.path().join("credentials.json.tmp")).unwrap();

        let result = store
            .remember(CredentialName::OpenLdbws, &secret("second"))
            .await;

        assert!(result.is_err());
        let keys = store.load().await.unwrap();
        assert!(!keys.contains_key(&CredentialName::OpenLdbws));
        assert_eq!(text(&keys, CredentialName::Tfl).as_deref(), Some("first"));
    }
}
