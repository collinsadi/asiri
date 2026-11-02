//! Creating chats and changing membership (architecture §7.4, §7.5, §11.4, §11.6).
//!
//! Group changes are commits. A commit is sent with the group's route and the epoch it was made
//! from; the relay accepts exactly one per epoch. If ours wins we merge it and queue the Welcome
//! for new members; if another commit won, we drop ours, wait until the winner has been applied
//! from our inbox, and try again.

use std::time::Duration;

use asiri_proto::relay::v1::{
    self as wire, PayloadKind, client_frame::Body as C, send_result::Status,
    server_frame::Body as S,
};
use openmls::prelude::KeyPackage;
use rusqlite::Connection;

use super::{Core, CoreEvent, lock_err};
use crate::crypto::padding;
use crate::error::{CoreError, CoreResult};
use crate::identity::Identity;
use crate::ids::{Id16, now_ms, random_bytes, random_id};
use crate::mls::{self, CommitOut, GroupMeta, MlsGroup};
use crate::store::{self, Chat, ChatKind};

const MAX_GROUP_MEMBERS: usize = 50;
const COMMIT_ATTEMPTS: usize = 6;

impl Core {
    /// Waits until the chat's epoch is beyond `epoch` (the winning commit has been applied).
    async fn wait_for_epoch(
        &self,
        chat_id: &[u8],
        epoch: u64,
        timeout: Duration,
    ) -> CoreResult<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let current = {
                let conn = self.inner.store.lock()?;
                mls::epoch(&self.inner.mls.load(&conn, chat_id)?)
            };
            if current > epoch {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(CoreError::Network(
                    "The group is busy; try again in a moment".into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Creates a commit with `build`, sends it for ordering, and merges it once accepted.
    /// `build` returns the commit and the inboxes that should receive its Welcome.
    pub(crate) async fn commit_op<F>(&self, chat_id: &[u8], build: F) -> CoreResult<()>
    where
        F: Fn(&Connection, &Identity, &mut MlsGroup) -> CoreResult<Option<(CommitOut, Vec<Id16>)>>,
    {
        for _ in 0..COMMIT_ATTEMPTS {
            let client = self.client()?;
            let prepared = {
                let id = self.inner.identity.read().map_err(lock_err)?;
                let device = id.registered()?.device_id;
                self.inner.store.tx(|c| {
                    let chat = store::chat(c, chat_id)?.ok_or(CoreError::NotFound)?;
                    let mut group = self.inner.mls.load(c, chat_id)?;
                    // Existing members (other than us) receive the commit itself.
                    let recipients: Vec<Id16> = mls::members(&group)
                        .into_iter()
                        .filter(|m| m.device_id != device)
                        .map(|m| m.inbox_id)
                        .collect();
                    Ok(build(c, &id, &mut group)?
                        .map(|(out, welcome_to)| (chat, out, recipients, welcome_to)))
                })?
            };
            let Some((chat, out, recipients, welcome_to)) = prepared else {
                return Ok(()); // nothing to commit
            };
            let reply = client
                .request(C::Send(wire::Send {
                    envelope_id: random_id().to_vec(),
                    kind: PayloadKind::Commit as i32,
                    group_route: chat.group_route.to_vec(),
                    epoch: out.from_epoch,
                    to_inboxes: recipients.iter().map(|r| r.to_vec()).collect(),
                    payload: padding::pad_envelope(&out.commit),
                    ttl_seconds: 0,
                }))
                .await;
            let status = match reply {
                Ok(S::SendResult(r)) => Status::try_from(r.status).unwrap_or(Status::Unspecified),
                Ok(_) => Status::Unspecified,
                Err(e) => {
                    self.discard_pending(chat_id)?;
                    return Err(e);
                }
            };
            match status {
                Status::Accepted => {
                    self.inner.store.tx(|c| {
                        let mut group = self.inner.mls.load(c, chat_id)?;
                        self.inner.mls.merge_pending(c, &mut group)?;
                        if let Some(welcome) = &out.welcome
                            && !welcome_to.is_empty()
                        {
                            store::enqueue(
                                c,
                                &random_id(),
                                Some(chat_id),
                                None,
                                PayloadKind::Welcome as i32,
                                &welcome_to,
                                &padding::pad_envelope(welcome),
                                0,
                            )?;
                        }
                        Ok(())
                    })?;
                    self.wake_outbox();
                    self.rotate_call_keys(chat_id);
                    return Ok(());
                }
                Status::StaleEpoch => {
                    self.discard_pending(chat_id)?;
                    self.wait_for_epoch(chat_id, out.from_epoch, Duration::from_secs(10))
                        .await?;
                }
                Status::RateLimited | Status::InboxFull => {
                    self.discard_pending(chat_id)?;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                _ => {
                    self.discard_pending(chat_id)?;
                    return Err(CoreError::Relay(
                        "The server refused the group change".into(),
                    ));
                }
            }
        }
        Err(CoreError::Network(
            "Couldn't update the group; try again".into(),
        ))
    }

    fn discard_pending(&self, chat_id: &[u8]) -> CoreResult<()> {
        self.inner.store.tx(|c| {
            let mut group = self.inner.mls.load(c, chat_id)?;
            if group.pending_commit().is_some() {
                self.inner.mls.discard_pending(c, &mut group)?;
            }
            Ok(())
        })
    }

    /// Fetches one key package per device of each user and checks every package against the
    /// signature key we verified (a malicious relay cannot substitute keys, §7.6).
    async fn fetch_key_packages(&self, users: &[Id16]) -> CoreResult<Vec<(KeyPackage, Id16)>> {
        let client = self.client()?;
        let devices = {
            let conn = self.inner.store.lock()?;
            let mut all = Vec::new();
            for u in users {
                let ds = store::devices_of(&conn, u)?;
                if ds.is_empty() {
                    return Err(CoreError::Invalid(
                        "Pair with this person in person first".into(),
                    ));
                }
                all.extend(ds);
            }
            all
        };
        let mut out = Vec::new();
        for d in devices {
            let reply = client
                .request(C::FetchKeyPackage(wire::FetchKeyPackage {
                    device_id: d.device_id.to_vec(),
                }))
                .await;
            let kp = match reply {
                Ok(S::KeyPackage(kp)) => kp.key_package,
                Ok(_) => return Err(CoreError::Network("unexpected reply".into())),
                Err(CoreError::Relay(_)) => {
                    return Err(CoreError::Network(
                        "A friend's device hasn't come online yet. Try again once they've opened Asiri.".into(),
                    ));
                }
                Err(e) => return Err(e),
            };
            let (package, info) = self.inner.mls.validate_key_package(&kp)?;
            if info.signature_key != d.signature_key
                || info.device_id != d.device_id
                || info.user_id != d.user_id
            {
                return Err(CoreError::Crypto(
                    "The server offered a key that doesn't match the one you verified. The chat was not created.".into(),
                ));
            }
            out.push((package, info.inbox_id));
        }
        Ok(out)
    }

    /// Creates a DM or a group with the given contacts (§11.4). For DMs, returns the existing
    /// chat if there is one.
    pub async fn create_chat(
        &self,
        kind: ChatKind,
        name: &str,
        members: &[Id16],
    ) -> CoreResult<Vec<u8>> {
        let me = self.user_id()?;
        let mut users: Vec<Id16> = members.iter().copied().filter(|u| *u != me).collect();
        users.sort_unstable();
        users.dedup();
        match kind {
            ChatKind::Direct if users.len() != 1 => {
                return Err(CoreError::Invalid(
                    "A direct chat has exactly one other person".into(),
                ));
            }
            ChatKind::Group if users.len() + 1 > MAX_GROUP_MEMBERS => {
                return Err(CoreError::Invalid(format!(
                    "Groups can have up to {MAX_GROUP_MEMBERS} members"
                )));
            }
            ChatKind::Group if name.trim().is_empty() => {
                return Err(CoreError::Invalid("Give the group a name".into()));
            }
            _ => {}
        }
        if kind == ChatKind::Direct {
            let conn = self.inner.store.lock()?;
            if let Some(existing) = store::dm_with(&conn, &users[0])? {
                return Ok(existing.chat_id);
            }
        }
        let packages = self.fetch_key_packages(&users).await?;
        let chat_id = random_bytes::<32>().to_vec();
        let route: Id16 = random_id();
        let timer = self
            .inner
            .settings
            .read()
            .map_err(lock_err)?
            .default_timer_s;
        {
            let id = self.inner.identity.read().map_err(lock_err)?;
            let admins: Vec<Id16> = match kind {
                ChatKind::Direct => vec![id.user_id, users[0]],
                ChatKind::Group => vec![id.user_id],
            };
            let credential = id.credential()?;
            self.inner.store.tx(|c| {
                self.inner.mls.create_group(
                    c,
                    &id.signer,
                    credential,
                    &chat_id,
                    GroupMeta { route, kind },
                    &admins,
                )?;
                let now = now_ms();
                store::insert_chat(
                    c,
                    &Chat {
                        chat_id: chat_id.clone(),
                        kind,
                        group_route: route,
                        name: if kind == ChatKind::Group {
                            name.trim().to_owned()
                        } else {
                            String::new()
                        },
                        description: String::new(),
                        avatar: None,
                        dm_user: (kind == ChatKind::Direct).then(|| users[0]),
                        suite: mls::suite_code(),
                        created_ms: now,
                        last_activity_ms: now,
                        unread: 0,
                        muted: false,
                        pinned: false,
                        archived: false,
                        timer_s: timer,
                        left: false,
                    },
                )?;
                Ok(())
            })?;
        }
        let kps: Vec<KeyPackage> = packages.iter().map(|(kp, _)| kp.clone()).collect();
        let welcome_to: Vec<Id16> = packages.iter().map(|(_, inbox)| *inbox).collect();
        let result = self
            .commit_op(&chat_id, |c, id, group| {
                Ok(Some((
                    self.inner.mls.add(c, group, &id.signer, &kps)?,
                    welcome_to.clone(),
                )))
            })
            .await;
        if let Err(e) = result {
            let _ = self.inner.store.tx(|c| store::delete_chat(c, &chat_id));
            return Err(e);
        }
        // Now that everyone is in, share names (inside MLS, never as server metadata).
        let mut events = Vec::new();
        {
            let id = self.inner.identity.read().map_err(lock_err)?;
            let device = id.registered()?.device_id;
            self.inner.store.tx(|c| {
                let text = match kind {
                    ChatKind::Direct => "Messages in this chat are end-to-end encrypted. Only the two of you can read them.".to_owned(),
                    ChatKind::Group => format!("You created “{}”", name.trim()),
                };
                self.system_message(c, &chat_id, (&id.user_id, &device), &text, &mut events)?;
                if kind == ChatKind::Group {
                    let chat = store::chat(c, &chat_id)?.ok_or(CoreError::NotFound)?;
                    self.queue_app_message(
                        c,
                        &id,
                        &chat,
                        asiri_proto::app::v1::app_message::Body::Group(asiri_proto::app::v1::GroupUpdate {
                            name: chat.name.clone(),
                            avatar: None,
                            description: String::new(),
                        }),
                        &super::outbound::SendOpts::default(),
                    )?;
                }
                if timer > 0 {
                    let chat = store::chat(c, &chat_id)?.ok_or(CoreError::NotFound)?;
                    self.queue_app_message(
                        c,
                        &id,
                        &chat,
                        asiri_proto::app::v1::app_message::Body::Timer(asiri_proto::app::v1::TimerChange { seconds: timer }),
                        &super::outbound::SendOpts::default(),
                    )?;
                }
                Ok(())
            })?;
        }
        self.send_profile_to(&chat_id)?;
        events.push(CoreEvent::ChatsChanged);
        self.emit_all(events);
        self.wake_outbox();
        Ok(chat_id)
    }

    fn require_admin(&self, chat_id: &[u8]) -> CoreResult<MlsGroup> {
        let me = self.user_id()?;
        let conn = self.inner.store.lock()?;
        let group = self.inner.mls.load(&conn, chat_id)?;
        if !mls::admins(&group).contains(&me) {
            return Err(CoreError::NotAllowed(
                "Only admins can change who's in the group".into(),
            ));
        }
        Ok(group)
    }

    /// Adds people to a group (admins only, §11.4 "Adding someone later").
    pub async fn add_members(&self, chat_id: &[u8], users: &[Id16]) -> CoreResult<()> {
        let group = self.require_admin(chat_id)?;
        let present: Vec<Id16> = mls::members(&group).iter().map(|m| m.user_id).collect();
        let new: Vec<Id16> = users
            .iter()
            .copied()
            .filter(|u| !present.contains(u))
            .collect();
        if new.is_empty() {
            return Ok(());
        }
        let distinct = present
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        if distinct + new.len() > MAX_GROUP_MEMBERS {
            return Err(CoreError::Invalid(format!(
                "Groups can have up to {MAX_GROUP_MEMBERS} members"
            )));
        }
        let packages = self.fetch_key_packages(&new).await?;
        let kps: Vec<KeyPackage> = packages.iter().map(|(kp, _)| kp.clone()).collect();
        let welcome_to: Vec<Id16> = packages.iter().map(|(_, i)| *i).collect();
        self.commit_op(chat_id, |c, id, group| {
            Ok(Some((
                self.inner.mls.add(c, group, &id.signer, &kps)?,
                welcome_to.clone(),
            )))
        })
        .await?;
        let mut events = Vec::new();
        {
            let id = self.inner.identity.read().map_err(lock_err)?;
            let device = id.registered()?.device_id;
            self.inner.store.tx(|c| {
                let names: Vec<String> = new
                    .iter()
                    .map(|u| {
                        store::contact(c, u)
                            .ok()
                            .flatten()
                            .map(|ct| ct.display_name)
                            .filter(|n| !n.is_empty())
                            .unwrap_or_else(|| "Someone".into())
                    })
                    .collect();
                self.system_message(
                    c,
                    chat_id,
                    (&id.user_id, &device),
                    &format!("You added {}", names.join(", ")),
                    &mut events,
                )?;
                // Bring the newcomers up to date on the group's name.
                let chat = store::chat(c, chat_id)?.ok_or(CoreError::NotFound)?;
                if chat.kind == ChatKind::Group {
                    self.queue_app_message(
                        c,
                        &id,
                        &chat,
                        asiri_proto::app::v1::app_message::Body::Group(
                            asiri_proto::app::v1::GroupUpdate {
                                name: chat.name.clone(),
                                avatar: chat.avatar.clone().map(|jpeg| {
                                    asiri_proto::app::v1::Attachment {
                                        mime_type: "image/jpeg".into(),
                                        thumbnail_jpeg: jpeg,
                                        ..Default::default()
                                    }
                                }),
                                description: chat.description.clone(),
                            },
                        ),
                        &super::outbound::SendOpts::default(),
                    )?;
                }
                Ok(())
            })?;
        }
        self.send_profile_to(chat_id)?;
        events.push(CoreEvent::ChatUpdated(chat_id.to_vec()));
        self.emit_all(events);
        Ok(())
    }

    /// Removes a person (every device of theirs) from a group (admins only, §11.6).
    pub async fn remove_member(&self, chat_id: &[u8], user: &Id16) -> CoreResult<()> {
        self.require_admin(chat_id)?;
        let user = *user;
        self.commit_op(chat_id, |c, id, group| {
            let leaves: Vec<u32> = mls::members(group)
                .iter()
                .filter(|m| m.user_id == user)
                .map(|m| m.leaf)
                .collect();
            if leaves.is_empty() {
                return Ok(None);
            }
            Ok(Some((
                self.inner.mls.remove(c, group, &id.signer, &leaves)?,
                vec![],
            )))
        })
        .await?;
        let mut events = Vec::new();
        {
            let id = self.inner.identity.read().map_err(lock_err)?;
            let device = id.registered()?.device_id;
            self.inner.store.tx(|c| {
                let name = store::contact(c, &user)?
                    .map(|ct| ct.display_name)
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "Someone".into());
                self.system_message(
                    c,
                    chat_id,
                    (&id.user_id, &device),
                    &format!("You removed {name}"),
                    &mut events,
                )
            })?;
        }
        events.push(CoreEvent::ChatUpdated(chat_id.to_vec()));
        self.emit_all(events);
        Ok(())
    }

    /// Promotes or demotes an admin (admins only).
    pub async fn set_admin(&self, chat_id: &[u8], user: &Id16, admin: bool) -> CoreResult<()> {
        self.require_admin(chat_id)?;
        let user = *user;
        self.commit_op(chat_id, |c, id, group| {
            let mut admins = mls::admins(group);
            if admin && !admins.contains(&user) {
                admins.push(user);
            } else if !admin {
                admins.retain(|a| *a != user);
            } else {
                return Ok(None);
            }
            if admins.is_empty() {
                return Err(CoreError::Invalid(
                    "A group needs at least one admin".into(),
                ));
            }
            Ok(Some((
                self.inner.mls.set_admins(c, group, &id.signer, &admins)?,
                vec![],
            )))
        })
        .await?;
        self.emit(CoreEvent::ChatUpdated(chat_id.to_vec()));
        Ok(())
    }

    /// Leaves a chat: hands admin to someone else if we're the last admin, sends a
    /// self-remove proposal the next admin commits, and stops showing the chat's messages
    /// (§11.6 "Leaving").
    pub async fn leave_chat(&self, chat_id: &[u8]) -> CoreResult<()> {
        let me = self.user_id()?;
        let (others, admins) = {
            let conn = self.inner.store.lock()?;
            let group = self.inner.mls.load(&conn, chat_id)?;
            let others: Vec<Id16> = mls::members(&group)
                .iter()
                .map(|m| m.user_id)
                .filter(|u| *u != me)
                .collect();
            (others, mls::admins(&group))
        };
        let admins_left = admins
            .iter()
            .filter(|a| **a != me && others.contains(a))
            .count();
        if admins.contains(&me)
            && admins_left == 0
            && let Some(successor) = others.first().copied()
        {
            self.set_admin(chat_id, &successor, true).await?;
        }
        let client = self.client()?;
        let (proposal, recipients) = {
            let id = self.inner.identity.read().map_err(lock_err)?;
            let device = id.registered()?.device_id;
            self.inner.store.tx(|c| {
                let mut group = self.inner.mls.load(c, chat_id)?;
                let recipients: Vec<Id16> = mls::members(&group)
                    .into_iter()
                    .filter(|m| m.device_id != device)
                    .map(|m| m.inbox_id)
                    .collect();
                Ok((
                    self.inner.mls.leave_proposal(c, &mut group, &id.signer)?,
                    recipients,
                ))
            })?
        };
        if !recipients.is_empty() {
            client
                .request(C::Send(wire::Send {
                    envelope_id: random_id().to_vec(),
                    kind: PayloadKind::Proposal as i32,
                    group_route: vec![],
                    epoch: 0,
                    to_inboxes: recipients.iter().map(|r| r.to_vec()).collect(),
                    payload: padding::pad_envelope(&proposal),
                    ttl_seconds: 0,
                }))
                .await?;
        }
        let mut events = Vec::new();
        {
            let id = self.inner.identity.read().map_err(lock_err)?;
            let device = id.registered()?.device_id;
            self.inner.store.tx(|c| {
                store::set_chat_flag(c, chat_id, "left_chat", true)?;
                self.system_message(c, chat_id, (&id.user_id, &device), "You left", &mut events)
            })?;
        }
        events.push(CoreEvent::ChatsChanged);
        self.emit_all(events);
        Ok(())
    }

    /// Commits members' pending leave proposals (we're an admin and online).
    pub(crate) async fn commit_pending_proposals(&self, chat_id: &[u8]) -> CoreResult<()> {
        self.commit_op(chat_id, |c, id, group| {
            Ok(self
                .inner
                .mls
                .commit_pending_proposals(c, group, &id.signer)?
                .map(|out| (out, vec![])))
        })
        .await?;
        self.emit(CoreEvent::ChatUpdated(chat_id.to_vec()));
        Ok(())
    }

    /// Rotates our key material in a chat (post-compromise security, §7.3 "Self-update").
    pub async fn self_update(&self, chat_id: &[u8]) -> CoreResult<()> {
        self.commit_op(chat_id, |c, id, group| {
            if !mls::is_active(group) {
                return Ok(None);
            }
            Ok(Some((
                self.inner.mls.self_update(c, group, &id.signer)?,
                vec![],
            )))
        })
        .await?;
        self.inner
            .store
            .tx(|c| store::set_last_self_update(c, chat_id, now_ms()))?;
        Ok(())
    }
}
