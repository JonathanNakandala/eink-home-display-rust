use std::net::SocketAddr;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const DEFAULT_BIND: &str = "[::]:8080";
const DEFAULT_DIRECTORY: &str = "served";
const DEFAULT_INSTANCE_NAME: &str = "Home display";
const DEFAULT_WAKE_DELAY_SECONDS: u32 = 30;
const DEFAULT_STALE_GRACE_SECONDS: u32 = 300;
const DEFAULT_REFRESH_COOLDOWN_SECONDS: u32 = 30;
const DEFAULT_DEVICE_OVERDUE_GRACE_SECONDS: u32 = 900;
const DEFAULT_TLS_BIND: &str = "[::]:8443";
const DEFAULT_PKI_DIRECTORY: &str = "pki";
const DEFAULT_SERVER_CERTIFICATE_DAYS: u32 = 90;
const DEFAULT_DEVICE_CERTIFICATE_DAYS: u32 = 90;
const DEFAULT_PAIRING_RETRY_MINUTES: u32 = 5;
/// The longest a certificate may be set to last: ten years. Far beyond sensible, but a limit all the same, because
/// a certificate cannot be dated past the year 9999 and a figure large enough overflows a date outright.
pub const MAX_CERTIFICATE_DAYS: u32 = 3650;
/// The longest a waiting display may be told to wait: a day, which is also the longest it is ever told to sleep.
pub const MAX_PAIRING_RETRY_MINUTES: u32 = 24 * 60;

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

fn default_streams() -> u32 {
    8
}

fn default_streams_per_source() -> u32 {
    2
}

fn default_access_token_minutes() -> u32 {
    crate::application::viewers::DEFAULT_ACCESS_MINUTES
}

/// The screen the page is shown on, for pictures drawn to fit it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScreenConfig {
    /// Width in pixels, from 320 to 3840. The layouts scale with the size they are drawn at, so 1920 by 1080 does for a
    /// full-HD tablet or screen; a lower size draws faster and a bigger file downloads slower.
    #[serde(default = "default_screen_width")]
    pub width: u32,
    /// Height in pixels, from 240 to 2160.
    #[serde(default = "default_screen_height")]
    pub height: u32,
}

fn default_screen_width() -> u32 {
    1920
}

fn default_screen_height() -> u32 {
    1080
}

impl Default for ScreenConfig {
    fn default() -> Self {
        Self {
            width: default_screen_width(),
            height: default_screen_height(),
        }
    }
}

