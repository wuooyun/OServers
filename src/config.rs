//! Application configuration management

use crate::servers::{ftp::FtpConfig, http::HttpConfig, ssh::SshConfig, tftp::TftpConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main application configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    pub http: HttpConfig,
    pub ftp: FtpConfig,
    pub tftp: TftpConfig,
    pub ssh: SshConfig,
}

impl AppConfig {
    /// Load configuration from file
    pub fn load() -> Self {
        let config_path = Self::config_path();
        if config_path.exists() {
            match std::fs::read_to_string(&config_path) {
                Ok(content) => match serde_json::from_str(&content) {
                    Ok(config) => return config,
                    Err(e) => {
                        tracing::warn!("Failed to parse config: {}", e);
                    }
                },
                Err(e) => {
                    tracing::warn!("Failed to read config: {}", e);
                }
            }
        }
        Self::default()
    }

    /// Save configuration to file
    pub fn save(&self) -> anyhow::Result<()> {
        let config_path = Self::config_path();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }

    /// Get configuration file path
    fn config_path() -> PathBuf {
        if let Some(proj_dirs) = directories::ProjectDirs::from("com", "oservers", "oservers") {
            proj_dirs.config_dir().join("config.json")
        } else {
            PathBuf::from("config.json")
        }
    }
}
