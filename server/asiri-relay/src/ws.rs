//! The real-time WebSocket session (architecture §8.4, §9.2).
//!
//! 1. The relay sends a random challenge; the device signs
//!    `"asiri-relay-auth-v1" || nonce || service address` with its relay auth key.
//! 2. Queued envelopes are delivered in order with a bounded in-flight window; the device stores
//!    them and acknowledges, and only then does the relay delete them (inbox pattern, §6.4).
//! 3. Requests (send, key packages, device management, call tokens) are answered with the same
//!    `req_id`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use asiri_proto::Message as _;
use asiri_proto::relay::v1::{
    self as wire, client_frame::Body as C, send_result::Status, server_frame::Body as S,
};
use axum::extract::ws::{Message, WebSocket};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::time::{Instant, sleep_until, timeout};

use crate::db::{self, CommitOutcome, DeviceState, FanOut, InvitePolicy};
use crate::error::{RelayError, RelayResult};
use crate::ids::{Id, now, parse_id, random_bytes};
use crate::ratelimit::Kind;
use crate::state::{AppState, SessionEntry};
use crate::{authorized, livekit};

const AUTH_TIMEOUT: Duration = Duration::from_secs(20);
/// Close the session if the device sends nothing for this long (it pings every ~45 s).
const IDLE_TIMEOUT: Duration = Duration::from_secs(150);
const MAX_RECIPIENTS: usize = 1_000;
const MAX_KEY_PACKAGE_BYTES: usize = 32 * 1024;
const MAX_KEY_PACKAGES_PER_DEVICE: usize = 200;

type Tx = SplitSink<WebSocket, Message>;
type Rx = SplitStream<WebSocket>;

struct Session {
    state: Arc<AppState>,
    device_id: Id,
    inbox_id: Id,
    is_admin: bool,
    entry: SessionEntry,
    cursor: u64,
    inflight: BTreeSet<u64>,
    drained_sent: bool,
}

/// Runs one WebSocket connection to completion.
pub async fn run(state: Arc<AppState>, socket: WebSocket) {
    let (mut tx, mut rx) = socket.split();
    match authenticate(&state, &mut tx, &mut rx).await {
        Ok(mut session) => {
            if let Err(e) = session.serve(&mut tx, &mut rx).await {
                tracing::debug!("session ended: {e}");
            }
            session
                .state
                .close_session(&session.device_id, session.entry.session_id);
        }
        Err(e) => {
            tracing::debug!("authentication failed: {e}");
            let _ = send(&mut tx, 0, S::Error(e.to_wire())).await;
        }
    }
    let _ = tx.close().await;
}

async fn send(tx: &mut Tx, req_id: u64, body: S) -> RelayResult<()> {
    let frame = wire::ServerFrame {
        req_id,
        body: Some(body),
    };
    tx.send(Message::Binary(frame.encode_to_vec().into()))
        .await
        .map_err(|e| RelayError::Internal(e.into()))
}

/// Reads the next binary frame; `Ok(None)` means the peer closed the connection.
async fn next_frame(rx: &mut Rx) -> RelayResult<Option<wire::ClientFrame>> {
    while let Some(msg) = rx.next().await {
        match msg.map_err(|e| RelayError::Internal(e.into()))? {
            Message::Binary(bytes) => {
                return wire::ClientFrame::decode(bytes.as_ref())
                    .map(Some)
                    .map_err(|_| RelayError::BadRequest("undecodable frame"));
            }
            Message::Close(_) => return Ok(None),
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Text(_) => return Err(RelayError::BadRequest("text frames are not used")),
        }
    }
    Ok(None)
}

