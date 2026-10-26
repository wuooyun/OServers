//! HTTP Server implementation using warp

use super::{LogMessage, ServerConfig, ServerError, ServerHandle, ServerStatus, SharedState};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use warp::Filter;

/// Characters that must be percent-encoded inside a single URL path segment.
///
/// Non-ASCII bytes (e.g. Chinese file names) are always encoded by
/// `utf8_percent_encode`, so directory names such as `中文目录` produce the
/// link `%E4%B8%AD%E6%96%87%E7%9B%AE%E5%BD%95/`.
const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'\\');

/// Maximum time (in seconds) we wait for warp's graceful shutdown to
/// drain existing connections before forcing the server to stop.
///
/// Hyper/warp keep existing HTTP connections alive by default for up to
/// ~20 seconds after a response, so browsers (and curl with keep-alive)
/// can hold the server future open for a long time on stop. This cap
/// ensures the UI returns promptly while still allowing in-flight
/// requests to finish.
const SHUTDOWN_TIMEOUT_SECS: u64 = 2;

/// HTTP server specific configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HttpConfig {
    pub root_dir: PathBuf,
    pub port: u16,
    pub allow_directory_listing: bool,
    pub auto_stop_seconds: Option<u64>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            root_dir: std::env::current_dir().unwrap_or_default(),
            port: 7777,
            allow_directory_listing: true,
            auto_stop_seconds: Some(360),
        }
    }
}

impl From<HttpConfig> for ServerConfig {
    fn from(cfg: HttpConfig) -> Self {
        ServerConfig {
            root_dir: cfg.root_dir,
            port: cfg.port,
            auto_stop_seconds: cfg.auto_stop_seconds,
        }
    }
}

/// Percent-encode a single path segment so generated links stay valid for
/// names containing non-ASCII characters, spaces, `#`, `%`, `&`, ...
fn encode_path_segment(segment: &str) -> String {
    utf8_percent_encode(segment, PATH_SEGMENT_ENCODE_SET).to_string()
}

/// Minimal HTML escaping for text interpolated into the listing page.
///
/// File names are user-controlled, so they must never be injected verbatim
/// into the HTML (a name like `<script>` would otherwise be executed).
fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Decode a request path tail (as returned by `warp::path::Tail`) into a
/// filesystem path relative to the served root.
///
/// `Tail` yields the raw, still percent-encoded path, so a request such as
/// `GET /%E4%B8%AD%E6%96%87/` arrives as `%E4%B8%AD%E6%96%87/`. It has to be
/// percent-decoded before being used on the filesystem, otherwise directories
/// and files with Chinese names can never be found.
///
/// Returns `(display_path, relative_path)` where `display_path` is the
/// decoded, slash-separated path used in logs and HTML, and `relative_path`
/// is safe to join onto the root. Returns `None` for undecodable input or for
/// paths that would escape the root (`..`, drive letters, separators inside a
/// segment, ...).
fn decode_request_tail(tail: &str) -> Option<(String, PathBuf)> {
    let decoded = percent_encoding::percent_decode_str(tail).decode_utf8().ok()?;
    let mut relative = PathBuf::new();

    for segment in decoded.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }

        // Every URL segment must map to exactly one plain filesystem
        // component; anything else (e.g. `..`, `C:`, `a\b`) is rejected so
        // that a crafted request cannot list or serve files outside root.
        let mut components = Path::new(segment).components();
        match (components.next(), components.next()) {
            (Some(std::path::Component::Normal(name)), None) => relative.push(name),
            _ => return None,
        }
    }

    Some((decoded.trim_matches('/').to_string(), relative))
}

