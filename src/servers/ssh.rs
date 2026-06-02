//! SSH/SFTP Server implementation using russh and russh-sftp

use super::{LogMessage, ServerConfig, ServerError, ServerHandle, ServerStatus, SharedState};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::fs::File as TokioFile;
use russh::{Channel, ChannelId};
use russh::server::{Auth, Msg, Session, Server};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};

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
            "Starting SSH/SFTP server on port {}...",
            port
        )));
    }

    // Get host keys
    let host_keys = match get_host_keys(&state) {
        Ok(keys) => keys,
        Err(e) => {
            let mut s = state.write();
            s.status = ServerStatus::Error(e.to_string());
            s.add_log(LogMessage::error(format!("Host key error: {}", e)));
            return Err(e);
        }
    };

    // Create custom preferred algorithms list including legacy algorithms
    // for wide compatibility with legacy devices (e.g. Cisco switches)
    let mut preferred = russh::Preferred::default();

    // Enable Diffie-Hellman Group 1 SHA1, Group 14 SHA1, and Group Exchange SHA1
    let mut kex = preferred.kex.into_owned();
    kex.push(russh::kex::DH_GEX_SHA1);
    kex.push(russh::kex::DH_G1_SHA1);
    kex.push(russh::kex::DH_G14_SHA1);
    preferred.kex = std::borrow::Cow::Owned(kex);

    // Enable AES-CBC ciphers
    let mut ciphers = preferred.cipher.into_owned();
    ciphers.push(russh::cipher::AES_128_CBC);
    ciphers.push(russh::cipher::AES_192_CBC);
    ciphers.push(russh::cipher::AES_256_CBC);
    preferred.cipher = std::borrow::Cow::Owned(ciphers);

    // Enable SHA1 MACs
    let mut macs = preferred.mac.into_owned();
    macs.push(russh::mac::HMAC_SHA1);
    macs.push(russh::mac::HMAC_SHA1_ETM);
    preferred.mac = std::borrow::Cow::Owned(macs);

    // SSH Config
    let ssh_config = russh::server::Config {
        auth_rejection_time: std::time::Duration::from_secs(1),
        auth_rejection_time_initial: Some(std::time::Duration::from_secs(0)),
        keys: host_keys,
        preferred,
        ..Default::default()
    };

    let mut server = MySshServer {
        config: config.clone(),
        state: state.clone(),
    };

    let addr = format!("0.0.0.0:{}", port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            let mut s = state.write();
            s.status = ServerStatus::Error(e.to_string());
            s.add_log(LogMessage::error(format!("Failed to bind port {}: {}", port, e)));
            return Err(ServerError::IoError(e));
        }
    };

    let running_server = server.run_on_socket(Arc::new(ssh_config), &listener);
    let server_handle = running_server.handle();

    // Update status to running
    {
        let mut s = state.write();
        s.status = ServerStatus::Running;
        s.add_log(LogMessage::info(format!(
            "SSH/SFTP server started on sftp://0.0.0.0:{}",
            port
        )));
        s.add_log(LogMessage::info(format!(
            "Root directory: {}",
            config.root_dir.display()
        )));
        s.add_log(LogMessage::info(format!("Username: {}", config.username)));
        s.add_log(LogMessage::info("Legacy algorithm compatibility: enabled (for traditional switches/servers)"));
    }

    tokio::select! {
        res = running_server => {
            if let Err(e) = res {
                let mut s = state.write();
                s.status = ServerStatus::Error(e.to_string());
                s.add_log(LogMessage::error(format!("SSH/SFTP server error: {}", e)));
                return Err(ServerError::IoError(e));
            }
        }
        _ = shutdown_rx.recv() => {
            server_handle.shutdown("Server stopping".to_string());
        }
    }

    // Update status
    {
        let mut s = state.write();
        s.status = ServerStatus::Stopped;
        s.add_log(LogMessage::info("SSH/SFTP server stopped"));
    }

    Ok(())
}

