//! FTP Server implementation using libunftp

use super::{LogMessage, ServerConfig, ServerError, ServerHandle, ServerStatus, SharedState};
use libunftp::auth::{AuthenticationError, Authenticator, Credentials, UserDetail};
use libunftp::options::ActivePassiveMode;
use libunftp::storage::{Fileinfo, Result as StorageResult, StorageBackend};
use std::fmt::{Debug, Display, Formatter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use unftp_sbe_fs::Filesystem;

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

/// Authenticated FTP user, carrying the client IP so that every storage
/// operation can be logged together with the originating client.
#[derive(Debug, Clone)]
struct FtpUser {
    username: String,
    client_ip: std::net::IpAddr,
}

impl Display for FtpUser {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.username)
    }
}

impl UserDetail for FtpUser {}

/// Simple authenticator for FTP
#[derive(Debug, Clone)]
struct SimpleAuthenticator {
    username: String,
    password: String,
    allow_anonymous: bool,
    state: SharedState,
}

#[async_trait::async_trait]
impl Authenticator<FtpUser> for SimpleAuthenticator {
    async fn authenticate(
        &self,
        username: &str,
        creds: &Credentials,
    ) -> Result<FtpUser, AuthenticationError> {
        let client_ip = creds.source_ip;

        // Allow anonymous if enabled
        if self.allow_anonymous && username == "anonymous" {
            self.state.write().add_log(LogMessage::info(format!(
                "[FTP] client={} Anonymous user logged in successfully",
                client_ip
            )));
            return Ok(FtpUser {
                username: username.to_string(),
                client_ip,
            });
        }

        // Check username and password
        if let Some(password) = creds.password.as_ref() {
            if username == self.username && password == &self.password {
                self.state.write().add_log(LogMessage::info(format!(
                    "[FTP] client={} User '{}' logged in successfully from {}",
                    client_ip, username, client_ip
                )));
                return Ok(FtpUser {
                    username: username.to_string(),
                    client_ip,
                });
            }
        }

        self.state.write().add_log(LogMessage::error(format!(
            "[FTP] client={} Failed login attempt for user '{}'",
            client_ip, username
        )));
        Err(AuthenticationError::BadPassword)
    }
}

/// Storage backend wrapper that logs every FTP storage operation
/// (LIST/NLST, RETR, STOR, DELE, MKD, RMD, RNFR/RNTO, CWD, ...)
/// together with the client IP and user name.
#[derive(Debug, Clone)]
struct LoggingFs {
    inner: Arc<Filesystem>,
    state: SharedState,
}

impl LoggingFs {
    fn log_ok(&self, user: &FtpUser, command: &str, detail: &str) {
        self.state.write().add_log(LogMessage::info(format!(
            "[FTP] client={} user={} {} {}",
            user.client_ip, user.username, command, detail
        )));
    }

    fn log_fail(&self, user: &FtpUser, command: &str, detail: &str, err: &libunftp::storage::Error) {
        self.state.write().add_log(LogMessage::error(format!(
            "[FTP] client={} user={} {} {} failed: {}",
            user.client_ip, user.username, command, detail, err
        )));
    }
}

#[async_trait::async_trait]
impl StorageBackend<FtpUser> for LoggingFs {
    type Metadata = <Filesystem as StorageBackend<FtpUser>>::Metadata;

    fn name(&self) -> &str {
        <Filesystem as StorageBackend<FtpUser>>::name(&self.inner)
    }

    fn supported_features(&self) -> u32 {
        <Filesystem as StorageBackend<FtpUser>>::supported_features(&self.inner)
    }

    async fn metadata<P: AsRef<Path> + Send + Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> StorageResult<Self::Metadata> {
        let detail = path.as_ref().display().to_string();
        let result = self.inner.metadata(user, path).await;
        match &result {
            Ok(_) => self.log_ok(user, "STAT/SIZE", &detail),
            Err(e) => self.log_fail(user, "STAT/SIZE", &detail, e),
        }
        result
    }

    async fn list<P: AsRef<Path> + Send + Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> StorageResult<Vec<Fileinfo<std::path::PathBuf, Self::Metadata>>> {
        let detail = path.as_ref().display().to_string();
        let result = self.inner.list(user, path).await;
        match &result {
            Ok(entries) => self.log_ok(
                user,
                "LIST/NLST",
                &format!("{} ({} entries)", detail, entries.len()),
            ),
            Err(e) => self.log_fail(user, "LIST/NLST", &detail, e),
        }
        result
    }

    async fn get<P: AsRef<Path> + Send + Debug>(
        &self,
        user: &FtpUser,
        path: P,
        start_pos: u64,
    ) -> StorageResult<Box<dyn tokio::io::AsyncRead + Send + Sync + Unpin>> {
        let detail = if start_pos > 0 {
            format!("{} (from offset {})", path.as_ref().display(), start_pos)
        } else {
            path.as_ref().display().to_string()
        };
        let result = self.inner.get(user, path, start_pos).await;
        match &result {
            Ok(_) => self.log_ok(user, "RETR", &detail),
            Err(e) => self.log_fail(user, "RETR", &detail, e),
        }
        result
    }

