//! HTTP Server implementation using warp

use super::{LogMessage, ServerConfig, ServerError, ServerHandle, ServerStatus, SharedState};
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::sync::mpsc;
use warp::Filter;

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

/// Generate HTML for directory listing
fn generate_directory_listing(path: &std::path::Path, request_path: &str) -> Option<String> {
    let entries = std::fs::read_dir(path).ok()?;

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
        request_path, request_path
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

        let (icon, href, size_str) = if is_dir {
            ("📂", format!("{}/", file_name_str), "-".to_string())
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let size_str = format_size(size);
            ("📄", file_name_str.to_string(), size_str)
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
            icon, href, file_name_str, size_str, modified
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
    {
        let mut s = state.write();
        s.status = ServerStatus::Starting;
        s.add_log(LogMessage::info(format!(
            "Starting HTTP server on port {}...",
            port
        )));
    }

    // Clone root for use in filters
    let root_for_listing = root.clone();

    // Directory listing handler
    let listing_root = root.clone();
    let dir_listing =
        warp::path::tail()
            .and(warp::get())
            .and_then(move |tail: warp::path::Tail| {
                let root = listing_root.clone();
                let allow = allow_listing;
                async move {
                    let request_path = format!("/{}", tail.as_str());
                    let full_path = root.join(tail.as_str());

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
                                return Ok(warp::reply::html(html));
                            }
                        }
                    }
                    Err(warp::reject::not_found())
                }
            });

    // Serve files
    let files = warp::fs::dir(root_for_listing);

    // Add logging
    let log_state = state.clone();
    let log = warp::log::custom(move |info| {
        let msg = format!(
            "{} {} {} {}ms",
            info.method(),
            info.path(),
            info.status().as_u16(),
            info.elapsed().as_millis()
        );
        log_state.write().add_log(LogMessage::info(msg));
    });

    // Combine routes: try dir listing first, then files
    let routes = dir_listing.or(files).with(log);

    let addr: SocketAddr = ([0, 0, 0, 0], port).into();

    // Update status to running
    {
        let mut s = state.write();
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

    // Create server with graceful shutdown
    let (_, server) = warp::serve(routes).bind_with_graceful_shutdown(addr, async move {
        shutdown_rx.recv().await;
    });

    // Handle auto-stop timeout
    if let Some(timeout_secs) = config.auto_stop_seconds {
        let state_clone = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(timeout_secs)).await;
            let mut s = state_clone.write();
            if matches!(s.status, ServerStatus::Running) {
                s.add_log(LogMessage::info(format!(
                    "Auto-stopping after {} seconds of inactivity",
                    timeout_secs
                )));
            }
        });
    }

    server.await;

    // Update status
    {
        let mut s = state.write();
        s.status = ServerStatus::Stopped;
        s.add_log(LogMessage::info("HTTP server stopped"));
    }

    Ok(())
}

/// Create a new HTTP server handle
#[allow(dead_code)]
pub fn create_handle(config: HttpConfig) -> ServerHandle {
    ServerHandle::new(config.into())
}
