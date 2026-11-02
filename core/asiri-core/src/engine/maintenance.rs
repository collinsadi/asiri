//! Housekeeping (architecture §7.3, §12, §16.2).
//!
//! * Local work runs every few seconds whether or not the relay is reachable, so disappearing
//!   messages vanish on time even offline: expired messages and their files, overdue gaps,
//!   unanswered rings, and old replay-defence records.
//! * Relay work runs every minute while connected: topping up key packages, committing
//!   members' pending leave proposals, weekly self-updates, and the daily update check.

use std::collections::HashSet;
use std::sync::{Arc, Weak};
use std::time::Duration;

use asiri_proto::relay::v1::{self as wire, client_frame::Body as C, server_frame::Body as S};
use tokio::time::MissedTickBehavior;

use super::text::mention;
use super::{Core, CoreEvent, Inner, lock_err};
use crate::error::{CoreError, CoreResult};
use crate::ids::{Id16, now_ms, random_id};
use crate::mls;
use crate::net::relay::RelayClient;
use crate::store::{self, MessageKind, MessageStatus, NewMessage};

const DAY_MS: i64 = 86_400_000;
const LOCAL_TICK: Duration = Duration::from_secs(5);
const RELAY_TICK: Duration = Duration::from_secs(60);
/// Let the backlog drain before the first relay pass of a session.
const RELAY_FIRST_PASS: Duration = Duration::from_secs(10);
/// One-time key packages kept on the relay, plus one last-resort package (§7.3).
pub(crate) const KEY_PACKAGES: u32 = 20;
/// Top up once fewer than this many remain.
const KEY_PACKAGES_LOW: u32 = 15;
/// Replace the last-resort package well before its 30-day lifetime ends.
const LAST_RESORT_EVERY_MS: i64 = 7 * DAY_MS;
/// Self-update every active chat this often (§7.3).
const SELF_UPDATE_EVERY_MS: i64 = 7 * DAY_MS;
/// At most this many self-updates per pass, so a long sleep doesn't cause a burst.
const SELF_UPDATES_PER_PASS: usize = 3;
/// After sleeping this long, self-update everywhere at the next chance (§7.3).
pub const LONG_SLEEP_S: u64 = 6 * 3600;
/// Messages waiting for an epoch that never comes are dropped after this long.
const DEFERRED_RETENTION_MS: i64 = DAY_MS;
/// Remember processed envelope IDs a little longer than the relay keeps envelopes (30 days).
const PROCESSED_RETENTION_MS: i64 = 31 * DAY_MS;
/// A ring nobody answered or cancelled is over after this long.
const RING_TIMEOUT_MS: i64 = 2 * 60_000;

/// `(call_id, chat_id, outgoing, video, caller)` of a ring that timed out.
type StaleRing = (Vec<u8>, Vec<u8>, bool, bool, Vec<u8>);

const K_LAST_RESORT: &str = "maintenance.last_resort_ms";
const K_SELF_UPDATE_AFTER: &str = "maintenance.self_update_after_ms";

fn kv_i64(conn: &rusqlite::Connection, key: &str) -> CoreResult<i64> {
    Ok(store::kv_get(conn, key)?
        .and_then(|v| <[u8; 8]>::try_from(v.as_slice()).ok())
        .map_or(0, i64::from_le_bytes))
}

fn kv_set_i64(conn: &rusqlite::Connection, key: &str, value: i64) -> CoreResult<()> {
    store::kv_set(conn, key, &value.to_le_bytes())
}

/// Starts the local housekeeping task. It holds only a weak reference, so it never keeps a
/// closed core alive.
pub(super) fn spawn_local(inner: &Arc<Inner>) {
    let weak: Weak<Inner> = Arc::downgrade(inner);
    inner.rt.spawn(async move {
        let mut tick = tokio::time::interval(LOCAL_TICK);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut pass: u64 = 0;
        loop {
            tick.tick().await;
            let Some(inner) = weak.upgrade() else { return };
            let core = Core { inner };
            let n = pass;
            let done = tokio::task::spawn_blocking(move || core.local_pass(n)).await;
            if let Ok(Err(e)) = done {
                tracing::debug!("local maintenance: {e}");
            }
            pass = pass.wrapping_add(1);
        }
    });
}

