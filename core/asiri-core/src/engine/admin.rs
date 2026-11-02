//! Server administration from the app (architecture §9.4, Settings → Server): the devices
//! registered on the relay and revoking a lost one. Only relay admins may use these.

use asiri_proto::relay::v1::{self as wire, client_frame::Body as C, server_frame::Body as S};

use super::{Core, ServerStatus};
use crate::error::{CoreError, CoreResult};
use crate::ids::{Id16, id16};

impl Core {
    /// Devices registered on the relay, its invite policy and how much it stores.
    pub async fn server_status(&self) -> CoreResult<ServerStatus> {
        let client = self.client()?;
        if !client.is_admin {
            return Err(CoreError::NotAllowed(
                "Only the server's admins can see this".into(),
            ));
        }
        match client.request(C::ListDevices(wire::ListDevices {})).await? {
            S::DeviceList(list) => Ok(ServerStatus {
                devices: list
                    .devices
                    .iter()
                    .filter_map(|d| id16(&d.device_id).map(|id| (id, d.state, d.is_admin)))
                    .collect(),
                invite_policy: list.invite_policy,
                stored_blob_bytes: list.stored_blob_bytes,
                queued_envelopes: list.queued_envelopes,
            }),
            _ => Err(CoreError::Network("unexpected reply".into())),
        }
    }

    /// Revokes a device (a lost or stolen Mac): the relay deletes its inbox and key packages and
    /// refuses it from now on. Chats it was in should also remove it (§11.6).
    pub async fn revoke_device(&self, device_id: &Id16) -> CoreResult<()> {
        let (_, mine) = self.me()?;
        if *device_id == mine {
            return Err(CoreError::Invalid("This is the Mac you're using".into()));
        }
        let client = self.client()?;
        match client
            .request(C::RevokeDevice(wire::RevokeDevice {
                device_id: device_id.to_vec(),
            }))
            .await?
        {
            S::Ok(_) => Ok(()),
            _ => Err(CoreError::Network("unexpected reply".into())),
        }
    }
}
