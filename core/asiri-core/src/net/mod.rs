//! Networking (architecture §8). Everything above the [`Dialer`] — WebSocket, HTTP, the call
//! signaling bridge — is identical for Tor and for the direct development transport
//! (docs/adr/0003-direct-transport-for-development.md).

pub mod bridge;
pub mod http;
pub mod relay;
#[cfg(feature = "tor")]
pub mod tor;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use asiri_proto::pairing::v1::relay_endpoint::Transport;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::{CoreError, CoreResult};
use crate::identity::RelayEndpoint;

/// A bidirectional byte stream to the relay host.
pub trait ByteStream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> ByteStream for T {}

/// A boxed [`ByteStream`].
pub type BoxStream = Box<dyn ByteStream>;

/// A boxed future, for object-safe async trait methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Which service on the relay host to reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port {
    /// asiri-relay (onion port 80).
    Relay,
    /// LiveKit signaling (onion port 7880).
    Calls,
}

/// Opens streams to the relay host.
pub trait Dialer: Send + Sync + 'static {
    /// Opens a new stream to `port`.
    fn dial(&self, port: Port) -> BoxFuture<'_, CoreResult<BoxStream>>;
    /// The address bound into the auth signature (what the relay calls its public address).
    fn service_address(&self) -> &str;
    /// The `Host` used for HTTP and WebSocket requests.
    fn host(&self) -> &str;
    /// Whether this transport hides metadata from the network (Tor) or not (direct).
    fn is_anonymous(&self) -> bool;
}

/// Plain TCP to `host:port` (development, tests and LAN setups).
pub struct DirectDialer {
    relay: String,
    calls: String,
}

impl DirectDialer {
    /// A dialer for a direct endpoint.
    #[must_use]
    pub fn new(endpoint: &RelayEndpoint) -> Self {
        let calls = if endpoint.call_address.is_empty() {
            let host = endpoint
                .address
                .rsplit_once(':')
                .map_or(endpoint.address.as_str(), |(h, _)| h);
            format!("{host}:7880")
        } else {
            endpoint.call_address.clone()
        };
        Self {
            relay: endpoint.address.clone(),
            calls,
        }
    }
}

impl Dialer for DirectDialer {
    fn dial(&self, port: Port) -> BoxFuture<'_, CoreResult<BoxStream>> {
        Box::pin(async move {
            let addr = match port {
                Port::Relay => &self.relay,
                Port::Calls => &self.calls,
            };
            let stream = tokio::time::timeout(
                std::time::Duration::from_secs(15),
                tokio::net::TcpStream::connect(addr),
            )
            .await
            .map_err(|_| CoreError::Offline)?
            .map_err(|_| CoreError::Offline)?;
            let _ = stream.set_nodelay(true);
            Ok(Box::new(stream) as BoxStream)
        })
    }

    fn service_address(&self) -> &str {
        &self.relay
    }

    fn host(&self) -> &str {
        &self.relay
    }

    fn is_anonymous(&self) -> bool {
        false
    }
}

/// Builds the dialer for an endpoint. Onion endpoints need the `tor` feature and a Tor client.
pub fn dialer_for(
    endpoint: &RelayEndpoint,
    #[cfg(feature = "tor")] tor: Option<&Arc<tor::Tor>>,
) -> CoreResult<Arc<dyn Dialer>> {
    match endpoint.transport {
        Transport::Direct => Ok(Arc::new(DirectDialer::new(endpoint))),
        Transport::Onion => {
            #[cfg(feature = "tor")]
            {
                let tor = tor.ok_or_else(|| CoreError::Network("Tor is not running".into()))?;
                Ok(Arc::new(tor::OnionDialer::new(Arc::clone(tor), endpoint)?))
            }
            #[cfg(not(feature = "tor"))]
            {
                Err(CoreError::Network(
                    "this build of Asiri cannot reach onion servers (built without Tor)".into(),
                ))
            }
        }
        Transport::Unspecified => Err(CoreError::Invalid("unknown transport".into())),
    }
}
