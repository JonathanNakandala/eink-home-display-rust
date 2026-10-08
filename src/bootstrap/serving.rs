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

use crate::adapters::certificate_authority::{self, Create, PrivateAuthority};
use crate::adapters::est_server::{Access, EstServer, EstSettings, guarded};
use crate::adapters::image_server::{self, SecureOffer, ServerSettings};
use crate::adapters::pairing_store::{FilePairingStore, Missing};
use crate::application::enrollment::{Enrollment, EnrollmentPolicy};
use crate::config::server::{ServerConfig, Transport};
use crate::domain::models::display::ImageFormat;
use crate::domain::services::certificate_authority::CertificateAuthority;
use crate::domain::services::clock::Clock;

/// The authority's certificate ending within this long is worth a warning at start-up: nothing yet
/// carries displays across a change of authority, so every one would have to be paired again.
const AUTHORITY_WARNING: Duration = Duration::days(2 * 365);

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

/// The certificate authority and the rules for joining it, when the transport uses them.
#[derive(Clone)]
pub struct Security {
    pub authority: Arc<PrivateAuthority>,
    pub enrollment: Arc<Enrollment>,
}

/// Opens the authority and the list of displays if `config.transport` uses them, so they can be given to
/// whatever reports on them before the listeners start.
///
/// `init_pki` is the owner saying that this is the time to make what is missing: the certificate
/// authority, and an empty list of displays. Without it, `https` makes nothing and says so (see
/// `open_authority`).
pub async fn open_security(
    config: &ServerConfig,
    clock: Arc<dyn Clock>,
    init_pki: bool,
) -> anyhow::Result<Option<Security>> {
    if !config.transport.serves_https() {
        return Ok(None);
    }
    let (authority, enrollment) = open_authority(config, clock, init_pki).await?;
    Ok(Some(Security {
        authority,
        enrollment,
    }))
}

