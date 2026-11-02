//! Attachments and calls end to end: encrypted upload and download, automatic downloads,
//! the relay storing only ciphertext, call tokens, shared call keys and key rotation.

use std::time::Duration;

use asiri_core::engine::AttachmentInput;
use asiri_core::store::{AttachmentState, ChatKind};
use asiri_core::{CallState, CoreEvent};
use asiri_sim::{Harness, PATIENCE, eventually};

fn random_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    getrandom::fill(&mut v).expect("randomness");
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn files_travel_encrypted_and_arrive_intact() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");

    // 3.5 MB spans several upload chunks and STREAM segments.
    let mut data = b"ASIRI-PLAINTEXT-MARKER ".repeat(4);
    data.extend(random_bytes(3_500_000));
    let id = ada
        .core
        .send_attachment(
            &dm,
            AttachmentInput {
                data: data.clone(),
                mime: "application/pdf".into(),
                file_name: "minutes.pdf".into(),
                ..AttachmentInput::default()
            },
            "Minutes from today",
            None,
        )
        .await?;
    ben.wait_for_text(&dm, "Minutes from today").await?;
    let m = ben.core.message(&id)?;
    let info = m.attachment.expect("attachment metadata");
    assert_eq!(info.file_name, "minutes.pdf");
    assert_eq!(info.size, data.len() as u64);
    // Documents aren't fetched until asked for.
    assert_eq!(info.state, Some(AttachmentState::Remote));

    // The relay only ever held ciphertext.
    let blobs = h.relay.config.blob_dir();
    let mut stored = Vec::new();
    for entry in std::fs::read_dir(&blobs)? {
        stored.extend(std::fs::read(entry?.path())?);
    }
    assert!(stored.len() >= data.len(), "the blob is on the relay");
    assert!(
        !stored.windows(23).any(|w| w == b"ASIRI-PLAINTEXT-MARKER "),
        "the relay saw plaintext"
    );

    ben.core.download_attachment(&id).await?;
    eventually("the download to finish", PATIENCE, || {
        ben.core.message(&id).is_ok_and(|m| {
            m.attachment
                .is_some_and(|a| a.state == Some(AttachmentState::Local))
        })
    })
    .await?;
    assert_eq!(ben.core.attachment_data(&id)?, data);
    let out = tempfile::tempdir()?;
    let dest = out.path().join("minutes.pdf");
    ben.core.export_attachment(&id, &dest)?;
    assert_eq!(std::fs::read(&dest)?, data);
    // The sender keeps its own copy.
    assert_eq!(ada.core.attachment_data(&id)?, data);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn photos_and_voice_notes_download_automatically() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");

    let photo = random_bytes(200_000);
    let photo_id = ada
        .core
        .send_attachment(
            &dm,
            AttachmentInput {
                data: photo.clone(),
                mime: "image/jpeg".into(),
                file_name: "IMG_0001.jpg".into(),
                width: 1200,
                height: 800,
                thumbnail_jpeg: Some(random_bytes(2_000)),
                blurhash: "LEHV6nWB2yk8pyo0adR*.7kCMdnj".into(),
                ..AttachmentInput::default()
            },
            "",
            None,
        )
        .await?;
    let voice = random_bytes(40_000);
    let voice_id = ada
        .core
        .send_attachment(
            &dm,
            AttachmentInput {
                data: voice.clone(),
                mime: "audio/mp4".into(),
                file_name: "voice.m4a".into(),
                duration_ms: 4_200,
                voice_note: true,
                waveform: Some((0..64).collect()),
                ..AttachmentInput::default()
            },
            "",
            None,
        )
        .await?;
    for id in [photo_id, voice_id] {
        eventually("the automatic download", PATIENCE, || {
            ben.core.message(&id).is_ok_and(|m| {
                m.attachment
                    .is_some_and(|a| a.state == Some(AttachmentState::Local))
            })
        })
        .await?;
    }
    assert_eq!(ben.core.attachment_data(&photo_id)?, photo);
    assert_eq!(ben.core.attachment_data(&voice_id)?, voice);
    let info = ben.core.message(&photo_id)?.attachment.expect("photo");
    assert_eq!((info.width, info.height), (1200, 800));
    assert!(info.thumbnail.is_some());
    let info = ben.core.message(&voice_id)?.attachment.expect("voice note");
    assert!(info.voice_note);
    assert_eq!(info.waveform.map(|w| w.len()), Some(64));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn calls_share_one_key_and_end_cleanly() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");

    let outgoing = ada.core.start_call(&dm, true).await?;
    assert!(outgoing.url.starts_with("ws://127.0.0.1:"));
    assert_eq!(outgoing.token.split('.').count(), 3, "a JWT");
    assert!(outgoing.video);
    let call_id = outgoing.call_id;
    eventually("Ben's phone to ring", PATIENCE, || {
        ben.events
            .count(|e| matches!(e, CoreEvent::IncomingCall(c) if c.call_id == call_id))
            == 1
    })
    .await?;
    assert_eq!(ben.core.call(&call_id)?.state, CallState::Ringing);

    let answered = ben.core.accept_call(&call_id).await?;
    // Both ends derive the same LiveKit key from the MLS exporter; the relay never sees it.
    assert_eq!(answered.key_base64, outgoing.key_base64);
    assert_eq!(answered.key_index, outgoing.key_index);
    assert_eq!(answered.emoji, outgoing.emoji);
    assert_eq!(answered.room_alias, outgoing.room_alias);
    assert_ne!(answered.participant_alias, outgoing.participant_alias);
    eventually("Ada to see the call answered", PATIENCE, || {
        ada.core
            .call(&call_id)
            .is_ok_and(|c| c.state == CallState::Active)
    })
    .await?;
    assert!(ada.core.is_call_active(&call_id));
    // LiveKit only sees random aliases; the call signals tell each side whose alias is whose.
    assert_eq!(
        ben.core
            .call_participant(&call_id, &outgoing.participant_alias)?,
        Some(ada.user_id())
    );
    assert_eq!(
        ada.core
            .call_participant(&call_id, &answered.participant_alias)?,
        Some(ben.user_id())
    );
    assert_eq!(
        ada.core
            .call_participant(&call_id, &outgoing.participant_alias)?,
        Some(ada.user_id()),
        "our own alias"
    );
    assert_eq!(ada.core.call_participant(&call_id, "rnobody")?, None);

    tokio::time::sleep(Duration::from_millis(1100)).await;
    ada.core.end_call(&call_id)?;
    eventually("Ben to see the call end", PATIENCE, || {
        ben.core
            .call(&call_id)
            .is_ok_and(|c| c.state == CallState::Ended)
    })
    .await?;
    assert!(!ada.core.is_call_active(&call_id));
    eventually("both call lines", PATIENCE, || {
        [&ada, &ben].iter().all(|c| {
            c.messages(&dm)
                .iter()
                .any(|m| m.body.starts_with("Video call · ") && m.body.ends_with(" s"))
        })
    })
    .await?;
    assert_eq!(ada.core.recent_calls(10)?.len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declined_calls_and_key_rotation_in_groups() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let cam = h.invite(&ada, "Cam").await?;
    let dm = ada.dm_with(&ben).expect("DM");

    // A declined DM call ends for the caller too.
    let call = ada.core.start_call(&dm, false).await?;
    eventually("Ben's phone to ring", PATIENCE, || {
        ben.core.call(&call.call_id).is_ok()
    })
    .await?;
    ben.core.decline_call(&call.call_id)?;
    eventually("Ada to see the decline", PATIENCE, || {
        ada.core
            .call(&call.call_id)
            .is_ok_and(|c| c.state == CallState::Ended)
    })
    .await?;
    assert!(
        ada.messages(&dm)
            .iter()
            .any(|m| m.body == "Voice call · not answered")
    );

    // In a group call, a membership change mid-call moves everyone to the next key.
    let group = ada
        .core
        .create_chat(ChatKind::Group, "Standup", &[ben.user_id()])
        .await?;
    eventually("Ben to join the group", PATIENCE, || {
        ben.core.chat(&group).is_ok()
    })
    .await?;
    let a = ada.core.start_call(&group, false).await?;
    eventually("Ben's ring", PATIENCE, || ben.core.call(&a.call_id).is_ok()).await?;
    let b = ben.core.accept_call(&a.call_id).await?;
    assert_eq!(a.key_base64, b.key_base64);

    ada.core.add_members(&group, &[cam.user_id()]).await?;
    eventually("both to rotate the call key", PATIENCE, || {
        let rotated = |c: &asiri_sim::Client| {
            c.events.all().into_iter().rev().find_map(|e| match e {
                CoreEvent::CallKeyRotated {
                    call_id,
                    key_base64,
                    ..
                } if call_id == a.call_id => Some(key_base64),
                _ => None,
            })
        };
        matches!((rotated(&ada), rotated(&ben)), (Some(x), Some(y)) if x == y && x != a.key_base64)
    })
    .await?;
    let (ka, ia) = ada.core.current_call_key(&a.call_id)?;
    let (kb, ib) = ben.core.current_call_key(&a.call_id)?;
    assert_eq!((ka, ia), (kb, ib));
    assert_ne!(ia, a.key_index);
    Ok(())
}
