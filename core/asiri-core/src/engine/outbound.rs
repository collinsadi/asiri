//! Outgoing app messages (architecture §11.3 sender side, §12).
//!
//! Every send saves the message as `pending` and the MLS ciphertext in the outbox inside one
//! transaction, before any network activity; the bubble appears immediately and its status
//! follows the relay's answer.

use std::time::{Duration, Instant};

use asiri_proto::Message as _;
use asiri_proto::app::v1::{self as app, app_message::Body, receipt::Kind as ReceiptKind};
use asiri_proto::relay::v1::PayloadKind;
use rusqlite::Connection;

use super::{Core, CoreEvent, lock_err};
use crate::error::{CoreError, CoreResult};
use crate::identity::Identity;
use crate::ids::{Id16, now_ms, random_id};
use crate::mls;
use crate::store::{self, Chat, MessageKind, MessageStatus, NewMessage};

/// Options for one outgoing app message.
pub(crate) struct SendOpts {
    /// Message ID (random if `None`).
    pub message_id: Option<Id16>,
    /// Count this message for gap detection.
    pub sequenced: bool,
    /// Update this stored message's status as the envelope progresses.
    pub track: bool,
    /// Reply target.
    pub reply_to: Option<Id16>,
    /// Disappearing timer to announce.
    pub expires_in_s: u32,
    /// Relay TTL (0 = default).
    pub ttl_s: u32,
}

impl Default for SendOpts {
    fn default() -> Self {
        Self {
            message_id: None,
            sequenced: true,
            track: false,
            reply_to: None,
            expires_in_s: 0,
            ttl_s: 0,
        }
    }
}

/// Human description of a disappearing-message timer.
#[must_use]
pub fn describe_timer(seconds: u32) -> String {
    match seconds {
        0 => "off".into(),
        s if s % 604_800 == 0 => plural(s / 604_800, "week"),
        s if s % 86_400 == 0 => plural(s / 86_400, "day"),
        s if s % 3_600 == 0 => plural(s / 3_600, "hour"),
        s if s % 60 == 0 => plural(s / 60, "minute"),
        s => plural(s, "second"),
    }
}

fn plural(n: u32, unit: &str) -> String {
    if n == 1 {
        format!("1 {unit}")
    } else {
        format!("{n} {unit}s")
    }
}

impl Core {
    /// Encrypts an app message for every other member device and queues it. Returns the
    /// message ID and whether anyone else will receive it.
    pub(crate) fn queue_app_message(
        &self,
        conn: &Connection,
        id: &Identity,
        chat: &Chat,
        body: Body,
        opts: &SendOpts,
    ) -> CoreResult<(Id16, bool)> {
        if chat.left {
            return Err(CoreError::NotAllowed(
                "You're no longer in this chat".into(),
            ));
        }
        let reg = id.registered()?;
        let message_id = opts.message_id.unwrap_or_else(random_id);
        let sender_seq = if opts.sequenced {
            store::next_my_seq(conn, &chat.chat_id)?
        } else {
            0
        };
        let msg = app::AppMessage {
            message_id: message_id.to_vec(),
            sender_seq,
            sent_at_ms: now_ms(),
            expires_in_s: opts.expires_in_s,
            reply_to: opts.reply_to.map(|r| r.to_vec()).unwrap_or_default(),
            body: Some(body),
        };
        let group = self.inner.mls.load(conn, &chat.chat_id)?;
        if !mls::is_active(&group) {
            return Err(CoreError::NotAllowed(
                "You're no longer in this chat".into(),
            ));
        }
        if mls::members(&group)
            .iter()
            .all(|m| m.device_id == reg.device_id)
        {
            return Ok((message_id, false));
        }
        // Encrypted just before sending (see `Core::encrypt_queued`): MLS can't encrypt while a
        // proposal is pending, and late encryption always uses the newest epoch.
        store::enqueue_plain(
            conn,
            &random_id(),
            &chat.chat_id,
            opts.track.then_some(&message_id),
            PayloadKind::Application as i32,
            &msg.encode_to_vec(),
            opts.ttl_s,
        )?;
        Ok((message_id, true))
    }

    fn chat_or_err(conn: &Connection, chat_id: &[u8]) -> CoreResult<Chat> {
        store::chat(conn, chat_id)?.ok_or(CoreError::NotFound)
    }

