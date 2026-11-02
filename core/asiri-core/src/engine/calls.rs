//! Calls (architecture §7.8, §10).
//!
//! `call_secret = MLS-Exporter("asiri/call/v1", call_id, 32)` is LiveKit's shared E2EE key.
//! Every member derives the same secret; the relay and LiveKit never see it. When the group's
//! epoch changes mid-call, every client derives the next key and switches to it.

use asiri_proto::app::v1::{self as app, app_message::Body, call_signal::Kind};
use asiri_proto::relay::v1::{self as wire, client_frame::Body as C, server_frame::Body as S};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use super::outbound::SendOpts;
use super::{ActiveCall, CallRecord, CallSession, CallState, Core, CoreEvent, lock_err};
use crate::crypto::sas;
use crate::error::{CoreError, CoreResult};
use crate::ids::{Id16, now_ms, random_bytes, random_id};
use crate::mls;
use crate::net::bridge::Bridge;
use crate::store;

/// LiveKit's key ring has 16 slots by default.
const KEY_RING: u64 = 16;

/// Room aliases are 1–64 of `[A-Za-z0-9_-]` (what the relay accepts).
#[must_use]
pub fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 64
        && alias
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// "Voice call" from "voice call".
pub(crate) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// "45 s", "12 min", "1 h 5 min".
#[must_use]
pub fn describe_duration(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s / 60),
        _ => {
            let (h, m) = (s / 3600, (s % 3600) / 60);
            if m == 0 {
                format!("{h} h")
            } else {
                format!("{h} h {m} min")
            }
        }
    }
}

impl Core {
    fn call_key(
        &self,
        chat_id: &[u8],
        call_id: &Id16,
    ) -> CoreResult<(String, u32, Vec<sas::EmojiWord>, u64)> {
        let conn = self.inner.store.lock()?;
        let group = self.inner.mls.load(&conn, chat_id)?;
        let secret = self.inner.mls.call_secret(&group, call_id)?;
        let epoch = mls::epoch(&group);
        Ok((
            STANDARD.encode(secret.as_slice()),
            u32::try_from(epoch % KEY_RING).unwrap_or(0),
            sas::call_emoji(&secret),
            epoch,
        ))
    }

    async fn call_session(
        &self,
        chat_id: &[u8],
        call_id: Id16,
        room_alias: &str,
        video: bool,
    ) -> CoreResult<CallSession> {
        let client = self.client()?;
        let token = match client
            .request(C::CallToken(wire::CallTokenRequest {
                room_alias: room_alias.to_owned(),
            }))
            .await?
        {
            S::CallToken(t) => t,
            _ => return Err(CoreError::Network("unexpected reply".into())),
        };
        let bridge = Bridge::start(self.ensure_dialer().await?).await?;
        let url = bridge.url();
        let (key_base64, key_index, emoji, epoch) = self.call_key(chat_id, &call_id)?;
        self.inner.calls.lock().map_err(lock_err)?.insert(
            call_id,
            ActiveCall {
                chat_id: chat_id.to_vec(),
                epoch,
                _bridge: bridge,
            },
        );
        Ok(CallSession {
            call_id,
            chat_id: chat_id.to_vec(),
            room_alias: room_alias.to_owned(),
            url,
            token: token.jwt,
            participant_alias: token.identity,
            key_base64,
            key_index,
            emoji,
            video,
        })
    }

    fn send_call_signal(
        &self,
        chat_id: &[u8],
        kind: Kind,
        call_id: &Id16,
        room_alias: &str,
        video: bool,
        alias: &str,
    ) -> CoreResult<()> {
        let id = self.inner.identity.read().map_err(lock_err)?;
        self.inner.store.tx(|c| {
            let chat = store::chat(c, chat_id)?.ok_or(CoreError::NotFound)?;
            self.queue_app_message(
                c,
                &id,
                &chat,
                Body::Call(app::CallSignal {
                    kind: kind as i32,
                    call_id: call_id.to_vec(),
                    room_alias: room_alias.to_owned(),
                    video,
                    participant_alias: alias.to_owned(),
                }),
                // Not sequenced: an unanswered ring expiring on the relay is not a lost message.
                &SendOpts {
                    sequenced: false,
                    ttl_s: if kind == Kind::Ring { 120 } else { 0 },
                    ..SendOpts::default()
                },
            )
            .map(|_| ())
        })?;
        drop(id);
        self.wake_outbox();
        Ok(())
    }

