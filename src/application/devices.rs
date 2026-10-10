//! What the displays say about themselves, for `/status` and `/metrics`.
//!
//! A display names itself on every request (`device`, see `DeviceId`), and reports on each check-in as
//! query parameters on the `/plan` or `/refresh` call it makes anyway (so it costs no extra radio time):
//! its battery, signal, and how its last wakes went. What each display was last sent is noted when it
//! fetches the image. Everything is kept under the display's own name, never worked out from an address
//! or the order requests arrived in, so two displays can't be mixed up. The server also knows when that display was told to come back, so it can tell a
//! display that is merely asleep from one that has gone quiet (a flat battery, a dead Wi-Fi
//! network): it is overdue once it is later than that, plus a grace period.
//!
//! Everything here comes from the network, so it is bounded and checked: a few devices, names that are
//! valid `DeviceId`s, and readings in a plausible range. A bad value is dropped, never
//! an error, because the display must still get its plan.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Duration};
use chrono_tz::Tz;
use serde::Serialize;
use utoipa::{IntoParams, ToSchema};

use super::api_schema::ImageFormatSchema;

use crate::domain::models::device_id::DeviceId;
use crate::domain::models::display::ImageFormat;
use crate::domain::models::profile::firmware_version;

/// More than a household has; stops a stray client filling memory with made-up names.
const MAX_DEVICES: usize = 16;
/// A Li-ion cell is never outside this, so anything else is a misread.
const MILLIVOLTS: std::ops::RangeInclusive<u32> = 2000..=5000;
const PERCENT: std::ops::RangeInclusive<u32> = 0..=100;
/// A TLS handshake the display timed: more than a millisecond, and well within the wake's own limit.
const TLS_MILLISECONDS: std::ops::RangeInclusive<u32> = 1..=120_000;
/// The lowest free heap a display saw during a wake. The chip has under a megabyte; anything past 16 MB is a misread.
const HEAP_BYTES: std::ops::RangeInclusive<u32> = 1..=16_777_216;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BatteryState {
    Ok,
    /// Charge soon; the display shows a label saying so.
    Low,
    /// The display has stopped refreshing to protect the cell.
    Empty,
}

impl BatteryState {
    pub const ALL: [BatteryState; 3] = [Self::Ok, Self::Low, Self::Empty];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Low => "low",
            Self::Empty => "empty",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == text)
    }
}

/// Why a display's last wake failed, as it reports it on the next wake that gets through. A wake can't
/// report its own failure (it asks `/plan` before it knows), and a Wi-Fi or server failure can only be
/// told once the display is talking to the server again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    /// Couldn't join the network.
    Wifi,
    /// The server didn't answer `/plan` usably.
    Server,
    /// `/plan` answered, but the image didn't download or decode.
    Download,
    /// Not enough memory to decode the image.
    Memory,
    /// The wake hung and was cut off.
    Timeout,
    /// The clock wasn't set, so no certificate could be checked and nothing that needs one was tried.
    Clock,
    /// A certificate was refused: the server's wasn't trusted, or the display's wasn't accepted.
    Certificate,
    /// The display asked to join and is waiting for the owner to approve it.
    Approval,
    /// The server doesn't know the display's key (it was erased, or replaced): the owner has to approve it again.
    Unrecognised,
}

impl FailureReason {
    pub const ALL: [FailureReason; 9] = [
        Self::Wifi,
        Self::Server,
        Self::Download,
        Self::Memory,
        Self::Timeout,
        Self::Clock,
        Self::Certificate,
        Self::Approval,
        Self::Unrecognised,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Server => "server",
            Self::Download => "download",
            Self::Memory => "memory",
            Self::Timeout => "timeout",
            Self::Clock => "clock",
            Self::Certificate => "certificate",
            Self::Approval => "approval",
            Self::Unrecognised => "unrecognised",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == text)
    }
}

/// A Wi-Fi signal is a small negative number of dBm; 0 is what a radio reports when it has no reading.
const RSSI_DBM: std::ops::RangeInclusive<i16> = -127..=-1;
/// Longer than any wake is allowed to run, so anything more is a misread.
const WAKE_SECONDS: std::ops::RangeInclusive<u32> = 0..=600;

