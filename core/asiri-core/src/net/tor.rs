//! Embedded Tor (architecture §8.2–§8.3): Arti with `onion-service-client`, state and cache in
//! the app container, the device's restricted-discovery key in Arti's keystore.

use std::path::Path;
use std::str::FromStr as _;
use std::sync::Arc;

use arti_client::config::{BoolOrAuto, TorClientConfigBuilder};
use arti_client::{HsId, KeystoreSelector, StreamPrefs, TorClient};
use tor_rtcompat::PreferredRuntime;
use x25519_dalek::StaticSecret;

use super::{BoxFuture, BoxStream, Dialer, Port};
use crate::error::{CoreError, CoreResult};
use crate::identity::RelayEndpoint;

/// A bootstrapped Tor client shared by every onion dialer.
pub struct Tor {
    client: Arc<TorClient<PreferredRuntime>>,
}

impl Tor {
    /// Bootstraps Tor with its state under `data_dir/tor` ("Connecting securely…", §8.3).
    pub async fn start(data_dir: &Path) -> CoreResult<Arc<Self>> {
        let state = data_dir.join("tor").join("state");
        let cache = data_dir.join("tor").join("cache");
        std::fs::create_dir_all(&state)?;
        std::fs::create_dir_all(&cache)?;
        let config = TorClientConfigBuilder::from_directories(state, cache)
            .build()
            .map_err(CoreError::network)?;
        let client = TorClient::create_bootstrapped(config)
            .await
            .map_err(|e| CoreError::Network(format!("Tor could not start: {e}")))?;
        Ok(Arc::new(Self { client }))
    }

    /// Installs our restricted-discovery key for `onion_address`, so the service's descriptor
    /// can be decrypted. Idempotent.
    pub fn authorize(&self, onion_address: &str, secret: &StaticSecret) -> CoreResult<()> {
        let hsid = HsId::from_str(onion_address)
            .map_err(|_| CoreError::Invalid("invalid onion address".into()))?;
        if let Ok(Some(existing)) = self.client.get_service_discovery_key(hsid) {
            let ours = x25519_dalek::PublicKey::from(secret);
            if existing.as_ref().as_bytes() == ours.as_bytes() {
                return Ok(());
            }
            let _ = self
                .client
                .remove_service_discovery_key(KeystoreSelector::Primary, hsid);
        }
        let sk = tor_hscrypto::pk::HsClientDescEncSecretKey::from(
            tor_llcrypto::pk::curve25519::StaticSecret::from(secret.to_bytes()),
        );
        self.client
            .insert_service_discovery_key(KeystoreSelector::Primary, hsid, sk)
            .map(|_| ())
            .map_err(|e| CoreError::Network(format!("could not store the onion key: {e}")))
    }
}

/// Dials the relay's onion service: port 80 for the relay, 7880 for call signaling.
pub struct OnionDialer {
    tor: Arc<Tor>,
    address: String,
    prefs: StreamPrefs,
}

impl OnionDialer {
    /// A dialer for an onion endpoint.
    pub fn new(tor: Arc<Tor>, endpoint: &RelayEndpoint) -> CoreResult<Self> {
        let mut prefs = StreamPrefs::new();
        prefs.connect_to_onion_services(BoolOrAuto::Explicit(true));
        Ok(Self {
            tor,
            address: endpoint.address.clone(),
            prefs,
        })
    }
}

impl Dialer for OnionDialer {
    fn dial(&self, port: Port) -> BoxFuture<'_, CoreResult<BoxStream>> {
        Box::pin(async move {
            let p = match port {
                Port::Relay => 80,
                Port::Calls => 7880,
            };
            let stream = self
                .tor
                .client
                .connect_with_prefs((self.address.as_str(), p), &self.prefs)
                .await
                .map_err(|e| CoreError::Network(format!("onion connection failed: {e}")))?;
            Ok(Box::new(stream) as BoxStream)
        })
    }

    fn service_address(&self) -> &str {
        &self.address
    }

    fn host(&self) -> &str {
        &self.address
    }

    fn is_anonymous(&self) -> bool {
        true
    }
}
