//! Images, files and voice notes (architecture §7.7, §8.5, §11.5).
//!
//! The app strips metadata and prepares the thumbnail and BlurHash (ImageIO); the core encrypts
//! with a fresh key, pads, uploads in resumable 1 MiB chunks over its own stream, and only then
//! sends the MLS message carrying the blob ID, key and hash. Both sides keep the ciphertext as
//! their local copy; plaintext exists only in memory while viewed, or where the user exports it.

use std::path::{Path, PathBuf};

use asiri_proto::Message as _;
use asiri_proto::app::v1::{self as app, app_message::Body};
use asiri_proto::relay::v1 as wire;

use super::outbound::SendOpts;
use super::{Core, CoreEvent, lock_err};
use crate::crypto::attachment;
use crate::error::{CoreError, CoreResult};
use crate::ids::{Id16, now_ms, random_id};
use crate::net::http;
use crate::store::{self, AttachmentInfo, AttachmentState, MessageKind, MessageStatus, NewMessage};

/// Largest file Asiri sends (§9.8).
pub const MAX_ATTACHMENT: u64 = 200 * 1024 * 1024;
const UPLOAD_CHUNK: usize = 1024 * 1024;

/// A file the app prepared for sending.
#[derive(Clone, Debug, Default)]
pub struct AttachmentInput {
    /// The (metadata-stripped) bytes.
    pub data: Vec<u8>,
    /// MIME type.
    pub mime: String,
    /// File name shown to the recipient.
    pub file_name: String,
    /// Image/video width.
    pub width: u32,
    /// Image/video height.
    pub height: u32,
    /// Audio/video duration.
    pub duration_ms: u32,
    /// A tiny JPEG (a few KB) shown immediately.
    pub thumbnail_jpeg: Option<Vec<u8>>,
    /// BlurHash placeholder.
    pub blurhash: String,
    /// Voice note.
    pub voice_note: bool,
    /// Voice-note waveform, one byte per bucket.
    pub waveform: Option<Vec<u8>>,
}

/// Encodes a BlurHash from RGBA pixels (4×3 components).
pub fn blurhash_encode(rgba: &[u8], width: u32, height: u32) -> CoreResult<String> {
    blurhash::encode(4, 3, width, height, rgba)
        .map_err(|e| CoreError::Invalid(format!("blurhash: {e:?}")))
}

/// Decodes a BlurHash into RGBA pixels.
pub fn blurhash_decode(hash: &str, width: u32, height: u32) -> CoreResult<Vec<u8>> {
    if width == 0 || height == 0 || width > 256 || height > 256 {
        return Err(CoreError::Invalid("blurhash size".into()));
    }
    blurhash::decode(hash, width, height, 1.0)
        .map_err(|e| CoreError::Invalid(format!("blurhash: {e:?}")))
}

impl Core {
    pub(crate) fn attachment_path(&self, message_id: &Id16) -> PathBuf {
        self.data_dir()
            .join("attachments")
            .join(format!("{}.blob", hex::encode(message_id)))
    }

    /// Encrypts and sends a file. The bubble appears at once; upload progress arrives as
    /// `MessageUpdated` events, and the message is sent when the upload completes.
    pub async fn send_attachment(
        &self,
        chat_id: &[u8],
        input: AttachmentInput,
        caption: &str,
        reply_to: Option<Id16>,
    ) -> CoreResult<Id16> {
        if input.data.is_empty() {
            return Err(CoreError::Invalid("The file is empty".into()));
        }
        if input.data.len() as u64 > MAX_ATTACHMENT {
            return Err(CoreError::Invalid("Files can be up to 200 MB".into()));
        }
        if input
            .thumbnail_jpeg
            .as_ref()
            .is_some_and(|t| t.len() > 64 * 1024)
        {
            return Err(CoreError::Invalid("Thumbnail is too large".into()));
        }
        let data = input.data.clone();
        let encrypted = tokio::task::spawn_blocking(move || attachment::encrypt(&data))
            .await
            .map_err(CoreError::internal)??;
        let message_id = random_id();
        let blob_id = random_id();
        tokio::fs::write(self.attachment_path(&message_id), &encrypted.blob).await?;
        let info = AttachmentInfo {
            blob_id,
            key: encrypted.key.to_vec(),
            sha256: encrypted.sha256.to_vec(),
            size: encrypted.plaintext_len,
            blob_size: encrypted.blob.len() as u64,
            mime: input.mime.chars().take(128).collect(),
            file_name: input.file_name.chars().take(255).collect(),
            width: input.width,
            height: input.height,
            duration_ms: input.duration_ms,
            thumbnail: input.thumbnail_jpeg,
            blurhash: input.blurhash.chars().take(128).collect(),
            voice_note: input.voice_note,
            waveform: input.waveform.filter(|w| w.len() <= 512),
            state: Some(AttachmentState::Uploading),
            progress: 0.0,
        };
        {
            let id = self.inner.identity.read().map_err(lock_err)?;
            let reg = id.registered()?;
            self.inner.store.tx(|c| {
                let chat = store::chat(c, chat_id)?.ok_or(CoreError::NotFound)?;
                if chat.left {
                    return Err(CoreError::NotAllowed(
                        "You're no longer in this chat".into(),
                    ));
                }
                let now = now_ms();
                store::insert_message(
                    c,
                    &NewMessage {
                        message_id,
                        chat_id,
                        sender_user: id.user_id,
                        sender_device: reg.device_id,
                        outgoing: true,
                        kind: MessageKind::Attachment,
                        body: caption.trim(),
                        reply_to,
                        sent_at_ms: now,
                        received_at_ms: now,
                        status: MessageStatus::Pending,
                        expires_at_ms: (chat.timer_s > 0)
                            .then(|| now + i64::from(chat.timer_s) * 1000),
                    },
                )?;
                store::insert_attachment(c, &message_id, &info)?;
                store::touch_chat(c, chat_id, now, 0)
            })?;
        }
        self.emit(CoreEvent::MessageAdded {
            chat_id: chat_id.to_vec(),
            message_id,
        });
        self.emit(CoreEvent::ChatsChanged);
        let core = self.clone();
        self.inner.rt.spawn(async move {
            if let Err(e) = core.upload_and_send(&message_id).await {
                tracing::debug!("attachment upload failed: {e}");
            }
        });
        Ok(message_id)
    }

