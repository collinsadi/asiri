//! End-to-end tests of the relay over real sockets: registration, authentication, ordered
//! delivery with acknowledgements, offline delivery, commit ordering, key packages, blobs,
//! revocation, invite policy and call tokens.

use std::net::SocketAddr;
use std::time::Duration;

use asiri_proto::Message as _;
use asiri_proto::relay::v1::{
    self as wire, client_frame::Body as C, send_result::Status, server_frame::Body as S,
};
use asiri_relay::config::{Config, LiveKitKeys};
use asiri_relay::db::{self, InvitePolicy};
use asiri_relay::state::AppState;
use asiri_relay::{admin, livekit};
use ed25519_dalek::{Signer, SigningKey};
use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<TcpStream>;

struct Relay {
    addr: SocketAddr,
    state: std::sync::Arc<AppState>,
    _dir: tempfile::TempDir,
}

async fn start_relay() -> Relay {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_dev(dir.path().to_path_buf());
    config.public_address = "relay.test".into();
    config.livekit = Some(LiveKitKeys {
        api_key: "APIe2e".into(),
        api_secret: "k".repeat(40),
    });
    let (addr, state, _handle) = asiri_relay::spawn(config).await.unwrap();
    Relay {
        addr,
        state,
        _dir: dir,
    }
}

/// A device as the relay sees it: keys plus the IDs it was assigned.
struct Device {
    auth: SigningKey,
    device_id: Vec<u8>,
    inbox_id: Vec<u8>,
}

fn new_key() -> SigningKey {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).unwrap();
    SigningKey::from_bytes(&seed)
}

async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&[u8]>,
    extra: &[(&str, String)],
    body: Vec<u8>,
) -> (u16, Vec<u8>) {
    let stream = TcpStream::connect(addr).await.unwrap();
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .unwrap();
    tokio::spawn(conn);
    let mut req = hyper::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "relay.test");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {}", hex::encode(t)));
    }
    for (k, v) in extra {
        req = req.header(*k, v);
    }
    let resp = sender
        .send_request(req.body(Full::new(Bytes::from(body))).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

async fn register(relay: &Relay, secret: &[u8]) -> Device {
    let auth = new_key();
    let pk = auth.verifying_key().to_bytes();
    let sig = auth.sign(&asiri_proto::register_transcript(secret, &pk));
    let req = wire::RegisterRequest {
        registration_secret: secret.to_vec(),
        auth_pubkey: pk.to_vec(),
        signature: sig.to_bytes().to_vec(),
    };
    let (status, body) = http(
        relay.addr,
        "POST",
        "/v1/register",
        None,
        &[],
        req.encode_to_vec(),
    )
    .await;
    assert_eq!(status, 200, "registration failed");
    let resp = wire::RegisterResponse::decode(body.as_slice()).unwrap();
    Device {
        auth,
        device_id: resp.device_id,
        inbox_id: resp.inbox_id,
    }
}

async fn bootstrap_admin(relay: &Relay) -> Device {
    let inv = admin::bootstrap(
        &relay.state.db,
        &relay.state.config,
        None,
        asiri_proto::pairing::v1::relay_endpoint::Transport::Direct,
        &relay.addr.to_string(),
        None,
    )
    .unwrap();
    register(relay, &inv.registration_secret).await
}

struct Session {
    ws: Ws,
    token: Vec<u8>,
    next_req: u64,
    pending: Vec<wire::ServerFrame>,
}

async fn connect(relay: &Relay, dev: &Device) -> Session {
    try_connect(relay, dev)
        .await
        .expect("authentication failed")
}

async fn try_connect(relay: &Relay, dev: &Device) -> Result<Session, wire::Error> {
    let stream = TcpStream::connect(relay.addr).await.unwrap();
    let (mut ws, _) = tokio_tungstenite::client_async(format!("ws://{}/v1/ws", relay.addr), stream)
        .await
        .unwrap();
    let challenge = match recv_raw(&mut ws).await.body {
        Some(S::Challenge(c)) => c,
        other => panic!("expected a challenge, got {other:?}"),
    };
    let transcript = asiri_proto::auth_transcript(&challenge.nonce, "relay.test");
    let frame = wire::ClientFrame {
        req_id: 1,
        body: Some(C::Auth(wire::AuthResponse {
            device_id: dev.device_id.clone(),
            signature: dev.auth.sign(&transcript).to_bytes().to_vec(),
        })),
    };
    ws.send(Message::Binary(frame.encode_to_vec().into()))
        .await
        .unwrap();
    match recv_raw(&mut ws).await.body {
        Some(S::AuthOk(ok)) => Ok(Session {
            ws,
            token: ok.session_token,
            next_req: 2,
            pending: vec![],
        }),
        Some(S::Error(e)) => Err(e),
        other => panic!("unexpected auth reply {other:?}"),
    }
}

async fn recv_raw(ws: &mut Ws) -> wire::ServerFrame {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a frame")
            .expect("connection closed")
            .unwrap();
        if let Message::Binary(b) = msg {
            return wire::ServerFrame::decode(b.as_ref()).unwrap();
        }
    }
}

