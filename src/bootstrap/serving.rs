//! Opens the listeners the configured transport calls for, and ties them together.
//!
//! - `http`: the plain listener, as it has always been. No authority, no TLS, nothing written to disk.
//! - `prefer-https`: the plain listener, and an HTTPS one beside it serving the same display routes
//!   and the way in (EST). Over HTTPS a display with a certificate is known by it; one without is
//!   served as over HTTP. The HTTPS port is announced over mDNS, beside the `_http._tcp` service.
//! - `https`: only HTTPS. The display routes need a certificate of the authority; only the way in is open.
//!   Announced as `_https._tcp` on the HTTPS port, never as `_http._tcp`.
//!
//! The listeners are opened first and run afterwards, so a port that is taken, or an authority that
//! can't be read, stops the program at start-up and not later, and so the addresses are known.

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use chrono::Duration;

use crate::adapters::admin;
use crate::adapters::certificate_authority::{self, Create, PrivateAuthority};
use crate::adapters::entropy::OsEntropy;
use crate::adapters::est_server::{Access, EstServer, EstSettings, guarded};
use crate::adapters::image_server::{self, SecureOffer, ServerSettings};
use crate::adapters::layout_choices::FileLayoutChoices;
use crate::adapters::pairing_store::{FilePairingStore, Missing};
use crate::adapters::viewer_registry::FileViewerRegistry;
use crate::application::enrollment::{Enrollment, EnrollmentPolicy};
use crate::application::handshakes::Handshakes;
use crate::application::viewers::Viewers;
use crate::config::server::{ServerConfig, Transport};
use crate::domain::models::display::ImageFormat;
use crate::domain::models::layout::{LayoutName, Layouts};
use crate::domain::models::schedule::Schedule;
use crate::domain::services::certificate_authority::CertificateAuthority;
use crate::domain::services::clock::Clock;
use crate::domain::services::layout_choices::LayoutChoices;
use crate::domain::services::published_images::PublishedImages;
use image_server::Handles;

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
    /// The admin socket, when it is offered.
    pub admin: Option<PathBuf>,
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
    /// Counted by the HTTPS server, reported by `/metrics`.
    pub handshakes: Arc<Handshakes>,
    /// Which layout the owner chose for each display. Kept beside the list of displays, and like the admin interface
    /// that sets it, only there when HTTPS is served.
    pub layout_choices: Arc<dyn LayoutChoices>,
    /// Who may look at the page for a browser, when it is not open. Kept beside the list of displays.
    pub viewers: Arc<Viewers>,
    /// The time the server goes by, which the layout schedule is read against.
    pub clock: Arc<dyn Clock>,
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
    let (authority, enrollment) = open_authority(config, clock.clone(), init_pki).await?;
    let layout_choices = FileLayoutChoices::open(&config.tls.directory)
        .await
        .context("Failed to open the layouts chosen for the displays")?;
    Ok(Some(Security {
        authority,
        enrollment,
        handshakes: Arc::default(),
        layout_choices: Arc::new(layout_choices),
        viewers: Arc::new(
            Viewers::new(
                Arc::new(
                    FileViewerRegistry::open(&config.tls.directory)
                        .await
                        .context("Failed to open the viewers of the web page")?,
                ),
                clock.clone(),
                Arc::new(OsEntropy),
            )
            .with_access_lifetime(chrono::Duration::minutes(
                config.web.access_token_minutes.into(),
            )),
        ),
        clock,
    }))
}

