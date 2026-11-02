//! In-person pairing (architecture §9.4, §11.2) and verification (§7.6).
//!
//! New device (Ada)                      Inviter (Collins)
//! ──────────────────                    ─────────────────
//! shows QR-A  (`device_offer_code`) ──▶ scans it (`invite_device`): AuthorizeDevice on the relay
//!                                       shows QR-B (onion address, one-time secret, own card)
//! scans QR-B (`redeem_invitation`)  ◀──
//! registers, uploads key packages, fetches Collins's key package (checked against QR-B),
//! creates the DM ─────── Welcome ─────▶ recognises the device it scanned
//! Both screens show the same six emoji; both people confirm (`confirm_verification`).

use std::time::Duration;

use asiri_proto::Message as _;
use asiri_proto::pairing::v1::{self as pairing, pairing_payload::Body as Code};
use asiri_proto::pairing_text;
use asiri_proto::relay::v1::{self as wire, client_frame::Body as C, server_frame::Body as S};

use super::{ConnectionState, Core, CoreEvent, PairingStep, lock_err};
use crate::crypto::sas;
use crate::error::{CoreError, CoreResult};
use crate::identity::{Registration, RelayEndpoint};
use crate::ids::{Id16, id16};
use crate::net::http;
use crate::store::{self, ChatKind, ContactDevice, Verification};

/// What a scanned or pasted code is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeKind {
    /// QR-A: a new device asking to be invited.
    DeviceOffer,
    /// QR-B or the admin bootstrap code: an invitation to register.
    Invitation,
    /// An existing user's contact card.
    Contact,
}

fn wrap(body: Code) -> String {
    pairing_text::encode(&pairing::PairingPayload {
        version: 1,
        body: Some(body),
    })
}

fn parse(text: &str) -> CoreResult<Code> {
    pairing_text::decode(text)
        .map_err(|e| CoreError::Pairing(e.to_string()))?
        .body
        .ok_or_else(|| CoreError::Pairing("This code is empty".into()))
}

fn need16(b: &[u8], what: &str) -> CoreResult<Id16> {
    id16(b).ok_or_else(|| CoreError::Pairing(format!("This code has an invalid {what}")))
}

fn need32(b: &[u8], what: &str) -> CoreResult<[u8; 32]> {
    b.try_into()
        .map_err(|_| CoreError::Pairing(format!("This code has an invalid {what}")))
}

fn clean_name(n: &str) -> String {
    n.trim().chars().take(64).collect()
}

impl Core {
    /// Identifies a scanned or pasted code without acting on it.
    pub fn classify_code(&self, text: &str) -> CoreResult<(CodeKind, String)> {
        Ok(match parse(text)? {
            Code::Offer(o) => (CodeKind::DeviceOffer, clean_name(&o.display_name)),
            Code::Invitation(i) => (
                CodeKind::Invitation,
                i.inviter
                    .map(|c| clean_name(&c.display_name))
                    .unwrap_or_default(),
            ),
            Code::Contact(c) => (CodeKind::Contact, clean_name(&c.display_name)),
        })
    }

    /// QR-A for this device (shown by a Mac that wants to join).
    pub fn device_offer_code(&self) -> CoreResult<String> {
        let id = self.inner.identity.read().map_err(lock_err)?;
        Ok(wrap(Code::Offer(id.device_offer())))
    }

    /// This device's contact card, for friends who already use Asiri (§11.2 last bullet).
    pub fn contact_code(&self) -> CoreResult<String> {
        let id = self.inner.identity.read().map_err(lock_err)?;
        Ok(wrap(Code::Contact(id.contact_card()?)))
    }