/// What a display's readings must be for the server to keep them. A reading outside its range, or not a
/// whole number, is ignored: the request is still answered, as the display must always get its plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryLimits {
    pub battery_millivolts: std::ops::RangeInclusive<u32>,
    pub battery_percent: std::ops::RangeInclusive<u32>,
    pub wifi_rssi_dbm: std::ops::RangeInclusive<i16>,
    pub wake_seconds: std::ops::RangeInclusive<u32>,
}

pub fn telemetry_limits() -> TelemetryLimits {
    TelemetryLimits {
        battery_millivolts: MILLIVOLTS,
        battery_percent: PERCENT,
        wifi_rssi_dbm: RSSI_DBM,
        wake_seconds: WAKE_SECONDS,
    }
}

/// The query parameters as received. All text, so a malformed one can't fail the request. A display adds
/// these to the `/plan` or `/refresh` call it makes anyway, so reporting costs no extra radio time. Every one
/// is optional, and one that is missing, malformed or out of range is ignored.
#[derive(Debug, Default, Clone, serde::Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RawTelemetry {
    /// Battery voltage, in millivolts.
    #[param(value_type = Option<u32>, example = 3712)]
    pub battery_mv: Option<String>,
    /// Battery charge, as a percentage.
    #[param(value_type = Option<u8>, example = 47)]
    pub battery_pct: Option<String>,
    /// How the display rates its own battery. The display decides: the server only passes it on.
    #[param(value_type = Option<BatteryState>)]
    pub battery_state: Option<String>,
    /// How many wakes in a row have failed, for any reason.
    #[param(value_type = Option<u32>, example = 2)]
    pub failed_wakes: Option<String>,
    /// Wi-Fi signal strength, in dBm.
    #[param(value_type = Option<i16>, example = -71)]
    pub rssi: Option<String>,
    /// Why the wake before this one failed, if it did.
    #[param(value_type = Option<FailureReason>)]
    pub last_failure: Option<String>,
    /// How long the wake before this one was awake, in seconds: what costs battery.
    #[param(value_type = Option<u32>, example = 24)]
    pub last_wake_s: Option<String>,
    /// How long the display's first TLS handshake of the wake before this one took, in milliseconds, as the
    /// display timed it (the server also times them, from its side: `connection` in `/status`).
    #[param(value_type = Option<u32>, example = 1100)]
    pub last_tls_ms: Option<String>,
    /// The least free heap the display had during the wake before this one, in bytes: how close the handshake
    /// came to running it out.
    #[param(value_type = Option<u32>, example = 61440)]
    pub last_heap_min: Option<String>,
    /// The version of the firmware the display runs, as released (letters, digits, `.`, `-` and `_`, at most 32).
    #[param(value_type = Option<String>, example = "0.2.0")]
    pub fw: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Telemetry {
    pub device: DeviceId,
    pub battery_millivolts: Option<u32>,
    pub battery_percent: Option<u8>,
    pub battery_state: Option<BatteryState>,
    pub failed_wakes: Option<u32>,
    pub wifi_rssi_dbm: Option<i16>,
    pub last_failure: Option<FailureReason>,
    pub last_wake_seconds: Option<u32>,
    pub last_tls_milliseconds: Option<u32>,
    pub last_heap_min_bytes: Option<u32>,
    pub firmware: Option<String>,
}

impl RawTelemetry {
    /// What `device` reported. A value that isn't usable is dropped and the rest kept.
    pub fn parse(&self, device: DeviceId) -> Telemetry {
        let number = |text: &Option<String>| text.as_deref().and_then(|t| t.parse::<u32>().ok());
        Telemetry {
            device,
            battery_millivolts: number(&self.battery_mv).filter(|mv| MILLIVOLTS.contains(mv)),
            battery_percent: number(&self.battery_pct)
                .filter(|pct| PERCENT.contains(pct))
                .map(|pct| pct as u8),
            battery_state: self.battery_state.as_deref().and_then(BatteryState::parse),
            failed_wakes: number(&self.failed_wakes),
            wifi_rssi_dbm: self
                .rssi
                .as_deref()
                .and_then(|t| t.parse::<i16>().ok())
                .filter(|dbm| RSSI_DBM.contains(dbm)),
            last_failure: self.last_failure.as_deref().and_then(FailureReason::parse),
            last_wake_seconds: number(&self.last_wake_s)
                .filter(|seconds| WAKE_SECONDS.contains(seconds)),
            last_tls_milliseconds: number(&self.last_tls_ms)
                .filter(|ms| TLS_MILLISECONDS.contains(ms)),
            last_heap_min_bytes: number(&self.last_heap_min)
                .filter(|bytes| HEAP_BYTES.contains(bytes)),
            firmware: self.fw.as_deref().and_then(firmware_version),
        }
    }
}

