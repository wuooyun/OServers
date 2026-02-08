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
pub struct ServerState {
    pub status: ServerStatus,
    pub logs: Vec<LogMessage>,
    pub config: ServerConfig,
}

impl ServerState {
    pub fn new(config: ServerConfig) -> Self {
        Self {
            status: ServerStatus::Stopped,
            logs: Vec::new(),
            config,
        }
    }

    pub fn add_log(&mut self, msg: LogMessage) {
        self.logs.push(msg);
        // Keep only last 100 messages
        if self.logs.len() > 100 {
            self.logs.remove(0);
        }
    }
}

pub type SharedState = Arc<RwLock<ServerState>>;

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
