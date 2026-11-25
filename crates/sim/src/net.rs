//! Bridge between `turmoil::net` simulated TCP sockets and `canban-transport` / `canban-client`.
use anyhow::{Context, Result, bail};
use canban_app_server::AppServer;
use canban_client::{ConnectFuture, ConnectedTransport, HostConnector};
use canban_transport::ClientTransport;
use canban_types::HostTransport;
use std::{io, sync::Arc, time::Duration};

/// [`HostConnector`] establishing real `yamux/1` + JSON-RPC sessions over `turmoil::net::TcpStream`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TurmoilConnector;

impl HostConnector for TurmoilConnector {
    fn supports(&self, transport: &HostTransport) -> bool {
        matches!(transport, HostTransport::Tcp { .. })
    }

    fn connect<'a>(
        &'a self,
        _host_id: &'a str,
        transport: &'a HostTransport,
        device_name: &'a str,
    ) -> ConnectFuture<'a> {
        let transport_owned = transport.clone();
        let device = device_name.to_owned();
        Box::pin(async move {
            let HostTransport::Tcp { addr, token } = transport_owned else {
                bail!("TurmoilConnector only supports TCP hosts");
            };
            let normalized = canban_types::normalize_daemon_addr(&addr);
            let stream = tokio::time::timeout(
                Duration::from_secs(10),
                turmoil::net::TcpStream::connect(normalized.as_str()),
            )
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("Timed out connecting to {normalized}"),
                )
            })?
            .with_context(|| format!("Canban is not reachable at {normalized}"))?;
            let client = Arc::new(
                ClientTransport::connect_stream_async(stream, normalized, token, device).await?,
            );
            Ok(ConnectedTransport::new(client))
        })
    }
}

/// Runs the simulated TCP accept loop for an [`AppServer`] inside a `turmoil` host.
pub async fn serve_turmoil_listener(
    server: Arc<AppServer>,
    listener: turmoil::net::TcpListener,
) -> Result<()> {
    let mut stop_rx = server.stop_receiver();
    loop {
        if *stop_rx.borrow() || server.stopped() {
            break;
        }
        tokio::select! {
            biased;
            _ = stop_rx.changed() => break,
            res = listener.accept() => {
                match res {
                    Ok((stream, peer_addr)) => {
                        let srv = Arc::clone(&server);
                        tokio::spawn(async move {
                            let _ = srv.accept_authenticated_stream(stream, Some(peer_addr)).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        }
    }
    Ok(())
}
