//! The admin policy every client enforces on every commit (architecture §7.5).
//!
//! * Any member may commit their own key updates and their own removal.
//! * Only admins (listed in the `asiri.admins` extension) may add or remove others or change the
//!   group context. In a DM both people are admins.
//! * The `asiri.group` extension (the relay route) can never change.
//! * Pre-shared keys, ReInit and external joins are not used by Asiri and are rejected.
//!
//! A violating commit is not merged; the caller reports it loudly.

use openmls::prelude::*;

use super::{CommitSummary, EXT_GROUP, MemberInfo, admins, member_info};
use crate::error::{CoreError, CoreResult};

/// A commit or proposal that breaks the admin policy.
#[derive(Debug, Clone)]
pub struct PolicyViolation(pub String);

fn violation(why: &str) -> CoreError {
    CoreError::NotAllowed(format!(
        "Rejected a group change that breaks the admin rules: {why}"
    ))
}

/// Validates a staged commit and summarizes its effect (before merging).
pub(super) fn check_commit(
    group: &MlsGroup,
    sender: &MemberInfo,
    staged: &StagedCommit,
) -> CoreResult<CommitSummary> {
    let admin_list = admins(group);
    let is_admin = admin_list.contains(&sender.user_id);
    let mut summary = CommitSummary::default();

    for add in staged.add_proposals() {
        let leaf = add.add_proposal().key_package().leaf_node();
        summary.added.push(member_info(
            leaf.credential(),
            leaf.signature_key().as_slice(),
            u32::MAX,
        )?);
    }
    if !summary.added.is_empty() && !is_admin {
        return Err(violation("only admins can add members"));
    }

    for rem in staged.remove_proposals() {
        let leaf = rem.remove_proposal().removed();
        let member = group
            .member_at(leaf)
            .ok_or_else(|| CoreError::Crypto("removal of an unknown member".into()))?;
        let info = member_info(&member.credential, &member.signature_key, leaf.u32())?;
        let proposed_by_removed = matches!(rem.sender(), Sender::Member(l) if *l == leaf);
        if !is_admin && info.user_id != sender.user_id && !proposed_by_removed {
            return Err(violation("only admins can remove other people"));
        }
        summary.removed.push(info);
    }

    for queued in staged.queued_proposals() {
        match queued.proposal() {
            Proposal::GroupContextExtensions(_) => {
                if !is_admin {
                    return Err(violation("only admins can change the group settings"));
                }
                summary.extensions_changed = true;
            }
            Proposal::PreSharedKey(_) | Proposal::ReInit(_) | Proposal::ExternalInit(_) => {
                return Err(violation("unsupported proposal type"));
            }
            _ => {}
        }
    }

    let before = group.extensions().unknown(EXT_GROUP).map(|e| e.0.clone());
    let after = staged
        .group_context()
        .extensions()
        .unknown(EXT_GROUP)
        .map(|e| e.0.clone());
    if before != after {
        return Err(violation("the chat's identity cannot change"));
    }
    if admins_after(staged).is_empty() {
        return Err(violation("a group must keep at least one admin"));
    }

    summary.self_removed = staged.self_removed();
    Ok(summary)
}

fn admins_after(staged: &StagedCommit) -> Vec<crate::ids::Id16> {
    use asiri_proto::Message as _;
    staged
        .group_context()
        .extensions()
        .unknown(super::EXT_ADMINS)
        .and_then(|e| asiri_proto::app::v1::AdminList::decode(e.0.as_slice()).ok())
        .map(|l| {
            l.user_ids
                .iter()
                .filter_map(|u| crate::ids::id16(u))
                .collect()
        })
        .unwrap_or_default()
}

/// Validates a standalone proposal. Members may only propose their own removal (leaving) or
/// their own update. Returns whether it is a self-removal.
pub(super) fn check_proposal(sender: &MemberInfo, proposal: &QueuedProposal) -> CoreResult<bool> {
    match proposal.proposal() {
        Proposal::Remove(r) if r.removed().u32() == sender.leaf => Ok(true),
        Proposal::Update(_) => Ok(false),
        _ => Err(violation("members may only propose leaving")),
    }
}
