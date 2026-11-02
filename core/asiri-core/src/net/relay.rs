//! The relay WebSocket session (architecture §8.4).
//!
//! 1. Open a stream with the [`Dialer`] and upgrade it to a WebSocket at `/v1/ws`.
//! 2. Answer the challenge by signing `"asiri-relay-auth-v1" || nonce || service address`.
//! 3. A background task reads frames, routes replies to waiting requests by `req_id`, forwards
//!    deliveries, and sends a ping every 45 s plus jitter; no pong within 20 s ends the session.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use asiri_proto::Message as _;
use asiri_proto::relay::v1::{self as wire, client_frame::Body as C, server_frame::Body as S};
use futures::{SinkExt, StreamExt};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use super::Dialer;
use crate::error::{CoreError, CoreResult};
use crate::ids::{Id16, id16, random_bytes};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PING_EVERY: Duration = Duration::from_secs(45);
const PONG_WITHIN: Duration = Duration::from_secs(20);

/// Something the relay pushed.
#[derive(Debug)]
pub enum Push {
    /// An envelope from our inbox.
    Deliver(wire::Deliver),
    /// The backlog waiting at connect time has been delivered.
    Drained,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<S>>>>;

/// An authenticated session with the relay.
pub struct RelayClient {
    out: mpsc::Sender<wire::ClientFrame>,
    pending: Pending,
    next_req: AtomicU64,
    closed: Arc<AtomicBool>,
    close_signal: Arc<Notify>,
    /// Bearer token for HTTP calls during this session.
    pub session_token: Vec<u8>,
    /// Our inbox.
    pub inbox_id: Id16,
    /// Whether this device administers the relay.
    pub is_admin: bool,
    /// Envelopes waiting when we connected.
    pub pending_envelopes: u32,
}

impl RelayClient {
    /// Connects and authenticates. Returns the client and the stream of pushes; the session
    /// ends when the push channel closes.
    pub async fn connect(
        dialer: &Arc<dyn Dialer>,
        device_id: &Id16,
        sign: impl FnOnce(&[u8]) -> Vec<u8>,
    ) -> CoreResult<(Arc<Self>, mpsc::Receiver<Push>)> {
        let stream = dialer.dial(super::Port::Relay).await?;
        let url = format!("ws://{}/v1/ws", dialer.host());
        let config = WebSocketConfig::default()
            .max_message_size(Some(asiri_proto::MAX_FRAME_BYTES))
            .max_frame_size(Some(asiri_proto::MAX_FRAME_BYTES));
        let (mut ws, _) = tokio::time::timeout(
            REQUEST_TIMEOUT,
            tokio_tungstenite::client_async_with_config(url, stream, Some(config)),
        )
        .await
        .map_err(|_| CoreError::Offline)?
        .map_err(CoreError::network)?;

        let challenge = match next_server_frame(&mut ws).await?.body {
            Some(S::Challenge(c)) => c,
            _ => return Err(CoreError::Network("unexpected first frame".into())),
        };
        if challenge.protocol_version != asiri_proto::RELAY_PROTOCOL_VERSION {
            return Err(CoreError::Network(
                "the server speaks a different protocol version".into(),
            ));
        }
        let signature = sign(&asiri_proto::auth_transcript(
            &challenge.nonce,
            dialer.service_address(),
        ));
        let auth = wire::ClientFrame {
            req_id: 1,
            body: Some(C::Auth(wire::AuthResponse {
                device_id: device_id.to_vec(),
                signature,
            })),
        };
        ws.send(Message::Binary(auth.encode_to_vec().into()))
            .await
            .map_err(CoreError::network)?;
        let ok = match next_server_frame(&mut ws).await?.body {
            Some(S::AuthOk(ok)) => ok,
            Some(S::Error(e)) => return Err(CoreError::Relay(e.detail)),
            _ => return Err(CoreError::Network("unexpected auth reply".into())),
        };

        let (out_tx, out_rx) = mpsc::channel::<wire::ClientFrame>(256);
        let (push_tx, push_rx) = mpsc::channel::<Push>(256);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let close_signal = Arc::new(Notify::new());
        tokio::spawn(io_loop(
            ws,
            out_rx,
            push_tx,
            Arc::clone(&pending),
            Arc::clone(&closed),
            Arc::clone(&close_signal),
        ));

        Ok((
            Arc::new(Self {
                out: out_tx,
                pending,
                next_req: AtomicU64::new(2),
                closed,
                close_signal,
                session_token: ok.session_token,
                inbox_id: id16(&ok.inbox_id)
                    .ok_or_else(|| CoreError::Network("malformed inbox id".into()))?,
                is_admin: ok.is_admin,
                pending_envelopes: ok.pending_envelopes,
            }),
            push_rx,
        ))
    }

    /// Whether the session has ended.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed) || self.out.is_closed()
    }