/// A display that has checked in: its battery, signal and how its last wakes went.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DeviceStatus {
    /// The name the display gives itself.
    #[schema(value_type = String, examples("reterminal-e1003-a1b2c3"), max_length = 32, pattern = "^[A-Za-z0-9._-]+$")]
    pub name: DeviceId,
    pub last_seen: DateTime<Tz>,
    pub age_seconds: u64,
    /// When the display was told to come back.
    pub expected_by: DateTime<Tz>,
    /// Later than it was told to be, plus the grace period.
    pub overdue: bool,
    pub battery_millivolts: Option<u32>,
    pub battery_percent: Option<u8>,
    pub battery_state: Option<BatteryState>,
    pub failed_wakes: Option<u32>,
    /// Wi-Fi signal strength at the last check-in.
    pub wifi_rssi_dbm: Option<i16>,
    /// Why the wake before that one failed, if it did.
    pub last_failure: Option<FailureReason>,
    /// How long the wake before that one was awake, which is what costs battery.
    pub last_wake_seconds: Option<u32>,
    /// How long its first TLS handshake of the wake before took, as the display timed it.
    pub last_tls_milliseconds: Option<u32>,
    /// The least free heap it had during that wake, in bytes.
    pub last_heap_min_bytes: Option<u32>,
    /// The version of the firmware it runs, as it said.
    pub firmware: Option<String>,
    /// How its TLS connections have begun, as the server saw them: resumed or full, and how long the handshake
    /// took. Empty for a display that has not connected over TLS since the server started.
    pub connection: Option<ConnectionStatus>,
    /// The last image this display was sent, if it has fetched one since the server started.
    pub last_image: Option<DeliveryStatus>,
}

/// How a display's TLS connections have begun since the server started, from the server's side: it cannot be
/// misreported by the display, and the time includes the display's own work and the network between.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ConnectionStatus {
    pub full_handshakes: u64,
    pub resumed_handshakes: u64,
    /// Whether the latest connection resumed a session (which saves the certificate exchange and signatures).
    pub last_resumed: bool,
    /// From accepting the latest connection to the end of its handshake.
    pub last_handshake_milliseconds: u32,
    pub last_at: DateTime<Tz>,
}

impl ConnectionStatus {
    pub fn of(handshakes: &crate::application::handshakes::DeviceHandshakes, zone: Tz) -> Self {
        Self {
            full_handshakes: handshakes.full,
            resumed_handshakes: handshakes.resumed,
            last_resumed: handshakes.last_resumed,
            last_handshake_milliseconds: u32::try_from(handshakes.last_handshake.as_millis())
                .unwrap_or(u32::MAX),
            last_at: handshakes.last_at.with_timezone(&zone),
        }
    }
}

/// The last image a display was sent: which format, how big, and when.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DeliveryStatus {
    #[schema(value_type = ImageFormatSchema)]
    pub format: ImageFormat,
    pub bytes: u64,
    pub at: DateTime<Tz>,
    pub age_seconds: u64,
}

#[derive(Debug, Clone, Copy)]
struct Delivery {
    format: ImageFormat,
    bytes: u64,
    at: DateTime<Tz>,
}

struct Record {
    last_seen: DateTime<Tz>,
    expected_by: DateTime<Tz>,
    telemetry: Telemetry,
    /// What the display was last sent. Kept when it checks in again, which replaces the rest.
    delivery: Option<Delivery>,
}

