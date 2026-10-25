//! Announces the image server over mDNS / DNS-SD (RFC 6762, RFC 6763), so a scan of the
//! network finds it and says where it is.
//!
//! It is published as an ordinary `_http._tcp` service, which every service browser
//! lists and can open, with the `path` TXT key naming the page. The `_home-display`
//! subtype lets a scan ask for just this server.
//!
//! When the server speaks HTTPS and nothing else, it is `_https._tcp` instead (RFC 6763 and the
//! IANA service registry), on the HTTPS port. Announcing a TLS-only port as `_http._tcp` would send a
//! browser, or any client that goes by the service type, to speak plain HTTP to it.
//!
//! The responder is watched, and started again when it stops matching the machine. When a computer sleeps and wakes (or its
//! address is changed by DHCP) the responder of `mdns-sd` drops the old address, and then fails to join the multicast group
//! on the new one, every five seconds, for as long as the process lives: it announces nothing and answers no query, so a
//! display that looks for the server finds nothing, and only a restart of the whole server put it right. A new responder
//! started in the same process joins at once.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context, bail};
use mdns_sd::{DaemonEvent, IfKind, IfPredicate, ServiceDaemon, ServiceInfo};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::ServerSettings;
use crate::adapters::listen::Families;
use crate::domain::models::display::ImageFormat;

const SERVICE_SUBTYPE: &str = "_home-display._sub._http._tcp.local.";
const SECURE_SERVICE_SUBTYPE: &str = "_home-display._sub._https._tcp.local.";
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

/// A registered service, kept announced. Dropping it withdraws the service with a goodbye (a record with a
/// zero TTL, RFC 6762 section 8.4), so scans stop listing it at once instead of after the TTL.
pub struct Advertisement {
    responder: Arc<Mutex<Option<Box<dyn Responder>>>>,
    supervisor: JoinHandle<()>,
}

/// What announces the service on the network, and can say whether it is doing so.
trait Responder: Send {
    /// Whether it is announcing on at least one address of the machine.
    fn serving(&self) -> bool;
}

/// The addresses of the machine that a client could reach the server on, in the form `Watch` compares them.
type Addresses = Arc<dyn Fn() -> BTreeSet<IpAddr> + Send + Sync>;

/// Starts a responder.
type Begin = Box<dyn Fn() -> anyhow::Result<Box<dyn Responder>> + Send>;

impl Advertisement {
    pub fn start(
        settings: &ServerSettings,
        port: u16,
        format: ImageFormat,
        families: Families,
        secure: Option<SecureOffer>,
    ) -> anyhow::Result<Self> {
        // Checked now, so that a name that cannot be announced stops the start as it always has.
        service_info(settings, port, format, families, secure)?;
        let settings = settings.clone();
        let addresses: Addresses = Arc::new(move || local_addresses(families));
        let begin: Begin = {
            let addresses = addresses.clone();
            Box::new(move || {
                let info = service_info(&settings, port, format, families, secure)?;
                Ok(Box::new(MdnsResponder::start(
                    info,
                    families,
                    addresses.clone(),
                )?))
            })
        };
        Self::supervise(begin, addresses, Timing::default())
    }

    /// Starts the first responder, and a task that starts it again when it stops matching the machine.
    fn supervise(begin: Begin, addresses: Addresses, timing: Timing) -> anyhow::Result<Self> {
        let responder = Arc::new(Mutex::new(Some(begin()?)));
        let supervisor = tokio::spawn(watch_over(responder.clone(), begin, addresses, timing));
        Ok(Self {
            responder,
            supervisor,
        })
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        self.supervisor.abort();
        // Dropped here and not left to the runtime, so that the goodbye has gone out when this returns.
        let responder = self
            .responder
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(responder);
    }
}