impl Session {
    /// Sends a request and waits for the reply with the same req_id, buffering pushes.
    async fn request(&mut self, body: C) -> S {
        let req_id = self.next_req;
        self.next_req += 1;
        let frame = wire::ClientFrame {
            req_id,
            body: Some(body),
        };
        self.ws
            .send(Message::Binary(frame.encode_to_vec().into()))
            .await
            .unwrap();
        loop {
            let f = recv_raw(&mut self.ws).await;
            if f.req_id == req_id {
                return f.body.unwrap();
            }
            self.pending.push(f);
        }
    }

    /// Next envelope delivery (from the buffer or the socket).
    async fn next_delivery(&mut self) -> wire::Deliver {
        loop {
            if let Some(pos) = self
                .pending
                .iter()
                .position(|f| matches!(f.body, Some(S::Deliver(_))))
                && let Some(S::Deliver(d)) = self.pending.remove(pos).body
            {
                return d;
            }
            let f = recv_raw(&mut self.ws).await;
            self.pending.push(f);
        }
    }

    async fn send_app(
        &mut self,
        envelope_id: [u8; 16],
        to: &[&Device],
        payload: &[u8],
    ) -> wire::SendResult {
        match self
            .request(C::Send(wire::Send {
                envelope_id: envelope_id.to_vec(),
                kind: wire::PayloadKind::Application as i32,
                to_inboxes: to.iter().map(|d| d.inbox_id.clone()).collect(),
                payload: payload.to_vec(),
                ..Default::default()
            }))
            .await
        {
            S::SendResult(r) => r,
            other => panic!("unexpected {other:?}"),
        }
    }

