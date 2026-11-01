//! HTTP endpoints (architecture §9.2): registration, encrypted blobs, releases and health.
//!
//! Every endpoint except registration requires `Authorization: Bearer <hex session token>`,
//! issued in `AuthOk` on the WebSocket. Bodies are Protocol Buffers.

use std::path::PathBuf;
use std::sync::Arc;

use asiri_proto::Message as _;
use asiri_proto::relay::v1 as wire;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use futures::StreamExt;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::db;
use crate::error::{RelayError, RelayResult};
use crate::ids::{Id, now, parse_hex_id};
use crate::ratelimit::Kind;
use crate::state::{AppState, TokenEntry};
use crate::ws::verify_signature;

const PROTOBUF: &str = "application/x-protobuf";
const MAX_RELEASE_BYTES: usize = 600 * 1024 * 1024;

/// The HTTP routes, to be merged into the relay router.
pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    let chunk_limit = state.config.limits.blob_chunk_bytes;
    Router::new()
        .route("/v1/register", post(register))
        .route(
            "/v1/blobs/{id}",
            put(put_blob)
                .layer(DefaultBodyLimit::max(chunk_limit))
                .get(get_blob),
        )
        .route("/v1/blobs/{id}/status", get(blob_status))
        .route("/v1/releases/latest", get(release_latest))
        .route(
            "/v1/releases/{name}",
            get(release_file)
                .put(put_release_file)
                .layer(DefaultBodyLimit::max(MAX_RELEASE_BYTES)),
        )
        .route("/v1/health", get(health))
}

fn proto<M: asiri_proto::Message>(status: StatusCode, msg: &M) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, PROTOBUF)],
        msg.encode_to_vec(),
    )
        .into_response()
}

impl IntoResponse for RelayError {
    fn into_response(self) -> Response {
        if let Self::Internal(e) = &self {
            tracing::warn!("http request failed: {e:#}");
        }
        proto(self.http_status(), &self.to_wire())
    }
}

fn bearer(state: &AppState, headers: &HeaderMap) -> RelayResult<TokenEntry> {
    let value = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(RelayError::Unauthenticated)?;
    let bytes = hex::decode(value.trim()).map_err(|_| RelayError::Unauthenticated)?;
    state.token(&bytes).ok_or(RelayError::Unauthenticated)
}

// ─── registration (§9.4) ────────────────────────────────────────────────────

async fn register(State(state): State<Arc<AppState>>, body: Bytes) -> RelayResult<Response> {
    let req = wire::RegisterRequest::decode(body).map_err(|_| RelayError::BadRequest("body"))?;
    let auth: [u8; 32] = req
        .auth_pubkey
        .as_slice()
        .try_into()
        .map_err(|_| RelayError::BadRequest("auth key must be 32 bytes"))?;
    let transcript = asiri_proto::register_transcript(&req.registration_secret, &auth);
    verify_signature(&auth, &transcript, &req.signature)?;
    let secret = req.registration_secret;
    let device = state
        .db
        .call(move |c| db::pending_device_for_secret(c, &secret, now()))
        .await?
        .ok_or(RelayError::Unauthenticated)?;
    if device.auth_pubkey.is_some_and(|expected| expected != auth) {
        return Err(RelayError::Unauthenticated);
    }
    let id = device.device_id;
    state
        .db
        .call(move |c| db::activate_device(c, &id, &auth))
        .await?;
    Ok(proto(
        StatusCode::OK,
        &wire::RegisterResponse {
            device_id: device.device_id.to_vec(),
            inbox_id: device.inbox_id.to_vec(),
            is_admin: device.is_admin,
        },
    ))
}

// ─── blobs (§7.7, §8.5) ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct UploadQuery {
    offset: u64,
    total: u64,
}

fn blob_paths(state: &AppState, id: &Id) -> (PathBuf, PathBuf) {
    let dir = state.config.blob_dir();
    let name = hex::encode(id);
    (dir.join(&name), dir.join(format!("{name}.part")))
}

