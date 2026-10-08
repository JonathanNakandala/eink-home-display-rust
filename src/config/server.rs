use std::net::SocketAddr;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const DEFAULT_BIND: &str = "[::]:8080";
const DEFAULT_DIRECTORY: &str = "served";
const DEFAULT_INSTANCE_NAME: &str = "E-ink home display";
const DEFAULT_WAKE_DELAY_SECONDS: u32 = 30;
const DEFAULT_STALE_GRACE_SECONDS: u32 = 300;
const DEFAULT_REFRESH_COOLDOWN_SECONDS: u32 = 30;
const DEFAULT_DEVICE_OVERDUE_GRACE_SECONDS: u32 = 900;
const DEFAULT_TLS_BIND: &str = "[::]:8443";
const DEFAULT_PKI_DIRECTORY: &str = "pki";
const DEFAULT_SERVER_CERTIFICATE_DAYS: u32 = 90;
const DEFAULT_DEVICE_CERTIFICATE_DAYS: u32 = 90;
const DEFAULT_PAIRING_RETRY_MINUTES: u32 = 5;

/// How displays reach the server.
///
/// `http` is plain HTTP, as before: nothing is encrypted and anyone on the network can read the
/// picture or pose as the server. `prefer-https` serves both: a display that has joined uses HTTPS and is
/// recognised by its certificate, and one that hasn't carries on over HTTP. That keeps every display
/// working while they are moved over, and protects against someone listening, but not against someone
/// who can interfere with the network, who can still send a display to HTTP. `https` serves HTTPS only
/// and only to displays that have joined: the one setting that protects against that. A display's own
/// setting (in its firmware) matters as much, since only a display set to HTTPS-only is certain not to
/// fall back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    #[default]
    Http,
    PreferHttps,
    Https,
}

impl Transport {
    /// Whether the plain-HTTP listener runs.
    pub fn serves_http(self) -> bool {
        !matches!(self, Self::Https)
    }

    /// Whether the HTTPS listener (and the certificate authority behind it) runs.
    pub fn serves_https(self) -> bool {
        !matches!(self, Self::Http)
    }

    /// Whether a display has to hold a certificate of the authority to be served.
    pub fn requires_certificate(self) -> bool {
        matches!(self, Self::Https)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::PreferHttps => "prefer-https",
            Self::Https => "https",
        }
    }
}

/// The admin interface: how the owner of a running server looks at it and changes it (opening the pairing window,
/// and later approving displays), from the same machine, with `displayctl`. It is a local socket, not a network
/// port. It runs only when HTTPS does (`transport = "prefer-https"` or `"https"`), because pairing is what it is for.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AdminConfig {
    /// Offer the admin interface. Turn it off and nothing can open the pairing window, so no new display can join
    /// until it is back on.
    #[serde(default = "default_admin_enabled")]
    pub enabled: bool,
    /// Where the socket is. Left out, it is `admin.sock` in `[server.tls] directory`. A Unix socket path has to be
    /// short (about 100 bytes), and the directory it is in has to be closed to other users; set this if either is
    /// a problem for the default.
    #[serde(default)]
    pub socket: Option<PathBuf>,
}

fn default_admin_enabled() -> bool {
    true
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            enabled: default_admin_enabled(),
            socket: None,
        }
    }
}

