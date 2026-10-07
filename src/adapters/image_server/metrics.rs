//! `/metrics` in the Prometheus text format, so any scraper (Prometheus, Grafana Agent,
//! VictoriaMetrics, Telegraf) can graph and alert on the service and its displays without a
//! custom integration. Everything is derived from the same `Status` that `/status` serves, so the
//! two can't disagree.
//!
//! Conventions followed: a `eink_` prefix, base units (seconds, volts), `_timestamp_seconds` for
//! a point in time, one gauge per state value (1 for the current state, 0 for the others) so an
//! alert is a plain comparison, and label values escaped.

use std::fmt::Write;

use crate::application::devices::{BatteryState, FailureReason};
use crate::application::status::{Health, Status};
use crate::domain::models::render_report::SourceState;

pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

const HEALTH_STATES: [(Health, &str); 5] = [
    (Health::Starting, "starting"),
    (Health::Ok, "ok"),
    (Health::Degraded, "degraded"),
    (Health::Failing, "failing"),
    (Health::Stale, "stale"),
];

pub fn render(status: &Status) -> String {
    let mut out = String::new();
    let gauge = |out: &mut String, name: &str, help: &str| {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge");
    };

    gauge(&mut out, "eink_info", "The running version. Always 1.");
    let _ = writeln!(out, "eink_info{{version=\"{}\"}} 1", escape(status.version));

    gauge(&mut out, "eink_render_state", "1 for the service's current state, 0 for the others.");
    for (state, name) in HEALTH_STATES {
        let _ = writeln!(out, "eink_render_state{{state=\"{name}\"}} {}", u8::from(status.state == state));
    }
    gauge(&mut out, "eink_rendering", "1 while a render is running.");
    let _ = writeln!(out, "eink_rendering {}", u8::from(status.rendering));
    gauge(&mut out, "eink_render_consecutive_failures", "Renders that failed in a row.");
    let _ = writeln!(out, "eink_render_consecutive_failures {}", status.consecutive_failures);
    gauge(&mut out, "eink_uptime_seconds", "Seconds since the service started.");
    let _ = writeln!(out, "eink_uptime_seconds {}", status.uptime_seconds);
    if let Some(image) = &status.image {
        gauge(&mut out, "eink_image_age_seconds", "Seconds since the served image was rendered.");
        let _ = writeln!(out, "eink_image_age_seconds {}", image.age_seconds);
    }
    if let Some(success) = &status.last_success {
        gauge(&mut out, "eink_last_render_success_timestamp_seconds", "When a render last succeeded.");
        let _ = writeln!(out, "eink_last_render_success_timestamp_seconds {}", success.at.timestamp());
    }

    if !status.sources.is_empty() {
        gauge(&mut out, "eink_source_state", "1 for a source's state on the last render, 0 for the others.");
        for source in &status.sources {
            let current = match source.state {
                SourceState::Fresh => "fresh",
                SourceState::Stale { .. } => "stale",
                SourceState::Unavailable { .. } => "unavailable",
            };
            for state in ["fresh", "stale", "unavailable"] {
                let _ = writeln!(
                    out,
                    "eink_source_state{{source=\"{}\",state=\"{state}\"}} {}",
                    escape(&source.name),
                    u8::from(state == current)
                );
            }
        }
    }

    if !status.devices.is_empty() {
        gauge(&mut out, "eink_device_last_seen_timestamp_seconds", "When the display last checked in.");
        for device in &status.devices {
            let _ = writeln!(
                out,
                "eink_device_last_seen_timestamp_seconds{{device=\"{}\"}} {}",
                escape(device.name.as_str()),
                device.last_seen.timestamp()
            );
        }
        gauge(&mut out, "eink_device_overdue", "1 if the display is later than it was told to be.");
        for device in &status.devices {
            let _ = writeln!(out, "eink_device_overdue{{device=\"{}\"}} {}", escape(device.name.as_str()), u8::from(device.overdue));
        }
        gauge(&mut out, "eink_device_failed_wakes", "Wakes in a row that failed, for any reason.");
        for device in &status.devices {
            if let Some(failed) = device.failed_wakes {
                let _ = writeln!(out, "eink_device_failed_wakes{{device=\"{}\"}} {failed}", escape(device.name.as_str()));
            }
        }
        gauge(&mut out, "eink_device_wifi_rssi_dbm", "The display's Wi-Fi signal strength at its last check-in.");
        for device in &status.devices {
            if let Some(dbm) = device.wifi_rssi_dbm {
                let _ = writeln!(out, "eink_device_wifi_rssi_dbm{{device=\"{}\"}} {dbm}", escape(device.name.as_str()));
            }
        }
        gauge(&mut out, "eink_device_last_wake_seconds", "How long the display's previous wake was awake.");
        for device in &status.devices {
            if let Some(seconds) = device.last_wake_seconds {
                let _ = writeln!(out, "eink_device_last_wake_seconds{{device=\"{}\"}} {seconds}", escape(device.name.as_str()));
            }
        }
        gauge(&mut out, "eink_device_last_failure", "1 for why the display's last failed wake failed (none if it didn't), 0 for the others.");
        for device in &status.devices {
            let name = escape(device.name.as_str());
            let _ = writeln!(out, "eink_device_last_failure{{device=\"{name}\",reason=\"none\"}} {}", u8::from(device.last_failure.is_none()));
            for reason in FailureReason::ALL {
                let _ = writeln!(
                    out,
                    "eink_device_last_failure{{device=\"{name}\",reason=\"{}\"}} {}",
                    reason.as_str(),
                    u8::from(device.last_failure == Some(reason))
                );
            }
        }
        gauge(&mut out, "eink_device_battery_volts", "The display's battery voltage.");
        for device in &status.devices {
            if let Some(mv) = device.battery_millivolts {
                let _ = writeln!(out, "eink_device_battery_volts{{device=\"{}\"}} {}", escape(device.name.as_str()), f64::from(mv) / 1000.0);
            }
        }
        gauge(&mut out, "eink_device_battery_ratio", "The display's battery charge, 0 to 1.");
        for device in &status.devices {
            if let Some(pct) = device.battery_percent {
                let _ = writeln!(out, "eink_device_battery_ratio{{device=\"{}\"}} {}", escape(device.name.as_str()), f64::from(pct) / 100.0);
            }
        }
        gauge(&mut out, "eink_device_battery_state", "1 for the display's battery state, 0 for the others.");
        for device in &status.devices {
            if let Some(current) = device.battery_state {
                for state in BatteryState::ALL {
                    let _ = writeln!(
                        out,
                        "eink_device_battery_state{{device=\"{}\",state=\"{}\"}} {}",
                        escape(device.name.as_str()),
                        state.as_str(),
                        u8::from(state == current)
                    );
                }
            }
        }
    }
    out
}