impl Core {
    /// One local pass; `n` counts passes so slower chores run less often.
    fn local_pass(&self, n: u64) -> CoreResult<()> {
        self.purge_expired()?;
        self.report_gaps()?;
        self.report_stuck_groups()?;
        self.expire_rings()?;
        if n.is_multiple_of(720) {
            // about once an hour
            self.inner.store.tx(|c| {
                store::prune_processed(c, now_ms() - PROCESSED_RETENTION_MS)?;
                store::prune_deferred(c, now_ms() - DEFERRED_RETENTION_MS)
            })?;
        }
        Ok(())
    }

    /// Deletes messages whose disappearing timer ran out, and their files (§12).
    pub(crate) fn purge_expired(&self) -> CoreResult<()> {
        let removed = self.inner.store.tx(|c| store::purge_expired(c, now_ms()))?;
        if removed.is_empty() {
            return Ok(());
        }
        for (chat_id, message_id) in removed {
            let _ = std::fs::remove_file(self.attachment_path(&message_id));
            self.emit(CoreEvent::MessageRemoved {
                chat_id,
                message_id,
            });
        }
        self.emit(CoreEvent::ChatsChanged);
        Ok(())
    }

    /// Tells the chat when a sender's messages are missing and didn't turn up in time.
    fn report_gaps(&self) -> CoreResult<()> {
        let id = self.inner.identity.read().map_err(lock_err)?;
        if id.registration.is_none() {
            return Ok(());
        }
        let mut events = Vec::new();
        self.inner.store.tx(|c| {
            for (chat_id, device, _count) in
                store::take_overdue_gaps(c, now_ms() - self.inner.config.gap_grace_ms)?
            {
                let Some(chat) = store::chat(c, &chat_id)? else {
                    continue;
                };
                if chat.left {
                    continue;
                }
                let user = store::device(c, &device)?.map_or([0; 16], |d| d.user_id);
                let text = format!(
                    "Some messages from {} may not have arrived.",
                    mention(&user)
                );
                self.system_message(c, &chat_id, (&user, &device), &text, &mut events)?;
            }
            Ok(())
        })?;
        drop(id);
        if !events.is_empty() {
            events.push(CoreEvent::ChatsChanged);
        }
        self.emit_all(events);
        Ok(())
    }

    /// Tells the chat when messages have been waiting for a group change that never arrived:
    /// the relay withheld a commit, so this device can't follow the group any more (§18).
    fn report_stuck_groups(&self) -> CoreResult<()> {
        let id = self.inner.identity.read().map_err(lock_err)?;
        let Ok(reg) = id.registered() else {
            return Ok(());
        };
        let device = reg.device_id;
        let mut events = Vec::new();
        self.inner.store.tx(|c| {
            for chat_id in store::take_stuck_groups(c, now_ms() - self.inner.config.gap_grace_ms)? {
                let text = "Some messages in this chat can't be read: a change to the group never \
                            arrived. If this lasts, ask an admin to remove you and add you again.";
                self.system_message(c, &chat_id, (&id.user_id, &device), text, &mut events)?;
                events.push(CoreEvent::SecurityWarning {
                    chat_id: Some(chat_id),
                    text: "A chat is out of sync; the server may be withholding messages.".into(),
                });
            }
            Ok(())
        })?;
        drop(id);
        self.emit_all(events);
        Ok(())
    }

