//! MLS behaviour that Asiri relies on, exercised with real encrypted stores.

use super::*;
use crate::store::Store;

struct Client {
    store: Store,
    mls: Mls,
    signer: SignatureKeyPair,
    user: Id16,
    device: Id16,
    inbox: Id16,
}

impl Client {
    fn new(n: u8) -> Self {
        let store = Store::open_in_memory().unwrap();
        let mls = Mls::new().unwrap();
        let signer = {
            let conn = store.lock().unwrap();
            mls.new_signer(&conn).unwrap()
        };
        Self {
            store,
            mls,
            signer,
            user: [n; 16],
            device: [n + 100; 16],
            inbox: [n + 200; 16],
        }
    }

    fn credential(&self) -> CredentialWithKey {
        Mls::credential(
            encode_identity(&self.user, &self.device, &self.inbox),
            &self.signer,
        )
    }

    fn key_package(&self) -> KeyPackage {
        let conn = self.store.lock().unwrap();
        let bytes = self
            .mls
            .key_package(&conn, &self.signer, self.credential(), false)
            .unwrap();
        let (kp, info) = self.mls.validate_key_package(&bytes).unwrap();
        assert_eq!(info.user_id, self.user);
        assert_eq!(info.inbox_id, self.inbox);
        kp
    }

    fn create(&self, gid: &[u8], kind: ChatKind, admins: &[Id16]) -> MlsGroup {
        let conn = self.store.lock().unwrap();
        self.mls
            .create_group(
                &conn,
                &self.signer,
                self.credential(),
                gid,
                GroupMeta {
                    route: [42; 16],
                    kind,
                },
                admins,
            )
            .unwrap()
    }

    fn add(&self, group: &mut MlsGroup, kps: &[KeyPackage]) -> CommitOut {
        let conn = self.store.lock().unwrap();
        let out = self.mls.add(&conn, group, &self.signer, kps).unwrap();
        self.mls.merge_pending(&conn, group).unwrap();
        out
    }

    fn join(&self, welcome: &[u8]) -> (MlsGroup, MemberInfo) {
        let conn = self.store.lock().unwrap();
        self.mls.join(&conn, welcome).unwrap()
    }

    fn send(&self, group: &mut MlsGroup, text: &[u8]) -> Vec<u8> {
        let conn = self.store.lock().unwrap();
        self.mls.encrypt(&conn, group, &self.signer, text).unwrap()
    }

    fn recv(&self, group: &mut MlsGroup, bytes: &[u8]) -> CoreResult<Incoming> {
        let conn = self.store.lock().unwrap();
        let (gid, msg) = parse_protocol_message(bytes)?;
        assert_eq!(gid, group.group_id().as_slice());
        self.mls.process(&conn, group, msg)
    }
}

fn app_text(i: Incoming) -> Vec<u8> {
    match i {
        Incoming::Application { plaintext, .. } => plaintext,
        other => panic!("expected an application message, got {other:?}"),
    }
}

#[test]
fn direct_chat_round_trip_with_post_quantum_suite() {
    let alice = Client::new(1);
    let bob = Client::new(2);
    let gid = [9u8; 32];
    let mut ga = alice.create(&gid, ChatKind::Direct, &[alice.user, bob.user]);
    let out = alice.add(&mut ga, &[bob.key_package()]);
    let (mut gb, inviter) = bob.join(out.welcome.as_ref().unwrap());

    assert_eq!(inviter.user_id, alice.user);
    assert_eq!(ga.ciphersuite(), SUITE);
    assert_eq!(
        group_meta(&gb),
        Some(GroupMeta {
            route: [42; 16],
            kind: ChatKind::Direct
        })
    );
    assert_eq!(admins(&gb), vec![alice.user, bob.user]);
    assert_eq!(members(&gb).len(), 2);

    let ct = alice.send(&mut ga, b"are we still on for 6?");
    let short = alice.send(&mut ga, b"k");
    assert_eq!(
        ct.len(),
        short.len(),
        "padding hides the length of short messages"
    );
    assert_eq!(app_text(bob.recv(&mut gb, &short).unwrap()), b"k");
    assert_eq!(
        app_text(bob.recv(&mut gb, &ct).unwrap()),
        b"are we still on for 6?"
    );
    let reply = bob.send(&mut gb, b"yes! usual place");
    assert_eq!(
        app_text(alice.recv(&mut ga, &reply).unwrap()),
        b"yes! usual place"
    );

    // Replaying a ciphertext is rejected by MLS itself.
    assert!(bob.recv(&mut gb, &ct).is_err());

    // Both sides derive the same call key; different calls get different keys.
    let ka = alice.mls.call_secret(&ga, b"call-1").unwrap();
    let kb = bob.mls.call_secret(&gb, b"call-1").unwrap();
    assert_eq!(ka.as_slice(), kb.as_slice());
    assert_ne!(
        ka.as_slice(),
        alice.mls.call_secret(&ga, b"call-2").unwrap().as_slice()
    );

    // A self-update moves both to the next epoch (post-compromise security).
    let conn = bob.store.lock().unwrap();
    let up = bob.mls.self_update(&conn, &mut gb, &bob.signer).unwrap();
    bob.mls.merge_pending(&conn, &mut gb).unwrap();
    drop(conn);
    match alice.recv(&mut ga, &up.commit).unwrap() {
        Incoming::Commit { summary, sender } => {
            assert_eq!(sender.user_id, bob.user);
            assert_eq!(summary.epoch, epoch(&gb));
        }
        other => panic!("{other:?}"),
    }
    assert_ne!(
        alice.mls.call_secret(&ga, b"call-1").unwrap().as_slice(),
        ka.as_slice(),
        "call keys rotate with the epoch"
    );
}

