//! Calls against a real LiveKit server (architecture §10): the relay's call tokens are accepted,
//! and the app reaches LiveKit's signaling through the loopback bridge.
//!
//! Needs a LiveKit server on 127.0.0.1:7880 that knows the simulator's key pair, so it only runs
//! when `ASIRI_LIVEKIT=1` (CI starts `livekit/livekit-server` for it):
//!
//! ```text
//! docker run --network host livekit/livekit-server:v1.13.10 \
//!   --bind 127.0.0.1 --node-ip 127.0.0.1 --keys "APIsimulator: simulator-secret-0123456789abcdefghijklmnop"
//! ASIRI_LIVEKIT=1 cargo test -p asiri-sim --test livekit
//! ```

use std::time::Duration;

use asiri_sim::{Harness, PATIENCE, eventually};
use futures::StreamExt;
use tokio_tungstenite::tungstenite::Message;

fn enabled() -> bool {
    if std::env::var("ASIRI_LIVEKIT").is_ok_and(|v| v == "1") {
        return true;
    }
    eprintln!("skipped: set ASIRI_LIVEKIT=1 with a LiveKit server on 127.0.0.1:7880");
    false
}

/// Opens LiveKit's signaling WebSocket the way the LiveKit SDKs do and returns the first frame.
async fn join(url: &str, token: &str) -> anyhow::Result<Vec<u8>> {
    let host = url
        .strip_prefix("ws://")
        .ok_or_else(|| anyhow::anyhow!("not a ws:// URL"))?;
    let stream = tokio::net::TcpStream::connect(host).await?;
    let url = format!(
        "{url}/rtc?access_token={token}&auto_subscribe=1&sdk=swift&version=2.19.0&protocol=15"
    );
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::client_async(url, stream),
    )
    .await??;
    let first = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("LiveKit closed the connection"))??;
    match first {
        Message::Binary(b) => Ok(b.to_vec()),
        other => anyhow::bail!("unexpected first frame: {other:?}"),
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_tokens_join_livekit_through_the_bridge() -> anyhow::Result<()> {
    if !enabled() {
        return Ok(());
    }
    let h = Harness::start().await?;
    let ada = h.admin("Ada").await?;
    let ben = h.invite(&ada, "Ben").await?;
    let dm = ada.dm_with(&ben).expect("DM");

    let a = ada.core.start_call(&dm, true).await?;
    eventually("Ben's ring", PATIENCE, || ben.core.call(&a.call_id).is_ok()).await?;
    let b = ben.core.accept_call(&a.call_id).await?;

    // Both join the same LiveKit room through their own bridges. The join response names the
    // room (the random alias) and the participant (the random alias inside the token): LiveKit
    // never learns who is calling whom.
    let join_a = join(&a.url, &a.token).await?;
    assert!(contains(&join_a, &a.room_alias), "joined the wrong room");
    assert!(contains(&join_a, &a.participant_alias));
    let join_b = join(&b.url, &b.token).await?;
    assert!(contains(&join_b, &b.room_alias));
    assert!(contains(&join_b, &b.participant_alias));
    assert_eq!(a.room_alias, b.room_alias);

    // A token that was tampered with is refused. (Change the signature's first character: the
    // last one may only carry padding bits, which decode to the same signature.)
    let (signed, signature) = a.token.rsplit_once('.').expect("a JWT");
    let mut chars = signature.chars();
    let first = chars.next().expect("a signature");
    let forged = format!(
        "{signed}.{}{}",
        if first == 'A' { 'B' } else { 'A' },
        chars.as_str()
    );
    assert!(
        join(&a.url, &forged).await.is_err(),
        "LiveKit accepted a forged token"
    );

    // Once the call ends, the bridge is gone.
    ada.core.end_call(&a.call_id)?;
    assert!(
        join(&a.url, &a.token).await.is_err(),
        "the bridge outlived the call"
    );
    Ok(())
}