async fn authenticate(state: &Arc<AppState>, tx: &mut Tx, rx: &mut Rx) -> RelayResult<Session> {
    let nonce: [u8; 32] = random_bytes();
    send(
        tx,
        0,
        S::Challenge(wire::AuthChallenge {
            nonce: nonce.to_vec(),
            protocol_version: asiri_proto::RELAY_PROTOCOL_VERSION,
        }),
    )
    .await?;

    let frame = timeout(AUTH_TIMEOUT, next_frame(rx))
        .await
        .map_err(|_| RelayError::Unauthenticated)??
        .ok_or(RelayError::Unauthenticated)?;
    let req_id = frame.req_id;
    let Some(C::Auth(auth)) = frame.body else {
        return Err(RelayError::Unauthenticated);
    };
    let device_id = parse_id(&auth.device_id).ok_or(RelayError::Unauthenticated)?;
    let device = state
        .db
        .call(move |c| db::device(c, &device_id))
        .await?
        .ok_or(RelayError::Unauthenticated)?;
    if device.state != DeviceState::Active {
        return Err(RelayError::Unauthenticated);
    }
    let pk = device.auth_pubkey.ok_or(RelayError::Unauthenticated)?;
    let transcript = asiri_proto::auth_transcript(&nonce, &state.config.public_address);
    verify_signature(&pk, &transcript, &auth.signature)?;

    let entry = state.open_session(device.device_id, device.inbox_id);
    let token = state.issue_token(device.device_id, device.is_admin);
    let inbox = device.inbox_id;
    let pending = state.db.call(move |c| db::queued_count(c, &inbox)).await?;
    send(
        tx,
        req_id,
        S::AuthOk(wire::AuthOk {
            session_token: token.to_vec(),
            protocol_version: asiri_proto::RELAY_PROTOCOL_VERSION,
            inbox_id: device.inbox_id.to_vec(),
            is_admin: device.is_admin,
            pending_envelopes: u32::try_from(pending).unwrap_or(u32::MAX),
        }),
    )
    .await?;
    Ok(Session {
        state: Arc::clone(state),
        device_id: device.device_id,
        inbox_id: device.inbox_id,
        is_admin: device.is_admin,
        entry,
        cursor: 0,
        inflight: BTreeSet::new(),
        drained_sent: false,
    })
}

/// Verifies an Ed25519 signature strictly (no malleable or small-order inputs).
pub fn verify_signature(pubkey: &[u8; 32], msg: &[u8], signature: &[u8]) -> RelayResult<()> {
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(pubkey).map_err(|_| RelayError::Unauthenticated)?;
    let sig =
        ed25519_dalek::Signature::from_slice(signature).map_err(|_| RelayError::Unauthenticated)?;
    key.verify_strict(msg, &sig)
        .map_err(|_| RelayError::Unauthenticated)
}

impl Session {
    async fn serve(&mut self, tx: &mut Tx, rx: &mut Rx) -> RelayResult<()> {
        self.deliver_more(tx).await?;
        let mut deadline = Instant::now() + IDLE_TIMEOUT;
        let wake = Arc::clone(&self.entry.wake);
        let kick = Arc::clone(&self.entry.kick);
        loop {
            tokio::select! {
                frame = next_frame(rx) => {
                    let Some(frame) = frame? else { return Ok(()) };
                    deadline = Instant::now() + IDLE_TIMEOUT;
                    self.handle(tx, frame).await?;
                }
                () = wake.notified() => self.deliver_more(tx).await?,
                () = kick.notified() => return Ok(()),
                () = sleep_until(deadline) => return Ok(()),
            }
        }
    }

