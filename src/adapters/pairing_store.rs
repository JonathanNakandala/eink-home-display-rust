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
use crate::domain::models::pairing::{Pairing, PairingState, PublicKey, Replacement, Rollover};
use crate::domain::models::profile::DeviceProfile;
use crate::domain::services::pairing_store::PairingStore;

const FILE: &str = "pairings.json";
/// The version before the last change, kept beside the file.
const PREVIOUS: &str = "pairings.json.bak";
/// Bumped when the layout changes, so an older server refuses a file it would misread.
const VERSION: u32 = 1;

pub struct FilePairingStore {
    path: PathBuf,
    pairings: Mutex<BTreeMap<DeviceId, Pairing>>,
}

/// What to do when there is no file of pairings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missing {
    /// Start with none, and write the empty file at once, so that its being gone later means something.
    StartEmpty,
    /// An error. Used when the display's certificates already exist: a missing file then is a file that
    /// was lost, and starting with no members would turn away every display that holds a good certificate.
    Refuse,
}

impl FilePairingStore {
    /// The pairings kept in `directory`.
    pub async fn open(directory: &Path, missing: Missing) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(directory)
            .await
            .with_context(|| format!("Failed to create {}", directory.display()))?;
        let path = directory.join(FILE);
        let previous = directory.join(PREVIOUS);
        let (pairings, found) = match tokio::fs::read(&path).await {
            Ok(bytes) => (
                parse(&bytes).with_context(|| {
                    let hint = if previous.exists() {
                        format!(
                            " (the version before the last change is {})",
                            previous.display()
                        )
                    } else {
                        String::new()
                    };
                    format!("{} is not usable; fix or remove it{hint}", path.display())
                })?,
                true,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if previous.exists() {
                    return Err(anyhow!(
                        "{} is missing but {} is there; restore it (copy the .bak back) or delete the .bak to start with no displays",
                        path.display(),
                        previous.display()
                    ));
                }
                if missing == Missing::Refuse {
                    return Err(anyhow!("{} is missing", path.display()));
                }
                (BTreeMap::new(), false)
            }
            Err(e) => {
                return Err(e).with_context(|| format!("Failed to read {}", path.display()));
            }
        };
        let store = Self {
            path,
            pairings: Mutex::new(pairings),
        };
        if !found {
            store.save(&BTreeMap::new()).await?;
        }
        Ok(store)
    }

    async fn save(&self, pairings: &BTreeMap<DeviceId, Pairing>) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec_pretty(&File::from(pairings))?;
        // Keep the version being replaced, for the day the new one turns out to be wrong (or is deleted).
        if let Some(directory) = self.path.parent() {
            match tokio::fs::copy(&self.path, directory.join(PREVIOUS)).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => log::warn!("Could not keep the previous list of displays: {e}"),
            }
        }
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
    state: State,
    requested_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// Added after version 1 first shipped; a file without it is read as having none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rollover: Option<RolloverRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replacement: Option<ReplacementRecord>,
    /// What the display said it is. Also added after version 1 first shipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<ProfileRecord>,
}

/// What a display said it is (domain/models/profile.rs), as it is kept in the file.
#[derive(Serialize, Deserialize)]
struct ProfileRecord {
    model: String,
    firmware: String,
    width: u32,
    height: u32,
    levels: u32,
    formats: Vec<String>,
}

impl ProfileRecord {
    fn of(profile: &DeviceProfile) -> Self {
        Self {
            model: profile.model.clone(),
            firmware: profile.firmware.clone(),
            width: profile.width,
            height: profile.height,
            levels: profile.levels,
            formats: profile.formats.clone(),
        }
    }

    /// Read back through the same rules as a profile from a display: a profile that does not pass them is left out and
    /// the display is kept, since the file is the server's own and what it says about a display is not worth refusing to
    /// start over.
    fn read(self, device: &DeviceId) -> Option<DeviceProfile> {
        DeviceProfile::new(
            &self.model,
            &self.firmware,
            self.width,
            self.height,
            self.levels,
            self.formats,
        )
        .inspect_err(|e| log::warn!("Ignoring the profile kept for {device}: {e}"))
        .ok()
    }
}