/// A page that shows the current picture in a web browser and keeps it up to date, at `/view`: for a tablet on the wall, or
/// a look at what the display shows from a laptop. It shows what `/image` serves, so it is as private as `/image` is.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebConfig {
    /// Serve the page. Off unless turned on, because it is another way to see the picture.
    #[serde(default)]
    pub enabled: bool,
    /// Let anyone who can reach the server see it, with no sign-in: as open as `/image` is with `transport = "http"`.
    /// Keep the server on a network you trust. With `transport = "https"` this is a way round the certificate that
    /// `/image` asks for, for the page only, so the server says so when it starts. Without it (the default) a browser needs
    /// a viewer token, which `displayctl viewer create` gives a link for. Tokens are only ever sent over HTTPS, so this
    /// needs `transport` to be `"prefer-https"` or `"https"`, and `[server.admin]` to be on; the page is then served on the
    /// HTTPS port only.
    #[serde(default)]
    pub open: bool,
    /// The address a browser reaches the HTTPS side at, as the link `displayctl viewer create` prints should have it,
    /// such as `https://display.example.home:8443`. Left out, it is the first name of the server's certificate and
    /// the HTTPS port.
    #[serde(default)]
    pub public_url: Option<String>,
    /// How long an access token lasts for a viewer made with `displayctl viewer create --refresh`, from 1 to 1440 minutes. It
    /// is renewed with the viewer's refresh token, by the page, before it runs out, so this is how long a token that is lost or
    /// copied stays good. A viewer made without `--refresh` has one token for its whole life, and this does not apply.
    #[serde(default = "default_access_token_minutes")]
    pub access_token_minutes: u32,
    /// How many streams of changes may be open at once, in all, from 1 to 15. A page with one is told of a new picture as it is
    /// drawn; each holds one of the server's 16 HTTPS connections (and plain HTTP has no such limit), so the rest are kept for
    /// everything else, displays included. A page that is not given one asks on a timer instead, which costs only a few seconds.
    #[serde(default = "default_streams")]
    pub streams: u32,
    /// How many of them may be open from one source, from 1 to 3. The HTTPS side lets one source (a whole IPv6 network counts as
    /// one, so every device in a home on IPv6) hold four connections, and a stream holds one, so this leaves the others for
    /// what else that source asks for.
    #[serde(default = "default_streams_per_source")]
    pub streams_per_source: u32,
    /// Draw the pictures for the page at the page's own size, in full colour, in place of showing the panel's (which are
    /// 16 greys at the panel's size, and so look it on a colour screen). It is another output, drawn for besides the display
    /// each time, so it costs a render of every layout more. Left out, the page shows the panel's pictures.
    #[serde(default)]
    pub screen: Option<ScreenConfig>,
    /// The layouts the page may show, by name as `displayctl layout list` has them. Left empty, all of them.
    #[serde(default)]
    pub layouts: Vec<String>,
    /// Addresses of reverse proxies in front of the server, such as `["192.168.1.2"]`, whose `X-Forwarded-For` is believed. The
    /// server counts failed sign-ins and open streams by who is asking, and behind a proxy everyone arrives from the proxy, so
    /// without this a few wrong tokens from one device stop every other, and one device's streams use up all. Only list a proxy
    /// that sets the header itself: anything else lets a client choose the address it is counted as. Left empty, the address
    /// that connects is the one counted.
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
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
    /// `home-display.internal`, which is what a display checks, so changing this never affects them.
    #[serde(default)]
    pub names: Vec<String>,
    /// How long the server's own certificate lasts, from 1 to 3650 days. It is replaced when a third of that is left.
    #[serde(default = "default_server_certificate_days")]
    pub server_certificate_days: u32,
    /// How long a display's certificate lasts, from 1 to 3650 days. Short on purpose: a display renews it with a third of its life
    /// left, so renewal happens all the time and not once in years when no one remembers how it works, and a
    /// display that has stopped renewing shows in `/status` within weeks. It is not what keeps a revoked
    /// display out (that takes effect on its next request). A display that is off for longer than this gets
    /// a new certificate by itself, with no one at the server, when it is next switched on.
    #[serde(default = "default_device_certificate_days")]
    pub device_certificate_days: u32,
    /// How long a display that is waiting for approval is told to wait before asking again, from 1 to 1440 minutes.
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
    /// A page for a web browser to look at the picture, at `/view`. See `WebConfig`.
    #[serde(default)]
    pub web: WebConfig,
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
            web: WebConfig::default(),
        }
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            open: false,
            layouts: Vec::new(),
            public_url: None,
            access_token_minutes: default_access_token_minutes(),
            streams: default_streams(),
            streams_per_source: default_streams_per_source(),
            screen: None,
            trusted_proxies: Vec::new(),
        }
    }
}