fn status_of(row: &db::BlobRow) -> wire::BlobStatus {
    wire::BlobStatus {
        received: row.received,
        complete: row.complete,
        size: row.size,
    }
}

/// Serializes writes to the same blob (striped by the first ID byte).
fn blob_lock(id: &Id) -> &'static tokio::sync::Mutex<()> {
    static LOCKS: std::sync::OnceLock<Vec<tokio::sync::Mutex<()>>> = std::sync::OnceLock::new();
    let locks = LOCKS.get_or_init(|| (0..32).map(|_| tokio::sync::Mutex::new(())).collect());
    &locks[usize::from(id[0]) % locks.len()]
}

async fn put_blob(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> RelayResult<Response> {
    let token = bearer(&state, &headers)?;
    let blob_id = parse_hex_id(&id).ok_or(RelayError::BadRequest("blob id"))?;
    if !state.limiter.check(&token.device_id, Kind::BlobChunk) {
        return Err(RelayError::RateLimited);
    }
    let limits = &state.config.limits;
    if q.total == 0 || q.total > limits.blob_bytes {
        return Err(RelayError::TooLarge);
    }
    let end = q
        .offset
        .checked_add(body.len() as u64)
        .filter(|e| *e <= q.total)
        .ok_or(RelayError::BadRequest("chunk exceeds declared size"))?;

    let _guard = blob_lock(&blob_id).lock().await;
    let existing = state.db.call(move |c| db::blob(c, &blob_id, now())).await?;
    match existing {
        None => {
            if q.offset != 0 {
                return Err(RelayError::Conflict("upload must start at offset 0"));
            }
            let (owner, total, quota) = (token.device_id, q.total, limits.blob_quota_per_device);
            let expires = now() + i64::from(limits.blob_ttl_seconds);
            state
                .db
                .call(move |c| db::create_blob(c, &blob_id, &owner, total, quota, expires))
                .await?;
        }
        Some(row) => {
            if row.owner_device != token.device_id {
                return Err(RelayError::Forbidden("blob belongs to another device"));
            }
            if row.complete {
                return Ok(proto(StatusCode::OK, &status_of(&row)));
            }
            if row.size != q.total {
                return Err(RelayError::Conflict("declared size changed"));
            }
            if row.received != q.offset {
                return Ok(proto(StatusCode::CONFLICT, &status_of(&row)));
            }
        }
    }

    let (final_path, part_path) = blob_paths(&state, &blob_id);
    tokio::fs::create_dir_all(state.config.blob_dir()).await?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&part_path)
        .await?;
    file.set_len(q.offset).await?; // drop any bytes from an interrupted, unrecorded write
    file.seek(std::io::SeekFrom::Start(q.offset)).await?;
    file.write_all(&body).await?;
    file.sync_data().await?;
    drop(file);
    let complete = end == q.total;
    if complete {
        tokio::fs::rename(&part_path, &final_path).await?;
    }
    state
        .db
        .call(move |c| db::set_blob_progress(c, &blob_id, end, complete))
        .await?;
    Ok(proto(
        StatusCode::OK,
        &wire::BlobStatus {
            received: end,
            complete,
            size: q.total,
        },
    ))
}

async fn blob_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> RelayResult<Response> {
    bearer(&state, &headers)?;
    let blob_id = parse_hex_id(&id).ok_or(RelayError::BadRequest("blob id"))?;
    let row = state
        .db
        .call(move |c| db::blob(c, &blob_id, now()))
        .await?
        .ok_or(RelayError::NotFound)?;
    Ok(proto(StatusCode::OK, &status_of(&row)))
}

/// Parses `bytes=START-` or `bytes=START-END` against a known size.
fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end: u64 = if end.trim().is_empty() {
        size.checked_sub(1)?
    } else {
        end.trim().parse::<u64>().ok()?.min(size.checked_sub(1)?)
    };
    (start <= end).then_some((start, end))
}