pub struct DeviceBoard {
    overdue_grace: Duration,
    devices: Mutex<BTreeMap<DeviceId, Record>>,
}

impl DeviceBoard {
    pub fn new(overdue_grace: std::time::Duration) -> Arc<Self> {
        Arc::new(Self {
            overdue_grace: Duration::from_std(overdue_grace).unwrap_or(Duration::MAX),
            devices: Mutex::default(),
        })
    }

    fn devices(&self) -> std::sync::MutexGuard<'_, BTreeMap<DeviceId, Record>> {
        self.devices.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Notes a check-in at `now`, from a display just told to come back in `next_seconds`.
    /// Logs what an operator would want to hear about: a battery getting worse, or a display
    /// that returns after going quiet.
    pub fn record(&self, now: DateTime<Tz>, telemetry: Telemetry, next_seconds: u64) {
        let expected_by = Duration::try_seconds(next_seconds.try_into().unwrap_or(i64::MAX))
            .and_then(|wait| now.checked_add_signed(wait))
            .unwrap_or(now);
        let mut devices = self.devices();
        if devices.len() >= MAX_DEVICES && !devices.contains_key(&telemetry.device) {
            log::warn!(
                "Ignoring {:?}: already tracking {MAX_DEVICES} devices",
                telemetry.device
            );
            return;
        }
        let previous = devices.get(&telemetry.device);
        match previous {
            None => {
                log::info!(
                    "New display {:?} checked in{}",
                    telemetry.device,
                    describe_battery(&telemetry)
                );
                if let Some(reason) = telemetry.last_failure {
                    log::warn!(
                        "Display {:?} {}",
                        telemetry.device,
                        describe_failure(reason, telemetry.failed_wakes)
                    );
                }
                if let Some(firmware) = &telemetry.firmware {
                    log::info!("Display {:?} runs firmware {firmware}", telemetry.device);
                }
            }
            Some(before) => {
                if let (Some(was), Some(now_runs)) =
                    (&before.telemetry.firmware, &telemetry.firmware)
                    && was != now_runs
                {
                    log::info!(
                        "Display {:?} now runs firmware {now_runs} (it ran {was})",
                        telemetry.device
                    );
                }
                if before.telemetry.last_failure != telemetry.last_failure {
                    match telemetry.last_failure {
                        Some(reason) => log::warn!(
                            "Display {:?} {}",
                            telemetry.device,
                            describe_failure(reason, telemetry.failed_wakes)
                        ),
                        None => log::info!("Display {:?} is working again", telemetry.device),
                    }
                }
                if before.telemetry.battery_state != telemetry.battery_state {
                    match telemetry.battery_state {
                        Some(BatteryState::Low | BatteryState::Empty) => log::warn!(
                            "Display {:?} battery is {}{}",
                            telemetry.device,
                            telemetry
                                .battery_state
                                .map_or("unknown", BatteryState::as_str),
                            describe_battery(&telemetry)
                        ),
                        _ => log::info!(
                            "Display {:?} battery is back to normal{}",
                            telemetry.device,
                            describe_battery(&telemetry)
                        ),
                    }
                }
                if now > before.expected_by + self.overdue_grace {
                    log::info!(
                        "Display {:?} is back after {} minutes of silence",
                        telemetry.device,
                        (now - before.expected_by).num_minutes()
                    );
                }
            }
        }
        let delivery = devices
            .get(&telemetry.device)
            .and_then(|record| record.delivery);
        devices.insert(
            telemetry.device.clone(),
            Record {
                last_seen: now,
                expected_by,
                telemetry,
                delivery,
            },
        );
    }

    /// Notes that `device` was sent an image of `bytes` in `format` at `now`. Only for a display that has
    /// checked in: a stray request can't add a device (the display always asks `/plan` first), so the limit
    /// on how many are tracked holds here too.
    pub fn image_served(
        &self,
        device: &DeviceId,
        format: ImageFormat,
        bytes: u64,
        now: DateTime<Tz>,
    ) {
        match self.devices().get_mut(device) {
            Some(record) => {
                record.delivery = Some(Delivery {
                    format,
                    bytes,
                    at: now,
                })
            }
            None => log::debug!("{device} fetched the image without checking in first"),
        }
    }

