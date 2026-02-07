//! FTP Server implementation using libunftp

use super::{LogMessage, ServerConfig, ServerError, ServerHandle, ServerStatus, SharedState};
use libunftp::auth::DefaultUser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use unftp_sbe_fs::ServerExt;

/// FTP server specific configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FtpConfig {
    pub root_dir: PathBuf,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub anonymous_access: bool,
    pub passive_mode: bool,
    pub passive_ports: (u16, u16),
}

impl Default for FtpConfig {
    fn default() -> Self {
        Self {
            root_dir: std::env::current_dir().unwrap_or_default(),
            port: 2121,
            username: "admin".to_string(),
            password: "admin".to_string(),
            anonymous_access: true,
            passive_mode: true,
            passive_ports: (50000, 50100),
        }
    }
}

impl From<FtpConfig> for ServerConfig {
    fn from(cfg: FtpConfig) -> Self {
        ServerConfig {
            root_dir: cfg.root_dir,
            port: cfg.port,
            auto_stop_seconds: None,
        }
    }
}

/// Simple authenticator for FTP
#[derive(Debug, Clone)]
struct SimpleAuthenticator {
    username: String,
    password: String,
    allow_anonymous: bool,
}

#[async_trait::async_trait]
impl libunftp::auth::Authenticator<DefaultUser> for SimpleAuthenticator {
    async fn authenticate(
        &self,
        username: &str,
        creds: &libunftp::auth::Credentials,
    ) -> Result<DefaultUser, libunftp::auth::AuthenticationError> {
        // Allow anonymous if enabled
        if self.allow_anonymous && username == "anonymous" {
            return Ok(DefaultUser);
        }

        // Check username and password
        if let Some(password) = creds.password.as_ref() {
            if username == self.username && password == &self.password {
                return Ok(DefaultUser);
            }
        }
        Err(libunftp::auth::AuthenticationError::BadPassword)
    }
}

/// Start FTP server
pub async fn start_server(
    config: FtpConfig,
    state: SharedState,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> Result<(), ServerError> {
    let root = config.root_dir.clone();
    let port = config.port;

    // Update status
    {
        let mut s = state.write();
        s.status = ServerStatus::Starting;
        s.add_log(LogMessage::info(format!("Starting FTP server on port {}...", port)));
    }

    // Create authenticator
    let authenticator = SimpleAuthenticator {
        username: config.username.clone(),
        password: config.password.clone(),
        allow_anonymous: config.anonymous_access,
    };

    // Build server
    let server = libunftp::Server::with_fs(root.clone())
        .authenticator(Arc::new(authenticator))
        .passive_ports(config.passive_ports.0..config.passive_ports.1)
        .build()
        .map_err(|e| ServerError::Other(e.to_string()))?;

    let addr = format!("0.0.0.0:{}", port);

    // Update status to running
    {
        let mut s = state.write();
        s.status = ServerStatus::Running;
        s.add_log(LogMessage::info(format!("FTP server started on ftp://0.0.0.0:{}", port)));
        s.add_log(LogMessage::info(format!("Root directory: {}", root.display())));
        if config.anonymous_access {
            s.add_log(LogMessage::info("Anonymous access: enabled"));
        }
        s.add_log(LogMessage::info(format!(
            "Mode: {} (passive ports: {}-{})",
            if config.passive_mode { "Passive" } else { "Active" },
            config.passive_ports.0,
            config.passive_ports.1
        )));
    }

    // Run server with shutdown signal
    tokio::select! {
        result = server.listen(addr) => {
            if let Err(e) = result {
                let mut s = state.write();
                s.status = ServerStatus::Error(e.to_string());
                s.add_log(LogMessage::error(format!("FTP server error: {}", e)));
                return Err(ServerError::Other(e.to_string()));
            }
        }
        _ = shutdown_rx.recv() => {
            // Shutdown requested
        }
    }

    // Update status
    {
        let mut s = state.write();
        s.status = ServerStatus::Stopped;
        s.add_log(LogMessage::info("FTP server stopped"));
    }

    Ok(())
}

/// Create a new FTP server handle
pub fn create_handle(config: FtpConfig) -> ServerHandle {
    ServerHandle::new(config.into())
}