/// Looks at the responder every so often and starts it again when `Watch` says so.
async fn watch_over(
    responder: Arc<Mutex<Option<Box<dyn Responder>>>>,
    begin: Begin,
    addresses: Addresses,
    timing: Timing,
) {
    let mut watch = Watch::new(timing, Instant::now(), addresses());
    loop {
        tokio::time::sleep(timing.every).await;
        let serving = responder
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|responder| responder.serving());
        let Some(why) = watch.judge(Instant::now(), addresses(), serving) else {
            continue;
        };
        tracing::warn!(%why, "Starting the mDNS responder again");
        // The old one goes first and its goodbye is sent, so that the new one does not find the name taken. Its drop waits
        // for the goodbye, so it is not done on this task's thread.
        let old = responder
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(old) = old {
            let _ = tokio::task::spawn_blocking(move || drop(old)).await;
        }
        match begin() {
            Ok(new) => {
                *responder.lock().unwrap_or_else(PoisonError::into_inner) = Some(new);
            }
            // Nothing is announced until the next try, which the silence it leaves brings about.
            Err(e) => {
                tracing::warn!(error = %format_args!("{e:#}"), "Could not start the mDNS responder again")
            }
        }
        watch.restarted(Instant::now(), addresses());
    }
}

/// How often the responder is looked at, and how patient to be with it.
#[derive(Debug, Clone, Copy)]
struct Timing {
    /// Between one look and the next.
    every: Duration,
    /// How long the responder may announce nothing, while the machine has an address, before it is started again.
    silent_for: Duration,
    /// The least time between two starts, doubled each time a start does not help (until `longest_gap`), so that a
    /// responder that cannot be made to work is tried now and then and not all the time.
    shortest_gap: Duration,
    longest_gap: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            every: Duration::from_secs(10),
            silent_for: Duration::from_secs(30),
            shortest_gap: Duration::from_secs(15),
            longest_gap: Duration::from_secs(300),
        }
    }
}

/// Why the responder is started again.
#[derive(Debug, PartialEq, Eq)]
enum Why {
    /// The machine has other addresses than the responder was started with, and they have stayed the same for a look.
    AddressesChanged,
    /// The responder announces on no address, though the machine has one.
    Silent,
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddressesChanged => f.write_str("the addresses of this machine have changed"),
            Self::Silent => f.write_str("it announces on no address, though this machine has one"),
        }
    }
}

/// Decides when the responder is started again. No clocks or networks of its own, so that it can be tried with any.
struct Watch {
    timing: Timing,
    /// The machine's addresses when the responder was last started.
    started_with: BTreeSet<IpAddr>,
    /// The machine's addresses at the look before, so that a change is acted on once it has stopped changing: a computer
    /// waking up takes its address away and gives it back in more than one step.
    previous: BTreeSet<IpAddr>,
    /// Since when the responder has announced on no address while the machine has one.
    silent_since: Option<Instant>,
    last_start: Instant,
    gap: Duration,
}

impl Watch {
    fn new(timing: Timing, now: Instant, addresses: BTreeSet<IpAddr>) -> Self {
        Self {
            timing,
            started_with: addresses.clone(),
            previous: addresses,
            silent_since: None,
            last_start: now,
            gap: timing.shortest_gap,
        }
    }

    /// A look: the machine's addresses now, and whether the responder says it is announcing.
    fn judge(&mut self, now: Instant, addresses: BTreeSet<IpAddr>, serving: bool) -> Option<Why> {
        let settled = addresses == self.previous;
        self.previous = addresses.clone();
        if serving {
            self.silent_since = None;
            self.gap = self.timing.shortest_gap;
        } else if addresses.is_empty() {
            // No network at all: there is nothing to announce on, and starting again would change nothing.
            self.silent_since = None;
        } else {
            self.silent_since.get_or_insert(now);
        }
        if addresses.is_empty() || now.duration_since(self.last_start) < self.gap {
            return None;
        }
        if addresses != self.started_with && settled {
            return Some(Why::AddressesChanged);
        }
        let silent_for = self.silent_since.map(|since| now.duration_since(since));
        if silent_for.is_some_and(|silent| silent >= self.timing.silent_for) {
            return Some(Why::Silent);
        }
        None
    }

    /// The responder has been started again.
    fn restarted(&mut self, now: Instant, addresses: BTreeSet<IpAddr>) {
        self.started_with = addresses;
        self.last_start = now;
        self.silent_since = None;
        self.gap = (self.gap * 2).min(self.timing.longest_gap);
    }
}