async fn get_blob(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> RelayResult<Response> {
    bearer(&state, &headers)?;
    let blob_id = parse_hex_id(&id).ok_or(RelayError::BadRequest("blob id"))?;
    let row = state
        .db
        .call(move |c| db::blob(c, &blob_id, now()))
        .await?
        .filter(|r| r.complete)
        .ok_or(RelayError::NotFound)?;
    let (path, _) = blob_paths(&state, &blob_id);
    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| RelayError::NotFound)?;
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| parse_range(v, row.size));
    let (status, start, end) = match range {
        None => (StatusCode::OK, 0, row.size.saturating_sub(1)),
        Some(Some((s, e))) => (StatusCode::PARTIAL_CONTENT, s, e),
        Some(None) => {
            let mut resp = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            if let Ok(v) = HeaderValue::from_str(&format!("bytes */{}", row.size)) {
                resp.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            return Ok(resp);
        }
    };
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let len = end - start + 1;
    let stream = tokio_util::io::ReaderStream::new(file.take(len));
    let mut resp = Response::new(Body::from_stream(stream));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        h.insert(header::CONTENT_LENGTH, v);
    }
    if status == StatusCode::PARTIAL_CONTENT
        && let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{end}/{}", row.size))
    {
        h.insert(header::CONTENT_RANGE, v);
    }
    Ok(resp)
}

// ─── releases (§16.2) ───────────────────────────────────────────────────────

fn valid_release_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

async fn serve_release(
    state: &AppState,
    name: &str,
    content_type: &'static str,
) -> RelayResult<Response> {
    if !valid_release_name(name) {
        return Err(RelayError::BadRequest("release name"));
    }
    let path = state.config.releases_dir().join(name);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| RelayError::NotFound)?;
    let len = file.metadata().await?.len();
    let mut resp = Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(file)));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(v) = HeaderValue::from_str(&len.to_string()) {
        h.insert(header::CONTENT_LENGTH, v);
    }
    Ok(resp)
}

async fn release_latest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> RelayResult<Response> {
    bearer(&state, &headers)?;
    serve_release(&state, "latest.json", "application/json").await
}

async fn release_file(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> RelayResult<Response> {
    bearer(&state, &headers)?;
    serve_release(&state, &name, "application/octet-stream").await
}

async fn put_release_file(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> RelayResult<Response> {
    let token = bearer(&state, &headers)?;
    if !token.is_admin {
        return Err(RelayError::Forbidden("admins only"));
    }
    if !valid_release_name(&name) {
        return Err(RelayError::BadRequest("release name"));
    }
    let dir = state.config.releases_dir();
    tokio::fs::create_dir_all(&dir).await?;
    let tmp = dir.join(format!(".{name}.upload"));
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut stream = body.into_data_stream();
    let mut written = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| RelayError::BadRequest("upload interrupted"))?;
        written += chunk.len();
        if written > MAX_RELEASE_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(RelayError::TooLarge);
        }
        file.write_all(&chunk).await?;
    }
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&tmp, dir.join(&name)).await?;
    Ok(proto(StatusCode::OK, &wire::Ok {}))
}

// ─── health ─────────────────────────────────────────────────────────────────

async fn health(State(state): State<Arc<AppState>>, headers: HeaderMap) -> RelayResult<Response> {
    bearer(&state, &headers)?;
    Ok(proto(
        StatusCode::OK,
        &wire::Health {
            status: "ok".into(),
            protocol_version: asiri_proto::RELAY_PROTOCOL_VERSION,
            server_version: env!("CARGO_PKG_VERSION").into(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges() {
        assert_eq!(parse_range("bytes=0-", 10), Some((0, 9)));
        assert_eq!(parse_range("bytes=4-6", 10), Some((4, 6)));
        assert_eq!(parse_range("bytes=4-60", 10), Some((4, 9)));
        assert_eq!(parse_range("bytes=11-", 10), None);
        assert_eq!(parse_range("items=0-1", 10), None);
    }

    #[test]
    fn release_names_cannot_escape() {
        assert!(valid_release_name("Asiri-1.2.0.dmg"));
        assert!(!valid_release_name("../etc/passwd"));
        assert!(!valid_release_name(".hidden"));
        assert!(!valid_release_name("a/b"));
    }
}