#[derive(Serialize, Deserialize)]
struct RolloverRecord {
    key: String,
    since: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
struct ReplacementRecord {
    key: String,
    approved: bool,
    requested_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<ProfileRecord>,
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
                    rollover: p.rollover.as_ref().map(|r| RolloverRecord {
                        key: STANDARD.encode(r.key.as_der()),
                        since: r.since,
                    }),
                    replacement: p.replacement.as_ref().map(|r| ReplacementRecord {
                        key: STANDARD.encode(r.key.as_der()),
                        approved: r.approved,
                        requested_at: r.requested_at,
                        profile: r.profile.as_ref().map(ProfileRecord::of),
                    }),
                    profile: p.profile.as_ref().map(ProfileRecord::of),
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
        let pairing =
            Pairing {
                key: PublicKey::from_der(
                    STANDARD
                        .decode(&record.key)
                        .with_context(|| format!("The key of {device} is not base64"))?,
                ),
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
                rollover: record
                    .rollover
                    .map(|r| {
                        Ok::<_, anyhow::Error>(Rollover {
                            key: PublicKey::from_der(STANDARD.decode(&r.key).with_context(
                                || format!("The new key of {device} is not base64"),
                            )?),
                            since: r.since,
                        })
                    })
                    .transpose()?,
                replacement: record
                    .replacement
                    .map(|r| {
                        Ok::<_, anyhow::Error>(Replacement {
                            key: PublicKey::from_der(STANDARD.decode(&r.key).with_context(
                                || format!("The replacement key of {device} is not base64"),
                            )?),
                            approved: r.approved,
                            requested_at: r.requested_at,
                            profile: r.profile.and_then(|profile| profile.read(&device)),
                        })
                    })
                    .transpose()?,
                profile: record.profile.and_then(|profile| profile.read(&device)),
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

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 8, 12, minute, 0).unwrap()
    }

    fn pairing(name: &str, state: PairingState) -> Pairing {
        let device = DeviceId::parse(name).unwrap();
        let key = PublicKey::from_der(name.as_bytes().repeat(8));
        let mut pairing = Pairing::new(device, key, state, at(1));
        pairing.updated_at = at(2);
        pairing
    }

    /// A member in the middle of changing keys, and another key asking to take its name.
    fn changing_keys() -> Pairing {
        let mut member = pairing(
            "f-changing",
            PairingState::Enrolled {
                serial: "01".to_owned(),
                not_after: at(30),
            },
        );
        let next = PublicKey::from_der(vec![7; 91]);
        member.rollover = Some(Rollover {
            key: next,
            since: at(3),
        });
        let other = PublicKey::from_der(vec![9; 91]);
        member.replacement = Some(Replacement {
            key: other,
            approved: true,
            requested_at: at(4),
            profile: None,
        });
        member
    }

    fn profile(model: &str) -> DeviceProfile {
        DeviceProfile::new(
            model,
            "0.2.0",
            1872,
            1404,
            16,
            vec!["bmp".to_owned(), "png".to_owned()],
        )
        .unwrap()
    }

    #[test]
    fn a_profile_survives_the_file_and_so_does_the_one_of_a_waiting_key() {
        let mut member = Pairing::new(
            DeviceId::parse("kitchen").unwrap(),
            PublicKey::from_der(vec![1; 91]),
            PairingState::Pending,
            at(1),
        );
        member.profile = Some(profile("reTerminal E1003"));
        member.replacement = Some(Replacement {
            key: PublicKey::from_der(vec![9; 91]),
            approved: false,
            requested_at: at(4),
            profile: Some(profile("Other Model")),
        });
        let mut all = BTreeMap::new();
        all.insert(member.device.clone(), member.clone());
        let bytes = serde_json::to_vec_pretty(&File::from(&all)).unwrap();
        let read = parse(&bytes).unwrap();
        assert_eq!(read[&member.device], member);
    }

    #[test]
    fn a_file_from_before_profiles_is_read_as_having_none() {
        let old = r#"{"version":1,"pairings":[{"device":"kitchen","key":"AQID","state":{"state":"pending"},"requested_at":"2026-10-08T12:00:00Z","updated_at":"2026-10-08T12:00:00Z"}]}"#;
        let read = parse(old.as_bytes()).unwrap();
        assert!(read[&DeviceId::parse("kitchen").unwrap()].profile.is_none());
    }

    #[test]
    fn a_profile_in_the_file_that_does_not_pass_the_rules_is_dropped_and_the_display_is_kept() {
        let odd = r#"{"version":1,"pairings":[{"device":"kitchen","key":"AQID","state":{"state":"pending"},"requested_at":"2026-10-08T12:00:00Z","updated_at":"2026-10-08T12:00:00Z","profile":{"model":"<script>","firmware":"0.2.0","width":1,"height":1,"levels":16,"formats":[]}}]}"#;
        let read = parse(odd.as_bytes()).unwrap();
        let kitchen = &read[&DeviceId::parse("kitchen").unwrap()];
        assert!(kitchen.profile.is_none());
        assert!(matches!(kitchen.state, PairingState::Pending));
    }

