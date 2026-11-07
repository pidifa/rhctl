/*
 * Copyright (C) 2022-2026 rhctl Contributors
 *
 * SPDX-License-Identifier: Apache-2.0
 * **************************************************************************
 * An SSH connection pool for managing sessions and channels across
 * multiple servers.
 *
 * Since: 1.0.0
 * Date: October 16, 2025
 */
use crate::domain::cmd_params::ServerMetadata;
use crate::domain::constants::{
    DANGEROUS_PATHS, DEFAULT_SSH_HANDSHAKE_TIMEOUT, DEFAULT_SSH_PORT, REMOTE_TEMP_RHCTL_FOLDER,
    SUDO_ERR_MSG, SYSTEM_TASK_NAME,
};
use crate::domain::yml_config::ServerConfig;
use crate::utils::file_utils::{expand_local_user_path, generate_remote_temp_dir, get_local_path_base_name, split_unix_path};
use crate::utils::log_utils::ask_user;
use crate::utils::ssh_utils::{feed_remote_stream, CrProgressPrinter};
use crate::{log_debug, log_error_direct, log_info, log_warn, log_warn_direct};
use async_recursion::async_recursion;
use async_trait::async_trait;
use dashmap::DashMap;
use dashmap::DashSet;
use dirs_next as dirs;
use futures::stream::{FuturesUnordered, StreamExt};
use russh::client::{Config, Handler};
use russh::client::{Handle, Msg};
use russh::ChannelMsg;
use russh_keys::key::PublicKey;
use russh_keys::PublicKeyBase64;
use russh_keys::{load_openssh_certificate, load_secret_key};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{self, BufReader, Cursor, Read, Write};
use std::path::Path;
use std::path::PathBuf;
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::process::Command as TokioCommand;
use tokio::sync::oneshot;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::{self};
use tokio::time::timeout;

#[derive(Debug)]
pub enum SshError {
    Russh(russh::Error),
    Io(io::Error),
    Custom(String),
}

/// Remote path kind used by the transfer path-mapping rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemotePathKind {
    File,
    Dir,
}

impl From<russh::Error> for SshError {
    fn from(err: russh::Error) -> Self {
        SshError::Russh(err)
    }
}

impl From<io::Error> for SshError {
    fn from(err: io::Error) -> Self {
        SshError::Io(err)
    }
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SshError::Russh(e) => write!(f, "SSH error: {}", e),
            SshError::Io(e) => write!(f, "IO error: {}", e),
            SshError::Custom(e) => write!(f, "Custom error: {}", e),
        }
    }
}

#[derive(Clone)]
struct Client {
    username: String,
    password: String,
}

#[async_trait]
impl russh::client::Handler for Client {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        // println!("server_public_key: {}",server_public_key.fingerprint());
        Ok(true)
    }
}

#[derive(Clone)]
struct DummyHandler {
    captured_key: Arc<Mutex<Option<PublicKey>>>,
    notify: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl DummyHandler {
    fn new() -> (Self, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        let handler = Self {
            captured_key: Arc::new(Mutex::new(None)),
            notify: Arc::new(Mutex::new(Some(tx))),
        };
        (handler, rx)
    }

    fn take_key(&self) -> Option<PublicKey> {
        self.captured_key.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Handler for DummyHandler {
    type Error = SshError;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        // Store key
        *self.captured_key.lock().unwrap() = Some(server_public_key.clone());

        // Notify the waiter
        if let Some(tx) = self.notify.lock().unwrap().take() {
            let _ = tx.send(());
        }

        Ok(true)
    }
}

#[derive(Clone, Debug)]
pub struct ServerOptions {
    pub session_acquire_timeout: Duration,
    pub max_session_lifetime: Option<Duration>,
    pub max_channels_per_session: u32,
    pub max_sessions_per_server: u32,
}

impl ServerOptions {
    pub fn new() -> Self {
        Self {
            max_sessions_per_server: 2000,
            session_acquire_timeout: Duration::from_secs(30),
            max_session_lifetime: Some(Duration::from_secs(600)),
            max_channels_per_session: 5,
        }
    }
}

#[derive(Clone)]
struct Live {
    raw: Arc<Handle<Client>>,
    created_at: Instant,
}

#[derive(Clone)]
struct Idle {
    live: Live,
    idle_since: Instant,
}

pub struct ChannelGuard {
    pub channel: russh::Channel<Msg>,
    _permit: OwnedSemaphorePermit,
}

impl Drop for ChannelGuard {
    fn drop(&mut self) {
        // Channels are automatically closed when dropped
    }
}

#[derive(Clone)]
pub struct LiveSessionWrapper {
    live: Live,
    channel_semaphore: Arc<Semaphore>,
    max_permits: usize,
}

impl LiveSessionWrapper {
    fn new(live: Live, max_channels_per_session: usize) -> Self {
        Self {
            live,
            channel_semaphore: Arc::new(Semaphore::new(max_channels_per_session as usize)),
            max_permits: max_channels_per_session as usize,
        }
    }

    pub async fn get_channel_guard(&self) -> Result<ChannelGuard, SshError> {
        let permit = self
            .channel_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| {
                SshError::Io(io::Error::new(
                    io::ErrorKind::Other,
                    "Failed to acquire channel permit",
                ))
            })?;

        let channel = self
            .live
            .raw
            .channel_open_session()
            .await
            .map_err(|e| SshError::Russh(e))?;

        Ok(ChannelGuard {
            channel,
            _permit: permit,
        })
    }

    pub fn is_fully_idle(&self) -> bool {
        self.channel_semaphore.available_permits() == self.max_permits
    }
}

struct SessionPool {
    server_metadata: Arc<ServerMetadata>,
    active_sessions: Mutex<Vec<Arc<LiveSessionWrapper>>>,
    idle_sessions: Mutex<Vec<Idle>>,
    session_semaphore: Arc<Semaphore>,
}

impl SessionPool {
    fn new(server_metadata: &Arc<ServerMetadata>) -> Self {
        Self {
            server_metadata: server_metadata.clone(),
            active_sessions: Mutex::new(vec![]),
            idle_sessions: Mutex::new(vec![]),
            session_semaphore: Arc::new(Semaphore::new(
                server_metadata.max_sessions_per_server as usize,
            )),
        }
    }
    async fn connect(&self) -> Result<Handle<Client>, SshError> {
        let addr = format!(
            "{}:{}",
            self.server_metadata.host, self.server_metadata.ssh_port
        );
        let stream = tokio::time::timeout(
            self.server_metadata.connect_timeout,
            TcpStream::connect(&addr),
        )
        .await
        .map_err(|_| {
            SshError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "Connection timed out",
            ))
        })?
        .map_err(|e| SshError::Io(io::Error::new(io::ErrorKind::Other, e)))?;

        let config = Arc::new(russh::client::Config::default());
        let client = Client {
            username: self.server_metadata.user.clone(),
            password: self.server_metadata.password.clone(),
        };
        let mut session = russh::client::connect_stream(config, stream, client).await?;
        let user = self.server_metadata.user.clone();

        // Prefer identity / certificate auth when configured, then fall back to password.
        if let Some(ref identity_path) = self.server_metadata.identity_file {
            let identity_path = expand_local_user_path(identity_path);
            let key_pair = match load_secret_key(&identity_path, None) {
                Ok(key) => key,
                Err(first_err) => {
                    if !self.server_metadata.password.is_empty() {
                        load_secret_key(&identity_path, Some(self.server_metadata.password.as_str()))
                            .map_err(|e| {
                                SshError::Custom(format!(
                                    "Failed to load identity file '{}': {} (also failed with password as passphrase: {})",
                                    identity_path, first_err, e
                                ))
                            })?
                    } else {
                        return Err(SshError::Custom(format!(
                            "Failed to load identity file '{}': {}. If the key is encrypted, provide --password as the passphrase.",
                            identity_path, first_err
                        )));
                    }
                }
            };

            let key_auth_ok = if let Some(ref cert_path) = self.server_metadata.certificate_file {
                let cert_path = expand_local_user_path(cert_path);
                let cert = load_openssh_certificate(&cert_path).map_err(|e| {
                    SshError::Custom(format!(
                        "Failed to load OpenSSH certificate '{}': {}",
                        cert_path, e
                    ))
                })?;
                session
                    .authenticate_openssh_cert(user.clone(), Arc::new(key_pair), cert)
                    .await
                    .map_err(SshError::Russh)?
            } else {
                session
                    .authenticate_publickey(user.clone(), Arc::new(key_pair))
                    .await
                    .map_err(SshError::Russh)?
            };

            if key_auth_ok {
                return Ok(session);
            }

            if !self.server_metadata.password.is_empty() {
                let password_auth_ok = session
                    .authenticate_password(user.clone(), self.server_metadata.password.clone())
                    .await
                    .map_err(SshError::Russh)?;
                if password_auth_ok {
                    return Ok(session);
                }
                return Err(SshError::Io(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Authentication failed (identity/certificate and password)",
                )));
            }

