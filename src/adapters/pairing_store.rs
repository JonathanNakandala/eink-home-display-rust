//! Keeps the pairings in one JSON file, so an approval, a certificate and a revocation survive a restart.
//!
//! There are a handful of displays, so the whole file is held in memory and written out whole on each
//! change. A write goes to a temporary file that is flushed and then renamed over the real one, so a
//! crash or a full disk leaves the old file or the new one, never half of one. Memory is only changed
//! once the file has been written, so a failed write is a failed change, and the next read agrees with
//! the disk.
//!
//! A file that can't be read is an error, never an empty start: the pairings are what lets the
//! displays in, and quietly forgetting them would lock every display out (or, worse, let in one that
//! had been revoked). The owner decides, by fixing or removing the file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::domain::models::device_id::DeviceId;
use crate::domain::models::pairing::{Pairing, PairingCode, PairingState, PublicKey};
use crate::domain::services::pairing_store::PairingStore;

const FILE: &str = "pairings.json";
/// Bumped when the layout changes, so an older server refuses a file it would misread.
const VERSION: u32 = 1;

pub struct FilePairingStore {
    path: PathBuf,
    pairings: Mutex<BTreeMap<DeviceId, Pairing>>,
}

impl FilePairingStore {
    /// The pairings kept in `directory`, none if there is no file yet.
    pub async fn open(directory: &Path) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(directory)
            .await
            .with_context(|| format!("Failed to create {}", directory.display()))?;
        let path = directory.join(FILE);
        let pairings = match tokio::fs::read(&path).await {
            Ok(bytes) => parse(&bytes)
                .with_context(|| format!("{} is not usable; fix or remove it", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => {
                return Err(e).with_context(|| format!("Failed to read {}", path.display()));
            }
        };
        Ok(Self {
            path,
            pairings: Mutex::new(pairings),
        })
    }

    async fn save(&self, pairings: &BTreeMap<DeviceId, Pairing>) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(&File::from(pairings))?;
        write_atomically(&self.path, &bytes)
            .await
            .with_context(|| format!("Failed to save {}", self.path.display()))
    }
}

#[async_trait::async_trait]
impl PairingStore for FilePairingStore {
    async fn get(&self, device: &DeviceId) -> anyhow::Result<Option<Pairing>> {
        Ok(self.pairings.lock().await.get(device).cloned())
    }

    async fn put(&self, pairing: &Pairing) -> anyhow::Result<()> {
        let mut held = self.pairings.lock().await;
        let mut changed = held.clone();
        changed.insert(pairing.device.clone(), pairing.clone());
        self.save(&changed).await?;
        *held = changed;
        Ok(())
    }

    async fn remove(&self, device: &DeviceId) -> anyhow::Result<()> {
        let mut held = self.pairings.lock().await;
        if !held.contains_key(device) {
            return Ok(());
        }
        let mut changed = held.clone();
        changed.remove(device);
        self.save(&changed).await?;
        *held = changed;
        Ok(())
    }

    async fn all(&self) -> anyhow::Result<Vec<Pairing>> {
        Ok(self.pairings.lock().await.values().cloned().collect())
    }
}