    /// Inviter side: authorizes the scanned device on the relay and returns QR-B.
    pub async fn invite_device(&self, offer_text: &str) -> CoreResult<PairingStep> {
        let Code::Offer(offer) = parse(offer_text)? else {
            return Err(CoreError::Pairing("That's not a new device's code".into()));
        };
        let user_id = need16(&offer.user_id, "user")?;
        let onion = need32(&offer.onion_client_key, "onion key")?;
        let auth = need32(&offer.auth_pubkey, "auth key")?;
        let sig = need32(&offer.signature_key, "signature key")?;
        let name = clean_name(&offer.display_name);
        if user_id == self.user_id()? {
            return Err(CoreError::Pairing("That's this Mac's own code".into()));
        }
        let client = self.client()?;
        let authorized = match client
            .request(C::AuthorizeDevice(wire::AuthorizeDevice {
                onion_client_key: onion.to_vec(),
                auth_pubkey: auth.to_vec(),
            }))
            .await?
        {
            S::DeviceAuthorized(d) => d,
            _ => return Err(CoreError::Network("unexpected reply".into())),
        };
        let device_id = need16(&authorized.device_id, "device")?;
        let inbox_id = need16(&authorized.inbox_id, "inbox")?;
        let (invitation, my_fp) = {
            let id = self.inner.identity.read().map_err(lock_err)?;
            let reg = id.registered()?;
            self.inner.store.tx(|c| {
                store::upsert_contact(c, &user_id, Some(&name))?;
                store::upsert_device(
                    c,
                    &ContactDevice {
                        device_id,
                        user_id,
                        inbox_id,
                        signature_key: sig.to_vec(),
                    },
                )?;
                store::add_pending_invite(c, &sig, &user_id, &name)
            })?;
            (
                pairing::Invitation {
                    relay: Some(reg.endpoint.to_wire()),
                    registration_secret: authorized.registration_secret,
                    device_id: device_id.to_vec(),
                    inbox_id: inbox_id.to_vec(),
                    inviter: Some(id.contact_card()?),
                },
                id.fingerprint(),
            )
        };
        self.emit(CoreEvent::ContactsChanged);
        Ok(PairingStep {
            emoji: sas::pairing_emoji(&my_fp, &sas::fingerprint(&sig)),
            peer_name: name,
            peer_user_id: user_id,
            reply_code: Some(wrap(Code::Invitation(invitation))),
            chat_id: None,
        })
    }

    /// New-device side: registers with the relay from QR-B (or the admin bootstrap code),
    /// connects, and — when the code came from a friend — creates the DM with them.
    pub async fn redeem_invitation(&self, text: &str) -> CoreResult<PairingStep> {
        let Code::Invitation(inv) = parse(text)? else {
            return Err(CoreError::Pairing("That's not an invitation".into()));
        };
        if self
            .inner
            .identity
            .read()
            .map_err(lock_err)?
            .registration
            .is_some()
        {
            return Err(CoreError::Pairing(
                "This Mac is already connected to a server".into(),
            ));
        }
        let endpoint = RelayEndpoint::from_wire(
            inv.relay
                .as_ref()
                .ok_or_else(|| CoreError::Pairing("This code has no server".into()))?,
        )?;
        let expected_device = need16(&inv.device_id, "device")?;
        let inviter = match &inv.inviter {
            Some(card) => Some((
                need16(&card.user_id, "user")?,
                need16(&card.device_id, "device")?,
                need16(&card.inbox_id, "inbox")?,
                need32(&card.signature_key, "signature key")?,
                clean_name(&card.display_name),
            )),
            None => None,
        };

        let dialer = self.dialer_for_endpoint(&endpoint).await?;
        let (auth_pub, signature) = {
            let id = self.inner.identity.read().map_err(lock_err)?;
            let pk = id.auth_public();
            (
                pk,
                id.sign_relay(&asiri_proto::register_transcript(
                    &inv.registration_secret,
                    &pk,
                )),
            )
        };
        let req = wire::RegisterRequest {
            registration_secret: inv.registration_secret.clone(),
            auth_pubkey: auth_pub.to_vec(),
            signature,
        };
        let resp = http::request(
            &dialer,
            "POST",
            "/v1/register",
            None,
            &[],
            req.encode_to_vec(),
        )
        .await?;
        if resp.status != 200 {
            return Err(match resp.status {
                401 => CoreError::Pairing(
                    "This code was already used or has expired. Ask for a new one.".into(),
                ),
                _ => http::error_from(&resp),
            });
        }
        let reg = wire::RegisterResponse::decode(resp.body.as_slice())?;
        let device_id = need16(&reg.device_id, "device")?;
        if device_id != expected_device {
            return Err(CoreError::Pairing(
                "The server answered for a different device".into(),
            ));
        }
        let inbox_id = need16(&reg.inbox_id, "inbox")?;
        {
            let mut id = self.inner.identity.write().map_err(lock_err)?;
            let registration = Registration {
                device_id,
                inbox_id,
                endpoint,
                is_admin: reg.is_admin,
            };
            self.inner
                .store
                .tx(|c| id.set_registration(c, registration))?;
        }
        *self.inner.dialer.lock().map_err(lock_err)? = Some(dialer);
        self.set_state(ConnectionState::Offline);
        self.connect()?;
        self.wait_until_connected(Duration::from_secs(90)).await?;
        self.replenish_key_packages().await?;

        let my_fp = self.inner.identity.read().map_err(lock_err)?.fingerprint();
        let Some((user_id, peer_device, peer_inbox, sig, name)) = inviter else {
            return Ok(PairingStep {
                emoji: vec![],
                peer_name: String::new(),
                peer_user_id: [0; 16],
                reply_code: None,
                chat_id: None,
            });
        };
        self.inner.store.tx(|c| {
            store::upsert_contact(c, &user_id, Some(&name))?;
            store::upsert_device(
                c,
                &ContactDevice {
                    device_id: peer_device,
                    user_id,
                    inbox_id: peer_inbox,
                    signature_key: sig.to_vec(),
                },
            )
        })?;
        self.emit(CoreEvent::ContactsChanged);
        let chat_id = self.create_chat(ChatKind::Direct, "", &[user_id]).await?;
        self.emit(CoreEvent::PairingCompleted {
            chat_id: chat_id.clone(),
            user_id,
        });
        Ok(PairingStep {
            emoji: sas::pairing_emoji(&my_fp, &sas::fingerprint(&sig)),
            peer_name: name,
            peer_user_id: user_id,
            reply_code: None,
            chat_id: Some(chat_id),
        })
    }

