//! The keys the server uses for other services, which the owner can set while it runs.
//!
//! A key can come from three places, and the last one set wins:
//!
//! 1. **The configuration file**, read when the server starts. Optional: a server with no key in its file asks for one.
//! 2. **A key the owner set and asked to be remembered** (`displayctl credential set --remember`), kept in a private file
//!    and read back when the server starts.
//! 3. **A key the owner set**, which lives in memory alone and is gone when the server stops.
//!
//! Taking a key away (`clear`) takes away what the owner set, remembered or not, and the one in the configuration file, if
//! there is one, is used again. Setting a key without asking for it to be remembered also forgets any copy that was kept:
//! "memory only" means that nothing of it is on disk.
//!
//! Nothing here, and nothing it hands out, shows a key. What can be asked is whether there is one, where it came from, and
//! since when; a key is only ever given to the service that uses it, through [`CredentialSource`].

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

use chrono::{DateTime, Utc};
use secrecy::SecretString;
use thiserror::Error;

use crate::domain::models::credential::{CredentialName, InvalidCredential};
use crate::domain::services::clock::Clock;
use crate::domain::services::credential_source::CredentialSource;
use crate::domain::services::remembered_credentials::RememberedCredentials;

/// Where the key in use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The configuration file.
    Config,
    /// The owner set it, and it is in memory only.
    Set,
    /// The owner set it and asked for it to be kept, and it was read back from where it is kept.
    Remembered,
}

/// What can be said of a key: never the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialStatus {
    pub name: CredentialName,
    /// Where the key in use came from; none if there is no key.
    pub origin: Option<Origin>,
    /// When the owner set it. Not known for one from the configuration file.
    pub since: Option<DateTime<Utc>>,
    /// Whether a copy is kept on disk, so that it outlasts a restart.
    pub remembered: bool,
}

impl CredentialStatus {
    pub fn is_set(&self) -> bool {
        self.origin.is_some()
    }
}

#[derive(Debug, Error)]
pub enum SetError {
    #[error(transparent)]
    Invalid(#[from] InvalidCredential),
    /// The key could not be kept on disk. Nothing was changed.
    #[error("the key could not be kept: {0:#}")]
    NotKept(anyhow::Error),
    /// A key was asked to be kept but the server has nowhere to keep it.
    #[error(
        "the server has nowhere to keep a key (it keeps them with the pairing data, which it has only when it serves HTTPS)"
    )]
    NowhereToKeep,
}

#[derive(Debug, Error)]
#[error("the key could not be taken away from where it is kept: {0:#}")]
pub struct ClearError(anyhow::Error);

/// What the owner set, which takes the place of the configuration file's key.
struct Chosen {
    key: SecretString,
    origin: Origin,
    since: Option<DateTime<Utc>>,
    remembered: bool,
}

#[derive(Default)]
struct Keys {
    /// From the configuration file; kept so that `clear` can go back to it.
    config: BTreeMap<CredentialName, SecretString>,
    chosen: BTreeMap<CredentialName, Chosen>,
}

pub struct Credentials {
    keys: RwLock<Keys>,
    clock: Arc<dyn Clock>,
    /// Where keys are kept for a restart, once there is somewhere (see `keep_in`).
    kept: OnceLock<Arc<dyn RememberedCredentials>>,
    /// Held while a key is set or taken away, which changes the disk and then memory, so that two at once can't leave
    /// the two disagreeing.
    changing: tokio::sync::Mutex<()>,
}

impl Credentials {
    /// With the keys the configuration file has, if any.
    pub fn new(
        clock: Arc<dyn Clock>,
        config: impl IntoIterator<Item = (CredentialName, SecretString)>,
    ) -> Arc<Self> {
        Arc::new(Self {
            keys: RwLock::new(Keys {
                config: config.into_iter().collect(),
                chosen: BTreeMap::new(),
            }),
            clock,
            kept: OnceLock::new(),
            changing: tokio::sync::Mutex::new(()),
        })
    }