/// HTTPS and the certificate authority behind it. Used when `transport` is `prefer-https` or `https`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TlsConfig {
    /// Address for HTTPS. Must differ from `bind`'s port when both run (`prefer-https`).
    #[serde(default = "default_tls_bind")]
    pub bind: SocketAddr,
    /// Where the server's certificate authority and the list of displays that have joined are kept.
    /// Back this directory up: its `authority.key` is the one thing that can't be remade without
    /// pairing every display again. Relative paths are resolved against the working directory.
    #[serde(default = "default_pki_directory")]
    pub directory: PathBuf,
    /// More host names and IP addresses for the server's certificate, for a browser or `curl` to connect
    /// by. Left empty, it is `<instance_name>.local` (the name announced over mDNS, with the characters a
    /// host name can't have replaced) and `localhost`. The certificate also always has the fixed name
    /// `eink-home-display.internal`, which is what a display checks, so changing this never affects them.
    #[serde(default)]
    pub names: Vec<String>,
    /// How long the server's own certificate lasts. It is replaced when a third of that is left.
    #[serde(default = "default_server_certificate_days")]
    pub server_certificate_days: u32,
    /// How long a display's certificate lasts. Short on purpose: a display renews it with a third of its life
    /// left, so renewal happens all the time and not once in years when no one remembers how it works, and a
    /// display that has stopped renewing shows in `/status` within weeks. It is not what keeps a revoked
    /// display out (that takes effect on its next request). A display that is off for longer than this gets
    /// a new certificate by itself, with no one at the server, when it is next switched on.
    #[serde(default = "default_device_certificate_days")]
    pub device_certificate_days: u32,
    /// How long a display that is waiting for approval is told to wait before asking again.
    #[serde(default = "default_pairing_retry_minutes")]
    pub pairing_retry_minutes: u32,
    /// Require a request for a certificate to prove it was made on the connection it arrived on
    /// (RFC 9266). Leave off until the displays do this.
    #[serde(default)]
    pub require_channel_binding: bool,
}

fn default_tls_bind() -> SocketAddr {
    DEFAULT_TLS_BIND
        .parse()
        .expect("default TLS address is valid")
}

fn default_pki_directory() -> PathBuf {
    PathBuf::from(DEFAULT_PKI_DIRECTORY)
}

fn default_server_certificate_days() -> u32 {
    DEFAULT_SERVER_CERTIFICATE_DAYS
}

fn default_device_certificate_days() -> u32 {
    DEFAULT_DEVICE_CERTIFICATE_DAYS
}

fn default_pairing_retry_minutes() -> u32 {
    DEFAULT_PAIRING_RETRY_MINUTES
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            bind: default_tls_bind(),
            directory: default_pki_directory(),
            names: Vec::new(),
            server_certificate_days: default_server_certificate_days(),
            device_certificate_days: default_device_certificate_days(),
            pairing_retry_minutes: default_pairing_retry_minutes(),
            require_channel_binding: false,
        }
    }
}

/// The HTTP server that hands the latest rendered image to displays that fetch it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ServerConfig {
    /// Address to listen on. The image is served at `/image`, with no authentication, so keep it on the LAN.
    /// `[::]:8080` takes IPv4 and IPv6 clients on one socket, and the server announces both over mDNS; a host
    /// without IPv6 falls back to IPv4. `0.0.0.0:8080` is IPv4 only, and `[::]` with a specific address is that
    /// address only.
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
    /// Where the image to serve is written. Relative paths are resolved against the working directory.
    #[serde(default = "default_directory")]
    pub directory: PathBuf,
    /// Announce the server over mDNS / DNS-SD so it shows up in a scan of the network.
    #[serde(default = "default_advertise")]
    pub advertise: bool,
    /// The name shown for the service in a scan, at most 63 bytes. The host name `<name>.local` is derived from it.
    #[serde(default = "default_instance_name")]
    pub instance_name: String,
    /// How long after a scheduled render the display is told to come back, so the new image is ready.
    /// Also how soon it retries while a render is due but not finished.
    #[serde(default = "default_wake_delay_seconds")]
    pub wake_delay_seconds: u32,
    /// How late a scheduled render may be before the image is reported stale.
    #[serde(default = "default_stale_grace_seconds")]
    pub stale_grace_seconds: u32,
    /// The display's button asks for a render with `POST /refresh`. Requests are refused (and the
    /// current image returned) when a render started within this many seconds.
    #[serde(default = "default_refresh_cooldown_seconds")]
    pub refresh_cooldown_seconds: u32,
    /// A display is reported overdue (`/status`, `/metrics`) once it is this much later than the
    /// time it was told to return at. Covers a slow Wi-Fi join; a flat battery far exceeds it.
    #[serde(default = "default_device_overdue_grace_seconds")]
    pub device_overdue_grace_seconds: u32,
    /// Plain HTTP, HTTPS, or both: `"http"`, `"prefer-https"` or `"https"`. See `[server.tls]` for the
    /// HTTPS side. The default is `"http"`, so a configuration without this behaves as it always has.
    #[serde(default)]
    pub transport: Transport,
    #[serde(default)]
    pub tls: TlsConfig,
    /// How the owner looks at the running server and changes it. See `AdminConfig`.
    #[serde(default)]
    pub admin: AdminConfig,
}