            return Err(SshError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Authentication failed (identity/certificate)",
            )));
        }

        if self.server_metadata.password.is_empty() {
            return Err(SshError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "No identity file or password provided for authentication",
            )));
        }

        let auth_result = session
            .authenticate_password(user, self.server_metadata.password.clone())
            .await
            .map_err(SshError::Russh)?;

        if !auth_result {
            return Err(SshError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Authentication failed (password)",
            )));
        }

        Ok(session)
    }

    pub async fn get_session(&self) -> Result<Arc<LiveSessionWrapper>, SshError> {
        {
            let mut idle_guard = self.idle_sessions.lock().unwrap();
            while let Some(idle) = idle_guard.pop() {
                if idle.live.raw.is_closed() {
                    continue;
                }
                let wrapper = Arc::new(LiveSessionWrapper::new(
                    idle.live,
                    self.server_metadata.max_channels_per_session,
                ));
                let mut active_guard = self.active_sessions.lock().unwrap();
                active_guard.push(wrapper.clone());
                return Ok(wrapper);
            }
        }

        {
            let active_guard = self.active_sessions.lock().unwrap();
            for s in active_guard.iter() {
                if !s.live.raw.is_closed() && s.channel_semaphore.available_permits() > 0 {
                    return Ok(s.clone());
                }
            }
        }

        let _permit = tokio::time::timeout(
            self.server_metadata.session_acquire_timeout,
            self.session_semaphore.clone().acquire_owned(),
        )
        .await
        .map_err(|_| {
            SshError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "Session acquire timed out",
            ))
        })?
        .map_err(|_| SshError::Io(io::Error::new(io::ErrorKind::Other, "Server pool closed")))?;

        let sess = self.connect().await?;
        let live = Live {
            raw: Arc::new(sess),
            created_at: Instant::now(),
        };
        let wrapper = Arc::new(LiveSessionWrapper::new(
            live,
            self.server_metadata.max_channels_per_session,
        ));
        self.active_sessions.lock().unwrap().push(wrapper.clone());
        Ok(wrapper)
    }

    fn recycle_session(&self) {
        let mut active_guard = self.active_sessions.lock().unwrap();
        let mut idle_guard = self.idle_sessions.lock().unwrap();

        active_guard.retain(|s| {
            if s.live.raw.is_closed() {
                false
            } else if s.is_fully_idle() {
                idle_guard.push(Idle {
                    live: s.live.clone(),
                    idle_since: Instant::now(),
                });
                false
            } else {
                true
            }
        });
    }

    fn cleanup_idle(&self) {
        let mut idle_guard = self.idle_sessions.lock().unwrap();
        idle_guard.retain(|s| {
            !s.live.raw.is_closed()
                && s.idle_since.elapsed() <= self.server_metadata.max_session_lifetime
        });
    }
}

pub struct ServerPool {
    servers: DashMap<u64, Arc<SessionPool>>,
    pub pending_clean_servers: Arc<DashSet<Arc<ServerMetadata>>>,
}

impl ServerPool {
    pub fn new() -> Self {
        Self {
            servers: DashMap::new(),
            pending_clean_servers: Arc::new(DashSet::new()),
        }
    }

    fn cleanup_idle_sessions(&self) {
        for pool in self.servers.iter() {
            pool.value().recycle_session();
            pool.value().cleanup_idle();
        }
    }

    pub fn generate_server_key(host: &str, port: u16, username: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        host.hash(&mut hasher);
        port.hash(&mut hasher);
        username.hash(&mut hasher);
        hasher.finish()
    }

    /// Check connectivity to many servers in parallel and record their SSH host keys.
    pub async fn check_servers_and_update_known_hosts(
        &self,
        servers: Vec<ServerConfig>,
    ) -> HashSet<String> {
        let mut futures = FuturesUnordered::new();

        for server in servers {
            let host = server.host.clone();
            let name = server.name.clone();
            let fut = task::spawn(Self::check_single_server_by_info(
                host,
                server.ssh_port.unwrap_or(DEFAULT_SSH_PORT),
                Some(name),
            ));
            futures.push(fut);
        }

        let mut failed_server_names = HashSet::new();

        while let Some(res) = futures.next().await {
            match res {
                Ok((server_name, server_host, result)) => {
                    if let Err(e) = result {
                        if let Some(name) = &server_name {
                            log_warn_direct!(
                                "Server '{}' ({}) failed: \n\t> {}",
                                name,
                                server_host,
                                e
                            );
                        } else {
                            log_warn_direct!("Server ({}) failed: \n\t> {}", server_host, e);
                        }
                        failed_server_names.insert(server_name.unwrap());
                    }
                }
                Err(e) => {
                    // The task itself panicked or was cancelled
                    log_error_direct!("Task error: \n\t> {}", e);
                    exit(1);
                    // Optional: can't get server name here
                }
            }
        }

        failed_server_names
    }

    /// Connect to a single SSH server by host, port, and name, and fetch its public key.
    pub async fn check_single_server_by_info(
        host: String,
        port: u16,
        name: Option<String>,
    ) -> (Option<String>, String, Result<(), String>) {
        let addr = format!("{}:{}", host, port);
        let res = match timeout(DEFAULT_SSH_HANDSHAKE_TIMEOUT, TcpStream::connect(&addr)).await {
            Ok(Ok(stream)) => {
                // Try to extract SSH host key
                match Self::fetch_ssh_host_key(stream).await {
                    Ok(key) => {
                        if let Err(e) = Self::add_host_to_known_hosts(&host, port, &key).await {
                            Err(format!("Failed to write known_hosts: {}", e))
                        } else {
                            Ok(())
                        }
                    }
                    Err(e) => Err(format!("Failed to fetch SSH key: {}", e)),
                }
            }
            Ok(Err(e)) => Err(format!("Connection error: {}", e)),
            Err(_) => Err("Initial connection timed out".to_string()),
        };

        (name, host.to_string(), res)
    }

    /// Fetch SSH public key from a remote host.
    /// This is a lightweight TCP-level handshake to read the server's identification and key.
    async fn fetch_ssh_host_key(stream: TcpStream) -> Result<PublicKey, String> {
        let config = Arc::new(Config::default());
        let (handler, rx) = DummyHandler::new();
        let handler_ref = handler.clone();

        // Connect: russh::client::connect_stream returns a Handle
        let handle = russh::client::connect_stream(config, stream, handler)
            .await
            .map_err(|e| format!("SSH handshake error: {}", e))?;

        // Wait for the check_server_key callback notification
        let _ = timeout(Duration::from_secs(10), rx)
            .await
            .map_err(|_| "Timed out waiting for server key".to_string())?;

        handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await
            .ok();

        handler_ref
            .take_key()
            .ok_or_else(|| "Server did not send key".to_string())
    }

    /// Asynchronously append a host key to ~/.ssh/known_hosts
    pub async fn add_host_to_known_hosts(
        host: &str,
        port: u16,
        key: &PublicKey,
    ) -> std::io::Result<()> {
        // println!("add_host_to_known_hosts: {}", host);
        // Get home directory
        let mut path = dirs::home_dir().ok_or(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Home directory not found",
        ))?;
        path.push(".ssh");
        tokio::fs::create_dir_all(&path).await.ok();
        path.push("known_hosts");

        // Encode the public key
        let encoded_key = key.public_key_base64();
        let line_to_add = if port == DEFAULT_SSH_PORT {
            format!("{} {} {}\n", host, key.name(), encoded_key)
        } else {
            format!("[{}]:{} {} {}\n", host, port, key.name(), encoded_key)
        };

        // If file exists, check if the same entry already exists
        let mut file_exists = false;
        let mut file_content = String::new();
        if tokio::fs::metadata(&path).await.is_ok() {
            file_exists = true;
            let mut file = tokio::fs::File::open(&path).await?;
            file.read_to_string(&mut file_content).await?;
        }

        // Check if same host+key already exists
        let already_exists = file_content.lines().any(|line| {
            let line = line.trim();
            line.starts_with(host) || line.starts_with(&format!("[{}]:{}", host, port))
        });
        if already_exists {
            // println!("key already exists, skip: {}", line_to_add);
            // Already stored, skip writing
            return Ok(());
        }

        // Append the new entry
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        file.write_all(line_to_add.as_bytes()).await?;