/// Generate HTML for directory listing
fn generate_directory_listing(path: &std::path::Path, request_path: &str) -> Option<String> {
    let entries = std::fs::read_dir(path).ok()?;
    let display_path = escape_html(request_path);

    let mut html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Index of {}</title>
    <style>
        body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; margin: 20px; background: #f5f5f5; }}
        h1 {{ color: #333; border-bottom: 2px solid #4CAF50; padding-bottom: 10px; }}
        table {{ border-collapse: collapse; width: 100%; background: white; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }}
        th, td {{ padding: 12px 15px; text-align: left; border-bottom: 1px solid #ddd; }}
        th {{ background: #4CAF50; color: white; }}
        tr:hover {{ background: #f1f1f1; }}
        a {{ color: #1976D2; text-decoration: none; }}
        a:hover {{ text-decoration: underline; }}
        .icon {{ margin-right: 8px; }}
        .size {{ color: #666; }}
        .date {{ color: #888; }}
    </style>
</head>
<body>
    <h1>📁 Index of {}</h1>
    <table>
        <tr><th>Name</th><th>Size</th><th>Modified</th></tr>
"#,
        display_path, display_path
    );

    // Add parent directory link if not at root
    if request_path != "/" {
        html.push_str(r#"        <tr><td><span class="icon">📂</span><a href="../">..</a></td><td>-</td><td>-</td></tr>
"#);
    }

    // Collect and sort entries
    let mut items: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    items.sort_by(|a, b| {
        let a_is_dir = a.path().is_dir();
        let b_is_dir = b.path().is_dir();
        match (a_is_dir, b_is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.file_name().cmp(&b.file_name()),
        }
    });

    for entry in items {
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();
        let entry_path = entry.path();
        let is_dir = entry_path.is_dir();

        // The href is percent-encoded (Chinese names become %E4%B8%AD...),
        // while the visible text is HTML-escaped.
        let href = encode_path_segment(&file_name_str);
        let text = escape_html(&file_name_str);

        let (icon, href, text, size_str) = if is_dir {
            ("📂", format!("{}/", href), text, "-".to_string())
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let size_str = format_size(size);
            ("📄", href, text, size_str)
        };

        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| {
                let datetime: chrono::DateTime<chrono::Local> = t.into();
                datetime.format("%Y-%m-%d %H:%M").to_string()
            })
            .unwrap_or_else(|_| "-".to_string());

        html.push_str(&format!(
            r#"        <tr><td><span class="icon">{}</span><a href="{}">{}</a></td><td class="size">{}</td><td class="date">{}</td></tr>
"#,
            icon, href, text, size_str, modified
        ));
    }

    html.push_str(
        r#"    </table>
    <p style="color:#888;margin-top:20px;font-size:12px;">OServers HTTP Server</p>
</body>
</html>"#,
    );

    Some(html)
}

/// Format a client address for logging
fn client_label(addr: Option<SocketAddr>) -> String {
    match addr {
        Some(addr) => addr.to_string(),
        None => "-".to_string(),
    }
}

/// Format file size in human readable format
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Start HTTP server
pub async fn start_server(
    config: HttpConfig,
    state: SharedState,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> Result<(), ServerError> {
    let root = config.root_dir.clone();
    let port = config.port;
    let allow_listing = config.allow_directory_listing;

    // Update status
    let run_id = state.write().begin_run(format!(
        "Starting HTTP server on port {}...",
        port
    ));

    // Clone root for use in filters
    let root_for_listing = root.clone();

    // Directory listing handler
    let listing_root = root.clone();
    let listing_state = state.clone();
    let dir_listing = warp::path::tail()
        .and(warp::get())
        .and(warp::addr::remote())
        .and_then(
            move |tail: warp::path::Tail, remote_addr: Option<SocketAddr>| {
                let root = listing_root.clone();
                let state = listing_state.clone();
                let allow = allow_listing;
                async move {
                    let client = client_label(remote_addr);

                    // `tail` is still percent-encoded; decode it (and reject
                    // unsafe paths) before touching the filesystem.
                    let Some((decoded_path, relative_path)) = decode_request_tail(tail.as_str())
                    else {
                        state.write().add_log(LogMessage::warning(format!(
                            "[HTTP] client={} LIST /{} denied (invalid or unsafe path)",
                            client,
                            tail.as_str()
                        )));
                        return Err(warp::reject::not_found());
                    };

                    let request_path = format!("/{}", decoded_path);
                    let full_path = root.join(&relative_path);

                    // Check if it's a directory
                    if full_path.is_dir() {
                        // Check for index.html first
                        let index_path = full_path.join("index.html");
                        if index_path.exists() {
                            // Let the file server handle index.html
                            return Err(warp::reject::not_found());
                        }

                        if allow {
                            if let Some(html) =
                                generate_directory_listing(&full_path, &request_path)
                            {
                                state.write().add_log(LogMessage::info(format!(
                                    "[HTTP] client={} LIST {} (directory listing)",
                                    client, request_path
                                )));
                                return Ok(warp::reply::html(html));
                            }
                        }
                        state.write().add_log(LogMessage::warning(format!(
                            "[HTTP] client={} LIST {} denied (directory listing disabled or unreadable)",
                            client, request_path
                        )));
                    }
                    Err(warp::reject::not_found())
                }
            },
        );

    // Serve files
    let files = warp::fs::dir(root_for_listing);

    // Log every request together with the client address, method, path,
    // response status, duration and user agent.
    let log_state = state.clone();
    let log = warp::log::custom(move |info| {
        let client = client_label(info.remote_addr());
        let method = info.method().as_str();
        // Show Chinese (and other non-ASCII) paths readably instead of the
        // raw `%E4%B8%AD...` encoding.
        let path = percent_encoding::percent_decode_str(info.path())
            .decode_utf8_lossy()
            .into_owned();
        let status = info.status().as_u16();
        let elapsed = info.elapsed().as_millis();
        let user_agent = info.user_agent().unwrap_or("-");

        let message = format!(
            "[HTTP] client={} {} {} -> {} ({}ms) ua=\"{}\"",
            client, method, path, status, elapsed, user_agent
        );

        let mut s = log_state.write();
        if status >= 500 {
            s.add_log(LogMessage::error(message));
        } else if status >= 400 {
            s.add_log(LogMessage::warning(message));
        } else {
            s.add_log(LogMessage::info(message));
        }
    });

    // Combine routes: try dir listing first, then files
    let routes = dir_listing.or(files).with(log);

    let addr: SocketAddr = ([0, 0, 0, 0], port).into();

    // The graceful-shutdown signal passed to warp must only fire when the
    // user asks to stop the server. Using a Notify lets the main loop below
    // keep serving indefinitely until that happens.
    let drain = Arc::new(tokio::sync::Notify::new());
    let drain_signal = drain.clone();

    // Bind before reporting Running: a bind failure is then reported as an
    // error instead of leaving the UI showing a running server that isn't.
    let server = match warp::serve(routes).try_bind_with_graceful_shutdown(addr, async move {
        drain_signal.notified().await;
    }) {
        Ok((_, server)) => server,
        Err(e) => {
            let mut s = state.write();
            if s.is_active_run(run_id) {
                s.status = ServerStatus::Error(e.to_string());
                s.add_log(LogMessage::error(format!(
                    "Failed to bind HTTP server on port {}: {}",
                    port, e
                )));
            }
            return Err(ServerError::Other(e.to_string()));
        }
    };

    // A newer run may have superseded this one while we were binding.
    // Drop the listener and leave the status to the newer run.
    if !state.read().is_active_run(run_id) {
        return Ok(());
    }

    // Update status to running
    {
        let mut s = state.write();
        if s.is_active_run(run_id) {
            s.status = ServerStatus::Running;
            s.add_log(LogMessage::info(format!(
                "HTTP server started on http://0.0.0.0:{}",
                port
            )));
            s.add_log(LogMessage::info(format!(
                "Serving files from: {}",
                root.display()
            )));
            if allow_listing {
                s.add_log(LogMessage::info("Directory listing: enabled"));
            }
        }
    }

    // Handle auto-stop timeout
    if let Some(timeout_secs) = config.auto_stop_seconds {
        let state_clone = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(timeout_secs)).await;
            let mut s = state_clone.write();
            if s.is_active_run(run_id) && matches!(s.status, ServerStatus::Running) {
                s.add_log(LogMessage::info(format!(
                    "Auto-stopping after {} seconds of inactivity",
                    timeout_secs
                )));
            }
        });
    }

    // Wait until the user asks to stop the server (or the previous sender is
    // dropped). Only then does warp begin draining; the timeout applies to
    // the drain phase, not to normal serving.
    let mut server = std::pin::pin!(server);
    tokio::select! {
        _ = &mut server => {
            // Server finished on its own (listener closed or fatal error)
        }
        _ = shutdown_rx.recv() => {
            // Ask warp to stop accepting and drain active connections.
            drain.notify_one();
            if tokio::time::timeout(
                Duration::from_secs(SHUTDOWN_TIMEOUT_SECS),
                &mut server,
            )
            .await
            .is_err()
            {
                let mut s = state.write();
                if s.is_active_run(run_id) {
                    s.add_log(LogMessage::warning(format!(
                        "HTTP server did not stop within {}s, forcing shutdown",
                        SHUTDOWN_TIMEOUT_SECS
                    )));
                }
            }
        }
    }

    // Update status, but only if this run is still the current one. Without
    // this check a task that is shutting down after a restart would set the
    // status to Stopped while the new run is actually serving requests.
    {
        let mut s = state.write();
        if s.is_active_run(run_id) {
            s.status = ServerStatus::Stopped;
            s.add_log(LogMessage::info("HTTP server stopped"));
        }
    }

    Ok(())
}

/// Create a new HTTP server handle
#[allow(dead_code)]
pub fn create_handle(config: HttpConfig) -> ServerHandle {
    ServerHandle::new(config.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servers::{ServerConfig, ServerState};
    use std::io::{Read, Write};
    use std::sync::Arc;

    fn get_free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn wait_for_status(state: &SharedState, expected: ServerStatus) -> bool {
        for _ in 0..100 {
            if state.read().status == expected {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    fn http_get(port: u16, path: &str) -> String {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            stream,
            "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            path
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn test_http_chinese_directory_is_listed_and_served() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let port = get_free_port();
        let test_dir = std::env::temp_dir().join(format!(
            "http_utf8_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        let chinese_dir = test_dir.join("中文目录");
        std::fs::create_dir_all(&chinese_dir).unwrap();
        std::fs::write(chinese_dir.join("测试.txt"), b"hello-chinese").unwrap();

        let state = Arc::new(parking_lot::RwLock::new(ServerState::new(ServerConfig {
            root_dir: test_dir.clone(),
            port,
            auto_stop_seconds: None,
        })));

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let config = HttpConfig {
            root_dir: test_dir.clone(),
            port,
            allow_directory_listing: true,
            auto_stop_seconds: None,
        };

        let state_clone = state.clone();
        let handle = runtime.spawn(async move {
            start_server(config, state_clone, shutdown_rx).await
        });

        assert!(wait_for_status(&state, ServerStatus::Running));

        // The root listing must link to the Chinese directory in
        // percent-encoded form.
        let root_listing = http_get(port, "/");
        assert!(root_listing.contains("200 OK"), "{}", root_listing);
        assert!(
            root_listing.contains(&format!("{}/", encode_path_segment("中文目录"))),
            "root listing does not link to the Chinese directory: {}",
            root_listing
        );

        // Browsers send the percent-encoded form; the listing filter has to
        // decode it before looking at the filesystem.
        let dir_url = format!("/{}/", encode_path_segment("中文目录"));
        let dir_listing = http_get(port, &dir_url);
        assert!(
            dir_listing.contains("200 OK"),
            "Chinese directory was not listed: {}",
            dir_listing
        );
        assert!(
            dir_listing.contains("测试.txt"),
            "file name missing from listing: {}",
            dir_listing
        );

        // Files inside a Chinese directory must be downloadable through the
        // same encoded path.
        let file_url = format!("{}{}", dir_url, encode_path_segment("测试.txt"));
        let file_response = http_get(port, &file_url);
        assert!(
            file_response.contains("200 OK") && file_response.contains("hello-chinese"),
            "Chinese file was not served: {}",
            file_response
        );

        runtime.block_on(async {
            shutdown_tx.send(()).await.unwrap();
        });
        runtime.block_on(handle).unwrap().unwrap();

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_http_rejects_path_traversal() {
        // Directory listing must never resolve a path outside the root.
        assert!(decode_request_tail("../").is_none());
        assert!(decode_request_tail("..%2F").is_none());
        assert!(decode_request_tail("a/../../b").is_none());
        // Encoded slashes must not smuggle in a parent segment.
        assert!(decode_request_tail("%2E%2E/").is_none());

        // Normal paths (including non-ASCII ones) are accepted.
        let (display, relative) = decode_request_tail("%E4%B8%AD%E6%96%87/file.txt").unwrap();
        assert_eq!(display, "中文/file.txt");
        assert_eq!(relative, PathBuf::from("中文").join("file.txt"));
    }

    #[test]
    fn test_http_server_stays_running_after_requests() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let port = get_free_port();
        let test_dir = std::env::temp_dir().join(format!(
            "http_state_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        std::fs::create_dir_all(&test_dir).unwrap();
        std::fs::write(test_dir.join("hello.txt"), b"hello").unwrap();

        let state = Arc::new(parking_lot::RwLock::new(ServerState::new(ServerConfig {
            root_dir: test_dir.clone(),
            port,
            auto_stop_seconds: None,
        })));

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let config = HttpConfig {
            root_dir: test_dir.clone(),
            port,
            allow_directory_listing: true,
            auto_stop_seconds: None,
        };

        let state_clone = state.clone();
        let handle = runtime.spawn(async move {
            start_server(config, state_clone, shutdown_rx).await
        });

        assert!(
            wait_for_status(&state, ServerStatus::Running),
            "server did not reach Running, status: {:?}",
            state.read().status
        );

        // Exercise the server, then make sure the status is still Running.
        let response = http_get(port, "/hello.txt");
        assert!(response.contains("200 OK"), "unexpected response: {}", response);

        // Wait longer than SHUTDOWN_TIMEOUT_SECS: the drain timeout must
        // only apply after a stop request, not during normal serving.
        std::thread::sleep(std::time::Duration::from_millis(
            (SHUTDOWN_TIMEOUT_SECS + 1) * 1000,
        ));
        assert_eq!(
            state.read().status,
            ServerStatus::Running,
            "status changed while the server was still running"
        );

        // The server must still answer requests at this point.
        let response = http_get(port, "/hello.txt");
        assert!(
            response.contains("200 OK"),
            "server stopped responding: {}",
            response
        );

        runtime.block_on(async {
            shutdown_tx.send(()).await.unwrap();
        });
        runtime.block_on(handle).unwrap().unwrap();
        assert_eq!(state.read().status, ServerStatus::Stopped);

        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_http_shutdown_is_bounded_with_idle_connection() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let port = get_free_port();
        let test_dir = std::env::temp_dir().join(format!(
            "http_shutdown_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        std::fs::create_dir_all(&test_dir).unwrap();
        std::fs::write(test_dir.join("hello.txt"), b"hello").unwrap();

        let state = Arc::new(parking_lot::RwLock::new(ServerState::new(ServerConfig {
            root_dir: test_dir.clone(),
            port,
            auto_stop_seconds: None,
        })));

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let config = HttpConfig {
            root_dir: test_dir.clone(),
            port,
            allow_directory_listing: true,
            auto_stop_seconds: None,
        };

        let state_clone = state.clone();
        let handle = runtime.spawn(async move {
            start_server(config, state_clone, shutdown_rx).await
        });

        assert!(wait_for_status(&state, ServerStatus::Running));

        // Open a keep-alive connection and leave it idle after one request.
        let mut idle_stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            idle_stream,
            "GET /hello.txt HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
        )
        .unwrap();
        let mut buf = [0u8; 1024];
        let _ = idle_stream.read(&mut buf).unwrap();

        // Stopping must not hang on the idle connection.
        let started = std::time::Instant::now();
        runtime.block_on(async {
            shutdown_tx.send(()).await.unwrap();
        });
        runtime.block_on(handle).unwrap().unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(SHUTDOWN_TIMEOUT_SECS + 2),
            "shutdown took {:?}",
            started.elapsed()
        );
        assert_eq!(state.read().status, ServerStatus::Stopped);

        drop(idle_stream);
        let _ = std::fs::remove_dir_all(&test_dir);
    }
}
