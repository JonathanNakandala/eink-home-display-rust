//! Announces the image server over mDNS / DNS-SD (RFC 6762, RFC 6763), so a scan of the
//! network finds it and says where it is.
//!
//! It is published as an ordinary `_http._tcp` service, which every service browser
//! lists and can open, with the `path` TXT key naming the page. The `_eink-display`
//! subtype lets a scan ask for just this server.
//!
//! When the server speaks HTTPS and nothing else, it is `_https._tcp` instead (RFC 6763 and the
//! IANA service registry), on the HTTPS port. Announcing a TLS-only port as `_http._tcp` would send a
//! browser, or any client that goes by the service type, to speak plain HTTP to it.

use std::time::Duration;

use anyhow::{Context, bail};
use mdns_sd::{DaemonEvent, IfKind, IfPredicate, ServiceDaemon, ServiceInfo};

use super::ServerSettings;
use crate::adapters::listen::Families;
use crate::domain::models::display::ImageFormat;

const SERVICE_SUBTYPE: &str = "_eink-display._sub._http._tcp.local.";
const SECURE_SERVICE_SUBTYPE: &str = "_eink-display._sub._https._tcp.local.";
/// Instance names, like any DNS label, are at most 63 bytes (RFC 6763 section 4.1.1).
const MAX_LABEL_BYTES: usize = 63;
const GOODBYE_TIMEOUT: Duration = Duration::from_secs(1);

/// The HTTPS the server offers, announced beside the service so a display finds it without being told.
/// `tlsport` is where; `secure` says whether plain HTTP is served too (`optional`) or not (`required`).
/// A display that has joined uses HTTPS when it is offered, and one set to HTTPS-only uses nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecureOffer {
    pub port: u16,
    pub required: bool,
}

impl SecureOffer {
    /// The service type to announce under: `_https._tcp` when plain HTTP is not served, else `_http._tcp`.
    fn subtype(offer: Option<Self>) -> &'static str {
        match offer {
            Some(Self { required: true, .. }) => SECURE_SERVICE_SUBTYPE,
            _ => SERVICE_SUBTYPE,
        }
    }
}

