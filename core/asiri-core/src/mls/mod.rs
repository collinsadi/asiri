//! The OpenMLS wrapper (architecture §7.3–§7.5, docs/adr/0002-openmls-libcrux-xwing.md).
//!
//! * Every chat is an MLS group using `MLS_256_XWING_CHACHA20POLY1305_SHA256_Ed25519` (hybrid
//!   X25519 + ML-KEM-768). There is no fallback suite: negotiation failures are hard errors.
//! * Handshake and application messages are always `PrivateMessage`s, padded to 256 bytes.
//! * The ratchet tree travels inside Welcome messages; the relay never stores it.
//! * Two private group-context extensions, readable only by members:
//!   `asiri.group` (the relay route and chat kind) and `asiri.admins` (who may add or remove
//!   others). Every client checks every commit against the admin list (§7.5).
//! * Credentials are Basic credentials carrying only random IDs (`CredentialIdentity`).
//!
//! All functions here are synchronous and run while the caller holds the store's connection, so
//! MLS state changes and app-data changes land in the same SQLCipher transaction.

mod policy;

use asiri_proto::Message as _;
use asiri_proto::app::v1::{AdminList, CredentialIdentity};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_libcrux_crypto::CryptoProvider;
use openmls_sqlite_storage::{Codec, SqliteStorageProvider};
use rusqlite::Connection;
use zeroize::Zeroizing;

pub use openmls::prelude::{GroupId, MlsGroup};
pub use policy::PolicyViolation;

use crate::error::{CoreError, CoreResult};
use crate::ids::{Id16, id16};
use crate::store::ChatKind;

/// The only cipher suite Asiri uses (architecture D4).
pub const SUITE: Ciphersuite = Ciphersuite::MLS_256_XWING_CHACHA20POLY1305_SHA256_Ed25519;
/// Private-use extension: the admin list (`AdminList`).
pub const EXT_ADMINS: u16 = 0xff0a;
/// Private-use extension: the chat's route and kind.
pub const EXT_GROUP: u16 = 0xff0b;
/// Past epochs kept so late messages still decrypt (§7.3).
pub const MAX_PAST_EPOCHS: usize = 3;
/// Ciphertext padding block (§7.3).
pub const PADDING: usize = 256;
/// Key-package lifetime (§7.3).
pub const KEY_PACKAGE_LIFETIME_S: u64 = 30 * 86_400;
/// Label for call keys (§7.8).
pub const CALL_EXPORTER_LABEL: &str = "asiri/call/v1";

fn mls_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Crypto(e.to_string())
}

/// JSON codec for the OpenMLS SQLite storage provider.
#[derive(Default)]
pub struct JsonCodec;

impl Codec for JsonCodec {
    type Error = serde_json::Error;

    fn to_vec<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(value)
    }

    fn from_slice<T: serde::de::DeserializeOwned>(slice: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(slice)
    }
}

/// Creates OpenMLS's tables in the encrypted database.
pub fn run_storage_migrations(conn: &mut Connection) -> CoreResult<()> {
    let mut storage = SqliteStorageProvider::<JsonCodec, &mut Connection>::new(conn);
    storage
        .run_migrations()
        .map_err(|e| CoreError::Storage(format!("MLS storage migration failed: {e}")))
}

/// The provider for one unit of work: libcrux crypto + storage on the borrowed connection.
pub struct Provider<'a> {
    crypto: &'a CryptoProvider,
    storage: SqliteStorageProvider<JsonCodec, &'a Connection>,
}

impl<'a> OpenMlsProvider for Provider<'a> {
    type CryptoProvider = CryptoProvider;
    type RandProvider = CryptoProvider;
    type StorageProvider = SqliteStorageProvider<JsonCodec, &'a Connection>;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        self.crypto
    }
}

/// The chat metadata carried in the `asiri.group` extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupMeta {
    /// The relay route.
    pub route: Id16,
    /// DM or group.
    pub kind: ChatKind,
}

impl GroupMeta {
    fn encode(self) -> Vec<u8> {
        let mut v = Vec::with_capacity(18);
        v.push(1); // version
        v.push(self.kind as u8);
        v.extend_from_slice(&self.route);
        v
    }

    fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != 18 || b[0] != 1 {
            return None;
        }
        Some(Self {
            kind: if b[1] == ChatKind::Group as u8 {
                ChatKind::Group
            } else {
                ChatKind::Direct
            },
            route: id16(&b[2..])?,
        })
    }
}

/// A group member as Asiri understands it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberInfo {
    /// The person.
    pub user_id: Id16,
    /// Their device.
    pub device_id: Id16,
    /// Where to deliver to that device.
    pub inbox_id: Id16,
    /// The device's MLS signature public key.
    pub signature_key: Vec<u8>,
    /// Leaf index in the ratchet tree.
    pub leaf: u32,
}

/// Encodes the identity carried in a credential.
#[must_use]
pub fn encode_identity(user_id: &Id16, device_id: &Id16, inbox_id: &Id16) -> Vec<u8> {
    CredentialIdentity {
        version: 1,
        user_id: user_id.to_vec(),
        device_id: device_id.to_vec(),
        inbox_id: inbox_id.to_vec(),
    }
    .encode_to_vec()
}

/// Parses the identity from a credential; rejects anything that is not a well-formed Basic
/// credential with three 16-byte IDs.
pub fn parse_identity(credential: &Credential) -> CoreResult<(Id16, Id16, Id16)> {
    let basic = BasicCredential::try_from(credential.clone()).map_err(mls_err)?;
    let id = CredentialIdentity::decode(basic.identity())?;
    if id.version != 1 {
        return Err(CoreError::Crypto("unsupported credential version".into()));
    }
    match (id16(&id.user_id), id16(&id.device_id), id16(&id.inbox_id)) {
        (Some(u), Some(d), Some(i)) => Ok((u, d, i)),
        _ => Err(CoreError::Crypto("malformed credential".into())),
    }
}

fn member_info(credential: &Credential, signature_key: &[u8], leaf: u32) -> CoreResult<MemberInfo> {
    let (user_id, device_id, inbox_id) = parse_identity(credential)?;
    Ok(MemberInfo {
        user_id,
        device_id,
        inbox_id,
        signature_key: signature_key.to_vec(),
        leaf,
    })
}

/// What a processed incoming MLS message turned out to be.
#[derive(Debug)]
pub enum Incoming {
    /// An application message.
    Application {
        /// Who sent it (authenticated by MLS).
        sender: MemberInfo,
        /// The decrypted, still-padded `AppMessage` bytes.
        plaintext: Vec<u8>,
    },
    /// A commit that has been validated against the admin policy and merged.
    Commit {
        /// Who committed.
        sender: MemberInfo,
        /// What changed.
        summary: CommitSummary,
    },
    /// A message this device sent, echoed back by the relay; ignored.
    Own,
    /// A proposal stored for the next commit (a member leaving).
    Proposal {
        /// Who proposed.
        sender: MemberInfo,
        /// Whether it is the sender removing themselves.
        self_remove: bool,
    },
}

/// The effect of a merged commit.
#[derive(Debug, Default)]
pub struct CommitSummary {
    /// Members added.
    pub added: Vec<MemberInfo>,
    /// Members removed.
    pub removed: Vec<MemberInfo>,
    /// The local device was removed.
    pub self_removed: bool,
    /// The admin list or group metadata changed.
    pub extensions_changed: bool,
    /// The epoch after merging.
    pub epoch: u64,
}

/// An outgoing commit and the Welcome for any added members.
pub struct CommitOut {
    /// Serialized commit (`PrivateMessage`).
    pub commit: Vec<u8>,
    /// Serialized Welcome, when members were added.
    pub welcome: Option<Vec<u8>>,
    /// The epoch the commit was created from (what the relay orders on).
    pub from_epoch: u64,
}

/// Owns the crypto provider; everything else borrows the store's connection.
pub struct Mls {
    crypto: CryptoProvider,
}

impl Mls {
    /// Creates the engine and checks that the post-quantum suite is available.
    pub fn new() -> CoreResult<Self> {
        let crypto = CryptoProvider::new().map_err(mls_err)?;
        crypto.supports(SUITE).map_err(|_| {
            CoreError::Crypto("the post-quantum cipher suite is unavailable".into())
        })?;
        Ok(Self { crypto })
    }

    /// A provider bound to `conn` for one unit of work.
    #[must_use]
    pub fn provider<'a>(&'a self, conn: &'a Connection) -> Provider<'a> {
        Provider {
            crypto: &self.crypto,
            storage: SqliteStorageProvider::new(conn),
        }
    }

