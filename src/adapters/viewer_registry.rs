//! Keeps the viewers in one JSON file, `viewers.json`, with the same care as the pairings (see `pairing_store`): the whole
//! file is held in memory and written out whole on each change, through a temporary file renamed over the real one, and memory
//! changes only once the write has worked.
//!
//! What is kept of a secret is its hash, never the secret. A file that can't be read is an error and never an empty start:
//! forgetting the viewers would forget which had been revoked.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::atomic_file::write_atomically;
use crate::domain::models::viewer::{
    LayoutScope, SecretHash, Viewer, ViewerId, ViewerLabel, ViewerState,
};
use crate::domain::services::viewer_registry::ViewerRegistry;

const FILE: &str = "viewers.json";
/// Bumped when the layout of the file changes, so an older server refuses a file it would misread.
const VERSION: u32 = 1;

pub struct FileViewerRegistry {
    path: PathBuf,
    viewers: Mutex<BTreeMap<ViewerId, Viewer>>,
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    viewers: Vec<Record>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    id: String,
    label: String,
    scope: String,
    lifetime_seconds: i64,
    created_at: DateTime<Utc>,
    state: State,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum State {
    Pending {
        /// SHA-256 of the code, base64url.
        code: String,
        code_expires_at: DateTime<Utc>,
    },
    Active {
        /// SHA-256 of the token, base64url.
        token: String,
        expires_at: DateTime<Utc>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_used_at: Option<DateTime<Utc>>,
    },
    Revoked {
        at: DateTime<Utc>,
    },
}

fn write_hash(hash: &SecretHash) -> String {
    URL_SAFE_NO_PAD.encode(hash.as_bytes())
}

fn read_hash(text: &str) -> anyhow::Result<SecretHash> {
    let bytes = URL_SAFE_NO_PAD
        .decode(text)
        .context("A hash is not base64url")?;
    Ok(SecretHash::from_bytes(
        bytes
            .try_into()
            .map_err(|_| anyhow!("A hash is not 32 bytes"))?,
    ))
}

impl From<&Viewer> for Record {
    fn from(viewer: &Viewer) -> Self {
        Self {
            id: viewer.id.to_string(),
            label: viewer.label.to_string(),
            scope: viewer.scope.to_scope_string(),
            lifetime_seconds: viewer.lifetime.num_seconds(),
            created_at: viewer.created_at,
            state: match &viewer.state {
                ViewerState::Pending {
                    code,
                    code_expires_at,
                } => State::Pending {
                    code: write_hash(code),
                    code_expires_at: *code_expires_at,
                },
                ViewerState::Active {
                    token,
                    expires_at,
                    last_used_at,
                } => State::Active {
                    token: write_hash(token),
                    expires_at: *expires_at,
                    last_used_at: *last_used_at,
                },
                ViewerState::Revoked { at } => State::Revoked { at: *at },
            },
        }
    }
}

impl Record {
    fn read(self) -> anyhow::Result<Viewer> {
        let id = ViewerId::parse(&self.id)
            .with_context(|| format!("The id {:?} is not valid", self.id))?;
        Ok(Viewer {
            label: ViewerLabel::parse(&self.label)
                .with_context(|| format!("The label of {id} is not valid"))?,
            scope: LayoutScope::from_scope_string(&self.scope)
                .with_context(|| format!("The scope of {id} is not valid"))?,
            lifetime: Duration::seconds(self.lifetime_seconds),
            created_at: self.created_at,
            state: match self.state {
                State::Pending {
                    code,
                    code_expires_at,
                } => ViewerState::Pending {
                    code: read_hash(&code).with_context(|| format!("The code of {id}"))?,
                    code_expires_at,
                },
                State::Active {
                    token,
                    expires_at,
                    last_used_at,
                } => ViewerState::Active {
                    token: read_hash(&token).with_context(|| format!("The token of {id}"))?,
                    expires_at,
                    last_used_at,
                },
                State::Revoked { at } => ViewerState::Revoked { at },
            },
            id,
        })
    }
}

fn parse(bytes: &[u8]) -> anyhow::Result<BTreeMap<ViewerId, Viewer>> {
    let file: File = serde_json::from_slice(bytes).context("It is not the expected JSON")?;
    anyhow::ensure!(
        file.version == VERSION,
        "It is version {}, and this server reads version {VERSION}",
        file.version
    );
    let mut viewers = BTreeMap::new();
    for record in file.viewers {
        let viewer = record.read()?;
        anyhow::ensure!(
            viewers.insert(viewer.id.clone(), viewer).is_none(),
            "A viewer is listed twice"
        );
    }
    Ok(viewers)
}

impl FileViewerRegistry {
    /// The viewers kept in `directory`; none if there is no file yet.
    pub async fn open(directory: &Path) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(directory)
            .await
            .with_context(|| format!("Failed to create {}", directory.display()))?;
        let path = directory.join(FILE);
        let viewers = match tokio::fs::read(&path).await {
            Ok(bytes) => parse(&bytes)
                .with_context(|| format!("{} is not usable; fix or remove it", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
        };
        Ok(Self {
            path,
            viewers: Mutex::new(viewers),
        })
    }

    async fn save(&self, viewers: &BTreeMap<ViewerId, Viewer>) -> anyhow::Result<()> {
        let mut records: Vec<&Viewer> = viewers.values().collect();
        records.sort_by_key(|viewer| viewer.created_at);
        let file = File {
            version: VERSION,
            viewers: records.into_iter().map(Record::from).collect(),
        };
        write_atomically(&self.path, &serde_json::to_vec_pretty(&file)?)
            .await
            .with_context(|| format!("Failed to save {}", self.path.display()))
    }
}

#[async_trait::async_trait]
impl ViewerRegistry for FileViewerRegistry {
    async fn get(&self, id: &ViewerId) -> anyhow::Result<Option<Viewer>> {
        Ok(self.viewers.lock().await.get(id).cloned())
    }