    async fn put<P: AsRef<Path> + Send + Debug, R: tokio::io::AsyncRead + Send + Sync + Unpin + 'static>(
        &self,
        user: &FtpUser,
        input: R,
        path: P,
        start_pos: u64,
    ) -> StorageResult<u64> {
        let detail = path.as_ref().display().to_string();
        let result = self.inner.put(user, input, path, start_pos).await;
        match &result {
            Ok(bytes) => self.log_ok(user, "STOR", &format!("{} ({} bytes)", detail, bytes)),
            Err(e) => self.log_fail(user, "STOR", &detail, e),
        }
        result
    }

    async fn del<P: AsRef<Path> + Send + Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> StorageResult<()> {
        let detail = path.as_ref().display().to_string();
        let result = self.inner.del(user, path).await;
        match &result {
            Ok(_) => self.log_ok(user, "DELE", &detail),
            Err(e) => self.log_fail(user, "DELE", &detail, e),
        }
        result
    }

    async fn mkd<P: AsRef<Path> + Send + Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> StorageResult<()> {
        let detail = path.as_ref().display().to_string();
        let result = self.inner.mkd(user, path).await;
        match &result {
            Ok(_) => self.log_ok(user, "MKD", &detail),
            Err(e) => self.log_fail(user, "MKD", &detail, e),
        }
        result
    }

    async fn rename<P: AsRef<Path> + Send + Debug>(
        &self,
        user: &FtpUser,
        from: P,
        to: P,
    ) -> StorageResult<()> {
        let detail = format!("{} -> {}", from.as_ref().display(), to.as_ref().display());
        let result = self.inner.rename(user, from, to).await;
        match &result {
            Ok(_) => self.log_ok(user, "RNFR/RNTO", &detail),
            Err(e) => self.log_fail(user, "RNFR/RNTO", &detail, e),
        }
        result
    }

    async fn rmd<P: AsRef<Path> + Send + Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> StorageResult<()> {
        let detail = path.as_ref().display().to_string();
        let result = self.inner.rmd(user, path).await;
        match &result {
            Ok(_) => self.log_ok(user, "RMD", &detail),
            Err(e) => self.log_fail(user, "RMD", &detail, e),
        }
        result
    }

    async fn cwd<P: AsRef<Path> + Send + Debug>(
        &self,
        user: &FtpUser,
        path: P,
    ) -> StorageResult<()> {
        let detail = path.as_ref().display().to_string();
        let result = self.inner.cwd(user, path).await;
        match &result {
            Ok(_) => self.log_ok(user, "CWD", &detail),
            Err(e) => self.log_fail(user, "CWD", &detail, e),
        }
        result
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
    let run_id = state.write().begin_run(format!(
        "Starting FTP server on port {}...",
        port
    ));

    // Create authenticator
    let authenticator = SimpleAuthenticator {
        username: config.username.clone(),
        password: config.password.clone(),
        allow_anonymous: config.anonymous_access,
        state: state.clone(),
    };

    // Determine transfer mode
    let transfer_mode = if config.passive_mode {
        ActivePassiveMode::ActiveAndPassive
    } else {
        ActivePassiveMode::ActiveOnly
    };

    // Wrap the filesystem backend so every client operation is logged
    let filesystem = match Filesystem::new(root.clone()) {
        Ok(filesystem) => filesystem,
        Err(e) => {
            let mut s = state.write();
            if s.is_active_run(run_id) {
                s.status = ServerStatus::Error(e.to_string());
                s.add_log(LogMessage::error(format!("FTP storage error: {}", e)));
            }
            return Err(ServerError::Other(e.to_string()));
        }
    };
    let logging_fs = LoggingFs {
        inner: Arc::new(filesystem),
        state: state.clone(),
    };

    // Build server with transfer mode
    let server = match libunftp::ServerBuilder::<LoggingFs, FtpUser>::with_authenticator(
        Box::new(move || logging_fs.clone()),
        Arc::new(authenticator),
    )
    .passive_ports(config.passive_ports.0..=config.passive_ports.1)
    .active_passive_mode(transfer_mode)
    .build()
    {
        Ok(server) => server,
        Err(e) => {
            let mut s = state.write();
            if s.is_active_run(run_id) {
                s.status = ServerStatus::Error(e.to_string());
                s.add_log(LogMessage::error(format!("FTP server error: {}", e)));
            }
            return Err(ServerError::Other(e.to_string()));
        }
    };

    let addr = format!("0.0.0.0:{}", port);

    // Update status to running
    {
        let mut s = state.write();
        if s.is_active_run(run_id) {
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
    }

    // Run server with shutdown signal
    tokio::select! {
        result = server.listen(addr) => {
            if let Err(e) = result {
                let mut s = state.write();
                if s.is_active_run(run_id) {
                    s.status = ServerStatus::Error(e.to_string());
                    s.add_log(LogMessage::error(format!("FTP server error: {}", e)));
                }
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
        if s.is_active_run(run_id) {
            s.status = ServerStatus::Stopped;
            s.add_log(LogMessage::info("FTP server stopped"));
        }
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

        // Verify logs contain the user login event with source IP
        let logs = state.read().logs.clone();
        assert!(
            logs.iter()
                .any(|l| l.message.contains("User 'admin' logged in successfully from 127.0.0.1")),
            "Expected login log message with source IP, found: {:?}",
            logs
        );

        // Stop the server
        shutdown_tx.send(()).await.unwrap();
        let _ = server_handle.await;

        // Clean up
        let _ = std::fs::remove_dir_all(&test_dir);
    }
}
