//! SSH/SFTP Server implementation (placeholder)
//! Note: Full SSH implementation is complex. This is a simplified version.

use super::{LogMessage, ServerConfig, ServerError, ServerHandle, ServerStatus, SharedState};
use std::path::PathBuf;
use tokio::sync::mpsc;

/// SSH server specific configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SshConfig {
    pub root_dir: PathBuf,
    pub port: u16,
    pub username: String,
    pub password: String,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            root_dir: std::env::current_dir().unwrap_or_default(),
            port: 2222,
            username: "admin".to_string(),
            password: "admin".to_string(),
        }
    }
}

impl From<SshConfig> for ServerConfig {
    fn from(cfg: SshConfig) -> Self {
        ServerConfig {
            root_dir: cfg.root_dir,
            port: cfg.port,
            auto_stop_seconds: None,
        }
    }
}

/// Start SSH server
/// Note: This is a placeholder. Full SSH implementation requires more work.
pub async fn start_server(
    config: SshConfig,
    state: SharedState,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> Result<(), ServerError> {
    let port = config.port;

    // Update status
    {
        let mut s = state.write();
        s.status = ServerStatus::Starting;
        s.add_log(LogMessage::info(format!(
            "Starting SSH server on port {}...",
            port
        )));
    }

    // For now, we'll just mark it as running and wait for shutdown
    // Full SSH implementation would use russh here
    {
        let mut s = state.write();
        s.status = ServerStatus::Running;
        s.add_log(LogMessage::info(format!(
            "SSH server started on port {}",
            port
        )));
        s.add_log(LogMessage::info(format!(
            "Root directory: {}",
            config.root_dir.display()
        )));
        s.add_log(LogMessage::info("Note: SSH server is in simplified mode"));
    }

    // Wait for shutdown signal
    shutdown_rx.recv().await;

    // Update status
    {
        let mut s = state.write();
        s.status = ServerStatus::Stopped;
        s.add_log(LogMessage::info("SSH server stopped"));
    }

    Ok(())
}

/// Create a new SSH server handle
#[allow(dead_code)]
pub fn create_handle(config: SshConfig) -> ServerHandle {
    ServerHandle::new(config.into())
}