fn default_device_overdue_grace_seconds() -> u32 {
    DEFAULT_DEVICE_OVERDUE_GRACE_SECONDS
}

fn default_refresh_cooldown_seconds() -> u32 {
    DEFAULT_REFRESH_COOLDOWN_SECONDS
}

fn default_wake_delay_seconds() -> u32 {
    DEFAULT_WAKE_DELAY_SECONDS
}

fn default_stale_grace_seconds() -> u32 {
    DEFAULT_STALE_GRACE_SECONDS
}

fn default_advertise() -> bool {
    true
}

fn default_instance_name() -> String {
    DEFAULT_INSTANCE_NAME.to_owned()
}

fn default_bind() -> SocketAddr {
    DEFAULT_BIND.parse().expect("default bind address is valid")
}

fn default_directory() -> PathBuf {
    PathBuf::from(DEFAULT_DIRECTORY)
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            directory: default_directory(),
            advertise: default_advertise(),
            instance_name: default_instance_name(),
            wake_delay_seconds: default_wake_delay_seconds(),
            stale_grace_seconds: default_stale_grace_seconds(),
            refresh_cooldown_seconds: default_refresh_cooldown_seconds(),
            device_overdue_grace_seconds: default_device_overdue_grace_seconds(),
            transport: Transport::default(),
            tls: TlsConfig::default(),
            admin: AdminConfig::default(),
        }
    }
}

impl ServerConfig {
    /// Where the admin socket is: as configured, or `admin.sock` beside the authority.
    pub fn admin_socket(&self) -> PathBuf {
        self.admin
            .socket
            .clone()
            .unwrap_or_else(|| self.tls.directory.join("admin.sock"))
    }

    /// Whether the settings make sense together, in words for whoever wrote them.
    pub fn check(&self) -> Result<(), String> {
        if !self.transport.serves_https() {
            return Ok(());
        }
        let tls = &self.tls;
        if self.transport.serves_http()
            && tls.bind.port() == self.bind.port()
            && self.bind.port() != 0
        {
            return Err(format!(
                "[server] bind and [server.tls] bind both use port {}; with transport = \"{}\" they must differ",
                self.bind.port(),
                self.transport.name()
            ));
        }
        for (name, days) in [
            ("server_certificate_days", tls.server_certificate_days),
            ("device_certificate_days", tls.device_certificate_days),
            ("pairing_retry_minutes", tls.pairing_retry_minutes),
        ] {
            if days == 0 {
                return Err(format!("[server.tls] {name} must be at least 1"));
            }
        }
        Ok(())
    }

    /// The names the server's certificate is for: those configured, or the host name it announces over
    /// mDNS and `localhost`.
    pub fn certificate_names(&self) -> Vec<String> {
        if !self.tls.names.is_empty() {
            return self.tls.names.clone();
        }
        let announced = crate::adapters::image_server::mdns_host_name(&self.instance_name);
        vec![announced, "localhost".to_owned()]
    }
}

impl From<&ServerConfig> for crate::application::plan::PlanTiming {
    fn from(config: &ServerConfig) -> Self {
        Self {
            wake_delay: std::time::Duration::from_secs(config.wake_delay_seconds.into()),
            stale_grace: std::time::Duration::from_secs(config.stale_grace_seconds.into()),
        }
    }
}