    async fn put(&self, viewer: &Viewer) -> anyhow::Result<()> {
        let mut held = self.viewers.lock().await;
        let mut changed = held.clone();
        changed.insert(viewer.id.clone(), viewer.clone());
        self.save(&changed).await?;
        *held = changed;
        Ok(())
    }

    async fn remove(&self, id: &ViewerId) -> anyhow::Result<()> {
        let mut held = self.viewers.lock().await;
        if !held.contains_key(id) {
            return Ok(());
        }
        let mut changed = held.clone();
        changed.remove(id);
        self.save(&changed).await?;
        *held = changed;
        Ok(())
    }

    async fn all(&self) -> anyhow::Result<Vec<Viewer>> {
        let mut all: Vec<Viewer> = self.viewers.lock().await.values().cloned().collect();
        all.sort_by_key(|viewer| viewer.created_at);
        Ok(all)
    }

    async fn find_by_code(&self, code: &SecretHash) -> anyhow::Result<Option<Viewer>> {
        Ok(self
            .viewers
            .lock()
            .await
            .values()
            .find(|viewer| matches!(&viewer.state, ViewerState::Pending { code: held, .. } if held == code))
            .cloned())
    }

    async fn find_by_token(&self, token: &SecretHash) -> anyhow::Result<Option<Viewer>> {
        Ok(self
            .viewers
            .lock()
            .await
            .values()
            .find(|viewer| matches!(&viewer.state, ViewerState::Active { token: held, .. } if held == token))
            .cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::viewer::{ExchangeCode, ViewerToken};

    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_790_000_000, 0).unwrap() + Duration::minutes(minutes)
    }

    fn viewer(seed: u8, state: ViewerState) -> Viewer {
        Viewer {
            id: ViewerId::from_random(&[seed; 5]),
            label: ViewerLabel::parse(&format!("viewer {seed}")).unwrap(),
            scope: LayoutScope::All,
            lifetime: Duration::days(90),
            created_at: at(i64::from(seed)),
            state,
        }
    }

    fn waiting(seed: u8) -> Viewer {
        viewer(
            seed,
            ViewerState::Pending {
                code: ExchangeCode::generate([seed; 32]).hash(),
                code_expires_at: at(60),
            },
        )
    }

    #[tokio::test]
    async fn viewers_of_every_state_survive_a_restart_with_what_was_kept_of_them() {
        let directory = tempfile::tempdir().unwrap();
        let registry = FileViewerRegistry::open(directory.path()).await.unwrap();
        let mut some = waiting(1);
        some.scope = LayoutScope::of(vec![
            crate::domain::models::layout::LayoutName::parse("departures").unwrap(),
        ]);
        let active = viewer(
            2,
            ViewerState::Active {
                token: ViewerToken::generate([2; 32]).hash(),
                expires_at: at(1000),
                last_used_at: Some(at(5)),
            },
        );
        let revoked = viewer(3, ViewerState::Revoked { at: at(7) });
        for v in [&some, &active, &revoked] {
            registry.put(v).await.unwrap();
        }

        let reopened = FileViewerRegistry::open(directory.path()).await.unwrap();

        assert_eq!(reopened.all().await.unwrap(), [some, active, revoked]);
    }