/// Opens what `config.transport` calls for, with `app` as the display routes and `web`, if there is one, the page for a
/// browser, which decides for itself who may use it and so is never put behind the certificate gate. `security` is what
/// `open_security` returned, which must be something when the transport serves HTTPS.
pub async fn start(
    config: &ServerConfig,
    app: Router,
    web: Option<Router>,
    format: ImageFormat,
    layouts: Layouts,
    security: Option<Security>,
) -> anyhow::Result<Listening> {
    let settings = ServerSettings::from(config);
    log::info!("Displays reach this server by: {}", config.transport.name());
    // A viewer token is only ever sent over HTTPS, so the page that takes one is not served in plain.
    let plain_web = web.clone().filter(|_| config.web.open);
    if !config.transport.serves_https() {
        let app = with_web(app, plain_web);
        let http = image_server::bind_http(&settings)?;
        let address = http.local_addr()?;
        return Ok(Listening {
            http: Some(address),
            https: None,
            enrollment: None,
            authority: None,
            secure: None,
            admin: None,
            serving: Box::pin(async move { http.serve(&settings, app, format, None).await }),
        });
    }

    let Security {
        authority,
        enrollment,
        handshakes,
        layout_choices,
        viewers,
        clock,
    } = security.context("HTTPS is served but the authority was not opened")?;
    let est_settings = EstSettings {
        bind: config.tls.bind,
        names: config.certificate_names(),
        certificate_lifetime: Duration::days(config.tls.server_certificate_days.into()),
        ..EstSettings::default()
    };

    let admin = open_admin(config, &enrollment, layout_choices, layouts, clock, viewers).await?;
    let admin_path = admin.as_ref().map(|admin| admin.path.clone());
    let admin = admin.map(|admin| admin.serving);

    if config.transport.serves_http() {
        // Both. The plain side serves as always; the HTTPS side adds the certificate to who is asking.
        let http = image_server::bind_http(&settings)?;
        let secure_app = with_web(
            guarded(app.clone(), enrollment.clone(), Access::Open),
            web.clone(),
        );
        let app = with_web(app, plain_web);
        let https = EstServer::bind(
            est_settings,
            authority.clone(),
            enrollment.clone(),
            Some(secure_app),
        )?
        .counting(handshakes);
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
            admin: admin_path,
            serving: Box::pin(async move {
                tokio::select! {
                    result = http.serve(&settings, app, format, Some(offer)) => result,
                    result = https.run() => result,
                    () = admin_or_never(admin) => Ok(()),
                }
            }),
        });
    }

    // HTTPS only. Nothing listens in plain, so the announcement points straight at the HTTPS port.
    let secure_app = with_web(guarded(app, enrollment.clone(), Access::Members), web);
    let https = EstServer::bind(
        est_settings,
        authority.clone(),
        enrollment.clone(),
        Some(secure_app),
    )?
    .counting(handshakes);
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
        admin: admin_path,
        serving: Box::pin(async move {
            let _advertisement =
                image_server::announce(&announced, port, format, families, Some(offer));
            tokio::select! {
                result = https.run() => result,
                () = admin_or_never(admin) => Ok(()),
            }
        }),
    })
}

/// Where a browser reaches the HTTPS side: as configured, or the first name of the server's certificate and the HTTPS port.
fn view_base(config: &ServerConfig) -> String {
    config.web.public_url.clone().unwrap_or_else(|| {
        let name = config
            .certificate_names()
            .into_iter()
            .next()
            .unwrap_or_else(|| "localhost".to_owned());
        format!("https://{name}:{}", config.tls.bind.port())
    })
}

/// What the admin interface is told of the page, so that it can say when a viewer made will not see what it was made for.
fn web_facts(config: &ServerConfig) -> admin::WebFacts {
    admin::WebFacts {
        base: view_base(config),
        enabled: config.web.enabled,
        open: config.web.open,
        layouts: config.web.layouts.clone(),
    }
}

/// `routes` with the page for a browser beside them, if there is one.
fn with_web(routes: Router, web: Option<Router>) -> Router {
    match web {
        Some(web) => routes.merge(web),
        None => routes,
    }
}

type Serving = Pin<Box<dyn Future<Output = ()> + Send>>;

/// The admin interface, open and not yet serving.
struct AdminListener {
    path: PathBuf,
    serving: Serving,
}

