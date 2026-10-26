//! Server trait and common types for the multi-server manager

pub mod ftp;
pub mod http;
pub mod ssh;
pub mod tftp;

use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;

/// Server error types
#[allow(dead_code)]
#[derive(Error, Debug)]
pub enum ServerError {
    #[error("Server already running")]
    AlreadyRunning,
    #[error("Server not running")]
    NotRunning,
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Other error: {0}")]
    Other(String),
}

/// Server status
#[derive(Debug, Clone, PartialEq)]
pub enum ServerStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error(String),
}

/// Log message from server
#[derive(Debug, Clone)]
pub struct LogMessage {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub level: LogLevel,
    pub message: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

impl LogMessage {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            timestamp: chrono::Local::now(),
            level: LogLevel::Info,
            message: message.into(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            timestamp: chrono::Local::now(),
            level: LogLevel::Warning,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            timestamp: chrono::Local::now(),
            level: LogLevel::Error,
            message: message.into(),
        }
    }
}

/// Common server configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServerConfig {
    pub root_dir: PathBuf,
    pub port: u16,
    pub auto_stop_seconds: Option<u64>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            root_dir: std::env::current_dir().unwrap_or_default(),
            port: 8080,
            auto_stop_seconds: None,
        }
    }
}

/// Shared server state
#[allow(dead_code)]
#[derive(Debug)]
pub struct ServerState {
    pub status: ServerStatus,
    pub logs: Vec<LogMessage>,
    pub config: ServerConfig,
    /// Generation counter, incremented each time a server run is started.
    ///
    /// A server task may only update the status while it still owns the
    /// current generation. This prevents a stale task (e.g. one that is
    /// shutting down after the user restarted the server) from overwriting
    /// the status of the newer run.
    pub run_id: u64,
}

impl ServerState {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            status: ServerStatus::Stopped,
            logs: Vec::new(),
            config,
            run_id: 0,
        }
    }

    /// Begin a new server run and mark the server as starting.
    ///
    /// Returns the generation id of the new run.
    pub fn begin_run(&mut self, message: impl Into<String>) -> u64 {
        self.run_id = self.run_id.wrapping_add(1);
        self.status = ServerStatus::Starting;
        self.add_log(LogMessage::info(message));
        self.run_id
    }

    /// Returns true if `run_id` is still the current run.
    pub fn is_active_run(&self, run_id: u64) -> bool {
        self.run_id == run_id
    }

    pub fn add_log(&mut self, msg: LogMessage) {
        // Mirror the message to the tracing subscriber so that per-client
        // operations are also visible on the console / in log files.
        match msg.level {
            LogLevel::Info => tracing::info!(target: "oservers", "{}", msg.message),
            LogLevel::Warning => tracing::warn!(target: "oservers", "{}", msg.message),
            LogLevel::Error => tracing::error!(target: "oservers", "{}", msg.message),
        }

        self.logs.push(msg);
        // Keep only the last MAX_LOG_ENTRIES messages
        if self.logs.len() > MAX_LOG_ENTRIES {
            self.logs.remove(0);
        }
    }
}

/// Maximum number of log entries kept in memory per server.
///
/// Client operations (every HTTP request, FTP command, SFTP operation and
/// TFTP transfer) are logged individually, so the buffer needs to be large
/// enough to stay useful.
pub const MAX_LOG_ENTRIES: usize = 1000;

pub type SharedState = Arc<RwLock<ServerState>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stale_run_cannot_overwrite_status() {
        let mut state = ServerState::new(ServerConfig::default());

        let stale_run = state.begin_run("starting server");
        let current_run = state.begin_run("restarting server");

        assert!(!state.is_active_run(stale_run));
        assert!(state.is_active_run(current_run));

        // A task from the previous run finishing after a restart must not
        // clobber the status of the run that is actually serving.
        state.status = ServerStatus::Running;
        if state.is_active_run(stale_run) {
            state.status = ServerStatus::Stopped;
        }

        assert_eq!(state.status, ServerStatus::Running);
    }
}

/// Server control handle
#[allow(dead_code)]
pub struct ServerHandle {
    pub state: SharedState,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl ServerHandle {
    #[allow(dead_code)]
    pub fn new(config: ServerConfig) -> Self {
        Self {
            state: Arc::new(RwLock::new(ServerState::new(config))),
            shutdown_tx: None,
        }
    }

    #[allow(dead_code)]
    pub fn set_shutdown_tx(&mut self, tx: mpsc::Sender<()>) {
        self.shutdown_tx = Some(tx);
    }

    #[allow(dead_code)]
    pub fn request_shutdown(&self) -> bool {
        if let Some(tx) = &self.shutdown_tx {
            tx.try_send(()).is_ok()
        } else {
            false
        }
    }

    #[allow(dead_code)]
    pub fn status(&self) -> ServerStatus {
        self.state.read().status.clone()
    }

    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        matches!(self.state.read().status, ServerStatus::Running)
    }
}