/// Opens what `config.transport` calls for, with `app` as the display routes. `security` is what
/// `open_security` returned, which must be something when the transport serves HTTPS.
pub async fn start(
    config: &ServerConfig,
    app: Router,
    format: ImageFormat,
    security: Option<Security>,
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

    let Security {
        authority,
        enrollment,
    } = security.context("HTTPS is served but the authority was not opened")?;
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
///
/// What is missing is made only where that does no harm. A new authority, and a new list, lock out every
/// display that holds a certificate from the old ones, and a missing directory is as often a volume
/// that was not mounted as a first start. So:
/// - the authority is made on a first start with `prefer-https` (displays carry on over plain HTTP
///   meanwhile), and with `https` only when asked with `init_pki`;
/// - a list of displays is started empty only beside a new authority, or when asked. With an authority
///   already there it is a lost file, and an error.
async fn open_authority(
    config: &ServerConfig,
    clock: Arc<dyn Clock>,
    init_pki: bool,
) -> anyhow::Result<(Arc<PrivateAuthority>, Arc<Enrollment>)> {
    let tls = &config.tls;
    let authority_there = tls.directory.join("authority.pem").exists()
        || tls.directory.join("authority.key").exists();
    let create = if init_pki || config.transport == Transport::PreferHttps {
        Create::IfMissing
    } else {
        Create::Never
    };
    let authority = Arc::new(
        certificate_authority::open(&tls.directory, create).with_context(|| {
            format!(
                "Failed to open the certificate authority in {}. With transport = \"https\" one is not made \
                 automatically: a missing directory is usually a volume that was not mounted, and a new \
                 authority would lock out every display. If this is the first start, run once with --init-pki",
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
    let missing = if init_pki || !authority_there {
        Missing::StartEmpty
    } else {
        Missing::Refuse
    };
    let store = FilePairingStore::open(&tls.directory, missing)
        .await
        .with_context(|| {
            format!(
                "Failed to open the list of displays in {}. The authority is there, so the list is a file \
                 that was lost: restore it from a backup. To start with no displays instead, run once \
                 with --init-pki",
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
        Harness, Identity, Trust, client_config, connect_with, enroll, join, new_key, send,
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

    /// A start with `--init-pki`, as a first start is.
    async fn listening(config: &ServerConfig) -> anyhow::Result<Listening> {
        started(config, true).await
    }

    /// A start without it, as every later one is.
    async fn restarted(config: &ServerConfig) -> anyhow::Result<Listening> {
        started(config, false).await
    }

    async fn started(config: &ServerConfig, init_pki: bool) -> anyhow::Result<Listening> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock::new(Tz::UTC));
        let security = open_security(config, clock, init_pki).await?;
        start(config, stand_in_display(), ImageFormat::Bmp, security).await
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
            restarted(&config(Transport::Https, pki.path()))
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

    #[tokio::test]
    async fn https_only_makes_nothing_in_an_empty_directory_unless_asked() {
        // Most often an empty directory is a volume that was not mounted, and a new authority would lock
        // out every display that holds a certificate from the real one.
        let parent = tempfile::tempdir().unwrap();
        let pki = parent.path().join("pki");
        let error = format!(
            "{:#}",
            restarted(&config(Transport::Https, &pki))
                .await
                .err()
                .expect("should refuse")
        );
        assert!(
            error.contains("--init-pki") && error.contains("volume"),
            "{error}"
        );
        assert!(!pki.exists(), "nothing was made");

        // Asked to, it makes both the authority and an empty list.
        listening(&config(Transport::Https, &pki)).await.unwrap();
        assert!(pki.join("authority.pem").exists() && pki.join("pairings.json").exists());
    }

    #[tokio::test]
    async fn prefer_https_makes_the_authority_on_a_first_start_without_being_asked() {
        // Displays carry on over plain HTTP meanwhile, so there is nothing to lock out.
        let pki = tempfile::tempdir().unwrap();
        restarted(&config(Transport::PreferHttps, pki.path()))
            .await
            .unwrap();
        assert!(pki.path().join("authority.pem").exists());
        assert!(pki.path().join("pairings.json").exists());
    }

    #[tokio::test]
    async fn an_authority_without_its_list_is_an_error_unless_asked() {
        let pki = tempfile::tempdir().unwrap();
        listening(&config(Transport::Https, pki.path()))
            .await
            .unwrap();
        std::fs::remove_file(pki.path().join("pairings.json")).unwrap();

        for transport in [Transport::PreferHttps, Transport::Https] {
            let error = format!(
                "{:#}",
                restarted(&config(transport, pki.path()))
                    .await
                    .err()
                    .expect("should refuse")
            );
            assert!(
                error.contains("list of displays") && error.contains("lost"),
                "{error}"
            );
            assert!(
                !pki.path().join("pairings.json").exists(),
                "nothing was made"
            );
        }
        // Asking for it starts an empty list, knowing that is what it does.
        listening(&config(Transport::Https, pki.path()))
            .await
            .unwrap();
        assert!(pki.path().join("pairings.json").exists());
    }

    #[tokio::test]
    async fn a_lost_list_is_restored_from_its_backup_and_the_display_finds_its_own_way_back() {
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

        // The list is deleted, and the server will not start with none.
        std::fs::remove_file(pki.path().join("pairings.json")).unwrap();
        let error = format!(
            "{:#}",
            restarted(&config(Transport::Https, pki.path()))
                .await
                .err()
                .expect("should refuse")
        );
        assert!(error.contains(".bak"), "{error}");

        // Copying the backup back is all it takes. It is the version before the last change, so the
        // display is not yet a member again, but it asks with the key it holds and is one, with no one
        // at the server.
        std::fs::copy(
            pki.path().join("pairings.json.bak"),
            pki.path().join("pairings.json"),
        )
        .unwrap();
        std::fs::remove_file(pki.path().join("pairings.json.bak")).unwrap();
        let second = harness_for(
            restarted(&config(Transport::Https, pki.path()))
                .await
                .unwrap(),
            tempfile::tempdir().unwrap(),
        );
        assert_eq!(secure_get(&second, "/image", Some(&kitchen)).await.0, 403);
        let authority = second.authority.certificate().to_vec();
        let again = enroll(
            &second,
            Trust::Authority(&authority),
            "kitchen",
            &kitchen.key,
        )
        .await;
        let renewed = Identity {
            certificate: again.certificates().remove(0),
            key: kitchen.key,
        };
        assert_eq!(secure_get(&second, "/image", Some(&renewed)).await.0, 200);
    }
}
