//! A malicious relay in a box (architecture §18, "malicious-relay tests").
//!
//! The proxy sits between the clients and an honest relay. HTTP passes through untouched. On
//! the WebSocket it decodes every frame and, following a per-inbox plan set by the test, drops,
//! duplicates, reorders or tampers with the envelopes it delivers, and can replay or inject
//! envelopes of its own. Everything it does is something a compromised relay could do, so the
//! tests prove what clients do when the server turns against them.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use asiri_proto::Message as _;
use asiri_proto::relay::v1::{
    self as wire, PayloadKind, client_frame::Body as C, server_frame::Body as S,
};
use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// Request IDs the proxy uses for its own frames; replies to them are swallowed.
const OWN_REQ_BASE: u64 = 1 << 62;

/// What to do with one delivered envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Deliver it normally.
    Forward,
    /// Never deliver it, and acknowledge it to the relay so it's gone for good (suppression).
    Drop,
    /// Deliver it twice.
    Duplicate,
    /// Flip one byte of the ciphertext.
    Tamper,
    /// Hold it back and deliver it right after the next envelope (reordering).
    Delay,
}

struct Plan {
    kind: Option<i32>,
    action: Action,
}

#[derive(Default)]
struct Shared {
    plans: HashMap<Vec<u8>, VecDeque<Plan>>,
    seen: HashMap<Vec<u8>, Vec<wire::Deliver>>,
    sessions: HashMap<Vec<u8>, mpsc::UnboundedSender<wire::Deliver>>,
}

impl Shared {
    fn next_action(&mut self, inbox: &[u8], kind: i32) -> Action {
        let Some(queue) = self.plans.get_mut(inbox) else {
            return Action::Forward;
        };
        match queue.iter().position(|p| p.kind.is_none_or(|k| k == kind)) {
            Some(i) => queue.remove(i).map_or(Action::Forward, |p| p.action),
            None => Action::Forward,
        }
    }
}

/// The proxy. Clients dial [`Proxy::addr`]; the relay's public address must be the same.
pub struct Proxy {
    /// Where clients connect.
    pub addr: SocketAddr,
    shared: Arc<Mutex<Shared>>,
    task: JoinHandle<()>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Proxy {
    /// Starts a proxy in front of the relay at `upstream`.
    pub async fn start(upstream: SocketAddr) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let task = {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let shared = Arc::clone(&shared);
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, upstream, shared).await {
                            tracing::debug!("evil proxy connection ended: {e}");
                        }
                    });
                }
            })
        };
        Ok(Self { addr, shared, task })
    }

    /// Applies `actions`, in order, to the next envelopes delivered to `inbox` (only those of
    /// `kind`, when given).
    pub fn plan(
        &self,
        inbox: &[u8],
        kind: Option<PayloadKind>,
        actions: impl IntoIterator<Item = Action>,
    ) {
        let mut shared = self.shared.lock().expect("proxy state");
        let queue = shared.plans.entry(inbox.to_vec()).or_default();
        for action in actions {
            queue.push_back(Plan {
                kind: kind.map(|k| k as i32),
                action,
            });
        }
    }

    /// Envelopes delivered (or dropped) to `inbox` so far, oldest first.
    #[must_use]
    pub fn seen(&self, inbox: &[u8]) -> Vec<wire::Deliver> {
        self.shared
            .lock()
            .expect("proxy state")
            .seen
            .get(inbox)
            .cloned()
            .unwrap_or_default()
    }

    /// Delivers a fabricated or replayed envelope to `inbox`'s live session.
    pub fn inject(&self, inbox: &[u8], deliver: wire::Deliver) -> anyhow::Result<()> {
        let shared = self.shared.lock().expect("proxy state");
        let tx = shared
            .sessions
            .get(inbox)
            .ok_or_else(|| anyhow::anyhow!("that inbox is not connected"))?;
        tx.send(deliver)
            .map_err(|_| anyhow::anyhow!("the session ended"))
    }
}

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(asiri_proto::MAX_FRAME_BYTES))
        .max_frame_size(Some(asiri_proto::MAX_FRAME_BYTES))
}