/// Create a new SSH server handle
#[allow(dead_code)]
pub fn create_handle(config: SshConfig) -> ServerHandle {
    ServerHandle::new(config.into())
}

fn get_host_keys(state: &SharedState) -> Result<Vec<ssh_key::PrivateKey>, ServerError> {
    let mut key_dir = None;
    if let Some(proj_dirs) = directories::ProjectDirs::from("com", "oservers", "oservers") {
        key_dir = Some(proj_dirs.config_dir().to_path_buf());
    }

    let key_dir = if let Some(dir) = key_dir {
        let _ = std::fs::create_dir_all(&dir);
        dir
    } else {
        PathBuf::from(".")
    };

    let ed25519_path = key_dir.join("ssh_key_ed25519.pem");
    let rsa_path = key_dir.join("ssh_key_rsa.pem");

    // Legacy migration: if the old ssh_key.pem exists, rename it to ssh_key_ed25519.pem
    let old_path = key_dir.join("ssh_key.pem");
    if old_path.exists() && !ed25519_path.exists() {
        let _ = std::fs::rename(&old_path, &ed25519_path);
    }

    let mut keys = Vec::new();

    // Load or generate Ed25519 Key
    let ed25519_key = if ed25519_path.exists() {
        match ssh_key::PrivateKey::read_openssh_file(&ed25519_path) {
            Ok(key) => {
                state.write().add_log(LogMessage::info(format!(
                    "SSH: Loaded Ed25519 host key from {}",
                    ed25519_path.display()
                )));
                key
            }
            Err(e) => {
                state.write().add_log(LogMessage::error(format!(
                    "SSH: Failed to load Ed25519 host key, generating new one: {}",
                    e
                )));
                generate_and_save_ed25519(&ed25519_path, state)?
            }
        }
    } else {
        generate_and_save_ed25519(&ed25519_path, state)?
    };
    keys.push(ed25519_key);

    // Load or generate RSA Key (2048-bit for maximum compatibility with legacy devices like Cisco 2900)
    let rsa_key = if rsa_path.exists() {
        match ssh_key::PrivateKey::read_openssh_file(&rsa_path) {
            Ok(key) => {
                state.write().add_log(LogMessage::info(format!(
                    "SSH: Loaded RSA host key from {}",
                    rsa_path.display()
                )));
                key
            }
            Err(e) => {
                state.write().add_log(LogMessage::error(format!(
                    "SSH: Failed to load RSA host key, generating new one: {}",
                    e
                )));
                generate_and_save_rsa(&rsa_path, state)?
            }
        }
    } else {
        generate_and_save_rsa(&rsa_path, state)?
    };
    keys.push(rsa_key);

    Ok(keys)
}

fn generate_and_save_ed25519(
    path: &std::path::Path,
    state: &SharedState,
) -> Result<ssh_key::PrivateKey, ServerError> {
    let key = ssh_key::PrivateKey::random(&mut rand::rng(), ssh_key::Algorithm::Ed25519)
        .map_err(|e| ServerError::Other(e.to_string()))?;

    if let Err(e) = key.write_openssh_file(path, ssh_key::LineEnding::LF) {
        state.write().add_log(LogMessage::error(format!(
            "SSH: Failed to save Ed25519 host key to {}: {}",
            path.display(),
            e
        )));
    } else {
        state.write().add_log(LogMessage::info(format!(
            "SSH: Generated and saved new Ed25519 host key to {}",
            path.display()
        )));
    }
    Ok(key)
}

