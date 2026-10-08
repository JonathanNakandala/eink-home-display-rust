//! Opens the listeners the configured transport calls for, and ties them together.
//!
//! - `http`: the plain listener, as it has always been. No authority, no TLS, nothing written to disk.
//! - `prefer-https`: the plain listener, and an HTTPS one beside it serving the same display routes
//!   and the way in (EST). Over HTTPS a display with a certificate is known by it; one without is
//!   served as over HTTP. The HTTPS port is announced over mDNS.
//! - `https`: only HTTPS. The display routes need a certificate of the authority; only the way in is open.
//!
//! The listeners are opened first and run afterwards, so a port that is taken, or an authority that
//! can't be read, stops the program at start-up and not later, and so the addresses are known.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use chrono::Duration;

use crate::adapters::certificate_authority::{self, PrivateAuthority};
use crate::adapters::est_server::{Access, EstServer, EstSettings, guarded};
use crate::adapters::image_server::{self, SecureOffer, ServerSettings};
use crate::adapters::pairing_store::FilePairingStore;
use crate::application::enrollment::{Enrollment, EnrollmentPolicy};
use crate::config::server::ServerConfig;
use crate::domain::models::display::ImageFormat;
use crate::domain::services::certificate_authority::CertificateAuthority;
use crate::domain::services::clock::Clock;

/// The authority's certificate ending within this long is worth saying so at start-up: a display
/// that is not paired again by then stops being served.
const AUTHORITY_WARNING: Duration = Duration::days(365);

/// Listeners that are open and not yet serving.
pub struct Listening {
    /// Where plain HTTP is served, if it is.
    pub http: Option<SocketAddr>,
    /// Where HTTPS is served, if it is.
    pub https: Option<SocketAddr>,
    /// For deciding who may join, when HTTPS is served.
    pub enrollment: Option<Arc<Enrollment>>,
    /// The certificate authority, when HTTPS is served.
    pub authority: Option<Arc<PrivateAuthority>>,
    /// The HTTPS announced over mDNS beside the service, when HTTPS is served.
    pub secure: Option<SecureOffer>,
    serving: Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>,
}

impl Listening {
    /// Serves until the future is dropped, or one of the listeners fails.
    pub async fn run(self) -> anyhow::Result<()> {
        self.serving.await
    }
}

/// Opens what `config.transport` calls for, with `app` as the display routes.
pub async fn start(
    config: &ServerConfig,
    app: Router,
    format: ImageFormat,
    clock: Arc<dyn Clock>,
) -> anyhow::Result<Listening> {
    let settings = ServerSettings::from(config);
    log::info!("Displays reach this server by: {}", config.transport.name());
    if !config.transport.serves_https() {
        let http = image_server::bind_http(&settings)?;
        let address = http.local_addr()?;
        return Ok(Listening {
            http: Some(address),
            https: None,
            enrollment: None,
            authority: None,
            secure: None,
            serving: Box::pin(async move { http.serve(&settings, app, format, None).await }),
        });
    }

    let (authority, enrollment) = open_authority(config, clock).await?;
    let est_settings = EstSettings {
        bind: config.tls.bind,
        names: config.certificate_names(),
        certificate_lifetime: Duration::days(config.tls.server_certificate_days.into()),
        ..EstSettings::default()
    };

    if config.transport.serves_http() {
        // Both. The plain side serves as always; the HTTPS side adds the certificate to who is asking.
        let http = image_server::bind_http(&settings)?;
        let secure_app = guarded(app.clone(), enrollment.clone(), Access::Open);
        let https = EstServer::bind(
            est_settings,
            authority.clone(),
            enrollment.clone(),
            Some(secure_app),
        )?;
        let (http_address, https_address) = (http.local_addr()?, https.local_addr()?);
        let offer = SecureOffer {
            port: https_address.port(),
            required: false,
        };
        return Ok(Listening {
            http: Some(http_address),
            https: Some(https_address),
            enrollment: Some(enrollment),
            authority: Some(authority),
            secure: Some(offer),
            serving: Box::pin(async move {
                tokio::select! {
                    result = http.serve(&settings, app, format, Some(offer)) => result,
                    result = https.run() => result,
                }
            }),
        });
    }

    // HTTPS only. Nothing listens in plain, so the announcement points straight at the HTTPS port.
    let secure_app = guarded(app, enrollment.clone(), Access::Members);
    let https = EstServer::bind(
        est_settings,
        authority.clone(),
        enrollment.clone(),
        Some(secure_app),
    )?;
    let address = https.local_addr()?;
    let mut announced = settings;
    announced.bind = config.tls.bind;
    let (families, port) = (https.families(), address.port());
    let offer = SecureOffer {
        port,
        required: true,
    };
    Ok(Listening {
        http: None,
        https: Some(address),
        enrollment: Some(enrollment),
        authority: Some(authority),
        secure: Some(offer),
        serving: Box::pin(async move {
            let _advertisement =
                image_server::announce(&announced, port, format, families, Some(offer));
            https.run().await
        }),
    })
}

