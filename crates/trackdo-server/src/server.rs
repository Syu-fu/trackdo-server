//! Runs the API on a listener until shutdown.

use std::future::Future;
use std::sync::Arc;

use thiserror::Error;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::auth::Tokens;
use crate::config::Config;
use crate::http::{AppState, router, with_cors};
use crate::notify::Notifier;
use crate::store::{Store, StoreError};

#[derive(Debug, Error)]
pub enum ServeError {
    #[error("cannot open the database: {0}")]
    Store(#[from] StoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Serves until `shutdown` completes, then ends open event streams and waits
/// for in-flight requests.
pub async fn serve(
    config: &Config,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), ServeError> {
    let closing = CancellationToken::new();
    let state = AppState {
        tokens: Arc::new(Tokens::new(config.tokens.clone())),
        store: Store::open(&config.database).await?,
        notifier: Notifier::default(),
        closing: closing.clone(),
    };
    // Event streams never end on their own, so graceful shutdown would wait
    // forever without closing them first.
    axum::serve(listener, with_cors(router(state), &config.cors_origins))
        .with_graceful_shutdown(async move {
            shutdown.await;
            closing.cancel();
        })
        .await?;
    Ok(())
}