fn generate_and_save_rsa(
    path: &std::path::Path,
    state: &SharedState,
) -> Result<ssh_key::PrivateKey, ServerError> {
    let key_data = ssh_key::private::KeypairData::from(
        ssh_key::private::RsaKeypair::random(&mut rand::rng(), 2048)
            .map_err(|e| ServerError::Other(e.to_string()))?,
    );
    let key = ssh_key::PrivateKey::new(key_data, "")
        .map_err(|e| ServerError::Other(e.to_string()))?;

    if let Err(e) = key.write_openssh_file(path, ssh_key::LineEnding::LF) {
        state.write().add_log(LogMessage::error(format!(
            "SSH: Failed to save RSA host key to {}: {}",
            path.display(),
            e
        )));
    } else {
        state.write().add_log(LogMessage::info(format!(
            "SSH: Generated and saved new 2048-bit RSA host key to {}",
            path.display()
        )));
    }
    Ok(key)
}

#[derive(Clone)]
struct MySshServer {
    config: SshConfig,
    state: SharedState,
}

impl Server for MySshServer {
    type Handler = SshSession;

    fn new_client(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
        SshSession::new(self.config.clone(), self.state.clone())
    }
}

struct SshSession {
    config: SshConfig,
    state: SharedState,
    clients: Arc<Mutex<HashMap<ChannelId, Channel<Msg>>>>,
}

impl SshSession {
    fn new(config: SshConfig, state: SharedState) -> Self {
        Self {
            config,
            state,
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn get_channel(&mut self, channel_id: ChannelId) -> Option<Channel<Msg>> {
        let mut clients = self.clients.lock().await;
        clients.remove(&channel_id)
    }
}

impl russh::server::Handler for SshSession {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == self.config.username && password == self.config.password {
            self.state.write().add_log(LogMessage::info(format!(
                "SSH: Successful login for user '{}'",
                user
            )));
            Ok(Auth::Accept)
        } else {
            self.state.write().add_log(LogMessage::error(format!(
                "SSH: Failed login attempt for user '{}'",
                user
            )));
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let mut clients = self.clients.lock().await;
        clients.insert(channel.id(), channel);
        Ok(true)
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let _ = session.close(channel);
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name == "sftp" {
            if let Some(channel) = self.get_channel(channel_id).await {
                session.channel_success(channel_id)?;

                let sftp = SftpSession::new(self.config.root_dir.clone(), self.state.clone());

                self.state.write().add_log(LogMessage::info("SSH: Starting SFTP subsystem"));

                tokio::spawn(async move {
                    russh_sftp::server::run(channel.into_stream(), sftp).await;
                });
            } else {
                let _ = session.channel_failure(channel_id);
            }
        } else {
            let _ = session.channel_failure(channel_id);
        }
        Ok(())
    }
}

struct OpenDir {
    entries: Vec<File>,
}

struct SftpSession {
    root_dir: PathBuf,
    next_handle_id: u64,
    files: HashMap<String, TokioFile>,
    dirs: HashMap<String, OpenDir>,
    version: Option<u32>,
    state: SharedState,
}

impl SftpSession {
    fn new(root_dir: PathBuf, state: SharedState) -> Self {
        Self {
            root_dir,
            next_handle_id: 1,
            files: HashMap::new(),
            dirs: HashMap::new(),
            version: None,
            state,
        }
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf, StatusCode> {
        let path = path.replace('\\', "/");
        let mut components = Vec::new();
        for comp in path.split('/') {
            if comp.is_empty() || comp == "." {
                continue;
            }
            if comp == ".." {
                components.pop();
            } else {
                components.push(comp);
            }
        }

        let mut resolved = self.root_dir.clone();
        for comp in components {
            resolved.push(comp);
        }

        if !resolved.starts_with(&self.root_dir) {
            return Err(StatusCode::PermissionDenied);
        }

        Ok(resolved)
    }
}

fn ok_status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Success".to_string(),
        language_tag: "en-US".to_string(),
    }
}

impl russh_sftp::server::Handler for SftpSession {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        if self.version.is_some() {
            return Err(StatusCode::ConnectionLost);
        }
        self.version = Some(version);
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let cleaned = if path.is_empty() || path == "." {
            "/".to_string()
        } else {
            let mut cleaned = path.replace('\\', "/");
            if !cleaned.starts_with('/') {
                cleaned = format!("/{}", cleaned);
            }
            cleaned
        };
        Ok(Name {
            id,
            files: vec![File::dummy(cleaned)],
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let local_path = self.resolve_path(&path)?;

        let mut dir_entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&local_path)
            .await
            .map_err(|_| StatusCode::NoSuchFile)?;

        if let Ok(metadata) = tokio::fs::metadata(&local_path).await {
            let attrs = FileAttributes::from(&metadata);
            dir_entries.push(File::new(".", attrs.clone()));
            if let Some(parent) = local_path.parent() {
                if let Ok(parent_metadata) = tokio::fs::metadata(parent).await {
                    let parent_attrs = FileAttributes::from(&parent_metadata);
                    dir_entries.push(File::new("..", parent_attrs));
                } else {
                    dir_entries.push(File::new("..", attrs.clone()));
                }
            } else {
                dir_entries.push(File::new("..", attrs.clone()));
            }
        }

        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Ok(metadata) = entry.metadata().await {
                let attrs = FileAttributes::from(&metadata);
                dir_entries.push(File::new(name, attrs));
            }
        }

