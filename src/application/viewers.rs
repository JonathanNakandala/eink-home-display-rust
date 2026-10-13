//! Who may look at the picture in a web browser, when the page isn't open to everyone.
//!
//! The owner **issues** a viewer and is given a one-time code. The browser **exchanges** the code for an access token,
//! once; the token is what **authenticates** each later request, until it runs out or the owner **revokes** it. See
//! `domain::models::viewer` for what the secrets are and what is kept of them.
//!
//! A change is a read, a decision and a write, so changes are made one at a time: a code can't be exchanged twice, and a
//! viewer revoked in the same instant as it is used stays revoked.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::domain::models::layout::Layouts;
use crate::domain::models::viewer::{
    ExchangeCode, LayoutScope, Viewer, ViewerError, ViewerId, ViewerLabel, ViewerState, ViewerToken,
};
use crate::domain::services::clock::Clock;
use crate::domain::services::entropy::Entropy;
use crate::domain::services::viewer_registry::ViewerRegistry;

/// The most viewers kept at once, expired and revoked ones included until the owner forgets them: more than a household
/// will ever have, and a bound on what a mistake or a script can pile up.
pub const MOST_VIEWERS: usize = 64;
/// How long a token lasts, unless the owner says: a quarter of a year, which is long enough not to be a chore for a wall
/// tablet and short enough that a lost one doesn't stay good for ever.
pub const DEFAULT_LIFETIME_DAYS: u32 = 90;
pub const MOST_LIFETIME_DAYS: u32 = 3650;
/// How long the link works, unless the owner says.
pub const DEFAULT_CODE_MINUTES: u32 = 15;
pub const MOST_CODE_MINUTES: u32 = 24 * 60;
/// How often the time a token was last used is written down: a request is far more often than that, and each write is the
/// whole file.
const LAST_USED_EVERY: Duration = Duration::minutes(10);

/// What the owner asks for.
#[derive(Debug, Clone, Default)]
pub struct IssueRequest {
    pub label: String,
    /// The layouts the viewer may look at, by name; none means all of them.
    pub layouts: Vec<String>,
    pub lifetime_days: Option<u32>,
    pub code_minutes: Option<u32>,
}

/// A viewer made, and the code for its link, which is shown now and never again.
#[derive(Debug)]
pub struct Issued {
    pub viewer: Viewer,
    pub code: ExchangeCode,
}

