//! `/metrics` in the Prometheus text format, so any scraper (Prometheus, Grafana Agent,
//! VictoriaMetrics, Telegraf) can graph and alert on the service and its displays without a
//! custom integration. Everything is derived from the same `Status` that `/status` serves, so the
//! two can't disagree.
//!
//! Conventions followed: a `home_display_` prefix, base units (seconds, volts), `_timestamp_seconds` for
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

/// How many TLS connections began with a full handshake and how many resumed a session. A display that
/// wakes every few minutes should mostly resume; a count that stays at full means tickets are not being used.
pub fn handshakes(out: &mut String, (full, resumed): (u64, u64)) {
    let _ = writeln!(
        out,
        "# HELP home_display_tls_handshakes_total TLS connections accepted since the service started, by how they began.\n\
         # TYPE home_display_tls_handshakes_total counter\n\
         home_display_tls_handshakes_total{{kind=\"full\"}} {full}\n\
         home_display_tls_handshakes_total{{kind=\"resumed\"}} {resumed}"
    );
}

pub fn render(status: &Status) -> String {
    let mut out = String::new();
    let gauge = |out: &mut String, name: &str, help: &str| {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge");
    };

    gauge(
        &mut out,
        "home_display_info",
        "The running version. Always 1.",
    );
    let _ = writeln!(
        out,
        "home_display_info{{version=\"{}\"}} 1",
        escape(status.version)
    );

    gauge(
        &mut out,
        "home_display_render_state",
        "1 for the service's current state, 0 for the others.",
    );
    for (state, name) in HEALTH_STATES {
        let _ = writeln!(
            out,
            "home_display_render_state{{state=\"{name}\"}} {}",
            u8::from(status.state == state)
        );
    }
    gauge(
        &mut out,
        "home_display_rendering",
        "1 while a render is running.",
    );
    let _ = writeln!(out, "home_display_rendering {}", u8::from(status.rendering));
    gauge(
        &mut out,
        "home_display_render_consecutive_failures",
        "Renders that failed in a row.",
    );
    let _ = writeln!(
        out,
        "home_display_render_consecutive_failures {}",
        status.consecutive_failures
    );
    gauge(
        &mut out,
        "home_display_uptime_seconds",
        "Seconds since the service started.",
    );
    let _ = writeln!(out, "home_display_uptime_seconds {}", status.uptime_seconds);
    if let Some(image) = &status.image {
        gauge(
            &mut out,
            "home_display_image_age_seconds",
            "Seconds since the served image was rendered.",
        );
        let _ = writeln!(out, "home_display_image_age_seconds {}", image.age_seconds);
    }
    if let Some(success) = &status.last_success {
        gauge(
            &mut out,
            "home_display_last_render_success_timestamp_seconds",
            "When a render last succeeded.",
        );
        let _ = writeln!(
            out,
            "home_display_last_render_success_timestamp_seconds {}",
            success.at.timestamp()
        );
    }

    if !status.sources.is_empty() {
        gauge(
            &mut out,
            "home_display_source_state",
            "1 for a source's state on the last render, 0 for the others.",
        );
        for source in &status.sources {
            let current = match source.state {
                SourceState::Fresh => "fresh",
                SourceState::Stale { .. } => "stale",
                SourceState::Unavailable { .. } => "unavailable",
            };
            for state in ["fresh", "stale", "unavailable"] {
                let _ = writeln!(
                    out,
                    "home_display_source_state{{source=\"{}\",state=\"{state}\"}} {}",
                    escape(&source.name),
                    u8::from(state == current)
                );
            }
        }
    }

    if !status.members.is_empty() {
        gauge(
            &mut out,
            "home_display_member_state",
            "1 for where the display stands in the certificate authority, 0 for the other states.",
        );
        for member in &status.members {
            let name = escape(&member.name);
            for state in ["pending", "approved", "member", "rejected", "revoked"] {
                let _ = writeln!(
                    out,
                    "home_display_member_state{{device=\"{name}\",state=\"{state}\"}} {}",
                    u8::from(member.state == state)
                );
            }
        }
        gauge(
            &mut out,
            "home_display_member_certificate_expiry_timestamp_seconds",
            "When the latest certificate a member was given ends. Alert on this being soon: a display renews with a third of its life left, so one that is close to the end has stopped renewing. Once past, it gets a new one by itself when switched on.",
        );
        for member in &status.members {
            if let Some(at) = member.certificate_not_after {
                let _ = writeln!(
                    out,
                    "home_display_member_certificate_expiry_timestamp_seconds{{device=\"{}\"}} {}",
                    escape(&member.name),
                    at.timestamp()
                );
            }
        }
        gauge(
            &mut out,
            "home_display_member_certificate_renewal_overdue",
            "1 if a member's certificate should have been renewed by now and has not been (about 63 days after it was issued, for a 90-day certificate). Alert on this: the display has stopped renewing, and the certificate will run out. 0 for the rest, and once it has run out.",
        );
        for member in &status.members {
            if member.certificate_not_after.is_some() {
                let _ = writeln!(
                    out,
                    "home_display_member_certificate_renewal_overdue{{device=\"{}\"}} {}",
                    escape(&member.name),
                    u8::from(member.renewal_overdue)
                );
            }
        }
        gauge(
            &mut out,
            "home_display_member_replacement_waiting",
            "1 if a different key is waiting for the owner to approve it taking the display's name.",
        );
        for member in &status.members {
            let _ = writeln!(
                out,
                "home_display_member_replacement_waiting{{device=\"{}\"}} {}",
                escape(&member.name),
                u8::from(member.replacement_waiting)
            );
        }
        gauge(
            &mut out,
            "home_display_member_changing_keys",
            "1 if the display has been given a certificate for a new key and has not used it yet.",
        );
        for member in &status.members {
            let _ = writeln!(
                out,
                "home_display_member_changing_keys{{device=\"{}\"}} {}",
                escape(&member.name),
                u8::from(member.changing_keys)
            );
        }
    }
    if !status.devices.is_empty() {
        gauge(
            &mut out,
            "home_display_device_last_seen_timestamp_seconds",
            "When the display last checked in.",
        );
        for device in &status.devices {
            let _ = writeln!(
                out,
                "home_display_device_last_seen_timestamp_seconds{{device=\"{}\"}} {}",
                escape(device.name.as_str()),
                device.last_seen.timestamp()
            );
        }
        gauge(
            &mut out,
            "home_display_device_overdue",
            "1 if the display is later than it was told to be.",
        );
        for device in &status.devices {
            let _ = writeln!(
                out,
                "home_display_device_overdue{{device=\"{}\"}} {}",
                escape(device.name.as_str()),
                u8::from(device.overdue)
            );
        }
        gauge(
            &mut out,
            "home_display_device_failed_wakes",
            "Wakes in a row that failed, for any reason.",
        );
        for device in &status.devices {
            if let Some(failed) = device.failed_wakes {
                let _ = writeln!(
                    out,
                    "home_display_device_failed_wakes{{device=\"{}\"}} {failed}",
                    escape(device.name.as_str())
                );
            }
        }
        gauge(
            &mut out,
            "home_display_device_wifi_rssi_dbm",
            "The display's Wi-Fi signal strength at its last check-in.",
        );
        for device in &status.devices {
            if let Some(dbm) = device.wifi_rssi_dbm {
                let _ = writeln!(
                    out,
                    "home_display_device_wifi_rssi_dbm{{device=\"{}\"}} {dbm}",
                    escape(device.name.as_str())
                );
            }
        }
        gauge(
            &mut out,
            "home_display_device_last_wake_seconds",
            "How long the display's previous wake was awake.",
        );
        for device in &status.devices {
            if let Some(seconds) = device.last_wake_seconds {
                let _ = writeln!(
                    out,
                    "home_display_device_last_wake_seconds{{device=\"{}\"}} {seconds}",
                    escape(device.name.as_str())
                );
            }
        }
        gauge(
            &mut out,
            "home_display_device_firmware_info",
            "The firmware version the display runs, as a label. Always 1.",
        );
        for device in &status.devices {
            if let Some(firmware) = &device.firmware {
                let _ = writeln!(
                    out,
                    "home_display_device_firmware_info{{device=\"{}\",firmware=\"{}\"}} 1",
                    escape(device.name.as_str()),
                    escape(firmware)
                );
            }
        }
        gauge(
            &mut out,
            "home_display_device_last_tls_seconds",
            "How long the display's first TLS handshake of its previous wake took, as the display timed it.",
        );
        for device in &status.devices {
            if let Some(ms) = device.last_tls_milliseconds {
                let _ = writeln!(
                    out,
                    "home_display_device_last_tls_seconds{{device=\"{}\"}} {}",
                    escape(device.name.as_str()),
                    f64::from(ms) / 1000.0
                );
            }
        }
        gauge(
            &mut out,
            "home_display_device_last_heap_min_bytes",
            "The least free heap the display had during its previous wake: how close the handshake came to running it out.",
        );
        for device in &status.devices {
            if let Some(bytes) = device.last_heap_min_bytes {
                let _ = writeln!(
                    out,
                    "home_display_device_last_heap_min_bytes{{device=\"{}\"}} {bytes}",
                    escape(device.name.as_str())
                );
            }
        }
        // The server's own view of the display's TLS connections (HTTPS only), which the display cannot misreport.
        let _ = writeln!(
            out,
            "# HELP home_display_device_tls_handshakes_total A display's TLS connections since the service started, by how they began.\n\
             # TYPE home_display_device_tls_handshakes_total counter"
        );
        for device in &status.devices {
            if let Some(connection) = &device.connection {
                let name = escape(device.name.as_str());
                let _ = writeln!(
                    out,
                    "home_display_device_tls_handshakes_total{{device=\"{name}\",kind=\"full\"}} {}\n\
                     home_display_device_tls_handshakes_total{{device=\"{name}\",kind=\"resumed\"}} {}",
                    connection.full_handshakes, connection.resumed_handshakes
                );
            }
        }
        gauge(
            &mut out,
            "home_display_device_tls_last_resumed",
            "1 if the display's latest TLS connection resumed a session, 0 if it began with a full handshake.",
        );
        for device in &status.devices {
            if let Some(connection) = &device.connection {
                let _ = writeln!(
                    out,
                    "home_display_device_tls_last_resumed{{device=\"{}\"}} {}",
                    escape(device.name.as_str()),
                    u8::from(connection.last_resumed)
                );
            }
        }
        gauge(
            &mut out,
            "home_display_device_tls_last_handshake_seconds",
            "How long the display's latest TLS handshake took, as the server saw it: from accepting the connection to its end, so it includes the display's own work and the network between.",
        );
        for device in &status.devices {
            if let Some(connection) = &device.connection {
                let _ = writeln!(
                    out,
                    "home_display_device_tls_last_handshake_seconds{{device=\"{}\"}} {}",
                    escape(device.name.as_str()),
                    f64::from(connection.last_handshake_milliseconds) / 1000.0
                );
            }
        }
        gauge(
            &mut out,
            "home_display_device_last_failure",
            "1 for why the display's last failed wake failed (none if it didn't), 0 for the others.",
        );
        for device in &status.devices {
            let name = escape(device.name.as_str());
            let _ = writeln!(
                out,
                "home_display_device_last_failure{{device=\"{name}\",reason=\"none\"}} {}",
                u8::from(device.last_failure.is_none())
            );
            for reason in FailureReason::ALL {
                let _ = writeln!(
                    out,
                    "home_display_device_last_failure{{device=\"{name}\",reason=\"{}\"}} {}",
                    reason.as_str(),
                    u8::from(device.last_failure == Some(reason))
                );
            }
        }
        gauge(
            &mut out,
            "home_display_device_battery_volts",
            "The display's battery voltage.",
        );
        for device in &status.devices {
            if let Some(mv) = device.battery_millivolts {
                let _ = writeln!(
                    out,
                    "home_display_device_battery_volts{{device=\"{}\"}} {}",
                    escape(device.name.as_str()),
                    f64::from(mv) / 1000.0
                );
            }
        }
        gauge(
            &mut out,
            "home_display_device_battery_ratio",
            "The display's battery charge, 0 to 1.",
        );
        for device in &status.devices {
            if let Some(pct) = device.battery_percent {
                let _ = writeln!(
                    out,
                    "home_display_device_battery_ratio{{device=\"{}\"}} {}",
                    escape(device.name.as_str()),
                    f64::from(pct) / 100.0
                );
            }
        }
        gauge(
            &mut out,
            "home_display_device_battery_state",
            "1 for the display's battery state, 0 for the others.",
        );
        for device in &status.devices {
            if let Some(current) = device.battery_state {
                for state in BatteryState::ALL {
                    let _ = writeln!(
                        out,
                        "home_display_device_battery_state{{device=\"{}\",state=\"{}\"}} {}",
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
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use chrono_tz::Europe::London;

    use super::*;
    use crate::application::devices::DeviceStatus;
    use crate::application::status::ImageStatus;
    use crate::application::status::MemberStatus;
    use crate::domain::models::render_report::SourceReport;

    #[test]
    fn handshakes_are_counted_by_how_they_began() {
        let mut out = String::new();
        handshakes(&mut out, (7, 42));
        assert_eq!(
            out,
            "# HELP home_display_tls_handshakes_total TLS connections accepted since the service started, by how they began.\n\
             # TYPE home_display_tls_handshakes_total counter\n\
             home_display_tls_handshakes_total{kind=\"full\"} 7\n\
             home_display_tls_handshakes_total{kind=\"resumed\"} 42\n"
        );
    }

    fn at() -> chrono::DateTime<chrono_tz::Tz> {
        London.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap()
    }

    fn status() -> Status {
        Status {
            state: Health::Degraded,
            rendering: false,
            image: Some(ImageStatus {
                rendered_at: at(),
                age_seconds: 90,
                version: 1,
            }),
            last_success: None,
            last_failure: None,
            consecutive_failures: 0,
            sources: vec![SourceReport {
                name: "TURNPIKE \"LANE\"".into(),
                state: SourceState::Stale { age_seconds: 300 },
            }],
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
                last_tls_milliseconds: Some(1100),
                last_heap_min_bytes: Some(61_440),
                firmware: Some("0.2.0".to_owned()),
                connection: Some(crate::application::devices::ConnectionStatus {
                    full_handshakes: 1,
                    resumed_handshakes: 4,
                    last_resumed: true,
                    last_handshake_milliseconds: 250,
                    last_at: at(),
                }),
                last_image: None,
            }],
            members: vec![
                MemberStatus {
                    name: "kitchen".to_owned(),
                    state: "member",
                    certificate_not_after: Some(
                        chrono_tz::Europe::London
                            .with_ymd_and_hms(2026, 12, 25, 12, 0, 0)
                            .unwrap(),
                    ),
                    certificate_expires_in_seconds: Some(172_800),
                    certificate_expired: false,
                    renewal_overdue: true,
                    changing_keys: true,
                    replacement_waiting: false,
                },
                MemberStatus {
                    name: "hall".to_owned(),
                    state: "pending",
                    certificate_not_after: None,
                    certificate_expires_in_seconds: None,
                    certificate_expired: false,
                    renewal_overdue: false,
                    changing_keys: false,
                    replacement_waiting: true,
                },
            ],
            next_render: None,
            schedule: Vec::new(),
            uptime_seconds: 3600,
            version: "1.2.3",
        }
    }

    #[test]
    fn exposes_where_each_display_stands_in_the_authority() {
        let text = render(&status());
        for line in [
            "home_display_member_state{device=\"kitchen\",state=\"member\"} 1",
            "home_display_member_state{device=\"kitchen\",state=\"pending\"} 0",
            "home_display_member_state{device=\"hall\",state=\"pending\"} 1",
            "home_display_member_state{device=\"hall\",state=\"member\"} 0",
            "home_display_member_certificate_expiry_timestamp_seconds{device=\"kitchen\"} 1798200000",
            "home_display_member_certificate_renewal_overdue{device=\"kitchen\"} 1",
            "home_display_member_replacement_waiting{device=\"hall\"} 1",
            "home_display_member_replacement_waiting{device=\"kitchen\"} 0",
            "home_display_member_changing_keys{device=\"kitchen\"} 1",
            "home_display_member_changing_keys{device=\"hall\"} 0",
        ] {
            assert!(
                text.lines().any(|l| l == line),
                "missing {line:?} in:\n{text}"
            );
        }
        // A display with no certificate has no expiry to report, not a zero that would alert.
        assert!(!text.contains("expiry_timestamp_seconds{device=\"hall\"}"));
        assert!(!text.contains("renewal_overdue{device=\"hall\"}"));
    }

    #[test]
    fn says_nothing_about_the_authority_when_there_are_no_members() {
        let mut plain = status();
        plain.members.clear();
        assert!(!render(&plain).contains("home_display_member"));
    }

    #[test]
    fn exposes_service_and_device_gauges() {
        let text = render(&status());
        for line in [
            "home_display_render_state{state=\"degraded\"} 1",
            "home_display_render_state{state=\"ok\"} 0",
            "home_display_image_age_seconds 90",
            "home_display_uptime_seconds 3600",
            "home_display_device_overdue{device=\"kitchen\"} 1",
            "home_display_device_failed_wakes{device=\"kitchen\"} 2",
            "home_display_device_wifi_rssi_dbm{device=\"kitchen\"} -71",
            "home_display_device_last_wake_seconds{device=\"kitchen\"} 24",
            "home_display_device_firmware_info{device=\"kitchen\",firmware=\"0.2.0\"} 1",
            "home_display_device_last_tls_seconds{device=\"kitchen\"} 1.1",
            "home_display_device_last_heap_min_bytes{device=\"kitchen\"} 61440",
            "home_display_device_tls_handshakes_total{device=\"kitchen\",kind=\"full\"} 1",
            "home_display_device_tls_handshakes_total{device=\"kitchen\",kind=\"resumed\"} 4",
            "home_display_device_tls_last_resumed{device=\"kitchen\"} 1",
            "home_display_device_tls_last_handshake_seconds{device=\"kitchen\"} 0.25",
            "home_display_device_last_failure{device=\"kitchen\",reason=\"download\"} 1",
            "home_display_device_last_failure{device=\"kitchen\",reason=\"wifi\"} 0",
            "home_display_device_last_failure{device=\"kitchen\",reason=\"none\"} 0",
            "home_display_device_battery_volts{device=\"kitchen\"} 3.712",
            "home_display_device_battery_ratio{device=\"kitchen\"} 0.47",
            "home_display_device_battery_state{device=\"kitchen\",state=\"low\"} 1",
            "home_display_device_battery_state{device=\"kitchen\",state=\"empty\"} 0",
            "home_display_source_state{source=\"TURNPIKE \\\"LANE\\\"\",state=\"stale\"} 1",
            "home_display_info{version=\"1.2.3\"} 1",
        ] {
            assert!(
                text.lines().any(|l| l == line),
                "missing {line:?} in:\n{text}"
            );
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
        assert!(!text.contains("home_display_image_age_seconds"));
        assert!(!text.contains("home_display_device"));
        assert!(!text.contains("home_display_source_state"));
    }
}
