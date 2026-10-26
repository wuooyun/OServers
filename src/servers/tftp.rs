//! TFTP Server implementation

use super::{LogMessage, ServerConfig, ServerError, ServerHandle, ServerStatus, SharedState};
use async_tftp::packet;
use async_tftp::server::Handler;
use async_tftp::server::handlers::{DirHandler, DirHandlerMode};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

/// TFTP server specific configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TftpConfig {
    pub root_dir: PathBuf,
    pub port: u16,
    pub read_only: bool,
}

impl Default for TftpConfig {
    fn default() -> Self {
        Self {
            root_dir: std::env::current_dir().unwrap_or_default(),
            port: 69,
            read_only: false,
        }
    }
}

impl From<TftpConfig> for ServerConfig {
    fn from(cfg: TftpConfig) -> Self {
        ServerConfig {
            root_dir: cfg.root_dir,
            port: cfg.port,
            auto_stop_seconds: None,
        }
    }
}

/// TFTP handler wrapper that logs every read (RRQ/download) and
/// write (WRQ/upload) request together with the client address.
struct LoggingHandler {
    inner: DirHandler,
    state: SharedState,
}

#[async_tftp::async_trait]
impl Handler for LoggingHandler {
    type Reader = <DirHandler as Handler>::Reader;
    type Writer = <DirHandler as Handler>::Writer;

    async fn read_req_open(
        &mut self,
        client: &SocketAddr,
        path: &Path,
    ) -> Result<(Self::Reader, Option<u64>), packet::Error> {
        let result = self.inner.read_req_open(client, path).await;
        match &result {
            Ok((_, size)) => {
                let size = size.map(|s| format!(" ({} bytes)", s)).unwrap_or_default();
                self.state.write().add_log(LogMessage::info(format!(
                    "[TFTP] client={} RRQ download {}{}",
                    client,
                    path.display(),
                    size
                )));
            }
            Err(e) => {
                self.state.write().add_log(LogMessage::error(format!(
                    "[TFTP] client={} RRQ download {} failed: {:?}",
                    client,
                    path.display(),
                    e
                )));
            }
        }
        result
    }

    async fn write_req_open(
        &mut self,
        client: &SocketAddr,
        path: &Path,
        size: Option<u64>,
    ) -> Result<Self::Writer, packet::Error> {
        let result = self.inner.write_req_open(client, path, size).await;
        match &result {
            Ok(_) => {
                let size = size.map(|s| format!(" ({} bytes)", s)).unwrap_or_default();
                self.state.write().add_log(LogMessage::info(format!(
                    "[TFTP] client={} WRQ upload {}{}",
                    client,
                    path.display(),
                    size
                )));
            }
            Err(e) => {
                self.state.write().add_log(LogMessage::error(format!(
                    "[TFTP] client={} WRQ upload {} failed: {:?}",
                    client,
                    path.display(),
                    e
                )));
            }
        }
        result
    }
}

/// Start TFTP server
pub async fn start_server(
    config: TftpConfig,
    state: SharedState,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> Result<(), ServerError> {
    let root = config.root_dir.clone();
    let port = config.port;

    // Update status
    let run_id = state
        .write()
        .begin_run(format!("Starting TFTP server on port {}...", port));

    let addr: std::net::SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();

    // Create TFTP handler (read-only, matching the previous behaviour)
    let dir_handler = match DirHandler::new(&root, DirHandlerMode::ReadOnly) {
        Ok(handler) => handler,
        Err(e) => {
            let mut s = state.write();
            if s.is_active_run(run_id) {
                s.status = ServerStatus::Error(e.to_string());
                s.add_log(LogMessage::error(format!(
                    "Failed to create TFTP server: {}",
                    e
                )));
            }
            return Err(ServerError::Other(e.to_string()));
        }
    };

    let builder = async_tftp::server::TftpServerBuilder::with_handler(LoggingHandler {
        inner: dir_handler,
        state: state.clone(),
    });

    // Create TFTP server
    let server = builder.bind(addr).build().await;

    match server {
        Ok(srv) => {
            // Update status to running
            {
                let mut s = state.write();
                if s.is_active_run(run_id) {
                    s.status = ServerStatus::Running;
                    s.add_log(LogMessage::info(format!(
                        "TFTP server started on tftp://0.0.0.0:{}",
                        port
                    )));
                    s.add_log(LogMessage::info(format!(
                        "Root directory: {}",
                        root.display()
                    )));
                    s.add_log(LogMessage::info(format!(
                        "Mode: {}",
                        if config.read_only {
                            "read-only"
                        } else {
                            "read-write"
                        }
                    )));
                }
            }

            // Run server with shutdown signal
            tokio::select! {
                result = srv.serve() => {
                    if let Err(e) = result {
                        let mut s = state.write();
                        if s.is_active_run(run_id) {
                            s.status = ServerStatus::Error(e.to_string());
                            s.add_log(LogMessage::error(format!("TFTP server error: {}", e)));
                        }
                        return Err(ServerError::Other(e.to_string()));
                    }
                }
                _ = shutdown_rx.recv() => {
                    // Shutdown requested
                }
            }
        }
        Err(e) => {
            let mut s = state.write();
            if s.is_active_run(run_id) {
                s.status = ServerStatus::Error(e.to_string());
                s.add_log(LogMessage::error(format!(
                    "Failed to build TFTP server: {}",
                    e
                )));
            }
            return Err(ServerError::Other(e.to_string()));
        }
    }

    // Update status
    {
        let mut s = state.write();
        if s.is_active_run(run_id) {
            s.status = ServerStatus::Stopped;
            s.add_log(LogMessage::info("TFTP server stopped"));
        }
    }

    Ok(())
}

/// Create a new TFTP server handle
#[allow(dead_code)]
pub fn create_handle(config: TftpConfig) -> ServerHandle {
    ServerHandle::new(config.into())
}
