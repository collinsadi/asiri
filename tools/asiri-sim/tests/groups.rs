//! Groups end to end: creation, names, the admin policy, adding, removing, promoting, leaving,
//! concurrent commits ordered by the relay, and weekly self-updates.

use asiri_core::CoreError;
use asiri_core::store::ChatKind;
use asiri_sim::{Client, Harness, PATIENCE, eventually};

fn member_names(c: &Client, chat: &[u8]) -> Vec<String> {
    let mut names: Vec<String> = c
        .core
        .members(chat)
        .unwrap_or_default()
        .into_iter()
        .map(|m| {
            if m.is_me {
                c.name.clone()
            } else {
                m.contact.display_name
            }
        })
        .collect();
    names.sort();
    names
}

async fn everyone_in(chat: &[u8], clients: &[&Client], expected: &[&str]) -> anyhow::Result<()> {
    let mut want: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
    want.sort();
    for c in clients {
        if let Err(e) = eventually(
            &format!("{} to see members {want:?}", c.name),
            PATIENCE,
            || member_names(c, chat) == want,
        )
        .await
        {
            anyhow::bail!(
                "{e}; {} sees {:?}, lines {:?}",
                c.name,
                member_names(c, chat),
                c.system_lines(chat)
            );
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn groups_follow_the_admin_policy() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let cam = h.invite(&ada, "Cam").await?;

    let group = ada
        .core
        .create_chat(
            ChatKind::Group,
            "Book club",
            &[ben.user_id(), cam.user_id()],
        )
        .await?;
    everyone_in(&group, &[&ada, &ben, &cam], &["Ada", "Ben", "Cam"]).await?;
    for c in [&ben, &cam] {
        eventually(
            &format!("{} to see the group name", c.name),
            PATIENCE,
            || c.core.chat(&group).is_ok_and(|s| s.title == "Book club"),
        )
        .await?;
    }
    assert!(ada.core.chat(&group)?.is_admin);
    assert!(!ben.core.chat(&group)?.is_admin);

    // Ben and Cam never met, yet they can talk: MLS tells each who the other is, and profiles
    // travel inside the group.
    cam.core
        .send_text(&group, "Has everyone finished chapter 3?", None)?;
    ben.wait_for_text(&group, "Has everyone finished chapter 3?")
        .await?;
    ada.wait_for_text(&group, "Has everyone finished chapter 3?")
        .await?;
    eventually("Ben to learn Cam's name from the group", PATIENCE, || {
        member_names(&ben, &group) == ["Ada", "Ben", "Cam"]
    })
    .await?;

    // Only admins manage the group.
    let denied = ben.core.remove_member(&group, &cam.user_id()).await;
    assert!(
        matches!(denied, Err(CoreError::NotAllowed(_))),
        "{denied:?}"
    );
    let denied = ben.core.update_group(&group, "Ben's club", "", None);
    assert!(
        matches!(denied, Err(CoreError::NotAllowed(_))),
        "{denied:?}"
    );

    // Ada renames the group and makes Ben an admin.
    ada.core
        .update_group(&group, "Sci-fi book club", "Monthly", None)?;
    eventually("Cam to see the new name", PATIENCE, || {
        cam.core
            .chat(&group)
            .is_ok_and(|s| s.title == "Sci-fi book club")
    })
    .await?;
    ada.core.set_admin(&group, &ben.user_id(), true).await?;
    eventually("Ben to become an admin", PATIENCE, || {
        ben.core.chat(&group).is_ok_and(|s| s.is_admin)
    })
    .await?;

    // Now Ben may remove Cam. Cam is told and can no longer write.
    ben.core.remove_member(&group, &cam.user_id()).await?;
    everyone_in(&group, &[&ada, &ben], &["Ada", "Ben"]).await?;
    eventually("Cam to learn she was removed", PATIENCE, || {
        cam.core.chat(&group).is_ok_and(|s| s.chat.left)
    })
    .await?;
    assert!(cam.core.send_text(&group, "still here?", None).is_err());
    assert!(
        cam.system_lines(&group)
            .iter()
            .any(|l| l == "Ben removed you")
    );
    assert!(
        ada.system_lines(&group)
            .iter()
            .any(|l| l == "Ben removed Cam")
    );

    // Messages after the removal never reach Cam.
    ada.core.send_text(&group, "Just us now", None)?;
    ben.wait_for_text(&group, "Just us now").await?;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!cam.texts(&group).iter().any(|t| t == "Just us now"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn members_join_later_and_leave_on_their_own() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let cam = h.invite(&ada, "Cam").await?;
    let group = ada
        .core
        .create_chat(ChatKind::Group, "Climbing", &[ben.user_id()])
        .await?;
    everyone_in(&group, &[&ada, &ben], &["Ada", "Ben"]).await?;

    ben.core.send_text(&group, "before Cam joined", None)?;
    ada.wait_for_text(&group, "before Cam joined").await?;

    ada.core.add_members(&group, &[cam.user_id()]).await?;
    everyone_in(&group, &[&ada, &ben, &cam], &["Ada", "Ben", "Cam"]).await?;
    eventually("Cam to see the group name", PATIENCE, || {
        cam.core.chat(&group).is_ok_and(|s| s.title == "Climbing")
    })
    .await?;
    // New members can't read what was sent before they joined (an MLS guarantee).
    assert!(cam.texts(&group).is_empty());
    cam.core.send_text(&group, "hi all", None)?;
    ben.wait_for_text(&group, "hi all").await?;

    // Ben leaves; a remaining member commits his removal.
    ben.core.leave_chat(&group).await?;
    assert!(ben.core.chat(&group)?.chat.left);
    everyone_in(&group, &[&ada, &cam], &["Ada", "Cam"]).await?;
    assert!(ada.system_lines(&group).iter().any(|l| l == "Ben left"));
    cam.core.send_text(&group, "bye Ben", None)?;
    ada.wait_for_text(&group, "bye Ben").await?;

    // The last admin leaving hands the group to someone else first.
    ada.core.leave_chat(&group).await?;
    everyone_in(&group, &[&cam], &["Cam"]).await?;
    assert!(cam.core.chat(&group)?.is_admin);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_commits_are_ordered_and_retried() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let cam = h.invite(&ada, "Cam").await?;
    let dan = h.invite(&ada, "Dan").await?;
    let group = ada
        .core
        .create_chat(ChatKind::Group, "Ops", &[ben.user_id(), cam.user_id()])
        .await?;
    everyone_in(&group, &[&ada, &ben, &cam], &["Ada", "Ben", "Cam"]).await?;

    // Three commits race for the same epoch: an add and two key rotations. The relay accepts
    // one per epoch; the others are discarded, re-applied on the new epoch and retried.
    let newcomer = [dan.user_id()];
    let (add, rot_b, rot_c) = tokio::join!(
        ada.core.add_members(&group, &newcomer),
        ben.core.self_update(&group),
        cam.core.self_update(&group),
    );
    add?;
    rot_b?;
    rot_c?;
    everyone_in(
        &group,
        &[&ada, &ben, &cam, &dan],
        &["Ada", "Ben", "Cam", "Dan"],
    )
    .await?;

    // Everyone ended on the same epoch and can still talk.
    dan.core.send_text(&group, "made it", None)?;
    for c in [&ada, &ben, &cam] {
        c.wait_for_text(&group, "made it").await?;
    }
    ben.core.send_text(&group, "welcome Dan", None)?;
    dan.wait_for_text(&group, "welcome Dan").await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_admins_cannot_create_policy_breaking_changes() -> anyhow::Result<()> {
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let cam = h.invite(&ada, "Cam").await?;
    let group = ada
        .core
        .create_chat(ChatKind::Group, "Policy", &[ben.user_id()])
        .await?;
    everyone_in(&group, &[&ada, &ben], &["Ada", "Ben"]).await?;
    // Ben may not add Cam (and doesn't know her anyway): refused before anything is sent.
    let denied = ben.core.add_members(&group, &[cam.user_id()]).await;
    assert!(denied.is_err());
    // Ben may not promote himself.
    let denied = ben.core.set_admin(&group, &ben.user_id(), true).await;
    assert!(
        matches!(denied, Err(CoreError::NotAllowed(_))),
        "{denied:?}"
    );
    everyone_in(&group, &[&ada, &ben], &["Ada", "Ben"]).await?;
    assert!(
        !ada.core
            .members(&group)?
            .iter()
            .any(|m| m.contact.user_id == ben.user_id() && m.is_admin)
    );
    Ok(())
}