/// The addresses this machine has that a client on the network could reach the server on: not loopback, not link-local,
/// and only the IP versions served.
fn local_addresses(families: Families) -> BTreeSet<IpAddr> {
    let Ok(all) = if_addrs::get_if_addrs() else {
        return BTreeSet::new();
    };
    all.into_iter()
        .filter(|intf| !intf.is_loopback() && !is_loopback_interface(&intf.name))
        .map(|intf| intf.ip())
        .filter(|ip| match families {
            Families::Both => true,
            Families::V4 => ip.is_ipv4(),
            Families::V6 => ip.is_ipv6(),
        })
        .filter(|ip| !is_link_local(ip))
        .collect()
}

fn is_link_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_link_local(),
        IpAddr::V6(ip) => ip.segments()[0] & 0xffc0 == 0xfe80,
    }
}

/// What the responder has said about itself.
#[derive(Default)]
struct Served {
    /// It has announced the service. Before that it has not joined anything.
    announced: bool,
    /// The addresses it is announcing on: those the machine had when it announced, and then what it has said about
    /// addresses it gained and lost. (It says nothing of the addresses it starts with.)
    addresses: BTreeSet<IpAddr>,
}

/// The responder of `mdns-sd`.
struct MdnsResponder {
    daemon: ServiceDaemon,
    fullname: String,
    served: Arc<Mutex<Served>>,
}

impl Responder for MdnsResponder {
    fn serving(&self) -> bool {
        let served = self.served.lock().unwrap_or_else(PoisonError::into_inner);
        served.announced && !served.addresses.is_empty()
    }
}

impl MdnsResponder {
    fn start(info: ServiceInfo, families: Families, addresses: Addresses) -> anyhow::Result<Self> {
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
            tracing::warn!(error = %e, "Could not narrow the mDNS interfaces");
        }
        let served = Arc::new(Mutex::new(Served::default()));
        let this = Self {
            daemon,
            fullname,
            served: served.clone(),
        };

        let events = this
            .daemon
            .monitor()
            .context("Failed to watch the mDNS responder")?;
        tokio::spawn(async move {
            while let Ok(event) = events.recv_async().await {
                let mut served = served.lock().unwrap_or_else(PoisonError::into_inner);
                match event {
                    // The responder probes first and renames itself if the name is taken.
                    DaemonEvent::NameChange(change) => {
                        tracing::warn!(
                            original = %change.original,
                            new_name = %change.new_name,
                            "mDNS name is taken, using another"
                        )
                    }
                    DaemonEvent::Error(e) => tracing::warn!(error = %e, "mDNS error"),
                    DaemonEvent::Announce(..) => {
                        tracing::debug!(?event, "mDNS event");
                        if !served.announced {
                            served.announced = true;
                            served.addresses = addresses();
                        }
                    }
                    DaemonEvent::IpAdd(ip) => {
                        tracing::debug!(?event, "mDNS event");
                        served.addresses.insert(ip);
                    }
                    DaemonEvent::IpDel(ip) => {
                        tracing::debug!(?event, "mDNS event");
                        served.addresses.remove(&ip);
                    }
                    other => tracing::debug!(event = ?other, "mDNS event"),
                }
            }
        });

        this.daemon
            .register(info)
            .context("Failed to register the mDNS service")?;
        tracing::info!(name = %this.fullname, "Advertising over mDNS");
        Ok(this)
    }
}

impl Drop for MdnsResponder {
    fn drop(&mut self) {
        // Best effort: the process may be ending, and the records expire on their own anyway.
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
        return "home-display".to_owned();
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
            instance_name: "Home display".to_owned(),
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
        assert_eq!(host_label("Home display"), "home-display");
        assert_eq!(host_label("  Living room!! "), "living-room");
        assert_eq!(host_label("日本語"), "home-display");
        assert_eq!(host_label(&"a".repeat(100)).len(), 63);
    }