    /// Takes `store` as where keys are kept, and brings back the ones in it, which win over the configuration file's. A
    /// store that cannot be read is an error and nothing is kept in it, so a key is not set and then lost.
    pub async fn keep_in(&self, store: Arc<dyn RememberedCredentials>) -> anyhow::Result<()> {
        let remembered = store.load().await?;
        let mut keys = self.keys.write().expect("the keys are not poisoned");
        for (name, key) in remembered {
            keys.chosen.insert(
                name,
                Chosen {
                    key,
                    origin: Origin::Remembered,
                    since: None,
                    remembered: true,
                },
            );
        }
        drop(keys);
        if self.kept.set(store).is_err() {
            anyhow::bail!("the place to keep keys was given twice");
        }
        Ok(())
    }

    /// Makes `key` the key for `name`, in place of whatever was. With `remember` it is also kept for a restart; without,
    /// any copy kept before is dropped. The key is checked first and nothing changes if it is refused or can't be kept.
    pub async fn set(
        &self,
        name: CredentialName,
        key: SecretString,
        remember: bool,
    ) -> Result<CredentialStatus, SetError> {
        use secrecy::ExposeSecret;

        name.check(key.expose_secret())?;
        let _changing = self.changing.lock().await;
        // The disk is changed first, and memory only once that has worked, so the two never disagree about a key that
        // was said to be kept.
        match (remember, self.kept.get()) {
            (true, None) => return Err(SetError::NowhereToKeep),
            (true, Some(store)) => store
                .remember(name, &key)
                .await
                .map_err(SetError::NotKept)?,
            (false, Some(store)) => store.forget(name).await.map_err(SetError::NotKept)?,
            (false, None) => {}
        }
        let since = Some(self.clock.now().with_timezone(&Utc));
        let mut keys = self.keys.write().expect("the keys are not poisoned");
        keys.chosen.insert(
            name,
            Chosen {
                key,
                origin: Origin::Set,
                since,
                remembered: remember,
            },
        );
        Ok(status(&keys, name))
    }

    /// Takes away the key the owner set for `name`, and the copy kept of it. The configuration file's key, if it has one,
    /// is the key again. Having nothing set is not an error.
    pub async fn clear(&self, name: CredentialName) -> Result<CredentialStatus, ClearError> {
        let _changing = self.changing.lock().await;
        if let Some(store) = self.kept.get() {
            store.forget(name).await.map_err(ClearError)?;
        }
        let mut keys = self.keys.write().expect("the keys are not poisoned");
        keys.chosen.remove(&name);
        Ok(status(&keys, name))
    }

    /// Every key there can be, with where it stands.
    pub fn list(&self) -> Vec<CredentialStatus> {
        let keys = self.keys.read().expect("the keys are not poisoned");
        CredentialName::ALL
            .into_iter()
            .map(|name| status(&keys, name))
            .collect()
    }

    pub fn status(&self, name: CredentialName) -> CredentialStatus {
        status(&self.keys.read().expect("the keys are not poisoned"), name)
    }
}

fn status(keys: &Keys, name: CredentialName) -> CredentialStatus {
    match (keys.chosen.get(&name), keys.config.contains_key(&name)) {
        (Some(chosen), _) => CredentialStatus {
            name,
            origin: Some(chosen.origin),
            since: chosen.since,
            remembered: chosen.remembered,
        },
        (None, true) => CredentialStatus {
            name,
            origin: Some(Origin::Config),
            since: None,
            remembered: false,
        },
        (None, false) => CredentialStatus {
            name,
            origin: None,
            since: None,
            remembered: false,
        },
    }
}

impl CredentialSource for Credentials {
    fn key(&self, name: CredentialName) -> Option<SecretString> {
        let keys = self.keys.read().expect("the keys are not poisoned");
        keys.chosen
            .get(&name)
            .map(|chosen| &chosen.key)
            .or_else(|| keys.config.get(&name))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::TimeZone;
    use chrono_tz::Tz;
    use secrecy::ExposeSecret;

    use super::*;

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Tz> {
            chrono_tz::UTC
                .with_ymd_and_hms(2026, 10, 17, 12, 0, 0)
                .unwrap()
        }
    }

    /// A place to keep keys that can be made to fail.
    #[derive(Default)]
    struct Kept {
        keys: Mutex<BTreeMap<CredentialName, String>>,
        failing: Mutex<bool>,
    }