    #[tokio::test]
    async fn only_hashes_are_written_and_a_secret_is_found_by_its_hash() {
        let directory = tempfile::tempdir().unwrap();
        let registry = FileViewerRegistry::open(directory.path()).await.unwrap();
        let code = ExchangeCode::generate([4; 32]);
        let token = ViewerToken::generate([5; 32]);
        registry
            .put(&viewer(
                4,
                ViewerState::Pending {
                    code: code.hash(),
                    code_expires_at: at(60),
                },
            ))
            .await
            .unwrap();
        registry
            .put(&viewer(
                5,
                ViewerState::Active {
                    token: token.hash(),
                    expires_at: at(1000),
                    last_used_at: None,
                },
            ))
            .await
            .unwrap();

        let written = std::fs::read_to_string(directory.path().join(FILE)).unwrap();

        assert!(!written.contains(&code.expose()[5..]) && !written.contains(&token.expose()[5..]));
        assert_eq!(
            registry
                .find_by_code(&code.hash())
                .await
                .unwrap()
                .unwrap()
                .id,
            ViewerId::from_random(&[4; 5])
        );
        assert_eq!(
            registry
                .find_by_token(&token.hash())
                .await
                .unwrap()
                .unwrap()
                .id,
            ViewerId::from_random(&[5; 5])
        );
        // A code is not a token and the reverse, and nothing unknown is found.
        assert!(
            registry
                .find_by_token(&code.hash())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            registry
                .find_by_code(&token.hash())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            registry
                .find_by_token(&ViewerToken::generate([9; 32]).hash())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_viewer_is_replaced_by_id_and_removed_and_removing_what_is_not_there_is_fine() {
        let directory = tempfile::tempdir().unwrap();
        let registry = FileViewerRegistry::open(directory.path()).await.unwrap();
        let mut one = waiting(1);
        registry.put(&one).await.unwrap();
        one.state = ViewerState::Revoked { at: at(9) };
        registry.put(&one).await.unwrap();
        assert_eq!(registry.all().await.unwrap(), [one.clone()]);

        registry.remove(&one.id).await.unwrap();
        registry.remove(&one.id).await.unwrap();

        assert!(registry.all().await.unwrap().is_empty());
        assert!(registry.get(&one.id).await.unwrap().is_none());
        let again = FileViewerRegistry::open(directory.path()).await.unwrap();
        assert!(again.all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn no_file_is_an_empty_start_and_a_file_that_is_not_usable_is_an_error_naming_it() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            FileViewerRegistry::open(directory.path())
                .await
                .unwrap()
                .all()
                .await
                .unwrap()
                .is_empty()
        );
        assert!(!directory.path().join(FILE).exists());

        for text in [
            "not json",
            r#"{"version":9,"viewers":[]}"#,
            r#"{"version":1,"viewers":[{"id":"BAD","label":"x","scope":"layout:*","lifetime_seconds":1,"created_at":"2026-10-12T10:00:00Z","state":{"state":"revoked","at":"2026-10-12T10:00:00Z"}}]}"#,
        ] {
            let bad = tempfile::tempdir().unwrap();
            std::fs::write(bad.path().join(FILE), text).unwrap();
            let error = FileViewerRegistry::open(bad.path()).await.err().unwrap();
            assert!(format!("{error:#}").contains(FILE), "{text}: {error:#}");
        }
    }

    #[tokio::test]
    async fn a_write_that_fails_changes_nothing_in_memory() {
        let directory = tempfile::tempdir().unwrap();
        let registry = FileViewerRegistry::open(directory.path()).await.unwrap();
        registry.put(&waiting(1)).await.unwrap();
        let gone = directory.path().to_path_buf();
        drop(directory);
        assert!(!gone.exists());

        assert!(registry.put(&waiting(2)).await.is_err());

        assert_eq!(registry.all().await.unwrap().len(), 1);
    }
}