    /// Sends queued envelopes until the in-flight window is full.
    async fn deliver_more(&mut self, tx: &mut Tx) -> RelayResult<()> {
        let window = self.state.config.limits.delivery_window;
        loop {
            let room = window.saturating_sub(self.inflight.len());
            if room == 0 {
                return Ok(());
            }
            let (inbox, cursor) = (self.inbox_id, self.cursor);
            let batch = self
                .state
                .db
                .call(move |c| db::envelopes_after(c, &inbox, cursor, room, now()))
                .await?;
            let exhausted = batch.len() < room;
            for env in batch {
                self.cursor = self.cursor.max(env.seq);
                self.inflight.insert(env.seq);
                send(
                    tx,
                    0,
                    S::Deliver(wire::Deliver {
                        seq: env.seq,
                        envelope_id: env.envelope_id,
                        kind: env.kind,
                        payload: env.payload,
                    }),
                )
                .await?;
            }
            if exhausted {
                if !self.drained_sent {
                    self.drained_sent = true;
                    send(tx, 0, S::QueueDrained(wire::QueueDrained {})).await?;
                }
                return Ok(());
            }
        }
    }

    async fn handle(&mut self, tx: &mut Tx, frame: wire::ClientFrame) -> RelayResult<()> {
        let req_id = frame.req_id;
        let Some(body) = frame.body else {
            return send(
                tx,
                req_id,
                S::Error(RelayError::BadRequest("empty frame").to_wire()),
            )
            .await;
        };
        let reply = match body {
            C::Ack(ack) => {
                let inbox = self.inbox_id;
                let seqs: Vec<u64> = ack
                    .seqs
                    .into_iter()
                    .filter(|s| self.inflight.contains(s))
                    .collect();
                for s in &seqs {
                    self.inflight.remove(s);
                }
                self.state
                    .db
                    .call(move |c| db::ack(c, &inbox, &seqs))
                    .await?;
                self.deliver_more(tx).await?;
                Ok(S::Ok(wire::Ok {}))
            }
            C::Send(s) => Ok(S::SendResult(self.handle_send(s).await?)),
            C::Ping(p) => Ok(S::Pong(wire::Pong { nonce: p.nonce })),
            C::PutKeyPackages(p) => self.put_key_packages(p).await.map(|()| S::Ok(wire::Ok {})),
            C::FetchKeyPackage(f) => self.fetch_key_package(f).await.map(S::KeyPackage),
            C::KeyPackageCount(_) => {
                let device = self.device_id;
                let (one_time, has_last_resort) = self
                    .state
                    .db
                    .call(move |c| db::key_package_count(c, &device, now()))
                    .await?;
                Ok(S::KeyPackageCount(wire::KeyPackageCount {
                    one_time,
                    has_last_resort,
                }))
            }
            C::AuthorizeDevice(a) => self.authorize_device(a).await.map(S::DeviceAuthorized),
            C::RevokeDevice(r) => self.revoke_device(r).await.map(|()| S::Ok(wire::Ok {})),
            C::ListDevices(_) => self.list_devices().await.map(S::DeviceList),
            C::CallToken(r) => self.call_token(&r).map(S::CallToken),
            C::Auth(_) => Err(RelayError::BadRequest("already authenticated")),
        };
        match reply {
            Ok(body) => send(tx, req_id, body).await,
            Err(RelayError::Internal(e)) => {
                tracing::warn!("request failed: {e:#}");
                send(tx, req_id, S::Error(RelayError::Internal(e).to_wire())).await
            }
            Err(e) => send(tx, req_id, S::Error(e.to_wire())).await,
        }
    }

