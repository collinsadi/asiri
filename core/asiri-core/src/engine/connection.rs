//! The connection loop (architecture §8.4 steps 4–6, §6.4).
//!
//! * Reconnects with exponential backoff from 1 s to 60 s plus jitter, immediately on network
//!   change or wake.
//! * Incoming envelopes are decrypted and stored first, and only then acknowledged (inbox
//!   pattern): a crash midway never loses a message, the relay simply redelivers it.
//! * Outgoing envelopes wait in the encrypted outbox until the relay accepts them (outbox
//!   pattern), so sending works offline and is idempotent across retries.

use std::sync::Arc;
use std::time::Duration;

use asiri_proto::relay::v1::{
    self as wire, client_frame::Body as C, send_result::Status, server_frame::Body as S,
};

use super::{ConnectionState, Core, CoreEvent, lock_err};
use crate::error::{CoreError, CoreResult};
use crate::ids::{Id16, now_ms, random_bytes};
use crate::net::relay::{Push, RelayClient};
use crate::net::{self, Dialer};
use crate::store::{self, MessageStatus};

const ACK_BATCH: usize = 32;
const MAX_BACKOFF: Duration = Duration::from_secs(60);

impl Core {
    pub(crate) fn set_state(&self, state: ConnectionState) {
        let changed = match self.inner.state.lock() {
            Ok(mut s) if *s != state => {
                *s = state;
                true
            }
            _ => false,
        };
        if changed {
            self.emit(CoreEvent::ConnectionChanged(state));
        }
    }

    /// The live relay session, or [`CoreError::Offline`].
    pub(crate) fn client(&self) -> CoreResult<Arc<RelayClient>> {
        self.inner
            .client
            .lock()
            .map_err(lock_err)?
            .clone()
            .filter(|c| !c.is_closed())
            .ok_or(CoreError::Offline)
    }

    pub(crate) async fn dialer_for_endpoint(
        &self,
        endpoint: &crate::identity::RelayEndpoint,
    ) -> CoreResult<Arc<dyn Dialer>> {
        #[cfg(feature = "tor")]
        if !endpoint.is_direct() {
            self.set_state(ConnectionState::StartingTor);
            let data_dir = self.data_dir().to_path_buf();
            let tor = self
                .inner
                .tor
                .get_or_try_init(|| crate::net::tor::Tor::start(&data_dir))
                .await?;
            {
                let id = self.inner.identity.read().map_err(lock_err)?;
                tor.authorize(&endpoint.address, &id.onion_client)?;
            }
            return net::dialer_for(endpoint, Some(tor));
        }
        net::dialer_for(
            endpoint,
            #[cfg(feature = "tor")]
            None,
        )
    }

    pub(crate) async fn ensure_dialer(&self) -> CoreResult<Arc<dyn Dialer>> {
        if let Some(d) = self.inner.dialer.lock().map_err(lock_err)?.clone() {
            return Ok(d);
        }
        let endpoint = {
            let id = self.inner.identity.read().map_err(lock_err)?;
            id.registered()?.endpoint.clone()
        };
        let dialer = self.dialer_for_endpoint(&endpoint).await?;
        *self.inner.dialer.lock().map_err(lock_err)? = Some(Arc::clone(&dialer));
        Ok(dialer)
    }

    /// Starts the background connection loop. Safe to call repeatedly.
    pub fn connect(&self) -> CoreResult<()> {
        self.me()?;
        let mut task = self.inner.net_task.lock().map_err(lock_err)?;
        if task.as_ref().is_some_and(|t| !t.is_finished()) {
            self.inner.network_wake.notify_one();
            return Ok(());
        }
        let _ = self.inner.shutdown.send(false);
        let core = self.clone();
        *task = Some(self.inner.rt.spawn(async move { core.run_network().await }));
        Ok(())
    }

    /// Closes the connection and stops reconnecting.
    pub async fn disconnect(&self) {
        let _ = self.inner.shutdown.send(true);
        if let Ok(Some(c)) = self.inner.client.lock().map(|c| c.clone()) {
            c.close();
        }
        let task = self.inner.net_task.lock().ok().and_then(|mut t| t.take());
        if let Some(t) = task {
            t.abort();
            let _ = t.await;
        }
        if let Ok(mut c) = self.inner.client.lock() {
            *c = None;
        }
        self.set_state(ConnectionState::Offline);
    }

    /// The network changed or the Mac woke up: retry right away (§6.7).
    pub fn notify_network_change(&self) {
        self.inner.network_wake.notify_one();
    }

