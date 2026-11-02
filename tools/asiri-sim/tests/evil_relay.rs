//! What clients do when the relay turns against them (architecture §18): the malicious-relay
//! proxy drops, duplicates, reorders, tampers with, replays, withholds and fabricates envelopes.
//! Clients must never accept forged or altered messages, must ignore replays and duplicates,
//! and must notice and report what went missing.

use std::time::Duration;

use asiri_core::store::ChatKind;
use asiri_proto::relay::v1::{self as wire, PayloadKind};
use asiri_sim::{Action, Client, Harness, PATIENCE, eventually};

fn random_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    getrandom::fill(&mut v).expect("randomness");
    v
}

/// Lets in-flight traffic (profiles, receipts) settle so plans hit the intended envelope.
async fn settle(clients: &[&Client]) -> anyhow::Result<()> {
    for c in clients {
        eventually("the outbox to drain", PATIENCE, || {
            c.core.pending_outgoing().is_ok_and(|n| n == 0)
        })
        .await?;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    Ok(())
}

fn count(c: &Client, chat: &[u8], text: &str) -> usize {
    c.texts(chat).iter().filter(|t| *t == text).count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicates_and_replays_are_ignored() -> anyhow::Result<()> {
    let h = Harness::start_evil().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");
    settle(&[&ada, &ben]).await?;
    let evil = h.evil();

    evil.plan(
        &ben.inbox_id(),
        Some(PayloadKind::Application),
        [Action::Duplicate],
    );
    ada.core.send_text(&dm, "only once", None)?;
    ben.wait_for_text(&dm, "only once").await?;

    // The relay replays the same ciphertext under a fresh envelope ID and sequence number.
    let original = evil
        .seen(&ben.inbox_id())
        .into_iter()
        .rev()
        .find(|d| d.kind == PayloadKind::Application as i32)
        .expect("a delivered message");
    evil.inject(
        &ben.inbox_id(),
        wire::Deliver {
            seq: 0,
            envelope_id: random_bytes(16),
            kind: original.kind,
            payload: original.payload.clone(),
        },
    )?;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(count(&ben, &dm, "only once"), 1);

    ada.core.send_text(&dm, "still fine", None)?;
    ben.wait_for_text(&dm, "still fine").await?;
    assert_eq!(count(&ben, &dm, "only once"), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tampered_messages_are_rejected_and_reported() -> anyhow::Result<()> {
    let h = Harness::start_evil().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");
    settle(&[&ada, &ben]).await?;

    h.evil().plan(
        &ben.inbox_id(),
        Some(PayloadKind::Application),
        [Action::Tamper],
    );
    ada.core.send_text(&dm, "pay 100 to Ben", None)?;
    ada.core.send_text(&dm, "after the tampered one", None)?;
    ben.wait_for_text(&dm, "after the tampered one").await?;
    assert_eq!(
        count(&ben, &dm, "pay 100 to Ben"),
        0,
        "an altered message was accepted"
    );
    eventually("Ben to be told a message is missing", PATIENCE, || {
        ben.system_lines(&dm)
            .iter()
            .any(|l| l == "Some messages from Ada may not have arrived.")
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn suppressed_messages_are_noticed() -> anyhow::Result<()> {
    let h = Harness::start_evil().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");
    settle(&[&ada, &ben]).await?;

    // The relay silently drops one message (and acknowledges it so it's gone for good).
    h.evil().plan(
        &ben.inbox_id(),
        Some(PayloadKind::Application),
        [Action::Drop],
    );
    ada.core.send_text(&dm, "meet at the north gate", None)?;
    ada.core.send_text(&dm, "see you soon", None)?;
    ben.wait_for_text(&dm, "see you soon").await?;
    assert_eq!(count(&ben, &dm, "meet at the north gate"), 0);
    eventually("Ben to be told a message is missing", PATIENCE, || {
        ben.system_lines(&dm)
            .iter()
            .any(|l| l == "Some messages from Ada may not have arrived.")
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reordered_messages_are_shown_in_order() -> anyhow::Result<()> {
    let h = Harness::start_evil().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");
    settle(&[&ada, &ben]).await?;

    h.evil().plan(
        &ben.inbox_id(),
        Some(PayloadKind::Application),
        [Action::Delay],
    );
    ada.core.send_text(&dm, "first", None)?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    ada.core.send_text(&dm, "second", None)?;
    ben.wait_for_text(&dm, "first").await?;
    ben.wait_for_text(&dm, "second").await?;
    let texts = ben.texts(&dm);
    let (a, b) = (
        texts.iter().position(|t| t == "first"),
        texts.iter().position(|t| t == "second"),
    );
    assert!(a < b, "timeline order follows the sender: {texts:?}");
    // Reordering is not loss: no gap warning.
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert!(
        !ben.system_lines(&dm)
            .iter()
            .any(|l| l.contains("may not have arrived")),
        "{:?}",
        ben.system_lines(&dm)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fabricated_envelopes_change_nothing() -> anyhow::Result<()> {
    let h = Harness::start_evil().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");
    settle(&[&ada, &ben]).await?;
    let before = ben.messages(&dm).len();

    // Garbage of every kind, padded correctly and not.
    for kind in [
        PayloadKind::Application,
        PayloadKind::Commit,
        PayloadKind::Proposal,
        PayloadKind::Welcome,
    ] {
        for payload in [random_bytes(512), random_bytes(77), vec![0u8; 256]] {
            h.evil().inject(
                &ben.inbox_id(),
                wire::Deliver {
                    seq: 0,
                    envelope_id: random_bytes(16),
                    kind: kind as i32,
                    payload,
                },
            )?;
        }
    }
    // A real message from Ada, replayed under every other label.
    let real = h
        .evil()
        .seen(&ben.inbox_id())
        .into_iter()
        .rev()
        .find(|d| d.kind == PayloadKind::Application as i32)
        .expect("a delivered message");
    for kind in [
        PayloadKind::Commit,
        PayloadKind::Proposal,
        PayloadKind::Welcome,
    ] {
        h.evil().inject(
            &ben.inbox_id(),
            wire::Deliver {
                seq: 0,
                envelope_id: random_bytes(16),
                kind: kind as i32,
                payload: real.payload.clone(),
            },
        )?;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        ben.messages(&dm).len(),
        before,
        "fabricated envelopes created messages"
    );
    assert_eq!(
        ben.core.chats()?.len(),
        1,
        "a fabricated Welcome created a chat"
    );
    ada.core.send_text(&dm, "the real one", None)?;
    ben.wait_for_text(&dm, "the real one").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_withheld_commit_is_reported() -> anyhow::Result<()> {
    let h = Harness::start_evil().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let cam = h.invite(&ada, "Cam").await?;
    let group = ada
        .core
        .create_chat(ChatKind::Group, "Board", &[ben.user_id()])
        .await?;
    eventually("Ben to join", PATIENCE, || ben.core.chat(&group).is_ok()).await?;
    settle(&[&ada, &ben]).await?;

    // The relay hides the commit that adds Cam from Ben, so Ben can't follow the new epoch.
    h.evil()
        .plan(&ben.inbox_id(), Some(PayloadKind::Commit), [Action::Drop]);
    ada.core.add_members(&group, &[cam.user_id()]).await?;
    eventually("Cam to join", PATIENCE, || cam.core.chat(&group).is_ok()).await?;
    ada.core.send_text(&group, "new epoch message", None)?;
    cam.wait_for_text(&group, "new epoch message").await?;

    // Ben can't read it (it's from an epoch he never reached) and is told so.
    eventually("Ben to be told the chat is out of sync", PATIENCE, || {
        ben.system_lines(&group)
            .iter()
            .any(|l| l.starts_with("Some messages in this chat can't be read"))
    })
    .await?;
    assert_eq!(count(&ben, &group, "new epoch message"), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_commit_unblocks_waiting_messages() -> anyhow::Result<()> {
    let h = Harness::start_evil().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let cam = h.invite(&ada, "Cam").await?;
    let group = ada
        .core
        .create_chat(ChatKind::Group, "Late", &[ben.user_id()])
        .await?;
    eventually("Ben to join", PATIENCE, || ben.core.chat(&group).is_ok()).await?;
    settle(&[&ada, &ben]).await?;

    // The commit adding Cam reaches Ben only after messages from the new epoch.
    h.evil()
        .plan(&ben.inbox_id(), Some(PayloadKind::Commit), [Action::Delay]);
    ada.core.add_members(&group, &[cam.user_id()]).await?;
    ada.core.send_text(&group, "from the new epoch", None)?;
    ben.wait_for_text(&group, "from the new epoch").await?;
    assert!(
        ben.system_lines(&group)
            .iter()
            .any(|l| l == "Ada added Cam")
    );
    Ok(())
}