    #[async_trait::async_trait]
    impl RememberedCredentials for Kept {
        async fn load(&self) -> anyhow::Result<BTreeMap<CredentialName, SecretString>> {
            Ok(self
                .keys
                .lock()
                .unwrap()
                .iter()
                .map(|(name, key)| (*name, SecretString::from(key.clone())))
                .collect())
        }

        async fn remember(&self, name: CredentialName, key: &SecretString) -> anyhow::Result<()> {
            if *self.failing.lock().unwrap() {
                anyhow::bail!("the disk is full");
            }
            self.keys
                .lock()
                .unwrap()
                .insert(name, key.expose_secret().to_owned());
            Ok(())
        }

        async fn forget(&self, name: CredentialName) -> anyhow::Result<()> {
            if *self.failing.lock().unwrap() {
                anyhow::bail!("the disk is full");
            }
            self.keys.lock().unwrap().remove(&name);
            Ok(())
        }
    }

    const WEATHER: &str = "0123456789abcdef0123456789abcdef";
    const OTHER_WEATHER: &str = "fedcba9876543210fedcba9876543210";

    fn secret(key: &str) -> SecretString {
        SecretString::from(key.to_owned())
    }

    fn empty() -> Arc<Credentials> {
        Credentials::new(Arc::new(FixedClock), [])
    }

    fn used(credentials: &Credentials, name: CredentialName) -> Option<String> {
        credentials
            .key(name)
            .map(|key| key.expose_secret().to_owned())
    }

    #[test]
    fn a_server_with_no_keys_has_none_and_says_so_for_each() {
        let credentials = empty();

        let all = credentials.list();

        assert_eq!(all.len(), CredentialName::ALL.len());
        assert!(all.iter().all(|status| !status.is_set()));
        assert_eq!(used(&credentials, CredentialName::OpenWeather), None);
    }

    #[test]
    fn a_key_from_the_configuration_is_used_and_says_where_it_is_from() {
        let credentials = Credentials::new(
            Arc::new(FixedClock),
            [(CredentialName::OpenWeather, secret(WEATHER))],
        );

        assert_eq!(
            used(&credentials, CredentialName::OpenWeather).as_deref(),
            Some(WEATHER)
        );
        let status = credentials.status(CredentialName::OpenWeather);
        assert_eq!(status.origin, Some(Origin::Config));
        assert_eq!(status.since, None);
        assert!(!status.remembered);
    }

    #[tokio::test]
    async fn a_key_set_is_used_at_once_and_wins_over_the_configuration() {
        let credentials = Credentials::new(
            Arc::new(FixedClock),
            [(CredentialName::OpenWeather, secret(WEATHER))],
        );

        let status = credentials
            .set(CredentialName::OpenWeather, secret(OTHER_WEATHER), false)
            .await
            .unwrap();

        assert_eq!(status.origin, Some(Origin::Set));
        assert!(status.since.is_some());
        assert_eq!(
            used(&credentials, CredentialName::OpenWeather).as_deref(),
            Some(OTHER_WEATHER)
        );
    }

    #[tokio::test]
    async fn a_key_that_cannot_be_one_is_refused_and_nothing_changes() {
        let credentials = Credentials::new(
            Arc::new(FixedClock),
            [(CredentialName::OpenWeather, secret(WEATHER))],
        );

        let error = credentials
            .set(CredentialName::OpenWeather, secret("too short"), false)
            .await
            .unwrap_err();

        assert!(matches!(error, SetError::Invalid(_)), "{error}");
        assert!(!error.to_string().contains("too short"), "{error}");
        assert_eq!(
            used(&credentials, CredentialName::OpenWeather).as_deref(),
            Some(WEATHER)
        );
    }

    #[tokio::test]
    async fn clearing_goes_back_to_the_configuration_and_having_nothing_set_is_fine() {
        let credentials = Credentials::new(
            Arc::new(FixedClock),
            [(CredentialName::OpenWeather, secret(WEATHER))],
        );
        credentials
            .set(CredentialName::OpenWeather, secret(OTHER_WEATHER), false)
            .await
            .unwrap();

        let status = credentials
            .clear(CredentialName::OpenWeather)
            .await
            .unwrap();

        assert_eq!(status.origin, Some(Origin::Config));
        assert_eq!(
            used(&credentials, CredentialName::OpenWeather).as_deref(),
            Some(WEATHER)
        );
        assert!(credentials.clear(CredentialName::Tfl).await.is_ok());
    }