    #[tokio::test]
    async fn a_key_change_and_a_replacement_waiting_survive_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        store.put(&changing_keys()).await.unwrap();
        drop(store);
        let reopened = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        assert_eq!(reopened.all().await.unwrap(), [changing_keys()]);
    }

    #[tokio::test]
    async fn a_file_from_before_key_changes_existed_is_read_as_having_none() {
        // The layout is only added to, so a file an earlier version wrote still loads.
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        store
            .put(&pairing("kitchen", PairingState::Pending))
            .await
            .unwrap();
        let text = std::fs::read_to_string(directory.path().join(FILE)).unwrap();
        assert!(
            !text.contains("rollover") && !text.contains("replacement"),
            "{text}"
        );
        let reopened = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        let loaded = reopened.all().await.unwrap();
        assert!(loaded[0].rollover.is_none() && loaded[0].replacement.is_none());
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
    async fn with_no_file_there_are_no_pairings_and_an_empty_file_is_made_at_once() {
        // Made at once so that its being gone later is something that can be noticed.
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        assert!(store.all().await.unwrap().is_empty());
        assert!(directory.path().join(FILE).exists());
        let reopened = FilePairingStore::open(directory.path(), Missing::Refuse)
            .await
            .unwrap();
        assert!(reopened.all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_missing_file_is_refused_when_there_should_be_one() {
        let directory = tempfile::tempdir().unwrap();
        let error = FilePairingStore::open(directory.path(), Missing::Refuse)
            .await
            .err()
            .expect("should refuse");
        assert!(error.to_string().contains("is missing"), "{error:#}");
        // And nothing was made for it.
        assert!(!directory.path().join(FILE).exists());
    }

    #[tokio::test]
    async fn the_version_before_the_last_change_is_kept() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        store
            .put(&pairing("kitchen", PairingState::Pending))
            .await
            .unwrap();
        store
            .put(&pairing("hall", PairingState::Pending))
            .await
            .unwrap();
        let previous = std::fs::read_to_string(directory.path().join(PREVIOUS)).unwrap();
        assert!(
            previous.contains("kitchen") && !previous.contains("hall"),
            "{previous}"
        );
        let current = std::fs::read_to_string(directory.path().join(FILE)).unwrap();
        assert!(current.contains("kitchen") && current.contains("hall"));
    }

    #[tokio::test]
    async fn a_deleted_file_with_its_backup_still_there_stops_start_up_and_says_how_to_restore() {
        for missing in [Missing::StartEmpty, Missing::Refuse] {
            let directory = tempfile::tempdir().unwrap();
            let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
                .await
                .unwrap();
            store
                .put(&pairing("kitchen", PairingState::Pending))
                .await
                .unwrap();
            store
                .put(&pairing("hall", PairingState::Pending))
                .await
                .unwrap();
            drop(store);
            std::fs::remove_file(directory.path().join(FILE)).unwrap();

            let error = FilePairingStore::open(directory.path(), missing)
                .await
                .err()
                .expect("should refuse");
            let text = format!("{error:#}");
            assert!(
                text.contains("is missing") && text.contains(".bak"),
                "{text}"
            );
            // Nothing was made or overwritten, so the backup is still the way back.
            assert!(!directory.path().join(FILE).exists());
            assert!(directory.path().join(PREVIOUS).exists());
        }
    }

    #[tokio::test]
    async fn an_unreadable_file_points_at_the_backup_when_there_is_one() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        store
            .put(&pairing("kitchen", PairingState::Pending))
            .await
            .unwrap();
        store
            .put(&pairing("hall", PairingState::Pending))
            .await
            .unwrap();
        drop(store);
        std::fs::write(directory.path().join(FILE), "damaged").unwrap();
        let error = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .err()
            .expect("should refuse");
        assert!(format!("{error:#}").contains(".bak"), "{error:#}");
    }

    #[tokio::test]
    async fn every_state_survives_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        for p in every_state() {
            store.put(&p).await.unwrap();
        }
        drop(store);
        let reopened = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        assert_eq!(reopened.all().await.unwrap(), every_state());
    }

    #[tokio::test]
    async fn they_come_back_in_name_order_whatever_order_they_were_kept_in() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
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
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
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
        let reopened = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        assert_eq!(reopened.all().await.unwrap(), all);
    }

    #[tokio::test]
    async fn a_removed_display_stays_removed_after_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        for p in every_state() {
            store.put(&p).await.unwrap();
        }
        let gone = DeviceId::parse("c-enrolled").unwrap();
        store.remove(&gone).await.unwrap();
        assert_eq!(store.get(&gone).await.unwrap(), None);
        let reopened = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        assert_eq!(reopened.get(&gone).await.unwrap(), None);
        assert_eq!(reopened.all().await.unwrap().len(), 4);
        // Removing one that is not there is not an error.
        reopened.remove(&gone).await.unwrap();
    }

    #[tokio::test]
    async fn no_temporary_file_is_left_behind() {
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        store
            .put(&pairing("kitchen", PairingState::Pending))
            .await
            .unwrap();
        let mut files: Vec<String> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        // The list, and the version before its last change; no half-written file between them.
        assert_eq!(files, [FILE, PREVIOUS]);
    }

    #[tokio::test]
    async fn a_file_that_can_not_be_read_is_an_error_and_is_left_alone() {
        for contents in [
            "not json".to_owned(),
            r#"{"version":1,"pairings":[{"device":"bad name!","key":"","code":"0000-0000-0000","state":{"state":"pending"},"requested_at":"2026-10-08T12:00:00Z","updated_at":"2026-10-08T12:00:00Z"}]}"#.to_owned(),
            r#"{"version":2,"pairings":[]}"#.to_owned(),
            r#"{"version":1,"pairings":[{"device":"kitchen","key":"!!!","code":"0000-0000-0000","state":{"state":"pending"},"requested_at":"2026-10-08T12:00:00Z","updated_at":"2026-10-08T12:00:00Z"}]}"#.to_owned(),
            r#"{"version":1,"pairings":[{"device":"kitchen","key":"AA==","state":{"state":"not-a-state"},"requested_at":"2026-10-08T12:00:00Z","updated_at":"2026-10-08T12:00:00Z"}]}"#.to_owned(),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join(FILE);
            std::fs::write(&path, &contents).unwrap();
            let error = FilePairingStore::open(directory.path(), Missing::StartEmpty)
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
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        store
            .put(&pairing("kitchen", PairingState::Pending))
            .await
            .unwrap();
        let text = std::fs::read_to_string(directory.path().join(FILE)).unwrap();
        let mut file: serde_json::Value = serde_json::from_str(&text).unwrap();
        let record = file["pairings"][0].clone();
        file["pairings"].as_array_mut().unwrap().push(record);
        std::fs::write(directory.path().join(FILE), file.to_string()).unwrap();
        let error = FilePairingStore::open(directory.path(), Missing::StartEmpty)
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
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
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
        let store = std::sync::Arc::new(
            FilePairingStore::open(directory.path(), Missing::StartEmpty)
                .await
                .unwrap(),
        );
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
        let reopened = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        assert_eq!(reopened.all().await.unwrap().len(), 20);
    }

    #[tokio::test]
    async fn no_code_is_written_to_the_file() {
        // The code is worked out when one is typed and never kept, so nobody reading the file can copy it.
        let directory = tempfile::tempdir().unwrap();
        let store = FilePairingStore::open(directory.path(), Missing::StartEmpty)
            .await
            .unwrap();
        let mut member = changing_keys();
        member.state = PairingState::Pending;
        store.put(&member).await.unwrap();
        store
            .put(&pairing("hall", PairingState::Pending))
            .await
            .unwrap();
        let text = std::fs::read_to_string(directory.path().join(FILE)).unwrap();
        assert!(!text.contains("\"code\""), "{text}");
        let fingerprint = crate::domain::models::pairing::Fingerprint::of(b"authority");
        for p in store.all().await.unwrap() {
            let code = crate::domain::models::pairing::PairingCode::derive(
                &fingerprint,
                &p.device,
                &p.key,
            );
            assert!(
                !text.contains(code.as_str()) && !text.contains(&code.as_str().replace('-', "")),
                "{text}"
            );
        }
        let backup = std::fs::read_to_string(directory.path().join(PREVIOUS)).unwrap();
        assert!(!backup.contains("\"code\""), "{backup}");
    }

    #[tokio::test]
    async fn a_file_that_still_has_codes_in_it_is_read_and_the_codes_are_dropped_when_it_is_next_written()
     {
        let directory = tempfile::tempdir().unwrap();
        let old = r#"{"version":1,"pairings":[{"device":"kitchen","key":"AAAA","code":"0000-0000-0000","state":{"state":"pending"},"requested_at":"2026-10-08T12:00:00Z","updated_at":"2026-10-08T12:00:00Z","replacement":{"key":"BBBB","code":"1111-1111-1111","approved":false,"requested_at":"2026-10-08T12:00:00Z"}}]}"#;
        std::fs::write(directory.path().join(FILE), old).unwrap();
        let store = FilePairingStore::open(directory.path(), Missing::Refuse)
            .await
            .unwrap();
        let all = store.all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].replacement.is_some());
        store.put(&all[0]).await.unwrap();
        let text = std::fs::read_to_string(directory.path().join(FILE)).unwrap();
        assert!(!text.contains("code"), "{text}");
    }
}
