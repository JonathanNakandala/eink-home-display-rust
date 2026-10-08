//! A real server for the firmware's tests on a computer, and for nothing else.
//!
//! It runs what the program runs when it serves HTTPS only (the certificate authority, EST, the admin socket), with
//! stand-ins for the display routes, so a client built on the same TLS library as the display can be run against it:
//! `esphome/host/`. It makes a fresh authority in the directory it is given, opens the pairing window and keeps it
//! open, and says where everything is on its first line of output.
//!
//!   cargo run --example est_fixture -- <directory> [port] [require-binding]
//!
//! The first line is `READY https=<address> admin=<socket>`, for a script to read. Approve a display with
//! `displayctl --socket <socket> approve <name> <code>`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use chrono::Duration;
use chrono_tz::Tz;
use eink_home_display_rust::adapters::authenticated::AuthenticatedDevice;
use eink_home_display_rust::adapters::clock::SystemClock;
use eink_home_display_rust::bootstrap::{open_security, start_serving};
use eink_home_display_rust::config::server::ServerConfig;
use eink_home_display_rust::domain::models::display::ImageFormat;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let directory = PathBuf::from(args.next().ok_or_else(|| anyhow::anyhow!("a directory"))?);
    let port: u16 = args.next().map(|p| p.parse()).transpose()?.unwrap_or(0);
    let require_binding = args.next().is_some_and(|flag| flag == "require-binding");

    // The admin socket is served only from a directory no other user can get into.
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }

    let text = format!(
        r#"
        transport = "https"
        advertise = false
        [tls]
        bind = "127.0.0.1:{port}"
        directory = {directory:?}
        names = ["localhost", "127.0.0.1"]
        require_channel_binding = {require_binding}
        "#
    );
    let config: ServerConfig = ::config::Config::builder()
        .add_source(::config::File::from_str(&text, ::config::FileFormat::Toml))
        .build()?
        .try_deserialize()?;

    let clock = Arc::new(SystemClock::new(Tz::UTC));
    let security = open_security(&config, clock, true)
        .await?
        .ok_or_else(|| anyhow::anyhow!("HTTPS is served"))?;
    let handshakes = security.handshakes.clone();

    // Who is asking, as the server knows it from the certificate; how the TLS connections began; a picture.
    let display = Router::new()
        .route("/image", get(|| async { "image-bytes" }))
        .route(
            "/who",
            get(
                |who: Option<axum::Extension<AuthenticatedDevice>>| async move {
                    who.map(|w| w.0.0.to_string())
                        .unwrap_or_else(|| "anonymous".to_owned())
                },
            ),
        )
        .route(
            "/handshakes",
            get(move || {
                let handshakes = handshakes.clone();
                async move {
                    let (full, resumed) = handshakes.counts();
                    format!("full={full} resumed={resumed}")
                }
            }),
        );

    let listening = start_serving(&config, display, ImageFormat::Png, Some(security)).await?;
    if let Some(enrollment) = &listening.enrollment {
        enrollment.open_window(Duration::days(1));
    }
    println!(
        "READY https={} admin={}",
        listening.https.map(|a| a.to_string()).unwrap_or_default(),
        listening
            .admin
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    );
    listening.run().await
}