impl From<&ServerConfig> for crate::adapters::image_server::ServerSettings {
    fn from(config: &ServerConfig) -> Self {
        Self {
            bind: config.bind,
            advertise: config.advertise,
            instance_name: config.instance_name.clone(),
            timing: config.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_text: &str) -> Result<ServerConfig, toml::de::Error> {
        toml::from_str(toml_text)
    }

    #[test]
    fn a_configuration_that_says_nothing_about_transport_is_plain_http_as_before() {
        let config = parse("").unwrap();
        assert_eq!(config.transport, Transport::Http);
        assert!(config.check().is_ok());
        // And the committed example, with the new keys taken out, is the same.
        let older = parse("bind = \"[::]:8080\"\ndirectory = \"served\"\n").unwrap();
        assert_eq!(older.transport, Transport::Http);
    }

    #[test]
    fn the_three_choices_are_spelled_as_documented() {
        for (text, expected) in [
            ("http", Transport::Http),
            ("prefer-https", Transport::PreferHttps),
            ("https", Transport::Https),
        ] {
            let config = parse(&format!("transport = \"{text}\"")).unwrap();
            assert_eq!(config.transport, expected);
            assert_eq!(expected.name(), text);
            // And back out the same way, so a written configuration reads the same.
            assert!(
                toml::to_string(&config)
                    .unwrap()
                    .contains(&format!("transport = \"{text}\""))
            );
        }
    }

    #[test]
    fn a_misspelt_choice_is_refused_and_says_what_is_allowed() {
        for bad in ["HTTPS", "preferhttps", "prefer_https", "tls", "strict", ""] {
            let error = parse(&format!("transport = \"{bad}\""))
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("http") && error.contains("prefer-https"),
                "{bad:?}: {error}"
            );
        }
    }

    #[test]
    fn each_choice_runs_the_listeners_it_should() {
        use Transport::*;
        // (serves plain HTTP, serves HTTPS, requires a certificate)
        assert_eq!(
            (
                Http.serves_http(),
                Http.serves_https(),
                Http.requires_certificate()
            ),
            (true, false, false)
        );
        assert_eq!(
            (
                PreferHttps.serves_http(),
                PreferHttps.serves_https(),
                PreferHttps.requires_certificate()
            ),
            (true, true, false)
        );
        assert_eq!(
            (
                Https.serves_http(),
                Https.serves_https(),
                Https.requires_certificate()
            ),
            (false, true, true)
        );
    }

    #[test]
    fn both_listeners_may_not_share_a_port() {
        let config = parse(
            "transport = \"prefer-https\"\nbind = \"[::]:9000\"\n[tls]\nbind = \"[::]:9000\"",
        )
        .unwrap();
        let error = config.check().unwrap_err();
        assert!(
            error.contains("9000") && error.contains("prefer-https"),
            "{error}"
        );
        // With one listener there is nothing to clash with.
        let strict =
            parse("transport = \"https\"\nbind = \"[::]:9000\"\n[tls]\nbind = \"[::]:9000\"")
                .unwrap();
        assert!(strict.check().is_ok());
        // Nor when the transport is plain HTTP and the TLS section is unused.
        let plain = parse("bind = \"[::]:9000\"\n[tls]\nbind = \"[::]:9000\"").unwrap();
        assert!(plain.check().is_ok());
        // A port of 0 means "any", as the tests use.
        let any =
            parse("transport = \"prefer-https\"\nbind = \"[::]:0\"\n[tls]\nbind = \"[::]:0\"")
                .unwrap();
        assert!(any.check().is_ok());
    }

    #[test]
    fn lifetimes_and_waits_of_zero_are_refused_when_https_is_on() {
        for key in [
            "server_certificate_days",
            "device_certificate_days",
            "pairing_retry_minutes",
        ] {
            let config = parse(&format!("transport = \"https\"\n[tls]\n{key} = 0")).unwrap();
            let error = config.check().unwrap_err();
            assert!(error.contains(key), "{error}");
        }
        // Unused settings are not judged.
        assert!(
            parse("[tls]\nserver_certificate_days = 0")
                .unwrap()
                .check()
                .is_ok()
        );
    }

    #[test]
    fn the_certificate_is_for_the_announced_name_unless_names_are_given() {
        let config = parse("instance_name = \"Living room\"").unwrap();
        assert_eq!(
            config.certificate_names(),
            ["living-room.local", "localhost"]
        );
        let named = parse("[tls]\nnames = [\"eink.example.net\", \"192.168.1.5\"]").unwrap();
        assert_eq!(
            named.certificate_names(),
            ["eink.example.net", "192.168.1.5"]
        );
    }
}