    /// Starts a call: gets a token, opens the signaling bridge, derives the key and rings the
    /// chat (§10.2).
    pub async fn start_call(&self, chat_id: &[u8], video: bool) -> CoreResult<CallSession> {
        let call_id = random_id();
        let room_alias = format!("r{}", hex::encode(random_bytes::<10>()));
        let session = self
            .call_session(chat_id, call_id, &room_alias, video)
            .await?;
        let caller = self.user_id()?;
        self.inner.store.tx(|c| {
            c.execute(
                "INSERT INTO calls(call_id, chat_id, room_alias, video, outgoing, caller, state, started_ms)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, 0, ?6)",
                rusqlite::params![&call_id[..], chat_id, room_alias, i64::from(video), &caller[..], now_ms()],
            )?;
            Ok(())
        })?;
        self.send_call_signal(
            chat_id,
            Kind::Ring,
            &call_id,
            &room_alias,
            video,
            &session.participant_alias,
        )?;
        Ok(session)
    }

    /// Answers an incoming call.
    pub async fn accept_call(&self, call_id: &Id16) -> CoreResult<CallSession> {
        let (chat_id, room_alias, video): (Vec<u8>, String, bool) = {
            let conn = self.inner.store.lock()?;
            conn.query_row(
                "SELECT chat_id, room_alias, video FROM calls WHERE call_id = ?1",
                rusqlite::params![&call_id[..]],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
            )
            .map_err(|_| CoreError::NotFound)?
        };
        let session = self
            .call_session(&chat_id, *call_id, &room_alias, video)
            .await?;
        self.inner.store.tx(|c| {
            c.execute(
                "UPDATE calls SET state = 1, answered_ms = ?2 WHERE call_id = ?1",
                rusqlite::params![&call_id[..], now_ms()],
            )?;
            Ok(())
        })?;
        self.send_call_signal(
            &chat_id,
            Kind::Accept,
            call_id,
            &room_alias,
            video,
            &session.participant_alias,
        )?;
        Ok(session)
    }

    /// Declines an incoming call.
    pub fn decline_call(&self, call_id: &Id16) -> CoreResult<()> {
        let (chat_id, alias, video) = self.call_row(call_id)?;
        self.send_call_signal(&chat_id, Kind::Decline, call_id, &alias, video, "")?;
        self.inner.store.tx(|c| {
            c.execute(
                "UPDATE calls SET state = 2, ended_ms = ?2 WHERE call_id = ?1",
                rusqlite::params![&call_id[..], now_ms()],
            )?;
            Ok(())
        })?;
        self.emit(CoreEvent::CallUpdated { call_id: *call_id });
        Ok(())
    }

    /// Leaves or ends a call and closes its signaling bridge.
    pub fn end_call(&self, call_id: &Id16) -> CoreResult<()> {
        let (chat_id, alias, video) = self.call_row(call_id)?;
        self.inner.calls.lock().map_err(lock_err)?.remove(call_id);
        self.send_call_signal(&chat_id, Kind::End, call_id, &alias, video, "")?;
        let mut events = Vec::new();
        let (me, device) = self.me()?;
        self.inner.store.tx(|c| {
            let answered: Option<i64> = c
                .query_row(
                    "SELECT answered_ms FROM calls WHERE call_id = ?1 AND state != 2",
                    rusqlite::params![&call_id[..]],
                    |r| r.get(0),
                )
                .unwrap_or(None);
            let changed = c.execute(
                "UPDATE calls SET state = 2, ended_ms = ?2 WHERE call_id = ?1 AND state != 2",
                rusqlite::params![&call_id[..], now_ms()],
            )?;
            if changed > 0 {
                let kind = if video { "Video call" } else { "Voice call" };
                let text = match answered {
                    Some(a) => format!("{kind} · {}", describe_duration(now_ms() - a)),
                    None => format!("{kind} · cancelled"),
                };
                let message_id = random_id();
                let now = now_ms();
                store::insert_message(
                    c,
                    &store::NewMessage {
                        message_id,
                        chat_id: &chat_id,
                        sender_user: me,
                        sender_device: device,
                        outgoing: true,
                        kind: store::MessageKind::Call,
                        body: &text,
                        reply_to: None,
                        sent_at_ms: now,
                        received_at_ms: now,
                        status: store::MessageStatus::Sent,
                        expires_at_ms: None,
                    },
                )?;
                events.push(CoreEvent::MessageAdded {
                    chat_id: chat_id.clone(),
                    message_id,
                });
            }
            Ok(())
        })?;
        events.push(CoreEvent::CallUpdated { call_id: *call_id });
        self.emit_all(events);
        Ok(())
    }