    /// The crypto provider (exporter, hashing).
    #[must_use]
    pub fn crypto(&self) -> &CryptoProvider {
        &self.crypto
    }

    /// Generates and stores a new Ed25519 signature key pair.
    pub fn new_signer(&self, conn: &Connection) -> CoreResult<SignatureKeyPair> {
        let signer = SignatureKeyPair::new(SignatureScheme::ED25519).map_err(mls_err)?;
        signer
            .store(self.provider(conn).storage())
            .map_err(mls_err)?;
        Ok(signer)
    }

    /// Loads a stored signature key pair by its public key.
    pub fn load_signer(
        &self,
        conn: &Connection,
        public_key: &[u8],
    ) -> CoreResult<SignatureKeyPair> {
        SignatureKeyPair::read(
            self.provider(conn).storage(),
            public_key,
            SignatureScheme::ED25519,
        )
        .ok_or_else(|| CoreError::Storage("signature key is missing".into()))
    }

    /// The device's credential.
    #[must_use]
    pub fn credential(identity: Vec<u8>, signer: &SignatureKeyPair) -> CredentialWithKey {
        CredentialWithKey {
            credential: BasicCredential::new(identity).into(),
            signature_key: signer.public().into(),
        }
    }

    fn capabilities() -> Capabilities {
        Capabilities::builder()
            .ciphersuites(vec![SUITE])
            .extensions(vec![
                ExtensionType::Unknown(EXT_ADMINS),
                ExtensionType::Unknown(EXT_GROUP),
            ])
            .proposals(vec![])
            .credentials(vec![CredentialType::Basic])
            .build()
    }

    /// Creates a key package (one-time, or last-resort) and returns it serialized.
    pub fn key_package(
        &self,
        conn: &Connection,
        signer: &SignatureKeyPair,
        credential: CredentialWithKey,
        last_resort: bool,
    ) -> CoreResult<Vec<u8>> {
        let mut builder = KeyPackage::builder()
            .key_package_lifetime(Lifetime::new(KEY_PACKAGE_LIFETIME_S))
            .leaf_node_capabilities(Self::capabilities());
        if last_resort {
            builder = builder.mark_as_last_resort();
        }
        let bundle = builder
            .build(SUITE, &self.provider(conn), signer, credential)
            .map_err(mls_err)?;
        bundle
            .key_package()
            .tls_serialize_detached()
            .map_err(mls_err)
    }

    /// Parses and validates a key package fetched from the relay.
    pub fn validate_key_package(&self, bytes: &[u8]) -> CoreResult<(KeyPackage, MemberInfo)> {
        let kp_in = KeyPackageIn::tls_deserialize_exact(bytes).map_err(mls_err)?;
        let kp = kp_in
            .validate(&self.crypto, ProtocolVersion::Mls10)
            .map_err(mls_err)?;
        if kp.ciphersuite() != SUITE {
            return Err(CoreError::Crypto(
                "key package uses a different cipher suite".into(),
            ));
        }
        let leaf = kp.leaf_node();
        let info = member_info(leaf.credential(), leaf.signature_key().as_slice(), u32::MAX)?;
        Ok((kp, info))
    }

    fn create_config(meta: GroupMeta, admins: &[Id16]) -> CoreResult<MlsGroupCreateConfig> {
        let extensions = Extensions::<GroupContext>::from_vec(vec![
            Extension::Unknown(EXT_GROUP, UnknownExtension(meta.encode())),
            Extension::Unknown(EXT_ADMINS, UnknownExtension(admin_list_bytes(admins))),
            Extension::RequiredCapabilities(RequiredCapabilitiesExtension::new(
                &[
                    ExtensionType::Unknown(EXT_ADMINS),
                    ExtensionType::Unknown(EXT_GROUP),
                ],
                &[],
                &[CredentialType::Basic],
            )),
        ])
        .map_err(mls_err)?;
        Ok(MlsGroupCreateConfig::builder()
            .ciphersuite(SUITE)
            .use_ratchet_tree_extension(true)
            .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            .padding_size(PADDING)
            .max_past_epochs(MAX_PAST_EPOCHS)
            .capabilities(Self::capabilities())
            .with_group_context_extensions(extensions)
            .build())
    }