    async fn authorize(&mut self) -> Result<wire::DeviceAuthorized, wire::Error> {
        let onion = [5u8; 32];
        let auth = new_key();
        match self
            .request(C::AuthorizeDevice(wire::AuthorizeDevice {
                onion_client_key: onion.to_vec(),
                auth_pubkey: auth.verifying_key().to_bytes().to_vec(),
            }))
            .await
        {
            S::DeviceAuthorized(d) => Ok(d),
            S::Error(e) => Err(e),
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// Admin authorizes a new device which registers with the auth key it presented.
async fn add_member(relay: &Relay, admin: &mut Session) -> Device {
    let auth = new_key();
    let reply = admin
        .request(C::AuthorizeDevice(wire::AuthorizeDevice {
            onion_client_key: vec![6u8; 32],
            auth_pubkey: auth.verifying_key().to_bytes().to_vec(),
        }))
        .await;
    let S::DeviceAuthorized(authorized) = reply else {
        panic!("authorize failed: {reply:?}");
    };
    let pk = auth.verifying_key().to_bytes();
    let sig = auth.sign(&asiri_proto::register_transcript(
        &authorized.registration_secret,
        &pk,
    ));
    let req = wire::RegisterRequest {
        registration_secret: authorized.registration_secret.clone(),
        auth_pubkey: pk.to_vec(),
        signature: sig.to_bytes().to_vec(),
    };
    let (status, body) = http(
        relay.addr,
        "POST",
        "/v1/register",
        None,
        &[],
        req.encode_to_vec(),
    )
    .await;
    assert_eq!(status, 200);
    let resp = wire::RegisterResponse::decode(body.as_slice()).unwrap();
    assert_eq!(resp.device_id, authorized.device_id);
    assert!(!resp.is_admin);
    Device {
        auth,
        device_id: resp.device_id,
        inbox_id: resp.inbox_id,
    }
}

#[tokio::test]
async fn offline_delivery_is_ordered_acknowledged_and_not_redelivered() {
    let relay = start_relay().await;
    let a = bootstrap_admin(&relay).await;
    let mut sa = connect(&relay, &a).await;
    let b = add_member(&relay, &mut sa).await;

    // Bob is offline: messages wait in his inbox.
    for i in 0..3u8 {
        let r = sa.send_app([i; 16], &[&b], &[i; 300]).await;
        assert_eq!(r.status, Status::Accepted as i32);
    }
    let mut sb = connect(&relay, &b).await;
    let mut seqs = vec![];
    for i in 0..3u8 {
        let d = sb.next_delivery().await;
        assert_eq!(d.envelope_id, vec![i; 16], "delivered in send order");
        assert_eq!(d.payload, vec![i; 300]);
        seqs.push(d.seq);
    }
    // Ack only the first two, then reconnect: the third must be redelivered, the others not.
    assert!(matches!(
        sb.request(C::Ack(wire::Ack {
            seqs: seqs[..2].to_vec()
        }))
        .await,
        S::Ok(_)
    ));
    drop(sb);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut sb = connect(&relay, &b).await;
    let d = sb.next_delivery().await;
    assert_eq!(d.envelope_id, vec![2u8; 16]);
    assert!(matches!(
        sb.request(C::Ack(wire::Ack { seqs: vec![d.seq] })).await,
        S::Ok(_)
    ));

    // Online delivery is pushed immediately.
    let r = sa.send_app([9; 16], &[&b], b"live").await;
    assert_eq!(r.status, Status::Accepted as i32);
    assert_eq!(sb.next_delivery().await.payload, b"live");
}

#[tokio::test]
async fn concurrent_commits_have_exactly_one_winner() {
    let relay = start_relay().await;
    let a = bootstrap_admin(&relay).await;
    let mut sa = connect(&relay, &a).await;
    let b = add_member(&relay, &mut sa).await;
    let mut sb = connect(&relay, &b).await;
    let route = vec![7u8; 16];
    let commit = |id: u8, to: &Device| {
        C::Send(wire::Send {
            envelope_id: vec![id; 16],
            kind: wire::PayloadKind::Commit as i32,
            group_route: route.clone(),
            epoch: 0,
            to_inboxes: vec![to.inbox_id.clone()],
            payload: vec![id; 64],
            ttl_seconds: 0,
        })
    };
    let (ra, rb) = tokio::join!(sa.request(commit(1, &b)), sb.request(commit(2, &a)));
    let statuses: Vec<i32> = [ra, rb]
        .into_iter()
        .map(|r| match r {
            S::SendResult(r) => r.status,
            other => panic!("{other:?}"),
        })
        .collect();
    let accepted = statuses
        .iter()
        .filter(|s| **s == Status::Accepted as i32)
        .count();
    let stale = statuses
        .iter()
        .filter(|s| **s == Status::StaleEpoch as i32)
        .count();
    assert_eq!(
        (accepted, stale),
        (1, 1),
        "one commit wins epoch 0: {statuses:?}"
    );
}

#[tokio::test]
async fn key_packages_blobs_and_call_tokens() {
    let relay = start_relay().await;
    let a = bootstrap_admin(&relay).await;
    let mut sa = connect(&relay, &a).await;
    let b = add_member(&relay, &mut sa).await;
    let mut sb = connect(&relay, &b).await;

    // Key packages: two one-time packages, then the last-resort package forever.
    let put = sb
        .request(C::PutKeyPackages(wire::PutKeyPackages {
            key_packages: vec![b"kp1".to_vec(), b"kp2".to_vec()],
            last_resort: b"kp-lr".to_vec(),
        }))
        .await;
    assert!(matches!(put, S::Ok(_)));
    let mut got = vec![];
    for _ in 0..4 {
        match sa
            .request(C::FetchKeyPackage(wire::FetchKeyPackage {
                device_id: b.device_id.clone(),
            }))
            .await
        {
            S::KeyPackage(kp) => got.push(kp.key_package),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        got,
        vec![
            b"kp1".to_vec(),
            b"kp2".to_vec(),
            b"kp-lr".to_vec(),
            b"kp-lr".to_vec()
        ]
    );

    // Blob: upload in two chunks with a wrong-offset retry in between, then ranged download.
    let blob = vec![0xABu8; 1500];
    let id = hex::encode([3u8; 16]);
    let put_chunk = |offset: usize, data: Vec<u8>| {
        let token = sa.token.clone();
        let addr = relay.addr;
        let id = id.clone();
        async move {
            http(
                addr,
                "PUT",
                &format!("/v1/blobs/{id}?offset={offset}&total=1500"),
                Some(&token),
                &[],
                data,
            )
            .await
        }
    };
    let (s1, _) = put_chunk(0, blob[..1000].to_vec()).await;
    assert_eq!(s1, 200);
    let (s_conflict, body) = put_chunk(200, blob[200..400].to_vec()).await;
    assert_eq!(s_conflict, 409);
    assert_eq!(
        wire::BlobStatus::decode(body.as_slice()).unwrap().received,
        1000
    );
    let (s2, body) = put_chunk(1000, blob[1000..].to_vec()).await;
    assert_eq!(s2, 200);
    assert!(wire::BlobStatus::decode(body.as_slice()).unwrap().complete);
    let (s, all) = http(
        relay.addr,
        "GET",
        &format!("/v1/blobs/{id}"),
        Some(&sb.token),
        &[],
        vec![],
    )
    .await;
    assert_eq!((s, all.len()), (200, 1500));
    let (s, part) = http(
        relay.addr,
        "GET",
        &format!("/v1/blobs/{id}"),
        Some(&sb.token),
        &[("range", "bytes=1400-".to_owned())],
        vec![],
    )
    .await;
    assert_eq!((s, part.len()), (206, 100));
    let (s, _) = http(
        relay.addr,
        "GET",
        &format!("/v1/blobs/{id}"),
        None,
        &[],
        vec![],
    )
    .await;
    assert_eq!(s, 401, "blobs need a session token");

    // Call tokens are signed with the LiveKit secret and carry a random identity.
    match sb
        .request(C::CallToken(wire::CallTokenRequest {
            room_alias: "r_abc".into(),
        }))
        .await
    {
        S::CallToken(t) => {
            let claims =
                livekit::verify(relay.state.config.livekit.as_ref().unwrap(), &t.jwt).unwrap();
            assert_eq!(claims["video"]["room"], "r_abc");
            assert_eq!(claims["sub"], t.identity.as_str());
            assert_ne!(t.identity, hex::encode(&b.device_id));
        }
        other => panic!("{other:?}"),
    }
    let (s, body) = http(
        relay.addr,
        "GET",
        "/v1/health",
        Some(&sa.token),
        &[],
        vec![],
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(wire::Health::decode(body.as_slice()).unwrap().status, "ok");
}

#[tokio::test]
async fn policy_revocation_and_bad_requests() {
    let relay = start_relay().await;
    let a = bootstrap_admin(&relay).await;
    let mut sa = connect(&relay, &a).await;
    let b = add_member(&relay, &mut sa).await;
    let mut sb = connect(&relay, &b).await;

    // Default policy: only admins add devices.
    let err = sb.authorize().await.unwrap_err();
    assert_eq!(err.code, wire::ErrorCode::Forbidden as i32);
    relay
        .state
        .db
        .call(|c| db::set_invite_policy(c, InvitePolicy::Members))
        .await
        .unwrap();
    assert!(
        sb.authorize().await.is_ok(),
        "members may invite after the policy change"
    );

    // Members can't list devices.
    assert!(matches!(
        sb.request(C::ListDevices(wire::ListDevices {})).await,
        S::Error(_)
    ));
    match sa.request(C::ListDevices(wire::ListDevices {})).await {
        S::DeviceList(l) => assert!(l.devices.len() >= 2),
        other => panic!("{other:?}"),
    }

    // Unknown recipients are reported and nothing is queued.
    let ghost = Device {
        auth: new_key(),
        device_id: vec![0; 16],
        inbox_id: vec![0xEE; 16],
    };
    let r = sa.send_app([1; 16], &[&b, &ghost], b"x").await;
    assert_eq!(r.status, Status::UnknownInbox as i32);
    assert_eq!(r.unknown_inboxes, vec![vec![0xEE; 16]]);

    // Oversized envelopes are refused.
    let r = sa
        .send_app([2; 16], &[&b], &vec![0u8; 300 * 1024 - 1024])
        .await;
    assert_eq!(r.status, Status::TooLarge as i32);

    // Revoking Bob ends his session and he can never authenticate again.
    let reply = sa
        .request(C::RevokeDevice(wire::RevokeDevice {
            device_id: b.device_id.clone(),
        }))
        .await;
    assert!(matches!(reply, S::Ok(_)));
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match sb.ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "revoked session is closed");
    assert!(try_connect(&relay, &b).await.is_err());

    // A forged signature is rejected.
    let mallory = Device {
        auth: new_key(),
        device_id: a.device_id.clone(),
        inbox_id: a.inbox_id.clone(),
    };
    let err = try_connect(&relay, &mallory).await.err().unwrap();
    assert_eq!(err.code, wire::ErrorCode::Unauthenticated as i32);
}