        Ok(())
    }

    async fn get_session_pool(&self, server_metadata: &Arc<ServerMetadata>) -> Arc<SessionPool> {
        if let Some(pool) = self.servers.get(&server_metadata.server_key) {
            return pool.clone();
        }
        let server_pool = Arc::new(SessionPool::new(&server_metadata));
        self.servers
            .insert(server_metadata.server_key, server_pool.clone());
        server_pool
    }

    pub async fn get_channel(
        &self,
        server_metadata: &Arc<ServerMetadata>,
    ) -> Result<ChannelGuard, String> {
        let server_pool = self.get_session_pool(server_metadata).await;
        let live_session_wrapper = server_pool
            .get_session()
            .await
            .map_err(|e| format!("Get session error:\n\t> {}", e))?;

        let guard = live_session_wrapper
            .get_channel_guard()
            .await
            .map_err(|e| format!("Get channel error:\n\t> {}", e))?;

        Ok(guard)
    }

    pub fn start_idle_cleanup(self: Arc<Self>, interval: Duration) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                self.cleanup_idle_sessions();
            }
        });
    }

    pub async fn resolve_remote_path(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        use_sudo: bool,
        path: &str,
    ) -> Result<String, String> {
        self.exec(
            server_metadata,
            task_name,
            &format!("echo {}", path),
            use_sudo,
        )
        .await
    }

    pub async fn check_global_remote_temp_dir(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        use_sudo: bool,
        silent: bool,
    ) -> Result<(), String> {
        if use_sudo && server_metadata.user != "root" {
            self.pending_clean_servers.insert(server_metadata.clone());
            let exists = self
                .file_or_dir_exists(
                    server_metadata,
                    task_name,
                    REMOTE_TEMP_RHCTL_FOLDER,
                    use_sudo,
                )
                .await?;
            if exists {
                ask_user(server_metadata,task_name,
                    format!(
                        "Remote path '{}' already exists. Transfering will DELETE it and use it as a temp folder. Continue?",
                        REMOTE_TEMP_RHCTL_FOLDER
                    )
                    .as_str(),
                    silent,
                )
                .await?;

                self.exec(
                    server_metadata,
                    task_name,
                    &format!("rm -rf \"{}\"", REMOTE_TEMP_RHCTL_FOLDER),
                    use_sudo,
                )
                .await?;
            }
        }
        Ok(())
    }

    pub async fn cleanup_pending_servers(&self) -> Result<(), String> {
        for server in self.pending_clean_servers.iter() {
            self.delete_global_temp_dir(&server, SYSTEM_TASK_NAME)
                .await?;
        }
        Ok(())
    }

    pub async fn file_or_dir_exists(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        path: &str,
        use_sudo: bool,
    ) -> Result<bool, String> {
        self.check_path(server_metadata, task_name, path, "-e", use_sudo)
            .await
    }

    pub async fn file_exists(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        path: &str,
        use_sudo: bool,
    ) -> Result<bool, String> {
        self.check_path(server_metadata, task_name, path, "-f", use_sudo)
            .await
    }

    pub async fn dir_exists(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        path: &str,
        use_sudo: bool,
    ) -> Result<bool, String> {
        self.check_path(server_metadata, task_name, path, "-d", use_sudo)
            .await
    }

    /// Classify a remote path as file / directory / missing with a single probe.
    /// Anything existing but neither file nor dir (socket, fifo, device, ...)
    /// is reported as an error so callers fail loudly instead of guessing.
    pub async fn remote_path_kind(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        path: &str,
        use_sudo: bool,
    ) -> Result<Option<RemotePathKind>, String> {
        let escaped = path.replace('\'', "'\\''");
        let probe = format!(
            "if [ -d '{p}' ]; then echo __RHCTL_DIR__; elif [ -f '{p}' ]; then echo __RHCTL_FILE__; elif [ -e '{p}' ]; then echo __RHCTL_OTHER__; else echo __RHCTL_MISSING__; fi",
            p = escaped
        );
        let out = self
            .exec(server_metadata, task_name, &probe, use_sudo)
            .await?;
        match out.trim() {
            "__RHCTL_DIR__" => Ok(Some(RemotePathKind::Dir)),
            "__RHCTL_FILE__" => Ok(Some(RemotePathKind::File)),
            "__RHCTL_MISSING__" => Ok(None),
            "__RHCTL_OTHER__" => Err(format!(
                "Remote path '{}' exists but is neither a file nor a directory",
                path
            )),
            other => Err(format!(
                "Failed to check remote path '{}' (unexpected output '{}')",
                path, other
            )),
        }
    }

    /// Check if any files or directories in `dir_path` match the given `pattern`
    pub async fn dir_has_pattern(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        dir_path: &str,
        pattern: &str,
        use_sudo: bool,
    ) -> Result<bool, String> {
        log_debug!(
            server_metadata,
            task_name,
            "Checking if directory '{}' has any content matching pattern '{}'",
            dir_path,
            pattern
        );

        // Exit 0 if any match exists, exit 1 otherwise.
        // `pattern` stays unquoted so the remote shell expands globs (e.g. dir/*).
        let full_cmd = format!(
            "for f in {}; do [ -e \"$f\" ] && exit 0; done; exit 1",
            pattern
        );

        let result = self
            .exec(server_metadata, task_name, &full_cmd, use_sudo)
            .await;

        match result {
            Ok(_) => {
                log_debug!(
                    server_metadata,
                    task_name,
                    "Directory '{}' has content matching '{}': true",
                    dir_path,
                    pattern
                );
                Ok(true)
            }
            Err(e) => {
                // Exit status 1 means no match
                if is_exit_status(&e, 1) {
                    log_debug!(
                        server_metadata,
                        task_name,
                        "Directory '{}' has no content matching '{}'",
                        dir_path,
                        pattern
                    );
                    Ok(false)
                } else {
                    Err(format!(
                        "Failed to check directory '{}' for pattern '{}'. \n\t> {}",
                        dir_path, pattern, e
                    ))
                }
            }
        }
    }

    /// Check if directory contains any hidden files (names starting with .)
    pub async fn dir_has_hidden_items(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        dir_path: &str,
        use_sudo: bool,
    ) -> Result<bool, String> {
        let pattern = format!("{}/.[!.]*", dir_path);
        self.dir_has_pattern(server_metadata, task_name, dir_path, &pattern, use_sudo)
            .await
    }

    /// Check if directory contains any normal (non-hidden) files
    pub async fn dir_has_normal_items(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        dir_path: &str,
        use_sudo: bool,
    ) -> Result<bool, String> {
        let pattern = format!("{0}/*", dir_path);
        self.dir_has_pattern(server_metadata, task_name, dir_path, &pattern, use_sudo)
            .await
    }

    pub async fn check_path(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        path: &str,
        flag: &str,
        use_sudo: bool,
    ) -> Result<bool, String> {
        log_debug!(
            server_metadata,
            task_name,
            "Checking if '{}' exists with flag '{}'",
            path,
            flag
        );

        let quoted_path =
            shlex::try_quote(path).map_err(|e| format!("Invalid remote path '{}': {}", path, e))?;
        let full_cmd = format!("test {} {}", flag, quoted_path);
        let result = self
            .exec(server_metadata, task_name, &full_cmd, use_sudo)
            .await;

        match result {
            Ok(_) => {
                log_debug!(
                    server_metadata,
                    task_name,
                    "Remote path '{}' exists (flag '{}'): true",
                    path,
                    flag
                );
                Ok(true)
            }
            Err(e) => {
                if is_exit_status(&e, 1) {
                    log_debug!(
                        server_metadata,
                        task_name,
                        "Remote path '{}' exists (flag '{}'): false",
                        path,
                        flag
                    );
                    Ok(false)
                } else {
                    Err(format!(
                        "Failed to check remote path '{}'. \n\t> {}",
                        path, e
                    ))
                }
            }
        }
    }

    pub async fn delete_global_temp_dir(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
    ) -> Result<(), String> {
        let remote_temp_rhctl_folder_exists = self
            .file_or_dir_exists(server_metadata, task_name, REMOTE_TEMP_RHCTL_FOLDER, true)
            .await?;
        if remote_temp_rhctl_folder_exists {
            log_debug!(
                server_metadata,
                task_name,
                "Clean up temp forder for server: host - {}, user - {}",
                server_metadata.host,
                server_metadata.user
            );
            self.exec(
                server_metadata,
                task_name,
                &format!("rm -rf \"{}\"", REMOTE_TEMP_RHCTL_FOLDER),
                true,
            )
            .await?;
        }
        Ok(())
    }

    pub async fn exec(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        cmd: &str,
        use_sudo: bool,
    ) -> Result<String, String> {
        self.exec_with_stream(server_metadata, task_name, cmd, use_sudo, false)
            .await
    }

    pub async fn exec_with_log(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        cmd: &str,
        use_sudo: bool,
    ) -> Result<String, String> {
        self.exec_with_stream(server_metadata, task_name, cmd, use_sudo, true)
            .await
    }

    async fn exec_with_stream(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        cmd: &str,
        use_sudo: bool,
        print_log: bool,
    ) -> Result<String, String> {
        let cmd = cmd.to_string();
        let mut channel_guard = self.get_channel(server_metadata).await?;

        // When no password is available (identity / key-based auth) use
        // non-interactive `sudo -n` so the command does not block waiting for
        // a password prompt that can never be satisfied.
        let sudo_interactive = use_sudo && !server_metadata.password.is_empty();

        log_debug!(
            server_metadata,
            task_name,
            "Streaming command: {} (sudo: {})",
            cmd,
            use_sudo
        );

        let full_cmd = if use_sudo {
            let quoted = shlex::try_quote(cmd.as_str())
                .map_err(|e| format!("Failed to quote sudo command: {}", e))?;

            let sudo_prefix = if sudo_interactive {
                "sudo -S -p '[RHCTL_SUDO_PASSWORD]'"
            } else {
                "sudo -n"
            };
            // Use an explicit prompt so we can distinguish the sudo password
            // request from normal command output.
            format!("{} bash -c {}", sudo_prefix, quoted)
        } else {
            cmd.to_string()
        };

        log_debug!(
            server_metadata,
            task_name,
            "Full command: {} (sudo: {})",
            full_cmd,
            use_sudo
        );

        if sudo_interactive {
            channel_guard
                .channel
                .request_pty(true, "xterm", 0, 0, 0, 0, &[])
                .await
                .map_err(|e| format!("Failed to request pty for sudo. \n\t> {}", e))?;
        }

        channel_guard
            .channel
            .exec(true, full_cmd.as_bytes().to_vec())
            .await
            .map_err(|e| format!("Failed to execute command '{}'. \n\t> {}", cmd, e))?;

        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();

        let mut skip_sudo_blank = use_sudo;
        let mut stdout_collected = Vec::new();
        let mut stderr_collected = Vec::new();

        let mut partial_stdout = String::new();
        let mut partial_stderr = String::new();

        let mut stdout_progress = CrProgressPrinter::new();
        let mut stderr_progress = CrProgressPrinter::new();

        let mut saw_exit_status = false;
        let mut sudo_password_sent = !sudo_interactive;

        while let Some(msg) = channel_guard.channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    let data_str = String::from_utf8_lossy(&data);

                    // sudo may send its password prompt through the PTY.
                    if sudo_interactive && !sudo_password_sent && data_str.contains("[RHCTL_SUDO_PASSWORD]")
                    {
                        let pw_with_newline = format!("{}\n", server_metadata.password);

                        channel_guard
                            .channel
                            .data(pw_with_newline.as_bytes())
                            .await
                            .map_err(|e| format!("Failed to send sudo password. \n\t> {}", e))?;

                        sudo_password_sent = true;

                        // Do not expose the sudo password prompt in command output.
                        continue;
                    }

                    stdout_collected.extend_from_slice(&data);

                    if print_log {
                        feed_remote_stream(
                            &mut partial_stdout,
                            &mut stdout_progress,
                            &data_str,
                            server_metadata,
                            task_name,
                            &mut skip_sudo_blank,
                        )?;
                    }
                }

                ChannelMsg::ExtendedData { data, ext } if ext == 1 => {
                    stderr_collected.extend_from_slice(&data);

                    if print_log {
                        let data_str = String::from_utf8_lossy(&data);

                        // Remote stderr (e.g. curl progress) streams as REMOTE, not ERROR.
                        let mut no_sudo_skip = false;

                        feed_remote_stream(
                            &mut partial_stderr,
                            &mut stderr_progress,
                            &data_str,
                            server_metadata,
                            task_name,
                            &mut no_sudo_skip,
                        )?;
                    }
                }

                ChannelMsg::ExitStatus { exit_status } => {
                    saw_exit_status = true;

                    if exit_status != 0 {
                        let stdout_str = String::from_utf8_lossy(&stdout_collected);
                        let stderr_str = String::from_utf8_lossy(&stderr_collected);

                        let mut msg;

                        // When no password is available (identity / key-based auth) and
                        // sudo requires one, report the real cause instead of the
                        // misleading use_sudo is not enabled message.
                        if !sudo_interactive && stderr_str.contains(SUDO_ERR_MSG) {
                            msg = format!(
                                "sudo requires a password, but no password was provided (identity / key-based auth). \n\t> Enable passwordless sudo for the remote user or supply --password. \n\t> Command line: \n{}",
                                cmd
                            );
                        } else if stderr_str.contains(SUDO_ERR_MSG) {
                            msg = format!(
                            "Detected 'sudo' at the start of a command line, but 'use_sudo' is not enabled. Enable 'use_sudo' to run commands with sudo. \n\t> Command line: \n{}",
                            cmd
                        );
                        } else {
                            msg = format!(
                                "Command failed with exit status {}. \n\t> Command line: \n{}",
                                exit_status, cmd
                            );

                            if !stdout_str.trim().is_empty() {
                                msg.push_str(&format!("\nRemote Output:\n\t{}", stdout_str));
                            }

                            if !stderr_str.trim().is_empty() {
                                msg.push_str(&format!("\nRemote Error:\n\t{}", stderr_str));
                            }
                        }

                        return Err(msg);
                    }

                    break;
                }

                _ => continue,
            }
        }

        if !saw_exit_status {
            return Err(format!(
                "Command finished without an exit status. \n\t> Command line: \n{}",
                cmd
            ));
        }

        if print_log {
            stdout_progress.flush(server_metadata, task_name)?;
            if !partial_stdout.is_empty()
                && !(use_sudo && skip_sudo_blank && partial_stdout.trim().is_empty())
            {
                stdout_progress.on_lf(&partial_stdout, server_metadata, task_name)?;
            }

            stderr_progress.flush(server_metadata, task_name)?;

            if !partial_stderr.is_empty() {
                stderr_progress.on_lf(&partial_stderr, server_metadata, task_name)?;
            }
        }

        stdout_buf = String::from_utf8_lossy(&stdout_collected).to_string();
        stderr_buf = String::from_utf8_lossy(&stderr_collected).to_string();

        channel_guard
            .channel
            .close()
            .await
            .map_err(|e| format!("Failed to close channel. \n\t> {}", e))?;

        stdout_buf = stdout_buf.trim().to_string();

        Ok(stdout_buf)
    }

    pub async fn upload_file_or_dir_contents_into_dir(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        local_file_or_dir: &Path,
        remote_dir: &str,
        new_file_name: Option<&str>,
        use_sudo: bool,
        use_rsync: bool,
        silent: bool,
        _direct_write_if_sudo: bool,
        print_log: bool,
    ) -> Result<(), String> {
        if local_file_or_dir.is_dir() {
            // When not using rsync, transfer the directory as a single tar archive
            // over SCP to avoid per-file round-trips (channel open, header, data,
            // ACKs for every file). With rsync we keep the existing per-entry
            // iteration since rsync handles directories efficiently.
            if !use_rsync {
                self.ask_safe_to_transfer(
                    server_metadata,
                    task_name,
                    remote_dir,
                    use_sudo,
                    silent,
                )
                .await?;

                return self
                    .upload_dir_via_tar_stream(
                        server_metadata,
                        task_name,
                        local_file_or_dir,
                        remote_dir,
                        use_sudo,
                    )
                    .await;
            }

            for entry in std::fs::read_dir(local_file_or_dir).map_err(|e| {
                format!(
                    "Failed to read local directory '{}'. \n\t> {}",
                    local_file_or_dir.display(),
                    e
                )
            })? {
                let entry =
                    entry.map_err(|e| format!("Error reading directory entry. \n\t> {}", e))?;
                let sub_path = entry.path();
                let base_name = get_local_path_base_name(&sub_path)?;
                let remote_sub = format!("{}/{}", remote_dir, base_name);   

                self.ask_safe_to_transfer(
                    server_metadata,
                    task_name,
                    &remote_sub,
                    use_sudo,
                    silent,
                )
                .await?;    

                self.do_upload(
                    server_metadata,
                    task_name,
                    use_sudo,
                    use_rsync,
                    &sub_path,
                    remote_dir,
                    new_file_name,
                )
                .await?;
            }   

            if print_log {
                log_info!(
                    server_metadata,
                    task_name,
                    "Successfully uploaded the contents of the folder '{}' into '{}'",
                    local_file_or_dir.display(),
                    remote_dir
                );
            }
        } else {
            let remote_file = format!(
                "{}/{}",
                remote_dir,
                new_file_name.unwrap_or(get_local_path_base_name(&local_file_or_dir)?.as_str())
            );  

            self.ask_safe_to_transfer(
                server_metadata,
                task_name,
                &remote_file,
                use_sudo,
                silent,
            )
            .await?;    

            self.do_upload(
                server_metadata,
                task_name,
                use_sudo,
                use_rsync,
                local_file_or_dir,
                remote_dir,
                new_file_name,
            )
            .await?;    

            if print_log {
                log_info!(
                    server_metadata,
                    task_name,
                    "Successfully uploaded the file '{}' to '{}'",
                    local_file_or_dir.display(),
                    remote_file
                );
            }
        }   

        Ok(())
    }

    /// Upload a local directory to a remote directory via SCP using a tar archive.
    ///
    /// Instead of opening one SSH channel per file (each with channel open,
    /// header, data, and ACK round-trips), we create a single tar archive
    /// locally, transfer it as one SCP file, and extract it remotely. This
    /// reduces N round-trips to ~1 and is especially beneficial for directories
    /// containing many small files.
    /// Upload a local directory to a remote directory by streaming a tar archive
    /// directly over the SSH channel. No temporary tar file is created locally
    /// or remotely — the tar stream is piped directly into a remote `tar -xf -`
    /// command.
    async fn upload_dir_via_tar_stream(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        local_dir: &Path,
        remote_dir: &str,
        use_sudo: bool,
    ) -> Result<(), String> {
        // Ensure the remote target directory exists.
        self.create_remote_dir_if_not_exists(server_metadata, task_name, remote_dir, use_sudo)
            .await?;

        // Build the remote extraction command that reads tar from stdin.
        // When no password is available (identity / key-based auth) use
        // non-interactive `sudo -n` so the command does not block waiting for
        // a password prompt that can never be satisfied.
        let extract_cmd = if use_sudo && server_metadata.user != "root" {
            let sudo_prefix = if server_metadata.password.is_empty() {
                "sudo -n"
            } else {
                "sudo -S -p '[RHCTL_SUDO_PASSWORD]'"
            };
            format!("{} tar -xf - --strip-components=1 -C {}", sudo_prefix, remote_dir)
        } else {
            format!("tar -xf - --strip-components=1 -C {}", remote_dir)
        };

        log_debug!(
            server_metadata,
            task_name,
            "Streaming tar upload to remote: {}",
            extract_cmd
        );

        // Get an SSH channel and exec the remote extraction command.
        let mut channel_guard = self.get_channel(server_metadata).await?;

        channel_guard
            .channel
            .exec(true, extract_cmd.as_bytes().to_vec())
            .await
            .map_err(|e| format!("Failed to exec remote tar extraction: {}", e))?;

        // If using sudo -S, feed the password before the tar stream.
        // Skipped when no password is available (identity mode): `sudo -n` does
        // not prompt for a password.
        if use_sudo && server_metadata.user != "root" && !server_metadata.password.is_empty() {
            let password_with_newline = format!("{}
", server_metadata.password);
            channel_guard
                .channel
                .data(password_with_newline.as_bytes())
                .await
                .map_err(|e| format!("Failed to send sudo password: {}", e))?;
        }

        // Spawn a local tar process that writes the archive to stdout.
        let dir_parent = local_dir.parent().unwrap_or(local_dir);
        let dir_name = local_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let mut tar_child = std::process::Command::new("tar")
            .arg("-cf")
            .arg("-")
            .arg("-C")
            .arg(dir_parent)
            .arg(&dir_name)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn tar process: {}", e))?;

        let tar_stdout = tar_child
            .stdout
            .take()
            .ok_or("Failed to capture tar stdout")?;

        // Stream tar output through the SSH channel in 64KiB chunks.
        use std::io::Read;
        let mut buf = [0u8; 65536];
        let mut reader = std::io::BufReader::new(tar_stdout);
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("Failed to read tar stream: {}", e))?;
            if n == 0 {
                break;
            }
            channel_guard
                .channel
                .data(&mut std::io::Cursor::new(&buf[..n]))
                .await
                .map_err(|e| format!("Failed to send tar stream chunk: {}", e))?;
        }

        // Send end-of-stream marker (null byte) and close the channel.
        let mut cursor = std::io::Cursor::new(&[0u8]);
        channel_guard
            .channel
            .data(&mut cursor)
            .await
            .map_err(|e| format!("Failed to send tar stream terminator: {}", e))?;

        channel_guard
            .channel
            .close()
            .await
            .map_err(|e| format!("Failed to close channel: {}", e))?;

        // Wait for the tar child process to finish.
        let tar_status = tar_child
            .wait()
            .map_err(|e| format!("Failed to wait for tar process: {}", e))?;
        if !tar_status.success() {
            let stderr_bytes = tar_child
                .stderr
                .as_mut()
                .map(|s| {
                    let mut b = Vec::new();
                    s.read_to_end(&mut b).ok();
                    b
                })
                .unwrap_or_default();
            let stderr = String::from_utf8_lossy(&stderr_bytes);
            return Err(format!(
                "Local tar process failed with status {:?}: {}",
                tar_status.code(),
                stderr
            ));
        }

        log_debug!(
            server_metadata,
            task_name,
            "Tar stream upload to '{}' completed",
            remote_dir
        );

        Ok(())
    }

    pub async fn upload_file_to_file(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        local_file: &Path,
        remote_file: &str,
        use_sudo: bool,
        use_rsync: bool,
        silent: bool,
    ) -> Result<(), String> {
        if !local_file.is_file() {
            return Err(format!(
                "Local file '{}' does not exist or is not a file",
                local_file.display()
            ));
        }
        // Overwrite confirmation keeps existing logic (ask then delete).
        self.ask_safe_to_transfer(server_metadata, task_name, remote_file, use_sudo, silent)
            .await?;
        // `do_upload` copies into a directory; split exact file target into
        // parent dir + new file name so both scp and rsync keep working.
        // `split_unix_path` returns (basename, parent).
        let (name, parent) = split_unix_path(remote_file)?;
        let parent = if parent.is_empty() { "/".to_string() } else { parent };
        self.create_remote_dir_if_not_exists(server_metadata, task_name, &parent, use_sudo)
            .await?;
        let file_name = name.clone();
        self.upload_file_or_dir_contents_into_dir_silent(
            server_metadata,
            task_name,
            local_file,
            &parent,
            Some(&file_name),
            use_sudo,
            use_rsync,
        )
        .await?;
        log_info!(
            server_metadata,
            task_name,
            "Successfully uploaded the file '{}' to '{}'",
            local_file.display(),
            remote_file
        );
        Ok(())
    }

    pub async fn upload_dir_replace_dir(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        local_dir: &Path,
        remote_dir: &str,
        use_sudo: bool,
        use_rsync: bool,
        silent: bool,
    ) -> Result<(), String> {
        if !local_dir.is_dir() {
            return Err(format!(
                "Local directory '{}' does not exist or is not a directory",
                local_dir.display()
            ));
        }
        // dir -> dir: replace the target directory (confirm first).
        self.ask_safe_to_transfer(server_metadata, task_name, remote_dir, use_sudo, silent)
            .await?;
        self.upload_dir_to_new_path(
            server_metadata,
            task_name,
            local_dir,
            remote_dir,
            use_sudo,
            use_rsync,
            true,
        )
        .await
    }

    pub async fn upload_dir_to_new_path(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        local_dir: &Path,
        remote_dir: &str,
        use_sudo: bool,
        use_rsync: bool,
        silent: bool,
    ) -> Result<(), String> {
        if !local_dir.is_dir() {
            return Err(format!(
                "Local directory '{}' does not exist or is not a directory",
                local_dir.display()
            ));
        }
        // dir -> non-existent: create a new directory at the target path,
        // then merge the source contents into it. Silent inner call: the
        // fresh directory was just created, no overwrite to confirm.
        self.create_remote_dir_if_not_exists(server_metadata, task_name, remote_dir, use_sudo)
            .await?;
        self.upload_file_or_dir_contents_into_dir_silent(
            server_metadata,
            task_name,
            local_dir,
            remote_dir,
            None,
            use_sudo,
            use_rsync,
        )
        .await?;
        let _ = silent;
        log_info!(
            server_metadata,
            task_name,
            "Successfully uploaded the contents of the folder '{}' into '{}'",
            local_dir.display(),
            remote_dir
        );
        Ok(())
    }

    /// Same as `upload_file_or_dir_contents_into_dir` but skips the per-child
    /// overwrite prompt. Used after the caller already confirmed once.
    async fn upload_file_or_dir_contents_into_dir_silent(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        local_file_or_dir: &Path,
        remote_dir: &str,
        new_file_name: Option<&str>,
        use_sudo: bool,
        use_rsync: bool,
    ) -> Result<(), String> {
        self.upload_file_or_dir_contents_into_dir(
            server_metadata,
            task_name,
            local_file_or_dir,
            remote_dir,
            new_file_name,
            use_sudo,
            use_rsync,
            true,
            false,
            false,
        )
        .await
    }

    pub async fn download_file_or_dir_contents_into_dir(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_file_or_dir: &str,
        local_dir: &Path,
        use_sudo: bool,
        use_rsync: bool,
        silent: bool,
        print_log: bool,
    ) -> Result<(), String> {
        if !local_dir.exists() {
            std::fs::create_dir_all(local_dir).map_err(|e| {
                format!(
                    "Failed to create local download directory '{}'. \n\t> {}",
                    local_dir.display(),
                    e
                )
            })?;
        }

        if !local_dir.is_dir() {
            return Err(format!(
                "Local download target '{}' is not a directory",
                local_dir.display()
            ));
        }

        let source_is_dir = self
            .dir_exists(
                server_metadata,
                task_name,
                remote_file_or_dir,
                use_sudo,
            )
            .await?;

        // A non-root sudo download cannot use local rsync directly because the
        // remote rsync process would need elevated permissions. Use direct SCP.
        let use_rsync = use_rsync
            && !(use_sudo && server_metadata.user != "root");

        self.do_download(
            server_metadata,
            task_name,
            remote_file_or_dir,
            local_dir,
            use_rsync,
            source_is_dir,
            silent,
            use_sudo
        )
        .await?;

        if print_log {
            log_info!(
                server_metadata,
                task_name,
                "Successfully downloaded '{}' into '{}'",
                remote_file_or_dir,
                local_dir.display()
            );
        }

        Ok(())
    }

    async fn do_download(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_source: &str,
        local_dir: &Path,
        use_rsync: bool,
        source_is_dir: bool,
        silent: bool,
        use_sudo: bool,
    ) -> Result<(), String> {
        let stage_dir = std::env::temp_dir().join(format!(
            "rhctl-download-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&stage_dir).map_err(|e| {
            format!(
                "Failed to create local download staging directory '{}': {}",
                stage_dir.display(),
                e
            )
        })?;

        let result = if use_rsync
            && self.command_exists("rsync")
            && !(use_sudo && server_metadata.user != "root")
        {
            self.do_download_with_rsync(
                server_metadata,
                task_name,
                remote_source,
                &stage_dir,
                source_is_dir,
            )
            .await
        } else {
            self.do_download_with_scp(
                server_metadata,
                task_name,
                remote_source,
                &stage_dir,
                source_is_dir,
                use_sudo
            )
            .await
        };

        let merge_result = match result {
            Ok(()) => {
                self.merge_download_stage(&stage_dir, local_dir, silent, server_metadata, task_name)
                    .await
            }
            Err(e) => Err(e),
        };
        let _ = std::fs::remove_dir_all(&stage_dir);
        merge_result
    }

    async fn do_download_with_rsync(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_source: &str,
        stage_dir: &Path,
        source_is_dir: bool,
    ) -> Result<(), String> {
        // Directory sources must merge CONTENTS into the staging dir (same as
        // the scp path). Without the trailing `/`, rsync would nest the
        // source dir inside staging and `merge_download_stage` would see an
        // unexpected layout.
        let remote_arg = if source_is_dir {
            let trimmed = remote_source.trim_end_matches('/');
            if trimmed.is_empty() {
                "/.".to_string()
            } else {
                format!("{}/.", trimmed)
            }
        } else {
            remote_source.to_string()
        };
        let remote_spec = format!(
            "{}@{}:{}",
            server_metadata.user, server_metadata.host, remote_arg
        );
        let status = if let Some(ref identity_file) = server_metadata.identity_file {
            let identity_path = expand_local_user_path(identity_file);
            let ssh_cmd = format!("ssh -p {} -i {}", server_metadata.ssh_port, identity_path);
            let status = TokioCommand::new("rsync")
                .arg("-a")
                .arg("-e")
                .arg(&ssh_cmd)
                .arg(&remote_spec)
                .arg(stage_dir)
                .status()
                .await
                .map_err(|e| format!("Failed to execute rsync download with identity: {}", e))?;
            if !status.success() {
                return Err(format!("rsync download with identity failed with exit code {:?}", status.code()));
            }
        } else if !server_metadata.password.is_empty() && self.command_exists("sshpass") {
            let status = TokioCommand::new("sshpass")
                .arg("-p")
                .arg(&server_metadata.password)
                .arg("rsync")
                .arg("-a")
                .arg("-e")
                .arg(format!("ssh -p {}", server_metadata.ssh_port))
                .arg(&remote_spec)
                .arg(stage_dir)
                .status()
                .await
                .map_err(|e| format!("Failed to execute rsync download via sshpass: {}", e))?;
            if !status.success() {
                return Err(format!("rsync download via sshpass failed with exit code {:?}", status.code()));
            }
        } else {
            return Err(
                "rsync download requires an identity file or sshpass for password authentication"
                    .to_string(),
            );
        };
        log_debug!(
            server_metadata,
            task_name,
            "rsync download from '{}' completed",
            remote_source
        );
        Ok(())
    }

    async fn do_download_with_scp(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_source: &str,
        stage_dir: &Path,
        source_is_dir: bool,
        use_sudo: bool
    ) -> Result<(), String> {
        if use_sudo && server_metadata.user != "root" {
            // Authenticate sudo first using the normal PTY-aware command path.
            // The actual SCP protocol then runs with `sudo -n`, so the password
            // prompt never becomes part of the SCP byte stream.
            self.exec(server_metadata, task_name, "true", true)
                .await
                .map_err(|e| {
                    format!("Failed to authenticate sudo for SCP download: {}", e)
                })?;
        }

        let quoted_source = shlex::try_quote(remote_source)
            .map_err(|e| format!("Invalid remote path '{}': {}", remote_source, e))?;

        let mut channel_guard = self.get_channel(server_metadata).await?;

        // Directories require recursive mode, otherwise the remote sender
        // fails with `scp: <path>: not a regular file`.
        let command = if use_sudo && server_metadata.user != "root" {
            if source_is_dir {
                format!("sudo -n scp -r -f {}", quoted_source)
            } else {
                format!("sudo -n scp -f {}", quoted_source)
            }
        } else if source_is_dir {
            format!("scp -r -f {}", quoted_source)
        } else {
            format!("scp -f {}", quoted_source)
        };
        channel_guard
            .channel
            .exec(true, command.as_bytes().to_vec())
            .await
            .map_err(|e| format!("Failed to start remote SCP sender: {}", e))?;

        let mut pending = Vec::new();
        self.scp_send_ack(&mut channel_guard.channel).await?;
        self.scp_receive_entries(
            &mut channel_guard.channel,
            &mut pending,
            stage_dir,
            source_is_dir,
        )
        .await?;
        channel_guard
            .channel
            .close()
            .await
            .map_err(|e| e.to_string())?;
        log_debug!(
            server_metadata,
            task_name,
            "SCP download from '{}' completed",
            remote_source
        );
        Ok(())
    }

    async fn scp_send_ack(&self, channel: &mut russh::Channel<Msg>) -> Result<(), String> {
        let mut cursor = Cursor::new(&[0u8]);
        channel.data(&mut cursor).await.map_err(|e| e.to_string())
    }

    async fn scp_next_data(
        &self,
        channel: &mut russh::Channel<Msg>,
        pending: &mut Vec<u8>,
    ) -> Result<(), String> {
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    pending.extend_from_slice(&data);
                    return Ok(());
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    return Err(format!(
                        "SCP remote error: {}",
                        String::from_utf8_lossy(&data)
                    ));
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    return Err(format!("SCP process exited with status {}", exit_status));
                }
                _ => {}
            }
        }
        Err("SCP connection closed before transfer completed".to_string())
    }

    async fn scp_read_line(
        &self,
        channel: &mut russh::Channel<Msg>,
        pending: &mut Vec<u8>,
    ) -> Result<String, String> {
        loop {
            if let Some(pos) = pending.iter().position(|b| *b == b'\n') {
                let line = pending.drain(..=pos).collect::<Vec<_>>();
                return String::from_utf8(line[..line.len() - 1].to_vec())
                    .map_err(|e| format!("Invalid SCP header: {}", e));
            }
            self.scp_next_data(channel, pending).await?;
        }
    }

    async fn scp_read_exact(
        &self,
        channel: &mut russh::Channel<Msg>,
        pending: &mut Vec<u8>,
        size: usize,
        output: &mut File,
    ) -> Result<(), String> {
        let mut remaining = size;
        while remaining > 0 {
            if pending.is_empty() {
                self.scp_next_data(channel, pending).await?;
            }
            let take = remaining.min(pending.len());
            output
                .write_all(&pending[..take])
                .map_err(|e| e.to_string())?;
            pending.drain(..take);
            remaining -= take;
        }
        Ok(())
    }

    async fn scp_receive_entries(
        &self,
        channel: &mut russh::Channel<Msg>,
        pending: &mut Vec<u8>,
        root: &Path,
        source_is_dir: bool,
    ) -> Result<(), String> {
        let mut dirs: Vec<PathBuf> = vec![root.to_path_buf()];
        loop {
            if pending.is_empty() {
                self.scp_next_data(channel, pending).await?;
            }
            match pending[0] {
                0 => {
                    pending.remove(0);
                    continue;
                }
                1 | 2 => {
                    let code = pending.remove(0);
                    let msg = self.scp_read_line(channel, pending).await?;
                    return Err(format!("SCP remote error {}: {}", code, msg));
                }
                // `T<mtime> 0 <atime> 0` precedes the next C/D entry when the
                // sender preserves timestamps (e.g. `scp -r`): ack and continue.
                b'T' => {
                    let _ = self.scp_read_line(channel, pending).await?;
                    self.scp_send_ack(channel).await?;
                }
                b'D' => {
                    let line = self.scp_read_line(channel, pending).await?;
                    let mut parts = line[1..].splitn(2, ' ');
                    let _mode = parts.next().unwrap_or("0755");
                    let rest = parts.next().unwrap_or("");
                    let mut rest_parts = rest.splitn(2, ' ');
                    let _size = rest_parts.next().unwrap_or("0");
                    let name = rest_parts.next().unwrap_or("");
                    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
                        return Err(format!("Invalid SCP directory name '{}': {}", name, line));
                    }
                    let path = dirs.last().unwrap().join(name);
                    std::fs::create_dir_all(&path)
                        .map_err(|e| format!("Failed to create '{}': {}", path.display(), e))?;
                    self.scp_send_ack(channel).await?;
                    dirs.push(path);
                }
                b'E' => {
                    let _ = self.scp_read_line(channel, pending).await?;
                    if dirs.len() <= 1 {
                        self.scp_send_ack(channel).await?;
                        return Ok(());
                    }
                    dirs.pop();
                    self.scp_send_ack(channel).await?;
                    // Popping back to the staging root means the outermost
                    // directory closed: the sender will only send exit status
                    // now, so finish instead of looping into it.
                    if source_is_dir && dirs.len() <= 1 {
                        return Ok(());
                    }
                }
                b'C' => {
                    let line = self.scp_read_line(channel, pending).await?;
                    let mut parts = line[1..].splitn(3, ' ');
                    let _mode = parts.next().unwrap_or("0644");
                    let size: usize = parts
                        .next()
                        .unwrap_or("0")
                        .parse()
                        .map_err(|e| format!("Invalid SCP file size: {}", e))?;
                    let name = parts.next().unwrap_or("");
                    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
                        return Err(format!("Invalid SCP file name '{}': {}", name, line));
                    }
                    let path = dirs.last().unwrap().join(name);
                    let mut file = File::create(&path)
                        .map_err(|e| format!("Failed to create '{}': {}", path.display(), e))?;
                    self.scp_send_ack(channel).await?;
                    self.scp_read_exact(channel, pending, size, &mut file)
                        .await?;
                    self.scp_send_ack(channel).await?;
                    if !source_is_dir && dirs.len() == 1 {
                        return Ok(());
                    }
                }
                other => return Err(format!("Unexpected SCP protocol byte: {}", other)),
            }
        }
    }

    /// Move a file or directory, falling back to copy+delete when source and
    /// target live on different filesystems (`rename` gives EXDEV there,
    /// e.g. staging under /tmp vs target on /mnt/...).
    fn move_path_fallback_copy(src: &Path, dst: &Path) -> Result<(), String> {
        match std::fs::rename(src, dst) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(18) => Self::copy_path_recursive(src, dst)
                .and_then(|_| Self::remove_path_all(src)),
            Err(e) => Err(format!(
                "Failed to move '{}' to '{}': {}",
                src.display(),
                dst.display(),
                e
            )),
        }
    }

    fn copy_path_recursive(src: &Path, dst: &Path) -> Result<(), String> {
        let md = std::fs::symlink_metadata(src)
            .map_err(|e| format!("Failed to stat '{}': {}", src.display(), e))?;
        if md.file_type().is_symlink() {
            let target = std::fs::read_link(src)
                .map_err(|e| format!("Failed to read link '{}': {}", src.display(), e))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, dst)
                .map_err(|e| format!("Failed to create symlink '{}': {}", dst.display(), e))?;
            #[cfg(not(unix))]
            return Err(format!("Cannot copy symlink '{}' on this platform", src.display()));
            return Ok(());
        }
        if md.is_dir() {
            std::fs::create_dir_all(dst)
                .map_err(|e| format!("Failed to create '{}': {}", dst.display(), e))?;
            for child in std::fs::read_dir(src)
                .map_err(|e| format!("Failed to read '{}': {}", src.display(), e))?
            {
                let child = child.map_err(|e| e.to_string())?;
                Self::copy_path_recursive(&child.path(), &dst.join(child.file_name()))?;
            }
            return Ok(());
        }
        if let Some(parent) = dst.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create '{}': {}", parent.display(), e))?;
            }
        }
        std::fs::copy(src, dst).map_err(|e| {
            format!("Failed to copy '{}' to '{}': {}", src.display(), dst.display(), e)
        })?;
        Ok(())
    }

    fn remove_path_all(path: &Path) -> Result<(), String> {
        if !path.exists() && std::fs::symlink_metadata(path).is_err() {
            return Ok(());
        }
        let md = std::fs::symlink_metadata(path)
            .map_err(|e| format!("Failed to stat '{}': {}", path.display(), e))?;
        if md.is_dir() && !md.file_type().is_symlink() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
        .map_err(|e| format!("Failed to remove '{}': {}", path.display(), e))
    }

    async fn merge_download_stage(
        &self,
        stage_dir: &Path,
        local_dir: &Path,
        silent: bool,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
    ) -> Result<(), String> {
        let entries = std::fs::read_dir(stage_dir)
            .map_err(|e| format!("Failed to read download staging directory: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read download staging entry: {}", e))?;
        if entries.len() != 1 {
            return Err("SCP download produced an unexpected staging layout".to_string());
        }
        let source = entries[0].path();
        let target = local_dir.join(entries[0].file_name());
        if source.is_dir() {
            let children = std::fs::read_dir(&source).map_err(|e| {
                format!(
                    "Failed to read downloaded directory '{}': {}",
                    source.display(),
                    e
                )
            })?;
            for child in children {
                let child = child.map_err(|e| e.to_string())?;
                let target_child = local_dir.join(child.file_name());
                if target_child.exists() {
                    ask_user(
                        server_metadata,
                        task_name,
                        &format!(
                            "Local path '{}' already exists. Overwrite it?",
                            target_child.display()
                        ),
                        silent,
                    )
                    .await?;
                    if target_child.is_dir() {
                        std::fs::remove_dir_all(&target_child)
                    } else {
                        std::fs::remove_file(&target_child)
                    }
                    .map_err(|e| format!("Failed to remove '{}': {}", target_child.display(), e))?;
                }
                Self::move_path_fallback_copy(&child.path(), &target_child).map_err(|e| {
                    format!(
                        "Failed to move downloaded item to '{}': {}",
                        target_child.display(),
                        e
                    )
                })?;
            }
            let _ = std::fs::remove_dir_all(&source);
        } else {
            if target.exists() {
                ask_user(
                    server_metadata,
                    task_name,
                    &format!(
                        "Local path '{}' already exists. Overwrite it?",
                        target.display()
                    ),
                    silent,
                )
                .await?;
                if target.is_dir() {
                    std::fs::remove_dir_all(&target)
                } else {
                    std::fs::remove_file(&target)
                }
                .map_err(|e| format!("Failed to remove '{}': {}", target.display(), e))?;
            }
            Self::move_path_fallback_copy(&source, &target).map_err(|e| {
                format!(
                    "Failed to move downloaded file to '{}': {}",
                    target.display(),
                    e
                )
            })?;
        }
        Ok(())
    }

    async fn move_and_delete_temp_dir(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        temp_dir: &str,
        remote_dir: &str,
        use_sudo: bool,
    ) -> Result<(), String> {
        // println!("move_and_delete_temp_dir remote_dir: ---{}---",remote_dir);
        let hidden_exists = self
            .dir_has_hidden_items(server_metadata, task_name, temp_dir, use_sudo)
            .await?;
        if hidden_exists {
            self.exec(
                server_metadata,
                task_name,
                &format!("mv \"{0}\"/.[!.]* \"{1}\"/", temp_dir, remote_dir),
                use_sudo,
            )
            .await?;
        }
        let normal_exists = self
            .dir_has_normal_items(server_metadata, task_name, temp_dir, use_sudo)
            .await?;
        // mv will not move hidden items
        if normal_exists {
            self.exec(
                server_metadata,
                task_name,
                &format!("mv \"{0}\"/* \"{1}\"/", temp_dir, remote_dir),
                use_sudo,
            )
            .await?;
        }

        self.exec(
            server_metadata,
            task_name,
            &format!("rm -rf \"{}\"", temp_dir),
            use_sudo,
        )
        .await?;

        Ok(())
    }

    #[async_recursion]
    async fn do_upload_with_scp_recursive(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        local_file_or_dir: &Path,
        remote_dir: &str,
        use_sudo: bool,
        new_base_name: Option<&str>,
    ) -> Result<(), String> {
        if !local_file_or_dir.exists() {
            return Err(format!(
                "Local path '{}' does not exist",
                local_file_or_dir.display()
            ));
        }

        let base_name = new_base_name
            .map(|s| s.to_string())
            .unwrap_or(get_local_path_base_name(&local_file_or_dir)?);
        let remote_target = format!("{}/{}", remote_dir, base_name);

        if local_file_or_dir.is_dir() {
            self.create_remote_dir(server_metadata, task_name, remote_target.as_str(), use_sudo)
                .await?;
            for entry in std::fs::read_dir(local_file_or_dir).map_err(|e| {
                format!(
                    "Failed to read local directory '{}'. \n\t> {}",
                    local_file_or_dir.display(),
                    e
                )
            })? {
                let entry =
                    entry.map_err(|e| format!("Error reading directory entry. \n\t> {}", e))?;
                let sub_path = entry.path();

                self.do_upload_with_scp_recursive(
                    server_metadata,
                    task_name,
                    &sub_path,
                    &remote_target,
                    use_sudo,
                    None,
                )
                .await?;
            }
        } else {
            let metadata = std::fs::metadata(local_file_or_dir)
                .map_err(|e| format!("Failed to stat local file {:?}: {}", local_file_or_dir, e))?;
            let file_size = metadata.len();

        // Authenticate sudo through the normal PTY-aware command path first.
        // Then use `sudo -n` for the binary SCP protocol so sudo's password
        // prompt never gets mixed into SCP protocol bytes.
        if use_sudo && server_metadata.user != "root" {
            self.exec(server_metadata, task_name, "true", true)
                .await
                .map_err(|e| {
                    format!("Failed to authenticate sudo for SCP upload: {}", e)
                })?;
        }       

        let quoted_target = shlex::try_quote(&remote_target)
            .map_err(|e| format!("Invalid remote target '{}': {}", remote_target, e))?;     

        let scp_cmd = if use_sudo && server_metadata.user != "root" {
            // When no password is available (identity / key-based auth) use
            // non-interactive `sudo -n` so the SCP protocol does not block
            // waiting for a password prompt.
            let sudo_prefix = if server_metadata.password.is_empty() {
                "sudo -n"
            } else {
                "sudo -S -p '[RHCTL_SUDO_PASSWORD]'"
            };
            format!("{} scp -t {}", sudo_prefix, quoted_target)
        } else {
            format!("scp -t {}", quoted_target)
        };

        log_debug!(
            server_metadata,
            task_name,
            "SCP upload command: {} (file: {})",
            scp_cmd,
            local_file_or_dir.display()
        );


        let mut channel_guard = self.get_channel(server_metadata).await?;

            channel_guard
                .channel
                .exec(true, scp_cmd.as_bytes().to_vec())
                .await
                .map_err(|e| e.to_string())?;

            // If using sudo -S, feed the password to sudo before the SCP protocol starts.
            // sudo reads the password from stdin, then execs scp -t with the remaining stdin.
            // Skipped when no password is available (identity mode): `sudo -n` does
            // not prompt for a password.
            if use_sudo && server_metadata.user != "root" && !server_metadata.password.is_empty() {
                let password_with_newline = format!("{}
", server_metadata.password);
                channel_guard
                    .channel
                    .data(password_with_newline.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
            }

            // Wait for initial ACK (SCP handshake)
            self.wait_for_ack(
                &mut channel_guard.channel,
                "initial SCP handshake",
                local_file_or_dir,
            )
            .await?;

            // Send header
            let filename = local_file_or_dir.file_name().unwrap().to_str().unwrap();
            let mode = 0o644;
            let header = format!("C{:04o} {} {}\n", mode, file_size, filename);
            channel_guard
                .channel
                .data(header.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            self.wait_for_ack(
                &mut channel_guard.channel,
                "SCP header",
                local_file_or_dir,
            )
            .await?;

            // Send file data
            let mut reader =
                BufReader::new(File::open(local_file_or_dir).map_err(|e| e.to_string())?);
            let mut buf = [0u8; 65536];
            loop {
                let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                let mut cursor = Cursor::new(&buf[..n]);
                channel_guard
                    .channel
                    .data(&mut cursor)
                    .await
                    .map_err(|e| e.to_string())?;
            }

            // Send end null byte
            let mut cursor = Cursor::new(&[0u8]);
            channel_guard
                .channel
                .data(&mut cursor)
                .await
                .map_err(|e| e.to_string())?;

            // Wait for ACK after finishing file transfer
            self.wait_for_ack(
                &mut channel_guard.channel,
                "SCP file transfer complete",
                local_file_or_dir,
            )
            .await?;

            channel_guard
                .channel
                .close()
                .await
                .map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    async fn wait_for_ack(
        &self,
        channel: &mut russh::Channel<Msg>,
        stage: &str,
        local_path: &Path,
    ) -> Result<(), String> {
        let mut ack_buf = Vec::new();
        let mut stderr_buf = Vec::new();
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    ack_buf.extend_from_slice(&data);
                    if ack_buf.contains(&0) {
                        return Ok(());
                    } else if ack_buf.contains(&1) || ack_buf.contains(&2) {
                        return Err(format!(
                            "SCP remote error during '{}' for file '{}': {}",
                            stage,
                            local_path.display(),
                            String::from_utf8_lossy(&ack_buf),
                        ));
                    }
                }
                ChannelMsg::ExtendedData { ext: 1, data } => {
                    // ext code 1 is stderr on the SSH channel.
                    stderr_buf.extend_from_slice(&data);
                }
                ChannelMsg::ExtendedData { .. } => {
                    // Ignore other extended data codes.
                }
                _ => continue,
            }
        }
        let stderr_tail = Self::stderr_tail(&stderr_buf);
        Err(format!(
            "SCP: No ACK from server during '{}' for file '{}'{}",
            stage,
            local_path.display(),
            stderr_tail
        ))
    }
    fn stderr_tail(buf: &[u8]) -> String {
        let text = String::from_utf8_lossy(buf);
        let text = text.trim();
        if text.is_empty() {
            String::new()
        } else {
            // Keep the last N lines to provide context without flooding the error message.
            let lines: Vec<&str> = text.lines().rev().take(20).collect();
            let tail: String = lines.into_iter().rev().collect::<Vec<&str>>().join("\n");
            format!("\nRemote SCP stderr:\n{}", tail)
        }
    }

    fn command_exists(&self, cmd: &str) -> bool {
        std::process::Command::new("which")
            .arg(cmd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    async fn do_upload(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        use_sudo: bool,
        use_rsync: bool,
        local_file_or_dir: &Path,
        remote_dir: &str,
        new_file_name: Option<&str>,
    ) -> Result<(), String> {
        log_debug!(
            server_metadata,
            task_name,
            "Attempting to upload '{}' to '{}'",
            local_file_or_dir.display(),
            remote_dir
        );

        // rsync cannot use our sudo authentication flow for a non-root remote user.
        // Use direct SCP instead so the remote SCP process itself runs under sudo.
        if use_rsync
            && self.command_exists("rsync")
            && !(use_sudo && server_metadata.user != "root")
        {
            log_debug!(server_metadata, task_name, "Using rsync for upload");
            let remote_target = if let Some(name) = new_file_name {
                format!("{}/{}", remote_dir, name)
            } else {
                remote_dir.to_string()
            };
            let local_path_raw = local_file_or_dir
                .to_str()
                .ok_or("Invalid local path encoding")?;
            // Directory sources must merge CONTENTS into the target (same as
            // the scp path below). Without the trailing `/.`, rsync would
            // nest the source dir inside the target instead.
            let local_path = if local_file_or_dir.is_dir() {
                let trimmed = local_path_raw.trim_end_matches('/');
                if trimmed.is_empty() {
                    "/.".to_string()
                } else {
                    format!("{}/.", trimmed)
                }
            } else {
                local_path_raw.to_string()
            };
            let remote_spec = format!(
                "{}@{}:{}",
                server_metadata.user, server_metadata.host, remote_target
            );

            if let Some(ref identity_file) = server_metadata.identity_file {
                let identity_path = expand_local_user_path(identity_file);
                let ssh_cmd = format!("ssh -p {} -i {}", server_metadata.ssh_port, identity_path);
                let status = TokioCommand::new("rsync")
                    .arg("-avz")
                    .arg("-e")
                    .arg(&ssh_cmd)
                    .arg(local_path)
                    .arg(&remote_spec)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await
                    .map_err(|e| format!("Failed to execute rsync with identity. \n\t> {}", e))?;

                if !status.success() {
                    return Err(format!(
                        "rsync (identity) failed with exit code {:?}",
                        status.code()
                    ));
                }
                return Ok(());
            }

            if !server_metadata.password.is_empty() {
                if self.command_exists("sshpass") {
                    let mut sshpass_cmd = TokioCommand::new("sshpass");
                    sshpass_cmd
                        .arg("-p")
                        .arg(&server_metadata.password)
                        .arg("rsync")
                        .arg("-avz")
                        .arg("-e")
                        .arg(format!("ssh -p {}", server_metadata.ssh_port))
                        .arg(local_path)
                        .arg(&remote_spec)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());

                    let status = sshpass_cmd
                        .status()
                        .await
                        .map_err(|e| format!("Failed to execute rsync via sshpass. \n\t> {}", e))?;

                    if !status.success() {
                        return Err(format!(
                            "rsync (sshpass) failed with exit code {:?}",
                            status.code()
                        ));
                    }
                    return Ok(());
                } else {
                    return Err("rsync password required but sshpass not found".to_string());
                }
            }

            return Err(
                "rsync requires either an identity file or a password for SSH authentication"
                    .to_string(),
            );
        } else {
            log_debug!(
                server_metadata,
                task_name,
                "Starting SCP upload from '{}' to '{}'",
                local_file_or_dir.display(),
                remote_dir
            );
            self.do_upload_with_scp_recursive(
                server_metadata,
                task_name,
                local_file_or_dir,
                remote_dir,
                use_sudo,
                new_file_name,
            )
            .await?;
            log_debug!(
                server_metadata,
                task_name,
                "SCP upload to '{}' completed",
                remote_dir
            );
        }

        Ok(())
    }

    pub async fn download_file_to_file(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_file: &str,
        local_file: &Path,
        use_sudo: bool,
        use_rsync: bool,
        silent: bool,
    ) -> Result<(), String> {
        if local_file.exists() {
            if local_file.is_dir() {
                return Err(format!(
                    "Local path '{}' is a directory, cannot overwrite with a file",
                    local_file.display()
                ));
            }
            crate::utils::log_utils::ask_user(
                server_metadata,
                task_name,
                &format!(
                    "Local file '{}' already exists. Overwrite it?",
                    local_file.display()
                ),
                silent,
            )
            .await?;
            std::fs::remove_file(local_file).map_err(|e| {
                format!(
                    "Failed to remove existing local file '{}': {}",
                    local_file.display(),
                    e
                )
            })?;
        }
        if let Some(parent) = local_file.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "Failed to create local parent directory '{}': {}",
                        parent.display(),
                        e
                    )
                })?;
            }
        }
        let parent = local_file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let name = local_file
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                format!("Invalid local file name '{}'", local_file.display())
            })?;
        let staging = parent.join(format!(".rhctl-dl-{}", name));
        if staging.exists() {
            if staging.is_dir() {
                std::fs::remove_dir_all(&staging).map_err(|e| {
                    format!(
                        "Failed to remove staging path '{}': {}",
                        staging.display(),
                        e
                    )
                })?;
            } else {
                std::fs::remove_file(&staging).map_err(|e| {
                    format!(
                        "Failed to remove staging path '{}': {}",
                        staging.display(),
                        e
                    )
                })?;
            }
        }
        std::fs::create_dir_all(&staging).map_err(|e| {
            format!(
                "Failed to create staging directory '{}': {}",
                staging.display(),
                e
            )
        })?;
        let dl: Result<(), String> = self
            .download_file_or_dir_contents_into_dir(
                server_metadata,
                task_name,
                remote_file,
                &staging,
                use_sudo,
                use_rsync,
                true,
                true,
            )
            .await;
        if let Err(e) = dl {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
        let entries: Vec<_> = std::fs::read_dir(&staging)
            .map_err(|e| {
                format!(
                    "Failed to read staging directory '{}': {}",
                    staging.display(),
                    e
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        if entries.len() != 1 {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!(
                "Download produced an unexpected staging layout for '{}'",
                remote_file
            ));
        }
        let staged = entries[0].path();
        if let Err(e) = Self::move_path_fallback_copy(&staged, local_file) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!(
                "Failed to move downloaded file to '{}': {}",
                local_file.display(),
                e
            ));
        }
        let _ = std::fs::remove_dir_all(&staging);
        log_info!(
            server_metadata,
            task_name,
            "Successfully downloaded the file '{}' to '{}'",
            remote_file,
            local_file.display()
        );
        Ok(())
    }

    pub async fn download_file_into_dir(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_file: &str,
        local_dir: &Path,
        use_sudo: bool,
        use_rsync: bool,
        silent: bool,
    ) -> Result<(), String> {
        std::fs::create_dir_all(local_dir).map_err(|e| {
            format!(
                "Failed to create local download directory '{}': {}",
                local_dir.display(),
                e
            )
        })?;
        self.download_file_or_dir_contents_into_dir(
            server_metadata,
            task_name,
            remote_file,
            local_dir,
            use_sudo,
            use_rsync,
            silent,
            true,
        )
        .await
    }

    pub async fn download_dir_replace_dir(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_dir: &str,
        local_dir: &Path,
        use_sudo: bool,
        use_rsync: bool,
        silent: bool,
    ) -> Result<(), String> {
        if local_dir.exists() {
            crate::utils::log_utils::ask_user(
                server_metadata,
                task_name,
                &format!(
                    "Local directory '{}' already exists. Overwriting will DELETE it and all its contents. Continue?",
                    local_dir.display()
                ),
                silent,
            )
            .await?;
            if local_dir.is_dir() {
                std::fs::remove_dir_all(local_dir).map_err(|e| {
                    format!(
                        "Failed to remove existing local directory '{}': {}",
                        local_dir.display(),
                        e
                    )
                })?;
            } else {
                std::fs::remove_file(local_dir).map_err(|e| {
                    format!(
                        "Failed to remove existing local file '{}': {}",
                        local_dir.display(),
                        e
                    )
                })?;
            }
        }
        self.download_dir_to_new_path(
            server_metadata,
            task_name,
            remote_dir,
            local_dir,
            use_sudo,
            use_rsync,
            true,
        )
        .await
    }

    pub async fn download_dir_to_new_path(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_dir: &str,
        local_dir: &Path,
        use_sudo: bool,
        use_rsync: bool,
        silent: bool,
    ) -> Result<(), String> {
        std::fs::create_dir_all(local_dir).map_err(|e| {
            format!(
                "Failed to create local download directory '{}': {}",
                local_dir.display(),
                e
            )
        })?;
        self.download_file_or_dir_contents_into_dir(
            server_metadata,
            task_name,
            remote_dir,
            local_dir,
            use_sudo,
            use_rsync,
            true,
            true,
        )
        .await?;
        let _ = silent;
        log_info!(
            server_metadata,
            task_name,
            "Successfully downloaded the contents of the folder '{}' into '{}'",
            remote_dir,
            local_dir.display()
        );
        Ok(())
    }

    pub async fn ask_safe_to_transfer(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_path: &str,
        use_sudo: bool,
        silent: bool,
    ) -> Result<(), String> {
        let is_file = self
            .file_exists(server_metadata, task_name, remote_path, use_sudo)
            .await?;
        let is_dir = self
            .dir_exists(server_metadata, task_name, remote_path, use_sudo)
            .await?;

        if is_file || is_dir {
            let prompt = if is_file {
                format!(
                    "Remote file '{}' already exists. Overwriting will DELETE it. Continue?",
                    remote_path
                )
            } else {
                format!(
                    "Remote directory '{}' already exists. Overwriting will DELETE it and all its contents. Continue?",
                    remote_path
                )
            };

            ask_user(server_metadata, task_name, &prompt, silent).await?;
            if DANGEROUS_PATHS.contains(&remote_path) {
                return Err(format!(
                    "Refusing to delete critical system path: '{}'",
                    remote_path
                ));
            }
            self.exec(
                server_metadata,
                task_name,
                &format!("rm -rf \"{}\"", remote_path),
                use_sudo,
            )
            .await
            .map_err(|e| {
                format!(
                    "Failed to remove existing remote {} '{}'. \n\t> {}",
                    if is_file { "file" } else { "directory" },
                    remote_path,
                    e
                )
            })?;
            if silent {
                log_warn!(
                    server_metadata,
                    task_name,
                    "Remote {} has been overwritten: '{}'",
                    if is_file { "file" } else { "directory" },
                    remote_path
                );
            } else {
                log_info!(
                    server_metadata,
                    task_name,
                    "Remote {} has been overwritten: '{}'",
                    if is_file { "file" } else { "directory" },
                    remote_path
                );
            }
            return Ok(());
        }

        Ok(())
    }

    pub async fn validate_remote_dir(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_dir: &str,
        use_sudo: bool,
    ) -> Result<(), String> {
        log_debug!(
            server_metadata,
            task_name,
            "Ensuring remote directory '{}' exists",
            remote_dir
        );
        if self
            .file_exists(server_metadata, task_name, remote_dir, use_sudo)
            .await?
        {
            return Err(format!(
                "Path '{}' exists and is a file, not a directory",
                remote_dir
            ));
        }

        if self
            .dir_exists(server_metadata, task_name, remote_dir, use_sudo)
            .await?
        {
            log_debug!(
                server_metadata,
                task_name,
                "Checking if remote directory '{}' is writable",
                remote_dir
            );
            let check_cmd = format!("test -w \"{}\"; echo $?", remote_dir);
            let output = self
                .exec(server_metadata, task_name, &check_cmd, use_sudo)
                .await
                .map_err(|e| {
                    format!(
                        "Failed to check write permission for '{}'. \n\t> {}",
                        remote_dir, e
                    )
                })?;
            if output.trim() != "0" {
                return Err(format!("Directory '{}' is not writable", remote_dir));
            }
        } else {
            self.create_remote_dir(server_metadata, task_name, remote_dir, use_sudo)
                .await?;
        }

        Ok(())
    }

    pub async fn create_remote_dir_if_not_exists(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_dir: &str,
        use_sudo: bool,
    ) -> Result<(), String> {
        if self
            .dir_exists(server_metadata, task_name, remote_dir, use_sudo)
            .await?
        {
            return Ok(());
        }
        self.create_remote_dir(server_metadata, task_name, remote_dir, use_sudo)
            .await
    }

    async fn create_remote_dir(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        remote_dir: &str,
        use_sudo: bool,
    ) -> Result<(), String> {
        let cmd = if use_sudo {
            format!(
                "mkdir -p \"{}\"; chown {} \"{}\"; chmod 700 \"{}\"",
                remote_dir, server_metadata.user, remote_dir, remote_dir
            )
        } else {
            format!("mkdir -p \"{}\"; chmod 700 \"{}\"", remote_dir, remote_dir)
        };

        self.exec(server_metadata, task_name, &cmd, use_sudo)
            .await
            .map_err(|e| {
                format!(
                    "Failed to create remote directory '{}'. \n\t> {}",
                    remote_dir, e
                )
            })?;
        Ok(())
    }

    pub async fn create_remote_temp_dir(
        &self,
        server_metadata: &Arc<ServerMetadata>,
        task_name: &str,
        prefix: &str,
        use_sudo: bool,
    ) -> Result<String, String> {
        let temp_dir = generate_remote_temp_dir(prefix);
        log_debug!(
            server_metadata,
            task_name,
            "Uploading to temporary path '{}' with sudo",
            temp_dir
        );
        self.create_remote_dir_if_not_exists(
            server_metadata,
            task_name,
            temp_dir.as_str(),
            use_sudo,
        )
        .await?;
        Ok(temp_dir)
    }
}

/// True when `err` reports exactly this numeric exit status (not a prefix match like 1 vs 10).
fn is_exit_status(err: &str, code: u32) -> bool {
    let marker = "exit status ";
    let Some(rest) = err.split(marker).nth(1) else {
        return false;
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u32>().ok() == Some(code)
}

