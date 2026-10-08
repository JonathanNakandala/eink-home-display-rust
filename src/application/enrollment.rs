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
use crate::domain::models::pairing::{
    Pairing, PairingCode, PairingState, PublicKey, Replacement, Rollover,
};
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
    /// The certificate shown names this display but is not for the key now enrolled under that name,
    /// as when the display was forgotten and another joined with the same name.
    #[error("the certificate shown is not this display's current one")]
    CertificateSuperseded,
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
    /// Held while a pairing is read, decided on and written back, so two changes to the same display
    /// can't interleave: a renewal that has read a display as a member must not write it back as one
    /// after the owner has revoked it.
    changes: tokio::sync::Mutex<()>,
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
            changes: tokio::sync::Mutex::new(()),
        }
    }

    /// Whether a request has to be tied to its connection, for the server to tell displays so.
    pub fn requires_channel_binding(&self) -> bool {
        self.policy.require_channel_binding
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
    /// A member's own key just gets its certificate again (so a display whose certificate has expired, or
    /// that lost it, needs no one). A different key does not replace it: it waits, as a replacement,
    /// for the owner to approve it by its code, while the member carries on with its own.
    ///
    /// `connection` is what the server worked out from the connection the request arrived on, to
    /// compare with what the request says about it.
    pub async fn enroll(
        &self,
        request: &[u8],
        connection: Option<&[u8]>,
    ) -> Result<Outcome, EnrollError> {
        let request = self.read(request, connection)?;
        let _changing = self.changes.lock().await;
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
            PairingState::Approved if pairing.key == request.key => {
                self.collect(pairing, request, now).await
            }
            PairingState::Enrolled { .. } => self.member_asks(pairing, request, now).await,
            _ => Err(Refusal::KeyMismatch.into()),
        }
    }

    /// A request, with no certificate shown, for the name of a member.
    async fn member_asks(
        &self,
        mut pairing: Pairing,
        request: CertificateRequest,
        now: DateTime<Utc>,
    ) -> Result<Outcome, EnrollError> {
        if request.key == pairing.key {
            // The certificate is public, only the key makes it usable, so handing it out again to
            // whoever holds the key (a display that lost it, or let it expire) is no risk.
            return self.collect(pairing, request, now).await;
        }
        if pairing
            .rollover
            .as_ref()
            .is_some_and(|rollover| rollover.key == request.key)
        {
            // Collecting the certificate for the key it is changing to, which is a member beside the old.
            let certificate = self.certificate_for(&request, now)?;
            self.record(&mut pairing, &certificate, now);
            self.store.put(&pairing).await?;
            return Ok(Outcome::Issued(certificate));
        }
        if let Some(replacement) = pairing
            .replacement
            .as_ref()
            .filter(|replacement| replacement.key == request.key)
        {
            if !replacement.approved {
                return Ok(Outcome::Pending {
                    retry_after: self.policy.retry_after,
                    code: replacement.code.clone(),
                });
            }
            // The owner approved it: it takes the name over, and the old key stops being one.
            pairing.key = request.key.clone();
            pairing.rollover = None;
            pairing.replacement = None;
            return self.collect(pairing, request, now).await;
        }
        // Some other key wants the name. Only while the owner is letting displays ask, and only to wait.
        if self.window_closes_at().is_none() {
            return Err(Refusal::KeyMismatch.into());
        }
        self.ensure_room(&pairing.device).await?;
        let code =
            PairingCode::derive(&self.authority.fingerprint(), &pairing.device, &request.key);
        pairing.replacement = Some(Replacement {
            key: request.key,
            code: code.clone(),
            approved: false,
            requested_at: now,
        });
        pairing.updated_at = now;
        self.store.put(&pairing).await?;
        log::info!(
            "{} is a member, and another key asked for its name; the owner approves that with the code {code}",
            pairing.device
        );
        Ok(Outcome::Pending {
            retry_after: self.policy.retry_after,
            code,
        })
    }

    /// A member renewing its certificate, perhaps with a new key. `caller` and `caller_key` are who the
    /// connection's client certificate says it is and the key it holds. A name alone isn't enough: a
    /// certificate for a display that was forgotten, or replaced by another of the same name, must not
    /// renew itself into the new display's place.
    ///
    /// Asking for a certificate for a new key does not take the old key away. Both are members until the
    /// new one is first used (see `authenticate`), so a response that is lost, or a display that is asleep
    /// when the change is made, can't leave it holding a certificate nobody accepts.
    pub async fn renew(
        &self,
        caller: &DeviceId,
        caller_key: &PublicKey,
        request: &[u8],
        connection: Option<&[u8]>,
    ) -> Result<IssuedCertificate, EnrollError> {
        let request = self.read(request, connection)?;
        if request.device != *caller {
            return Err(Refusal::WrongDevice.into());
        }
        let _changing = self.changes.lock().await;
        let now = self.now();
        let pairing = self.store.get(caller).await?;
        let mut pairing = match pairing {
            Some(p) if matches!(p.state, PairingState::Enrolled { .. }) => p,
            Some(p) if p.state == PairingState::Revoked => return Err(Refusal::Revoked.into()),
            _ => return Err(Refusal::NotEnrolled.into()),
        };
        let used_new_key = pairing
            .rollover
            .as_ref()
            .is_some_and(|rollover| rollover.key == *caller_key);
        if pairing.key != *caller_key && !used_new_key {
            return Err(Refusal::CertificateSuperseded.into());
        }
        if used_new_key {
            self.promote(&mut pairing);
        }
        let certificate = self.certificate_for(&request, now)?;
        if request.key != pairing.key {
            pairing.rollover = Some(Rollover {
                key: request.key,
                since: now,
            });
        }
        self.record(&mut pairing, &certificate, now);
        self.store.put(&pairing).await?;
        Ok(certificate)
    }

    /// Whether `device`, holding `key`, is a member now, as its certificate alone can't say: a revoked
    /// display's certificate is still valid until it expires, and so is that of one that was replaced.
    ///
    /// A display that is changing keys is a member under either, and its first request with the new
    /// one is what makes the new one the key (it has shown it got the certificate), so this is also
    /// where the old key stops being one.
    pub async fn authenticate(&self, device: &DeviceId, key: &PublicKey) -> anyhow::Result<bool> {
        let _changing = self.changes.lock().await;
        let now = self.now();
        let Some(mut pairing) = self.store.get(device).await? else {
            return Ok(false);
        };
        match pairing.state {
            PairingState::Enrolled { not_after, .. } if not_after > now => {}
            _ => return Ok(false),
        }
        if pairing.key == *key {
            return Ok(true);
        }
        if pairing
            .rollover
            .as_ref()
            .is_some_and(|rollover| rollover.key == *key)
        {
            self.promote(&mut pairing);
            pairing.updated_at = now;
            self.store.put(&pairing).await?;
            log::info!("{device} is using its new key; the old one is no longer accepted");
            return Ok(true);
        }
        Ok(false)
    }

    /// The owner confirms a display, or a replacement for one, by typing in the code its panel shows.
    pub async fn approve(
        &self,
        device: &DeviceId,
        typed: &PairingCode,
    ) -> Result<(), ApproveError> {
        let _changing = self.changes.lock().await;
        let now = self.now();
        let mut pairing = self
            .store
            .get(device)
            .await?
            .ok_or_else(|| ApproveError::Unknown(device.clone()))?;
        let waiting_for = match (&pairing.state, &pairing.replacement) {
            (PairingState::Pending, _) => Some(pairing.code.clone()),
            (PairingState::Enrolled { .. }, Some(replacement)) if !replacement.approved => {
                Some(replacement.code.clone())
            }
            _ => None,
        };
        let Some(expected) = waiting_for else {
            return Err(ApproveError::NotPending {
                device: device.clone(),
                state: pairing.state.name(),
            });
        };
        let same: bool = typed
            .as_str()
            .as_bytes()
            .ct_eq(expected.as_str().as_bytes())
            .into();
        if !same {
            log::warn!(
                "A pairing code typed for {device} did not match what the server worked out for it"
            );
            return Err(ApproveError::WrongCode(device.clone()));
        }
        match pairing.state {
            PairingState::Pending => pairing.state = PairingState::Approved,
            _ => {
                if let Some(replacement) = pairing.replacement.as_mut() {
                    replacement.approved = true;
                }
            }
        }
        pairing.updated_at = now;
        self.store.put(&pairing).await?;
        log::info!("Approved {device}; it gets its certificate the next time it asks");
        Ok(())
    }

    /// Turns a waiting display down, until the owner removes it. For a member with a replacement
    /// waiting, turns the replacement down and leaves the member as it was.
    pub async fn reject(&self, device: &DeviceId) -> Result<(), ApproveError> {
        let _changing = self.changes.lock().await;
        let mut pairing = self
            .store
            .get(device)
            .await?
            .ok_or_else(|| ApproveError::Unknown(device.clone()))?;
        if matches!(pairing.state, PairingState::Enrolled { .. }) && pairing.replacement.is_some() {
            pairing.replacement = None;
            pairing.updated_at = self.now();
            self.store.put(&pairing).await?;
            log::info!("Turned down the replacement for {device}; it carries on as it was");
            return Ok(());
        }
        self.change_state(pairing, PairingState::Rejected, |s| {
            matches!(s, PairingState::Pending | PairingState::Approved)
        })
        .await?;
        log::info!("Turned down {device}");
        Ok(())
    }

    /// Ends a member's membership. Its certificate stays valid until it expires, but nothing that
    /// checks `authenticate` accepts it, and it can't renew.
    pub async fn revoke(&self, device: &DeviceId) -> Result<(), ApproveError> {
        let _changing = self.changes.lock().await;
        let pairing = self
            .store
            .get(device)
            .await?
            .ok_or_else(|| ApproveError::Unknown(device.clone()))?;
        self.change_state(pairing, PairingState::Revoked, |s| {
            matches!(s, PairingState::Enrolled { .. } | PairingState::Approved)
        })
        .await?;
        log::warn!("Revoked {device}");
        Ok(())
    }

    /// Forgets a display, whatever its state, so it can ask again as if for the first time.
    pub async fn forget(&self, device: &DeviceId) -> anyhow::Result<()> {
        let _changing = self.changes.lock().await;
        self.store.remove(device).await?;
        log::info!("Forgot {device}");
        Ok(())
    }

    pub async fn pairings(&self) -> anyhow::Result<Vec<Pairing>> {
        self.store.all().await
    }

    /// Changes `pairing` to `state` if it is in one `allowed` says may change. The caller holds the lock.
    async fn change_state(
        &self,
        mut pairing: Pairing,
        state: PairingState,
        allowed: impl Fn(&PairingState) -> bool,
    ) -> Result<(), ApproveError> {
        if !allowed(&pairing.state) {
            return Err(ApproveError::NotPending {
                device: pairing.device.clone(),
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

    /// Fails if there is no room for another display (or replacement) to be waiting.
    async fn ensure_room(&self, device: &DeviceId) -> Result<(), EnrollError> {
        let waiting = self
            .store
            .all()
            .await?
            .iter()
            .filter(|p| {
                p.device != *device && (p.state == PairingState::Pending || p.replacement.is_some())
            })
            .count();
        if waiting >= MAX_PENDING {
            return Err(Refusal::TooManyPending.into());
        }
        Ok(())
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
        self.ensure_room(&request.device).await?;
        let code =
            PairingCode::derive(&self.authority.fingerprint(), &request.device, &request.key);
        let pairing = Pairing::new(
            request.device,
            request.key,
            code.clone(),
            PairingState::Pending,
            now,
        );
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

    /// Gives `pairing`'s display a certificate for the key in `request`, which becomes its key.
    async fn collect(
        &self,
        mut pairing: Pairing,
        request: CertificateRequest,
        now: DateTime<Utc>,
    ) -> Result<Outcome, EnrollError> {
        let certificate = self.certificate_for(&request, now)?;
        pairing.code =
            PairingCode::derive(&self.authority.fingerprint(), &pairing.device, &request.key);
        pairing.key = request.key;
        self.record(&mut pairing, &certificate, now);
        self.store.put(&pairing).await?;
        Ok(Outcome::Issued(certificate))
    }

    /// A certificate for the key in `request`. Not while the clock is plainly wrong: a certificate
    /// dated from a clock that has not been set would be refused by every display whose clock is right.
    fn certificate_for(
        &self,
        request: &CertificateRequest,
        now: DateTime<Utc>,
    ) -> Result<IssuedCertificate, EnrollError> {
        if now < earliest_plausible() {
            return Err(EnrollError::Failed(anyhow::anyhow!(
                "the system clock reads {now}, which can't be right; not issuing certificates until it is set"
            )));
        }
        Ok(self
            .authority
            .issue(request, now, now + self.policy.certificate_lifetime)?)
    }

    fn record(&self, pairing: &mut Pairing, certificate: &IssuedCertificate, now: DateTime<Utc>) {
        pairing.state = PairingState::Enrolled {
            serial: certificate.serial.clone(),
            not_after: certificate.not_after,
        };
        pairing.updated_at = now;
        log::info!(
            "Issued {} a certificate (serial {}, until {})",
            pairing.device,
            certificate.serial,
            certificate.not_after.format("%Y-%m-%d")
        );
    }

    /// Makes the key a display is changing to its key.
    fn promote(&self, pairing: &mut Pairing) {
        if let Some(rollover) = pairing.rollover.take() {
            pairing.code = PairingCode::derive(
                &self.authority.fingerprint(),
                &pairing.device,
                &rollover.key,
            );
            pairing.key = rollover.key;
        }
    }
}

/// A date before which this program had not been written. A clock reading earlier than this has not
/// been set.
fn earliest_plausible() -> DateTime<Utc> {
    use chrono::TimeZone;
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
        .single()
        .expect("a valid date")
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

    /// The public key the fake authority reads out of a request made with `key`.
    fn held_key(key: &str) -> PublicKey {
        PublicKey::from_der(key.as_bytes().to_vec())
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
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
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
    async fn another_key_asking_for_a_members_name_waits_and_the_member_carries_on() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment.open_window(Duration::minutes(10));

        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k9"), None)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            Outcome::Pending {
                retry_after: Duration::minutes(5),
                code: f.code("kitchen", "k9"),
            }
        );
        // Asking again changes nothing, and the member is untouched while it waits.
        assert_eq!(
            f.enrollment
                .enroll(&csr("kitchen", "k9"), None)
                .await
                .unwrap(),
            outcome
        );
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        assert!(
            !f.enrollment
                .authenticate(&device("kitchen"), &held_key("k9"))
                .await
                .unwrap()
        );
        // And it still renews.
        f.enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k1"),
                None,
            )
            .await
            .unwrap();
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
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k1"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            renewed.not_after,
            f.clock.now().to_utc() + Duration::days(365)
        );
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_member_changing_keys_is_a_member_under_both_until_it_uses_the_new_one() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k2"),
                None,
            )
            .await
            .unwrap();
        // It has a certificate for k2 but may not have it yet, or may not have used it: both work.
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        // The first use of the new key shows it arrived, and then the old one is done.
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k2"))
                .await
                .unwrap()
        );
        assert!(
            !f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k2"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_member_can_not_renew_for_another_display() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        member(&f, "hall", "k2").await;
        let result = f
            .enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("hall", "k9"),
                None,
            )
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
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k1"),
                None,
            )
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
        assert!(
            !f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        let result = f
            .enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k1"),
                None,
            )
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
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        f.advance(Duration::days(2));
        assert!(
            !f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
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

    #[tokio::test]
    async fn a_certificate_for_a_display_that_was_replaced_can_not_renew_into_its_place() {
        // The owner forgot "kitchen" and another display joined under the same name. The first one's
        // certificate is still valid for months and names "kitchen", but it is not the one enrolled.
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment.forget(&device("kitchen")).await.unwrap();
        member(&f, "kitchen", "k2").await;

        let result = f
            .enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k9"),
                None,
            )
            .await;
        assert!(matches!(
            result,
            Err(EnrollError::Refused(Refusal::CertificateSuperseded))
        ));
        // The new display is untouched, and renews as itself.
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k2"))
                .await
                .unwrap()
        );
        assert!(
            !f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        f.enrollment
            .renew(
                &device("kitchen"),
                &held_key("k2"),
                &csr("kitchen", "k2"),
                None,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn after_a_change_of_key_only_the_new_key_is_a_member() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k2"),
                None,
            )
            .await
            .unwrap();
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k2"))
                .await
                .unwrap()
        );
        assert!(
            !f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        let old = f
            .enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k1"),
                None,
            )
            .await;
        assert!(matches!(
            old,
            Err(EnrollError::Refused(Refusal::CertificateSuperseded))
        ));
    }

    /// A store that holds a write of a member until told to let it through, to put a change in the
    /// middle of another.
    #[derive(Default)]
    struct GatedStore {
        inner: MemoryStore,
        armed: std::sync::atomic::AtomicBool,
        waiting: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl PairingStore for GatedStore {
        async fn get(&self, device: &DeviceId) -> anyhow::Result<Option<Pairing>> {
            self.inner.get(device).await
        }
        async fn put(&self, pairing: &Pairing) -> anyhow::Result<()> {
            let enrolled = matches!(pairing.state, PairingState::Enrolled { .. });
            if enrolled && self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                self.waiting.notify_one();
                self.release.notified().await;
            }
            self.inner.put(pairing).await
        }
        async fn remove(&self, device: &DeviceId) -> anyhow::Result<()> {
            self.inner.remove(device).await
        }
        async fn all(&self) -> anyhow::Result<Vec<Pairing>> {
            self.inner.all().await
        }
    }

    /// Waits for the store to be holding a write, and fails the test if it never does instead of hanging.
    async fn reached(store: &GatedStore) {
        tokio::time::timeout(std::time::Duration::from_secs(10), store.waiting.notified())
            .await
            .expect("the write that was to be held was never made");
    }

    /// A member, "kitchen" (key k1), on a store that can hold one of its writes.
    async fn gated_member() -> (Arc<Enrollment>, Arc<GatedStore>) {
        let store = Arc::new(GatedStore::default());
        let clock = Arc::new(TestClock(Mutex::new(
            Utc.with_ymd_and_hms(2026, 10, 8, 12, 0, 0).unwrap(),
        )));
        let enrollment = Arc::new(Enrollment::new(
            Arc::new(FakeAuthority::new()),
            store.clone(),
            clock,
            EnrollmentPolicy {
                certificate_lifetime: Duration::days(365),
                retry_after: Duration::minutes(5),
                require_channel_binding: false,
            },
        ));
        enrollment.open_window(Duration::minutes(10));
        enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        let code = PairingCode::derive(
            &Fingerprint::of(b"authority"),
            &device("kitchen"),
            &held_key("k1"),
        );
        enrollment.approve(&device("kitchen"), &code).await.unwrap();
        enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        (enrollment, store)
    }

    /// A renewal started and paused just before it writes the display back as a member.
    async fn renewal_under_way() -> (
        Arc<Enrollment>,
        Arc<GatedStore>,
        tokio::task::JoinHandle<Result<IssuedCertificate, EnrollError>>,
    ) {
        let (enrollment, store) = gated_member().await;
        store.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let renewing = tokio::spawn({
            let enrollment = enrollment.clone();
            async move {
                enrollment
                    .renew(
                        &device("kitchen"),
                        &held_key("k1"),
                        &csr("kitchen", "k1"),
                        None,
                    )
                    .await
            }
        });
        reached(&store).await;
        (enrollment, store, renewing)
    }

    #[tokio::test]
    async fn a_revocation_is_not_undone_by_a_new_key_being_promoted_at_the_same_time() {
        let (enrollment, store) = gated_member().await;
        enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k2"),
                None,
            )
            .await
            .unwrap();
        // The display's first request with k2 is promoting it, and is paused before it writes...
        store.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let promoting = tokio::spawn({
            let enrollment = enrollment.clone();
            async move {
                enrollment
                    .authenticate(&device("kitchen"), &held_key("k2"))
                    .await
            }
        });
        reached(&store).await;
        // ...when the owner revokes it.
        let revoking = tokio::spawn({
            let enrollment = enrollment.clone();
            async move { enrollment.revoke(&device("kitchen")).await }
        });
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        store.release.notify_one();
        assert!(promoting.await.unwrap().unwrap());
        revoking.await.unwrap().unwrap();

        assert_eq!(
            enrollment.pairings().await.unwrap().remove(0).state,
            PairingState::Revoked
        );
        assert!(
            !enrollment
                .authenticate(&device("kitchen"), &held_key("k2"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_revocation_is_not_undone_by_a_renewal_that_was_already_under_way() {
        let (enrollment, store, renewing) = renewal_under_way().await;
        let revoking = tokio::spawn({
            let enrollment = enrollment.clone();
            async move { enrollment.revoke(&device("kitchen")).await }
        });
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        store.release.notify_one();
        renewing.await.unwrap().unwrap();
        revoking.await.unwrap().unwrap();

        // The revocation came last, so it is what stands.
        let state = enrollment.pairings().await.unwrap().remove(0).state;
        assert_eq!(state, PairingState::Revoked);
        assert!(
            !enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn forgetting_a_display_is_not_undone_by_a_renewal_that_was_already_under_way() {
        let (enrollment, store, renewing) = renewal_under_way().await;
        let forgetting = tokio::spawn({
            let enrollment = enrollment.clone();
            async move { enrollment.forget(&device("kitchen")).await }
        });
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        store.release.notify_one();
        renewing.await.unwrap().unwrap();
        forgetting.await.unwrap().unwrap();

        assert!(enrollment.pairings().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_member_whose_certificate_has_expired_gets_a_new_one_by_asking_with_its_key() {
        // A display left in a drawer for longer than its certificate lasts. No window is open and no one
        // is at the server: holding the key it joined with is enough, as it was to get the certificate.
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.advance(Duration::days(400));
        assert!(
            !f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        assert_eq!(f.enrollment.window_closes_at(), None);

        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        assert!(matches!(outcome, Outcome::Issued(_)), "{outcome:?}");
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn an_expired_certificate_does_not_let_a_different_key_take_the_place() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.advance(Duration::days(400));
        let result = f.enrollment.enroll(&csr("kitchen", "k2"), None).await;
        assert_eq!(refused(result), Refusal::KeyMismatch);
    }

    #[tokio::test]
    async fn a_replacement_the_owner_approves_takes_the_name_and_the_old_key_is_done() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k9"), None)
            .await
            .unwrap();

        f.enrollment
            .approve(&device("kitchen"), &f.code("kitchen", "k9"))
            .await
            .unwrap();
        // Approved is not collected: until it is, the old key is the member.
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k9"), None)
            .await
            .unwrap();
        assert!(matches!(outcome, Outcome::Issued(_)), "{outcome:?}");

        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k9"))
                .await
                .unwrap()
        );
        assert!(
            !f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        let stale = f
            .enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k1"),
                None,
            )
            .await;
        assert!(matches!(
            stale,
            Err(EnrollError::Refused(Refusal::CertificateSuperseded))
        ));
    }

    #[tokio::test]
    async fn a_replacement_needs_the_window_and_its_own_code() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        // No window: nothing is recorded, as before.
        let closed = f.enrollment.enroll(&csr("kitchen", "k9"), None).await;
        assert_eq!(refused(closed), Refusal::KeyMismatch);
        assert!(
            f.enrollment.pairings().await.unwrap()[0]
                .replacement
                .is_none()
        );

        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k9"), None)
            .await
            .unwrap();
        // The member's own code, or another's, approves nothing.
        for wrong in [f.code("kitchen", "k1"), f.code("hall", "k9")] {
            let result = f.enrollment.approve(&device("kitchen"), &wrong).await;
            assert!(
                matches!(result, Err(ApproveError::WrongCode(_))),
                "{result:?}"
            );
        }
        // Not approved, so it still only waits.
        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k9"), None)
            .await
            .unwrap();
        assert!(matches!(outcome, Outcome::Pending { .. }));
    }

    #[tokio::test]
    async fn turning_a_replacement_down_leaves_the_member_as_it_was() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k9"), None)
            .await
            .unwrap();
        f.enrollment.reject(&device("kitchen")).await.unwrap();

        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        let pairings = f.enrollment.pairings().await.unwrap();
        assert!(pairings[0].replacement.is_none());
        assert!(matches!(pairings[0].state, PairingState::Enrolled { .. }));
        // It may ask again, and it is as if it had not.
        let again = f
            .enrollment
            .enroll(&csr("kitchen", "k9"), None)
            .await
            .unwrap();
        assert!(matches!(again, Outcome::Pending { .. }));
    }

    #[tokio::test]
    async fn a_second_replacement_takes_the_first_ones_place_and_its_code() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k8"), None)
            .await
            .unwrap();
        f.enrollment
            .enroll(&csr("kitchen", "k9"), None)
            .await
            .unwrap();
        let old = f
            .enrollment
            .approve(&device("kitchen"), &f.code("kitchen", "k8"))
            .await;
        assert!(matches!(old, Err(ApproveError::WrongCode(_))));
        f.enrollment
            .approve(&device("kitchen"), &f.code("kitchen", "k9"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn displays_waiting_to_join_count_towards_the_limit_on_replacements() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment.open_window(Duration::minutes(10));
        for i in 0..MAX_PENDING {
            f.enrollment
                .enroll(&csr(&format!("device-{i}"), "k"), None)
                .await
                .unwrap();
        }
        let result = f.enrollment.enroll(&csr("kitchen", "k9"), None).await;
        assert_eq!(refused(result), Refusal::TooManyPending);
    }

    #[tokio::test]
    async fn a_display_that_missed_the_response_collects_the_certificate_for_its_new_key_again() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k2"),
                None,
            )
            .await
            .unwrap();
        // The answer never arrived, so it asks again for k2, with no window open and no one at the server.
        assert_eq!(f.enrollment.window_closes_at(), None);
        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k2"), None)
            .await
            .unwrap();
        assert!(matches!(outcome, Outcome::Issued(_)), "{outcome:?}");
        // Meanwhile the old key still works, so nothing stopped in the meantime.
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_new_key_stays_a_member_however_long_the_display_is_off_before_it_uses_it() {
        // The display stored the certificate for its new key, then went flat in a drawer. Whatever the
        // time, it must find the way open with what it holds, and no owner involved.
        for days in [40, 400] {
            let f = Fixture::new();
            member(&f, "kitchen", "k1").await;
            f.enrollment
                .renew(
                    &device("kitchen"),
                    &held_key("k1"),
                    &csr("kitchen", "k2"),
                    None,
                )
                .await
                .unwrap();
            f.advance(Duration::days(days));
            assert_eq!(f.enrollment.window_closes_at(), None);

            // If the certificate has expired meanwhile, it asks again for one, with k2 and nothing else.
            if !f
                .enrollment
                .authenticate(&device("kitchen"), &held_key("k2"))
                .await
                .unwrap()
            {
                let outcome = f
                    .enrollment
                    .enroll(&csr("kitchen", "k2"), None)
                    .await
                    .unwrap();
                assert!(
                    matches!(outcome, Outcome::Issued(_)),
                    "{days} days: {outcome:?}"
                );
            }
            assert!(
                f.enrollment
                    .authenticate(&device("kitchen"), &held_key("k2"))
                    .await
                    .unwrap(),
                "{days} days"
            );
            // Using it made it the key.
            assert!(
                !f.enrollment
                    .authenticate(&device("kitchen"), &held_key("k1"))
                    .await
                    .unwrap()
            );
        }
    }

    #[tokio::test]
    async fn renewing_with_the_new_key_counts_as_using_it() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k2"),
                None,
            )
            .await
            .unwrap();
        // The display renews, now with k2's certificate.
        f.enrollment
            .renew(
                &device("kitchen"),
                &held_key("k2"),
                &csr("kitchen", "k2"),
                None,
            )
            .await
            .unwrap();
        assert!(
            !f.enrollment
                .authenticate(&device("kitchen"), &held_key("k1"))
                .await
                .unwrap()
        );
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k2"))
                .await
                .unwrap()
        );
        assert!(f.enrollment.pairings().await.unwrap()[0].rollover.is_none());
    }

    #[tokio::test]
    async fn a_third_key_during_a_change_replaces_the_pending_one() {
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        for next in ["k2", "k3"] {
            f.enrollment
                .renew(
                    &device("kitchen"),
                    &held_key("k1"),
                    &csr("kitchen", next),
                    None,
                )
                .await
                .unwrap();
        }
        assert!(
            !f.enrollment
                .authenticate(&device("kitchen"), &held_key("k2"))
                .await
                .unwrap()
        );
        assert!(
            f.enrollment
                .authenticate(&device("kitchen"), &held_key("k3"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn no_certificate_is_issued_while_the_clock_is_plainly_wrong() {
        let f = Fixture::new();
        f.advance(Duration::days(-1000)); // a clock that has not been set
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        f.enrollment
            .approve(&device("kitchen"), &f.code("kitchen", "k1"))
            .await
            .unwrap();
        let result = f.enrollment.enroll(&csr("kitchen", "k1"), None).await;
        assert!(
            matches!(&result, Err(EnrollError::Failed(e)) if e.to_string().contains("clock")),
            "{result:?}"
        );
        // Nothing was issued, so once the clock is right the same request is granted.
        f.advance(Duration::days(1000));
        let outcome = f
            .enrollment
            .enroll(&csr("kitchen", "k1"), None)
            .await
            .unwrap();
        assert!(matches!(outcome, Outcome::Issued(_)));
    }

    #[tokio::test]
    async fn a_takeover_ends_a_key_change_that_was_under_way() {
        // The display was changing to k2 when another key, k9, was approved to take the name over. Once
        // it has, neither of the keys it replaced is a member, the pending k2 included.
        let f = Fixture::new();
        member(&f, "kitchen", "k1").await;
        f.enrollment
            .renew(
                &device("kitchen"),
                &held_key("k1"),
                &csr("kitchen", "k2"),
                None,
            )
            .await
            .unwrap();
        f.enrollment.open_window(Duration::minutes(10));
        f.enrollment
            .enroll(&csr("kitchen", "k9"), None)
            .await
            .unwrap();
        f.enrollment
            .approve(&device("kitchen"), &f.code("kitchen", "k9"))
            .await
            .unwrap();
        f.enrollment
            .enroll(&csr("kitchen", "k9"), None)
            .await
            .unwrap();

        let device = device("kitchen");
        assert!(
            f.enrollment
                .authenticate(&device, &held_key("k9"))
                .await
                .unwrap()
        );
        assert!(
            !f.enrollment
                .authenticate(&device, &held_key("k2"))
                .await
                .unwrap()
        );
        assert!(
            !f.enrollment
                .authenticate(&device, &held_key("k1"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn replacements_waiting_count_towards_the_limit_on_new_displays() {
        // Eight members each have a different key asking for their name. That is as many waiting as are
        // allowed, so a stranger can't add to them by asking to join.
        let f = Fixture::new();
        for i in 0..MAX_PENDING {
            let name = format!("device-{i}");
            member(&f, &name, "k1").await;
            f.enrollment.open_window(Duration::minutes(10));
            f.enrollment.enroll(&csr(&name, "k9"), None).await.unwrap();
        }
        f.enrollment.open_window(Duration::minutes(10));
        let result = f.enrollment.enroll(&csr("newcomer", "k1"), None).await;
        assert_eq!(refused(result), Refusal::TooManyPending);
    }
}