    /// Sends a request and waits for its reply. Relay errors become [`CoreError::Relay`].
    pub async fn request(&self, body: C) -> CoreResult<S> {
        if self.is_closed() {
            return Err(CoreError::Offline);
        }
        let req_id = self.next_req.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| CoreError::internal("lock"))?
            .insert(req_id, tx);
        let frame = wire::ClientFrame {
            req_id,
            body: Some(body),
        };
        if self.out.send(frame).await.is_err() {
            return Err(CoreError::Offline);
        }
        let reply = tokio::time::timeout(REQUEST_TIMEOUT, rx).await;
        if let Ok(mut p) = self.pending.lock() {
            p.remove(&req_id);
        }
        match reply {
            Ok(Ok(S::Error(e))) => Err(CoreError::Relay(e.detail)),
            Ok(Ok(body)) => Ok(body),
            Ok(Err(_)) => Err(CoreError::Offline),
            Err(_) => Err(CoreError::Network(
                "the server did not answer in time".into(),
            )),
        }
    }

    /// Acknowledges stored envelopes (fire and forget; unacked envelopes are redelivered).
    pub async fn ack(&self, seqs: Vec<u64>) -> CoreResult<()> {
        if seqs.is_empty() {
            return Ok(());
        }
        self.request(C::Ack(wire::Ack { seqs })).await.map(|_| ())
    }

    /// Ends the session: the WebSocket closes right away and waiting requests fail.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.close_signal.notify_one();
    }
}

async fn next_server_frame<S2>(
    ws: &mut tokio_tungstenite::WebSocketStream<S2>,
) -> CoreResult<wire::ServerFrame>
where
    S2: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let msg = tokio::time::timeout(REQUEST_TIMEOUT, ws.next())
            .await
            .map_err(|_| CoreError::Offline)?
            .ok_or(CoreError::Offline)?
            .map_err(CoreError::network)?;
        match msg {
            Message::Binary(b) => return Ok(wire::ServerFrame::decode(b.as_ref())?),
            Message::Close(_) => return Err(CoreError::Offline),
            _ => {}
        }
    }
}

fn jitter() -> Duration {
    Duration::from_millis(u64::from(u16::from_le_bytes(random_bytes::<2>())) % 10_000)
}

async fn io_loop<S2>(
    mut ws: tokio_tungstenite::WebSocketStream<S2>,
    mut out: mpsc::Receiver<wire::ClientFrame>,
    push: mpsc::Sender<Push>,
    pending: Pending,
    closed: Arc<AtomicBool>,
    close_signal: Arc<Notify>,
) where
    S2: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    let mut next_ping = tokio::time::Instant::now() + PING_EVERY + jitter();
    let mut awaiting_pong: Option<(u64, tokio::time::Instant)> = None;
    loop {
        let deadline = awaiting_pong.map_or(next_ping, |(_, d)| d.min(next_ping));
        tokio::select! {
            msg = ws.next() => {
                let Some(Ok(msg)) = msg else { break };
                let Message::Binary(bytes) = msg else {
                    if matches!(msg, Message::Close(_)) { break }
                    continue;
                };
                let Ok(frame) = wire::ServerFrame::decode(bytes.as_ref()) else { break };
                match frame.body {
                    Some(S::Deliver(d)) => {
                        if push.send(Push::Deliver(d)).await.is_err() { break }
                    }
                    Some(S::QueueDrained(_)) => {
                        let _ = push.send(Push::Drained).await;
                    }
                    Some(S::Pong(p)) if awaiting_pong.is_some_and(|(n, _)| n == p.nonce) => {
                        awaiting_pong = None;
                    }
                    Some(body) if frame.req_id != 0 => {
                        let waiter = pending.lock().ok().and_then(|mut p| p.remove(&frame.req_id));
                        if let Some(w) = waiter { let _ = w.send(body); }
                    }
                    _ => {}
                }
            }
            () = close_signal.notified() => break,
            frame = out.recv() => {
                let Some(frame) = frame else { break };
                if ws.send(Message::Binary(frame.encode_to_vec().into())).await.is_err() { break }
            }
            () = tokio::time::sleep_until(deadline) => {
                if let Some((_, d)) = awaiting_pong && tokio::time::Instant::now() >= d {
                    break; // no pong: the connection is dead
                }
                if tokio::time::Instant::now() >= next_ping {
                    let nonce = u64::from_le_bytes(random_bytes());
                    let ping = wire::ClientFrame { req_id: 0, body: Some(C::Ping(wire::Ping { nonce })) };
                    if ws.send(Message::Binary(ping.encode_to_vec().into())).await.is_err() { break }
                    awaiting_pong = Some((nonce, tokio::time::Instant::now() + PONG_WITHIN));
                    next_ping = tokio::time::Instant::now() + PING_EVERY + jitter();
                }
            }
        }
    }
    closed.store(true, Ordering::Relaxed);
    if let Ok(mut p) = pending.lock() {
        p.clear(); // waiting requests see the sender dropped → Offline
    }
    let _ = ws.close(None).await;
}
