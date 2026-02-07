//! TFTP Server implementation

use super::{LogMessage, ServerConfig, ServerError, ServerHandle, ServerStatus, SharedState};
use std::path::PathBuf;
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

/// Start TFTP server
pub async fn start_server(
    config: TftpConfig,
    state: SharedState,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> Result<(), ServerError> {
    let root = config.root_dir.clone();
    let port = config.port;

    // Update status
    {
        let mut s = state.write();
        s.status = ServerStatus::Starting;
        s.add_log(LogMessage::info(format!("Starting TFTP server on port {}...", port)));
    }

    let addr: std::net::SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();

    // Create TFTP server
    let server_result = async_tftp::server::TftpServerBuilder::with_dir_ro(&root);

    match server_result {
        Ok(builder) => {
            let server = builder.bind(addr).build().await;
            match server {
                Ok(srv) => {
                    // Update status to running
                    {
                        let mut s = state.write();
                        s.status = ServerStatus::Running;
                        s.add_log(LogMessage::info(format!(
                            "TFTP server started on tftp://0.0.0.0:{}",
                            port
                        )));
                        s.add_log(LogMessage::info(format!("Root directory: {}", root.display())));
                        s.add_log(LogMessage::info(format!(
                            "Mode: {}",
                            if config.read_only {
                                "read-only"
                            } else {
                                "read-write"
                            }
                        )));
                    }

                    // Run server with shutdown signal
                    tokio::select! {
                        result = srv.serve() => {
                            if let Err(e) = result {
                                let mut s = state.write();
                                s.status = ServerStatus::Error(e.to_string());
                                s.add_log(LogMessage::error(format!("TFTP server error: {}", e)));
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
                    s.status = ServerStatus::Error(e.to_string());
                    s.add_log(LogMessage::error(format!("Failed to build TFTP server: {}", e)));
                    return Err(ServerError::Other(e.to_string()));
                }
            }
        }
        Err(e) => {
            let mut s = state.write();
            s.status = ServerStatus::Error(e.to_string());
            s.add_log(LogMessage::error(format!("Failed to create TFTP server: {}", e)));
            return Err(ServerError::Other(e.to_string()));
        }
    }

    // Update status
    {
        let mut s = state.write();
        s.status = ServerStatus::Stopped;
        s.add_log(LogMessage::info("TFTP server stopped"));
    }

    Ok(())
}

/// Create a new TFTP server handle
pub fn create_handle(config: TftpConfig) -> ServerHandle {
    ServerHandle::new(config.into())
}