/// Opens the admin socket if it is offered (it is, unless turned off, whenever HTTPS is served). A socket that
/// cannot be opened stops start-up and says what to change, rather than leaving the owner with a server that
/// cannot be paired.
async fn open_admin(
    config: &ServerConfig,
    enrollment: &Arc<Enrollment>,
    layout_choices: Arc<dyn LayoutChoices>,
    layouts: Layouts,
    clock: Arc<dyn Clock>,
    viewers: Arc<Viewers>,
) -> anyhow::Result<Option<AdminListener>> {
    if !config.admin.enabled {
        log::info!("The admin interface is turned off; no display can be paired until it is on");
        return Ok(None);
    }
    let path = config.admin_socket();
    let listener = admin::bind(&path).await.with_context(|| {
        "Failed to open the admin interface (set [server.admin] enabled = false to run without it)"
    })?;
    let app = admin::router(
        enrollment.clone(),
        layout_choices,
        layouts,
        clock,
        viewers,
        web_facts(config),
    );
    log::info!("Admin interface on {}", path.display());
    Ok(Some(AdminListener {
        path,
        serving: Box::pin(async move {
            if let Err(e) = axum::serve(listener, app).await {
                log::error!("The admin interface stopped: {e}");
            }
            // The server carries on without it; it must not end because of it.
            std::future::pending::<()>().await;
        }),
    }))
}