impl WebConfig {
    /// Whether the settings make sense, in words for whoever wrote them.
    pub fn check(&self) -> Result<(), String> {
        let most = crate::application::viewers::MOST_ACCESS_MINUTES;
        if !(1..=most).contains(&self.access_token_minutes) {
            return Err(format!(
                "[server.web] access_token_minutes is {}; it must be from 1 to {most}",
                self.access_token_minutes
            ));
        }
        if !(1..=15).contains(&self.streams) {
            return Err(format!(
                "[server.web] streams is {}; it must be from 1 to 15, so that some of the server's 16 connections are left for everything else",
                self.streams
            ));
        }
        if !(1..=3).contains(&self.streams_per_source) {
            return Err(format!(
                "[server.web] streams_per_source is {}; it must be from 1 to 3, so that one of a source's four connections is left for everything else",
                self.streams_per_source
            ));
        }
        if let Some(url) = &self.public_url {
            let parsed = url::Url::parse(url)
                .map_err(|e| format!("[server.web] public_url {url:?} is not an address: {e}"))?;
            if parsed.scheme() != "https"
                || parsed.host_str().is_none()
                || parsed.path() != "/"
                || parsed.query().is_some()
                || parsed.fragment().is_some()
            {
                return Err(format!(
                    "[server.web] public_url {url:?} must be an https address with just a host and a port, like https://display.example.home:8443"
                ));
            }
        }
        if let Some(screen) = &self.screen {
            if !self.enabled {
                return Err(
                    "[server.web.screen] is set but the page is off: set [server.web] enabled = true, or take it out".to_owned(),
                );
            }
            if !(320..=3840).contains(&screen.width) || !(240..=2160).contains(&screen.height) {
                return Err(format!(
                    "[server.web.screen] is {}x{}; the width must be from 320 to 3840 and the height from 240 to 2160",
                    screen.width, screen.height
                ));
            }
        }
        for name in &self.layouts {
            crate::domain::models::layout::LayoutName::parse(name)
                .map_err(|e| format!("[server.web] layouts has {name:?}: {e}"))?;
        }
        for address in &self.trusted_proxies {
            address.parse::<std::net::IpAddr>().map_err(|e| {
                format!(
                    "[server.web] trusted_proxies has {address:?}, which is not an IP address: {e}"
                )
            })?;
        }
        Ok(())
    }
}

impl ServerConfig {
    /// Where the pictures drawn for the page are kept, when it has a screen of its own: beside the panel's, in a folder whose
    /// name (it starts with `_`) no layout can have, so that the two never meet.
    pub fn web_images_directory(&self) -> PathBuf {
        self.directory.join("_web")
    }

    /// Where the admin socket is: as configured, or `admin.sock` beside the authority.
    pub fn admin_socket(&self) -> PathBuf {
        self.admin
            .socket
            .clone()
            .unwrap_or_else(|| self.tls.directory.join("admin.sock"))
    }