    #[tokio::test]
    async fn a_key_asked_to_be_kept_is_kept_and_comes_back_after_a_restart() {
        let kept = Arc::new(Kept::default());
        let first = empty();
        first.keep_in(kept.clone()).await.unwrap();

        let status = first
            .set(CredentialName::OpenWeather, secret(WEATHER), true)
            .await
            .unwrap();
        assert!(status.remembered);

        let second = empty();
        second.keep_in(kept).await.unwrap();
        let status = second.status(CredentialName::OpenWeather);
        assert_eq!(status.origin, Some(Origin::Remembered));
        assert!(status.remembered);
        assert_eq!(
            used(&second, CredentialName::OpenWeather).as_deref(),
            Some(WEATHER)
        );
    }

    #[tokio::test]
    async fn a_key_not_asked_to_be_kept_leaves_nothing_behind() {
        let kept = Arc::new(Kept::default());
        let credentials = empty();
        credentials.keep_in(kept.clone()).await.unwrap();

        credentials
            .set(CredentialName::OpenWeather, secret(WEATHER), false)
            .await
            .unwrap();

        assert!(kept.keys.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn setting_a_key_without_keeping_it_drops_the_copy_kept_before() {
        let kept = Arc::new(Kept::default());
        let credentials = empty();
        credentials.keep_in(kept.clone()).await.unwrap();
        credentials
            .set(CredentialName::OpenWeather, secret(WEATHER), true)
            .await
            .unwrap();

        credentials
            .set(CredentialName::OpenWeather, secret(OTHER_WEATHER), false)
            .await
            .unwrap();

        assert!(
            kept.keys.lock().unwrap().is_empty(),
            "the old key would come back at the next start"
        );
        assert!(!credentials.status(CredentialName::OpenWeather).remembered);
    }

    #[tokio::test]
    async fn clearing_a_key_also_drops_the_copy_kept() {
        let kept = Arc::new(Kept::default());
        let credentials = empty();
        credentials.keep_in(kept.clone()).await.unwrap();
        credentials
            .set(CredentialName::Tfl, secret("tfl-key"), true)
            .await
            .unwrap();

        credentials.clear(CredentialName::Tfl).await.unwrap();

        assert!(kept.keys.lock().unwrap().is_empty());
        assert!(!credentials.status(CredentialName::Tfl).is_set());
    }

    #[tokio::test]
    async fn a_key_that_cannot_be_kept_is_not_set_at_all() {
        let kept = Arc::new(Kept::default());
        let credentials = Credentials::new(
            Arc::new(FixedClock),
            [(CredentialName::OpenWeather, secret(WEATHER))],
        );
        credentials.keep_in(kept.clone()).await.unwrap();
        *kept.failing.lock().unwrap() = true;

        let error = credentials
            .set(CredentialName::OpenWeather, secret(OTHER_WEATHER), true)
            .await
            .unwrap_err();

        assert!(matches!(error, SetError::NotKept(_)), "{error}");
        assert_eq!(
            used(&credentials, CredentialName::OpenWeather).as_deref(),
            Some(WEATHER),
            "memory and disk must not disagree"
        );
    }

    #[tokio::test]
    async fn asking_to_keep_a_key_where_nothing_can_be_kept_says_so() {
        let credentials = empty();

        let error = credentials
            .set(CredentialName::Tfl, secret("tfl-key"), true)
            .await
            .unwrap_err();

        assert!(matches!(error, SetError::NowhereToKeep), "{error}");
        assert!(!credentials.status(CredentialName::Tfl).is_set());
    }

    #[tokio::test]
    async fn what_is_said_of_a_key_never_holds_the_key() {
        let credentials = empty();
        credentials
            .set(CredentialName::OpenWeather, secret(WEATHER), false)
            .await
            .unwrap();

        let said = format!("{:?}", credentials.list());

        assert!(!said.contains(WEATHER), "{said}");
    }
}
