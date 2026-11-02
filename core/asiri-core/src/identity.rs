//! Device identity (architecture §7.2): keys are born on the device and never leave it
//! unencrypted. All of them live in the SQLCipher database.
//!
//! | Key                    | Algorithm | Purpose                                              |
//! |------------------------|-----------|------------------------------------------------------|
//! | MLS signature key      | Ed25519   | signs MLS messages; its fingerprint identifies us    |
//! | relay auth key         | Ed25519   | proves to the relay that this device is registered   |
//! | onion client key       | X25519    | lets the device discover the onion service at all    |

use asiri_proto::pairing::v1::{self as pairing, relay_endpoint::Transport};
use ed25519_dalek::{Signer as _, SigningKey};
use openmls_basic_credential::SignatureKeyPair;
use prost::Message as _;
use rusqlite::Connection;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::crypto::sas;
use crate::error::{CoreError, CoreResult};
use crate::ids::{Id16, id16, random_bytes, random_id};
use crate::mls::{self, Mls};
use crate::store;

const K_USER: &str = "identity.user_id";
const K_SIG: &str = "identity.signature_public";
const K_AUTH: &str = "identity.relay_auth_secret";
const K_ONION: &str = "identity.onion_client_secret";
const K_REG: &str = "identity.registration";
const K_NAME: &str = "profile.display_name";
const K_AVATAR: &str = "profile.avatar";

/// How to reach the relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayEndpoint {
    /// Onion or direct.
    pub transport: Transport,
    /// `<56 chars>.onion` or `host:port`; also the service address signed during auth.
    pub address: String,
    /// Call-signaling address for direct relays (`host:port`); empty means "relay host:7880".
    pub call_address: String,
}

impl RelayEndpoint {
    /// From the pairing representation.
    pub fn from_wire(e: &pairing::RelayEndpoint) -> CoreResult<Self> {
        let transport = Transport::try_from(e.transport)
            .ok()
            .filter(|t| *t != Transport::Unspecified)
            .ok_or_else(|| {
                CoreError::Pairing("This code doesn't say how to reach the server".into())
            })?;
        if e.address.is_empty() || e.address.len() > 255 {
            return Err(CoreError::Pairing(
                "This code has an invalid server address".into(),
            ));
        }
        if transport == Transport::Onion && !e.address.ends_with(".onion") {
            return Err(CoreError::Pairing(
                "This code has an invalid onion address".into(),
            ));
        }
        Ok(Self {
            transport,
            address: e.address.clone(),
            call_address: e.call_address.clone(),
        })
    }

    /// To the pairing representation.
    #[must_use]
    pub fn to_wire(&self) -> pairing::RelayEndpoint {
        pairing::RelayEndpoint {
            transport: self.transport as i32,
            address: self.address.clone(),
            call_address: self.call_address.clone(),
        }
    }

    /// Whether this endpoint bypasses Tor (development only).
    #[must_use]
    pub fn is_direct(&self) -> bool {
        self.transport == Transport::Direct
    }
}

/// The relay registration of this device.
#[derive(Clone, Debug)]
pub struct Registration {
    /// Relay-assigned device ID.
    pub device_id: Id16,
    /// Relay-assigned inbox ID.
    pub inbox_id: Id16,
    /// The relay.
    pub endpoint: RelayEndpoint,
    /// Whether this device administers the relay.
    pub is_admin: bool,
}

impl Registration {
    fn encode(&self) -> Vec<u8> {
        let wire = pairing::Invitation {
            relay: Some(self.endpoint.to_wire()),
            registration_secret: vec![u8::from(self.is_admin)],
            device_id: self.device_id.to_vec(),
            inbox_id: self.inbox_id.to_vec(),
            inviter: None,
        };
        wire.encode_to_vec()
    }

    fn decode(bytes: &[u8]) -> CoreResult<Self> {
        let w = pairing::Invitation::decode(bytes)?;
        Ok(Self {
            device_id: id16(&w.device_id)
                .ok_or_else(|| CoreError::Storage("registration".into()))?,
            inbox_id: id16(&w.inbox_id).ok_or_else(|| CoreError::Storage("registration".into()))?,
            endpoint: RelayEndpoint::from_wire(
                w.relay
                    .as_ref()
                    .ok_or_else(|| CoreError::Storage("registration".into()))?,
            )?,
            is_admin: w.registration_secret.first() == Some(&1),
        })
    }
}