    pub fn snapshot(&self, now: DateTime<Tz>) -> Vec<DeviceStatus> {
        self.devices()
            .iter()
            .map(|(name, record)| DeviceStatus {
                name: name.clone(),
                last_seen: record.last_seen,
                age_seconds: (now - record.last_seen).num_seconds().max(0).unsigned_abs(),
                expected_by: record.expected_by,
                overdue: now > record.expected_by + self.overdue_grace,
                battery_millivolts: record.telemetry.battery_millivolts,
                battery_percent: record.telemetry.battery_percent,
                battery_state: record.telemetry.battery_state,
                failed_wakes: record.telemetry.failed_wakes,
                wifi_rssi_dbm: record.telemetry.wifi_rssi_dbm,
                last_failure: record.telemetry.last_failure,
                last_wake_seconds: record.telemetry.last_wake_seconds,
                last_tls_milliseconds: record.telemetry.last_tls_milliseconds,
                last_heap_min_bytes: record.telemetry.last_heap_min_bytes,
                firmware: record.telemetry.firmware.clone(),
                connection: None,
                last_image: record.delivery.map(|delivery| DeliveryStatus {
                    format: delivery.format,
                    bytes: delivery.bytes,
                    at: delivery.at,
                    age_seconds: (now - delivery.at).num_seconds().max(0).unsigned_abs(),
                }),
            })
            .collect()
    }
}

fn describe_failure(reason: FailureReason, failed_wakes: Option<u32>) -> String {
    match failed_wakes {
        Some(count) if count > 0 => format!(
            "had {count} failed wake(s) in a row, the last because of {}",
            reason.as_str()
        ),
        _ => format!(
            "reports its last wake failed because of {}",
            reason.as_str()
        ),
    }
}

