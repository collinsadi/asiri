//! The test buddy (`asiri-sim buddy`) pairs like a new Mac and answers like a friend, so one
//! person with one Mac can try every part of Asiri.

use asiri_core::engine::AttachmentInput;
use asiri_core::ids::Id16;
use asiri_core::store::ChatKind;
use asiri_sim::buddy::Buddy;
use asiri_sim::{Client, Harness, PATIENCE, eventually};

fn dm_with(client: &Client, peer: Id16) -> Option<Vec<u8>> {
    client
        .core
        .chats()
        .ok()?
        .into_iter()
        .find(|c| c.chat.kind == ChatKind::Direct && c.chat.dm_user == Some(peer))
        .map(|c| c.chat.chat_id)
}

async fn wait_for_reply(ada: &Client, dm: &[u8], reply: &str) -> anyhow::Result<()> {
    eventually(
        &format!("the buddy to answer “{reply}”"),
        PATIENCE,
        || ada.texts(dm).iter().any(|t| t == reply),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_buddy_pairs_and_answers_texts_files_and_calls() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;

    // Pairing, as the command line does it: the buddy's code into Asiri, Asiri's reply back.
    let dir = tempfile::tempdir()?;
    let buddy = Buddy::open("Ben", dir.path(), &[7; 32])?;
    let offer = buddy.core.device_offer_code()?;
    let invitation = ada.core.invite_device(&offer).await?;
    let reply = invitation.reply_code.clone().expect("a reply code");
    let joined = buddy.core.redeem_invitation(&reply).await?;
    assert_eq!(
        joined.emoji, invitation.emoji,
        "both screens show the same emoji"
    );
    let ben = buddy.core.profile()?.user_id;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(buddy.serve(async {
        let _ = stopped.await;
    }));

    eventually("the DM", PATIENCE, || dm_with(&ada, ben).is_some()).await?;
    let dm = dm_with(&ada, ben).expect("DM");
    eventually("the buddy's hello", PATIENCE, || {
        ada.texts(&dm)
            .iter()
            .any(|t| t.starts_with("Hi! I'm Ben, a test buddy."))
    })
    .await?;

    ada.core.send_text(&dm, "hello there", None)?;
    wait_for_reply(&ada, &dm, "Hey! 👋 Good to hear from you.").await?;
    ada.core.send_text(&dm, "ping", None)?;
    wait_for_reply(&ada, &dm, "pong 🏓").await?;
    let note = ada.core.send_text(&dm, "Lunch at noon", None)?;
    wait_for_reply(&ada, &dm, "You said: “Lunch at noon”").await?;
    eventually("a 👍 on the note", PATIENCE, || {
        ada.core
            .message(&note)
            .is_ok_and(|m| m.reactions.iter().any(|r| r.emoji == "👍"))
    })
    .await?;

    // Files: it downloads and decrypts them, then says so.
    ada.core
        .send_attachment(
            &dm,
            AttachmentInput {
                data: b"shopping list: rice, plantain, pepper".to_vec(),
                mime: "text/plain".into(),
                file_name: "list.txt".into(),
                ..AttachmentInput::default()
            },
            "",
            None,
        )
        .await?;
    wait_for_reply(
        &ada,
        &dm,
        "Got your “list.txt”: 37 bytes decrypted fine on my side.",
    )
    .await?;

    // Calls: it declines politely.
    let call = ada.core.start_call(&dm, false).await?;
    wait_for_reply(
        &ada,
        &dm,
        "I'm a test buddy, so I can't take calls. Try calling a friend!",
    )
    .await?;
    eventually("the call to end", PATIENCE, || {
        ada.core
            .call(&call.call_id)
            .is_ok_and(|c| c.state == asiri_core::CallState::Ended)
    })
    .await?;

    let _ = stop.send(());
    serving.await??;
    Ok(())
}
