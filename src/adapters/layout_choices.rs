//! Keeps the owner's choice of layout for each display in one small JSON file, `layout-choices.json`.
//!
//! The same care as the file of pairings (see `pairing_store`) with less at stake: the whole file is held in memory and
//! written out whole on each change, through a temporary file renamed over the real one, and memory only changes once the
//! write has worked. A choice that is no longer valid (a name that could not be a display's or a layout's) is dropped with
//! a warning, since the file is the server's own and a stray entry is not worth refusing to start over; a file that is not
//! JSON at all is an error, never an empty start, so that a choice isn't quietly forgotten.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::atomic_file::write_atomically;
use crate::domain::models::device_id::DeviceId;
use crate::domain::models::layout::LayoutName;
use crate::domain::services::layout_choices::LayoutChoices;

const FILE: &str = "layout-choices.json";
/// Bumped when the layout of the file changes, so an older server refuses a file it would misread.
const VERSION: u32 = 1;

pub struct FileLayoutChoices {
    path: PathBuf,
    choices: Mutex<BTreeMap<DeviceId, LayoutName>>,
}

/// The file as it is written: names as text, kept apart from the domain types.
#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    choices: BTreeMap<String, String>,
}

impl FileLayoutChoices {
    /// The choices kept in `directory`; none if there is no file yet.
    pub async fn open(directory: &Path) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(directory)
            .await
            .with_context(|| format!("Failed to create {}", directory.display()))?;
        let path = directory.join(FILE);
        let choices = match tokio::fs::read(&path).await {
            Ok(bytes) => parse(&bytes)
                .with_context(|| format!("{} is not usable; fix or remove it", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
        };
        Ok(Self {
            path,
            choices: Mutex::new(choices),
        })
    }

    async fn save(&self, choices: &BTreeMap<DeviceId, LayoutName>) -> anyhow::Result<()> {
        let file = File {
            version: VERSION,
            choices: choices
                .iter()
                .map(|(device, layout)| (device.to_string(), layout.to_string()))
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&file)?;
        write_atomically(&self.path, &bytes)
            .await
            .with_context(|| format!("Failed to save {}", self.path.display()))
    }
}

fn parse(bytes: &[u8]) -> anyhow::Result<BTreeMap<DeviceId, LayoutName>> {
    let file: File = serde_json::from_slice(bytes).context("It is not the expected JSON")?;
    anyhow::ensure!(
        file.version == VERSION,
        "It is version {}, and this server reads version {VERSION}",
        file.version
    );
    let mut choices = BTreeMap::new();
    for (device, layout) in file.choices {
        match (DeviceId::parse(&device), LayoutName::parse(&layout)) {
            (Ok(device), Ok(layout)) => {
                choices.insert(device, layout);
            }
            _ => {
                tracing::warn!(
                    "Ignoring the layout choice {device:?}: {layout:?}, which is not valid"
                )
            }
        }
    }
    Ok(choices)
}

#[async_trait::async_trait]
impl LayoutChoices for FileLayoutChoices {
    async fn get(&self, device: &DeviceId) -> anyhow::Result<Option<LayoutName>> {
        Ok(self.choices.lock().await.get(device).cloned())
    }

    async fn set(&self, device: &DeviceId, layout: LayoutName) -> anyhow::Result<()> {
        let mut held = self.choices.lock().await;
        let mut changed = held.clone();
        changed.insert(device.clone(), layout);
        self.save(&changed).await?;
        *held = changed;
        Ok(())
    }

    async fn clear(&self, device: &DeviceId) -> anyhow::Result<()> {
        let mut held = self.choices.lock().await;
        if !held.contains_key(device) {
            return Ok(());
        }
        let mut changed = held.clone();
        changed.remove(device);
        self.save(&changed).await?;
        *held = changed;
        Ok(())
    }

    async fn all(&self) -> anyhow::Result<BTreeMap<DeviceId, LayoutName>> {
        Ok(self.choices.lock().await.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str) -> DeviceId {
        DeviceId::parse(name).unwrap()
    }

    fn layout(name: &str) -> LayoutName {
        LayoutName::parse(name).unwrap()
    }

    #[tokio::test]
    async fn a_choice_is_kept_replaced_and_taken_away_and_survives_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let choices = FileLayoutChoices::open(directory.path()).await.unwrap();
        assert!(choices.get(&device("kitchen")).await.unwrap().is_none());

        choices
            .set(&device("kitchen"), layout("departures"))
            .await
            .unwrap();
        choices.set(&device("hall"), layout("clock")).await.unwrap();
        choices
            .set(&device("kitchen"), layout("dashboard"))
            .await
            .unwrap();

        let reopened = FileLayoutChoices::open(directory.path()).await.unwrap();
        let all = reopened.all().await.unwrap();
        assert_eq!(all[&device("kitchen")], layout("dashboard"));
        assert_eq!(all[&device("hall")], layout("clock"));

        reopened.clear(&device("kitchen")).await.unwrap();
        // Nothing there to take away is fine.
        reopened.clear(&device("kitchen")).await.unwrap();
        let again = FileLayoutChoices::open(directory.path()).await.unwrap();
        assert_eq!(again.all().await.unwrap().len(), 1);
        assert!(again.get(&device("kitchen")).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn no_file_is_an_empty_start_and_writes_nothing_until_there_is_a_choice() {
        let directory = tempfile::tempdir().unwrap();

        let choices = FileLayoutChoices::open(directory.path()).await.unwrap();

        assert!(choices.all().await.unwrap().is_empty());
        assert!(!directory.path().join(FILE).exists());
    }

    #[tokio::test]
    async fn a_file_that_is_not_json_or_is_another_version_is_an_error_naming_it() {
        for text in ["not json", r#"{"version":9,"choices":{}}"#] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join(FILE), text).unwrap();

            let error = FileLayoutChoices::open(directory.path())
                .await
                .err()
                .unwrap();

            assert!(format!("{error:#}").contains(FILE), "{text}: {error:#}");
        }
    }

    #[tokio::test]
    async fn an_entry_that_is_not_valid_is_dropped_and_the_rest_kept() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(FILE),
            r#"{"version":1,"choices":{"kitchen":"departures","hall":"../etc","bad name":"clock"}}"#,
        )
        .unwrap();

        let choices = FileLayoutChoices::open(directory.path()).await.unwrap();

        let all = choices.all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[&device("kitchen")], layout("departures"));
    }

    #[tokio::test]
    async fn a_write_that_fails_changes_nothing_in_memory() {
        let directory = tempfile::tempdir().unwrap();
        let choices = FileLayoutChoices::open(directory.path()).await.unwrap();
        choices
            .set(&device("kitchen"), layout("departures"))
            .await
            .unwrap();
        // The folder has gone, so the file can't be written.
        let gone = directory.path().to_path_buf();
        drop(directory);
        assert!(!gone.exists());

        let result = choices.set(&device("kitchen"), layout("clock")).await;

        assert!(result.is_err());
        assert_eq!(
            choices.get(&device("kitchen")).await.unwrap().unwrap(),
            layout("departures")
        );
    }
}