#[derive(Debug, Error)]
pub enum IssueError {
    #[error(transparent)]
    Invalid(#[from] ViewerError),
    #[error("There is no layout called {0}")]
    UnknownLayout(String),
    #[error("A token lasts from 1 to {MOST_LIFETIME_DAYS} days")]
    InvalidLifetime,
    #[error("The link works for from 1 to {MOST_CODE_MINUTES} minutes")]
    InvalidCodeTime,
    #[error("There are {MOST_VIEWERS} viewers already; forget some that are no longer wanted")]
    TooMany,
    #[error("the server failed: {0:#}")]
    Failed(#[from] anyhow::Error),
}

/// A token, made when a code is exchanged, which is the only time it is known.
#[derive(Debug)]
pub struct Grant {
    pub token: ViewerToken,
    pub expires_in: Duration,
    pub scope: LayoutScope,
    pub viewer: ViewerId,
}

#[derive(Debug, Error)]
pub enum ExchangeError {
    /// Not a code, not one there is, one already used, or one that has run out: not told apart, so that what is sent
    /// can't be used to find out which codes there are.
    #[error("the code is not good")]
    InvalidGrant,
    #[error("the server failed: {0:#}")]
    Failed(#[from] anyhow::Error),
}

/// Who a token belongs to.
#[derive(Debug, Clone)]
pub struct Authenticated {
    pub viewer: ViewerId,
    pub label: ViewerLabel,
    pub scope: LayoutScope,
}

#[derive(Debug, Error)]
pub enum AuthError {
    /// Not a token, not one there is, one that has run out, or one revoked: not told apart.
    #[error("the token is not good")]
    InvalidToken,
    #[error("the server failed: {0:#}")]
    Failed(#[from] anyhow::Error),
}

#[derive(Debug, Error)]
pub enum ChangeError {
    #[error("no viewer has the id {0}")]
    Unknown(ViewerId),
    #[error("the server failed: {0:#}")]
    Failed(#[from] anyhow::Error),
}

pub struct Viewers {
    registry: Arc<dyn ViewerRegistry>,
    clock: Arc<dyn Clock>,
    entropy: Arc<dyn Entropy>,
    changes: Mutex<()>,
}

impl Viewers {
    pub fn new(
        registry: Arc<dyn ViewerRegistry>,
        clock: Arc<dyn Clock>,
        entropy: Arc<dyn Entropy>,
    ) -> Self {
        Self {
            registry,
            clock,
            entropy,
            changes: Mutex::new(()),
        }
    }

    fn now(&self) -> DateTime<Utc> {
        self.clock.now().to_utc()
    }

    fn random(&self) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        self.entropy.fill(&mut bytes);
        bytes
    }

    /// Makes a viewer, waiting for its link to be used. It may be given only layouts that are among `layouts`.
    pub async fn issue(
        &self,
        request: IssueRequest,
        layouts: &Layouts,
    ) -> Result<Issued, IssueError> {
        let label = ViewerLabel::parse(&request.label)?;
        let scope = request
            .layouts
            .iter()
            .map(|name| {
                layouts
                    .find(name)
                    .map(|layout| layout.name.clone())
                    .ok_or_else(|| IssueError::UnknownLayout(name.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let lifetime_days = request.lifetime_days.unwrap_or(DEFAULT_LIFETIME_DAYS);
        if !(1..=MOST_LIFETIME_DAYS).contains(&lifetime_days) {
            return Err(IssueError::InvalidLifetime);
        }
        let code_minutes = request.code_minutes.unwrap_or(DEFAULT_CODE_MINUTES);
        if !(1..=MOST_CODE_MINUTES).contains(&code_minutes) {
            return Err(IssueError::InvalidCodeTime);
        }

        let _changing = self.changes.lock().await;
        if self.registry.all().await?.len() >= MOST_VIEWERS {
            return Err(IssueError::TooMany);
        }
        let now = self.now();
        let code = ExchangeCode::generate(self.random());
        let viewer = Viewer {
            id: ViewerId::from_random(&self.random()),
            label,
            scope: LayoutScope::of(scope),
            lifetime: Duration::days(lifetime_days.into()),
            created_at: now,
            state: ViewerState::Pending {
                code: code.hash(),
                code_expires_at: now + Duration::minutes(code_minutes.into()),
            },
        };
        self.registry.put(&viewer).await?;
        log::info!(
            "Made the viewer {} ({}) for {}; its link works for {code_minutes} minute(s)",
            viewer.id,
            viewer.label,
            viewer.scope.to_scope_string()
        );
        Ok(Issued { viewer, code })
    }

    /// Trades a code for a token, once. A code that is not good in any way is `InvalidGrant`.
    pub async fn exchange(&self, code: &str) -> Result<Grant, ExchangeError> {
        let Ok(code) = ExchangeCode::parse(code) else {
            return Err(ExchangeError::InvalidGrant);
        };
        let _changing = self.changes.lock().await;
        let now = self.now();
        let Some(mut viewer) = self.registry.find_by_code(&code.hash()).await? else {
            return Err(ExchangeError::InvalidGrant);
        };
        if !matches!(
            viewer.standing(now),
            crate::domain::models::viewer::ViewerStanding::Waiting
        ) {
            log::info!("A code for {} was used after it ran out", viewer.id);
            return Err(ExchangeError::InvalidGrant);
        }
        let token = ViewerToken::generate(self.random());
        let expires_at = now + viewer.lifetime;
        viewer.state = ViewerState::Active {
            token: token.hash(),
            expires_at,
            last_used_at: None,
        };
        // Written before the token is given out, so a token that fails to be kept is never handed over, and the code
        // that is spent is spent.
        self.registry.put(&viewer).await?;
        log::info!("{} ({}) has signed in", viewer.id, viewer.label);
        Ok(Grant {
            token,
            expires_in: expires_at - now,
            scope: viewer.scope,
            viewer: viewer.id,
        })
    }

    /// Who a token belongs to, if it is good.
    pub async fn authenticate(&self, token: &str) -> Result<Authenticated, AuthError> {
        let Ok(token) = ViewerToken::parse(token) else {
            return Err(AuthError::InvalidToken);
        };
        let hash = token.hash();
        let now = self.now();
        let Some(viewer) = self.registry.find_by_token(&hash).await? else {
            return Err(AuthError::InvalidToken);
        };
        let ViewerState::Active {
            expires_at,
            last_used_at,
            ..
        } = &viewer.state
        else {
            return Err(AuthError::InvalidToken);
        };
        if now >= *expires_at {
            return Err(AuthError::InvalidToken);
        }
        if last_used_at.is_none_or(|used| now - used >= LAST_USED_EVERY) {
            self.note_use(&viewer.id, &token, now).await;
        }
        Ok(Authenticated {
            viewer: viewer.id,
            label: viewer.label,
            scope: viewer.scope,
        })
    }

    /// Notes that `token` was used, if it still is the viewer's, so a revoke that came just before isn't undone. Best
    /// effort: a request is worth more than knowing when it was made.
    async fn note_use(&self, id: &ViewerId, token: &ViewerToken, now: DateTime<Utc>) {
        let _changing = self.changes.lock().await;
        let outcome = async {
            let Some(mut viewer) = self.registry.get(id).await? else {
                return Ok(());
            };
            if let ViewerState::Active {
                token: held,
                last_used_at,
                ..
            } = &mut viewer.state
                && *held == token.hash()
            {
                *last_used_at = Some(now);
                self.registry.put(&viewer).await?;
            }
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = outcome {
            log::warn!("Could not note that {id} was used: {e:#}");
        }
    }

    /// Ends a viewer's access at once, whatever state it is in. Revoking one already revoked is not an error.
    pub async fn revoke(&self, id: &ViewerId) -> Result<Viewer, ChangeError> {
        let _changing = self.changes.lock().await;
        let mut viewer = self
            .registry
            .get(id)
            .await?
            .ok_or_else(|| ChangeError::Unknown(id.clone()))?;
        if !matches!(viewer.state, ViewerState::Revoked { .. }) {
            viewer.state = ViewerState::Revoked { at: self.now() };
            self.registry.put(&viewer).await?;
            log::warn!("Revoked the viewer {} ({})", viewer.id, viewer.label);
        }
        Ok(viewer)
    }

    /// Ends the access of whoever holds `token`, as the holder asks (RFC 7009). A token that is not good, or not there, is
    /// not an error: the answer must not say which tokens exist.
    pub async fn revoke_token(&self, token: &str) -> Result<(), ChangeError> {
        let Ok(token) = ViewerToken::parse(token) else {
            return Ok(());
        };
        let found = self.registry.find_by_token(&token.hash()).await?;
        match found {
            Some(viewer) => self.revoke(&viewer.id).await.map(|_| ()),
            None => Ok(()),
        }
    }

    /// Forgets a viewer altogether, which also frees its place. Its token, if it had one, stops working.
    pub async fn forget(&self, id: &ViewerId) -> Result<Viewer, ChangeError> {
        let _changing = self.changes.lock().await;
        let viewer = self
            .registry
            .get(id)
            .await?
            .ok_or_else(|| ChangeError::Unknown(id.clone()))?;
        self.registry.remove(id).await?;
        log::info!("Forgot the viewer {} ({})", viewer.id, viewer.label);
        Ok(viewer)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<Viewer>> {
        self.registry.all().await
    }

    pub async fn get(&self, id: &ViewerId) -> Result<Viewer, ChangeError> {
        self.registry
            .get(id)
            .await?
            .ok_or_else(|| ChangeError::Unknown(id.clone()))
    }

    /// The time the server goes by.
    pub fn time(&self) -> DateTime<Utc> {
        self.now()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};

    use chrono_tz::Europe::London;

    use super::*;
    use crate::adapters::clock::FixedClock;
    use crate::adapters::viewer_registry::FileViewerRegistry;
    use crate::domain::models::layout::{Layout, LayoutName};
    use crate::domain::models::viewer::ViewerStanding;

    /// Randomness that counts, so each secret differs and a test can say what it was.
    struct Counting(AtomicU8);
    impl Entropy for Counting {
        fn fill(&self, bytes: &mut [u8]) {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            bytes.fill(n);
        }
    }

    struct Rig {
        viewers: Viewers,
        layouts: Layouts,
        clock: Arc<FixedClock>,
        _directory: tempfile::TempDir,
    }

    fn at(minutes: i64) -> chrono::DateTime<chrono_tz::Tz> {
        use chrono::TimeZone;
        London.with_ymd_and_hms(2026, 10, 12, 9, 0, 0).unwrap() + Duration::minutes(minutes)
    }

    async fn rig() -> Rig {
        let directory = tempfile::tempdir().unwrap();
        let clock = FixedClock::at(at(0));
        let viewers = Viewers::new(
            Arc::new(FileViewerRegistry::open(directory.path()).await.unwrap()),
            clock.clone(),
            Arc::new(Counting(AtomicU8::new(1))),
        );
        Rig {
            layouts: Layouts::new(vec![
                Layout::new("dashboard", "Everything"),
                Layout::new("departures", "Just the trains"),
            ])
            .unwrap(),
            viewers,
            clock,
            _directory: directory,
        }
    }

    fn request(label: &str) -> IssueRequest {
        IssueRequest {
            label: label.to_owned(),
            ..IssueRequest::default()
        }
    }

    async fn signed_in(rig: &Rig, label: &str) -> (ViewerId, ViewerToken) {
        let issued = rig
            .viewers
            .issue(request(label), &rig.layouts)
            .await
            .unwrap();
        let grant = rig.viewers.exchange(issued.code.expose()).await.unwrap();
        (grant.viewer, grant.token)
    }

    #[tokio::test]
    async fn a_viewer_is_issued_waiting_with_a_code_and_the_defaults() {
        let rig = rig().await;

        let issued = rig
            .viewers
            .issue(request("kitchen tablet"), &rig.layouts)
            .await
            .unwrap();

        let viewer = &issued.viewer;
        assert_eq!(viewer.label.as_str(), "kitchen tablet");
        assert_eq!(viewer.scope, LayoutScope::All);
        assert_eq!(viewer.lifetime, Duration::days(90));
        assert_eq!(viewer.standing(rig.viewers.time()), ViewerStanding::Waiting);
        assert!(issued.code.expose().starts_with("hdc1_"));
        // What is kept is a hash: the code is not in the registry, and a viewer is listed.
        let listed = rig.viewers.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!format!("{listed:?}").contains(&issued.code.expose()[5..]));
    }

    #[tokio::test]
    async fn what_is_asked_for_is_checked() {
        let rig = rig().await;
        let with = |change: &dyn Fn(&mut IssueRequest)| {
            let mut r = request("tablet");
            change(&mut r);
            r
        };

        assert!(matches!(
            rig.viewers.issue(request(""), &rig.layouts).await,
            Err(IssueError::Invalid(_))
        ));
        assert!(matches!(
            rig.viewers.issue(with(&|r| r.layouts = vec!["nonesuch".into()]), &rig.layouts).await,
            Err(IssueError::UnknownLayout(name)) if name == "nonesuch"
        ));
        assert!(matches!(
            rig.viewers
                .issue(with(&|r| r.lifetime_days = Some(0)), &rig.layouts)
                .await,
            Err(IssueError::InvalidLifetime)
        ));
        assert!(matches!(
            rig.viewers
                .issue(
                    with(&|r| r.lifetime_days = Some(MOST_LIFETIME_DAYS + 1)),
                    &rig.layouts
                )
                .await,
            Err(IssueError::InvalidLifetime)
        ));
        assert!(matches!(
            rig.viewers
                .issue(with(&|r| r.code_minutes = Some(0)), &rig.layouts)
                .await,
            Err(IssueError::InvalidCodeTime)
        ));
        assert!(matches!(
            rig.viewers
                .issue(
                    with(&|r| r.code_minutes = Some(MOST_CODE_MINUTES + 1)),
                    &rig.layouts
                )
                .await,
            Err(IssueError::InvalidCodeTime)
        ));
        // None of that made a viewer.
        assert!(rig.viewers.list().await.unwrap().is_empty());
        // What is allowed is.
        let ok = rig
            .viewers
            .issue(
                IssueRequest {
                    label: "ok".into(),
                    layouts: vec!["departures".into()],
                    lifetime_days: Some(30),
                    code_minutes: Some(5),
                },
                &rig.layouts,
            )
            .await
            .unwrap();
        assert_eq!(ok.viewer.scope.to_scope_string(), "layout:departures");
        assert_eq!(ok.viewer.lifetime, Duration::days(30));
    }

    #[tokio::test]
    async fn the_code_is_exchanged_for_a_token_once_and_only_once() {
        let rig = rig().await;
        let issued = rig
            .viewers
            .issue(request("tablet"), &rig.layouts)
            .await
            .unwrap();

        let grant = rig.viewers.exchange(issued.code.expose()).await.unwrap();

        assert!(grant.token.expose().starts_with("hdv1_"));
        assert_eq!(grant.expires_in, Duration::days(90));
        assert_eq!(grant.scope.to_scope_string(), "layout:*");
        // The code is spent: the same one again is no good, and the token is not shown anywhere it can be read back.
        assert!(matches!(
            rig.viewers.exchange(issued.code.expose()).await,
            Err(ExchangeError::InvalidGrant)
        ));
        let viewer = rig.viewers.get(&grant.viewer).await.unwrap();
        assert_eq!(viewer.standing(rig.viewers.time()), ViewerStanding::Active);
        assert!(!format!("{viewer:?}").contains(&grant.token.expose()[5..]));
    }

    #[tokio::test]
    async fn two_exchanges_of_one_code_at_once_give_one_token() {
        let rig = Arc::new(rig().await);
        let issued = rig
            .viewers
            .issue(request("tablet"), &rig.layouts)
            .await
            .unwrap();
        let code = issued.code.expose().to_owned();

        let attempts: Vec<_> = (0..8)
            .map(|_| {
                let (rig, code) = (rig.clone(), code.clone());
                tokio::spawn(async move { rig.viewers.exchange(&code).await.is_ok() })
            })
            .collect();
        let mut wins = 0;
        for attempt in attempts {
            wins += usize::from(attempt.await.unwrap());
        }

        assert_eq!(wins, 1);
    }

    #[tokio::test]
    async fn a_code_that_is_late_or_made_up_or_the_wrong_kind_is_not_good_in_the_same_way() {
        let rig = rig().await;
        let issued = rig
            .viewers
            .issue(request("tablet"), &rig.layouts)
            .await
            .unwrap();
        let token = ViewerToken::generate([9; 32]);

        rig.clock.set(at(15));
        for bad in [
            issued.code.expose(),
            ExchangeCode::generate([200; 32]).expose(),
            token.expose(),
            "hdc1_short",
            "",
            "not a code",
        ] {
            assert!(
                matches!(
                    rig.viewers.exchange(bad).await,
                    Err(ExchangeError::InvalidGrant)
                ),
                "{bad:?}"
            );
        }
        // It was late, not used: the viewer says so.
        let viewer = rig.viewers.list().await.unwrap().remove(0);
        assert_eq!(
            viewer.standing(rig.viewers.time()),
            ViewerStanding::LinkExpired
        );
    }

    #[tokio::test]
    async fn a_token_authenticates_until_it_runs_out_and_says_what_the_viewer_may_see() {
        let rig = rig().await;
        let issued = rig
            .viewers
            .issue(
                IssueRequest {
                    label: "tablet".into(),
                    layouts: vec!["departures".into()],
                    ..IssueRequest::default()
                },
                &rig.layouts,
            )
            .await
            .unwrap();
        let token = rig
            .viewers
            .exchange(issued.code.expose())
            .await
            .unwrap()
            .token;

        let who = rig.viewers.authenticate(token.expose()).await.unwrap();
        assert_eq!(who.label.as_str(), "tablet");
        assert!(who.scope.permits(&LayoutName::parse("departures").unwrap()));
        assert!(!who.scope.permits(&LayoutName::parse("dashboard").unwrap()));

        // Still good the day before, gone on the day.
        rig.clock
            .set(at(0) + Duration::days(90) - Duration::minutes(1));
        assert!(rig.viewers.authenticate(token.expose()).await.is_ok());
        rig.clock.set(at(0) + Duration::days(90));
        assert!(matches!(
            rig.viewers.authenticate(token.expose()).await,
            Err(AuthError::InvalidToken)
        ));
    }

    #[tokio::test]
    async fn what_is_not_a_good_token_is_not_told_apart() {
        let rig = rig().await;
        let (id, token) = signed_in(&rig, "tablet").await;
        let code = rig
            .viewers
            .issue(request("other"), &rig.layouts)
            .await
            .unwrap()
            .code;

        rig.viewers.revoke(&id).await.unwrap();

        for bad in [
            token.expose().to_owned(),
            ViewerToken::generate([77; 32]).expose().to_owned(),
            code.expose().to_owned(),
            "hdv1_nope".to_owned(),
            String::new(),
        ] {
            assert!(
                matches!(
                    rig.viewers.authenticate(&bad).await,
                    Err(AuthError::InvalidToken)
                ),
                "{bad:?}"
            );
        }
    }

    #[tokio::test]
    async fn revoking_ends_access_at_once_and_is_idempotent_and_leaves_the_others_alone() {
        let rig = rig().await;
        let (kitchen, kitchen_token) = signed_in(&rig, "kitchen").await;
        let (_, hall_token) = signed_in(&rig, "hall").await;

        let revoked = rig.viewers.revoke(&kitchen).await.unwrap();
        rig.viewers.revoke(&kitchen).await.unwrap();

        assert_eq!(
            revoked.standing(rig.viewers.time()),
            ViewerStanding::Revoked
        );
        assert!(
            rig.viewers
                .authenticate(kitchen_token.expose())
                .await
                .is_err()
        );
        assert!(rig.viewers.authenticate(hall_token.expose()).await.is_ok());
        assert!(matches!(
            rig.viewers.revoke(&ViewerId::from_random(&[99; 5])).await,
            Err(ChangeError::Unknown(_))
        ));
    }

    #[tokio::test]
    async fn a_viewer_waiting_can_be_revoked_and_its_code_stops_working() {
        let rig = rig().await;
        let issued = rig
            .viewers
            .issue(request("tablet"), &rig.layouts)
            .await
            .unwrap();

        rig.viewers.revoke(&issued.viewer.id).await.unwrap();

        assert!(matches!(
            rig.viewers.exchange(issued.code.expose()).await,
            Err(ExchangeError::InvalidGrant)
        ));
    }

    #[tokio::test]
    async fn the_holder_can_end_its_own_access_and_asking_about_anything_else_changes_nothing() {
        let rig = rig().await;
        let (_, token) = signed_in(&rig, "tablet").await;
        let (_, other) = signed_in(&rig, "other").await;

        rig.viewers.revoke_token(token.expose()).await.unwrap();
        // Unknown, malformed and already-revoked tokens are all the same quiet success.
        rig.viewers.revoke_token(token.expose()).await.unwrap();
        rig.viewers
            .revoke_token(ViewerToken::generate([55; 32]).expose())
            .await
            .unwrap();
        rig.viewers.revoke_token("garbage").await.unwrap();

        assert!(rig.viewers.authenticate(token.expose()).await.is_err());
        assert!(rig.viewers.authenticate(other.expose()).await.is_ok());
    }

    #[tokio::test]
    async fn forgetting_removes_the_viewer_and_ends_its_token() {
        let rig = rig().await;
        let (id, token) = signed_in(&rig, "tablet").await;

        let forgotten = rig.viewers.forget(&id).await.unwrap();

        assert_eq!(forgotten.label.as_str(), "tablet");
        assert!(rig.viewers.list().await.unwrap().is_empty());
        assert!(rig.viewers.authenticate(token.expose()).await.is_err());
        assert!(matches!(
            rig.viewers.forget(&id).await,
            Err(ChangeError::Unknown(_))
        ));
    }

    #[tokio::test]
    async fn use_is_noted_only_every_so_often_and_never_undoes_a_revoke() {
        let rig = rig().await;
        let (id, token) = signed_in(&rig, "tablet").await;
        let last_used = || async {
            match rig.viewers.get(&id).await.unwrap().state {
                ViewerState::Active { last_used_at, .. } => last_used_at,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(last_used().await, None);

        rig.viewers.authenticate(token.expose()).await.unwrap();
        let first = last_used().await.unwrap();
        // Soon after: not written again.
        rig.clock.set(at(3));
        rig.viewers.authenticate(token.expose()).await.unwrap();
        assert_eq!(last_used().await, Some(first));
        // Long enough after: is.
        rig.clock.set(at(11));
        rig.viewers.authenticate(token.expose()).await.unwrap();
        assert!(last_used().await.unwrap() > first);

        // A revoke, then a late note of use of the old token: still revoked.
        rig.viewers.revoke(&id).await.unwrap();
        rig.viewers.note_use(&id, &token, rig.viewers.time()).await;
        assert_eq!(
            rig.viewers
                .get(&id)
                .await
                .unwrap()
                .standing(rig.viewers.time()),
            ViewerStanding::Revoked
        );
    }

    #[tokio::test]
    async fn there_is_a_most_and_forgetting_frees_a_place() {
        let rig = rig().await;
        let mut ids = Vec::new();
        for i in 0..MOST_VIEWERS {
            ids.push(
                rig.viewers
                    .issue(request(&format!("viewer {i}")), &rig.layouts)
                    .await
                    .unwrap()
                    .viewer
                    .id,
            );
        }

        assert!(matches!(
            rig.viewers.issue(request("one more"), &rig.layouts).await,
            Err(IssueError::TooMany)
        ));
        rig.viewers.forget(&ids[0]).await.unwrap();
        assert!(
            rig.viewers
                .issue(request("one more"), &rig.layouts)
                .await
                .is_ok()
        );
    }
}