        let handle_str = format!("d_{}", self.next_handle_id);
        self.next_handle_id += 1;

        self.dirs.insert(
            handle_str.clone(),
            OpenDir {
                entries: dir_entries,
            },
        );

        Ok(Handle {
            id,
            handle: handle_str,
        })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        if let Some(open_dir) = self.dirs.get_mut(&handle) {
            if open_dir.entries.is_empty() {
                Err(StatusCode::Eof)
            } else {
                let files = std::mem::take(&mut open_dir.entries);
                Ok(Name { id, files })
            }
        } else {
            Err(StatusCode::NoSuchFile)
        }
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let local_path = self.resolve_path(&path)?;
        let metadata = tokio::fs::metadata(&local_path)
            .await
            .map_err(|_| StatusCode::NoSuchFile)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&metadata),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let local_path = self.resolve_path(&path)?;
        let metadata = tokio::fs::symlink_metadata(&local_path)
            .await
            .map_err(|_| StatusCode::NoSuchFile)?;
        Ok(Attrs {
            id,
            attrs: FileAttributes::from(&metadata),
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        if let Some(file) = self.files.get(&handle) {
            let metadata = file.metadata().await.map_err(|_| StatusCode::Failure)?;
            Ok(Attrs {
                id,
                attrs: FileAttributes::from(&metadata),
            })
        } else {
            Err(StatusCode::NoSuchFile)
        }
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = self.resolve_path(&filename)?;

        let open_options: std::fs::OpenOptions = pflags.into();
        let path_clone = path.clone();

        let std_file = tokio::task::spawn_blocking(move || open_options.open(path_clone))
            .await
            .map_err(|_| StatusCode::Failure)?
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
                std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
                _ => StatusCode::Failure,
            })?;

        let file = TokioFile::from_std(std_file);

        let handle_str = format!("f_{}", self.next_handle_id);
        self.next_handle_id += 1;

        self.files.insert(handle_str.clone(), file);

        self.state.write().add_log(LogMessage::info(format!(
            "SFTP: Opened file '{}'",
            filename
        )));