/// A registered service. Dropping it withdraws the service with a goodbye (a record with a
/// zero TTL, RFC 6762 section 8.4), so scans stop listing it at once instead of after the TTL.
pub struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertisement {
    pub fn start(
        settings: &ServerSettings,
        port: u16,
        format: ImageFormat,
        families: Families,
        secure: Option<SecureOffer>,
    ) -> anyhow::Result<Self> {
        let info = service_info(settings, port, format, families, secure)?;
        let fullname = info.get_fullname().to_owned();
        let daemon = ServiceDaemon::new().context("Failed to start the mDNS responder")?;
        // Nothing to announce on loopback, and no use listening for an IP version that isn't served.
        let loopback =
            IfPredicate::new(|intf| intf.is_loopback() || is_loopback_interface(&intf.name));
        let mut unused = vec![IfKind::Predicate(loopback)];
        match families {
            Families::V4 => unused.push(IfKind::IPv6),
            Families::V6 => unused.push(IfKind::IPv4),
            Families::Both => {}
        }
        if let Err(e) = daemon.disable_interface(unused) {
            log::warn!("Could not narrow the mDNS interfaces: {e}");
        }
        let this = Self { daemon, fullname };

        let events = this
            .daemon
            .monitor()
            .context("Failed to watch the mDNS responder")?;
        tokio::spawn(async move {
            while let Ok(event) = events.recv_async().await {
                match event {
                    // The responder probes first and renames itself if the name is taken.
                    DaemonEvent::NameChange(change) => {
                        log::warn!(
                            "mDNS name {} is taken, using {}",
                            change.original,
                            change.new_name
                        )
                    }
                    DaemonEvent::Error(e) => log::warn!("mDNS error: {e}"),
                    other => log::debug!("mDNS: {other:?}"),
                }
            }
        });

        this.daemon
            .register(info)
            .context("Failed to register the mDNS service")?;
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

/// `families` is what the server really accepts: only those get address records, so a client is never
/// pointed at an address it can't use. (The responder sends no "no such record" answer, so a client
/// asking for the other family's address has to wait out a timeout; announcing both is better when both work.)
fn service_info(
    settings: &ServerSettings,
    port: u16,
    format: ImageFormat,
    families: Families,
    secure: Option<SecureOffer>,
) -> anyhow::Result<ServiceInfo> {
    let name = settings.instance_name.trim();
    if name.is_empty() || name.len() > MAX_LABEL_BYTES {
        bail!(
            "the instance name must be 1 to {MAX_LABEL_BYTES} bytes, not {:?}",
            settings.instance_name
        );
    }
    let host = format!("{}.local.", host_label(name));
    // txtvers first, as RFC 6763 section 6.7 recommends; keys are kept short and lowercase.
    // `format` is what is served now, and `formats` everything it can serve (set by display.image_format),
    // for whoever is looking at a scan. The display doesn't use either: it decodes by Content-Type.
    let formats = ImageFormat::ALL.map(ImageFormat::extension).join(",");
    let tls_port = secure.map(|offer| offer.port.to_string());
    let mut txt = vec![
        ("txtvers", "1"),
        ("path", "/image"),
        ("format", format.extension()),
        ("formats", formats.as_str()),
        ("version", env!("CARGO_PKG_VERSION")),
    ];
    if let (Some(offer), Some(port)) = (secure, tls_port.as_deref()) {
        txt.push(("tlsport", port));
        txt.push((
            "secure",
            if offer.required {
                "required"
            } else {
                "optional"
            },
        ));
    }

    let service_type = SecureOffer::subtype(secure);
    let ip = settings.bind.ip();
    let mut info = if ip.is_unspecified() {
        // Listening everywhere, so announce every address the host has, and follow it when they change.
        ServiceInfo::new(service_type, name, &host, "", port, &txt[..])?.enable_addr_auto()
    } else {
        ServiceInfo::new(service_type, name, &host, ip, port, &txt[..])?
    };
    // Only the interfaces a client on the network can be on: not loopback, and only the IP versions served.
    info.set_interfaces(vec![IfKind::Predicate(IfPredicate::new(move |intf| {
        !intf.is_loopback()
            && !is_loopback_interface(&intf.name)
            && match families {
                Families::Both => true,
                Families::V4 => intf.ip().is_ipv4(),
                Families::V6 => intf.ip().is_ipv6(),
            }
    }))]);
    Ok(info)
}

/// The loopback interface by name (`lo`, `lo0`). It carries link-local IPv6 addresses (`fe80::1`) that
/// aren't loopback addresses, and the responder would announce them on the real interface too.
fn is_loopback_interface(name: &str) -> bool {
    name.strip_prefix("lo")
        .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
}

/// The host name the server announces for itself (`<label>.local`), which is also what a display
/// reaches it by and what its certificate has to be for.
pub fn mdns_host_name(instance_name: &str) -> String {
    format!("{}.local", host_label(instance_name.trim()))
}

/// A DNS host label from a display name: lowercase letters, digits and single hyphens.
fn host_label(name: &str) -> String {
    let label: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if label.is_empty() {
        return "eink-display".to_owned();
    }
    label
        .chars()
        .take(MAX_LABEL_BYTES)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use mdns_sd::ServiceEvent;

    use super::*;
    use crate::application::plan::PlanTiming;

    fn settings() -> ServerSettings {
        ServerSettings {
            bind: "[::]:8080".parse().unwrap(),
            advertise: true,
            instance_name: "E-ink home display".to_owned(),
            timing: PlanTiming {
                wake_delay: Duration::from_secs(30),
                stale_grace: Duration::from_secs(300),
            },
        }
    }

    #[test]
    fn the_loopback_interface_is_recognised_by_name() {
        for name in ["lo", "lo0", "lo1"] {
            assert!(is_loopback_interface(name), "{name}");
        }
        for name in ["en0", "eth0", "wlan0", "lowpan0", "docker0", "br-lo"] {
            assert!(!is_loopback_interface(name), "{name}");
        }
    }

    #[test]
    fn host_labels_are_valid_dns_labels() {
        assert_eq!(host_label("E-ink home display"), "e-ink-home-display");
        assert_eq!(host_label("  Living room!! "), "living-room");
        assert_eq!(host_label("日本語"), "eink-display");
        assert_eq!(host_label(&"a".repeat(100)).len(), 63);
    }

    #[test]
    fn describes_an_http_service_with_a_subtype_and_a_path() {
        let config = settings();
        let info = service_info(&config, 8080, ImageFormat::Png, Families::Both, None).unwrap();

        assert_eq!(info.get_fullname(), "E-ink home display._http._tcp.local.");
        assert_eq!(info.get_type(), "_http._tcp.local.");
        assert_eq!(info.get_subtype().as_deref(), Some(SERVICE_SUBTYPE));
        assert_eq!(info.get_hostname(), "e-ink-home-display.local.");
        assert_eq!(info.get_port(), 8080);
        assert!(info.is_addr_auto());
        assert_eq!(info.get_property_val_str("txtvers"), Some("1"));
        assert_eq!(info.get_property_val_str("path"), Some("/image"));
        assert_eq!(info.get_property_val_str("format"), Some("png"));
        assert_eq!(info.get_property_val_str("formats"), Some("bmp,png,qoi"));
    }

    #[test]
    fn plain_http_announces_no_https() {
        let info = service_info(&settings(), 8080, ImageFormat::Png, Families::Both, None).unwrap();
        assert_eq!(info.get_property_val_str("tlsport"), None);
        assert_eq!(info.get_property_val_str("secure"), None);
    }

    #[test]
    fn offering_https_beside_http_says_where_and_that_http_is_still_there() {
        let offer = SecureOffer {
            port: 8443,
            required: false,
        };
        let info = service_info(
            &settings(),
            8080,
            ImageFormat::Png,
            Families::Both,
            Some(offer),
        )
        .unwrap();
        assert_eq!(info.get_port(), 8080);
        assert_eq!(info.get_property_val_str("tlsport"), Some("8443"));
        assert_eq!(info.get_property_val_str("secure"), Some("optional"));
        // What was announced before is unchanged.
        assert_eq!(info.get_property_val_str("path"), Some("/image"));
        assert_eq!(info.get_property_val_str("txtvers"), Some("1"));
    }

    #[test]
    fn plain_and_both_are_announced_as_http_and_https_only_as_https() {
        let kinds = |offer| {
            let info =
                service_info(&settings(), 8443, ImageFormat::Png, Families::Both, offer).unwrap();
            (
                info.get_type().to_owned(),
                info.get_subtype().clone().unwrap(),
                info.get_fullname().to_owned(),
            )
        };
        for offer in [
            None,
            Some(SecureOffer {
                port: 8443,
                required: false,
            }),
        ] {
            let (ty, sub, full) = kinds(offer);
            assert_eq!(ty, "_http._tcp.local.", "{offer:?}");
            assert_eq!(sub, "_eink-display._sub._http._tcp.local.");
            assert_eq!(full, "E-ink home display._http._tcp.local.");
        }
        let (ty, sub, full) = kinds(Some(SecureOffer {
            port: 8443,
            required: true,
        }));
        assert_eq!(ty, "_https._tcp.local.");
        assert_eq!(sub, "_eink-display._sub._https._tcp.local.");
        assert_eq!(full, "E-ink home display._https._tcp.local.");
    }

    #[test]
    fn https_only_is_announced_on_the_https_port_and_as_required() {
        let offer = SecureOffer {
            port: 8443,
            required: true,
        };
        let info = service_info(
            &settings(),
            8443,
            ImageFormat::Png,
            Families::Both,
            Some(offer),
        )
        .unwrap();
        assert_eq!(info.get_port(), 8443);
        assert_eq!(info.get_property_val_str("tlsport"), Some("8443"));
        assert_eq!(info.get_property_val_str("secure"), Some("required"));
    }

    #[test]
    fn a_specific_bind_address_is_announced_as_is() {
        let config = ServerSettings {
            bind: "192.168.1.5:9000".parse().unwrap(),
            ..settings()
        };
        let info = service_info(&config, 9000, ImageFormat::Bmp, Families::V4, None).unwrap();

        assert!(!info.is_addr_auto());
        assert!(
            info.get_addresses()
                .contains(&"192.168.1.5".parse::<IpAddr>().unwrap())
        );
    }

    #[test]
    fn a_specific_ipv6_bind_address_is_announced_as_is() {
        let config = ServerSettings {
            bind: "[fd00::5]:9000".parse().unwrap(),
            ..settings()
        };
        let info = service_info(&config, 9000, ImageFormat::Bmp, Families::V6, None).unwrap();

        assert!(!info.is_addr_auto());
        assert!(
            info.get_addresses()
                .contains(&"fd00::5".parse::<IpAddr>().unwrap())
        );
    }

    #[test]
    fn rejects_unusable_instance_names() {
        for name in ["", "   ", &"x".repeat(64)] {
            let config = ServerSettings {
                instance_name: name.to_owned(),
                ..settings()
            };
            assert!(
                service_info(&config, 80, ImageFormat::Bmp, Families::Both, None).is_err(),
                "{name:?}"
            );
        }
    }

    /// Needs working multicast on this machine, so it is not part of the normal run:
    /// `cargo test advertised_service_is_found_by_a_scan -- --ignored`
    #[tokio::test]
    #[ignore = "needs multicast networking"]
    async fn advertised_service_is_found_by_a_scan() {
        let config = ServerSettings {
            instance_name: "Eink test".to_owned(),
            ..settings()
        };
        let advertisement =
            Advertisement::start(&config, 18080, ImageFormat::Bmp, Families::Both, None).unwrap();

        let scanner = ServiceDaemon::new().unwrap();
        let found = scanner.browse("_http._tcp.local.").unwrap();
        let resolved = tokio::time::timeout(Duration::from_secs(15), async {
            while let Ok(event) = found.recv_async().await {
                if let ServiceEvent::ServiceResolved(service) = event
                    && service.get_fullname().starts_with("Eink test.")
                {
                    return Some(service);
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

    /// Needs working multicast, like the one above:
    /// `cargo test strict_https_is_found_as_an_https_service -- --ignored`
    #[tokio::test]
    #[ignore = "needs multicast networking"]
    async fn strict_https_is_found_as_an_https_service_and_not_as_an_http_one() {
        let config = ServerSettings {
            instance_name: "Eink strict test".to_owned(),
            ..settings()
        };
        let offer = SecureOffer {
            port: 18443,
            required: true,
        };
        let advertisement = Advertisement::start(
            &config,
            18443,
            ImageFormat::Bmp,
            Families::Both,
            Some(offer),
        )
        .unwrap();

        let scanner = ServiceDaemon::new().unwrap();
        let secure = scanner.browse("_https._tcp.local.").unwrap();
        let found = tokio::time::timeout(Duration::from_secs(15), async {
            while let Ok(event) = secure.recv_async().await {
                if let ServiceEvent::ServiceResolved(service) = event
                    && service.get_fullname().starts_with("Eink strict test.")
                {
                    return Some(service);
                }
            }
            None
        })
        .await
        .expect("timed out waiting for the service")
        .expect("scan ended without finding it");
        assert_eq!(found.get_port(), 18443);
        assert_eq!(found.get_property_val_str("secure"), Some("required"));
        assert_eq!(found.get_property_val_str("tlsport"), Some("18443"));

        // And a client that browses for plain HTTP services is not pointed at it.
        let plain = scanner.browse("_http._tcp.local.").unwrap();
        let seen_as_http = tokio::time::timeout(Duration::from_secs(4), async {
            while let Ok(event) = plain.recv_async().await {
                if let ServiceEvent::ServiceResolved(service) = event
                    && service.get_fullname().starts_with("Eink strict test.")
                {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        assert!(
            !seen_as_http,
            "a TLS-only port must not be listed as _http._tcp"
        );

        drop(advertisement);
        let _ = scanner.shutdown();
    }
}
