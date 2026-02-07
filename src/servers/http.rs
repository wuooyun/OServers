//! HTTP Server implementation using warp

use super::{LogMessage, ServerConfig, ServerError, ServerHandle, ServerStatus, SharedState};
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::sync::mpsc;
use warp::Filter;

/// HTTP server specific configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HttpConfig {
    pub root_dir: PathBuf,
    pub port: u16,
    pub allow_directory_listing: bool,
    pub auto_stop_seconds: Option<u64>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            root_dir: std::env::current_dir().unwrap_or_default(),
            port: 7777,
            allow_directory_listing: true,
            auto_stop_seconds: Some(360),
        }
    }
}

impl From<HttpConfig> for ServerConfig {
    fn from(cfg: HttpConfig) -> Self {
        ServerConfig {
            root_dir: cfg.root_dir,
            port: cfg.port,
            auto_stop_seconds: cfg.auto_stop_seconds,
        }
    }
}

/// Start HTTP server
pub async fn start_server(
    config: HttpConfig,
    state: SharedState,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> Result<(), ServerError> {
    let root = config.root_dir.clone();
    let port = config.port;
    let allow_listing = config.allow_directory_listing;

    // Update status
    {
        let mut s = state.write();
        s.status = ServerStatus::Starting;
        s.add_log(LogMessage::info(format!("Starting HTTP server on port {}...", port)));
    }

    // Create file serving filter
    let files = if allow_listing {
        warp::fs::dir(root.clone())
    } else {
        warp::fs::dir(root.clone())
    };

    // Add logging
    let log = warp::log::custom(|info| {
        tracing::info!(
            "{} {} {} {}ms",
            info.method(),
            info.path(),
            info.status().as_u16(),
            info.elapsed().as_millis()
        );
    });

    let routes = files.with(log);

    let addr: SocketAddr = ([0, 0, 0, 0], port).into();

    // Update status to running
    {
        let mut s = state.write();
        s.status = ServerStatus::Running;
        s.add_log(LogMessage::info(format!("HTTP server started on http://0.0.0.0:{}", port)));
        s.add_log(LogMessage::info(format!("Serving files from: {}", root.display())));
    }

    // Create server with graceful shutdown
    let (_, server) = warp::serve(routes).bind_with_graceful_shutdown(addr, async move {
        shutdown_rx.recv().await;
    });

    // Handle auto-stop timeout
    if let Some(timeout_secs) = config.auto_stop_seconds {
        let state_clone = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(timeout_secs)).await;
            let mut s = state_clone.write();
            if matches!(s.status, ServerStatus::Running) {
                s.add_log(LogMessage::info(format!(
                    "Auto-stopping after {} seconds of inactivity",
                    timeout_secs
                )));
            }
        });
    }

    server.await;

    // Update status
    {
        let mut s = state.write();
        s.status = ServerStatus::Stopped;
        s.add_log(LogMessage::info("HTTP server stopped"));
    }

    Ok(())
}

/// Create a new HTTP server handle
pub fn create_handle(config: HttpConfig) -> ServerHandle {
    ServerHandle::new(config.into())
}
