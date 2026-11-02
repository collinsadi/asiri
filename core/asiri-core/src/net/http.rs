//! HTTP/1.1 over a dialer stream (architecture §8.5): registration, blob transfer, releases.
//! A fresh stream per request keeps large transfers off the WebSocket's stream.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::{Dialer, Port};
use crate::error::{CoreError, CoreResult};

/// An HTTP response.
pub struct Response {
    /// Status code.
    pub status: u16,
    /// Body.
    pub body: Vec<u8>,
    /// `Content-Range` total, when present.
    pub content_range_total: Option<u64>,
}

const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);

/// Sends a request on a fresh stream; the returned task drives the connection.
async fn send(
    dialer: &Arc<dyn Dialer>,
    method: &str,
    path: &str,
    token: Option<&[u8]>,
    headers: &[(&str, String)],
    body: Vec<u8>,
) -> CoreResult<(http::Response<Incoming>, tokio::task::JoinHandle<()>)> {
    let stream = dialer.dial(Port::Relay).await?;
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .map_err(CoreError::network)?;
    let conn_task = tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut req = http::Request::builder()
        .method(method)
        .uri(path)
        .header(http::header::HOST, dialer.host())
        .header(http::header::USER_AGENT, "Asiri");
    if let Some(t) = token {
        req = req.header(
            http::header::AUTHORIZATION,
            format!("Bearer {}", hex::encode(t)),
        );
    }
    if !body.is_empty() {
        req = req.header(http::header::CONTENT_TYPE, "application/octet-stream");
    }
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let req = req
        .body(Full::new(Bytes::from(body)))
        .map_err(CoreError::internal)?;
    let resp = match tokio::time::timeout(ANSWER_TIMEOUT, sender.send_request(req)).await {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => {
            conn_task.abort();
            return Err(CoreError::network(e));
        }
        Err(_) => {
            conn_task.abort();
            return Err(CoreError::Network(
                "the server did not answer in time".into(),
            ));
        }
    };
    Ok((resp, conn_task))
}

/// Performs one request and reads the whole response.
pub async fn request(
    dialer: &Arc<dyn Dialer>,
    method: &str,
    path: &str,
    token: Option<&[u8]>,
    headers: &[(&str, String)],
    body: Vec<u8>,
) -> CoreResult<Response> {
    let (resp, conn_task) = send(dialer, method, path, token, headers, body).await?;
    let status = resp.status().as_u16();
    let content_range_total = resp
        .headers()
        .get(http::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit_once('/'))
        .and_then(|(_, total)| total.parse().ok());
    let body = tokio::time::timeout(ANSWER_TIMEOUT, resp.into_body().collect())
        .await
        .map_err(|_| CoreError::Network("the server stopped sending".into()))?
        .map_err(CoreError::network)?
        .to_bytes()
        .to_vec();
    conn_task.abort();
    Ok(Response {
        status,
        body,
        content_range_total,
    })
}

/// A body streamed to disk by [`download`].
pub struct Download {
    /// Status code. On anything but 200 nothing was written and `error` holds the body.
    pub status: u16,
    /// Bytes written.
    pub written: u64,
    /// SHA-256 of the bytes written.
    pub sha256: [u8; 32],
    /// The error body, for non-200 answers.
    pub error: Option<Response>,
}

/// Streams a `GET` response body into `dest`, hashing as it goes. Fails once more than
/// `max_bytes` arrive, so a hostile server can't fill the disk.
pub async fn download(
    dialer: &Arc<dyn Dialer>,
    path: &str,
    token: Option<&[u8]>,
    dest: &Path,
    max_bytes: u64,
) -> CoreResult<Download> {
    let (resp, conn_task) = send(dialer, "GET", path, token, &[], Vec::new()).await?;
    let status = resp.status().as_u16();
    let mut body = resp.into_body();
    if status != 200 {
        let bytes = tokio::time::timeout(ANSWER_TIMEOUT, body.collect())
            .await
            .map_err(|_| CoreError::Network("the server stopped sending".into()))?
            .map_err(CoreError::network)?
            .to_bytes()
            .to_vec();
        conn_task.abort();
        return Ok(Download {
            status,
            written: 0,
            sha256: [0; 32],
            error: Some(Response {
                status,
                body: bytes,
                content_range_total: None,
            }),
        });
    }
    let mut file = tokio::fs::File::create(dest).await?;
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let result: CoreResult<()> = async {
        while let Some(frame) = tokio::time::timeout(CHUNK_TIMEOUT, body.frame())
            .await
            .map_err(|_| CoreError::Network("the download stalled".into()))?
        {
            let frame = frame.map_err(CoreError::network)?;
            let Ok(data) = frame.into_data() else {
                continue;
            };
            written += data.len() as u64;
            if written > max_bytes {
                return Err(CoreError::Crypto(
                    "the server sent more than it should have".into(),
                ));
            }
            hasher.update(&data);
            file.write_all(&data).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        Ok(())
    }
    .await;
    conn_task.abort();
    if let Err(e) = result {
        drop(file);
        let _ = tokio::fs::remove_file(dest).await;
        return Err(e);
    }
    Ok(Download {
        status,
        written,
        sha256: hasher.finalize().into(),
        error: None,
    })
}

/// Turns a non-2xx protobuf error body into a [`CoreError`].
pub fn error_from(resp: &Response) -> CoreError {
    use asiri_proto::Message as _;
    let detail = asiri_proto::relay::v1::Error::decode(resp.body.as_slice())
        .map(|e| e.detail)
        .unwrap_or_else(|_| format!("HTTP {}", resp.status));
    CoreError::Relay(detail)
}