/// This device's keys and registration.
pub struct Identity {
    /// Random user ID (stable across a person's devices).
    pub user_id: Id16,
    /// MLS signature key pair.
    pub signer: SignatureKeyPair,
    /// Relay auth key.
    pub relay_auth: SigningKey,
    /// Onion restricted-discovery key.
    pub onion_client: StaticSecret,
    /// Relay registration, once paired.
    pub registration: Option<Registration>,
    /// Name shared with friends.
    pub display_name: String,
    /// Photo shared with friends (JPEG).
    pub avatar: Option<Vec<u8>>,
}

impl Identity {
    /// Loads the identity, creating fresh keys on first launch (§11.1 step 4).
    pub fn load_or_create(conn: &Connection, mls: &Mls) -> CoreResult<Self> {
        let user_id = match store::kv_get(conn, K_USER)? {
            Some(v) => id16(&v).ok_or_else(|| CoreError::Storage("user id".into()))?,
            None => {
                let id = random_id();
                store::kv_set(conn, K_USER, &id)?;
                id
            }
        };
        let signer = match store::kv_get(conn, K_SIG)? {
            Some(pk) => mls.load_signer(conn, &pk)?,
            None => {
                let s = mls.new_signer(conn)?;
                store::kv_set(conn, K_SIG, s.public())?;
                s
            }
        };
        let relay_auth = match store::kv_get(conn, K_AUTH)? {
            Some(sk) => {
                let bytes: Zeroizing<[u8; 32]> = Zeroizing::new(
                    sk.as_slice()
                        .try_into()
                        .map_err(|_| CoreError::Storage("auth key".into()))?,
                );
                SigningKey::from_bytes(&bytes)
            }
            None => {
                let seed = Zeroizing::new(random_bytes::<32>());
                store::kv_set(conn, K_AUTH, seed.as_ref())?;
                SigningKey::from_bytes(&seed)
            }
        };
        let onion_client = match store::kv_get(conn, K_ONION)? {
            Some(sk) => {
                let bytes: [u8; 32] = sk
                    .as_slice()
                    .try_into()
                    .map_err(|_| CoreError::Storage("onion key".into()))?;
                StaticSecret::from(bytes)
            }
            None => {
                let seed = Zeroizing::new(random_bytes::<32>());
                store::kv_set(conn, K_ONION, seed.as_ref())?;
                StaticSecret::from(*seed)
            }
        };
        let registration = store::kv_get(conn, K_REG)?
            .map(|b| Registration::decode(&b))
            .transpose()?;
        let display_name = store::kv_get(conn, K_NAME)?
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let avatar = store::kv_get(conn, K_AVATAR)?;
        Ok(Self {
            user_id,
            signer,
            relay_auth,
            onion_client,
            registration,
            display_name,
            avatar,
        })
    }

    /// Saves the relay registration.
    pub fn set_registration(&mut self, conn: &Connection, reg: Registration) -> CoreResult<()> {
        store::kv_set(conn, K_REG, &reg.encode())?;
        self.registration = Some(reg);
        Ok(())
    }

    /// Saves the profile.
    pub fn set_profile(
        &mut self,
        conn: &Connection,
        name: &str,
        avatar: Option<Vec<u8>>,
    ) -> CoreResult<()> {
        store::kv_set(conn, K_NAME, name.as_bytes())?;
        match &avatar {
            Some(a) => store::kv_set(conn, K_AVATAR, a)?,
            None => store::kv_delete(conn, K_AVATAR)?,
        }
        self.display_name = name.to_owned();
        self.avatar = avatar;
        Ok(())
    }

    /// The registration, or [`CoreError::NotRegistered`].
    pub fn registered(&self) -> CoreResult<&Registration> {
        self.registration.as_ref().ok_or(CoreError::NotRegistered)
    }

    /// The MLS credential for this device (requires registration: it embeds the device and
    /// inbox IDs).
    pub fn credential(&self) -> CoreResult<openmls::prelude::CredentialWithKey> {
        let reg = self.registered()?;
        Ok(Mls::credential(
            mls::encode_identity(&self.user_id, &reg.device_id, &reg.inbox_id),
            &self.signer,
        ))
    }