/// Opens the authority and the list of displays, and the rules for joining, from `config`.
async fn open_authority(
    config: &ServerConfig,
    clock: Arc<dyn Clock>,
) -> anyhow::Result<(Arc<PrivateAuthority>, Arc<Enrollment>)> {
    let tls = &config.tls;
    let authority = Arc::new(
        certificate_authority::open(&tls.directory).with_context(|| {
            format!(
                "Failed to open the certificate authority in {}",
                tls.directory.display()
            )
        })?,
    );
    let remaining = authority.not_after() - clock.now().to_utc();
    log::info!(
        "Certificate authority {} (valid until {})",
        authority.fingerprint(),
        authority.not_after().format("%Y-%m-%d")
    );
    if remaining < AUTHORITY_WARNING {
        log::warn!(
            "The certificate authority ends on {}; every display has to be paired again by then",
            authority.not_after().format("%Y-%m-%d")
        );
    }
    let store = FilePairingStore::open(&tls.directory)
        .await
        .with_context(|| {
            format!(
                "Failed to open the list of displays in {}",
                tls.directory.display()
            )
        })?;
    let enrollment = Arc::new(Enrollment::new(
        authority.clone(),
        Arc::new(store),
        clock,
        EnrollmentPolicy {
            certificate_lifetime: Duration::days(tls.device_certificate_days.into()),
            retry_after: Duration::minutes(tls.pairing_retry_minutes.into()),
            require_channel_binding: tls.require_channel_binding,
        },
    ));
    Ok((authority, enrollment))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono_tz::Tz;
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::*;
    use crate::adapters::clock::SystemClock;
    use crate::adapters::est_server::tests::{
        Harness, Identity, Trust, client_config, connect_with, join, new_key, send,
        stand_in_display,
    };
    use crate::config::server::{TlsConfig, Transport};

    fn config(transport: Transport, pki: &Path) -> ServerConfig {
        ServerConfig {
            transport,
            bind: "127.0.0.1:0".parse().unwrap(),
            advertise: false,
            tls: TlsConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                directory: pki.to_owned(),
                names: vec!["localhost".to_owned()],
                ..TlsConfig::default()
            },
            ..ServerConfig::default()
        }
    }

    async fn listening(config: &ServerConfig) -> anyhow::Result<Listening> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock::new(Tz::UTC));
        start(config, stand_in_display(), ImageFormat::Bmp, clock).await
    }

    /// What plain HTTP answers to a GET of `path`: the status and the body.
    async fn plain_get(address: SocketAddr, path: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(address).await.unwrap();
        let request = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        let _ = stream.read_to_end(&mut raw).await;
        let text = String::from_utf8_lossy(&raw).into_owned();
        let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
        let body = text
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or("")
            .trim()
            .to_owned();
        (status, body)
    }

    /// What HTTPS answers, from a client that shows `identity` or nothing (and trusts what it is shown,
    /// as a display that has not yet fetched the authority does).
    async fn secure_get(
        harness: &Harness,
        path: &str,
        identity: Option<&Identity>,
    ) -> (u16, String) {
        let config = client_config(Trust::Anything, identity, &[&rustls::version::TLS13]);
        let mut stream = connect_with(harness, config)
            .await
            .expect("a TLS connection");
        let response = send(&mut stream, "GET", path, None, b"").await;
        (response.status, response.text().trim().to_owned())
    }

    /// The HTTPS side of a running server as the EST tests' harness, to join displays with.
    fn secure_side(listening: &Listening) -> (SocketAddr, Arc<Enrollment>, Arc<PrivateAuthority>) {
        (
            listening.https.expect("HTTPS is served"),
            listening.enrollment.clone().expect("an enrollment"),
            listening.authority.clone().expect("an authority"),
        )
    }

    fn harness_for(listening: Listening, pki: TempDir) -> Harness {
        let (address, enrollment, authority) = secure_side(&listening);
        Harness {
            address,
            enrollment,
            authority,
            server: tokio::spawn(listening.run()),
            _directory: pki,
        }
    }

    #[tokio::test]
    async fn plain_http_opens_one_listener_and_writes_nothing_to_disk() {
        let pki = tempfile::tempdir().unwrap();
        let state = pki.path().join("state");
        let listening = listening(&config(Transport::Http, &state)).await.unwrap();
        assert!(
            listening.https.is_none()
                && listening.enrollment.is_none()
                && listening.authority.is_none()
        );
        assert_eq!(listening.secure, None, "plain HTTP announces no HTTPS");
        let http = listening.http.expect("plain HTTP is served");
        let server = tokio::spawn(listening.run());
        assert_eq!(
            plain_get(http, "/image").await,
            (200, "image-bytes".to_owned())
        );
        assert!(!state.exists(), "no authority is made when HTTPS is off");
        server.abort();
    }

    #[tokio::test]
    async fn prefer_https_serves_both_and_a_member_is_known_by_its_certificate() {
        let pki = tempfile::tempdir().unwrap();
        let listening = listening(&config(Transport::PreferHttps, pki.path()))
            .await
            .unwrap();
        let http = listening.http.expect("plain HTTP is served");
        assert!(listening.https.is_some());
        assert_ne!(http.port(), listening.https.unwrap().port());
        // Announced beside the plain service, saying plain is still there.
        assert_eq!(
            listening.secure,
            Some(SecureOffer {
                port: listening.https.unwrap().port(),
                required: false
            })
        );
        let harness = harness_for(listening, pki);

        // Plain HTTP carries on as before, for every display.
        assert_eq!(
            plain_get(http, "/image").await,
            (200, "image-bytes".to_owned())
        );
        assert_eq!(plain_get(http, "/who").await, (200, "anonymous".to_owned()));
        // HTTPS serves the same routes to a display with no certificate yet, and the way in.
        assert_eq!(
            secure_get(&harness, "/image", None).await,
            (200, "image-bytes".to_owned())
        );
        assert_eq!(
            secure_get(&harness, "/.well-known/est/cacerts", None)
                .await
                .0,
            200
        );

        // A display that joins is then known by its certificate.
        let kitchen = join(&harness, "kitchen", new_key()).await;
        assert_eq!(
            secure_get(&harness, "/who", Some(&kitchen)).await,
            (200, "kitchen".to_owned())
        );
        // Revoking it turns it away on HTTPS, and does nothing to the plain port, which was open anyway.
        harness
            .enrollment
            .revoke(&crate::domain::models::device_id::DeviceId::parse("kitchen").unwrap())
            .await
            .unwrap();
        assert_eq!(secure_get(&harness, "/who", Some(&kitchen)).await.0, 403);
        assert_eq!(plain_get(http, "/who").await, (200, "anonymous".to_owned()));
    }

    #[tokio::test]
    async fn https_only_opens_no_plain_listener_and_serves_members_alone() {
        let pki = tempfile::tempdir().unwrap();
        let listening = listening(&config(Transport::Https, pki.path()))
            .await
            .unwrap();
        assert!(listening.http.is_none(), "nothing listens in plain");
        // Announced as the only way, on the HTTPS port.
        assert_eq!(
            listening.secure,
            Some(SecureOffer {
                port: listening.https.unwrap().port(),
                required: true
            })
        );
        let harness = harness_for(listening, pki);

        let refused = secure_get(&harness, "/image", None).await;
        assert_eq!(refused.0, 403);
        assert!(!refused.1.contains("image-bytes"));
        // The way in is open, or nothing could join.
        assert_eq!(
            secure_get(&harness, "/.well-known/est/cacerts", None)
                .await
                .0,
            200
        );

        let kitchen = join(&harness, "kitchen", new_key()).await;
        assert_eq!(
            secure_get(&harness, "/image", Some(&kitchen)).await,
            (200, "image-bytes".to_owned())
        );
        harness
            .enrollment
            .revoke(&crate::domain::models::device_id::DeviceId::parse("kitchen").unwrap())
            .await
            .unwrap();
        assert_eq!(secure_get(&harness, "/image", Some(&kitchen)).await.0, 403);
    }

    #[tokio::test]
    async fn what_was_paired_is_still_paired_after_a_restart() {
        let pki = tempfile::tempdir().unwrap();
        let first = harness_for(
            listening(&config(Transport::Https, pki.path()))
                .await
                .unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let kitchen = join(&first, "kitchen", new_key()).await;
        assert_eq!(secure_get(&first, "/image", Some(&kitchen)).await.0, 200);
        drop(first);

        // The same directory, a new process's worth of state: same authority, same members.
        let second = harness_for(
            listening(&config(Transport::Https, pki.path()))
                .await
                .unwrap(),
            tempfile::tempdir().unwrap(),
        );
        assert_eq!(
            secure_get(&second, "/image", Some(&kitchen)).await,
            (200, "image-bytes".to_owned())
        );
    }

    #[tokio::test]
    async fn a_port_that_is_taken_stops_start_up_not_later() {
        let pki = tempfile::tempdir().unwrap();
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut taken_http = config(Transport::PreferHttps, pki.path());
        taken_http.bind = taken.local_addr().unwrap();
        assert!(listening(&taken_http).await.is_err());
        let mut taken_https = config(Transport::Https, pki.path());
        taken_https.tls.bind = taken.local_addr().unwrap();
        assert!(listening(&taken_https).await.is_err());
    }

    #[tokio::test]
    async fn a_certificate_name_the_authority_cannot_use_stops_start_up() {
        let pki = tempfile::tempdir().unwrap();
        let mut bad = config(Transport::Https, pki.path());
        bad.tls.names = vec!["has space".to_owned()];
        let error = format!("{:#}", listening(&bad).await.err().expect("should refuse"));
        assert!(error.contains("has space"), "{error}");
    }

    #[tokio::test]
    async fn an_unreadable_authority_stops_start_up_and_is_left_alone() {
        let pki = tempfile::tempdir().unwrap();
        std::fs::write(pki.path().join("authority.pem"), "not a certificate").unwrap();
        std::fs::write(pki.path().join("authority.key"), "not a key").unwrap();
        let error = format!(
            "{:#}",
            listening(&config(Transport::Https, pki.path()))
                .await
                .err()
                .expect("should refuse")
        );
        assert!(error.contains("authority"), "{error}");
        assert_eq!(
            std::fs::read_to_string(pki.path().join("authority.key")).unwrap(),
            "not a key"
        );
    }
}