    async fn run_network(self) {
        let mut shutdown = self.inner.shutdown.subscribe();
        let mut backoff = Duration::from_secs(1);
        loop {
            if *shutdown.borrow() {
                break;
            }
            let started = tokio::time::Instant::now();
            if let Err(e) = self.session(&mut shutdown).await {
                tracing::debug!("relay session ended: {e}");
            }
            if let Ok(mut c) = self.inner.client.lock() {
                *c = None;
            }
            if *shutdown.borrow() {
                break;
            }
            self.set_state(ConnectionState::Offline);
            if started.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(1); // the last session was healthy
            }
            let jitter =
                Duration::from_millis(u64::from(u16::from_le_bytes(random_bytes())) % 1000);
            tokio::select! {
                () = tokio::time::sleep(backoff + jitter) => {}
                () = self.inner.network_wake.notified() => backoff = Duration::from_secs(1),
                _ = shutdown.changed() => {}
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    async fn session(&self, shutdown: &mut tokio::sync::watch::Receiver<bool>) -> CoreResult<()> {
        let dialer = self.ensure_dialer().await?;
        self.set_state(ConnectionState::Connecting);
        let (device_id, _) = {
            let id = self.inner.identity.read().map_err(lock_err)?;
            (id.registered()?.device_id, ())
        };
        let core = self.clone();
        let (client, mut pushes) = RelayClient::connect(&dialer, &device_id, move |msg| {
            core.inner
                .identity
                .read()
                .map(|id| id.sign_relay(msg))
                .unwrap_or_default()
        })
        .await?;
        *self.inner.client.lock().map_err(lock_err)? = Some(Arc::clone(&client));
        self.set_state(if client.pending_envelopes > 0 {
            ConnectionState::Syncing
        } else {
            ConnectionState::Connected
        });

        // The outbox drains in its own task so slow sends never delay incoming messages.
        let outbox = {
            let core = self.clone();
            let client = Arc::clone(&client);
            tokio::spawn(async move { core.outbox_loop(client).await })
        };
        let maintenance = {
            let core = self.clone();
            let client = Arc::clone(&client);
            tokio::spawn(async move { core.maintenance_loop(client).await })
        };

        let mut acks: Vec<u64> = Vec::new();
        let mut ack_tick = tokio::time::interval(Duration::from_millis(250));
        let result = loop {
            tokio::select! {
                push = pushes.recv() => match push {
                    Some(Push::Deliver(d)) => {
                        let seq = d.seq;
                        if self.handle_delivery(d).await {
                            acks.push(seq);
                        }
                        if acks.len() >= ACK_BATCH
                            && let Err(e) = client.ack(std::mem::take(&mut acks)).await
                        {
                            break Err(e);
                        }
                    }
                    Some(Push::Drained) => {
                        if let Err(e) = client.ack(std::mem::take(&mut acks)).await { break Err(e); }
                        self.set_state(ConnectionState::Connected);
                    }
                    None => break Err(CoreError::Offline),
                },
                _ = ack_tick.tick(), if !acks.is_empty() => {
                    if let Err(e) = client.ack(std::mem::take(&mut acks)).await { break Err(e); }
                }
                _ = shutdown.changed() => break Ok(()),
            }
        };
        if !acks.is_empty() {
            let _ = client.ack(acks).await;
        }
        client.close();
        outbox.abort();
        maintenance.abort();
        result
    }

    async fn outbox_loop(&self, client: Arc<RelayClient>) {
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        loop {
            tokio::select! {
                () = self.inner.outbox_wake.notified() => {}
                _ = tick.tick() => {}
            }
            if client.is_closed() {
                return;
            }
            self.flush_receipts();
            if let Err(e) = self.flush_outbox(&client).await {
                if e.is_transient() {
                    return;
                }
                tracing::warn!("outbox: {e}");
            }
        }
    }

    /// Encrypts a queued application message for the chat's current members. Returns `None`
    /// when it must wait (a proposal is pending) or was dropped (we left the chat).
    fn encrypt_queued(&self, item: &store::OutboxItem) -> CoreResult<Option<(Vec<Id16>, Vec<u8>)>> {
        let Some(chat_id) = item.chat_id.as_deref() else {
            self.inner.store.tx(|c| store::dequeue(c, item.id))?;
            return Ok(None);
        };
        let plaintext = item.plaintext.as_deref().unwrap_or_default();
        let id = self.inner.identity.read().map_err(lock_err)?;
        let device = id.registered()?.device_id;
        self.inner.store.tx(|c| {
            let mut group = match self.inner.mls.load(c, chat_id) {
                Ok(g) if crate::mls::is_active(&g) => g,
                _ => {
                    store::dequeue(c, item.id)?;
                    return Ok(None);
                }
            };
            if group.has_pending_proposals() {
                store::defer(c, item.id, now_ms() + 1_000)?;
                return Ok(None);
            }
            let recipients: Vec<Id16> = crate::mls::members(&group)
                .into_iter()
                .filter(|m| m.device_id != device)
                .map(|m| m.inbox_id)
                .collect();
            if recipients.is_empty() {
                store::dequeue(c, item.id)?;
                return Ok(None);
            }
            let ciphertext = self
                .inner
                .mls
                .encrypt(c, &mut group, &id.signer, plaintext)?;
            let payload = crate::crypto::padding::pad_envelope(&ciphertext);
            store::set_encrypted(c, item.id, &recipients, &payload)?;
            Ok(Some((recipients, payload)))
        })
    }

    pub(crate) fn wake_outbox(&self) {
        self.inner.outbox_wake.notify_one();
    }

    /// Sends due envelopes oldest first.
    async fn flush_outbox(&self, client: &RelayClient) -> CoreResult<()> {
        loop {
            let items = {
                let conn = self.inner.store.lock()?;
                store::due_outbox(&conn, now_ms(), 16)?
            };
            if items.is_empty() {
                return Ok(());
            }
            for mut item in items {
                if item.plaintext.is_some() {
                    match self.encrypt_queued(&item)? {
                        Some((recipients, payload)) => {
                            item.recipients = recipients;
                            item.payload = payload;
                        }
                        None => continue, // deferred (pending proposal) or dropped
                    }
                }
                let reply = client
                    .request(C::Send(wire::Send {
                        envelope_id: item.envelope_id.to_vec(),
                        kind: item.kind,
                        group_route: vec![],
                        epoch: 0,
                        to_inboxes: item.recipients.iter().map(|r| r.to_vec()).collect(),
                        payload: item.payload.clone(),
                        ttl_seconds: item.ttl_s,
                    }))
                    .await;
                let mut events = Vec::new();
                match reply {
                    Ok(S::SendResult(r)) => {
                        match Status::try_from(r.status).unwrap_or(Status::Unspecified) {
                            Status::Accepted => {
                                self.inner.store.tx(|c| {
                                    store::dequeue(c, item.id)?;
                                    if let (Some(m), Some(chat)) =
                                        (item.message_id, item.chat_id.clone())
                                        && store::set_status(c, &m, MessageStatus::Sent)?
                                    {
                                        events.push(CoreEvent::MessageUpdated {
                                            chat_id: chat,
                                            message_id: m,
                                        });
                                    }
                                    Ok(())
                                })?;
                            }
                            Status::UnknownInbox => {
                                let unknown: Vec<Id16> = r
                                    .unknown_inboxes
                                    .iter()
                                    .filter_map(|i| crate::ids::id16(i))
                                    .collect();
                                let remaining: Vec<Id16> = item
                                    .recipients
                                    .iter()
                                    .filter(|i| !unknown.contains(i))
                                    .copied()
                                    .collect();
                                self.inner.store.tx(|c| {
                                    if remaining.is_empty() {
                                        store::dequeue(c, item.id)?;
                                        if let (Some(m), Some(chat)) =
                                            (item.message_id, item.chat_id.clone())
                                            && store::set_status(c, &m, MessageStatus::Sent)?
                                        {
                                            events.push(CoreEvent::MessageUpdated {
                                                chat_id: chat,
                                                message_id: m,
                                            });
                                        }
                                    } else {
                                        store::set_recipients(c, item.id, &remaining)?;
                                    }
                                    Ok(())
                                })?;
                            }
                            Status::TooLarge => {
                                self.inner.store.tx(|c| {
                                    store::dequeue(c, item.id)?;
                                    if let (Some(m), Some(chat)) =
                                        (item.message_id, item.chat_id.clone())
                                    {
                                        store::set_status(c, &m, MessageStatus::Failed)?;
                                        events.push(CoreEvent::MessageUpdated {
                                            chat_id: chat,
                                            message_id: m,
                                        });
                                    }
                                    Ok(())
                                })?;
                            }
                            _ => {
                                let delay = 5_000 * i64::from(item.attempts.min(12) + 1);
                                self.inner
                                    .store
                                    .tx(|c| store::defer(c, item.id, now_ms() + delay))?;
                                self.emit_all(events);
                                return Ok(());
                            }
                        }
                    }
                    Ok(_) => {
                        self.inner
                            .store
                            .tx(|c| store::defer(c, item.id, now_ms() + 10_000))?;
                    }
                    Err(e) if e.is_transient() => return Err(e),
                    Err(e) => {
                        tracing::warn!("send refused: {e}");
                        self.inner.store.tx(|c| {
                            if item.attempts >= 5 {
                                store::dequeue(c, item.id)?;
                                if let (Some(m), Some(chat)) =
                                    (item.message_id, item.chat_id.clone())
                                {
                                    store::set_status(c, &m, MessageStatus::Failed)?;
                                    events.push(CoreEvent::MessageUpdated {
                                        chat_id: chat,
                                        message_id: m,
                                    });
                                }
                            } else {
                                store::defer(c, item.id, now_ms() + 15_000)?;
                            }
                            Ok(())
                        })?;
                    }
                }
                self.emit_all(events);
            }
        }
    }
}
