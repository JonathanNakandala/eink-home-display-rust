//! How a display joins the server's private certificate authority, and stays in it.
//!
//! The first request can't be authenticated: the display has nothing yet to trust the server by, and the
//! server has nothing to know the display by. So the owner stands in for both. They open a pairing
//! window, the display asks to join and shows a `PairingCode` on its panel, and the owner types that
//! code in at the server. Only then does the display's next request get its certificate. After that it
//! is known by the certificate, and renewing needs no one.
//!
//! This follows the shape of EST (RFC 7030 and its updates): a request that waits for a person is
//! answered "not yet, ask again in so long", and the display, which keeps no state, simply asks again.
//!
//! Everything a request carries comes from the network, so the unauthenticated path is narrow: it
//! creates something only while the owner has the window open, and only up to a few at a time.

use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Duration, Utc};
use subtle::ConstantTimeEq;
use thiserror::Error;

use crate::domain::models::device_id::DeviceId;
use crate::domain::models::pairing::{Pairing, PairingCode, PairingState};
use crate::domain::services::certificate_authority::{
    CertificateAuthority, CertificateRequest, IssuedCertificate, RequestError,
};
use crate::domain::services::clock::Clock;
use crate::domain::services::pairing_store::PairingStore;

/// More than a household has waiting at once. Anyone on the network can ask while the window is open,
/// so what a stranger can fill is bounded; the owner clears the rest by rejecting them.
pub const MAX_PENDING: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct EnrollmentPolicy {
    /// How long a display's certificate lasts. A display that is off for longer has to be paired again.
    pub certificate_lifetime: Duration,
    /// What a waiting display is told to wait. It is the display's battery that pays for asking, so
    /// not seconds.
    pub retry_after: Duration,
    /// Whether a request must prove it was signed on the connection it arrived on. Off until the
    /// display can do it; a request that does carry the proof is checked either way.
    pub require_channel_binding: bool,
}

/// What a request that was understood came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Issued(IssuedCertificate),
    /// Not yet: the owner has to approve it, then ask again after `retry_after`.
    Pending {
        retry_after: Duration,
        code: PairingCode,
    },
}

/// Why a request was turned down. The display's own mistakes and the owner's decisions, which it
/// must not retry; a server fault is `EnrollError::Failed` instead.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Refusal {
    #[error("{0}")]
    BadRequest(#[from] RequestError),
    #[error("the request must be tied to its connection and is not")]
    ChannelBindingMissing,
    #[error("the request was signed on a different connection")]
    ChannelBindingMismatch,
    #[error("the server is not accepting new displays")]
    NotAccepting,
    #[error("too many displays are waiting to be approved")]
    TooManyPending,
    #[error("this display was turned down")]
    Rejected,
    #[error("this display is no longer a member")]
    Revoked,
    #[error("this display is already known by a different key")]
    KeyMismatch,
    #[error("this display has not joined")]
    NotEnrolled,
    #[error("the request is for a different display than the one asking")]
    WrongDevice,
}