    /// Friends who both already use Asiri exchange contact cards. The device with the smaller
    /// device ID creates the DM; the other one receives its Welcome.
    pub async fn add_contact(&self, card_text: &str) -> CoreResult<PairingStep> {
        let Code::Contact(card) = parse(card_text)? else {
            return Err(CoreError::Pairing("That's not a contact code".into()));
        };
        let user_id = need16(&card.user_id, "user")?;
        let device_id = need16(&card.device_id, "device")?;
        let inbox_id = need16(&card.inbox_id, "inbox")?;
        let sig = need32(&card.signature_key, "signature key")?;
        let name = clean_name(&card.display_name);
        let (me, my_device) = self.me()?;
        if user_id == me {
            return Err(CoreError::Pairing("That's your own code".into()));
        }
        let my_fp = self.inner.identity.read().map_err(lock_err)?.fingerprint();
        self.inner.store.tx(|c| {
            store::upsert_contact(c, &user_id, Some(&name))?;
            store::upsert_device(
                c,
                &ContactDevice {
                    device_id,
                    user_id,
                    inbox_id,
                    signature_key: sig.to_vec(),
                },
            )?;
            store::add_pending_invite(c, &sig, &user_id, &name)
        })?;
        self.emit(CoreEvent::ContactsChanged);
        let chat_id = if my_device < device_id {
            Some(self.create_chat(ChatKind::Direct, "", &[user_id]).await?)
        } else {
            let conn = self.inner.store.lock()?;
            store::dm_with(&conn, &user_id)?.map(|c| c.chat_id)
        };
        Ok(PairingStep {
            emoji: sas::pairing_emoji(&my_fp, &sas::fingerprint(&sig)),
            peer_name: name,
            peer_user_id: user_id,
            reply_code: None,
            chat_id,
        })
    }

    /// Marks a contact verified after the emoji (in person) or safety number (remotely) matched.
    pub fn confirm_verification(&self, user_id: &Id16, in_person: bool) -> CoreResult<()> {
        let v = if in_person {
            Verification::InPerson
        } else {
            Verification::Remote
        };
        self.inner.store.tx(|c| {
            if store::contact(c, user_id)?.is_none() {
                return Err(CoreError::NotFound);
            }
            store::set_verification(c, user_id, v)
        })?;
        self.emit(CoreEvent::ContactsChanged);
        self.emit(CoreEvent::ChatsChanged);
        Ok(())
    }

    /// The six emoji for a contact we have scanned.
    pub fn pairing_emoji(&self, user_id: &Id16) -> CoreResult<Vec<sas::EmojiWord>> {
        let their = self.contact_fingerprint(user_id)?;
        let mine = self.inner.identity.read().map_err(lock_err)?.fingerprint();
        Ok(sas::pairing_emoji(&mine, &their))
    }

    /// The 60-digit safety number with a contact (§7.6 "Remote fallback").
    pub fn safety_number(&self, user_id: &Id16) -> CoreResult<String> {
        let their = self.contact_fingerprint(user_id)?;
        let mine = self.inner.identity.read().map_err(lock_err)?.fingerprint();
        Ok(sas::safety_number(&mine, &their))
    }

    fn contact_fingerprint(&self, user_id: &Id16) -> CoreResult<[u8; 32]> {
        let conn = self.inner.store.lock()?;
        let device = store::devices_of(&conn, user_id)?
            .into_iter()
            .next()
            .ok_or(CoreError::NotFound)?;
        Ok(sas::fingerprint(&device.signature_key))
    }

    /// Waits for the relay session (used right after registering).
    pub async fn wait_until_connected(&self, timeout: Duration) -> CoreResult<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.client().is_ok() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(CoreError::Offline);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