    #[test]
    fn describes_an_http_service_with_a_subtype_and_a_path() {
        let config = settings();
        let info = service_info(&config, 8080, ImageFormat::Png, Families::Both, None).unwrap();

        assert_eq!(info.get_fullname(), "Home display._http._tcp.local.");
        assert_eq!(info.get_type(), "_http._tcp.local.");
        assert_eq!(info.get_subtype().as_deref(), Some(SERVICE_SUBTYPE));
        assert_eq!(info.get_hostname(), "home-display.local.");
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
            assert_eq!(sub, "_home-display._sub._http._tcp.local.");
            assert_eq!(full, "Home display._http._tcp.local.");
        }
        let (ty, sub, full) = kinds(Some(SecureOffer {
            port: 8443,
            required: true,
        }));
        assert_eq!(ty, "_https._tcp.local.");
        assert_eq!(sub, "_home-display._sub._https._tcp.local.");
        assert_eq!(full, "Home display._https._tcp.local.");
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
            instance_name: "Home display test".to_owned(),
            ..settings()
        };
        let advertisement =
            Advertisement::start(&config, 18080, ImageFormat::Bmp, Families::Both, None).unwrap();

        let scanner = ServiceDaemon::new().unwrap();
        let found = scanner.browse("_http._tcp.local.").unwrap();
        let resolved = tokio::time::timeout(Duration::from_secs(15), async {
            while let Ok(event) = found.recv_async().await {
                if let ServiceEvent::ServiceResolved(service) = event
                    && service.get_fullname().starts_with("Home display test.")
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
            instance_name: "Home display strict test".to_owned(),
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
                    && service
                        .get_fullname()
                        .starts_with("Home display strict test.")
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
                    && service
                        .get_fullname()
                        .starts_with("Home display strict test.")
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

    // --- keeping it announced ---------------------------------------------------------------------------------

    fn set(addresses: &[&str]) -> BTreeSet<IpAddr> {
        addresses.iter().map(|a| a.parse().unwrap()).collect()
    }

    fn after(start: Instant, seconds: u64) -> Instant {
        start + Duration::from_secs(seconds)
    }

    #[test]
    fn a_responder_that_serves_on_a_machine_that_has_not_changed_is_left_alone() {
        let start = Instant::now();
        let mut watch = Watch::new(Timing::default(), start, set(&["192.168.0.73"]));

        for seconds in (10..=3600).step_by(10) {
            let judged = watch.judge(after(start, seconds), set(&["192.168.0.73"]), true);
            assert_eq!(judged, None, "after {seconds} s");
        }
    }

    #[test]
    fn new_addresses_are_acted_on_at_the_second_look_that_finds_them_and_not_the_first() {
        let start = Instant::now();
        let mut watch = Watch::new(Timing::default(), start, set(&["192.168.0.73"]));

        // A computer waking up gives its address back in more than one step: wait until it has stopped changing.
        assert_eq!(
            watch.judge(after(start, 20), set(&["192.168.0.78"]), true),
            None
        );
        assert_eq!(
            watch.judge(after(start, 30), set(&["192.168.0.78"]), true),
            Some(Why::AddressesChanged)
        );
    }

    #[test]
    fn addresses_that_go_and_come_back_between_two_looks_are_not_acted_on() {
        let start = Instant::now();
        let mut watch = Watch::new(Timing::default(), start, set(&["192.168.0.73"]));

        assert_eq!(
            watch.judge(after(start, 20), set(&["192.168.0.78"]), true),
            None
        );
        assert_eq!(
            watch.judge(after(start, 30), set(&["192.168.0.73"]), true),
            None
        );
        assert_eq!(
            watch.judge(after(start, 40), set(&["192.168.0.73"]), true),
            None
        );
    }

    #[test]
    fn a_responder_that_announces_nothing_is_started_again_when_its_patience_is_up() {
        let start = Instant::now();
        let mut watch = Watch::new(Timing::default(), start, set(&["192.168.0.73"]));

        // As after a sleep: the machine has its address, and the responder is not on it.
        for seconds in [10, 20, 30] {
            let judged = watch.judge(after(start, seconds), set(&["192.168.0.73"]), false);
            assert_eq!(judged, None, "after {seconds} s");
        }
        assert_eq!(
            watch.judge(after(start, 40), set(&["192.168.0.73"]), false),
            Some(Why::Silent)
        );
    }

    #[test]
    fn a_machine_with_no_address_is_not_the_responders_fault() {
        let start = Instant::now();
        let mut watch = Watch::new(Timing::default(), start, set(&["192.168.0.73"]));

        for seconds in (10..=3600).step_by(10) {
            assert_eq!(
                watch.judge(after(start, seconds), BTreeSet::new(), false),
                None,
                "after {seconds} s"
            );
        }
        // And the silence is counted from when the address comes back, not from when it went.
        assert_eq!(
            watch.judge(after(start, 3610), set(&["192.168.0.73"]), false),
            None
        );
        assert_eq!(
            watch.judge(after(start, 3630), set(&["192.168.0.73"]), false),
            None
        );
        assert_eq!(
            watch.judge(after(start, 3640), set(&["192.168.0.73"]), false),
            Some(Why::Silent)
        );
    }

    #[test]
    fn starts_that_do_not_help_are_spaced_further_and_further_apart() {
        let start = Instant::now();
        let machine = set(&["192.168.0.73"]);
        let mut watch = Watch::new(Timing::default(), start, machine.clone());

        let mut starts = Vec::new();
        for seconds in (10..=1000).step_by(10) {
            let now = after(start, seconds);
            if watch.judge(now, machine.clone(), false).is_some() {
                watch.restarted(now, machine.clone());
                starts.push(seconds);
            }
        }

        // After 30 s of silence, then twice, four times, eight times the least gap, never more than five minutes.
        assert_eq!(starts, [40, 80, 140, 260, 500, 800]);
    }

    #[test]
    fn a_responder_that_serves_again_gets_the_short_gap_back() {
        let start = Instant::now();
        let machine = set(&["192.168.0.73"]);
        let mut watch = Watch::new(Timing::default(), start, machine.clone());
        let look = |watch: &mut Watch, seconds, serving| {
            let now = after(start, seconds);
            let judged = watch.judge(now, machine.clone(), serving);
            if judged.is_some() {
                watch.restarted(now, machine.clone());
            }
            judged
        };
        for seconds in (10..=80).step_by(10) {
            look(&mut watch, seconds, false);
        }
        // Two starts in, the gap is a minute. It serves for a moment, and then goes quiet again.
        assert_eq!(look(&mut watch, 90, true), None);

        for seconds in [100, 110, 120] {
            assert_eq!(look(&mut watch, seconds, false), None, "after {seconds} s");
        }
        assert_eq!(look(&mut watch, 130, false), Some(Why::Silent));
    }

    /// A responder that only keeps a record of when it was made and when it was withdrawn.
    struct Fake {
        id: usize,
        serving: bool,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Responder for Fake {
        fn serving(&self) -> bool {
            self.serving
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.log.lock().unwrap().push(format!("stop {}", self.id));
        }
    }

    struct Rig {
        log: Arc<Mutex<Vec<String>>>,
        machine: Arc<Mutex<BTreeSet<IpAddr>>>,
        advertisement: Advertisement,
    }

    impl Rig {
        /// `silent_first`: the first responder announces on nothing, as one that has lost its interface does.
        fn new(silent_first: bool) -> Self {
            let log = Arc::new(Mutex::new(Vec::new()));
            let machine = Arc::new(Mutex::new(set(&["192.168.0.73"])));
            let starts = Arc::new(Mutex::new(0usize));
            let begin: Begin = {
                let log = log.clone();
                Box::new(move || {
                    let mut starts = starts.lock().unwrap();
                    *starts += 1;
                    log.lock().unwrap().push(format!("start {}", *starts));
                    Ok(Box::new(Fake {
                        id: *starts,
                        serving: !(silent_first && *starts == 1),
                        log: log.clone(),
                    }))
                })
            };
            let addresses: Addresses = {
                let machine = machine.clone();
                Arc::new(move || machine.lock().unwrap().clone())
            };
            Self {
                log,
                machine,
                advertisement: Advertisement::supervise(begin, addresses, Timing::default())
                    .unwrap(),
            }
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    /// Waits, in real time, until `done`. The responder is withdrawn on a thread of its own, which paused time doesn't wait for.
    async fn until(done: impl Fn() -> bool) {
        for _ in 0..1000 {
            if done() {
                return;
            }
            tokio::task::yield_now().await;
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("it did not happen");
    }

    #[tokio::test(start_paused = true)]
    async fn a_machine_that_changes_address_gets_a_new_responder_once_the_old_one_is_withdrawn() {
        let rig = Rig::new(false);
        tokio::time::sleep(Duration::from_secs(100)).await;
        assert_eq!(rig.log(), ["start 1"], "nothing has changed");

        *rig.machine.lock().unwrap() = set(&["192.168.0.78"]);
        tokio::time::sleep(Duration::from_secs(30)).await;
        until(|| rig.log().len() >= 3).await;

        // The old one goes first, so that the new one does not find the name taken.
        assert_eq!(rig.log(), ["start 1", "stop 1", "start 2"]);
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert_eq!(rig.log().len(), 3, "and it is left alone after that");
    }

    #[tokio::test(start_paused = true)]
    async fn a_responder_that_announces_nothing_is_replaced_by_one_that_does() {
        let rig = Rig::new(true);

        tokio::time::sleep(Duration::from_secs(60)).await;
        until(|| rig.log().len() >= 3).await;

        assert_eq!(rig.log(), ["start 1", "stop 1", "start 2"]);
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert_eq!(rig.log().len(), 3, "the new one serves, so it stays");
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_advertisement_withdraws_the_responder_and_ends_the_watching() {
        let Rig {
            log,
            machine,
            advertisement,
        } = Rig::new(false);
        tokio::time::sleep(Duration::from_secs(20)).await;

        drop(advertisement);
        assert_eq!(
            *log.lock().unwrap(),
            ["start 1", "stop 1"],
            "withdrawn by the time drop returns"
        );

        *machine.lock().unwrap() = set(&["192.168.0.78"]);
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert_eq!(log.lock().unwrap().len(), 2, "nothing is started after it");
    }

    /// Needs working multicast on this machine, so it is not part of the normal run:
    /// `cargo test a_responder_started_again -- --ignored`
    #[tokio::test]
    #[ignore = "needs multicast networking"]
    async fn a_responder_started_again_is_still_found_by_a_scan() {
        let config = ServerSettings {
            instance_name: "Home display restart test".to_owned(),
            ..settings()
        };
        let machine = Arc::new(Mutex::new(local_addresses(Families::Both)));
        let addresses: Addresses = {
            let machine = machine.clone();
            Arc::new(move || machine.lock().unwrap().clone())
        };
        let starts = Arc::new(Mutex::new(0usize));
        let begin: Begin = {
            let (addresses, starts) = (addresses.clone(), starts.clone());
            Box::new(move || {
                *starts.lock().unwrap() += 1;
                let info = service_info(&config, 18081, ImageFormat::Bmp, Families::Both, None)?;
                Ok(Box::new(MdnsResponder::start(
                    info,
                    Families::Both,
                    addresses.clone(),
                )?))
            })
        };
        let timing = Timing {
            every: Duration::from_secs(1),
            silent_for: Duration::from_secs(120),
            shortest_gap: Duration::from_secs(1),
            longest_gap: Duration::from_secs(5),
        };
        let advertisement = Advertisement::supervise(begin, addresses, timing).unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;

        // The machine seems to gain an address, which is what a wake from sleep looks like to the watching.
        machine
            .lock()
            .unwrap()
            .insert("10.255.255.1".parse().unwrap());
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(*starts.lock().unwrap(), 2, "started again, once");

        let scanner = ServiceDaemon::new().unwrap();
        let found = scanner.browse("_http._tcp.local.").unwrap();
        let resolved = tokio::time::timeout(Duration::from_secs(15), async {
            while let Ok(event) = found.recv_async().await {
                if let ServiceEvent::ServiceResolved(service) = event
                    && service
                        .get_fullname()
                        .starts_with("Home display restart test.")
                {
                    return Some(service);
                }
            }
            None
        })
        .await
        .expect("timed out waiting for the service after the responder was started again")
        .expect("scan ended without finding it");
        assert_eq!(resolved.get_port(), 18081);

        drop(advertisement);
        let _ = scanner.shutdown();
    }
}