#[test]
fn admin_policy_is_enforced_by_receivers() {
    let alice = Client::new(1);
    let bob = Client::new(2);
    let carol = Client::new(3);
    let dave = Client::new(4);
    let gid = [8u8; 32];
    let mut ga = alice.create(&gid, ChatKind::Group, &[alice.user]);
    let out = alice.add(&mut ga, &[bob.key_package(), carol.key_package()]);
    let welcome = out.welcome.unwrap();
    let (mut gb, _) = bob.join(&welcome);
    let (mut gc, _) = carol.join(&welcome);

    // Bob is not an admin: his attempt to add Dave is rejected by everyone else.
    let conn = bob.store.lock().unwrap();
    let rogue = bob
        .mls
        .add(&conn, &mut gb, &bob.signer, &[dave.key_package()])
        .unwrap();
    bob.mls.discard_pending(&conn, &mut gb).unwrap();
    drop(conn);
    let err = alice.recv(&mut ga, &rogue.commit).unwrap_err();
    assert!(matches!(err, CoreError::NotAllowed(_)), "{err}");
    assert!(carol.recv(&mut gc, &rogue.commit).is_err());

    // The admin removes Carol; Bob sees who left and Carol sees she was removed.
    let carol_leaf = members(&ga)
        .iter()
        .find(|m| m.user_id == carol.user)
        .unwrap()
        .leaf;
    let conn = alice.store.lock().unwrap();
    let rm = alice
        .mls
        .remove(&conn, &mut ga, &alice.signer, &[carol_leaf])
        .unwrap();
    alice.mls.merge_pending(&conn, &mut ga).unwrap();
    drop(conn);
    match bob.recv(&mut gb, &rm.commit).unwrap() {
        Incoming::Commit { summary, .. } => {
            assert_eq!(summary.removed.len(), 1);
            assert_eq!(summary.removed[0].user_id, carol.user);
            assert!(!summary.self_removed);
        }
        other => panic!("{other:?}"),
    }
    match carol.recv(&mut gc, &rm.commit).unwrap() {
        Incoming::Commit { summary, .. } => assert!(summary.self_removed),
        other => panic!("{other:?}"),
    }
    // Carol can no longer read the group.
    let after = alice.send(&mut ga, b"carol can't see this");
    assert!(carol.recv(&mut gc, &after).is_err());
    assert_eq!(
        app_text(bob.recv(&mut gb, &after).unwrap()),
        b"carol can't see this"
    );
}

#[test]
fn members_leave_by_proposal_and_admin_commits_it() {
    let alice = Client::new(1);
    let bob = Client::new(2);
    let mut ga = alice.create(&[7u8; 32], ChatKind::Group, &[alice.user]);
    let out = alice.add(&mut ga, &[bob.key_package()]);
    let (mut gb, _) = bob.join(out.welcome.as_ref().unwrap());

    let conn = bob.store.lock().unwrap();
    let proposal = bob.mls.leave_proposal(&conn, &mut gb, &bob.signer).unwrap();
    drop(conn);
    match alice.recv(&mut ga, &proposal).unwrap() {
        Incoming::Proposal {
            self_remove,
            sender,
        } => {
            assert!(self_remove);
            assert_eq!(sender.user_id, bob.user);
        }
        other => panic!("{other:?}"),
    }
    let conn = alice.store.lock().unwrap();
    let commit = alice
        .mls
        .commit_pending_proposals(&conn, &mut ga, &alice.signer)
        .unwrap()
        .expect("a pending proposal to commit");
    alice.mls.merge_pending(&conn, &mut ga).unwrap();
    drop(conn);
    assert_eq!(members(&ga).len(), 1);
    match bob.recv(&mut gb, &commit.commit).unwrap() {
        Incoming::Commit { summary, .. } => assert!(summary.self_removed),
        other => panic!("{other:?}"),
    }
}

#[test]
fn late_messages_from_a_past_epoch_still_decrypt() {
    let alice = Client::new(1);
    let bob = Client::new(2);
    let mut ga = alice.create(&[6u8; 32], ChatKind::Direct, &[alice.user, bob.user]);
    let out = alice.add(&mut ga, &[bob.key_package()]);
    let (mut gb, _) = bob.join(out.welcome.as_ref().unwrap());

    let early = alice.send(&mut ga, b"sent just before the change");
    let conn = alice.store.lock().unwrap();
    let up = alice
        .mls
        .self_update(&conn, &mut ga, &alice.signer)
        .unwrap();
    alice.mls.merge_pending(&conn, &mut ga).unwrap();
    drop(conn);
    // Bob applies the commit first, then the older message arrives.
    assert!(matches!(
        bob.recv(&mut gb, &up.commit).unwrap(),
        Incoming::Commit { .. }
    ));
    assert_eq!(
        app_text(bob.recv(&mut gb, &early).unwrap()),
        b"sent just before the change"
    );
}

#[test]
fn group_state_survives_reloading_from_storage() {
    let alice = Client::new(1);
    let bob = Client::new(2);
    let gid = [5u8; 32];
    let mut ga = alice.create(&gid, ChatKind::Direct, &[alice.user, bob.user]);
    let out = alice.add(&mut ga, &[bob.key_package()]);
    let (gb, _) = bob.join(out.welcome.as_ref().unwrap());
    drop(gb);
    let ct = alice.send(&mut ga, b"persisted");
    let mut gb = {
        let conn = bob.store.lock().unwrap();
        bob.mls.load(&conn, &gid).unwrap()
    };
    assert_eq!(app_text(bob.recv(&mut gb, &ct).unwrap()), b"persisted");
    let signer = {
        let conn = bob.store.lock().unwrap();
        bob.mls.load_signer(&conn, bob.signer.public()).unwrap()
    };
    assert_eq!(signer.public(), bob.signer.public());
}