/// Runs the admin interface if there is one. It never finishes: the server's life is not its.
async fn admin_or_never(admin: Option<Serving>) {
    match admin {
        Some(serving) => serving.await,
        None => std::future::pending().await,
    }
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
    let authority_there = tls.directory.join("root.pem").exists()
        || tls.directory.join("root.key").exists()
        || tls.directory.join("intermediate.pem").exists();
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
        "Certificate authority: root {} (valid until {}), intermediate valid until {}",
        authority.fingerprint(),
        authority.not_after().format("%Y-%m-%d"),
        authority.intermediate_not_after().format("%Y-%m-%d")
    );
    if remaining < AUTHORITY_WARNING {
        log::warn!(
            "The certificate authority's root ends on {}; every display has to be paired again by then",
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

/// The page for a browser, if `[server.web]` turns it on, and what it shows: a layout the page may show that there is not is
/// an error that stops the start. It says plainly, when it starts, who can see the picture through it.
pub fn web_routes(
    config: &ServerConfig,
    images: Arc<dyn PublishedImages>,
    format: ImageFormat,
    schedule: Schedule,
    handles: Handles,
    clock: Arc<dyn Clock>,
) -> anyhow::Result<Option<Router>> {
    let web = &config.web;
    if !web.enabled {
        return Ok(None);
    }
    config.check().map_err(anyhow::Error::msg)?;
    // With a screen of its own the page shows what was drawn for it: PNGs in a folder of their own, and a render's version
    // is the PNG's date. Without, it shows the panel's.
    let (images, format) = if web.screen.is_some() {
        (
            Arc::new(crate::adapters::published_images::DirectoryImages::new(
                config.web_images_directory(),
            )) as Arc<dyn PublishedImages>,
            ImageFormat::Png,
        )
    } else {
        (images, format)
    };
    let settings = image_server::WebSettings {
        stream: image_server::StreamTiming {
            most: web.streams as usize,
            most_per_source: web.streams_per_source as usize,
            ..Default::default()
        },
        open: web.open,
        layouts: web
            .layouts
            .iter()
            .map(|name| LayoutName::parse(name).map_err(|e| anyhow::anyhow!("{name:?}: {e}")))
            .collect::<Result<_, _>>()?,
        trusted_proxies: web
            .trusted_proxies
            .iter()
            .map(|address| {
                address
                    .parse()
                    .map_err(|e| anyhow::anyhow!("{address:?}: {e}"))
            })
            .collect::<Result<_, _>>()?,
    };
    let router = image_server::web_router(
        &settings,
        images,
        format,
        schedule,
        ServerSettings::from(config).timing,
        handles,
        clock,
    )
    .map_err(anyhow::Error::msg)?;
    if web.open {
        log::warn!(
            "The web view at /view is open: anyone who can reach this server can see the picture{}",
            if config.transport.requires_certificate() {
                ", with no certificate, which /image still asks for"
            } else {
                ""
            }
        );
    } else {
        log::info!(
            "The web view at /view is on the HTTPS port only, and needs a viewer token: make one with `displayctl viewer create`"
        );
    }
    Ok(Some(router))
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

    /// As `config`, with the page for a browser open to anyone: so it is served in plain too.
    fn open_config(transport: Transport, pki: &Path) -> ServerConfig {
        let mut config = config(transport, pki);
        config.web.open = true;
        config
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
        started_with_web(config, init_pki, None).await
    }

    async fn started_with_web(
        config: &ServerConfig,
        init_pki: bool,
        web: Option<Router>,
    ) -> anyhow::Result<Listening> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock::new(Tz::UTC));
        let security = open_security(config, clock, init_pki).await?;
        start(
            config,
            stand_in_display(),
            web,
            ImageFormat::Bmp,
            Layouts::default(),
            security,
        )
        .await
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
            handshakes: Arc::default(),
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

    /// Stands in for the page for a browser: what matters here is where it is mounted.
    fn a_page() -> Router {
        Router::new().route("/view", axum::routing::get(|| async { "the page" }))
    }

    #[tokio::test]
    async fn the_page_for_a_browser_is_served_on_plain_http_only_when_there_is_one() {
        let pki = tempfile::tempdir().unwrap();
        let without = started_with_web(&open_config(Transport::Http, pki.path()), true, None)
            .await
            .unwrap();
        let without_address = without.http.expect("plain HTTP is served");
        let _without_server = tokio::spawn(without.run());
        let with = started_with_web(
            &open_config(Transport::Http, pki.path()),
            true,
            Some(a_page()),
        )
        .await
        .unwrap();
        let with_address = with.http.expect("plain HTTP is served");
        let _with_server = tokio::spawn(with.run());

        assert_eq!(plain_get(without_address, "/view").await.0, 404);
        assert_eq!(
            plain_get(with_address, "/view").await,
            (200, "the page".to_owned())
        );
        // The display routes are as they were.
        assert_eq!(
            plain_get(with_address, "/image").await,
            (200, "image-bytes".to_owned())
        );
    }

    #[tokio::test]
    async fn the_page_for_a_browser_is_outside_the_certificate_gate_which_the_display_routes_stay_behind()
     {
        let pki = tempfile::tempdir().unwrap();
        let listening =
            started_with_web(&config(Transport::Https, pki.path()), true, Some(a_page()))
                .await
                .unwrap();
        let harness = harness_for(listening, pki);

        // A browser has no certificate, and gets the page; a display's route still needs one.
        assert_eq!(
            secure_get(&harness, "/view", None).await,
            (200, "the page".to_owned())
        );
        assert_eq!(secure_get(&harness, "/image", None).await.0, 403);
    }

    #[tokio::test]
    async fn prefer_https_puts_the_page_on_both_listeners() {
        let pki = tempfile::tempdir().unwrap();
        let listening = started_with_web(
            &open_config(Transport::PreferHttps, pki.path()),
            true,
            Some(a_page()),
        )
        .await
        .unwrap();
        let http = listening.http.expect("plain HTTP is served");
        let harness = harness_for(listening, pki);

        assert_eq!(plain_get(http, "/view").await.0, 200);
        assert_eq!(secure_get(&harness, "/view", None).await.0, 200);
    }

    #[tokio::test]
    async fn a_page_that_needs_a_token_is_never_served_in_plain_where_a_token_would_travel_in_the_clear()
     {
        let pki = tempfile::tempdir().unwrap();
        // The default: not open, so a viewer token is needed, and prefer-https has a plain side too.
        let listening = started_with_web(
            &config(Transport::PreferHttps, pki.path()),
            true,
            Some(a_page()),
        )
        .await
        .unwrap();
        let http = listening.http.expect("plain HTTP is served");
        let harness = harness_for(listening, pki);

        assert_eq!(plain_get(http, "/view").await.0, 404, "not in plain");
        assert_eq!(secure_get(&harness, "/view", None).await.0, 200, "over TLS");
        // The display routes carry on in plain as ever.
        assert_eq!(plain_get(http, "/image").await.0, 200);
    }

    /// The routes of the page for a browser over `config`, with pictures published for the panel and for the page in the
    /// places they are kept, served on a free port.
    async fn page_of(config: &ServerConfig, panel: &Path) -> String {
        use crate::application::devices::DeviceBoard;
        use crate::application::refresh::RefreshControl;
        use crate::application::status::StatusBoard;
        use crate::domain::models::layout::Layout;

        let clock: Arc<dyn Clock> = Arc::new(SystemClock::new(Tz::UTC));
        let layouts = Layouts::new(vec![Layout::new("dashboard", "Everything")]).unwrap();
        let handles = Handles {
            layouts,
            layout_choices: None,
            viewers: None,
            refresh: RefreshControl::new(std::time::Duration::from_secs(30)),
            status: StatusBoard::new(clock.now()),
            devices: DeviceBoard::new(std::time::Duration::from_secs(900)),
            members: None,
            handshakes: None,
        };
        let web = web_routes(
            config,
            Arc::new(crate::adapters::published_images::DirectoryImages::new(
                panel,
            )),
            ImageFormat::Bmp,
            Schedule::parse_every("1h").unwrap(),
            handles,
            clock,
        )
        .unwrap()
        .expect("the page is on");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, web).await });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn a_page_with_a_screen_of_its_own_shows_what_was_drawn_for_it_and_one_without_shows_the_panels()
     {
        use crate::adapters::published_images::DirectoryImages;
        use crate::domain::models::layout::LayoutName;

        let tmp = tempfile::tempdir().unwrap();
        let dashboard = LayoutName::parse("dashboard").unwrap();
        // The panel has a BMP (and a PNG, as the reTerminal publishes both); the page's folder has a PNG of its own.
        let panel = DirectoryImages::new(tmp.path());
        panel
            .publish(&dashboard, ImageFormat::Bmp, b"the panel's bmp")
            .await
            .unwrap();
        panel
            .publish(&dashboard, ImageFormat::Png, b"the panel's png")
            .await
            .unwrap();

        let mut config = ServerConfig {
            directory: tmp.path().to_path_buf(),
            ..ServerConfig::default()
        };
        config.web.enabled = true;
        config.web.open = true;

        // No screen: the panel's picture.
        let without = page_of(&config, tmp.path()).await;
        let picture = reqwest::get(format!("{without}/view/api/image"))
            .await
            .unwrap();
        assert_eq!(picture.bytes().await.unwrap().as_ref(), b"the panel's png");

        // A screen of its own, and nothing drawn for it yet: not the panel's, so nothing yet.
        config.web.screen = Some(crate::config::server::ScreenConfig::default());
        let with = page_of(&config, tmp.path()).await;
        assert_eq!(
            reqwest::get(format!("{with}/view/api/state"))
                .await
                .unwrap()
                .status(),
            404
        );
        // Drawn for it, it is shown, from the PNG's own date.
        DirectoryImages::new(config.web_images_directory())
            .publish(&dashboard, ImageFormat::Png, b"drawn for the page")
            .await
            .unwrap();
        let state = reqwest::get(format!("{with}/view/api/state"))
            .await
            .unwrap();
        assert_eq!(state.status(), 200);
        let picture = reqwest::get(format!("{with}/view/api/image"))
            .await
            .unwrap();
        assert_eq!(
            picture.bytes().await.unwrap().as_ref(),
            b"drawn for the page"
        );
    }

    /// The stream of changes over the HTTPS side, where a connection is ended after two minutes and where the page, with no
    /// certificate, is let in: it opens, says the state at once, and goes on without being cut by what the server does to
    /// connections.
    #[tokio::test]
    async fn the_stream_of_changes_works_over_tls_and_is_there_for_a_page_with_no_certificate() {
        use crate::adapters::published_images::DirectoryImages;
        use crate::application::devices::DeviceBoard;
        use crate::application::refresh::RefreshControl;
        use crate::application::status::StatusBoard;
        use crate::domain::models::layout::{Layout, LayoutName};

        let pki = tempfile::tempdir().unwrap();
        let images = tempfile::tempdir().unwrap();
        let dashboard = LayoutName::parse("dashboard").unwrap();
        let store = DirectoryImages::new(images.path());
        for format in [ImageFormat::Png, ImageFormat::Bmp] {
            store
                .publish(&dashboard, format, b"a picture")
                .await
                .unwrap();
        }
        let mut config = config(Transport::Https, pki.path());
        config.web.enabled = true;
        config.web.open = true;
        let clock: Arc<dyn Clock> = Arc::new(SystemClock::new(Tz::UTC));
        let security = open_security(&config, clock.clone(), true)
            .await
            .unwrap()
            .unwrap();
        let layouts = Layouts::new(vec![Layout::new("dashboard", "Everything")]).unwrap();
        let handles = Handles {
            layouts: layouts.clone(),
            layout_choices: None,
            viewers: Some(security.viewers.clone()),
            refresh: RefreshControl::new(std::time::Duration::from_secs(30)),
            status: StatusBoard::new(clock.now()),
            devices: DeviceBoard::new(std::time::Duration::from_secs(900)),
            members: None,
            handshakes: None,
        };
        let web = web_routes(
            &config,
            Arc::new(DirectoryImages::new(images.path())),
            ImageFormat::Bmp,
            Schedule::parse_every("1h").unwrap(),
            handles,
            clock,
        )
        .unwrap()
        .expect("the page is on");
        let listening = start(
            &config,
            stand_in_display(),
            Some(web),
            ImageFormat::Bmp,
            layouts,
            Some(security),
        )
        .await
        .unwrap();
        let harness = harness_for(listening, pki);

        let tls = client_config(Trust::Anything, None, &[&rustls::version::TLS13]);
        let mut stream = connect_with(&harness, tls).await.expect("a TLS connection");
        stream
            .write_all(b"GET /view/api/events HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut received = String::new();
        let mut buffer = [0u8; 2048];
        let read = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !received.contains("event: state") {
                let n = stream.read(&mut buffer).await.unwrap();
                assert!(n > 0, "closed after {received:?}");
                received.push_str(&String::from_utf8_lossy(&buffer[..n]));
            }
        })
        .await;

        assert!(read.is_ok(), "no event in {received:?}");
        assert!(received.starts_with("HTTP/1.1 200"), "{received:?}");
        assert!(
            received
                .to_lowercase()
                .contains("content-type: text/event-stream"),
            "{received:?}"
        );
        assert!(received.contains("retry: 2000"), "{received:?}");
    }

    /// The whole way a viewer gets in over HTTPS: made through the admin socket, signed in over TLS with the link's code, and
    /// then the token that came back is what the page's API asks for, while the display routes stay behind the certificate.
    #[tokio::test]
    async fn a_viewer_made_through_the_admin_socket_signs_in_over_tls_and_the_display_routes_stay_shut()
     {
        use crate::adapters::admin::{AdminClient, IssueViewer};
        use crate::adapters::published_images::DirectoryImages;
        use crate::application::devices::DeviceBoard;
        use crate::application::refresh::RefreshControl;
        use crate::application::status::StatusBoard;
        use crate::domain::models::layout::Layout;

        let pki = tempfile::tempdir().unwrap();
        let images = tempfile::tempdir().unwrap();
        let mut config = config(Transport::Https, pki.path());
        // On, and needing a token (not open).
        config.web.enabled = true;
        let clock: Arc<dyn Clock> = Arc::new(SystemClock::new(Tz::UTC));
        let security = open_security(&config, clock.clone(), true)
            .await
            .unwrap()
            .unwrap();
        let layouts = Layouts::new(vec![Layout::new("dashboard", "Everything")]).unwrap();
        let handles = Handles {
            layouts: layouts.clone(),
            layout_choices: None,
            viewers: Some(security.viewers.clone()),
            refresh: RefreshControl::new(std::time::Duration::from_secs(30)),
            status: StatusBoard::new(clock.now()),
            devices: DeviceBoard::new(std::time::Duration::from_secs(900)),
            members: None,
            handshakes: None,
        };
        let web = web_routes(
            &config,
            Arc::new(DirectoryImages::new(images.path())),
            ImageFormat::Bmp,
            Schedule::parse_every("1h").unwrap(),
            handles,
            clock,
        )
        .unwrap()
        .expect("the page is on");
        let listening = start(
            &config,
            stand_in_display(),
            Some(web),
            ImageFormat::Bmp,
            layouts,
            Some(security),
        )
        .await
        .unwrap();
        let admin = AdminClient::new(listening.admin.clone().expect("the admin socket is open"));
        let harness = harness_for(listening, pki);

        let issued = admin
            .issue_viewer(&IssueViewer {
                label: "tablet".to_owned(),
                ..IssueViewer::default()
            })
            .await
            .unwrap();

        // Over TLS, with no certificate (a browser has none): the page is there, the API wants a token, a display's route a
        // certificate.
        assert_eq!(secure_get(&harness, "/view", None).await.0, 200);
        assert_eq!(secure_get(&harness, "/view/api/layouts", None).await.0, 401);
        assert_eq!(secure_get(&harness, "/image", None).await.0, 403);
        // The code from the link is traded for a token, over TLS.
        let tls = client_config(Trust::Anything, None, &[&rustls::version::TLS13]);
        let mut stream = connect_with(&harness, tls).await.expect("a TLS connection");
        let code = issued.link.split("code=").nth(1).unwrap();
        let body = format!(
            "grant_type=urn:ietf:params:oauth:grant-type:pre-authorized_code&pre-authorized_code={code}"
        );
        let response = send(
            &mut stream,
            "POST",
            "/view/api/token",
            Some("application/x-www-form-urlencoded"),
            body.as_bytes(),
        )
        .await;
        assert_eq!(response.status, 200, "{}", response.text());
        assert!(
            response.text().contains("\"access_token\":\"hdv1_"),
            "{}",
            response.text()
        );
        // The owner sees it signed in, through the same socket.
        let listed = admin.viewers().await.unwrap().viewers;
        assert_eq!(
            listed[0].status,
            crate::adapters::admin::ViewerStatus::Active
        );
        // And the code is spent.
        let mut again = connect_with(
            &harness,
            client_config(Trust::Anything, None, &[&rustls::version::TLS13]),
        )
        .await
        .unwrap();
        let second = send(
            &mut again,
            "POST",
            "/view/api/token",
            Some("application/x-www-form-urlencoded"),
            body.as_bytes(),
        )
        .await;
        assert_eq!(second.status, 400);
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
        std::fs::write(pki.path().join("root.pem"), "not a certificate").unwrap();
        std::fs::write(pki.path().join("intermediate.pem"), "not a key").unwrap();
        let error = format!(
            "{:#}",
            listening(&config(Transport::Https, pki.path()))
                .await
                .err()
                .expect("should refuse")
        );
        assert!(error.contains("authority"), "{error}");
        assert_eq!(
            std::fs::read_to_string(pki.path().join("intermediate.pem")).unwrap(),
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
        assert!(pki.join("root.pem").exists() && pki.join("pairings.json").exists());
    }

    #[tokio::test]
    async fn prefer_https_makes_the_authority_on_a_first_start_without_being_asked() {
        // Displays carry on over plain HTTP meanwhile, so there is nothing to lock out.
        let pki = tempfile::tempdir().unwrap();
        restarted(&config(Transport::PreferHttps, pki.path()))
            .await
            .unwrap();
        assert!(pki.path().join("root.pem").exists());
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

    // --- the admin interface -------------------------------------------------------------------------

    #[tokio::test]
    async fn the_window_opened_over_the_admin_socket_is_the_one_the_enrolment_endpoint_obeys() {
        for transport in [Transport::PreferHttps, Transport::Https] {
            let pki = tempfile::tempdir().unwrap();
            let listening = listening(&config(transport, pki.path())).await.unwrap();
            let socket = listening
                .admin
                .clone()
                .expect("the admin interface is offered");
            assert_eq!(socket, pki.path().join("admin.sock"));
            let harness = harness_for(listening, pki);
            let client = admin::AdminClient::new(&socket);
            let key = new_key();
            let authority = harness.authority.certificate().to_vec();

            // Shut: a display that asks to join is turned away.
            let turned_away = enroll(&harness, Trust::Authority(&authority), "kitchen", &key).await;
            assert_eq!(
                turned_away.status,
                403,
                "{transport:?}: {}",
                turned_away.text()
            );

            // Opened over the socket: the same request now waits for the owner.
            assert!(client.open_window(15).await.unwrap().open);
            let waiting = enroll(&harness, Trust::Authority(&authority), "kitchen", &key).await;
            assert_eq!(waiting.status, 202, "{transport:?}: {}", waiting.text());

            // Closed over the socket: a new display is turned away again.
            client.close_window().await.unwrap();
            let other = enroll(&harness, Trust::Authority(&authority), "hall", &new_key()).await;
            assert_eq!(other.status, 403, "{transport:?}");
        }
    }

    #[tokio::test]
    async fn plain_http_offers_no_admin_interface_and_makes_nothing() {
        let pki = tempfile::tempdir().unwrap();
        let listening = listening(&config(Transport::Http, &pki.path().join("state")))
            .await
            .unwrap();
        assert!(listening.admin.is_none());
        assert!(!pki.path().join("state").exists());
    }

    #[tokio::test]
    async fn the_admin_interface_can_be_turned_off() {
        let pki = tempfile::tempdir().unwrap();
        let mut config = config(Transport::Https, pki.path());
        config.admin.enabled = false;
        let listening = listening(&config).await.unwrap();
        assert!(listening.admin.is_none());
        assert!(!pki.path().join("admin.sock").exists());
    }

    #[tokio::test]
    async fn the_admin_socket_can_be_put_somewhere_else() {
        let pki = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(elsewhere.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let mut config = config(Transport::Https, pki.path());
        config.admin.socket = Some(elsewhere.path().join("ctl.sock"));
        let listening = listening(&config).await.unwrap();
        assert_eq!(listening.admin, Some(elsewhere.path().join("ctl.sock")));
        let _server = tokio::spawn(listening.run());
        assert!(
            admin::AdminClient::new(elsewhere.path().join("ctl.sock"))
                .window()
                .await
                .is_ok()
        );
        assert!(!pki.path().join("admin.sock").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_socket_that_cannot_be_opened_stops_start_up_and_says_how_to_go_on_without_it() {
        let pki = tempfile::tempdir().unwrap();
        // Something that is not a socket is where the socket should go.
        std::fs::write(pki.path().join("admin.sock"), "precious").unwrap();
        let error = format!(
            "{:#}",
            listening(&config(Transport::Https, pki.path()))
                .await
                .err()
                .expect("should refuse")
        );
        assert!(
            error.contains("enabled = false") && error.contains("not a socket"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(pki.path().join("admin.sock")).unwrap(),
            "precious"
        );
    }
}