    fn join_config() -> MlsGroupJoinConfig {
        MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            .padding_size(PADDING)
            .max_past_epochs(MAX_PAST_EPOCHS)
            .build()
    }

    /// Creates a new group with the local device as its only member.
    pub fn create_group(
        &self,
        conn: &Connection,
        signer: &SignatureKeyPair,
        credential: CredentialWithKey,
        group_id: &[u8],
        meta: GroupMeta,
        admins: &[Id16],
    ) -> CoreResult<MlsGroup> {
        let config = Self::create_config(meta, admins)?;
        MlsGroup::new_with_group_id(
            &self.provider(conn),
            signer,
            &config,
            GroupId::from_slice(group_id),
            credential,
        )
        .map_err(mls_err)
    }

    /// Loads a group from storage.
    pub fn load(&self, conn: &Connection, group_id: &[u8]) -> CoreResult<MlsGroup> {
        MlsGroup::load(
            self.provider(conn).storage(),
            &GroupId::from_slice(group_id),
        )
        .map_err(mls_err)?
        .ok_or(CoreError::NotFound)
    }

    /// Adds members (one key package per device). The commit is pending until merged.
    pub fn add(
        &self,
        conn: &Connection,
        group: &mut MlsGroup,
        signer: &SignatureKeyPair,
        key_packages: &[KeyPackage],
    ) -> CoreResult<CommitOut> {
        let from_epoch = group.epoch().as_u64();
        let (commit, welcome, _info) = group
            .add_members(&self.provider(conn), signer, key_packages)
            .map_err(mls_err)?;
        Ok(CommitOut {
            commit: commit.tls_serialize_detached().map_err(mls_err)?,
            welcome: Some(welcome.tls_serialize_detached().map_err(mls_err)?),
            from_epoch,
        })
    }

    /// Removes leaves. The commit is pending until merged.
    pub fn remove(
        &self,
        conn: &Connection,
        group: &mut MlsGroup,
        signer: &SignatureKeyPair,
        leaves: &[u32],
    ) -> CoreResult<CommitOut> {
        let from_epoch = group.epoch().as_u64();
        let indices: Vec<LeafNodeIndex> = leaves.iter().map(|l| LeafNodeIndex::new(*l)).collect();
        let (commit, welcome, _info) = group
            .remove_members(&self.provider(conn), signer, &indices)
            .map_err(mls_err)?;
        Ok(CommitOut {
            commit: commit.tls_serialize_detached().map_err(mls_err)?,
            welcome: welcome
                .map(|w| w.tls_serialize_detached())
                .transpose()
                .map_err(mls_err)?,
            from_epoch,
        })
    }

    /// A self-update commit (post-compromise security, §7.3).
    pub fn self_update(
        &self,
        conn: &Connection,
        group: &mut MlsGroup,
        signer: &SignatureKeyPair,
    ) -> CoreResult<CommitOut> {
        let from_epoch = group.epoch().as_u64();
        let bundle = group
            .self_update(&self.provider(conn), signer, LeafNodeParameters::default())
            .map_err(mls_err)?;
        Ok(CommitOut {
            commit: bundle.commit().tls_serialize_detached().map_err(mls_err)?,
            welcome: None,
            from_epoch,
        })
    }

    /// Replaces the admin list (admins only; checked by every receiver).
    pub fn set_admins(
        &self,
        conn: &Connection,
        group: &mut MlsGroup,
        signer: &SignatureKeyPair,
        admins: &[Id16],
    ) -> CoreResult<CommitOut> {
        let from_epoch = group.epoch().as_u64();
        let mut exts: Vec<Extension> = group.extensions().iter().cloned().collect();
        exts.retain(|e| !matches!(e, Extension::Unknown(t, _) if *t == EXT_ADMINS));
        exts.push(Extension::Unknown(
            EXT_ADMINS,
            UnknownExtension(admin_list_bytes(admins)),
        ));
        let extensions = Extensions::<GroupContext>::from_vec(exts).map_err(mls_err)?;
        let (commit, _, _) = group
            .update_group_context_extensions(&self.provider(conn), extensions, signer)
            .map_err(mls_err)?;
        Ok(CommitOut {
            commit: commit.tls_serialize_detached().map_err(mls_err)?,
            welcome: None,
            from_epoch,
        })
    }

