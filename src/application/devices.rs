//! What the displays say about themselves, for `/status` and `/metrics`.
//!
//! A display reports on each check-in, as query parameters on the `/plan` or `/refresh` call it
//! makes anyway (so it costs no extra radio time): its name, battery, and how many wakes in a row
//! failed. The server also knows when that display was told to come back, so it can tell a
//! display that is merely asleep from one that has gone quiet (a flat battery, a dead Wi-Fi
//! network): it is overdue once it is later than that, plus a grace period.
//!
//! Everything here comes from the network, so it is bounded and checked: a few devices, short
//! names of plain characters, and readings in a plausible range. A bad value is dropped, never
//! an error, because the display must still get its plan.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Duration, Local};
use serde::Serialize;

/// More than a household has; stops a stray client filling memory with made-up names.
const MAX_DEVICES: usize = 16;
const MAX_NAME_LEN: usize = 32;
/// A Li-ion cell is never outside this, so anything else is a misread.
const MILLIVOLTS: std::ops::RangeInclusive<u32> = 2000..=5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
}

impl FailureReason {
    pub const ALL: [FailureReason; 5] = [Self::Wifi, Self::Server, Self::Download, Self::Memory, Self::Timeout];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Server => "server",
            Self::Download => "download",
            Self::Memory => "memory",
            Self::Timeout => "timeout",
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

/// The query parameters as received. All text, so a malformed one can't fail the request.
#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct RawTelemetry {
    pub device: Option<String>,
    pub battery_mv: Option<String>,
    pub battery_pct: Option<String>,
    pub battery_state: Option<String>,
    pub failed_wakes: Option<String>,
    pub rssi: Option<String>,
    pub last_failure: Option<String>,
    pub last_wake_s: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Telemetry {
    pub device: String,
    pub battery_millivolts: Option<u32>,
    pub battery_percent: Option<u8>,
    pub battery_state: Option<BatteryState>,
    pub failed_wakes: Option<u32>,
    pub wifi_rssi_dbm: Option<i16>,
    pub last_failure: Option<FailureReason>,
    pub last_wake_seconds: Option<u32>,
}

impl RawTelemetry {
    /// None unless a usable device name came with it: without one there is nobody to report on.
    pub fn parse(&self) -> Option<Telemetry> {
        let device = self.device.as_deref()?.trim();
        let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
        if device.is_empty() || device.len() > MAX_NAME_LEN || !device.chars().all(plain) {
            return None;
        }
        let number = |text: &Option<String>| text.as_deref().and_then(|t| t.parse::<u32>().ok());
        Some(Telemetry {
            device: device.to_owned(),
            battery_millivolts: number(&self.battery_mv).filter(|mv| MILLIVOLTS.contains(mv)),
            battery_percent: number(&self.battery_pct).filter(|pct| *pct <= 100).map(|pct| pct as u8),
            battery_state: self.battery_state.as_deref().and_then(BatteryState::parse),
            failed_wakes: number(&self.failed_wakes),
            wifi_rssi_dbm: self.rssi.as_deref().and_then(|t| t.parse::<i16>().ok()).filter(|dbm| RSSI_DBM.contains(dbm)),
            last_failure: self.last_failure.as_deref().and_then(FailureReason::parse),
            last_wake_seconds: number(&self.last_wake_s).filter(|seconds| WAKE_SECONDS.contains(seconds)),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceStatus {
    pub name: String,
    pub last_seen: DateTime<Local>,
    pub age_seconds: u64,
    /// When the display was told to come back.
    pub expected_by: DateTime<Local>,
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
}

struct Record {
    last_seen: DateTime<Local>,
    expected_by: DateTime<Local>,
    telemetry: Telemetry,
}

pub struct DeviceBoard {
    overdue_grace: Duration,
    devices: Mutex<BTreeMap<String, Record>>,
}

impl DeviceBoard {
    pub fn new(overdue_grace: std::time::Duration) -> Arc<Self> {
        Arc::new(Self {
            overdue_grace: Duration::from_std(overdue_grace).unwrap_or(Duration::MAX),
            devices: Mutex::default(),
        })
    }

    fn devices(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Record>> {
        self.devices.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Notes a check-in at `now`, from a display just told to come back in `next_seconds`.
    /// Logs what an operator would want to hear about: a battery getting worse, or a display
    /// that returns after going quiet.
    pub fn record(&self, now: DateTime<Local>, telemetry: Telemetry, next_seconds: u64) {
        let expected_by = Duration::try_seconds(next_seconds.try_into().unwrap_or(i64::MAX))
            .and_then(|wait| now.checked_add_signed(wait))
            .unwrap_or(now);
        let mut devices = self.devices();
        if devices.len() >= MAX_DEVICES && !devices.contains_key(&telemetry.device) {
            log::warn!("Ignoring {:?}: already tracking {MAX_DEVICES} devices", telemetry.device);
            return;
        }
        let previous = devices.get(&telemetry.device);
        match previous {
            None => {
                log::info!("New display {:?} checked in{}", telemetry.device, describe_battery(&telemetry));
                if let Some(reason) = telemetry.last_failure {
                    log::warn!("Display {:?} {}", telemetry.device, describe_failure(reason, telemetry.failed_wakes));
                }
            }
            Some(before) => {
                if before.telemetry.last_failure != telemetry.last_failure {
                    match telemetry.last_failure {
                        Some(reason) => log::warn!("Display {:?} {}", telemetry.device, describe_failure(reason, telemetry.failed_wakes)),
                        None => log::info!("Display {:?} is working again", telemetry.device),
                    }
                }
                if before.telemetry.battery_state != telemetry.battery_state {
                    match telemetry.battery_state {
                        Some(BatteryState::Low | BatteryState::Empty) => log::warn!(
                            "Display {:?} battery is {}{}",
                            telemetry.device,
                            telemetry.battery_state.map_or("unknown", BatteryState::as_str),
                            describe_battery(&telemetry)
                        ),
                        _ => log::info!("Display {:?} battery is back to normal{}", telemetry.device, describe_battery(&telemetry)),
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
        devices.insert(telemetry.device.clone(), Record { last_seen: now, expected_by, telemetry });
    }

    pub fn snapshot(&self, now: DateTime<Local>) -> Vec<DeviceStatus> {
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
            })
            .collect()
    }
}

fn describe_failure(reason: FailureReason, failed_wakes: Option<u32>) -> String {
    match failed_wakes {
        Some(count) if count > 0 => format!("had {count} failed wake(s) in a row, the last because of {}", reason.as_str()),
        _ => format!("reports its last wake failed because of {}", reason.as_str()),
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

    use super::*;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 6, 15, h, m, 0).unwrap()
    }

    fn raw(pairs: &[(&str, &str)]) -> RawTelemetry {
        let get = |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string());
        RawTelemetry {
            device: get("device"),
            battery_mv: get("battery_mv"),
            battery_pct: get("battery_pct"),
            battery_state: get("battery_state"),
            failed_wakes: get("failed_wakes"),
            rssi: get("rssi"),
            last_failure: get("last_failure"),
            last_wake_s: get("last_wake_s"),
        }
    }

    fn telemetry(name: &str, state: Option<BatteryState>) -> Telemetry {
        Telemetry {
            device: name.into(),
            battery_millivolts: Some(3700),
            battery_percent: Some(45),
            battery_state: state,
            failed_wakes: Some(0),
            wifi_rssi_dbm: Some(-60),
            last_failure: None,
            last_wake_seconds: Some(21),
        }
    }

    #[test]
    fn parses_good_telemetry() {
        let parsed = raw(&[
            ("device", "reterminal-e1003"),
            ("battery_mv", "3712"),
            ("battery_pct", "47"),
            ("battery_state", "low"),
            ("failed_wakes", "2"),
            ("rssi", "-67"),
            ("last_failure", "download"),
            ("last_wake_s", "24"),
        ])
        .parse()
        .unwrap();
        assert_eq!(parsed.wifi_rssi_dbm, Some(-67));
        assert_eq!(parsed.last_failure, Some(FailureReason::Download));
        assert_eq!(parsed.last_wake_seconds, Some(24));
        assert_eq!(parsed.device, "reterminal-e1003");
        assert_eq!(parsed.battery_millivolts, Some(3712));
        assert_eq!(parsed.battery_percent, Some(47));
        assert_eq!(parsed.battery_state, Some(BatteryState::Low));
        assert_eq!(parsed.failed_wakes, Some(2));
    }

    #[test]
    fn drops_bad_values_but_keeps_the_rest() {
        let parsed = raw(&[
            ("device", "kitchen"),
            ("battery_mv", "999999"),
            ("battery_pct", "250"),
            ("battery_state", "on fire"),
            ("failed_wakes", "many"),
            ("rssi", "0"),
            ("last_failure", "gremlins"),
            ("last_wake_s", "99999"),
        ])
        .parse()
        .unwrap();
        assert_eq!((parsed.wifi_rssi_dbm, parsed.last_failure, parsed.last_wake_seconds), (None, None, None));
        assert_eq!(parsed.battery_millivolts, None);
        assert_eq!(parsed.battery_percent, None);
        assert_eq!(parsed.battery_state, None);
        assert_eq!(parsed.failed_wakes, None);
    }

    #[test]
    fn every_failure_reason_the_display_can_name_is_understood() {
        for reason in FailureReason::ALL {
            let parsed = raw(&[("device", "a"), ("last_failure", reason.as_str())]).parse().unwrap();
            assert_eq!(parsed.last_failure, Some(reason));
        }
        // The edges of what a signal and a wake can be.
        for (rssi, ok) in [("-127", true), ("-1", true), ("0", false), ("-128", false), ("12", false), ("weak", false)] {
            let parsed = raw(&[("device", "a"), ("rssi", rssi)]).parse().unwrap();
            assert_eq!(parsed.wifi_rssi_dbm.is_some(), ok, "{rssi}");
        }
        for (seconds, ok) in [("0", true), ("600", true), ("601", false), ("-3", false)] {
            let parsed = raw(&[("device", "a"), ("last_wake_s", seconds)]).parse().unwrap();
            assert_eq!(parsed.last_wake_seconds.is_some(), ok, "{seconds}");
        }
    }

    #[test]
    fn the_latest_reason_and_signal_are_kept_and_shown() {
        let board = DeviceBoard::new(std::time::Duration::from_secs(900));
        board.record(at(12, 0), Telemetry { last_failure: Some(FailureReason::Server), failed_wakes: Some(3), ..telemetry("a", None) }, 600);
        let device = &board.snapshot(at(12, 1))[0];
        assert_eq!((device.last_failure, device.wifi_rssi_dbm, device.last_wake_seconds), (Some(FailureReason::Server), Some(-60), Some(21)));

        board.record(at(12, 10), telemetry("a", None), 600);
        assert_eq!(board.snapshot(at(12, 11))[0].last_failure, None);
    }

    #[test]
    fn needs_a_plain_short_device_name() {
        assert!(raw(&[("battery_mv", "3700")]).parse().is_none());
        for bad in ["", "  ", "has space", "quote\"d", "new\nline", &"x".repeat(33), "emoji🔋"] {
            assert!(raw(&[("device", bad)]).parse().is_none(), "{bad:?}");
        }
        assert!(raw(&[("device", &"x".repeat(32))]).parse().is_some());
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
        board.record(at(12, 10), Telemetry { battery_millivolts: Some(3350), ..telemetry("a", Some(BatteryState::Low)) }, 600);
        board.record(at(12, 10), telemetry("b", None), 600);

        let devices = board.snapshot(at(12, 11));
        assert_eq!(devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
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
        assert_eq!(board.snapshot(at(12, 6))[0].battery_state, Some(BatteryState::Low));
    }
}
