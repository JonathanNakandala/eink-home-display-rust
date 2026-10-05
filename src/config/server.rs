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
        }
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