    /// Stores and sends a text message (returns immediately; delivery happens in the background).
    pub fn send_text(
        &self,
        chat_id: &[u8],
        text: &str,
        reply_to: Option<Id16>,
    ) -> CoreResult<Id16> {
        let text = text.trim_end();
        if text.trim().is_empty() {
            return Err(CoreError::Invalid("Message is empty".into()));
        }
        if text.len() > 64 * 1024 {
            return Err(CoreError::Invalid(
                "Message is too long; send it as a file".into(),
            ));
        }
        let id = self.inner.identity.read().map_err(lock_err)?;
        let reg = id.registered()?;
        let mut events = Vec::new();
        let message_id = self.inner.store.tx(|c| {
            let chat = Self::chat_or_err(c, chat_id)?;
            let message_id = random_id();
            let now = now_ms();
            store::insert_message(
                c,
                &NewMessage {
                    message_id,
                    chat_id,
                    sender_user: id.user_id,
                    sender_device: reg.device_id,
                    outgoing: true,
                    kind: MessageKind::Text,
                    body: text,
                    reply_to,
                    sent_at_ms: now,
                    received_at_ms: now,
                    status: MessageStatus::Pending,
                    expires_at_ms: (chat.timer_s > 0).then(|| now + i64::from(chat.timer_s) * 1000),
                },
            )?;
            let (_, delivered) = self.queue_app_message(
                c,
                &id,
                &chat,
                Body::Text(app::Text {
                    body: text.to_owned(),
                    mentions: vec![],
                    link_preview: None,
                }),
                &SendOpts {
                    message_id: Some(message_id),
                    track: true,
                    reply_to,
                    expires_in_s: chat.timer_s,
                    ..SendOpts::default()
                },
            )?;
            if !delivered {
                store::set_status(c, &message_id, MessageStatus::Sent)?;
            }
            store::touch_chat(c, chat_id, now, 0)?;
            events.push(CoreEvent::MessageAdded {
                chat_id: chat_id.to_vec(),
                message_id,
            });
            events.push(CoreEvent::ChatsChanged);
            Ok(message_id)
        })?;
        drop(id);
        self.emit_all(events);
        self.wake_outbox();
        Ok(message_id)
    }

    fn mine_in_chat(
        conn: &Connection,
        me: &Id16,
        chat_id: &[u8],
        target: &Id16,
    ) -> CoreResult<store::Message> {
        let m = store::message(conn, me, target)?.ok_or(CoreError::NotFound)?;
        if m.chat_id != chat_id {
            return Err(CoreError::NotFound);
        }
        Ok(m)
    }

    /// Adds or removes a reaction.
    pub fn react(
        &self,
        chat_id: &[u8],
        target: &Id16,
        emoji: &str,
        remove: bool,
    ) -> CoreResult<()> {
        if emoji.is_empty() || emoji.chars().count() > 16 {
            return Err(CoreError::Invalid("Invalid reaction".into()));
        }
        let id = self.inner.identity.read().map_err(lock_err)?;
        self.inner.store.tx(|c| {
            let chat = Self::chat_or_err(c, chat_id)?;
            Self::mine_in_chat(c, &id.user_id, chat_id, target)?;
            store::set_reaction(c, target, &id.user_id, emoji, remove)?;
            self.queue_app_message(
                c,
                &id,
                &chat,
                Body::Reaction(app::Reaction {
                    target: target.to_vec(),
                    emoji: emoji.to_owned(),
                    remove,
                }),
                &SendOpts::default(),
            )?;
            Ok(())
        })?;
        drop(id);
        self.emit(CoreEvent::MessageUpdated {
            chat_id: chat_id.to_vec(),
            message_id: *target,
        });
        self.wake_outbox();
        Ok(())
    }