async fn handle(
    mut client: TcpStream,
    upstream: SocketAddr,
    shared: Arc<Mutex<Shared>>,
) -> anyhow::Result<()> {
    // Look at the request line without consuming it.
    let mut head = [0u8; 16];
    let mut waited = Duration::ZERO;
    let n = loop {
        let n = client.peek(&mut head).await?;
        if n >= 10 || n == 0 || waited > Duration::from_secs(5) {
            break n;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        waited += Duration::from_millis(5);
    };
    if head[..n].starts_with(b"GET /v1/ws") {
        websocket(client, upstream, shared).await
    } else {
        let mut up = TcpStream::connect(upstream).await?;
        tokio::io::copy_bidirectional(&mut client, &mut up).await?;
        Ok(())
    }
}

fn binary(frame: &wire::ServerFrame) -> Message {
    Message::Binary(frame.encode_to_vec().into())
}

fn deliver_frame(d: wire::Deliver) -> wire::ServerFrame {
    wire::ServerFrame {
        req_id: 0,
        body: Some(S::Deliver(d)),
    }
}

async fn websocket(
    client: TcpStream,
    upstream: SocketAddr,
    shared: Arc<Mutex<Shared>>,
) -> anyhow::Result<()> {
    let down = tokio_tungstenite::accept_async_with_config(client, Some(ws_config())).await?;
    let up_stream = TcpStream::connect(upstream).await?;
    let (up, _) = tokio_tungstenite::client_async_with_config(
        format!("ws://{upstream}/v1/ws"),
        up_stream,
        Some(ws_config()),
    )
    .await?;
    let (mut down_tx, mut down_rx) = down.split();
    let (mut up_tx, mut up_rx) = up.split();
    let (inject_tx, mut inject_rx) = mpsc::unbounded_channel::<wire::Deliver>();
    let mut inbox: Option<Vec<u8>> = None;
    let mut held: Option<wire::ServerFrame> = None;
    let mut own_req = OWN_REQ_BASE;

    let result: anyhow::Result<()> = async {
        loop {
            tokio::select! {
                msg = down_rx.next() => {
                    let Some(Ok(msg)) = msg else { return Ok(()) };
                    up_tx.send(msg).await?;
                }
                msg = up_rx.next() => {
                    let Some(Ok(msg)) = msg else { return Ok(()) };
                    let Message::Binary(bytes) = &msg else {
                        down_tx.send(msg).await?;
                        continue;
                    };
                    let frame = wire::ServerFrame::decode(bytes.as_ref())?;
                    if frame.req_id >= OWN_REQ_BASE {
                        continue; // the relay answering one of our own acks
                    }
                    match &frame.body {
                        Some(S::AuthOk(ok)) => {
                            inbox = Some(ok.inbox_id.clone());
                            shared.lock().expect("proxy state").sessions.insert(ok.inbox_id.clone(), inject_tx.clone());
                        }
                        Some(S::Deliver(d)) => {
                            let target = inbox.clone().unwrap_or_default();
                            let action = {
                                let mut s = shared.lock().expect("proxy state");
                                s.seen.entry(target.clone()).or_default().push(d.clone());
                                s.next_action(&target, d.kind)
                            };
                            match action {
                                Action::Forward => down_tx.send(binary(&frame)).await?,
                                Action::Drop => {
                                    own_req += 1;
                                    let ack = wire::ClientFrame {
                                        req_id: own_req,
                                        body: Some(C::Ack(wire::Ack { seqs: vec![d.seq] })),
                                    };
                                    up_tx.send(Message::Binary(ack.encode_to_vec().into())).await?;
                                }
                                Action::Duplicate => {
                                    down_tx.send(binary(&frame)).await?;
                                    down_tx.send(binary(&frame)).await?;
                                }
                                Action::Tamper => {
                                    let mut d = d.clone();
                                    let len = u32::from_be_bytes([d.payload[0], d.payload[1], d.payload[2], d.payload[3]]) as usize;
                                    if let Some(b) = d.payload.get_mut(3 + len) {
                                        *b ^= 0x5a;
                                    }
                                    down_tx.send(binary(&deliver_frame(d))).await?;
                                }
                                Action::Delay => {
                                    if let Some(previous) = held.replace(frame.clone()) {
                                        down_tx.send(binary(&previous)).await?;
                                    }
                                    continue;
                                }
                            }
                            if let Some(h) = held.take() {
                                down_tx.send(binary(&h)).await?;
                            }
                            continue;
                        }
                        _ => {}
                    }
                    down_tx.send(msg).await?;
                }
                Some(d) = inject_rx.recv() => {
                    down_tx.send(binary(&deliver_frame(d))).await?;
                }
            }
        }
    }
    .await;
    if let Some(inbox) = inbox {
        let mut s = shared.lock().expect("proxy state");
        // A reconnect may already have replaced this session.
        if s.sessions
            .get(&inbox)
            .is_some_and(|tx| tx.same_channel(&inject_tx))
        {
            s.sessions.remove(&inbox);
        }
    }
    result
}