    /// Ends rings that nobody answered or cancelled (the caller went offline mid-ring).
    fn expire_rings(&self) -> CoreResult<()> {
        let active: HashSet<Id16> = self
            .inner
            .calls
            .lock()
            .map_err(lock_err)?
            .keys()
            .copied()
            .collect();
        let now = now_ms();
        let mut events = Vec::new();
        self.inner.store.tx(|c| {
            let stale: Vec<StaleRing> = {
                let mut stmt = c.prepare(
                    "SELECT call_id, chat_id, outgoing, video, caller FROM calls
                     WHERE state = 0 AND started_ms < ?1",
                )?;
                stmt.query_map(rusqlite::params![now - RING_TIMEOUT_MS], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get::<_, i64>(2)? != 0,
                        r.get::<_, i64>(3)? != 0,
                        r.get(4)?,
                    ))
                })?
                .collect::<Result<_, _>>()?
            };
            for (call_id, chat_id, outgoing, video, caller) in stale {
                let Some(call) = crate::ids::id16(&call_id) else {
                    continue;
                };
                if outgoing && active.contains(&call) {
                    continue; // still waiting in the room for someone to join
                }
                c.execute(
                    "UPDATE calls SET state = 2, ended_ms = ?2 WHERE call_id = ?1",
                    rusqlite::params![call_id, now],
                )?;
                if store::chat(c, &chat_id)?.is_none() {
                    continue;
                }
                let kind = if video { "video call" } else { "voice call" };
                let (text, status) = if outgoing {
                    (
                        format!("{} · not answered", super::calls::capitalize(kind)),
                        MessageStatus::Sent,
                    )
                } else {
                    (format!("Missed {kind}"), MessageStatus::Received)
                };
                let message_id = random_id();
                let sender = crate::ids::id16(&caller).unwrap_or([0; 16]);
                store::insert_message(
                    c,
                    &NewMessage {
                        message_id,
                        chat_id: &chat_id,
                        sender_user: sender,
                        sender_device: [0; 16],
                        outgoing,
                        kind: MessageKind::Call,
                        body: &text,
                        reply_to: None,
                        sent_at_ms: now,
                        received_at_ms: now,
                        status,
                        expires_at_ms: None,
                    },
                )?;
                store::touch_chat(c, &chat_id, now, 0)?;
                events.push(CoreEvent::MessageAdded {
                    chat_id,
                    message_id,
                });
                events.push(CoreEvent::CallUpdated { call_id: call });
            }
            Ok(())
        })?;
        self.emit_all(events);
        Ok(())
    }

    /// The Mac woke from sleep (§6.7): reconnect right away and, after a long sleep, rotate our
    /// keys in every chat at the next chance (§7.3 "after waking from a long sleep").
    pub fn notify_wake(&self, asleep_s: u64) -> CoreResult<()> {
        if asleep_s >= LONG_SLEEP_S {
            self.inner
                .store
                .tx(|c| kv_set_i64(c, K_SELF_UPDATE_AFTER, now_ms()))?;
        }
        self.notify_network_change();
        Ok(())
    }

    // ─── relay work ────────────────────────────────────────────────────────

    /// Runs relay housekeeping every minute for as long as `client`'s session lasts.
    pub(crate) async fn maintenance_loop(&self, client: Arc<RelayClient>) {
        let mut tick =
            tokio::time::interval_at(tokio::time::Instant::now() + RELAY_FIRST_PASS, RELAY_TICK);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if client.is_closed() {
                return;
            }
            if let Err(e) = self.relay_pass().await {
                if e.is_transient() {
                    return; // the session is ending; the next one starts a new loop
                }
                tracing::debug!("maintenance: {e}");
            }
        }
    }

    async fn relay_pass(&self) -> CoreResult<()> {
        self.replenish_key_packages().await?;
        self.commit_stalled_proposals().await?;
        self.self_update_due_chats().await?;
        self.check_for_update_if_due().await?;
        Ok(())
    }

    /// Keeps 20 one-time key packages and a fresh last-resort package on the relay so friends
    /// can add this device while it's offline (§7.3).
    pub(crate) async fn replenish_key_packages(&self) -> CoreResult<()> {
        let _serial = self.inner.key_package_lock.lock().await;
        let client = self.client()?;
        let count = match client
            .request(C::KeyPackageCount(wire::KeyPackageCountRequest {}))
            .await?
        {
            S::KeyPackageCount(c) => c,
            _ => return Err(CoreError::Network("unexpected reply".into())),
        };
        let last_resort_at = {
            let conn = self.inner.store.lock()?;
            kv_i64(&conn, K_LAST_RESORT)?
        };
        let refresh_last_resort =
            !count.has_last_resort || now_ms() - last_resort_at >= LAST_RESORT_EVERY_MS;
        let missing = if count.one_time < KEY_PACKAGES_LOW {
            KEY_PACKAGES - count.one_time
        } else {
            0
        };
        if missing == 0 && !refresh_last_resort {
            return Ok(());
        }
        let core = self.clone();
        let (one_time, last_resort) =
            tokio::task::spawn_blocking(move || -> CoreResult<(Vec<Vec<u8>>, Vec<u8>)> {
                let id = core.inner.identity.read().map_err(lock_err)?;
                let credential = id.credential()?;
                core.inner.store.tx(|c| {
                    let mut one_time = Vec::with_capacity(missing as usize);
                    for _ in 0..missing {
                        one_time.push(core.inner.mls.key_package(
                            c,
                            &id.signer,
                            credential.clone(),
                            false,
                        )?);
                    }
                    let last_resort = if refresh_last_resort {
                        core.inner
                            .mls
                            .key_package(c, &id.signer, credential.clone(), true)?
                    } else {
                        Vec::new()
                    };
                    Ok((one_time, last_resort))
                })
            })
            .await
            .map_err(CoreError::internal)??;
        match client
            .request(C::PutKeyPackages(wire::PutKeyPackages {
                key_packages: one_time,
                last_resort,
            }))
            .await?
        {
            S::Ok(_) => {}
            _ => return Err(CoreError::Network("unexpected reply".into())),
        }
        if refresh_last_resort {
            self.inner
                .store
                .tx(|c| kv_set_i64(c, K_LAST_RESORT, now_ms()))?;
        }
        Ok(())
    }

    /// Chats where we're a member and someone's proposal (a leave) is waiting for a commit.
    fn chats_with_pending_proposals(&self) -> CoreResult<Vec<Vec<u8>>> {
        let conn = self.inner.store.lock()?;
        let mut out = Vec::new();
        for chat in store::chats(&conn)?.into_iter().filter(|c| !c.left) {
            if let Ok(group) = self.inner.mls.load(&conn, &chat.chat_id)
                && mls::is_active(&group)
                && group.has_pending_proposals()
            {
                out.push(chat.chat_id);
            }
        }
        Ok(out)
    }

    /// Commits leave proposals that nobody committed yet (the member who saw them first may
    /// have gone offline).
    async fn commit_stalled_proposals(&self) -> CoreResult<()> {
        for chat_id in self.chats_with_pending_proposals()? {
            if let Err(e) = self.commit_pending_proposals(&chat_id).await {
                if e.is_transient() {
                    return Err(e);
                }
                tracing::debug!("could not commit a pending proposal: {e}");
            }
        }
        Ok(())
    }

    /// Weekly (and after a long sleep) key rotation in every active chat (§7.3).
    async fn self_update_due_chats(&self) -> CoreResult<()> {
        let now = now_ms();
        let due: Vec<Vec<u8>> = {
            let conn = self.inner.store.lock()?;
            let after = kv_i64(&conn, K_SELF_UPDATE_AFTER)?;
            let mut due = Vec::new();
            for chat in store::chats(&conn)?.into_iter().filter(|c| !c.left) {
                let last = store::last_self_update(&conn, &chat.chat_id)?.max(chat.created_ms);
                if (last < after || now - last >= SELF_UPDATE_EVERY_MS)
                    && self
                        .inner
                        .mls
                        .load(&conn, &chat.chat_id)
                        .is_ok_and(|g| mls::is_active(&g))
                {
                    due.push((last, chat.chat_id));
                }
            }
            due.sort();
            due.into_iter()
                .take(SELF_UPDATES_PER_PASS)
                .map(|(_, id)| id)
                .collect()
        };
        for chat_id in due {
            match self.self_update(&chat_id).await {
                Ok(()) => {}
                Err(e) if e.is_transient() => return Err(e),
                Err(e) => {
                    tracing::debug!("self-update failed: {e}");
                    // Try again in about a day rather than every minute.
                    self.inner.store.tx(|c| {
                        store::set_last_self_update(
                            c,
                            &chat_id,
                            now - SELF_UPDATE_EVERY_MS + DAY_MS,
                        )
                    })?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_numbers_round_trip() {
        let store = store::Store::open_in_memory().unwrap();
        store.tx(|c| kv_set_i64(c, "k", 1_234_567_890_123)).unwrap();
        let conn = store.lock().unwrap();
        assert_eq!(kv_i64(&conn, "k").unwrap(), 1_234_567_890_123);
        assert_eq!(kv_i64(&conn, "missing").unwrap(), 0);
    }
}