    /// Edits one of our own text messages (§12 "Edits and deletes").
    pub fn edit_message(&self, chat_id: &[u8], target: &Id16, text: &str) -> CoreResult<()> {
        if text.trim().is_empty() {
            return Err(CoreError::Invalid("Message is empty".into()));
        }
        let id = self.inner.identity.read().map_err(lock_err)?;
        let device = id.registered()?.device_id;
        self.inner.store.tx(|c| {
            let chat = Self::chat_or_err(c, chat_id)?;
            let m = Self::mine_in_chat(c, &id.user_id, chat_id, target)?;
            if !m.outgoing || m.sender_device != device || m.kind != MessageKind::Text || m.deleted
            {
                return Err(CoreError::NotAllowed(
                    "You can only edit your own messages".into(),
                ));
            }
            store::edit_message(c, target, text)?;
            self.queue_app_message(
                c,
                &id,
                &chat,
                Body::Edit(app::Edit {
                    target: target.to_vec(),
                    text: Some(app::Text {
                        body: text.to_owned(),
                        mentions: vec![],
                        link_preview: None,
                    }),
                }),
                &SendOpts::default(),
            )?;
            Ok(())
        })?;
        drop(id);
        self.emit(CoreEvent::MessageUpdated {
            chat_id: chat_id.to_vec(),
            message_id: *target,
        });
        self.emit(CoreEvent::ChatsChanged);
        self.wake_outbox();
        Ok(())
    }

    /// Deletes one of our own messages for everyone.
    pub fn delete_for_everyone(&self, chat_id: &[u8], target: &Id16) -> CoreResult<()> {
        let id = self.inner.identity.read().map_err(lock_err)?;
        let device = id.registered()?.device_id;
        self.inner.store.tx(|c| {
            let chat = Self::chat_or_err(c, chat_id)?;
            let m = Self::mine_in_chat(c, &id.user_id, chat_id, target)?;
            if !m.outgoing || m.sender_device != device {
                return Err(CoreError::NotAllowed(
                    "You can only delete your own messages for everyone".into(),
                ));
            }
            store::tombstone_message(c, target)?;
            self.queue_app_message(
                c,
                &id,
                &chat,
                Body::Delete(app::Delete {
                    target: target.to_vec(),
                }),
                &SendOpts::default(),
            )?;
            Ok(())
        })?;
        drop(id);
        let _ = std::fs::remove_file(self.attachment_path(target));
        self.emit(CoreEvent::MessageUpdated {
            chat_id: chat_id.to_vec(),
            message_id: *target,
        });
        self.emit(CoreEvent::ChatsChanged);
        self.wake_outbox();
        Ok(())
    }

    /// Removes a message from this Mac only.
    pub fn delete_for_me(&self, chat_id: &[u8], target: &Id16) -> CoreResult<()> {
        self.inner.store.tx(|c| {
            c.execute(
                "DELETE FROM messages WHERE message_id = ?1 AND chat_id = ?2",
                rusqlite::params![&target[..], chat_id],
            )?;
            Ok(())
        })?;
        let _ = std::fs::remove_file(self.attachment_path(target));
        self.emit(CoreEvent::MessageRemoved {
            chat_id: chat_id.to_vec(),
            message_id: *target,
        });
        self.emit(CoreEvent::ChatsChanged);
        Ok(())
    }