    async fn handle_send(&self, s: wire::Send) -> RelayResult<wire::SendResult> {
        let result = |status: Status| wire::SendResult {
            status: status as i32,
            current_epoch: 0,
            unknown_inboxes: vec![],
        };
        let limits = &self.state.config.limits;
        if s.payload.len() > limits.envelope_bytes {
            return Ok(result(Status::TooLarge));
        }
        if !self.state.limiter.check(&self.device_id, Kind::Send) {
            return Ok(result(Status::RateLimited));
        }
        if s.envelope_id.len() != asiri_proto::ID_LEN
            || s.to_inboxes.is_empty()
            || s.to_inboxes.len() > MAX_RECIPIENTS
        {
            return Err(RelayError::BadRequest("invalid send"));
        }
        let kind = wire::PayloadKind::try_from(s.kind)
            .map_err(|_| RelayError::BadRequest("unknown payload kind"))?;
        if kind == wire::PayloadKind::Unspecified {
            return Err(RelayError::BadRequest("unknown payload kind"));
        }
        let mut recipients = Vec::with_capacity(s.to_inboxes.len());
        for inbox in &s.to_inboxes {
            let id = parse_id(inbox).ok_or(RelayError::BadRequest("invalid inbox"))?;
            if !recipients.contains(&id) {
                recipients.push(id);
            }
        }
        let commit = if kind == wire::PayloadKind::Commit {
            Some(parse_id(&s.group_route).ok_or(RelayError::BadRequest("commit needs a route"))?)
        } else {
            None
        };
        let max_ttl = i64::from(limits.envelope_ttl_seconds);
        let ttl = if s.ttl_seconds == 0 {
            max_ttl
        } else {
            i64::from(s.ttl_seconds).min(max_ttl)
        };
        let backlog = limits.inbox_backlog;
        let epoch = s.epoch;
        let rcpts = recipients.clone();
        let outcome = self
            .state
            .db
            .call(move |c| {
                let t = now();
                db::fan_out(
                    c,
                    &rcpts,
                    &db::OutgoingEnvelope {
                        envelope_id: &s.envelope_id,
                        kind: s.kind,
                        payload: &s.payload,
                        expires_at: t + ttl,
                    },
                    commit.as_ref().map(|r| (r, epoch)),
                    backlog,
                    t,
                )
            })
            .await?;
        Ok(match outcome {
            FanOut::Queued(CommitOutcome::Accepted) => {
                self.state.wake_inboxes(&recipients);
                result(Status::Accepted)
            }
            FanOut::Queued(CommitOutcome::StaleEpoch { current_epoch }) => wire::SendResult {
                status: Status::StaleEpoch as i32,
                current_epoch,
                unknown_inboxes: vec![],
            },
            FanOut::UnknownInboxes(list) => wire::SendResult {
                status: Status::UnknownInbox as i32,
                current_epoch: 0,
                unknown_inboxes: list.iter().map(|i| i.to_vec()).collect(),
            },
            FanOut::InboxFull => result(Status::InboxFull),
        })
    }

    async fn put_key_packages(&self, p: wire::PutKeyPackages) -> RelayResult<()> {
        if p.key_packages.len() > 100
            || p.key_packages
                .iter()
                .chain(std::iter::once(&p.last_resort))
                .any(|kp| kp.len() > MAX_KEY_PACKAGE_BYTES)
        {
            return Err(RelayError::TooLarge);
        }
        let device = self.device_id;
        let expires = now() + i64::from(self.state.config.limits.key_package_ttl_seconds);
        self.state
            .db
            .call(move |c| {
                let lr = (!p.last_resort.is_empty()).then_some(p.last_resort.as_slice());
                db::put_key_packages(
                    c,
                    &device,
                    &p.key_packages,
                    lr,
                    expires,
                    MAX_KEY_PACKAGES_PER_DEVICE,
                )
            })
            .await
    }

    async fn fetch_key_package(&self, f: wire::FetchKeyPackage) -> RelayResult<wire::KeyPackage> {
        if !self
            .state
            .limiter
            .check(&self.device_id, Kind::KeyPackageFetch)
        {
            return Err(RelayError::RateLimited);
        }
        let target = parse_id(&f.device_id).ok_or(RelayError::BadRequest("invalid device"))?;
        let kp = self
            .state
            .db
            .call(move |c| db::take_key_package(c, &target, now()))
            .await?
            .ok_or(RelayError::NotFound)?;
        Ok(wire::KeyPackage {
            device_id: target.to_vec(),
            key_package: kp,
        })
    }