    pub(crate) async fn restart_upload(&self, message_id: &Id16) -> CoreResult<()> {
        let chat_id = self.message(message_id)?.chat_id;
        self.inner.store.tx(|c| {
            store::set_status(c, message_id, MessageStatus::Pending)?;
            store::set_attachment_state(c, message_id, AttachmentState::Uploading, 0.0)
        })?;
        self.emit(CoreEvent::MessageUpdated {
            chat_id,
            message_id: *message_id,
        });
        let core = self.clone();
        let id = *message_id;
        self.inner.rt.spawn(async move {
            if let Err(e) = core.upload_and_send(&id).await {
                tracing::debug!("attachment upload failed: {e}");
            }
        });
        Ok(())
    }

    async fn upload_and_send(&self, message_id: &Id16) -> CoreResult<()> {
        let m = self.message(message_id)?;
        let att = m.attachment.clone().ok_or(CoreError::NotFound)?;
        let blob = tokio::fs::read(self.attachment_path(message_id)).await?;
        let result = self
            .upload_blob(&att.blob_id, &blob, &m.chat_id, message_id)
            .await;
        let mut events = vec![CoreEvent::MessageUpdated {
            chat_id: m.chat_id.clone(),
            message_id: *message_id,
        }];
        match result {
            Ok(()) => {
                let id = self.inner.identity.read().map_err(lock_err)?;
                self.inner.store.tx(|c| {
                    let chat = store::chat(c, &m.chat_id)?.ok_or(CoreError::NotFound)?;
                    store::set_attachment_state(c, message_id, AttachmentState::Local, 1.0)?;
                    let body = Body::Attachment(app::Attachment {
                        blob_id: att.blob_id.to_vec(),
                        key: att.key.clone(),
                        ciphertext_sha256: att.sha256.clone(),
                        size: att.size,
                        mime_type: att.mime.clone(),
                        file_name: att.file_name.clone(),
                        width: att.width,
                        height: att.height,
                        duration_ms: att.duration_ms,
                        thumbnail_jpeg: att.thumbnail.clone().unwrap_or_default(),
                        blurhash: att.blurhash.clone(),
                        caption: m.body.clone(),
                        voice_note: att.voice_note,
                        waveform: att.waveform.clone().unwrap_or_default(),
                        blob_size: att.blob_size,
                    });
                    let (_, delivered) = self.queue_app_message(
                        c,
                        &id,
                        &chat,
                        body,
                        &SendOpts {
                            message_id: Some(*message_id),
                            track: true,
                            reply_to: m.reply_to,
                            expires_in_s: chat.timer_s,
                            ..SendOpts::default()
                        },
                    )?;
                    if !delivered {
                        store::set_status(c, message_id, MessageStatus::Sent)?;
                    }
                    Ok(())
                })?;
                drop(id);
                self.emit_all(events);
                self.wake_outbox();
                Ok(())
            }
            Err(e) => {
                self.inner.store.tx(|c| {
                    store::set_attachment_state(c, message_id, AttachmentState::Failed, 0.0)?;
                    store::set_status(c, message_id, MessageStatus::Failed).map(|_| ())
                })?;
                events.push(CoreEvent::ChatsChanged);
                self.emit_all(events);
                Err(e)
            }
        }
    }

