//! The server's EST endpoint over TLS 1.3 (RFC 7030 and its updates): how a display joins, and renews.
//!
//! It listens on its own port with its own accept loop, because EST needs two things from the TLS
//! connection that a stock server hides: the channel binding (RFC 9266), and the certificate the client
//! showed. Both are read once the handshake is done and handed to the handlers with the request.
//!
//! The first bytes from anyone are untrusted, so the loop is bounded: a limit on connections at once,
//! a time to finish the handshake, a time to send the request head, a time to finish the request, and a
//! time for the whole connection. A client that does none of that is dropped rather than waited for.

mod routes;
#[cfg(test)]
mod tests;
mod tls;
mod wire;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use x509_parser::prelude::FromDer;

pub use self::routes::{ClientIdentity, Connection};
use self::tls::RenewingCertificate;
use crate::adapters::certificate_authority::PrivateAuthority;
use crate::adapters::listen::{self, Bound};
use crate::application::enrollment::Enrollment;
use crate::domain::models::device_id::DeviceId;
use crate::domain::models::pairing::PublicKey;
use crate::domain::services::certificate_authority::CertificateAuthority;

/// RFC 9266: the label, and no context, whose exported value identifies one TLS 1.3 connection.
const BINDING_LABEL: &[u8] = b"EXPORTER-Channel-Binding";
const BINDING_LEN: usize = 32;

#[derive(Debug, Clone)]
pub struct EstSettings {
    pub bind: SocketAddr,
    /// The names and addresses the server's certificate is for: what a display connects to.
    pub names: Vec<String>,
    pub certificate_lifetime: chrono::Duration,
    pub max_connections: usize,
    pub handshake_timeout: Duration,
    pub header_timeout: Duration,
    pub request_timeout: Duration,
    pub connection_lifetime: Duration,
}

impl Default for EstSettings {
    fn default() -> Self {
        Self {
            bind: "[::]:8443".parse().expect("a valid address"),
            names: Vec::new(),
            certificate_lifetime: chrono::Duration::days(90),
            max_connections: 16,
            handshake_timeout: Duration::from_secs(10),
            header_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(15),
            connection_lifetime: Duration::from_secs(60),
        }
    }
}

pub struct EstServer {
    listener: TcpListener,
    settings: EstSettings,
    authority: Arc<PrivateAuthority>,
    certificate: Arc<RenewingCertificate>,
    acceptor: TlsAcceptor,
    app: axum::Router,
}

impl EstServer {
    /// Opens the port and makes the server's certificate. Fails at once if either can't be done.
    pub fn bind(
        settings: EstSettings,
        authority: Arc<PrivateAuthority>,
        enrollment: Arc<Enrollment>,
    ) -> anyhow::Result<Self> {
        let Bound { listener, families } = listen::bind(settings.bind)?;
        let certificate = Arc::new(RenewingCertificate::new(
            &authority,
            &settings.names,
            Utc::now(),
            settings.certificate_lifetime,
        )?);
        let config = tls::server_config(certificate.clone(), authority.certificate())?;
        let app = routes::router(
            enrollment,
            authority.clone() as Arc<dyn CertificateAuthority>,
            settings.request_timeout,
        );
        log::info!(
            "Serving EST over TLS 1.3 at https://{} ({families}); server certificate for {}",
            settings.bind,
            settings.names.join(", ")
        );
        Ok(Self {
            listener,
            settings,
            authority,
            certificate,
            acceptor: TlsAcceptor::from(Arc::new(config)),
            app,
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Serves until the future is dropped, which also ends every connection.
    pub async fn run(self) -> anyhow::Result<()> {
        let Self {
            listener,
            settings,
            authority,
            certificate,
            acceptor,
            app,
        } = self;
        let slots = Arc::new(Semaphore::new(settings.max_connections));
        let mut connections = JoinSet::new();
        let mut renewal = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, peer) = match accepted {
                        Ok(accepted) => accepted,
                        Err(e) => {
                            // Out of descriptors, say: back off instead of spinning.
                            log::warn!("Failed to accept a connection: {e}");
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            continue;
                        }
                    };
                    let Ok(slot) = slots.clone().try_acquire_owned() else {
                        log::debug!("Dropped {peer}: too many connections");
                        continue;
                    };
                    let (acceptor, app, settings) = (acceptor.clone(), app.clone(), settings.clone());
                    connections.spawn(async move {
                        let _slot = slot;
                        if let Err(e) = serve_connection(stream, acceptor, app, &settings).await {
                            log::debug!("EST connection from {peer} ended: {e:#}");
                        }
                    });
                }
                // Finished connections are collected as they end, so the set doesn't grow.
                Some(_) = connections.join_next(), if !connections.is_empty() => {}
                _ = renewal.tick() => {
                    match certificate.renew_if_due(
                        &authority,
                        &settings.names,
                        Utc::now(),
                        settings.certificate_lifetime,
                    ) {
                        Ok(true) => log::info!("Renewed the server certificate"),
                        Ok(false) => {}
                        Err(e) => log::error!("Failed to renew the server certificate: {e:#}"),
                    }
                }
            }
        }
    }
}

async fn serve_connection(
    stream: tokio::net::TcpStream,
    acceptor: TlsAcceptor,
    app: axum::Router,
    settings: &EstSettings,
) -> anyhow::Result<()> {
    let tls = tokio::time::timeout(settings.handshake_timeout, acceptor.accept(stream))
        .await
        .context("The handshake took too long")?
        .context("The handshake failed")?;
    let connection = {
        let (_, session) = tls.get_ref();
        Connection {
            binding: session
                .export_keying_material([0u8; BINDING_LEN], BINDING_LABEL, Some(&[]))
                .ok()
                .map(|v| v.to_vec()),
            client: session
                .peer_certificates()
                .and_then(|chain| chain.first())
                .and_then(|certificate| client_named_in(certificate.as_ref())),
        }
    };
    let service = TowerToHyperService::new(app.layer(axum::Extension(connection)));
    let served = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(settings.header_timeout)
        .serve_connection(TokioIo::new(tls), service);
    tokio::time::timeout(settings.connection_lifetime, served)
        .await
        .context("The connection went on too long")?
        .context("The connection failed")
}

/// Who a (verified) client certificate is for: the common name, and the key it certifies.
fn client_named_in(certificate: &[u8]) -> Option<ClientIdentity> {
    let (_, certificate) = x509_parser::certificate::X509Certificate::from_der(certificate).ok()?;
    let name = certificate
        .subject()
        .iter_common_name()
        .next()?
        .as_str()
        .ok()?;
    Some(ClientIdentity {
        device: DeviceId::parse(name).ok()?,
        key: PublicKey::from_der(certificate.public_key().raw.to_vec()),
    })
}
