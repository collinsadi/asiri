//! Incoming envelopes (architecture §11.3 receiver side, §12 rules).
//!
//! Each envelope is processed in one transaction: replay check, MLS decryption (which advances
//! the ratchet), and the resulting app-data changes. The MLS step runs inside a savepoint, so a
//! message that fails to decrypt or breaks the admin policy leaves no partial state behind, but
//! is still marked as processed and acknowledged.

use asiri_proto::Message as _;
use asiri_proto::app::v1::{
    self as app, app_message::Body, call_signal::Kind as CallKind, receipt::Kind as ReceiptKind,
};
use asiri_proto::relay::v1::{self as wire, PayloadKind};
use rusqlite::Connection;

use super::{CallInvite, Core, CoreEvent, lock_err};
use crate::crypto::padding;
use crate::error::{CoreError, CoreResult};
use crate::identity::Identity;
use crate::ids::{Id16, id16, now_ms};
use crate::mls::{self, Incoming, MemberInfo, MlsGroup};
use crate::store::{
    self, AttachmentInfo, AttachmentState, Chat, ChatKind, ContactDevice, MessageKind,
    MessageStatus, NewMessage, Verification,
};

/// Work to do after the transaction commits.
#[derive(Debug)]
pub(crate) enum FollowUp {
    /// Fetch an attachment in the background.
    Download(Id16),
    /// Commit a member's pending leave proposal.
    CommitProposals(Vec<u8>),
    /// Share our profile in a chat we just joined.
    SendProfile(Vec<u8>),
    /// Queue a delivery receipt.
    Delivered(Vec<u8>, Id16),
    /// The group's epoch changed: rotate keys of any call in this chat.
    RotateCallKeys(Vec<u8>),
}

const MAX_TEXT: usize = 64 * 1024;
const MAX_NAME: usize = 64;