/// A label value: backslash, quote and newline escaped, as the exposition format requires.
fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeZone};

    use super::*;
    use crate::application::devices::DeviceStatus;
    use crate::application::status::ImageStatus;
    use crate::domain::models::render_report::SourceReport;

    fn at() -> chrono::DateTime<Local> {
        Local.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap()
    }

    fn status() -> Status {
        Status {
            state: Health::Degraded,
            rendering: false,
            image: Some(ImageStatus { rendered_at: at(), age_seconds: 90, version: 1 }),
            last_success: None,
            last_failure: None,
            consecutive_failures: 0,
            sources: vec![SourceReport { name: "TURNPIKE \"LANE\"".into(), state: SourceState::Stale { age_seconds: 300 } }],
            devices: vec![DeviceStatus {
                name: "kitchen".parse().unwrap(),
                last_seen: at(),
                age_seconds: 5,
                expected_by: at(),
                overdue: true,
                battery_millivolts: Some(3712),
                battery_percent: Some(47),
                battery_state: Some(BatteryState::Low),
                failed_wakes: Some(2),
                wifi_rssi_dbm: Some(-71),
                last_failure: Some(FailureReason::Download),
                last_wake_seconds: Some(24),
                last_image: None,
            }],
            next_render: None,
            schedule: Vec::new(),
            uptime_seconds: 3600,
            version: "1.2.3",
        }
    }

    #[test]
    fn exposes_service_and_device_gauges() {
        let text = render(&status());
        for line in [
            "eink_render_state{state=\"degraded\"} 1",
            "eink_render_state{state=\"ok\"} 0",
            "eink_image_age_seconds 90",
            "eink_uptime_seconds 3600",
            "eink_device_overdue{device=\"kitchen\"} 1",
            "eink_device_failed_wakes{device=\"kitchen\"} 2",
            "eink_device_wifi_rssi_dbm{device=\"kitchen\"} -71",
            "eink_device_last_wake_seconds{device=\"kitchen\"} 24",
            "eink_device_last_failure{device=\"kitchen\",reason=\"download\"} 1",
            "eink_device_last_failure{device=\"kitchen\",reason=\"wifi\"} 0",
            "eink_device_last_failure{device=\"kitchen\",reason=\"none\"} 0",
            "eink_device_battery_volts{device=\"kitchen\"} 3.712",
            "eink_device_battery_ratio{device=\"kitchen\"} 0.47",
            "eink_device_battery_state{device=\"kitchen\",state=\"low\"} 1",
            "eink_device_battery_state{device=\"kitchen\",state=\"empty\"} 0",
            "eink_source_state{source=\"TURNPIKE \\\"LANE\\\"\",state=\"stale\"} 1",
            "eink_info{version=\"1.2.3\"} 1",
        ] {
            assert!(text.lines().any(|l| l == line), "missing {line:?} in:\n{text}");
        }
    }

    #[test]
    fn every_series_has_help_and_type_before_it() {
        let text = render(&status());
        let mut declared = std::collections::HashSet::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                declared.insert(rest.split(' ').next().unwrap().to_owned());
            } else if !line.starts_with('#') {
                let name = line.split(['{', ' ']).next().unwrap();
                assert!(declared.contains(name), "{name} has no TYPE line before it");
            }
        }
    }

    #[test]
    fn omits_what_is_not_known() {
        let mut bare = status();
        bare.image = None;
        bare.devices.clear();
        bare.sources.clear();
        let text = render(&bare);
        assert!(!text.contains("eink_image_age_seconds"));
        assert!(!text.contains("eink_device"));
        assert!(!text.contains("eink_source_state"));
    }
}