#[derive(Debug, Error)]
pub enum EnrollError {
    #[error(transparent)]
    Refused(#[from] Refusal),
    #[error("the server failed: {0:#}")]
    Failed(#[from] anyhow::Error),
}

#[derive(Debug, Error)]
pub enum ApproveError {
    #[error("no display called {0} has asked to join")]
    Unknown(DeviceId),
    #[error("{device} is {state}, not waiting for approval")]
    NotPending {
        device: DeviceId,
        state: &'static str,
    },
    /// What was typed is not what the server worked out for that display. Either a typing mistake or,
    /// if the display really shows what was typed, someone between the two.
    #[error("that is not the code {0} would show")]
    WrongCode(DeviceId),
    #[error("the server failed: {0:#}")]
    Failed(#[from] anyhow::Error),
}

pub struct Enrollment {
    authority: Arc<dyn CertificateAuthority>,
    store: Arc<dyn PairingStore>,
    clock: Arc<dyn Clock>,
    policy: EnrollmentPolicy,
    /// When the window the owner opened closes, if one is open.
    window: Mutex<Option<DateTime<Utc>>>,
}

impl Enrollment {
    pub fn new(
        authority: Arc<dyn CertificateAuthority>,
        store: Arc<dyn PairingStore>,
        clock: Arc<dyn Clock>,
        policy: EnrollmentPolicy,
    ) -> Self {
        Self {
            authority,
            store,
            clock,
            policy,
            window: Mutex::new(None),
        }
    }

    fn now(&self) -> DateTime<Utc> {
        self.clock.now().to_utc()
    }

    /// Lets new displays ask to join until `now + duration`.
    pub fn open_window(&self, duration: Duration) -> DateTime<Utc> {
        let closes = self.now() + duration;
        *self.window.lock().unwrap_or_else(PoisonError::into_inner) = Some(closes);
        closes
    }

    pub fn close_window(&self) {
        *self.window.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// When the open window closes, or nothing if there is none.
    pub fn window_closes_at(&self) -> Option<DateTime<Utc>> {
        let closes = (*self.window.lock().unwrap_or_else(PoisonError::into_inner))?;
        (closes > self.now()).then_some(closes)
    }

    /// A request to join, or to collect a certificate that was approved. Not authenticated, which is why
    /// the owner's approval comes before anything is issued.
    ///
    /// `connection` is what the server worked out from the connection the request arrived on, to
    /// compare with what the request says about it.
    pub async fn enroll(
        &self,
        request: &[u8],
        connection: Option<&[u8]>,
    ) -> Result<Outcome, EnrollError> {
        let request = self.read(request, connection)?;
        let now = self.now();
        let Some(pairing) = self.store.get(&request.device).await? else {
            return self.begin(request, now).await;
        };
        match pairing.state {
            PairingState::Rejected => Err(Refusal::Rejected.into()),
            PairingState::Revoked => Err(Refusal::Revoked.into()),
            PairingState::Pending if pairing.key == request.key => Ok(Outcome::Pending {
                retry_after: self.policy.retry_after,
                code: pairing.code,
            }),
            // The display lost its key (it was wiped and flashed again). Anything can say that, so
            // while it is only waiting the new request takes the old one's place, and the code the
            // owner is typing from the panel stops matching if it was not the display's own.
            PairingState::Pending if self.window_closes_at().is_some() => {
                self.begin(request, now).await
            }
            PairingState::Approved | PairingState::Enrolled { .. }
                if pairing.key == request.key =>
            {
                // The certificate is public, only the key makes it usable, so handing it out again to
                // whoever holds the key (a display that lost it) is no risk.
                self.issue(pairing, request, now).await
            }
            _ => Err(Refusal::KeyMismatch.into()),
        }
    }

    /// A member renewing its certificate, perhaps with a new key. `caller` is who the connection's
    /// client certificate says it is.
    pub async fn renew(
        &self,
        caller: &DeviceId,
        request: &[u8],
        connection: Option<&[u8]>,
    ) -> Result<IssuedCertificate, EnrollError> {
        let request = self.read(request, connection)?;
        if request.device != *caller {
            return Err(Refusal::WrongDevice.into());
        }
        let pairing = self.store.get(caller).await?;
        let pairing = match pairing {
            Some(p) if matches!(p.state, PairingState::Enrolled { .. }) => p,
            Some(p) if p.state == PairingState::Revoked => return Err(Refusal::Revoked.into()),
            _ => return Err(Refusal::NotEnrolled.into()),
        };
        match self.issue(pairing, request, self.now()).await? {
            Outcome::Issued(certificate) => Ok(certificate),
            Outcome::Pending { .. } => unreachable!("issuing never leaves a request pending"),
        }
    }

    /// Whether `device` is a member now, as its certificate alone can't say: a revoked display's
    /// certificate is still valid until it expires.
    pub async fn is_member(&self, device: &DeviceId) -> anyhow::Result<bool> {
        let pairing = self.store.get(device).await?;
        Ok(pairing.is_some_and(|p| {
            matches!(p.state, PairingState::Enrolled { not_after, .. } if not_after > self.now())
        }))
    }

    /// The owner confirms a display by typing in the code its panel shows.
    pub async fn approve(
        &self,
        device: &DeviceId,
        typed: &PairingCode,
    ) -> Result<(), ApproveError> {
        let mut pairing = self
            .store
            .get(device)
            .await?
            .ok_or_else(|| ApproveError::Unknown(device.clone()))?;
        if pairing.state != PairingState::Pending {
            return Err(ApproveError::NotPending {
                device: device.clone(),
                state: pairing.state.name(),
            });
        }
        let same: bool = typed
            .as_str()
            .as_bytes()
            .ct_eq(pairing.code.as_str().as_bytes())
            .into();
        if !same {
            log::warn!(
                "A pairing code typed for {device} did not match what the server worked out for it"
            );
            return Err(ApproveError::WrongCode(device.clone()));
        }
        pairing.state = PairingState::Approved;
        pairing.updated_at = self.now();
        self.store.put(&pairing).await?;
        log::info!("Approved {device}; it gets its certificate the next time it asks");
        Ok(())
    }

    /// Turns a waiting display down, until the owner removes it.
    pub async fn reject(&self, device: &DeviceId) -> Result<(), ApproveError> {
        self.set_state(device, PairingState::Rejected, |s| {
            matches!(s, PairingState::Pending | PairingState::Approved)
        })
        .await?;
        log::info!("Turned down {device}");
        Ok(())
    }

    /// Ends a member's membership. Its certificate stays valid until it expires, but nothing that
    /// checks `is_member` accepts it, and it can't renew.
    pub async fn revoke(&self, device: &DeviceId) -> Result<(), ApproveError> {
        self.set_state(device, PairingState::Revoked, |s| {
            matches!(s, PairingState::Enrolled { .. } | PairingState::Approved)
        })
        .await?;
        log::warn!("Revoked {device}");
        Ok(())
    }

    /// Forgets a display, whatever its state, so it can ask again as if for the first time.
    pub async fn forget(&self, device: &DeviceId) -> anyhow::Result<()> {
        self.store.remove(device).await?;
        log::info!("Forgot {device}");
        Ok(())
    }

    pub async fn pairings(&self) -> anyhow::Result<Vec<Pairing>> {
        self.store.all().await
    }

    async fn set_state(
        &self,
        device: &DeviceId,
        state: PairingState,
        allowed: impl Fn(&PairingState) -> bool,
    ) -> Result<(), ApproveError> {
        let mut pairing = self
            .store
            .get(device)
            .await?
            .ok_or_else(|| ApproveError::Unknown(device.clone()))?;
        if !allowed(&pairing.state) {
            return Err(ApproveError::NotPending {
                device: device.clone(),
                state: pairing.state.name(),
            });
        }
        pairing.state = state;
        pairing.updated_at = self.now();
        self.store.put(&pairing).await?;
        Ok(())
    }

    /// Reads a request and checks it was signed on the connection it came over, if it says so.
    fn read(
        &self,
        request: &[u8],
        connection: Option<&[u8]>,
    ) -> Result<CertificateRequest, Refusal> {
        let request = self.authority.inspect(request)?;
        match (&request.channel_binding, connection) {
            (None, _) if self.policy.require_channel_binding => Err(Refusal::ChannelBindingMissing),
            (Some(said), Some(actual)) if bool::from(said.as_slice().ct_eq(actual)) => Ok(request),
            (Some(_), _) => Err(Refusal::ChannelBindingMismatch),
            (None, _) => Ok(request),
        }
    }

    /// Records a first request from a display, if the owner is letting displays ask.
    async fn begin(
        &self,
        request: CertificateRequest,
        now: DateTime<Utc>,
    ) -> Result<Outcome, EnrollError> {
        if self.window_closes_at().is_none() {
            return Err(Refusal::NotAccepting.into());
        }
        let waiting = self
            .store
            .all()
            .await?
            .iter()
            .filter(|p| p.state == PairingState::Pending && p.device != request.device)
            .count();
        if waiting >= MAX_PENDING {
            return Err(Refusal::TooManyPending.into());
        }
        let code =
            PairingCode::derive(&self.authority.fingerprint(), &request.device, &request.key);
        let pairing = Pairing {
            device: request.device,
            key: request.key,
            code: code.clone(),
            state: PairingState::Pending,
            requested_at: now,
            updated_at: now,
        };
        self.store.put(&pairing).await?;
        log::info!(
            "{} asked to join; the owner approves it with the code {code}",
            pairing.device
        );
        Ok(Outcome::Pending {
            retry_after: self.policy.retry_after,
            code,
        })
    }

    async fn issue(
        &self,
        mut pairing: Pairing,
        request: CertificateRequest,
        now: DateTime<Utc>,
    ) -> Result<Outcome, EnrollError> {
        let certificate =
            self.authority
                .issue(&request, now, now + self.policy.certificate_lifetime)?;
        pairing.code =
            PairingCode::derive(&self.authority.fingerprint(), &pairing.device, &request.key);
        pairing.key = request.key;
        pairing.state = PairingState::Enrolled {
            serial: certificate.serial.clone(),
            not_after: certificate.not_after,
        };
        pairing.updated_at = now;
        self.store.put(&pairing).await?;
        log::info!(
            "Issued {} a certificate (serial {}, until {})",
            pairing.device,
            certificate.serial,
            certificate.not_after.format("%Y-%m-%d")
        );
        Ok(Outcome::Issued(certificate))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::TimeZone;
    use chrono_tz::Tz;

    use super::*;
    use crate::domain::models::pairing::{Fingerprint, PublicKey};

    /// A stand-in authority. A "request" is `device|key|binding` as text, and a certificate is just
    /// a record of who it was for, so the rules can be tested without any cryptography.
    struct FakeAuthority {
        issued: Mutex<u64>,
    }

    impl FakeAuthority {
        fn new() -> Self {
            Self {
                issued: Mutex::new(0),
            }
        }
    }

    impl CertificateAuthority for FakeAuthority {
        fn certificate(&self) -> &[u8] {
            b"authority"
        }

        fn fingerprint(&self) -> Fingerprint {
            Fingerprint::of(b"authority")
        }

        fn inspect(&self, der: &[u8]) -> Result<CertificateRequest, RequestError> {
            let text = std::str::from_utf8(der)
                .map_err(|_| RequestError::Malformed("not text".to_owned()))?;
            let mut parts = text.splitn(3, '|');
            let device = DeviceId::parse(parts.next().unwrap_or(""))
                .map_err(|e| RequestError::BadDevice(e.to_string()))?;
            let key = parts
                .next()
                .ok_or_else(|| RequestError::Malformed("no key".to_owned()))?;
            if key == "forged" {
                return Err(RequestError::BadSignature);
            }
            Ok(CertificateRequest {
                device,
                key: PublicKey::from_der(key.as_bytes().to_vec()),
                channel_binding: parts
                    .next()
                    .filter(|b| !b.is_empty())
                    .map(|b| b.as_bytes().to_vec()),
                der: der.to_vec(),
            })
        }

        fn issue(
            &self,
            request: &CertificateRequest,
            _now: DateTime<Utc>,
            not_after: DateTime<Utc>,
        ) -> anyhow::Result<IssuedCertificate> {
            let mut count = self.issued.lock().unwrap();
            *count += 1;
            Ok(IssuedCertificate {
                der: format!("certificate for {}", request.device).into_bytes(),
                serial: format!("{count}"),
                not_after,
            })
        }
    }

    #[derive(Default)]
    struct MemoryStore(Mutex<BTreeMap<DeviceId, Pairing>>);

    #[async_trait::async_trait]
    impl PairingStore for MemoryStore {
        async fn get(&self, device: &DeviceId) -> anyhow::Result<Option<Pairing>> {
            Ok(self.0.lock().unwrap().get(device).cloned())
        }
        async fn put(&self, pairing: &Pairing) -> anyhow::Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(pairing.device.clone(), pairing.clone());
            Ok(())
        }
        async fn remove(&self, device: &DeviceId) -> anyhow::Result<()> {
            self.0.lock().unwrap().remove(device);
            Ok(())
        }
        async fn all(&self) -> anyhow::Result<Vec<Pairing>> {
            Ok(self.0.lock().unwrap().values().cloned().collect())
        }
    }

    struct TestClock(Mutex<DateTime<Utc>>);

    impl Clock for TestClock {
        fn now(&self) -> DateTime<Tz> {
            self.0.lock().unwrap().with_timezone(&Tz::UTC)
        }
    }

    struct Fixture {
        enrollment: Enrollment,
        clock: Arc<TestClock>,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with(false)
        }

        fn with(require_channel_binding: bool) -> Self {
            let clock = Arc::new(TestClock(Mutex::new(
                Utc.with_ymd_and_hms(2026, 10, 8, 12, 0, 0).unwrap(),
            )));
            let enrollment = Enrollment::new(
                Arc::new(FakeAuthority::new()),
                Arc::new(MemoryStore::default()),
                clock.clone(),
                EnrollmentPolicy {
                    certificate_lifetime: Duration::days(365),
                    retry_after: Duration::minutes(5),
                    require_channel_binding,
                },
            );
            Self { enrollment, clock }
        }

        fn advance(&self, by: Duration) {
            *self.clock.0.lock().unwrap() += by;
        }

        /// The code the display would show for this name and key.
        fn code(&self, name: &str, key: &str) -> PairingCode {
            PairingCode::derive(
                &Fingerprint::of(b"authority"),
                &device(name),
                &PublicKey::from_der(key.as_bytes().to_vec()),
            )
        }
    }

    fn device(name: &str) -> DeviceId {
        DeviceId::parse(name).unwrap()
    }

    fn csr(name: &str, key: &str) -> Vec<u8> {
        format!("{name}|{key}|").into_bytes()
    }

    fn refused(result: Result<Outcome, EnrollError>) -> Refusal {
        match result {
            Err(EnrollError::Refused(refusal)) => refusal,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_display_that_asks_with_no_window_open_is_turned_away() {
        let f = Fixture::new();
        let result = f.enrollment.enroll(&csr("kitchen", "k1"), None).await;
        assert_eq!(refused(result), Refusal::NotAccepting);
        assert!(f.enrollment.pairings().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn with_the_window_open_it_waits_with_the_code_its_panel_shows() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            Outcome::Pending {
                retry_after: Duration::minutes(5),
                code: f.code("kitchen", "k1"),
            }
        );
    }

    #[tokio::test]
    async fn asking_again_while_waiting_gives_the_same_answer_and_adds_nothing() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        let first = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        let again = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        assert_eq!(first, again);
        assert_eq!(f.enrollment.pairings().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_window_closes_when_its_time_is_up() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        assert!(f.enrollment.window_closes_at().is_some());
        f.advance(Duration::minutes(11));
        assert!(f.enrollment.window_closes_at().is_none());
        let result = f.enrollment.enroll(&csr("kitchen", "k1"), None).await;
        assert_eq!(refused(result), Refusal::NotAccepting);
    }

    #[tokio::test]
    async fn closing_the_window_by_hand_stops_new_displays() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment.close_window();
        let result = f.enrollment.enroll(&csr("kitchen", "k1"), None).await;
        assert_eq!(refused(result), Refusal::NotAccepting);
    }

    #[tokio::test]
    async fn no_certificate_is_issued_until_the_owner_approves() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        for _ in 0..3 {
            let outcome = f
                .enrollment
                .enroll(&csr("kitchen", "k1"), None)
                .await
                .unwrap();
            assert!(matches!(outcome, Outcome::Pending { .. }));
        }
    }

    #[tokio::test]
    async fn after_approval_the_next_request_gets_a_certificate() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        f.enrollment
            .approve(&device("kitchen"), &f.code("kitchen", "k1"))
            .await
            .unwrap();
        let Outcome::Issued(certificate) = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap()
        else {
            panic!("expected a certificate");
        };
        assert_eq!(certificate.der, b"certificate for kitchen");
        assert_eq!(
            certificate.not_after,
            f.clock.now().to_utc() + Duration::days(365)
        );
        assert!(f.enrollment.is_member(&device("kitchen")).await.unwrap());
    }

    #[tokio::test]
    async fn approval_works_after_the_window_has_closed() {
        // The window is for new displays to ask; the owner may take their time over the code.
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        f.advance(Duration::hours(2));
        f.enrollment
            .approve(&device("kitchen"), &f.code("kitchen", "k1"))
            .await
            .unwrap();
        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        assert!(matches!(outcome, Outcome::Issued(_)));
    }

    #[tokio::test]
    async fn a_wrong_code_does_not_approve() {
        // A code that does not match is what someone between the display and the server causes.
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        let wrong = PairingCode::parse("0000-0000-0000").unwrap();
        let result = f.enrollment.approve(&device("kitchen"), &wrong).await;
        assert!(
            matches!(result, Err(ApproveError::WrongCode(_))),
            "{result:?}"
        );
        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        assert!(matches!(outcome, Outcome::Pending { .. }), "still waiting");
    }

    #[tokio::test]
    async fn the_code_of_another_display_does_not_approve_this_one() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        f.enrollment.enroll(&csr("hall", "k2"), None).await.unwrap();
        let result = f
            .enrollment
            .approve(&device("kitchen"), &f.code("hall", "k2"))
            .await;
        assert!(
            matches!(result, Err(ApproveError::WrongCode(_))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn approving_a_display_that_never_asked_is_an_error() {
        let f = Fixture::new();
        let result = f
            .enrollment
            .approve(&device("kitchen"), &f.code("kitchen", "k1"))
            .await;
        assert!(
            matches!(result, Err(ApproveError::Unknown(_))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn approving_twice_is_an_error_not_a_second_approval() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        let code = f.code("kitchen", "k1");
        f.enrollment
            .approve(&device("kitchen"), &code)
            .await
            .unwrap();
        let again = f.enrollment.approve(&device("kitchen"), &code).await;
        assert!(
            matches!(
                again,
                Err(ApproveError::NotPending {
                    state: "approved",
                    ..
                })
            ),
            "{again:?}"
        );
    }

    #[tokio::test]
    async fn a_member_asking_again_with_its_key_gets_its_certificate_again() {
        // A display that was reset but kept its key, or lost the response.
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        f.enrollment
            .approve(&device("kitchen"), &f.code("kitchen", "k1"))
            .await
            .unwrap();
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        f.enrollment.close_window();
        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        assert!(matches!(outcome, Outcome::Issued(_)));
    }

    #[tokio::test]
    async fn a_name_a_member_holds_can_not_be_taken_by_another_key() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        f.enrollment
            .approve(&device("kitchen"), &f.code("kitchen", "k1"))
            .await
            .unwrap();
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        // Even with the window open: the owner has to forget the old one first.
        let result = f.enrollment.enroll(&csr("kitchen", "stolen"), None).await;
        assert_eq!(refused(result), Refusal::KeyMismatch);
    }

    #[tokio::test]
    async fn while_waiting_a_new_key_replaces_the_old_and_the_code_changes() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k2"), None)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            Outcome::Pending {
                retry_after: Duration::minutes(5),
                code: f.code("kitchen", "k2"),
            }
        );
        // The code of the replaced request no longer approves anything.
        let old = f
            .enrollment
            .approve(&device("kitchen"), &f.code("kitchen", "k1"))
            .await;
        assert!(matches!(old, Err(ApproveError::WrongCode(_))), "{old:?}");
    }

    #[tokio::test]
    async fn once_the_window_is_shut_a_waiting_display_can_not_be_replaced() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        f.enrollment.close_window();
        let result = f.enrollment.enroll(&csr("kitchen", "k2"), None).await;
        assert_eq!(refused(result), Refusal::KeyMismatch);
    }

    #[tokio::test]
    async fn a_stranger_can_only_fill_so_many_places() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        for i in 0..MAX_PENDING {
            f.enrollment
                .enroll(&csr(&format!("device-{i}"), "k"), None)
                .await
                .unwrap();
        }
        let result = f.enrollment.enroll(&csr("one-too-many", "k"), None).await;
        assert_eq!(refused(result), Refusal::TooManyPending);
        // One already waiting can still ask again.
        f.enrollment
            .enroll(&csr("device-0", "k"), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_turned_down_display_stays_turned_down() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        f.enrollment.reject(&device("kitchen")).await.unwrap();
        let result = f.enrollment.enroll(&csr("kitchen", "k1"), None).await;
        assert_eq!(refused(result), Refusal::Rejected);
        // Until the owner removes it, after which it is a stranger again.
        f.enrollment.forget(&device("kitchen")).await.unwrap();
        let again = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        assert!(matches!(again, Outcome::Pending { .. }));
    }

    async fn member(f: &Fixture, name: &str, key: &str) {
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment.enroll(&csr(name, key), None).await.unwrap();
        f.enrollment
            .approve(&device(name), &f.code(name, key))
            .await
            .unwrap();
        f.enrollment.enroll(&csr(name, key), None).await.unwrap();
        f.enrollment.close_window();
    }

    #[tokio::test]
    async fn a_member_renews_without_the_owner() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.advance(Duration::days(300));
        let renewed = f
            .enrollment
            .renew(&device("kitchen"), &csr("kitchen", "k1"), None)
            .await
            .unwrap();
        assert_eq!(
            renewed.not_after,
            f.clock.now().to_utc() + Duration::days(365)
        );
        assert!(f.enrollment.is_member(&device("kitchen")).await.unwrap());
    }