fn clamp(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

impl Core {
    /// Processes one delivery; returns whether it may be acknowledged.
    pub(crate) async fn handle_delivery(&self, d: wire::Deliver) -> bool {
        let core = self.clone();
        let result = tokio::task::spawn_blocking(move || core.process_envelope(&d)).await;
        match result {
            Ok(Ok((events, follow_ups))) => {
                self.emit_all(events);
                for f in follow_ups {
                    self.follow_up(f);
                }
                true
            }
            Ok(Err(CoreError::Storage(e))) => {
                // Possibly transient (disk full, lock): leave it for redelivery.
                tracing::warn!("could not store an incoming message: {e}");
                false
            }
            Ok(Err(e)) => {
                tracing::warn!("dropped an incoming envelope: {e}");
                true
            }
            Err(e) => {
                tracing::warn!("envelope task failed: {e}");
                false
            }
        }
    }

    fn follow_up(&self, f: FollowUp) {
        let core = self.clone();
        match f {
            FollowUp::Download(message_id) => {
                self.inner.rt.spawn(async move {
                    if let Err(e) = core.download_attachment(&message_id).await {
                        tracing::debug!("auto-download failed: {e}");
                    }
                });
            }
            FollowUp::CommitProposals(chat_id) => {
                self.inner.rt.spawn(async move {
                    if let Err(e) = core.commit_pending_proposals(&chat_id).await {
                        tracing::debug!("could not commit a pending proposal: {e}");
                    }
                });
            }
            FollowUp::SendProfile(chat_id) => {
                if let Err(e) = self.send_profile_to(&chat_id) {
                    tracing::debug!("could not share the profile: {e}");
                }
            }
            FollowUp::Delivered(chat_id, message_id) => {
                self.queue_delivered_receipt(chat_id, message_id)
            }
            FollowUp::RotateCallKeys(chat_id) => self.rotate_call_keys(&chat_id),
        }
    }

    fn process_envelope(&self, d: &wire::Deliver) -> CoreResult<(Vec<CoreEvent>, Vec<FollowUp>)> {
        let id = self.inner.identity.read().map_err(lock_err)?;
        let mut events = Vec::new();
        let mut follow = Vec::new();
        self.inner.store.tx(|conn| {
            if !store::mark_envelope_processed(conn, &d.envelope_id, now_ms())? {
                return Ok(()); // a replay or duplicate (§18 malicious relay)
            }
            let payload = padding::unpad_envelope(&d.payload)?;
            conn.execute_batch("SAVEPOINT envelope")?;
            let outcome = match PayloadKind::try_from(d.kind) {
                Ok(PayloadKind::Welcome) => {
                    self.on_welcome(conn, &id, payload, &mut events, &mut follow)
                }
                Ok(PayloadKind::Application | PayloadKind::Commit | PayloadKind::Proposal) => {
                    self.on_mls(conn, &id, payload, &mut events, &mut follow)
                }
                _ => Ok(()),
            };
            match outcome {
                Ok(()) => conn.execute_batch("RELEASE envelope")?,
                Err(e) => {
                    conn.execute_batch("ROLLBACK TO envelope; RELEASE envelope")?;
                    events.clear();
                    follow.clear();
                    if let CoreError::NotAllowed(text) = &e {
                        events.push(CoreEvent::SecurityWarning {
                            chat_id: None,
                            text: text.clone(),
                        });
                    }
                    tracing::warn!("rejected an incoming message: {e}");
                }
            }
            Ok(())
        })?;
        Ok((events, follow))
    }

    /// Records a member we learned about from MLS, warning when a known device's key changes.
    fn learn_member(
        &self,
        conn: &Connection,
        m: &MemberInfo,
        events: &mut Vec<CoreEvent>,
    ) -> CoreResult<()> {
        store::upsert_contact(conn, &m.user_id, None)?;
        if let Some(existing) = store::device(conn, &m.device_id)?
            && existing.signature_key != m.signature_key
        {
            // §7.6 "Key changes": anything not linked by a verified device is blocked until re-verified.
            store::set_verification(conn, &m.user_id, Verification::Changed)?;
            events.push(CoreEvent::SecurityWarning {
                chat_id: None,
                text:
                    "A friend's security key changed. Verify them again before trusting this chat."
                        .into(),
            });
            return Ok(());
        }
        store::upsert_device(
            conn,
            &ContactDevice {
                device_id: m.device_id,
                user_id: m.user_id,
                inbox_id: m.inbox_id,
                signature_key: m.signature_key.clone(),
            },
        )
    }

    pub(crate) fn name_of(&self, conn: &Connection, id: &Identity, user: &Id16) -> String {
        if *user == id.user_id {
            return "You".into();
        }
        store::contact(conn, user)
            .ok()
            .flatten()
            .map(|c| c.display_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Someone".into())
    }

    pub(crate) fn system_message(
        &self,
        conn: &Connection,
        chat_id: &[u8],
        author: (&Id16, &Id16),
        text: &str,
        events: &mut Vec<CoreEvent>,
    ) -> CoreResult<()> {
        let now = now_ms();
        let message_id = crate::ids::random_id();
        store::insert_message(
            conn,
            &NewMessage {
                message_id,
                chat_id,
                sender_user: *author.0,
                sender_device: *author.1,
                outgoing: false,
                kind: MessageKind::System,
                body: text,
                reply_to: None,
                sent_at_ms: now,
                received_at_ms: now,
                status: MessageStatus::Received,
                expires_at_ms: None,
            },
        )?;
        store::touch_chat(conn, chat_id, now, 0)?;
        events.push(CoreEvent::MessageAdded {
            chat_id: chat_id.to_vec(),
            message_id,
        });
        Ok(())
    }

    fn on_welcome(
        &self,
        conn: &Connection,
        id: &Identity,
        payload: &[u8],
        events: &mut Vec<CoreEvent>,
        follow: &mut Vec<FollowUp>,
    ) -> CoreResult<()> {
        let (group, inviter) = self.inner.mls.join(conn, payload)?;
        let meta = mls::group_meta(&group)
            .ok_or_else(|| CoreError::Crypto("missing group metadata".into()))?;
        let chat_id = group.group_id().as_slice().to_vec();
        if store::chat(conn, &chat_id)?.is_some()
            || store::chat_by_route(conn, &meta.route)?.is_some()
        {
            return Ok(());
        }
        let members = mls::members(&group);
        for m in members.iter().filter(|m| m.user_id != id.user_id) {
            self.learn_member(conn, m, events)?;
        }
        let paired = store::take_pending_invite(conn, &inviter.signature_key)?;
        let dm_user = (meta.kind == ChatKind::Direct)
            .then(|| {
                members
                    .iter()
                    .find(|m| m.user_id != id.user_id)
                    .map(|m| m.user_id)
            })
            .flatten();
        let now = now_ms();
        store::insert_chat(
            conn,
            &Chat {
                chat_id: chat_id.clone(),
                kind: meta.kind,
                group_route: meta.route,
                name: String::new(),
                description: String::new(),
                avatar: None,
                dm_user,
                suite: mls::suite_code(),
                created_ms: now,
                last_activity_ms: now,
                unread: 0,
                muted: false,
                pinned: false,
                archived: false,
                timer_s: 0,
                left: false,
            },
        )?;
        let text = match meta.kind {
            ChatKind::Direct => {
                "Messages in this chat are end-to-end encrypted. Only the two of you can read them."
                    .to_owned()
            }
            ChatKind::Group => format!(
                "{} added you to the group",
                self.name_of(conn, id, &inviter.user_id)
            ),
        };
        self.system_message(
            conn,
            &chat_id,
            (&inviter.user_id, &inviter.device_id),
            &text,
            events,
        )?;
        events.push(CoreEvent::ChatsChanged);
        if paired.is_some() {
            events.push(CoreEvent::PairingCompleted {
                chat_id: chat_id.clone(),
                user_id: inviter.user_id,
            });
        }
        follow.push(FollowUp::SendProfile(chat_id));
        Ok(())
    }

    fn on_mls(
        &self,
        conn: &Connection,
        id: &Identity,
        payload: &[u8],
        events: &mut Vec<CoreEvent>,
        follow: &mut Vec<FollowUp>,
    ) -> CoreResult<()> {
        let (gid, message) = mls::parse_protocol_message(payload)?;
        let Some(chat) = store::chat(conn, &gid)? else {
            return Ok(()); // not a chat we're in
        };
        let mut group = self.inner.mls.load(conn, &gid)?;
        match self.inner.mls.process(conn, &mut group, message)? {
            Incoming::Application { sender, plaintext } => {
                if store::contact(conn, &sender.user_id)?.is_some_and(|c| c.blocked) {
                    return Ok(());
                }
                self.on_app_message(conn, id, &chat, &group, &sender, &plaintext, events, follow)
            }
            Incoming::Commit { sender, summary } => {
                for m in &summary.added {
                    self.learn_member(conn, m, events)?;
                }
                let me = (id.user_id, id.registered()?.device_id);
                let by = self.name_of(conn, id, &sender.user_id);
                if !summary.added.is_empty() {
                    let names: Vec<String> = summary
                        .added
                        .iter()
                        .map(|m| self.name_of(conn, id, &m.user_id))
                        .collect();
                    self.system_message(
                        conn,
                        &chat.chat_id,
                        (&sender.user_id, &sender.device_id),
                        &format!("{by} added {}", names.join(", ")),
                        events,
                    )?;
                }
                for m in &summary.removed {
                    let text = if m.user_id == sender.user_id || m.device_id == sender.device_id {
                        format!("{} left", self.name_of(conn, id, &m.user_id))
                    } else if m.user_id == me.0 {
                        format!("{by} removed you")
                    } else {
                        format!("{by} removed {}", self.name_of(conn, id, &m.user_id))
                    };
                    self.system_message(
                        conn,
                        &chat.chat_id,
                        (&sender.user_id, &sender.device_id),
                        &text,
                        events,
                    )?;
                }
                if summary.self_removed {
                    store::set_chat_flag(conn, &chat.chat_id, "left_chat", true)?;
                }
                if summary.extensions_changed {
                    self.system_message(
                        conn,
                        &chat.chat_id,
                        (&sender.user_id, &sender.device_id),
                        &format!("{by} changed who can manage the group"),
                        events,
                    )?;
                }
                events.push(CoreEvent::ChatUpdated(chat.chat_id.clone()));
                follow.push(FollowUp::RotateCallKeys(chat.chat_id.clone()));
                Ok(())
            }
            Incoming::Proposal { self_remove, .. } => {
                // Any member may commit someone's own removal (§7.5), and must: MLS refuses to
                // encrypt application messages while a proposal is pending.
                if self_remove {
                    follow.push(FollowUp::CommitProposals(chat.chat_id));
                }
                Ok(())
            }
            Incoming::Own => Ok(()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn on_app_message(
        &self,
        conn: &Connection,
        id: &Identity,
        chat: &Chat,
        group: &MlsGroup,
        sender: &MemberInfo,
        plaintext: &[u8],
        events: &mut Vec<CoreEvent>,
        follow: &mut Vec<FollowUp>,
    ) -> CoreResult<()> {
        let msg = app::AppMessage::decode(plaintext)?;
        let Some(message_id) = id16(&msg.message_id) else {
            return Err(CoreError::Crypto("malformed message id".into()));
        };
        let now = now_ms();
        let chat_id = chat.chat_id.as_slice();
        if msg.sender_seq > 0 {
            store::observe_seq(conn, chat_id, &sender.device_id, msg.sender_seq, now)?;
        }
        let expires_at_ms =
            (msg.expires_in_s > 0).then(|| now + i64::from(msg.expires_in_s) * 1000);
        let reply_to = id16(&msg.reply_to);
        let base = NewMessage {
            message_id,
            chat_id,
            sender_user: sender.user_id,
            sender_device: sender.device_id,
            outgoing: false,
            kind: MessageKind::Text,
            body: "",
            reply_to,
            sent_at_ms: msg.sent_at_ms,
            received_at_ms: now,
            status: MessageStatus::Received,
            expires_at_ms,
        };
        let changed = |events: &mut Vec<CoreEvent>, m: Id16| {
            events.push(CoreEvent::MessageUpdated {
                chat_id: chat_id.to_vec(),
                message_id: m,
            });
        };
        // Targets of reactions, edits, deletes and receipts must be messages in this chat.
        let target_in_chat = |t: &[u8]| -> CoreResult<Option<store::Message>> {
            let Some(t) = id16(t) else { return Ok(None) };
            Ok(store::message(conn, &id.user_id, &t)?.filter(|m| m.chat_id == chat_id))
        };

        match msg.body {
            Some(Body::Text(t)) => {
                let body = clamp(&t.body, MAX_TEXT);
                if store::insert_message(
                    conn,
                    &NewMessage {
                        body: &body,
                        ..base
                    },
                )? {
                    store::touch_chat(conn, chat_id, msg.sent_at_ms.min(now), 1)?;
                    events.push(CoreEvent::MessageAdded {
                        chat_id: chat_id.to_vec(),
                        message_id,
                    });
                    events.push(CoreEvent::Notify {
                        chat_id: chat_id.to_vec(),
                        message_id,
                        sender: sender.user_id,
                    });
                    events.push(CoreEvent::ChatsChanged);
                    follow.push(FollowUp::Delivered(chat_id.to_vec(), message_id));
                }
            }
            Some(Body::Attachment(a)) => {
                let (Some(blob_id), true, true) = (
                    id16(&a.blob_id),
                    a.key.len() == 32,
                    a.ciphertext_sha256.len() == 32,
                ) else {
                    return Err(CoreError::Crypto("malformed attachment".into()));
                };
                if a.size > 200 * 1024 * 1024 {
                    return Err(CoreError::Crypto("attachment too large".into()));
                }
                let caption = clamp(&a.caption, MAX_TEXT);
                if store::insert_message(
                    conn,
                    &NewMessage {
                        kind: MessageKind::Attachment,
                        body: &caption,
                        ..base
                    },
                )? {
                    let info = AttachmentInfo {
                        blob_id,
                        key: a.key.clone(),
                        sha256: a.ciphertext_sha256.clone(),
                        size: a.size,
                        blob_size: a.blob_size,
                        mime: clamp(&a.mime_type, 128),
                        file_name: clamp(&a.file_name, 255),
                        width: a.width,
                        height: a.height,
                        duration_ms: a.duration_ms,
                        thumbnail: (!a.thumbnail_jpeg.is_empty()
                            && a.thumbnail_jpeg.len() <= 64 * 1024)
                            .then(|| a.thumbnail_jpeg.clone()),
                        blurhash: clamp(&a.blurhash, 128),
                        voice_note: a.voice_note,
                        waveform: (!a.waveform.is_empty() && a.waveform.len() <= 512)
                            .then(|| a.waveform.clone()),
                        state: Some(AttachmentState::Remote),
                        progress: 0.0,
                    };
                    store::insert_attachment(conn, &message_id, &info)?;
                    store::touch_chat(conn, chat_id, msg.sent_at_ms.min(now), 1)?;
                    events.push(CoreEvent::MessageAdded {
                        chat_id: chat_id.to_vec(),
                        message_id,
                    });
                    events.push(CoreEvent::Notify {
                        chat_id: chat_id.to_vec(),
                        message_id,
                        sender: sender.user_id,
                    });
                    events.push(CoreEvent::ChatsChanged);
                    follow.push(FollowUp::Delivered(chat_id.to_vec(), message_id));
                    let settings = self.inner.settings.read().map_err(lock_err)?;
                    let auto = (info.mime.starts_with("image/") && settings.auto_download_images)
                        || a.voice_note;
                    if auto && info.size <= settings.auto_download_max_bytes {
                        follow.push(FollowUp::Download(message_id));
                    }
                }
            }
            Some(Body::Reaction(r)) => {
                if let Some(target) = target_in_chat(&r.target)? {
                    let emoji = clamp(&r.emoji, 16);
                    if !emoji.is_empty() {
                        store::set_reaction(
                            conn,
                            &target.message_id,
                            &sender.user_id,
                            &emoji,
                            r.remove,
                        )?;
                        changed(events, target.message_id);
                    }
                }
            }
            Some(Body::Edit(e)) => {
                if let Some(target) = target_in_chat(&e.target)?
                    && target.sender_device == sender.device_id
                    && target.kind == MessageKind::Text
                {
                    let body = clamp(&e.text.map(|t| t.body).unwrap_or_default(), MAX_TEXT);
                    store::edit_message(conn, &target.message_id, &body)?;
                    changed(events, target.message_id);
                    events.push(CoreEvent::ChatsChanged);
                }
            }
            Some(Body::Delete(d)) => {
                if let Some(target) = target_in_chat(&d.target)?
                    && target.sender_device == sender.device_id
                {
                    store::tombstone_message(conn, &target.message_id)?;
                    let _ = std::fs::remove_file(self.attachment_path(&target.message_id));
                    changed(events, target.message_id);
                    events.push(CoreEvent::ChatsChanged);
                }
            }
            Some(Body::Receipt(r)) => {
                let read = r.kind == ReceiptKind::Read as i32;
                let recipients = mls::members(group)
                    .iter()
                    .map(|m| m.user_id)
                    .filter(|u| *u != id.user_id)
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                for t in r.targets.iter().take(1000) {
                    if let Some(target) = target_in_chat(t)?
                        && target.outgoing
                    {
                        store::record_receipt(
                            conn,
                            &target.message_id,
                            &sender.user_id,
                            read,
                            now,
                        )?;
                        let (delivered, read_n) = store::receipt_counts(conn, &target.message_id)?;
                        let n = u32::try_from(recipients.max(1)).unwrap_or(1);
                        let status = if read_n >= n {
                            MessageStatus::Read
                        } else if delivered >= n {
                            MessageStatus::Delivered
                        } else {
                            continue;
                        };
                        if store::set_status(conn, &target.message_id, status)? {
                            changed(events, target.message_id);
                        }
                    }
                }
            }
            Some(Body::Typing(t)) => {
                events.push(CoreEvent::Typing {
                    chat_id: chat_id.to_vec(),
                    user_id: sender.user_id,
                    active: t.active,
                });
            }
            Some(Body::Profile(p)) => {
                store::upsert_contact(
                    conn,
                    &sender.user_id,
                    Some(&clamp(&p.display_name, MAX_NAME)),
                )?;
                if let Some(avatar) = p.avatar.filter(|a| {
                    !a.thumbnail_jpeg.is_empty() && a.thumbnail_jpeg.len() <= 128 * 1024
                }) {
                    store::set_contact_avatar(conn, &sender.user_id, Some(&avatar.thumbnail_jpeg))?;
                }
                events.push(CoreEvent::ContactsChanged);
                events.push(CoreEvent::ChatsChanged);
            }
            Some(Body::Group(g)) => {
                if !mls::admins(group).contains(&sender.user_id) {
                    return Err(CoreError::NotAllowed(
                        "Ignored a group change from someone who isn't an admin".into(),
                    ));
                }
                let name = clamp(&g.name, 100);
                let avatar = g
                    .avatar
                    .as_ref()
                    .map(|a| a.thumbnail_jpeg.clone())
                    .filter(|a| !a.is_empty() && a.len() <= 128 * 1024);
                store::set_chat_meta(
                    conn,
                    chat_id,
                    &name,
                    &clamp(&g.description, 500),
                    avatar.as_deref(),
                )?;
                if name != chat.name && !chat.name.is_empty() {
                    let by = self.name_of(conn, id, &sender.user_id);
                    self.system_message(
                        conn,
                        chat_id,
                        (&sender.user_id, &sender.device_id),
                        &format!("{by} renamed the group to “{name}”"),
                        events,
                    )?;
                }
                events.push(CoreEvent::ChatUpdated(chat_id.to_vec()));
                events.push(CoreEvent::ChatsChanged);
            }
            Some(Body::Call(c)) => self.on_call_signal(conn, id, chat, sender, &c, events)?,
            Some(Body::Timer(t)) => {
                store::set_chat_timer(conn, chat_id, t.seconds)?;
                let by = self.name_of(conn, id, &sender.user_id);
                let text = if t.seconds == 0 {
                    format!("{by} turned off disappearing messages")
                } else {
                    format!(
                        "{by} set disappearing messages to {}",
                        super::outbound::describe_timer(t.seconds)
                    )
                };
                self.system_message(
                    conn,
                    chat_id,
                    (&sender.user_id, &sender.device_id),
                    &text,
                    events,
                )?;
                events.push(CoreEvent::ChatUpdated(chat_id.to_vec()));
            }
            Some(Body::ServerMove(s)) => {
                if !mls::admins(group).contains(&sender.user_id) {
                    return Err(CoreError::NotAllowed(
                        "Ignored a server move from someone who isn't an admin".into(),
                    ));
                }
                let by = self.name_of(conn, id, &sender.user_id);
                self.system_message(
                    conn,
                    chat_id,
                    (&sender.user_id, &sender.device_id),
                    &format!(
                        "{by} is moving the group to a new server. Pair again in person to follow."
                    ),
                    events,
                )?;
                events.push(CoreEvent::SecurityWarning {
                    chat_id: Some(chat_id.to_vec()),
                    text: format!(
                        "{by} asked everyone to move to {}",
                        clamp(&s.onion_address, 80)
                    ),
                });
            }
            None => {
                if store::insert_message(
                    conn,
                    &NewMessage {
                        kind: MessageKind::Unsupported,
                        body: "Update Asiri to see this message.",
                        ..base
                    },
                )? {
                    store::touch_chat(conn, chat_id, now, 1)?;
                    events.push(CoreEvent::MessageAdded {
                        chat_id: chat_id.to_vec(),
                        message_id,
                    });
                }
            }
        }
        Ok(())
    }

    fn on_call_signal(
        &self,
        conn: &Connection,
        id: &Identity,
        chat: &Chat,
        sender: &MemberInfo,
        c: &app::CallSignal,
        events: &mut Vec<CoreEvent>,
    ) -> CoreResult<()> {
        let Some(call_id) = id16(&c.call_id) else {
            return Err(CoreError::Crypto("malformed call id".into()));
        };
        let now = now_ms();
        match CallKind::try_from(c.kind).unwrap_or(CallKind::Unspecified) {
            CallKind::Ring => {
                if sender.user_id == id.user_id || !super::calls::valid_alias(&c.room_alias) {
                    return Ok(());
                }
                conn.execute(
                    "INSERT OR IGNORE INTO calls(call_id, chat_id, room_alias, video, outgoing, caller, state, started_ms)
                     VALUES (?1, ?2, ?3, ?4, 0, ?5, 0, ?6)",
                    rusqlite::params![&call_id[..], chat.chat_id, c.room_alias, i64::from(c.video), &sender.user_id[..], now],
                )?;
                events.push(CoreEvent::IncomingCall(CallInvite {
                    call_id,
                    chat_id: chat.chat_id.clone(),
                    caller: sender.user_id,
                    video: c.video,
                }));
            }
            CallKind::Accept | CallKind::Joined => {
                conn.execute(
                    "UPDATE calls SET state = 1, answered_ms = COALESCE(answered_ms, ?2) WHERE call_id = ?1",
                    rusqlite::params![&call_id[..], now],
                )?;
                events.push(CoreEvent::CallUpdated { call_id });
            }
            CallKind::Decline | CallKind::Left | CallKind::End => {
                let row: Option<(i64, Option<i64>, i64, i64)> = conn
                    .query_row(
                        "SELECT state, answered_ms, outgoing, video FROM calls WHERE call_id = ?1",
                        rusqlite::params![&call_id[..]],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .ok();
                let ends = c.kind == CallKind::End as i32
                    || (c.kind == CallKind::Decline as i32 && chat.kind == ChatKind::Direct);
                if ends
                    && let Some((state, answered, outgoing, video)) = row
                    && state != 2
                {
                    conn.execute(
                        "UPDATE calls SET state = 2, ended_ms = ?2 WHERE call_id = ?1",
                        rusqlite::params![&call_id[..], now],
                    )?;
                    let kind = if video == 1 {
                        "video call"
                    } else {
                        "voice call"
                    };
                    let text = match answered {
                        Some(a) => format!(
                            "{} · {}",
                            super::calls::capitalize(kind),
                            super::calls::describe_duration(now - a)
                        ),
                        None if outgoing == 0 => format!("Missed {kind}"),
                        None => format!("{} · not answered", super::calls::capitalize(kind)),
                    };
                    self.call_message(conn, chat, sender, &text, events)?;
                }
                events.push(CoreEvent::CallUpdated { call_id });
            }
            CallKind::Unspecified => {}
        }
        Ok(())
    }

    fn call_message(
        &self,
        conn: &Connection,
        chat: &Chat,
        sender: &MemberInfo,
        text: &str,
        events: &mut Vec<CoreEvent>,
    ) -> CoreResult<()> {
        let now = now_ms();
        let message_id = crate::ids::random_id();
        store::insert_message(
            conn,
            &NewMessage {
                message_id,
                chat_id: &chat.chat_id,
                sender_user: sender.user_id,
                sender_device: sender.device_id,
                outgoing: false,
                kind: MessageKind::Call,
                body: text,
                reply_to: None,
                sent_at_ms: now,
                received_at_ms: now,
                status: MessageStatus::Received,
                expires_at_ms: None,
            },
        )?;
        store::touch_chat(conn, &chat.chat_id, now, 0)?;
        events.push(CoreEvent::MessageAdded {
            chat_id: chat.chat_id.clone(),
            message_id,
        });
        Ok(())
    }
}