    /// Sends a typing indicator, at most every 4 seconds while active (§12: never stored,
    /// expires after 10 seconds).
    pub fn set_typing(&self, chat_id: &[u8], active: bool) -> CoreResult<()> {
        if !self
            .inner
            .settings
            .read()
            .map_err(lock_err)?
            .typing_indicators
        {
            return Ok(());
        }
        {
            let mut sent = self.inner.typing_sent.lock().map_err(lock_err)?;
            if active {
                if sent
                    .get(chat_id)
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(4))
                {
                    return Ok(());
                }
                sent.insert(chat_id.to_vec(), Instant::now());
            } else if sent.remove(chat_id).is_none() {
                return Ok(());
            }
        }
        let id = self.inner.identity.read().map_err(lock_err)?;
        self.inner.store.tx(|c| {
            let chat = Self::chat_or_err(c, chat_id)?;
            self.queue_app_message(
                c,
                &id,
                &chat,
                Body::Typing(app::Typing { active }),
                &SendOpts {
                    sequenced: false,
                    ttl_s: 30,
                    ..SendOpts::default()
                },
            )?;
            Ok(())
        })?;
        drop(id);
        self.wake_outbox();
        Ok(())
    }

    /// Marks a chat as read and, if enabled, sends read receipts.
    pub fn mark_read(&self, chat_id: &[u8]) -> CoreResult<()> {
        let send_receipts = self.inner.settings.read().map_err(lock_err)?.read_receipts;
        let id = self.inner.identity.read().map_err(lock_err)?;
        let queued = self.inner.store.tx(|c| {
            let chat = Self::chat_or_err(c, chat_id)?;
            let unread = store::unread_incoming(c, chat_id, i64::MAX)?;
            store::clear_unread(c, chat_id)?;
            if send_receipts && !unread.is_empty() && !chat.left {
                self.queue_app_message(
                    c,
                    &id,
                    &chat,
                    Body::Receipt(app::Receipt {
                        kind: ReceiptKind::Read as i32,
                        targets: unread.iter().map(|(m, _)| m.to_vec()).collect(),
                    }),
                    &SendOpts {
                        sequenced: false,
                        ..SendOpts::default()
                    },
                )?;
                return Ok(true);
            }
            Ok(false)
        })?;
        drop(id);
        self.emit(CoreEvent::ChatsChanged);
        if queued {
            self.wake_outbox();
        }
        Ok(())
    }

    pub(crate) fn queue_delivered_receipt(&self, chat_id: Vec<u8>, message_id: Id16) {
        if let Ok(mut due) = self.inner.receipts_due.lock() {
            due.entry(chat_id).or_default().push(message_id);
        }
    }

    /// Sends batched delivery receipts (called from the outbox loop).
    pub(crate) fn flush_receipts(&self) {
        let due: Vec<(Vec<u8>, Vec<Id16>)> = match self.inner.receipts_due.lock() {
            Ok(mut d) => d.drain().collect(),
            Err(_) => return,
        };
        if due.is_empty() {
            return;
        }
        let Ok(id) = self.inner.identity.read() else {
            return;
        };
        for (chat_id, ids) in due {
            let r = self.inner.store.tx(|c| {
                let chat = Self::chat_or_err(c, &chat_id)?;
                self.queue_app_message(
                    c,
                    &id,
                    &chat,
                    Body::Receipt(app::Receipt {
                        kind: ReceiptKind::Delivered as i32,
                        targets: ids.iter().map(|m| m.to_vec()).collect(),
                    }),
                    &SendOpts {
                        sequenced: false,
                        ..SendOpts::default()
                    },
                )
            });
            if let Err(e) = r {
                tracing::debug!("could not queue a delivery receipt: {e}");
            }
        }
    }

    /// Updates our name and photo and shares them with every chat.
    pub fn set_profile(&self, display_name: &str, avatar_jpeg: Option<Vec<u8>>) -> CoreResult<()> {
        let name = display_name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(CoreError::Invalid(
                "Choose a name between 1 and 64 characters".into(),
            ));
        }
        if avatar_jpeg.as_ref().is_some_and(|a| a.len() > 128 * 1024) {
            return Err(CoreError::Invalid("That photo is too large".into()));
        }
        let mut id = self.inner.identity.write().map_err(lock_err)?;
        let registered = id.registration.is_some();
        let chat_ids = self.inner.store.tx(|c| {
            id.set_profile(c, name, avatar_jpeg.clone())?;
            Ok(store::chats(c)?
                .into_iter()
                .filter(|ch| !ch.left)
                .map(|ch| ch.chat_id)
                .collect::<Vec<_>>())
        })?;
        drop(id);
        if registered {
            for chat_id in chat_ids {
                if let Err(e) = self.send_profile_to(&chat_id) {
                    tracing::debug!("profile not shared with a chat: {e}");
                }
            }
            self.wake_outbox();
        }
        Ok(())
    }

    /// Queues our current profile into one chat.
    pub(crate) fn send_profile_to(&self, chat_id: &[u8]) -> CoreResult<()> {
        let id = self.inner.identity.read().map_err(lock_err)?;
        if id.display_name.is_empty() && id.avatar.is_none() {
            return Ok(());
        }
        let avatar = id.avatar.clone().map(|jpeg| app::Attachment {
            mime_type: "image/jpeg".into(),
            thumbnail_jpeg: jpeg,
            ..Default::default()
        });
        self.inner.store.tx(|c| {
            let chat = Self::chat_or_err(c, chat_id)?;
            self.queue_app_message(
                c,
                &id,
                &chat,
                Body::Profile(app::ProfileUpdate {
                    display_name: id.display_name.clone(),
                    avatar,
                }),
                &SendOpts::default(),
            )?;
            Ok(())
        })?;
        drop(id);
        self.wake_outbox();
        Ok(())
    }

    /// Sets the disappearing-message timer for a chat (0 turns it off).
    pub fn set_disappearing_timer(&self, chat_id: &[u8], seconds: u32) -> CoreResult<()> {
        if seconds > 365 * 86_400 {
            return Err(CoreError::Invalid("Timer is too long".into()));
        }
        let id = self.inner.identity.read().map_err(lock_err)?;
        let device = id.registered()?.device_id;
        let mut events = Vec::new();
        self.inner.store.tx(|c| {
            let chat = Self::chat_or_err(c, chat_id)?;
            store::set_chat_timer(c, chat_id, seconds)?;
            self.queue_app_message(
                c,
                &id,
                &chat,
                Body::Timer(app::TimerChange { seconds }),
                &SendOpts::default(),
            )?;
            let text = if seconds == 0 {
                "You turned off disappearing messages".to_owned()
            } else {
                format!(
                    "You set disappearing messages to {}",
                    describe_timer(seconds)
                )
            };
            self.system_message(c, chat_id, (&id.user_id, &device), &text, &mut events)?;
            Ok(())
        })?;
        drop(id);
        events.push(CoreEvent::ChatUpdated(chat_id.to_vec()));
        self.emit_all(events);
        self.wake_outbox();
        Ok(())
    }

    /// Renames a group or changes its description or photo (admins only).
    pub fn update_group(
        &self,
        chat_id: &[u8],
        name: &str,
        description: &str,
        avatar_jpeg: Option<Vec<u8>>,
    ) -> CoreResult<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(CoreError::Invalid(
                "Group names are 1–100 characters".into(),
            ));
        }
        let id = self.inner.identity.read().map_err(lock_err)?;
        let device = id.registered()?.device_id;
        let mut events = Vec::new();
        self.inner.store.tx(|c| {
            let chat = Self::chat_or_err(c, chat_id)?;
            let group = self.inner.mls.load(c, chat_id)?;
            if !mls::admins(&group).contains(&id.user_id) {
                return Err(CoreError::NotAllowed(
                    "Only admins can change the group".into(),
                ));
            }
            store::set_chat_meta(c, chat_id, name, description, avatar_jpeg.as_deref())?;
            let avatar = avatar_jpeg.clone().map(|jpeg| app::Attachment {
                mime_type: "image/jpeg".into(),
                thumbnail_jpeg: jpeg,
                ..Default::default()
            });
            self.queue_app_message(
                c,
                &id,
                &chat,
                Body::Group(app::GroupUpdate {
                    name: name.to_owned(),
                    avatar,
                    description: description.to_owned(),
                }),
                &SendOpts::default(),
            )?;
            if !chat.name.is_empty() && chat.name != name {
                self.system_message(
                    c,
                    chat_id,
                    (&id.user_id, &device),
                    &format!("You renamed the group to “{name}”"),
                    &mut events,
                )?;
            }
            Ok(())
        })?;
        drop(id);
        events.push(CoreEvent::ChatUpdated(chat_id.to_vec()));
        events.push(CoreEvent::ChatsChanged);
        self.emit_all(events);
        self.wake_outbox();
        Ok(())
    }

    /// Re-sends a message that failed (text) or restarts a failed upload (attachments).
    pub async fn retry_message(&self, message_id: &Id16) -> CoreResult<()> {
        let m = self.message(message_id)?;
        if !m.outgoing || m.status != MessageStatus::Failed {
            return Ok(());
        }
        if m.kind == MessageKind::Attachment {
            return self.restart_upload(message_id).await;
        }
        let id = self.inner.identity.read().map_err(lock_err)?;
        self.inner.store.tx(|c| {
            let chat = Self::chat_or_err(c, &m.chat_id)?;
            store::set_status(c, message_id, MessageStatus::Pending)?;
            self.queue_app_message(
                c,
                &id,
                &chat,
                Body::Text(app::Text {
                    body: m.body.clone(),
                    mentions: vec![],
                    link_preview: None,
                }),
                &SendOpts {
                    message_id: Some(*message_id),
                    track: true,
                    reply_to: m.reply_to,
                    ..SendOpts::default()
                },
            )?;
            Ok(())
        })?;
        drop(id);
        self.emit(CoreEvent::MessageUpdated {
            chat_id: m.chat_id,
            message_id: *message_id,
        });
        self.wake_outbox();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::describe_timer;

    #[test]
    fn timers_read_naturally() {
        assert_eq!(describe_timer(604_800), "1 week");
        assert_eq!(describe_timer(2 * 86_400), "2 days");
        assert_eq!(describe_timer(3_600), "1 hour");
        assert_eq!(describe_timer(300), "5 minutes");
        assert_eq!(describe_timer(30), "30 seconds");
    }
}