    #[tokio::test]
    async fn a_member_can_renew_with_a_new_key_and_then_only_that_key_is_its_own() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment
            .renew(&device("kitchen"), &csr("kitchen", "k2"), None)
            .await
            .unwrap();
        let old = f.enrollment.enroll(&csr("kitchen", "k1"), None).await;
        assert_eq!(refused(old), Refusal::KeyMismatch);
        let new = f
            .enrollment
            .enroll(&csr("kitchen", "k2"), None)
            .await
            .unwrap();
        assert!(matches!(new, Outcome::Issued(_)));
    }

    #[tokio::test]
    async fn a_member_can_not_renew_for_another_display() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        member(&f, "hall", "k2").await;
        let result = f
            .enrollment
            .renew(&device("kitchen"), &csr("hall", "k9"), None)
            .await;
        assert!(matches!(
            result,
            Err(EnrollError::Refused(Refusal::WrongDevice))
        ));
    }

    #[tokio::test]
    async fn someone_who_never_joined_can_not_renew() {
        let f = Fixture::new();
        let result = f
            .enrollment
            .renew(&device("kitchen"), &csr("kitchen", "k1"), None)
            .await;
        assert!(matches!(
            result,
            Err(EnrollError::Refused(Refusal::NotEnrolled))
        ));
    }

    #[tokio::test]
    async fn a_revoked_display_is_no_longer_a_member_and_can_not_renew() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment.revoke(&device("kitchen")).await.unwrap();
        assert!(!f.enrollment.is_member(&device("kitchen")).await.unwrap());
        let result = f
            .enrollment
            .renew(&device("kitchen"), &csr("kitchen", "k1"), None)
            .await;
        assert!(matches!(
            result,
            Err(EnrollError::Refused(Refusal::Revoked))
        ));
        let asking = f.enrollment.enroll(&csr("kitchen", "k1"), None).await;
        assert_eq!(refused(asking), Refusal::Revoked);
    }

    #[tokio::test]
    async fn membership_ends_when_the_certificate_does() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.advance(Duration::days(364));
        assert!(f.enrollment.is_member(&device("kitchen")).await.unwrap());
        f.advance(Duration::days(2));
        assert!(!f.enrollment.is_member(&device("kitchen")).await.unwrap());
    }

    #[tokio::test]
    async fn only_a_member_can_be_revoked() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        let result = f.enrollment.revoke(&device("kitchen")).await;
        assert!(
            matches!(
                result,
                Err(ApproveError::NotPending {
                    state: "pending",
                    ..
                })
            ),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn a_request_with_a_bad_signature_is_refused_before_anything_is_recorded() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        let result = f.enrollment.enroll(&csr("kitchen", "forged"), None).await;
        assert_eq!(
            refused(result),
            Refusal::BadRequest(RequestError::BadSignature)
        );
        assert!(f.enrollment.pairings().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_request_that_names_an_invalid_device_is_refused() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        let result = f.enrollment.enroll(b"bad name!|k1|", None).await;
        assert!(matches!(
            refused(result),
            Refusal::BadRequest(RequestError::BadDevice(_))
        ));
    }

    #[tokio::test]
    async fn a_request_tied_to_its_connection_is_accepted_when_the_connection_matches() {
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        let outcome = f
            .enrollment
            .enroll(b"kitchen|k1|abc", Some(b"abc"))
            .await
            .unwrap();
        assert!(matches!(outcome, Outcome::Pending { .. }));
    }

    #[tokio::test]
    async fn a_request_signed_on_another_connection_is_refused() {
        // A request replayed over a connection of the attacker's own.
        let f = Fixture::new();
        f.enrollment.open_window(Duration::minutes(10));
        for connection in [Some(&b"xyz"[..]), None] {
            let result = f.enrollment.enroll(b"kitchen|k1|abc", connection).await;
            assert_eq!(refused(result), Refusal::ChannelBindingMismatch);
        }
    }

    #[tokio::test]
    async fn without_the_policy_a_request_need_not_be_tied_to_its_connection() {
        let f = Fixture::with(false);
        f.enrollment.open_window(Duration::minutes(10));
        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), Some(b"abc"))
            .await
            .unwrap();
        assert!(matches!(outcome, Outcome::Pending { .. }));
    }

    #[tokio::test]
    async fn with_the_policy_a_request_must_be_tied_to_its_connection() {
        let f = Fixture::with(true);
        f.enrollment.open_window(Duration::minutes(10));
        let result = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), Some(b"abc"))
            .await;
        assert_eq!(refused(result), Refusal::ChannelBindingMissing);
        let tied = f
            .enrollment
            .enroll(b"kitchen|k1|abc", Some(b"abc"))
            .await
            .unwrap();
        assert!(matches!(tied, Outcome::Pending { .. }));
    }
}
