//! The call signaling bridge (architecture §8.6).
//!
//! During a call — and only then — a listener on `127.0.0.1:<random port>` forwards each
//! connection over the dialer (Tor in production) to LiveKit's signaling port, so the LiveKit SDK
//! can connect to `ws://127.0.0.1:<port>` with a short-lived token. It closes when the call ends.

use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::{Dialer, Port};
use crate::error::{CoreError, CoreResult};

/// A running bridge; dropping it closes the listener and every forwarded connection.
pub struct Bridge {
    /// The loopback port LiveKit should connect to.
    pub port: u16,
    task: JoinHandle<()>,
}

impl Bridge {
    /// Starts a bridge to the dialer's call port.
    pub async fn start(dialer: Arc<dyn Dialer>) -> CoreResult<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(CoreError::network)?;
        let port = listener.local_addr().map_err(CoreError::network)?.port();
        let task = tokio::spawn(async move {
            let mut conns = tokio::task::JoinSet::new();
            loop {
                let Ok((mut local, peer)) = listener.accept().await else {
                    break;
                };
                if !peer.ip().is_loopback() {
                    continue;
                }
                let dialer = Arc::clone(&dialer);
                conns.spawn(async move {
                    let _ = local.set_nodelay(true);
                    if let Ok(mut upstream) = dialer.dial(Port::Calls).await {
                        let _ = tokio::io::copy_bidirectional(&mut local, &mut upstream).await;
                    }
                });
                while conns.try_join_next().is_some() {}
            }
        });
        Ok(Self { port, task })
    }

    /// The URL to hand to LiveKit.
    #[must_use]
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // Aborting the accept task drops the JoinSet, which aborts every forwarded connection.
        self.task.abort();
    }
}