    /// A proposal removing the local device; the next admin online commits it (§7.5 "Leave").
    pub fn leave_proposal(
        &self,
        conn: &Connection,
        group: &mut MlsGroup,
        signer: &SignatureKeyPair,
    ) -> CoreResult<Vec<u8>> {
        let msg = group
            .leave_group(&self.provider(conn), signer)
            .map_err(mls_err)?;
        msg.tls_serialize_detached().map_err(mls_err)
    }

    /// Commits stored proposals (an admin committing someone's leave).
    pub fn commit_pending_proposals(
        &self,
        conn: &Connection,
        group: &mut MlsGroup,
        signer: &SignatureKeyPair,
    ) -> CoreResult<Option<CommitOut>> {
        if !group.has_pending_proposals() {
            return Ok(None);
        }
        let from_epoch = group.epoch().as_u64();
        let (commit, welcome, _) = group
            .commit_to_pending_proposals(&self.provider(conn), signer)
            .map_err(mls_err)?;
        Ok(Some(CommitOut {
            commit: commit.tls_serialize_detached().map_err(mls_err)?,
            welcome: welcome
                .map(|w| w.tls_serialize_detached())
                .transpose()
                .map_err(mls_err)?,
            from_epoch,
        }))
    }

    /// Applies our own pending commit after the relay accepted it.
    pub fn merge_pending(&self, conn: &Connection, group: &mut MlsGroup) -> CoreResult<()> {
        group
            .merge_pending_commit(&self.provider(conn))
            .map_err(mls_err)
    }

    /// Drops our own pending commit after the relay rejected it (another commit won the epoch).
    pub fn discard_pending(&self, conn: &Connection, group: &mut MlsGroup) -> CoreResult<()> {
        group
            .clear_pending_commit(self.provider(conn).storage())
            .map_err(mls_err)
    }

    /// Encrypts application content.
    pub fn encrypt(
        &self,
        conn: &Connection,
        group: &mut MlsGroup,
        signer: &SignatureKeyPair,
        plaintext: &[u8],
    ) -> CoreResult<Vec<u8>> {
        let msg = group
            .create_message(&self.provider(conn), signer, plaintext)
            .map_err(mls_err)?;
        msg.tls_serialize_detached().map_err(mls_err)
    }

    /// Joins a group from a Welcome. Returns the group and the member who added us.
    pub fn join(&self, conn: &Connection, welcome: &[u8]) -> CoreResult<(MlsGroup, MemberInfo)> {
        let msg = MlsMessageIn::tls_deserialize_exact(welcome).map_err(mls_err)?;
        let MlsMessageBodyIn::Welcome(welcome) = msg.extract() else {
            return Err(CoreError::Crypto("expected a Welcome".into()));
        };
        if welcome.ciphersuite() != SUITE {
            return Err(CoreError::Crypto(
                "group uses a different cipher suite".into(),
            ));
        }
        let provider = self.provider(conn);
        let staged =
            StagedWelcome::new_from_welcome(&provider, &Self::join_config(), welcome, None)
                .map_err(mls_err)?;
        let inviter = staged.welcome_sender().map_err(mls_err)?;
        let inviter = member_info(
            inviter.credential(),
            inviter.signature_key().as_slice(),
            staged.welcome_sender_index().u32(),
        )?;
        let group = staged.into_group(&provider).map_err(mls_err)?;
        if group_meta(&group).is_none() {
            return Err(CoreError::Crypto(
                "group is missing its Asiri metadata".into(),
            ));
        }
        Ok((group, inviter))
    }

