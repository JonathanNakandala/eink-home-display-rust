use std::net::SocketAddr;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const DEFAULT_BIND: &str = "0.0.0.0:8080";
const DEFAULT_DIRECTORY: &str = "served";

/// The HTTP server that hands the latest rendered image to displays that fetch it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ServerConfig {
    /// Address to listen on. The image is served at `/image`, with no authentication, so keep it on the LAN.
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
    /// Where the image to serve is written. Relative paths are resolved against the working directory.
    #[serde(default = "default_directory")]
    pub directory: PathBuf,
}

fn default_bind() -> SocketAddr {
    DEFAULT_BIND.parse().expect("default bind address is valid")
}

fn default_directory() -> PathBuf {
    PathBuf::from(DEFAULT_DIRECTORY)
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { bind: default_bind(), directory: default_directory() }
    }
}