    fn call_row(&self, call_id: &Id16) -> CoreResult<(Vec<u8>, String, bool)> {
        let conn = self.inner.store.lock()?;
        conn.query_row(
            "SELECT chat_id, room_alias, video FROM calls WHERE call_id = ?1",
            rusqlite::params![&call_id[..]],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
        )
        .map_err(|_| CoreError::NotFound)
    }

    /// The current key for an active call.
    pub fn current_call_key(&self, call_id: &Id16) -> CoreResult<(String, u32)> {
        let chat_id = self
            .inner
            .calls
            .lock()
            .map_err(lock_err)?
            .get(call_id)
            .map(|c| c.chat_id.clone())
            .ok_or(CoreError::NotFound)?;
        let (key, index, _, _) = self.call_key(&chat_id, call_id)?;
        Ok((key, index))
    }

    /// Re-derives keys for calls in a chat whose epoch changed (a member joined or left).
    pub(crate) fn rotate_call_keys(&self, chat_id: &[u8]) {
        let due: Vec<Id16> = match self.inner.calls.lock() {
            Ok(calls) => calls
                .iter()
                .filter(|(_, c)| c.chat_id == chat_id)
                .map(|(id, _)| *id)
                .collect(),
            Err(_) => return,
        };
        for call_id in due {
            let Ok((key_base64, key_index, _, epoch)) = self.call_key(chat_id, &call_id) else {
                continue;
            };
            let changed = self
                .inner
                .calls
                .lock()
                .ok()
                .and_then(|mut calls| {
                    calls
                        .get_mut(&call_id)
                        .map(|c| std::mem::replace(&mut c.epoch, epoch) != epoch)
                })
                .unwrap_or(false);
            if changed {
                self.emit(CoreEvent::CallKeyRotated {
                    call_id,
                    key_base64,
                    key_index,
                });
            }
        }
    }

    /// Whether this device is in the call right now.
    pub fn is_call_active(&self, call_id: &Id16) -> bool {
        self.inner
            .calls
            .lock()
            .is_ok_and(|c| c.contains_key(call_id))
    }

    fn record_from_row(&self, r: &rusqlite::Row<'_>) -> rusqlite::Result<CallRecord> {
        let call_id: Vec<u8> = r.get(0)?;
        let call_id = crate::ids::id16(&call_id).unwrap_or([0; 16]);
        let caller: Vec<u8> = r.get(4)?;
        Ok(CallRecord {
            call_id,
            chat_id: r.get(1)?,
            video: r.get::<_, i64>(2)? != 0,
            outgoing: r.get::<_, i64>(3)? != 0,
            caller: crate::ids::id16(&caller).unwrap_or([0; 16]),
            state: match r.get::<_, i64>(5)? {
                0 => CallState::Ringing,
                1 => CallState::Active,
                _ => CallState::Ended,
            },
            started_ms: r.get(6)?,
            answered_ms: r.get(7)?,
            ended_ms: r.get(8)?,
            joined: self.is_call_active(&call_id),
        })
    }

    /// One call.
    pub fn call(&self, call_id: &Id16) -> CoreResult<CallRecord> {
        let conn = self.inner.store.lock()?;
        conn.query_row(
            "SELECT call_id, chat_id, video, outgoing, caller, state, started_ms, answered_ms, ended_ms
             FROM calls WHERE call_id = ?1",
            rusqlite::params![&call_id[..]],
            |r| self.record_from_row(r),
        )
        .map_err(|_| CoreError::NotFound)
    }

    /// Recent calls, newest first (the Calls list).
    pub fn recent_calls(&self, limit: u32) -> CoreResult<Vec<CallRecord>> {
        let conn = self.inner.store.lock()?;
        let mut stmt = conn.prepare(
            "SELECT call_id, chat_id, video, outgoing, caller, state, started_ms, answered_ms, ended_ms
             FROM calls ORDER BY started_ms DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![i64::from(limit.min(500))], |r| {
                self.record_from_row(r)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_aliases() {
        assert_eq!(describe_duration(45_000), "45 s");
        assert_eq!(describe_duration(12 * 60_000), "12 min");
        assert_eq!(describe_duration(3_900_000), "1 h 5 min");
        assert!(valid_alias("r0123abc"));
        assert!(!valid_alias("bad alias"));
    }
}
