//! Direct messages end to end: pairing, delivery, receipts, reactions, edits, deletes, typing,
//! profiles, verification, offline delivery and restarts.

use std::time::Duration;

use asiri_core::CoreEvent;
use asiri_core::store::{MessageStatus, Verification};
use asiri_sim::{Harness, PATIENCE, eventually};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pairing_creates_a_verified_dm_and_messages_flow_both_ways() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    assert!(ada.core.profile()?.is_admin);
    let ben = h.invite(&ada, "Ben").await?;
    assert!(!ben.core.profile()?.is_admin);

    let dm_b = ben.dm_with(&ada).expect("Ben has the DM");
    let dm_a = ada.dm_with(&ben).expect("Ada has the DM");
    assert_eq!(dm_a, dm_b);
    // Names travel inside the codes and MLS, never as server metadata.
    assert_eq!(ada.core.chat(&dm_a)?.title, "Ben");
    assert_eq!(ben.core.chat(&dm_b)?.title, "Ada");
    eventually("Ada's pairing to complete", PATIENCE, || {
        ada.events
            .count(|e| matches!(e, CoreEvent::PairingCompleted { .. }))
            == 1
    })
    .await?;

    // Both confirm the emoji: the contact becomes verified in person.
    ada.core.confirm_verification(&ben.user_id(), true)?;
    ben.core.confirm_verification(&ada.user_id(), true)?;
    assert_eq!(
        ada.core.chat(&dm_a)?.verification,
        Some(Verification::InPerson)
    );
    assert_eq!(
        ada.core.safety_number(&ben.user_id())?,
        ben.core.safety_number(&ada.user_id())?
    );

    let hello = ben.core.send_text(&dm_b, "Hello Ada", None)?;
    ada.wait_for_text(&dm_a, "Hello Ada").await?;
    ada.core.send_text(&dm_a, "Hi Ben 👋", None)?;
    ben.wait_for_text(&dm_b, "Hi Ben 👋").await?;

    // Delivery receipts come back automatically; read receipts once Ada opens the chat.
    eventually("Ben's message to show as delivered", PATIENCE, || {
        ben.core
            .message(&hello)
            .is_ok_and(|m| m.status == MessageStatus::Delivered)
    })
    .await?;
    ada.core.mark_read(&dm_a)?;
    eventually("Ben's message to show as read", PATIENCE, || {
        ben.core
            .message(&hello)
            .is_ok_and(|m| m.status == MessageStatus::Read)
    })
    .await?;
    assert_eq!(ada.core.chat(&dm_a)?.chat.unread, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reactions_edits_deletes_typing_and_replies() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");

    let m = ada.core.send_text(&dm, "Dinner tonight?", None)?;
    ben.wait_for_text(&dm, "Dinner tonight?").await?;

    ben.core.react(&dm, &m, "❤️", false)?;
    eventually("Ada to see Ben's reaction", PATIENCE, || {
        ada.core.message(&m).is_ok_and(|msg| {
            msg.reactions
                .iter()
                .any(|r| r.emoji == "❤️" && r.count == 1)
        })
    })
    .await?;
    ben.core.react(&dm, &m, "❤️", true)?;
    eventually("the reaction to disappear", PATIENCE, || {
        ada.core
            .message(&m)
            .is_ok_and(|msg| msg.reactions.is_empty())
    })
    .await?;

    ada.core.edit_message(&dm, &m, "Dinner tomorrow?")?;
    eventually("Ben to see the edit", PATIENCE, || {
        ben.core
            .message(&m)
            .is_ok_and(|msg| msg.body == "Dinner tomorrow?" && msg.edited)
    })
    .await?;
    // Only the author can edit.
    assert!(ben.core.edit_message(&dm, &m, "hijacked").is_err());

    let reply = ben.core.send_text(&dm, "Sure!", Some(m))?;
    ada.wait_for_text(&dm, "Sure!").await?;
    assert_eq!(ada.core.message(&reply)?.reply_to, Some(m));

    ada.core.set_typing(&dm, true)?;
    eventually("Ben to see Ada typing", PATIENCE, || {
        ben.events
            .count(|e| matches!(e, CoreEvent::Typing { active: true, .. }))
            >= 1
    })
    .await?;

    ada.core.delete_for_everyone(&dm, &m)?;
    eventually("Ben to see the delete", PATIENCE, || {
        ben.core
            .message(&m)
            .is_ok_and(|msg| msg.deleted && msg.body.is_empty())
    })
    .await?;
    // Deleting for me only touches this Mac.
    ben.core.delete_for_me(&dm, &reply)?;
    assert!(ben.core.message(&reply).is_err());
    assert!(ada.core.message(&reply).is_ok());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messages_wait_for_offline_friends_and_survive_restarts() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let mut ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");

    // Ben quits; Ada keeps writing.
    ben.core.disconnect().await;
    for i in 1..=5 {
        ada.core.send_text(&dm, &format!("message {i}"), None)?;
    }
    eventually("Ada's messages to reach the relay", PATIENCE, || {
        ada.core.pending_outgoing().is_ok_and(|n| n == 0)
    })
    .await?;

    // Ben relaunches: the engine reopens its encrypted store, reconnects and catches up in order.
    ben.restart().await?;
    ben.wait_for_text(&dm, "message 5").await?;
    let texts = ben.texts(&dm);
    assert_eq!(
        texts,
        (1..=5).map(|i| format!("message {i}")).collect::<Vec<_>>()
    );

    // MLS state survived the restart: Ben can still write.
    ben.core.send_text(&dm, "back online", None)?;
    ada.wait_for_text(&dm, "back online").await?;

    // Sending while offline queues the message and delivers it on reconnect.
    ada.core.disconnect().await;
    let queued = ada.core.send_text(&dm, "sent while offline", None)?;
    assert_eq!(ada.core.message(&queued)?.status, MessageStatus::Pending);
    tokio::time::sleep(Duration::from_millis(300)).await;
    ada.core.connect()?;
    ben.wait_for_text(&dm, "sent while offline").await?;
    eventually("the queued message to be sent", PATIENCE, || {
        ada.core.message(&queued).is_ok_and(|m| {
            matches!(
                m.status,
                MessageStatus::Sent | MessageStatus::Delivered | MessageStatus::Read
            )
        })
    })
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profiles_and_disappearing_messages() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");

    ada.core.set_profile("Ada Lovelace", None)?;
    eventually("Ben to see Ada's new name", PATIENCE, || {
        ben.core.chat(&dm).is_ok_and(|c| c.title == "Ada Lovelace")
    })
    .await?;

    ada.core.set_disappearing_timer(&dm, 2)?;
    eventually("Ben to adopt the timer", PATIENCE, || {
        ben.core.chat(&dm).is_ok_and(|c| c.chat.timer_s == 2)
    })
    .await?;
    let gone = ada.core.send_text(&dm, "this will vanish", None)?;
    ben.wait_for_text(&dm, "this will vanish").await?;
    eventually(
        "the message to disappear on both Macs",
        Duration::from_secs(20),
        || ada.core.message(&gone).is_err() && ben.core.message(&gone).is_err(),
    )
    .await?;
    assert!(
        ben.events
            .count(|e| matches!(e, CoreEvent::MessageRemoved { .. }))
            >= 1
    );

    ada.core.set_disappearing_timer(&dm, 0)?;
    eventually("Ben to turn the timer off", PATIENCE, || {
        ben.core.chat(&dm).is_ok_and(|c| c.chat.timer_s == 0)
    })
    .await?;
    let kept = ada.core.send_text(&dm, "this stays", None)?;
    ben.wait_for_text(&dm, "this stays").await?;
    assert!(ben.core.message(&kept)?.expires_at_ms.is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_finds_messages_inside_the_encrypted_store() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");
    ada.core
        .send_text(&dm, "The launch code is pineapple", None)?;
    ben.wait_for_text(&dm, "The launch code is pineapple")
        .await?;
    let hits = ben.core.search("pineap", 10)?;
    assert_eq!(hits.len(), 1);
    assert!(ben.core.search("banana", 10)?.is_empty());
    Ok(())
}