    /// Decrypts and handles one incoming handshake or application message.
    pub fn process(
        &self,
        conn: &Connection,
        group: &mut MlsGroup,
        message: ProtocolMessage,
    ) -> CoreResult<Incoming> {
        let provider = self.provider(conn);
        let processed = group.process_message(&provider, message).map_err(mls_err)?;
        let sender = match processed.sender() {
            Sender::Member(leaf) => {
                let leaf = *leaf;
                let member = group
                    .member_at(leaf)
                    .ok_or_else(|| CoreError::Crypto("unknown sender".into()))?;
                member_info(&member.credential, &member.signature_key, leaf.u32())?
            }
            _ => return Err(CoreError::Crypto("messages must come from members".into())),
        };
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(app) => Ok(Incoming::Application {
                sender,
                plaintext: app.into_bytes(),
            }),
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                let summary = policy::check_commit(group, &sender, &staged)?;
                if group.pending_commit().is_some() {
                    group
                        .clear_pending_commit(provider.storage())
                        .map_err(mls_err)?;
                }
                group
                    .merge_staged_commit(&provider, *staged)
                    .map_err(mls_err)?;
                let summary = CommitSummary {
                    epoch: group.epoch().as_u64(),
                    ..summary
                };
                Ok(Incoming::Commit { sender, summary })
            }
            ProcessedMessageContent::ProposalMessage(proposal) => {
                let self_remove = policy::check_proposal(&sender, &proposal)?;
                group
                    .store_pending_proposal(provider.storage(), *proposal)
                    .map_err(mls_err)?;
                Ok(Incoming::Proposal {
                    sender,
                    self_remove,
                })
            }
            ProcessedMessageContent::ExternalJoinProposalMessage(_) => {
                Err(CoreError::Crypto("external joins are not allowed".into()))
            }
            // Our own messages are never fanned back to us; if a relay does it anyway, skip.
            ProcessedMessageContent::OwnPendingCommit
            | ProcessedMessageContent::OwnPrivateMessage => Ok(Incoming::Own),
        }
    }

    /// Derives the 32-byte call key for `call_id` from the current epoch (§7.8).
    pub fn call_secret(&self, group: &MlsGroup, call_id: &[u8]) -> CoreResult<Zeroizing<Vec<u8>>> {
        group
            .export_secret(&self.crypto, CALL_EXPORTER_LABEL, call_id, 32)
            .map(Zeroizing::new)
            .map_err(mls_err)
    }
}

/// Parses wire bytes into a protocol message, returning the group ID it addresses.
pub fn parse_protocol_message(bytes: &[u8]) -> CoreResult<(Vec<u8>, ProtocolMessage)> {
    let msg = MlsMessageIn::tls_deserialize_exact(bytes).map_err(mls_err)?;
    let protocol = msg.try_into_protocol_message().map_err(mls_err)?;
    if !matches!(protocol, ProtocolMessage::PrivateMessage(_)) {
        // §7.3: everything is PrivateMessage; plaintext handshakes are refused outright.
        return Err(CoreError::Crypto(
            "plaintext MLS messages are not accepted".into(),
        ));
    }
    Ok((protocol.group_id().as_slice().to_vec(), protocol))
}

fn admin_list_bytes(admins: &[Id16]) -> Vec<u8> {
    AdminList {
        user_ids: admins.iter().map(|a| a.to_vec()).collect(),
    }
    .encode_to_vec()
}

/// The group's admin list.
#[must_use]
pub fn admins(group: &MlsGroup) -> Vec<Id16> {
    group
        .extensions()
        .unknown(EXT_ADMINS)
        .and_then(|e| AdminList::decode(e.0.as_slice()).ok())
        .map(|l| l.user_ids.iter().filter_map(|u| id16(u)).collect())
        .unwrap_or_default()
}

/// The chat metadata stored in the group context.
#[must_use]
pub fn group_meta(group: &MlsGroup) -> Option<GroupMeta> {
    group
        .extensions()
        .unknown(EXT_GROUP)
        .and_then(|e| GroupMeta::decode(&e.0))
}

/// Every current member, with parsed identities. Members whose credentials don't parse are
/// skipped (they cannot be addressed).
#[must_use]
pub fn members(group: &MlsGroup) -> Vec<MemberInfo> {
    group
        .members()
        .filter_map(|m| member_info(&m.credential, &m.signature_key, m.index.u32()).ok())
        .collect()
}

/// The local device's own leaf index.
#[must_use]
pub fn own_leaf(group: &MlsGroup) -> u32 {
    group.own_leaf_index().u32()
}

/// The current epoch.
#[must_use]
pub fn epoch(group: &MlsGroup) -> u64 {
    group.epoch().as_u64()
}

/// Whether the local device is still a member.
#[must_use]
pub fn is_active(group: &MlsGroup) -> bool {
    group.is_active()
}

/// The suite code point (recorded per chat for future migrations).
#[must_use]
pub fn suite_code() -> u16 {
    u16::from(SUITE)
}

#[cfg(test)]
mod tests;
