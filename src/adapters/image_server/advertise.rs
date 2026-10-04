//! Announces the image server over mDNS / DNS-SD (RFC 6762, RFC 6763), so a scan of the
//! network finds it and says where it is.
//!
//! It is published as an ordinary `_http._tcp` service, which every service browser
//! lists and can open, with the `path` TXT key naming the page. The `_eink-display`
//! subtype lets a scan ask for just this server.

use std::net::IpAddr;
use std::time::Duration;

use anyhow::{bail, Context};
use mdns_sd::{DaemonEvent, IfKind, ServiceDaemon, ServiceInfo};

use crate::config::server::ServerConfig;
use crate::domain::models::display::ImageFormat;

const SERVICE_SUBTYPE: &str = "_eink-display._sub._http._tcp.local.";
/// Instance names, like any DNS label, are at most 63 bytes (RFC 6763 section 4.1.1).
const MAX_LABEL_BYTES: usize = 63;
const GOODBYE_TIMEOUT: Duration = Duration::from_secs(1);

/// A registered service. Dropping it withdraws the service with a goodbye (a record with a
/// zero TTL, RFC 6762 section 8.4), so scans stop listing it at once instead of after the TTL.
pub struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertisement {
    pub fn start(config: &ServerConfig, port: u16, format: ImageFormat) -> anyhow::Result<Self> {
        let info = service_info(config, port, format)?;
        let fullname = info.get_fullname().to_owned();
        let daemon = ServiceDaemon::new().context("Failed to start the mDNS responder")?;
        let this = Self { daemon, fullname };

        let events = this.daemon.monitor().context("Failed to watch the mDNS responder")?;
        tokio::spawn(async move {
            while let Ok(event) = events.recv_async().await {
                match event {
                    // The responder probes first and renames itself if the name is taken.
                    DaemonEvent::NameChange(change) => {
                        log::warn!("mDNS name {} is taken, using {}", change.original, change.new_name)
                    }
                    DaemonEvent::Error(e) => log::warn!("mDNS error: {e}"),
                    other => log::debug!("mDNS: {other:?}"),
                }
            }
        });

        this.daemon.register(info).context("Failed to register the mDNS service")?;
        log::info!("Advertising {} over mDNS", this.fullname);
        Ok(this)
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        // Best effort: the process is ending, and the records expire on their own anyway.
        if let Ok(done) = self.daemon.unregister(&self.fullname) {
            let _ = done.recv_timeout(GOODBYE_TIMEOUT);
        }
        if let Ok(done) = self.daemon.shutdown() {
            let _ = done.recv_timeout(GOODBYE_TIMEOUT);
        }
    }
}

fn service_info(config: &ServerConfig, port: u16, format: ImageFormat) -> anyhow::Result<ServiceInfo> {
    let name = config.instance_name.trim();
    if name.is_empty() || name.len() > MAX_LABEL_BYTES {
        bail!("server.instance_name must be 1 to {MAX_LABEL_BYTES} bytes, not {:?}", config.instance_name);
    }
    let host = format!("{}.local.", host_label(name));
    // txtvers first, as RFC 6763 section 6.7 recommends; keys are kept short and lowercase.
    let txt = [
        ("txtvers", "1"),
        ("path", "/image"),
        ("format", format.extension()),
        ("version", env!("CARGO_PKG_VERSION")),
    ];

    let ip = config.bind.ip();
    let mut info = if ip.is_unspecified() {
        // Listening everywhere, so announce every address the host has, and follow it when they change.
        ServiceInfo::new(SERVICE_SUBTYPE, name, &host, "", port, &txt[..])?.enable_addr_auto()
    } else {
        ServiceInfo::new(SERVICE_SUBTYPE, name, &host, ip, port, &txt[..])?
    };
    // An IPv4 bind isn't reachable over IPv6, so don't announce AAAA records that would mislead.
    if matches!(ip, IpAddr::V4(_)) {
        info.set_interfaces(vec![IfKind::IPv4]);
    }
    Ok(info)
}

/// A DNS host label from a display name: lowercase letters, digits and single hyphens.
fn host_label(name: &str) -> String {
    let label: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if label.is_empty() {
        return "eink-display".to_owned();
    }
    label.chars().take(MAX_LABEL_BYTES).collect::<String>().trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use mdns_sd::ServiceEvent;

    use super::*;

    #[test]
    fn host_labels_are_valid_dns_labels() {
        assert_eq!(host_label("E-ink home display"), "e-ink-home-display");
        assert_eq!(host_label("  Living room!! "), "living-room");
        assert_eq!(host_label("日本語"), "eink-display");
        assert_eq!(host_label(&"a".repeat(100)).len(), 63);
    }

    #[test]
    fn describes_an_http_service_with_a_subtype_and_a_path() {
        let config = ServerConfig::default();
        let info = service_info(&config, 8080, ImageFormat::Png).unwrap();

        assert_eq!(info.get_fullname(), "E-ink home display._http._tcp.local.");
        assert_eq!(info.get_type(), "_http._tcp.local.");
        assert_eq!(info.get_subtype().as_deref(), Some(SERVICE_SUBTYPE));
        assert_eq!(info.get_hostname(), "e-ink-home-display.local.");
        assert_eq!(info.get_port(), 8080);
        assert!(info.is_addr_auto());
        assert_eq!(info.get_property_val_str("txtvers"), Some("1"));
        assert_eq!(info.get_property_val_str("path"), Some("/image"));
        assert_eq!(info.get_property_val_str("format"), Some("png"));
    }

    #[test]
    fn a_specific_bind_address_is_announced_as_is() {
        let config = ServerConfig { bind: "192.168.1.5:9000".parse().unwrap(), ..Default::default() };
        let info = service_info(&config, 9000, ImageFormat::Bmp).unwrap();

        assert!(!info.is_addr_auto());
        assert!(info.get_addresses().contains(&"192.168.1.5".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn rejects_unusable_instance_names() {
        for name in ["", "   ", &"x".repeat(64)] {
            let config = ServerConfig { instance_name: name.to_owned(), ..Default::default() };
            assert!(service_info(&config, 80, ImageFormat::Bmp).is_err(), "{name:?}");
        }
    }

    /// Needs working multicast on this machine, so it is not part of the normal run:
    /// `cargo test advertised_service_is_found_by_a_scan -- --ignored`
    #[tokio::test]
    #[ignore = "needs multicast networking"]
    async fn advertised_service_is_found_by_a_scan() {
        let config = ServerConfig { instance_name: "Eink test".to_owned(), ..Default::default() };
        let advertisement = Advertisement::start(&config, 18080, ImageFormat::Bmp).unwrap();

        let scanner = ServiceDaemon::new().unwrap();
        let found = scanner.browse("_http._tcp.local.").unwrap();
        let resolved = tokio::time::timeout(Duration::from_secs(15), async {
            while let Ok(event) = found.recv_async().await {
                if let ServiceEvent::ServiceResolved(service) = event {
                    if service.get_fullname().starts_with("Eink test.") {
                        return Some(service);
                    }
                }
            }
            None
        })
        .await
        .expect("timed out waiting for the service")
        .expect("scan ended without finding it");

        assert_eq!(resolved.get_port(), 18080);
        assert_eq!(resolved.get_property_val_str("path"), Some("/image"));
        assert!(!resolved.get_addresses().is_empty());

        drop(advertisement);
        let _ = scanner.shutdown();
    }
}