        Ok(Handle {
            id,
            handle: handle_str,
        })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        if let Some(file) = self.files.get_mut(&handle) {
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|_| StatusCode::Failure)?;
            let mut buf = vec![0u8; len as usize];
            let n = file.read(&mut buf).await.map_err(|_| StatusCode::Failure)?;
            if n == 0 {
                return Err(StatusCode::Eof);
            }
            buf.truncate(n);
            Ok(Data { id, data: buf })
        } else {
            Err(StatusCode::NoSuchFile)
        }
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        use tokio::io::{AsyncSeekExt, AsyncWriteExt};
        if let Some(file) = self.files.get_mut(&handle) {
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|_| StatusCode::Failure)?;
            file.write_all(&data)
                .await
                .map_err(|_| StatusCode::Failure)?;
            Ok(ok_status(id))
        } else {
            Err(StatusCode::NoSuchFile)
        }
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        if self.files.remove(&handle).is_some() || self.dirs.remove(&handle).is_some() {
            Ok(ok_status(id))
        } else {
            Err(StatusCode::NoSuchFile)
        }
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let path = self.resolve_path(&filename)?;
        tokio::fs::remove_file(&path).await.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
            std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
            _ => StatusCode::Failure,
        })?;

        self.state.write().add_log(LogMessage::info(format!(
            "SFTP: Removed file '{}'",
            filename
        )));

        Ok(ok_status(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let local_path = self.resolve_path(&path)?;
        tokio::fs::create_dir(&local_path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
                std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
                _ => StatusCode::Failure,
            })?;

        self.state.write().add_log(LogMessage::info(format!(
            "SFTP: Created directory '{}'",
            path
        )));

        Ok(ok_status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let local_path = self.resolve_path(&path)?;
        tokio::fs::remove_dir(&local_path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
                std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
                _ => StatusCode::Failure,
            })?;

        self.state.write().add_log(LogMessage::info(format!(
            "SFTP: Removed directory '{}'",
            path
        )));

        Ok(ok_status(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let old_local = self.resolve_path(&oldpath)?;
        let new_local = self.resolve_path(&newpath)?;
        tokio::fs::rename(&old_local, &new_local)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
                std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
                _ => StatusCode::Failure,
            })?;

        self.state.write().add_log(LogMessage::info(format!(
            "SFTP: Renamed '{}' to '{}'",
            oldpath, newpath
        )));

        Ok(ok_status(id))
    }

    async fn setstat(
        &mut self,
        id: u32,
        _path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        Ok(ok_status(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        _handle: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        Ok(ok_status(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use crate::servers::{ServerConfig, ServerState};

    struct TestClient;

    impl russh::client::Handler for TestClient {
        type Error = russh::Error;

        async fn check_server_key(
            &mut self,
            _server_public_key: &russh::keys::PublicKey,
        ) -> Result<bool, Self::Error> {
            Ok(true)
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
    async fn test_sftp_server_operations() {
        let port = get_free_port();
        let test_dir = std::env::temp_dir().join(format!(
            "sftp_test_{}",
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

        let config = SshConfig {
            root_dir: test_dir.clone(),
            port,
            username: "admin".to_string(),
            password: "admin".to_string(),
        };

        let state_clone = state.clone();
        let server_handle = tokio::spawn(async move {
            start_server(config, state_clone, shutdown_rx).await
        });

        // Wait for the server to start
        let mut started = false;
        for _ in 0..50 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            if matches!(state.read().status, ServerStatus::Running) {
                started = true;
                break;
            }
        }
        assert!(started, "SFTP Server failed to start");

        // Connect to port
        let client_config = russh::client::Config::default();
        let sh = TestClient;
        let mut session = russh::client::connect(Arc::new(client_config), ("127.0.0.1", port), sh)
            .await
            .unwrap();

        let auth_res = session.authenticate_password("admin", "admin").await.unwrap();
        assert!(auth_res.success(), "SFTP Authentication failed");

        let channel = session.channel_open_session().await.unwrap();
        channel.request_subsystem(true, "sftp").await.unwrap();

        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .unwrap();

        // 1. Create directory
        let sub_dir = "sub_dir";
        sftp.create_dir(sub_dir).await.unwrap();
        assert!(test_dir.join(sub_dir).exists());
        assert!(test_dir.join(sub_dir).is_dir());

        // 2. Write file
        let file_name = "test_sftp.txt";
        let file_content = b"Hello from SFTP client test!";
        let mut file = sftp
            .open_with_flags(
                file_name,
                OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ,
            )
            .await
            .unwrap();

        file.write_all(file_content).await.unwrap();
        file.shutdown().await.unwrap();

        let expected_file_path = test_dir.join(file_name);
        assert!(expected_file_path.exists());
        let content = std::fs::read(&expected_file_path).unwrap();
        assert_eq!(content, file_content);

        // 3. Read file
        let mut file_read = sftp.open(file_name).await.unwrap();
        let mut read_content = Vec::new();
        file_read.read_to_end(&mut read_content).await.unwrap();
        assert_eq!(read_content, file_content);
        file_read.shutdown().await.unwrap();

        // 4. List directory
        let entries = sftp.read_dir(".").await.unwrap();
        let file_names: Vec<String> = entries
            .into_iter()
            .map(|e| e.file_name().to_string())
            .collect();
        assert!(file_names.contains(&file_name.to_string()));
        assert!(file_names.contains(&sub_dir.to_string()));

        // 5. Clean up via SFTP
        sftp.remove_file(file_name).await.unwrap();
        assert!(!expected_file_path.exists());

        sftp.remove_dir(sub_dir).await.unwrap();
        assert!(!test_dir.join(sub_dir).exists());

        // Stop the server
        shutdown_tx.send(()).await.unwrap();
        let _ = server_handle.await;

        // Clean up temp dir
        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[tokio::test]
    async fn test_sftp_legacy_client_operations() {
        let port = get_free_port();
        let test_dir = std::env::temp_dir().join(format!(
            "sftp_legacy_test_{}",
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

        let config = SshConfig {
            root_dir: test_dir.clone(),
            port,
            username: "admin".to_string(),
            password: "admin".to_string(),
        };

        let state_clone = state.clone();
        let server_handle = tokio::spawn(async move {
            start_server(config, state_clone, shutdown_rx).await
        });

        // Wait for the server to start
        let mut started = false;
        for _ in 0..50 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            if matches!(state.read().status, ServerStatus::Running) {
                started = true;
                break;
            }
        }
        assert!(started, "SFTP Server failed to start");

        // Connect using a client restricted strictly to legacy algorithms
        let mut client_config = russh::client::Config::default();
        let mut client_preferred = russh::Preferred::default();
        
        // Only allow DH Group 14 SHA1
        client_preferred.kex = std::borrow::Cow::Owned(vec![russh::kex::DH_G14_SHA1]);
        
        // Only allow AES-128-CBC
        client_preferred.cipher = std::borrow::Cow::Owned(vec![russh::cipher::AES_128_CBC]);
        
        // Only allow HMAC-SHA1
        client_preferred.mac = std::borrow::Cow::Owned(vec![russh::mac::HMAC_SHA1]);
        
        // Only allow ssh-rsa signature (SHA1)
        client_preferred.key = std::borrow::Cow::Owned(vec![ssh_key::Algorithm::Rsa { hash: None }]);
        
        client_config.preferred = client_preferred;

        let sh = TestClient;
        let mut session = russh::client::connect(Arc::new(client_config), ("127.0.0.1", port), sh)
            .await
            .unwrap();

        let auth_res = session.authenticate_password("admin", "admin").await.unwrap();
        assert!(auth_res.success(), "SFTP Legacy Authentication failed");

        let channel = session.channel_open_session().await.unwrap();
        channel.request_subsystem(true, "sftp").await.unwrap();

        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .unwrap();

        // Write a test file
        let file_name = "test_legacy.txt";
        let file_content = b"Legacy SFTP test data!";
        let mut file = sftp
            .open_with_flags(
                file_name,
                OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::READ,
            )
            .await
            .unwrap();

        file.write_all(file_content).await.unwrap();
        file.shutdown().await.unwrap();

        let expected_file_path = test_dir.join(file_name);
        assert!(expected_file_path.exists());
        let content = std::fs::read(&expected_file_path).unwrap();
        assert_eq!(content, file_content);

        // Stop the server
        shutdown_tx.send(()).await.unwrap();
        let _ = server_handle.await;

        // Clean up temp dir
        let _ = std::fs::remove_dir_all(&test_dir);
    }
}
