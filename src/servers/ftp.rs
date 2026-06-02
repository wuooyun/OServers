//! FTP Server implementation using libunftp

use super::{LogMessage, ServerConfig, ServerError, ServerHandle, ServerStatus, SharedState};
use libunftp::auth::DefaultUser;
use libunftp::options::ActivePassiveMode;
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
        s.add_log(LogMessage::info(format!(
            "Starting FTP server on port {}...",
            port
        )));
    }

    // Create authenticator
    let authenticator = SimpleAuthenticator {
        username: config.username.clone(),
        password: config.password.clone(),
        allow_anonymous: config.anonymous_access,
    };

    // Determine transfer mode
    let transfer_mode = if config.passive_mode {
        ActivePassiveMode::ActiveAndPassive
    } else {
        ActivePassiveMode::ActiveOnly
    };

    // Build server with transfer mode
    let server = libunftp::Server::with_fs(root.clone())
        .authenticator(Arc::new(authenticator))
        .passive_ports(config.passive_ports.0..=config.passive_ports.1)
        .active_passive_mode(transfer_mode)
        .build()
        .map_err(|e| ServerError::Other(e.to_string()))?;

    let addr = format!("0.0.0.0:{}", port);

    // Update status to running
    {
        let mut s = state.write();
        s.status = ServerStatus::Running;
        s.add_log(LogMessage::info(format!(
            "FTP server started on ftp://0.0.0.0:{}",
            port
        )));
        s.add_log(LogMessage::info(format!(
            "Root directory: {}",
            root.display()
        )));
        if config.anonymous_access {
            s.add_log(LogMessage::info("Anonymous access: enabled"));
        }
        let mode_desc = match transfer_mode {
            ActivePassiveMode::ActiveAndPassive => "Active + Passive",
            ActivePassiveMode::PassiveOnly => "Passive only",
            ActivePassiveMode::ActiveOnly => "Active only",
        };
        s.add_log(LogMessage::info(format!(
            "Transfer mode: {} (passive ports: {}-{})",
            mode_desc, config.passive_ports.0, config.passive_ports.1
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
#[allow(dead_code)]
pub fn create_handle(config: FtpConfig) -> ServerHandle {
    ServerHandle::new(config.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use crate::servers::{ServerConfig, ServerState};

    async fn read_line(stream: &mut TcpStream) -> String {
        let mut line = Vec::new();
        let mut buf = [0u8; 1];
        loop {
            match stream.read_exact(&mut buf).await {
                Ok(_) => {
                    line.push(buf[0]);
                    if buf[0] == b'\n' {
                        break;
                    }
                }
                Err(e) => panic!("failed to read line: {}", e),
            }
        }
        String::from_utf8_lossy(&line).into_owned()
    }

    async fn read_ftp_response(stream: &mut TcpStream) -> (u16, String) {
        loop {
            let line = read_line(stream).await;
            if line.len() >= 4 {
                let code_str = &line[0..3];
                let space_char = &line[3..4];
                if code_str.chars().all(|c| c.is_ascii_digit()) && space_char == " " {
                    let code = code_str.parse::<u16>().unwrap();
                    return (code, line);
                }
            }
        }
    }

    fn get_free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[tokio::test]
    async fn test_ftp_put() {
        let port = get_free_port();
        let test_dir = std::env::temp_dir().join(format!(
            "ftp_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        std::fs::create_dir_all(&test_dir).unwrap();

        let state = Arc::new(parking_lot::RwLock::new(ServerState::new(ServerConfig {
            root_dir: test_dir.clone(),
            port,
            auto_stop_seconds: None,
        })));

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);

        let config = FtpConfig {
            root_dir: test_dir.clone(),
            port,
            username: "admin".to_string(),
            password: "admin".to_string(),
            anonymous_access: false,
            passive_mode: true,
            passive_ports: (50000, 50100),
        };

        let state_clone = state.clone();
        let server_handle = tokio::spawn(async move {
            start_server(config, state_clone, shutdown_rx).await
        });

        // Wait for the server to start
        let mut started = false;
        for _ in 0..50 {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            if matches!(state.read().status, ServerStatus::Running) {
                started = true;
                break;
            }
        }
        assert!(started, "FTP Server failed to start");

        // Connect to control port
        let mut control_stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        // 1. Read greeting
        let (code, _) = read_ftp_response(&mut control_stream).await;
        assert_eq!(code, 220);

        // 2. Send USER
        control_stream.write_all(b"USER admin\r\n").await.unwrap();
        let (code, _) = read_ftp_response(&mut control_stream).await;
        assert_eq!(code, 331);

        // 3. Send PASS
        control_stream.write_all(b"PASS admin\r\n").await.unwrap();
        let (code, _) = read_ftp_response(&mut control_stream).await;
        assert_eq!(code, 230);

        // 4. Send PASV
        control_stream.write_all(b"PASV\r\n").await.unwrap();
        let (code, resp) = read_ftp_response(&mut control_stream).await;
        assert_eq!(code, 227);

        // Parse passive port
        let start_idx = resp.find('(').expect("invalid PASV response");
        let end_idx = resp.find(')').expect("invalid PASV response");
        let parts: Vec<&str> = resp[start_idx + 1..end_idx].split(',').collect();
        assert_eq!(parts.len(), 6);
        let p1: u16 = parts[4].parse().unwrap();
        let p2: u16 = parts[5].parse().unwrap();
        let passive_port = (p1 << 8) + p2;

        // 5. Send STOR
        control_stream
            .write_all(b"STOR test_file.txt\r\n")
            .await
            .unwrap();

        // Connect data connection
        let mut data_stream = TcpStream::connect(format!("127.0.0.1:{}", passive_port))
            .await
            .unwrap();

        // Read 150
        let (code, _) = read_ftp_response(&mut control_stream).await;
        assert!(code == 150 || code == 125);

        // 6. Write data
        let file_content = b"Hello OServers FTP Server Test!";
        data_stream.write_all(file_content).await.unwrap();
        data_stream.shutdown().await.unwrap();

        // 7. Read 226
        let (code, _) = read_ftp_response(&mut control_stream).await;
        assert_eq!(code, 226);

        // 8. Send QUIT
        control_stream.write_all(b"QUIT\r\n").await.unwrap();
        let (code, _) = read_ftp_response(&mut control_stream).await;
        assert_eq!(code, 221);

        // Verify file content
        let expected_file_path = test_dir.join("test_file.txt");
        assert!(expected_file_path.exists());
        let content = std::fs::read(&expected_file_path).unwrap();
        assert_eq!(content, file_content);

        // Stop the server
        shutdown_tx.send(()).await.unwrap();
        let _ = server_handle.await;

        // Clean up
        let _ = std::fs::remove_dir_all(&test_dir);
    }
}