    async fn authorize_device(
        &self,
        a: wire::AuthorizeDevice,
    ) -> RelayResult<wire::DeviceAuthorized> {
        let policy = self.state.db.call(|c| db::invite_policy(c)).await?;
        if policy == InvitePolicy::Admins && !self.is_admin {
            return Err(RelayError::Forbidden("only admins can add devices"));
        }
        if !self.state.limiter.check(&self.device_id, Kind::Authorize) {
            return Err(RelayError::RateLimited);
        }
        let onion: [u8; 32] = a
            .onion_client_key
            .as_slice()
            .try_into()
            .map_err(|_| RelayError::BadRequest("onion key must be 32 bytes"))?;
        let auth: [u8; 32] = a
            .auth_pubkey
            .as_slice()
            .try_into()
            .map_err(|_| RelayError::BadRequest("auth key must be 32 bytes"))?;
        ed25519_dalek::VerifyingKey::from_bytes(&auth)
            .map_err(|_| RelayError::BadRequest("invalid auth key"))?;
        let expires = now() + i64::from(self.state.config.limits.registration_ttl_seconds);
        let pending = self
            .state
            .db
            .call(move |c| db::create_pending_device(c, onion, Some(auth), false, expires))
            .await?;
        if let Some(dir) = &self.state.config.authorized_dir
            && onion != [0u8; 32]
        {
            authorized::write_key(dir, &pending.device_id, &onion)?;
        }
        Ok(wire::DeviceAuthorized {
            device_id: pending.device_id.to_vec(),
            inbox_id: pending.inbox_id.to_vec(),
            registration_secret: pending.registration_secret.to_vec(),
        })
    }

    async fn revoke_device(&self, r: wire::RevokeDevice) -> RelayResult<()> {
        let target = parse_id(&r.device_id).ok_or(RelayError::BadRequest("invalid device"))?;
        if target != self.device_id && !self.is_admin {
            return Err(RelayError::Forbidden(
                "only admins can revoke other devices",
            ));
        }
        self.state
            .db
            .call(move |c| db::revoke_device(c, &target, now()))
            .await?;
        if let Some(dir) = &self.state.config.authorized_dir {
            authorized::remove_key(dir, &target)?;
        }
        self.state.kick(&target);
        Ok(())
    }

    async fn list_devices(&self) -> RelayResult<wire::DeviceList> {
        if !self.is_admin {
            return Err(RelayError::Forbidden("admins only"));
        }
        self.state
            .db
            .call(|c| {
                let devices = db::list_devices(c)?
                    .into_iter()
                    .map(|d| wire::device_list::Entry {
                        device_id: d.device_id.to_vec(),
                        state: d.state as u32,
                        is_admin: d.is_admin,
                    })
                    .collect();
                Ok(wire::DeviceList {
                    devices,
                    invite_policy: db::invite_policy(c)?.as_str().to_owned(),
                    stored_blob_bytes: db::stored_blob_bytes(c)?,
                    queued_envelopes: db::total_queued(c)?,
                })
            })
            .await
    }

    fn call_token(&self, r: &wire::CallTokenRequest) -> RelayResult<wire::CallToken> {
        let keys = self
            .state
            .config
            .livekit
            .as_ref()
            .ok_or(RelayError::Forbidden(
                "calls are not configured on this relay",
            ))?;
        if !livekit::valid_room_alias(&r.room_alias) {
            return Err(RelayError::BadRequest("invalid room alias"));
        }
        if !self.state.limiter.check(&self.device_id, Kind::CallToken) {
            return Err(RelayError::RateLimited);
        }
        let identity = hex::encode(random_bytes::<10>());
        let ttl = self.state.config.limits.call_token_ttl_seconds;
        let jwt = livekit::mint(keys, &identity, &r.room_alias, now(), ttl)
            .map_err(|e| RelayError::Internal(e.into()))?;
        Ok(wire::CallToken {
            jwt,
            expires_in_s: ttl,
            identity,
        })
    }
}