    async fn upload_blob(
        &self,
        blob_id: &Id16,
        blob: &[u8],
        chat_id: &[u8],
        message_id: &Id16,
    ) -> CoreResult<()> {
        let dialer = self.ensure_dialer().await?;
        let token = self.client()?.session_token.clone();
        let total = blob.len() as u64;
        let hex_id = hex::encode(blob_id);
        // Resume an interrupted upload where it stopped.
        let mut offset = match http::request(
            &dialer,
            "GET",
            &format!("/v1/blobs/{hex_id}/status"),
            Some(&token),
            &[],
            vec![],
        )
        .await
        {
            Ok(r) if r.status == 200 => {
                let s = wire::BlobStatus::decode(r.body.as_slice())?;
                if s.complete {
                    return Ok(());
                }
                s.received
            }
            _ => 0,
        };
        while offset < total {
            let start = usize::try_from(offset).map_err(CoreError::internal)?;
            let end = (start + UPLOAD_CHUNK).min(blob.len());
            let resp = http::request(
                &dialer,
                "PUT",
                &format!("/v1/blobs/{hex_id}?offset={offset}&total={total}"),
                Some(&token),
                &[],
                blob[start..end].to_vec(),
            )
            .await?;
            match resp.status {
                200 | 409 => {
                    let s = wire::BlobStatus::decode(resp.body.as_slice())?;
                    if s.received < offset && resp.status == 200 {
                        return Err(CoreError::Network("upload went backwards".into()));
                    }
                    offset = s.received;
                    if s.complete {
                        offset = total;
                    }
                }
                _ => return Err(http::error_from(&resp)),
            }
            #[allow(clippy::cast_precision_loss)]
            let progress = offset as f64 / total as f64;
            self.inner.store.tx(|c| {
                store::set_attachment_state(c, message_id, AttachmentState::Uploading, progress)
            })?;
            self.emit(CoreEvent::MessageUpdated {
                chat_id: chat_id.to_vec(),
                message_id: *message_id,
            });
        }
        Ok(())
    }

    /// Downloads an attachment's ciphertext, verifies its hash, and stores it locally.
    pub async fn download_attachment(&self, message_id: &Id16) -> CoreResult<()> {
        let m = self.message(message_id)?;
        let att = m.attachment.clone().ok_or(CoreError::NotFound)?;
        let path = self.attachment_path(message_id);
        if att.state == Some(AttachmentState::Local) && path.exists() {
            return Ok(());
        }
        self.inner.store.tx(|c| {
            store::set_attachment_state(c, message_id, AttachmentState::Downloading, 0.0)
        })?;
        self.emit(CoreEvent::MessageUpdated {
            chat_id: m.chat_id.clone(),
            message_id: *message_id,
        });
        let result = async {
            let dialer = self.ensure_dialer().await?;
            let token = self.client()?.session_token.clone();
            let part = path.with_extension("part");
            let mut have = tokio::fs::read(&part).await.unwrap_or_default();
            let headers = if have.is_empty() {
                vec![]
            } else {
                vec![("range", format!("bytes={}-", have.len()))]
            };
            let resp = http::request(
                &dialer,
                "GET",
                &format!("/v1/blobs/{}", hex::encode(att.blob_id)),
                Some(&token),
                &headers,
                vec![],
            )
            .await?;
            match resp.status {
                200 => have = resp.body,
                206 => have.extend_from_slice(&resp.body),
                404 => {
                    return Err(CoreError::Network(
                        "This file is no longer on the server (files expire after 30 days)".into(),
                    ));
                }
                _ => return Err(http::error_from(&resp)),
            }
            use sha2::Digest as _;
            let digest: [u8; 32] = sha2::Sha256::digest(&have).into();
            if digest[..] != att.sha256[..] {
                let _ = tokio::fs::remove_file(&part).await;
                return Err(CoreError::Crypto("The downloaded file was modified".into()));
            }
            tokio::fs::write(&part, &have).await?;
            tokio::fs::rename(&part, &path).await?;
            Ok(())
        }
        .await;
        let state = if result.is_ok() {
            AttachmentState::Local
        } else {
            AttachmentState::Failed
        };
        self.inner.store.tx(|c| {
            store::set_attachment_state(
                c,
                message_id,
                state,
                if result.is_ok() { 1.0 } else { 0.0 },
            )
        })?;
        self.emit(CoreEvent::MessageUpdated {
            chat_id: m.chat_id,
            message_id: *message_id,
        });
        result
    }

    /// Decrypts a downloaded attachment into memory (for the in-app viewer and players).
    pub fn attachment_data(&self, message_id: &Id16) -> CoreResult<Vec<u8>> {
        let m = self.message(message_id)?;
        let att = m.attachment.ok_or(CoreError::NotFound)?;
        let blob =
            std::fs::read(self.attachment_path(message_id)).map_err(|_| CoreError::NotFound)?;
        let plain = attachment::decrypt(&att.key, &blob, &att.sha256, att.size)?;
        Ok(plain.to_vec())
    }

    /// Writes a plaintext copy where the user chose (§13.3 "Save to…"). The copy is no longer
    /// protected by Asiri.
    pub fn export_attachment(&self, message_id: &Id16, destination: &Path) -> CoreResult<()> {
        let data = zeroize::Zeroizing::new(self.attachment_data(message_id)?);
        std::fs::write(destination, data.as_slice())?;
        Ok(())
    }
}