    /// The fingerprint of our MLS signature key.
    #[must_use]
    pub fn fingerprint(&self) -> [u8; 32] {
        sas::fingerprint(self.signer.public())
    }

    /// Relay auth public key.
    #[must_use]
    pub fn auth_public(&self) -> [u8; 32] {
        self.relay_auth.verifying_key().to_bytes()
    }

    /// Signs a relay challenge or registration transcript.
    #[must_use]
    pub fn sign_relay(&self, msg: &[u8]) -> Vec<u8> {
        self.relay_auth.sign(msg).to_bytes().to_vec()
    }

    /// Onion client public key.
    #[must_use]
    pub fn onion_public(&self) -> [u8; 32] {
        PublicKey::from(&self.onion_client).to_bytes()
    }

    /// The onion client key in tor's `authorized_clients` format, as shown on the
    /// "Set up a server" screen and passed to `asiri-relay admin bootstrap`.
    #[must_use]
    pub fn onion_public_line(&self) -> String {
        format!(
            "descriptor:x25519:{}",
            data_encoding::BASE32_NOPAD.encode(&self.onion_public())
        )
    }

    /// Our contact card (shared in person).
    pub fn contact_card(&self) -> CoreResult<pairing::ContactCard> {
        let reg = self.registered()?;
        Ok(pairing::ContactCard {
            user_id: self.user_id.to_vec(),
            device_id: reg.device_id.to_vec(),
            inbox_id: reg.inbox_id.to_vec(),
            signature_key: self.signer.public().to_vec(),
            display_name: self.display_name.clone(),
        })
    }

    /// QR-A: what a new device shows so an existing member can invite it (§11.2 step 1).
    #[must_use]
    pub fn device_offer(&self) -> pairing::DeviceOffer {
        pairing::DeviceOffer {
            user_id: self.user_id.to_vec(),
            onion_client_key: self.onion_public().to_vec(),
            auth_pubkey: self.auth_public().to_vec(),
            signature_key: self.signer.public().to_vec(),
            display_name: self.display_name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn identity_persists_and_registration_round_trips() {
        let store = Store::open_in_memory().unwrap();
        let mls = Mls::new().unwrap();
        let conn = store.lock().unwrap();
        let mut a = Identity::load_or_create(&conn, &mls).unwrap();
        assert!(a.registration.is_none());
        assert!(a.credential().is_err(), "credentials need a registration");
        a.set_registration(
            &conn,
            Registration {
                device_id: [1; 16],
                inbox_id: [2; 16],
                endpoint: RelayEndpoint {
                    transport: Transport::Onion,
                    address: format!("{}.onion", "a".repeat(56)),
                    call_address: String::new(),
                },
                is_admin: true,
            },
        )
        .unwrap();
        a.set_profile(&conn, "Ada", None).unwrap();
        let b = Identity::load_or_create(&conn, &mls).unwrap();
        assert_eq!(a.user_id, b.user_id);
        assert_eq!(a.signer.public(), b.signer.public());
        assert_eq!(a.auth_public(), b.auth_public());
        assert_eq!(a.onion_public(), b.onion_public());
        assert_eq!(b.display_name, "Ada");
        let reg = b.registration.unwrap();
        assert!(reg.is_admin);
        assert_eq!(reg.inbox_id, [2; 16]);
        assert!(a.onion_public_line().starts_with("descriptor:x25519:"));
        assert_eq!(a.onion_public_line().len(), "descriptor:x25519:".len() + 52);
    }

    #[test]
    fn endpoints_are_validated() {
        let ok = pairing::RelayEndpoint {
            transport: Transport::Direct as i32,
            address: "127.0.0.1:8080".into(),
            call_address: String::new(),
        };
        assert!(RelayEndpoint::from_wire(&ok).unwrap().is_direct());
        let bad_onion = pairing::RelayEndpoint {
            transport: Transport::Onion as i32,
            address: "example.com".into(),
            call_address: String::new(),
        };
        assert!(RelayEndpoint::from_wire(&bad_onion).is_err());
    }
}