fn describe_battery(telemetry: &Telemetry) -> String {
    match (telemetry.battery_millivolts, telemetry.battery_percent) {
        (Some(mv), Some(pct)) => format!(" ({mv} mV, {pct}%)"),
        (Some(mv), None) => format!(" ({mv} mV)"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    use super::*;

    fn at(h: u32, m: u32) -> DateTime<Tz> {
        London.with_ymd_and_hms(2026, 6, 15, h, m, 0).unwrap()
    }

    fn raw(pairs: &[(&str, &str)]) -> RawTelemetry {
        let get = |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        };
        RawTelemetry {
            battery_mv: get("battery_mv"),
            battery_pct: get("battery_pct"),
            battery_state: get("battery_state"),
            failed_wakes: get("failed_wakes"),
            rssi: get("rssi"),
            last_failure: get("last_failure"),
            last_wake_s: get("last_wake_s"),
            last_tls_ms: get("last_tls_ms"),
            last_heap_min: get("last_heap_min"),
            fw: get("fw"),
        }
    }

    fn id(name: &str) -> DeviceId {
        DeviceId::parse(name).unwrap()
    }

    fn telemetry(name: &str, state: Option<BatteryState>) -> Telemetry {
        Telemetry {
            device: id(name),
            battery_millivolts: Some(3700),
            battery_percent: Some(45),
            battery_state: state,
            failed_wakes: Some(0),
            wifi_rssi_dbm: Some(-60),
            last_failure: None,
            last_wake_seconds: Some(21),
            last_tls_milliseconds: Some(1100),
            last_heap_min_bytes: Some(61_440),
            firmware: Some("0.1.0".to_owned()),
        }
    }

    #[test]
    fn parses_good_telemetry() {
        let parsed = raw(&[
            ("battery_mv", "3712"),
            ("battery_pct", "47"),
            ("battery_state", "low"),
            ("failed_wakes", "2"),
            ("rssi", "-67"),
            ("last_failure", "download"),
            ("last_wake_s", "24"),
            ("last_tls_ms", "1100"),
            ("last_heap_min", "61440"),
            ("fw", "0.3.0-beta.1"),
        ])
        .parse(id("reterminal-e1003"));
        assert_eq!(parsed.firmware.as_deref(), Some("0.3.0-beta.1"));
        assert_eq!(parsed.last_tls_milliseconds, Some(1100));
        assert_eq!(parsed.last_heap_min_bytes, Some(61_440));
        assert_eq!(parsed.wifi_rssi_dbm, Some(-67));
        assert_eq!(parsed.last_failure, Some(FailureReason::Download));
        assert_eq!(parsed.last_wake_seconds, Some(24));
        assert_eq!(parsed.device.as_str(), "reterminal-e1003");
        assert_eq!(parsed.battery_millivolts, Some(3712));
        assert_eq!(parsed.battery_percent, Some(47));
        assert_eq!(parsed.battery_state, Some(BatteryState::Low));
        assert_eq!(parsed.failed_wakes, Some(2));
    }

    #[test]
    fn drops_bad_values_but_keeps_the_rest() {
        let parsed = raw(&[
            ("battery_mv", "999999"),
            ("battery_pct", "250"),
            ("battery_state", "on fire"),
            ("failed_wakes", "many"),
            ("rssi", "0"),
            ("last_failure", "gremlins"),
            ("last_wake_s", "99999"),
            ("last_tls_ms", "0"),
            ("last_heap_min", "4294967295"),
            ("fw", "1.0 <script>"),
        ])
        .parse(id("kitchen"));
        assert_eq!(parsed.firmware, None);
        assert_eq!(parsed.last_tls_milliseconds, None);
        assert_eq!(parsed.last_heap_min_bytes, None);
        assert_eq!(
            (
                parsed.wifi_rssi_dbm,
                parsed.last_failure,
                parsed.last_wake_seconds
            ),
            (None, None, None)
        );
        assert_eq!(parsed.battery_millivolts, None);
        assert_eq!(parsed.battery_percent, None);
        assert_eq!(parsed.battery_state, None);
        assert_eq!(parsed.failed_wakes, None);
    }

    #[test]
    fn the_failure_names_are_the_ones_the_firmware_sends() {
        // esphome/tests/report_test.cpp pins the same list on the other side, so a name changed on one fails a test
        // on the other.
        let names: Vec<_> = FailureReason::ALL
            .iter()
            .map(|reason| reason.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "wifi",
                "server",
                "download",
                "memory",
                "timeout",
                "clock",
                "certificate",
                "approval",
                "unrecognised"
            ]
        );
    }

    #[test]
    fn every_failure_reason_the_display_can_name_is_understood() {
        for reason in FailureReason::ALL {
            let parsed = raw(&[("last_failure", reason.as_str())]).parse(id("a"));
            assert_eq!(parsed.last_failure, Some(reason));
        }
        // The edges of what a signal and a wake can be.
        for (rssi, ok) in [
            ("-127", true),
            ("-1", true),
            ("0", false),
            ("-128", false),
            ("12", false),
            ("weak", false),
        ] {
            let parsed = raw(&[("rssi", rssi)]).parse(id("a"));
            assert_eq!(parsed.wifi_rssi_dbm.is_some(), ok, "{rssi}");
        }
        for (seconds, ok) in [("0", true), ("600", true), ("601", false), ("-3", false)] {
            let parsed = raw(&[("last_wake_s", seconds)]).parse(id("a"));
            assert_eq!(parsed.last_wake_seconds.is_some(), ok, "{seconds}");
        }
    }

    #[test]
    fn the_latest_reason_and_signal_are_kept_and_shown() {
        let board = DeviceBoard::new(std::time::Duration::from_secs(900));
        board.record(
            at(12, 0),
            Telemetry {
                last_failure: Some(FailureReason::Server),
                failed_wakes: Some(3),
                ..telemetry("a", None)
            },
            600,
        );
        let device = &board.snapshot(at(12, 1))[0];
        assert_eq!(
            (
                device.last_failure,
                device.wifi_rssi_dbm,
                device.last_wake_seconds
            ),
            (Some(FailureReason::Server), Some(-60), Some(21))
        );

        board.record(at(12, 10), telemetry("a", None), 600);
        assert_eq!(board.snapshot(at(12, 11))[0].last_failure, None);
    }

    #[test]
    fn a_device_is_overdue_only_after_its_grace_period() {
        let board = DeviceBoard::new(std::time::Duration::from_secs(15 * 60));
        board.record(at(12, 0), telemetry("a", Some(BatteryState::Ok)), 600);

        // Told to return at 12:10; overdue after 12:25.
        assert!(!board.snapshot(at(12, 9))[0].overdue);
        assert!(!board.snapshot(at(12, 25))[0].overdue);
        let late = &board.snapshot(at(12, 26))[0];
        assert!(late.overdue);
        assert_eq!(late.age_seconds, 26 * 60);

        // Checking in again clears it.
        board.record(at(12, 27), telemetry("a", Some(BatteryState::Ok)), 600);
        assert!(!board.snapshot(at(12, 28))[0].overdue);
    }

    #[test]
    fn a_long_sleep_overnight_is_not_overdue() {
        let board = DeviceBoard::new(std::time::Duration::from_secs(15 * 60));
        board.record(at(22, 55), telemetry("a", None), 7 * 3600 + 330);
        assert!(!board.snapshot(at(5, 59))[0].overdue);
    }

    #[test]
    fn keeps_the_latest_reading_for_each_device() {
        let board = DeviceBoard::new(std::time::Duration::from_secs(900));
        board.record(at(12, 0), telemetry("a", Some(BatteryState::Ok)), 600);
        board.record(
            at(12, 10),
            Telemetry {
                battery_millivolts: Some(3350),
                ..telemetry("a", Some(BatteryState::Low))
            },
            600,
        );
        board.record(at(12, 10), telemetry("b", None), 600);

        let devices = board.snapshot(at(12, 11));
        assert_eq!(
            devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(devices[0].battery_millivolts, Some(3350));
        assert_eq!(devices[0].battery_state, Some(BatteryState::Low));
    }

    #[test]
    fn stops_tracking_new_names_past_the_limit() {
        let board = DeviceBoard::new(std::time::Duration::from_secs(900));
        for i in 0..MAX_DEVICES + 5 {
            board.record(at(12, 0), telemetry(&format!("d{i}"), None), 600);
        }
        assert_eq!(board.snapshot(at(12, 1)).len(), MAX_DEVICES);
        // A known one still updates.
        board.record(at(12, 5), telemetry("d0", Some(BatteryState::Low)), 600);
        assert_eq!(
            board.snapshot(at(12, 6))[0].battery_state,
            Some(BatteryState::Low)
        );
    }

    #[test]
    fn what_a_display_was_last_sent_is_kept_per_display_and_survives_its_next_check_in() {
        let board = DeviceBoard::new(std::time::Duration::from_secs(900));
        board.record(at(12, 0), telemetry("a", None), 600);
        board.record(at(12, 0), telemetry("b", None), 600);
        assert!(board.snapshot(at(12, 1))[0].last_image.is_none());

        // Two displays fetching in the same minute are told apart by name, not by which asked last.
        board.image_served(&id("a"), ImageFormat::Qoi, 163_000, at(12, 2));
        board.image_served(&id("b"), ImageFormat::Bmp, 2_629_366, at(12, 2));
        let devices = board.snapshot(at(12, 5));
        let (a, b) = (
            devices[0].last_image.as_ref().unwrap(),
            devices[1].last_image.as_ref().unwrap(),
        );
        assert_eq!(
            (devices[0].name.as_str(), a.format, a.bytes, a.age_seconds),
            ("a", ImageFormat::Qoi, 163_000, 180)
        );
        assert_eq!(
            (devices[1].name.as_str(), b.format, b.bytes),
            ("b", ImageFormat::Bmp, 2_629_366)
        );

        // A later check-in replaces the readings, not what was sent.
        board.record(at(12, 10), telemetry("a", None), 600);
        let kept = board.snapshot(at(12, 11)).remove(0).last_image.unwrap();
        assert_eq!((kept.format, kept.bytes), (ImageFormat::Qoi, 163_000));
    }

    #[test]
    fn a_display_that_never_checked_in_is_not_added_by_fetching() {
        let board = DeviceBoard::new(std::time::Duration::from_secs(900));
        board.image_served(&id("stranger"), ImageFormat::Bmp, 1, at(12, 0));
        assert!(board.snapshot(at(12, 1)).is_empty());
    }
}