/// Writes `bytes` to `path` so that a reader, or a crash, sees the old contents or the new, not a mix.
async fn write_atomically(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
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

/// The file's layout. Kept apart from the domain types, so a change to those can't silently change
/// what is on disk.
#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    pairings: Vec<Record>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    device: String,
    /// The display's public key, base64 of its DER.
    key: String,
    code: String,
    state: State,
    requested_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum State {
    Pending,
    Approved,
    Enrolled {
        serial: String,
        not_after: DateTime<Utc>,
    },
    Rejected,
    Revoked,
}

impl From<&BTreeMap<DeviceId, Pairing>> for File {
    fn from(pairings: &BTreeMap<DeviceId, Pairing>) -> Self {
        Self {
            version: VERSION,
            pairings: pairings
                .values()
                .map(|p| Record {
                    device: p.device.to_string(),
                    key: STANDARD.encode(p.key.as_der()),
                    code: p.code.to_string(),
                    state: match &p.state {
                        PairingState::Pending => State::Pending,
                        PairingState::Approved => State::Approved,
                        PairingState::Enrolled { serial, not_after } => State::Enrolled {
                            serial: serial.clone(),
                            not_after: *not_after,
                        },
                        PairingState::Rejected => State::Rejected,
                        PairingState::Revoked => State::Revoked,
                    },
                    requested_at: p.requested_at,
                    updated_at: p.updated_at,
                })
                .collect(),
        }
    }
}

fn parse(bytes: &[u8]) -> anyhow::Result<BTreeMap<DeviceId, Pairing>> {
    let file: File = serde_json::from_slice(bytes).context("It is not the expected JSON")?;
    if file.version != VERSION {
        return Err(anyhow!(
            "It is version {}, and this server reads version {VERSION}",
            file.version
        ));
    }
    let mut pairings = BTreeMap::new();
    for record in file.pairings {
        let device = DeviceId::parse(&record.device)
            .with_context(|| format!("The name {:?} is not valid", record.device))?;
        let pairing = Pairing {
            key: PublicKey::from_der(
                STANDARD
                    .decode(&record.key)
                    .with_context(|| format!("The key of {device} is not base64"))?,
            ),
            code: PairingCode::parse(&record.code)
                .with_context(|| format!("The code of {device} is not valid"))?,
            state: match record.state {
                State::Pending => PairingState::Pending,
                State::Approved => PairingState::Approved,
                State::Enrolled { serial, not_after } => {
                    PairingState::Enrolled { serial, not_after }
                }
                State::Rejected => PairingState::Rejected,
                State::Revoked => PairingState::Revoked,
            },
            requested_at: record.requested_at,
            updated_at: record.updated_at,
            device,
        };
        if pairings.insert(pairing.device.clone(), pairing).is_some() {
            return Err(anyhow!("A display is listed twice"));
        }
    }
    Ok(pairings)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::domain::models::pairing::Fingerprint;

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 8, 12, minute, 0).unwrap()
    }

    fn pairing(name: &str, state: PairingState) -> Pairing {
        let device = DeviceId::parse(name).unwrap();
        let key = PublicKey::from_der(name.as_bytes().repeat(8));
        Pairing {
            code: PairingCode::derive(&Fingerprint::of(b"authority"), &device, &key),
            device,
            key,
            state,
            requested_at: at(1),
            updated_at: at(2),
        }
    }

    fn every_state() -> Vec<Pairing> {
        vec![
            pairing("a-pending", PairingState::Pending),
            pairing("b-approved", PairingState::Approved),
            pairing(
                "c-enrolled",
                PairingState::Enrolled {
                    serial: "4f00aa".to_owned(),
                    not_after: at(30),
                },
            ),
            pairing("d-rejected", PairingState::Rejected),
            pairing("e-revoked", PairingState::Revoked),
        ]
    }

    #[tokio::test]
    async fn with_no_file_there_are_no_pairings_and_none_is_made_until_one_is_kept() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path()).await.unwrap();
        assert!(store.all().await.unwrap().is_empty());
        assert!(!directory.path().join(FILE).exists());
    }

    #[tokio::test]
    async fn every_state_survives_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path()).await.unwrap();
        for p in every_state() {
            store.put(&p).await.unwrap();
        }
        drop(store);
        let reopened = FilePairingStore::open(directory.path()).await.unwrap();
        assert_eq!(reopened.all().await.unwrap(), every_state());
    }

    #[tokio::test]
    async fn they_come_back_in_name_order_whatever_order_they_were_kept_in() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path()).await.unwrap();
        for p in every_state().into_iter().rev() {
            store.put(&p).await.unwrap();
        }
        let names: Vec<String> = store
            .all()
            .await
            .unwrap()
            .iter()
            .map(|p| p.device.to_string())
            .collect();
        assert_eq!(
            names,
            [
                "a-pending",
                "b-approved",
                "c-enrolled",
                "d-rejected",
                "e-revoked"
            ]
        );
    }

    #[tokio::test]
    async fn keeping_a_display_again_replaces_it() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path()).await.unwrap();
        store
            .put(&pairing("kitchen", PairingState::Pending))
            .await
            .unwrap();
        store
            .put(&pairing("kitchen", PairingState::Approved))
            .await
            .unwrap();
        let all = store.all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].state, PairingState::Approved);
        let reopened = FilePairingStore::open(directory.path()).await.unwrap();
        assert_eq!(reopened.all().await.unwrap(), all);
    }

    #[tokio::test]
    async fn a_removed_display_stays_removed_after_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path()).await.unwrap();
        for p in every_state() {
            store.put(&p).await.unwrap();
        }
        let gone = DeviceId::parse("c-enrolled").unwrap();
        store.remove(&gone).await.unwrap();
        assert_eq!(store.get(&gone).await.unwrap(), None);
        let reopened = FilePairingStore::open(directory.path()).await.unwrap();
        assert_eq!(reopened.get(&gone).await.unwrap(), None);
        assert_eq!(reopened.all().await.unwrap().len(), 4);
        // Removing one that is not there is not an error.
        reopened.remove(&gone).await.unwrap();
    }

    #[tokio::test]
    async fn no_temporary_file_is_left_behind() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path()).await.unwrap();
        store
            .put(&pairing("kitchen", PairingState::Pending))
            .await
            .unwrap();
        let files: Vec<String> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, [FILE]);
    }

    #[tokio::test]
    async fn a_file_that_can_not_be_read_is_an_error_and_is_left_alone() {
        for contents in [
            "not json".to_owned(),
            r#"{"version":1,"pairings":[{"device":"bad name!","key":"","code":"0000-0000-0000","state":{"state":"pending"},"requested_at":"2026-10-08T12:00:00Z","updated_at":"2026-10-08T12:00:00Z"}]}"#.to_owned(),
            r#"{"version":2,"pairings":[]}"#.to_owned(),
            r#"{"version":1,"pairings":[{"device":"kitchen","key":"!!!","code":"0000-0000-0000","state":{"state":"pending"},"requested_at":"2026-10-08T12:00:00Z","updated_at":"2026-10-08T12:00:00Z"}]}"#.to_owned(),
            r#"{"version":1,"pairings":[{"device":"kitchen","key":"AA==","code":"12","state":{"state":"pending"},"requested_at":"2026-10-08T12:00:00Z","updated_at":"2026-10-08T12:00:00Z"}]}"#.to_owned(),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(FILE);
            std::fs::write(&path, &contents).unwrap();
            let error = FilePairingStore::open(directory.path())
                .await
                .err()
                .unwrap_or_else(|| panic!("should refuse {contents}"));
            assert!(format!("{error:#}").contains("fix or remove it"), "{error:#}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), contents, "left alone");
        }
    }

    #[tokio::test]
    async fn the_same_display_listed_twice_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path()).await.unwrap();
        store
            .put(&pairing("kitchen", PairingState::Pending))
            .await
            .unwrap();
        let text = std::fs::read_to_string(directory.path().join(FILE)).unwrap();
        let mut file: serde_json::Value = serde_json::from_str(&text).unwrap();
        let record = file["pairings"][0].clone();
        file["pairings"].as_array_mut().unwrap().push(record);
        std::fs::write(directory.path().join(FILE), file.to_string()).unwrap();
        let error = FilePairingStore::open(directory.path())
            .await
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("twice"), "{error:#}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_write_changes_nothing_in_memory_or_on_disk() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path()).await.unwrap();
        store
            .put(&pairing("kitchen", PairingState::Pending))
            .await
            .unwrap();
        let before = std::fs::read(directory.path().join(FILE)).unwrap();

        // A directory nobody can write to. Skip if this runs as someone who can anyway.
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let writable = std::fs::File::create(directory.path().join("probe")).is_ok();
        let outcome = if writable {
            None
        } else {
            Some(store.put(&pairing("hall", PairingState::Pending)).await)
        };
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let Some(outcome) = outcome else { return };

        assert!(outcome.is_err());
        assert_eq!(
            store.all().await.unwrap().len(),
            1,
            "memory still has only the first"
        );
        assert_eq!(std::fs::read(directory.path().join(FILE)).unwrap(), before);
    }

    #[tokio::test]
    async fn two_changes_at_once_both_land() {
        let directory = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(FilePairingStore::open(directory.path()).await.unwrap());
        let tasks: Vec<_> = (0..20)
            .map(|i| {
                let store = store.clone();
                tokio::spawn(async move {
                    store
                        .put(&pairing(&format!("device-{i:02}"), PairingState::Pending))
                        .await
                        .unwrap();
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(store.all().await.unwrap().len(), 20);
        let reopened = FilePairingStore::open(directory.path()).await.unwrap();
        assert_eq!(reopened.all().await.unwrap().len(), 20);
    }
}