    /// Whether the settings make sense together, in words for whoever wrote them.
    pub fn check(&self) -> Result<(), String> {
        self.web.check()?;
        if self.web.enabled && !self.web.open {
            if !self.transport.serves_https() {
                return Err(
                    "[server.web] needs open = true with transport = \"http\": a viewer token is only ever sent over HTTPS, so there is no way to use one"
                        .to_owned(),
                );
            }
            if !self.admin.enabled {
                return Err(
                    "[server.web] without open = true needs viewer tokens, which are issued through the admin interface, which is turned off ([server.admin] enabled = false)"
                        .to_owned(),
                );
            }
        }
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
        for (name, value, most) in [
            (
                "server_certificate_days",
                tls.server_certificate_days,
                MAX_CERTIFICATE_DAYS,
            ),
            (
                "device_certificate_days",
                tls.device_certificate_days,
                MAX_CERTIFICATE_DAYS,
            ),
            (
                "pairing_retry_minutes",
                tls.pairing_retry_minutes,
                MAX_PAIRING_RETRY_MINUTES,
            ),
        ] {
            if !(1..=most).contains(&value) {
                return Err(format!(
                    "[server.tls] {name} is {value}; it must be from 1 to {most}"
                ));
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
        let named = parse("[tls]\nnames = [\"display.example.net\", \"192.168.1.5\"]").unwrap();
        assert_eq!(
            named.certificate_names(),
            ["display.example.net", "192.168.1.5"]
        );
    }

    #[test]
    fn lifetimes_and_waits_must_be_from_one_to_their_most() {
        for (key, most) in [
            ("server_certificate_days", MAX_CERTIFICATE_DAYS),
            ("device_certificate_days", MAX_CERTIFICATE_DAYS),
            ("pairing_retry_minutes", MAX_PAIRING_RETRY_MINUTES),
        ] {
            // The most is allowed; one more is not; nor is a figure large enough to overflow a date.
            let at_most = parse(&format!("transport = \"https\"\n[tls]\n{key} = {most}")).unwrap();
            assert!(at_most.check().is_ok(), "{key}");
            for bad in [most + 1, u32::MAX] {
                let config =
                    parse(&format!("transport = \"https\"\n[tls]\n{key} = {bad}")).unwrap();
                let error = config.check().unwrap_err();
                assert!(
                    error.contains(key)
                        && error.contains(&bad.to_string())
                        && error.contains(&most.to_string()),
                    "{key} = {bad}: {error}"
                );
            }
        }
        // Unused settings are still not judged.
        assert!(
            parse("[tls]\ndevice_certificate_days = 4000000000")
                .unwrap()
                .check()
                .is_ok()
        );
    }

    #[test]
    fn the_web_view_is_off_unless_turned_on_and_then_needs_a_token_unless_open() {
        let config = parse("").unwrap();
        assert!(!config.web.enabled && !config.web.open && config.web.layouts.is_empty());
        assert!(config.check().is_ok());

        let open = parse("[web]\nenabled = true\nopen = true\nlayouts = [\"departures\"]").unwrap();
        assert!(open.check().is_ok(), "open works over plain http");

        // Tokens travel only over HTTPS, so they need it, and the admin interface to be issued.
        let plain = parse("[web]\nenabled = true").unwrap();
        let error = plain.check().unwrap_err();
        assert!(
            error.contains("[server.web]") && error.contains("HTTPS"),
            "{error}"
        );
        for transport in ["prefer-https", "https"] {
            let tokens = parse(&format!(
                "transport = \"{transport}\"\n[web]\nenabled = true"
            ))
            .unwrap();
            assert!(tokens.check().is_ok(), "{transport}");
            let no_admin = parse(&format!(
                "transport = \"{transport}\"\n[web]\nenabled = true\n[admin]\nenabled = false"
            ))
            .unwrap();
            let error = no_admin.check().unwrap_err();
            assert!(error.contains("admin"), "{transport}: {error}");
            // Open needs neither.
            let open_no_admin = parse(&format!(
                "transport = \"{transport}\"\n[web]\nenabled = true\nopen = true\n[admin]\nenabled = false"
            ))
            .unwrap();
            assert!(open_no_admin.check().is_ok(), "{transport}");
        }
        // Off, none of it matters.
        assert!(parse("[admin]\nenabled = false").unwrap().check().is_ok());
    }

    #[test]
    fn a_public_url_is_an_https_address_with_nothing_after_the_host_and_port() {
        for good in [
            "https://display.local:8443",
            "https://display.local",
            "https://192.168.1.5:8443/",
        ] {
            let config = parse(&format!("[web]\npublic_url = {good:?}")).unwrap();
            assert!(config.check().is_ok(), "{good}");
        }
        for bad in [
            "http://display.local:8443",
            "display.local",
            "https://display.local/view",
            "https://display.local/?x=1",
            "https://display.local/#frag",
            "",
        ] {
            let config = parse(&format!("[web]\npublic_url = {bad:?}")).unwrap();
            let error = config.check().unwrap_err();
            assert!(error.contains("public_url"), "{bad}: {error}");
        }
    }

    #[test]
    fn a_layout_name_in_the_web_settings_that_cannot_be_one_is_refused() {
        for bad in ["Bad Name", "../etc", ""] {
            let config = parse(&format!(
                "[web]\nenabled = true\nopen = true\nlayouts = [{bad:?}]"
            ))
            .unwrap();
            let error = config.check().unwrap_err();
            assert!(error.contains("[server.web] layouts"), "{bad}: {error}");
        }
    }

    #[test]
    fn the_access_token_time_for_refreshing_viewers_has_a_default_and_a_range() {
        assert_eq!(parse("").unwrap().web.access_token_minutes, 60);
        let set = parse("[web]\naccess_token_minutes = 15").unwrap();
        assert_eq!(set.web.access_token_minutes, 15);
        assert!(set.check().is_ok());
        for bad in [0, 1441] {
            let config = parse(&format!("[web]\naccess_token_minutes = {bad}")).unwrap();
            let error = config.check().unwrap_err();
            assert!(
                error.contains("access_token_minutes") && error.contains("1440"),
                "{bad}: {error}"
            );
        }
    }

    #[test]
    fn a_screen_for_the_page_has_defaults_a_range_and_needs_the_page_on() {
        // Nothing, no screen: the page shows the panel's pictures.
        assert!(parse("").unwrap().web.screen.is_none());

        let full_hd = parse("[web]\nenabled = true\nopen = true\n[web.screen]").unwrap();
        let screen = full_hd.web.screen.as_ref().unwrap();
        assert_eq!((screen.width, screen.height), (1920, 1080));
        assert!(full_hd.check().is_ok());
        let own =
            parse("[web]\nenabled = true\nopen = true\n[web.screen]\nwidth = 1280\nheight = 800")
                .unwrap();
        assert_eq!(own.web.screen.as_ref().unwrap().width, 1280);
        assert!(own.check().is_ok());

        for (width, height) in [(319, 1080), (3841, 1080), (1920, 239), (1920, 2161), (0, 0)] {
            let bad = parse(&format!(
                "[web]\nenabled = true\nopen = true\n[web.screen]\nwidth = {width}\nheight = {height}"
            ))
            .unwrap();
            let error = bad.check().unwrap_err();
            assert!(
                error.contains("[server.web.screen]") && error.contains("3840"),
                "{width}x{height}: {error}"
            );
        }
        // A screen with the page off is a mistake worth saying so, not a thing quietly ignored.
        let off = parse("[web]\nopen = true\n[web.screen]").unwrap();
        assert!(off.check().unwrap_err().contains("page is off"));
    }

    #[test]
    fn the_pages_own_pictures_are_kept_where_no_layout_can_be() {
        let config = parse("directory = \"/srv/display\"").unwrap();
        let kept = config.web_images_directory();

        assert_eq!(kept, std::path::PathBuf::from("/srv/display/_web"));
        // A layout name is lower-case letters, digits and hyphens: it can never be `_web`.
        assert!(crate::domain::models::layout::LayoutName::parse("_web").is_err());
    }

    #[test]
    fn the_streams_a_page_may_hold_have_defaults_and_leave_connections_for_everything_else() {
        let config = parse("").unwrap();
        assert_eq!((config.web.streams, config.web.streams_per_source), (8, 2));
        let set = parse("[web]\nstreams = 4\nstreams_per_source = 1").unwrap();
        assert_eq!((set.web.streams, set.web.streams_per_source), (4, 1));
        assert!(set.check().is_ok());

        for (streams, per_source) in [(0, 2), (16, 2), (8, 0), (8, 4)] {
            let bad = parse(&format!(
                "[web]\nstreams = {streams}\nstreams_per_source = {per_source}"
            ))
            .unwrap();
            let error = bad.check().unwrap_err();
            assert!(
                error.contains("[server.web] streams"),
                "{streams}/{per_source}: {error}"
            );
        }
    }

    #[test]
    fn trusted_proxies_are_addresses() {
        assert!(parse("").unwrap().web.trusted_proxies.is_empty());
        let set = parse("[web]\ntrusted_proxies = [\"192.168.1.2\", \"fd00::2\"]").unwrap();
        assert!(set.check().is_ok());
        let bad = parse("[web]\ntrusted_proxies = [\"proxy.home\"]").unwrap();
        assert!(bad.check().unwrap_err().contains("trusted_proxies"));
    }
}
