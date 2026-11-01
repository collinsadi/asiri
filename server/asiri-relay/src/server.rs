//! Wiring: router, background retention, graceful shutdown.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::db::{self, Db};
use crate::ids::now;
use crate::state::AppState;
use crate::{authorized, gc, http_api, ws};

/// How often retention runs.
const GC_INTERVAL: Duration = Duration::from_secs(600);

/// Builds the relay router.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/ws", get(ws_upgrade))
        .merge(http_api::routes(&state))
        .with_state(state)
}

async fn ws_upgrade(State(state): State<Arc<AppState>>, upgrade: WebSocketUpgrade) -> Response {
    upgrade
        .max_message_size(asiri_proto::MAX_FRAME_BYTES)
        .max_frame_size(asiri_proto::MAX_FRAME_BYTES)
        .on_upgrade(move |socket| ws::run(state, socket))
}

/// Opens the database and directories and returns the shared state.
pub fn prepare(config: Config) -> anyhow::Result<Arc<AppState>> {
    std::fs::create_dir_all(config.blob_dir())?;
    std::fs::create_dir_all(config.releases_dir())?;
    let db = Db::open(&config.db_path())?;
    if let Some(dir) = &config.authorized_dir {
        let keys = db.call_blocking(|c| db::reachable_onion_keys(c))?;
        authorized::resync(dir, &keys)?;
    }
    Ok(AppState::new(config, db))
}

fn spawn_gc(state: &Arc<AppState>) -> JoinHandle<()> {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(GC_INTERVAL);
        loop {
            tick.tick().await;
            let blob_dir = state.config.blob_dir();
            if let Err(e) = state.db.call(move |c| gc::run(c, &blob_dir, now())).await {
                tracing::warn!("retention pass failed: {e}");
            }
        }
    })
}

/// Runs the relay until SIGINT/SIGTERM.
pub async fn serve(config: Config) -> anyhow::Result<()> {
    let listen = config.listen;
    let state = prepare(config)?;
    let gc_task = spawn_gc(&state);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::warn!("asiri-relay listening on {}", listener.local_addr()?);
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    gc_task.abort();
    Ok(())
}

/// Starts the relay on `config.listen` (use port 0 for an ephemeral port) and returns the bound
/// address. Used by tests, the simulator and local development.
pub async fn spawn(config: Config) -> anyhow::Result<(SocketAddr, Arc<AppState>, JoinHandle<()>)> {
    let listen = config.listen;
    let state = prepare(config)?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let addr = listener.local_addr()?;
    let app = router(Arc::clone(&state));
    let gc_task = spawn_gc(&state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
        gc_task.abort();
    });
    Ok((addr, state, handle))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = term => {},
    }
}
