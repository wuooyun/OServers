//! Main application GUI using egui

use crate::config::AppConfig;
use crate::servers::{
    ftp::{self, FtpConfig},
    http::{self, HttpConfig},
    ssh::{self, SshConfig},
    tftp::{self, TftpConfig},
    LogLevel, LogMessage, ServerStatus, SharedState,
};
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::RwLock;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

/// Server type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerType {
    Http,
    Ftp,
    Tftp,
    Ssh,
}

impl ServerType {
    const ALL: [ServerType; 4] = [
        ServerType::Http,
        ServerType::Ftp,
        ServerType::Tftp,
        ServerType::Ssh,
    ];

    fn name(&self) -> &'static str {
        match self {
            ServerType::Http => "HTTP Server",
            ServerType::Ftp => "FTP Server",
            ServerType::Tftp => "TFTP Server",
            ServerType::Ssh => "SSH/SFTP Server",
        }
    }

    fn default_port(&self) -> u16 {
        match self {
            ServerType::Http => 7777,
            ServerType::Ftp => 2121,
            ServerType::Tftp => 69,
            ServerType::Ssh => 2222,
        }
    }
}

/// Server state wrapper
struct ServerEntry {
    server_type: ServerType,
    state: SharedState,
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl ServerEntry {
    fn new(server_type: ServerType) -> Self {
        let config = crate::servers::ServerConfig {
            root_dir: std::env::current_dir().unwrap_or_default(),
            port: server_type.default_port(),
            auto_stop_seconds: None,
        };
        Self {
            server_type,
            state: Arc::new(RwLock::new(crate::servers::ServerState::new(config))),
            shutdown_tx: None,
        }
    }

    fn status(&self) -> ServerStatus {
        self.state.read().status.clone()
    }

    fn is_running(&self) -> bool {
        matches!(self.status(), ServerStatus::Running)
    }

    fn logs(&self) -> Vec<LogMessage> {
        self.state.read().logs.clone()
    }
}

/// Main application state
pub struct OServersApp {
    config: AppConfig,
    servers: Vec<ServerEntry>,
    selected_server: Option<usize>,
    runtime: Arc<Runtime>,
    
    // Temporary UI state for editing
    http_port: String,
    http_root_dir: String,
    http_allow_listing: bool,
    http_auto_stop: bool,
    http_auto_stop_secs: String,
    
    ftp_port: String,
    ftp_root_dir: String,
    ftp_username: String,
    ftp_password: String,
    ftp_anonymous: bool,
    ftp_passive_mode: bool,
    ftp_passive_ports_start: String,
    ftp_passive_ports_end: String,
    
    tftp_port: String,
    tftp_root_dir: String,
    tftp_read_only: bool,
    
    ssh_port: String,
    ssh_root_dir: String,
    ssh_username: String,
    ssh_password: String,
}

impl OServersApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Configure Chinese font support
        Self::setup_fonts(&cc.egui_ctx);
        
        let config = AppConfig::load();
        let runtime = Arc::new(Runtime::new().expect("Failed to create tokio runtime"));
        
        let servers = ServerType::ALL
            .iter()
            .map(|&st| ServerEntry::new(st))
            .collect();


        Self {
            http_port: config.http.port.to_string(),
            http_root_dir: config.http.root_dir.display().to_string(),
            http_allow_listing: config.http.allow_directory_listing,
            http_auto_stop: config.http.auto_stop_seconds.is_some(),
            http_auto_stop_secs: config.http.auto_stop_seconds.unwrap_or(360).to_string(),
            
            ftp_port: config.ftp.port.to_string(),
            ftp_root_dir: config.ftp.root_dir.display().to_string(),
            ftp_username: config.ftp.username.clone(),
            ftp_password: config.ftp.password.clone(),
            ftp_anonymous: config.ftp.anonymous_access,
            ftp_passive_mode: config.ftp.passive_mode,
            ftp_passive_ports_start: config.ftp.passive_ports.0.to_string(),
            ftp_passive_ports_end: config.ftp.passive_ports.1.to_string(),
            
            tftp_port: config.tftp.port.to_string(),
            tftp_root_dir: config.tftp.root_dir.display().to_string(),
            tftp_read_only: config.tftp.read_only,
            
            ssh_port: config.ssh.port.to_string(),
            ssh_root_dir: config.ssh.root_dir.display().to_string(),
            ssh_username: config.ssh.username.clone(),
            ssh_password: config.ssh.password.clone(),
            
            config,
            servers,
            selected_server: Some(0),
            runtime,
        }
    }

    /// Setup Chinese font support
    fn setup_fonts(ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();
        
        // Try to load Microsoft YaHei (微软雅黑) from Windows
        let font_paths = [
            "C:\\Windows\\Fonts\\msyh.ttc",      // Microsoft YaHei
            "C:\\Windows\\Fonts\\msyhbd.ttc",    // Microsoft YaHei Bold
            "C:\\Windows\\Fonts\\simsun.ttc",    // SimSun
            "C:\\Windows\\Fonts\\simhei.ttf",    // SimHei
        ];
        
        for font_path in &font_paths {
            if let Ok(font_data) = std::fs::read(font_path) {
                fonts.font_data.insert(
                    "chinese_font".to_owned(),
                    std::sync::Arc::new(egui::FontData::from_owned(font_data)),
                );
                
                // Add Chinese font as FALLBACK (push to end, not insert at 0)
                // This way the default font handles emojis, Chinese font handles CJK characters
                fonts.families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .push("chinese_font".to_owned());
                
                // Add to monospace fonts (used for code/logs)
                fonts.families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .push("chinese_font".to_owned());
                
                tracing::info!("Loaded Chinese font from: {}", font_path);
                break;
            }
        }
        
        ctx.set_fonts(fonts);
    }

    fn start_server(&mut self, idx: usize) {
        let entry = &mut self.servers[idx];
        if entry.is_running() {
            return;
        }

        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        entry.shutdown_tx = Some(shutdown_tx);
        let state = entry.state.clone();

        match entry.server_type {
            ServerType::Http => {
                let config = HttpConfig {
                    port: self.http_port.parse().unwrap_or(7777),
                    root_dir: PathBuf::from(&self.http_root_dir),
                    allow_directory_listing: self.http_allow_listing,
                    auto_stop_seconds: if self.http_auto_stop {
                        self.http_auto_stop_secs.parse().ok()
                    } else {
                        None
                    },
                };
                self.runtime.spawn(async move {
                    let _ = http::start_server(config, state, shutdown_rx).await;
                });
            }
            ServerType::Ftp => {
                let config = FtpConfig {
                    port: self.ftp_port.parse().unwrap_or(2121),
                    root_dir: PathBuf::from(&self.ftp_root_dir),
                    username: self.ftp_username.clone(),
                    password: self.ftp_password.clone(),
                    anonymous_access: self.ftp_anonymous,
                    passive_mode: self.ftp_passive_mode,
                    passive_ports: (
                        self.ftp_passive_ports_start.parse().unwrap_or(50000),
                        self.ftp_passive_ports_end.parse().unwrap_or(50100),
                    ),
                };
                self.runtime.spawn(async move {
                    let _ = ftp::start_server(config, state, shutdown_rx).await;
                });
            }
            ServerType::Tftp => {
                let config = TftpConfig {
                    port: self.tftp_port.parse().unwrap_or(69),
                    root_dir: PathBuf::from(&self.tftp_root_dir),
                    read_only: self.tftp_read_only,
                };
                self.runtime.spawn(async move {
                    let _ = tftp::start_server(config, state, shutdown_rx).await;
                });
            }
            ServerType::Ssh => {
                let config = SshConfig {
                    port: self.ssh_port.parse().unwrap_or(2222),
                    root_dir: PathBuf::from(&self.ssh_root_dir),
                    username: self.ssh_username.clone(),
                    password: self.ssh_password.clone(),
                };
                self.runtime.spawn(async move {
                    let _ = ssh::start_server(config, state, shutdown_rx).await;
                });
            }
        }
    }

    fn stop_server(&mut self, idx: usize) {
        let entry = &mut self.servers[idx];
        if let Some(tx) = entry.shutdown_tx.take() {
            let _ = tx.try_send(());
        }
        {
            let mut s = entry.state.write();
            s.status = ServerStatus::Stopping;
        }
    }

    fn save_config(&mut self) {
        self.config.http = HttpConfig {
            port: self.http_port.parse().unwrap_or(7777),
            root_dir: PathBuf::from(&self.http_root_dir),
            allow_directory_listing: self.http_allow_listing,
            auto_stop_seconds: if self.http_auto_stop {
                self.http_auto_stop_secs.parse().ok()
            } else {
                None
            },
        };
        self.config.ftp = FtpConfig {
            port: self.ftp_port.parse().unwrap_or(2121),
            root_dir: PathBuf::from(&self.ftp_root_dir),
            username: self.ftp_username.clone(),
            password: self.ftp_password.clone(),
            anonymous_access: self.ftp_anonymous,
            passive_mode: self.ftp_passive_mode,
            passive_ports: (
                self.ftp_passive_ports_start.parse().unwrap_or(50000),
                self.ftp_passive_ports_end.parse().unwrap_or(50100),
            ),
        };
        self.config.tftp = TftpConfig {
            port: self.tftp_port.parse().unwrap_or(69),
            root_dir: PathBuf::from(&self.tftp_root_dir),
            read_only: self.tftp_read_only,
        };
        self.config.ssh = SshConfig {
            port: self.ssh_port.parse().unwrap_or(2222),
            root_dir: PathBuf::from(&self.ssh_root_dir),
            username: self.ssh_username.clone(),
            password: self.ssh_password.clone(),
        };
        if let Err(e) = self.config.save() {
            tracing::error!("Failed to save config: {}", e);
        }
    }
}

impl eframe::App for OServersApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Request continuous updates for real-time log display
        ctx.request_repaint();

        egui::SidePanel::left("server_list")
            .resizable(true)
            .min_width(200.0)
            .show(ctx, |ui| {
                ui.heading("🖥 Servers");
                ui.separator();

                for (idx, entry) in self.servers.iter().enumerate() {
                    let is_selected = self.selected_server == Some(idx);
                    let status = entry.status();
                    
                    // Use colored circles instead of emoji for reliable display
                    let status_color = match &status {
                        ServerStatus::Stopped => egui::Color32::GRAY,
                        ServerStatus::Starting => egui::Color32::YELLOW,
                        ServerStatus::Running => egui::Color32::GREEN,
                        ServerStatus::Stopping => egui::Color32::from_rgb(255, 165, 0), // Orange
                        ServerStatus::Error(_) => egui::Color32::RED,
                    };

                    ui.horizontal(|ui| {
                        // Draw a colored circle as status indicator
                        let (rect, _response) = ui.allocate_exact_size(
                            egui::vec2(16.0, 16.0),
                            egui::Sense::hover()
                        );
                        let center = rect.center();
                        ui.painter().circle_filled(center, 6.0, status_color);
                        
                        if ui.selectable_label(is_selected, entry.server_type.name()).clicked() {
                            self.selected_server = Some(idx);
                        }
                    });
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(idx) = self.selected_server {
                // Extract all needed data from entry first to avoid borrow conflicts
                let server_type = self.servers[idx].server_type;
                let status = self.servers[idx].status();
                let is_running = self.servers[idx].is_running();
                let logs = self.servers[idx].logs();

                // Track button clicks
                let mut start_clicked = false;
                let mut stop_clicked = false;

                ui.horizontal(|ui| {
                    ui.heading(format!("{} Settings", server_type.name()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if is_running {
                            if ui.button("⏹ Stop").clicked() {
                                stop_clicked = true;
                            }
                        } else {
                            if ui.button("▶ Start").clicked() {
                                start_clicked = true;
                            }
                        }
                    });
                });

                // Handle button clicks after the closure
                if stop_clicked {
                    self.stop_server(idx);
                }
                if start_clicked {
                    self.start_server(idx);
                }

                ui.separator();

                // Status display
                let status_text = match &status {
                    ServerStatus::Stopped => "Stopped".to_string(),
                    ServerStatus::Starting => "Starting...".to_string(),
                    ServerStatus::Running => "Running".to_string(),
                    ServerStatus::Stopping => "Stopping...".to_string(),
                    ServerStatus::Error(e) => format!("Error: {}", e),
                };
                ui.horizontal(|ui| {
                    ui.label("Status:");
                    ui.label(&status_text);
                });

                ui.separator();

                // Server-specific settings
                egui::ScrollArea::vertical()
                    .id_salt(format!("settings_scroll_{}", idx))
                    .max_height(200.0)
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!is_running, |ui| {
                        
                        match server_type {
                            ServerType::Http => {
                                egui::Grid::new("http_settings")
                                    .num_columns(2)
                                    .spacing([10.0, 8.0])
                                    .show(ui, |ui| {
                                        ui.label("Root directory:");
                                        ui.horizontal(|ui| {
                                            ui.text_edit_singleline(&mut self.http_root_dir);
                                            if ui.button("📁").clicked() {
                                                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                                    self.http_root_dir = path.display().to_string();
                                                }
                                            }
                                        });
                                        ui.end_row();

                                        ui.label("Listening port:");
                                        ui.text_edit_singleline(&mut self.http_port);
                                        ui.end_row();

                                        ui.label("Directory listing:");
                                        ui.checkbox(&mut self.http_allow_listing, "Allow users to list directory content");
                                        ui.end_row();

                                        ui.label("Auto stop:");
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut self.http_auto_stop, "Stop server after");
                                            ui.add_enabled(
                                                self.http_auto_stop,
                                                egui::TextEdit::singleline(&mut self.http_auto_stop_secs).desired_width(50.0)
                                            );
                                            ui.label("seconds");
                                        });
                                        ui.end_row();
                                    });
                            }
                            ServerType::Ftp => {
                                egui::Grid::new("ftp_settings")
                                    .num_columns(2)
                                    .spacing([10.0, 8.0])
                                    .show(ui, |ui| {
                                        ui.label("Root directory:");
                                        ui.horizontal(|ui| {
                                            ui.text_edit_singleline(&mut self.ftp_root_dir);
                                            if ui.button("📁").clicked() {
                                                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                                    self.ftp_root_dir = path.display().to_string();
                                                }
                                            }
                                        });
                                        ui.end_row();

                                        ui.label("Listening port:");
                                        ui.text_edit_singleline(&mut self.ftp_port);
                                        ui.end_row();

                                        ui.label("Username:");
                                        ui.text_edit_singleline(&mut self.ftp_username);
                                        ui.end_row();

                                        ui.label("Password:");
                                        ui.add(egui::TextEdit::singleline(&mut self.ftp_password).password(true));
                                        ui.end_row();

                                        ui.label("Anonymous:");
                                        ui.checkbox(&mut self.ftp_anonymous, "Allow anonymous access");
                                        ui.end_row();

                                        ui.label("Transfer mode:");
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut self.ftp_passive_mode, "Passive mode (recommended)");
                                        });
                                        ui.end_row();

                                        ui.label("Passive ports:");
                                        ui.horizontal(|ui| {
                                            ui.text_edit_singleline(&mut self.ftp_passive_ports_start);
                                            ui.label("-");
                                            ui.text_edit_singleline(&mut self.ftp_passive_ports_end);
                                        });
                                        ui.end_row();
                                    });
                            }
                            ServerType::Tftp => {
                                egui::Grid::new("tftp_settings")
                                    .num_columns(2)
                                    .spacing([10.0, 8.0])
                                    .show(ui, |ui| {
                                        ui.label("Root directory:");
                                        ui.horizontal(|ui| {
                                            ui.text_edit_singleline(&mut self.tftp_root_dir);
                                            if ui.button("📁").clicked() {
                                                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                                    self.tftp_root_dir = path.display().to_string();
                                                }
                                            }
                                        });
                                        ui.end_row();

                                        ui.label("Listening port:");
                                        ui.text_edit_singleline(&mut self.tftp_port);
                                        ui.end_row();

                                        ui.label("Mode:");
                                        ui.checkbox(&mut self.tftp_read_only, "Read-only mode");
                                        ui.end_row();
                                    });
                            }
                            ServerType::Ssh => {
                                egui::Grid::new("ssh_settings")
                                    .num_columns(2)
                                    .spacing([10.0, 8.0])
                                    .show(ui, |ui| {
                                        ui.label("Root directory:");
                                        ui.horizontal(|ui| {
                                            ui.text_edit_singleline(&mut self.ssh_root_dir);
                                            if ui.button("📁").clicked() {
                                                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                                    self.ssh_root_dir = path.display().to_string();
                                                }
                                            }
                                        });
                                        ui.end_row();

                                        ui.label("Listening port:");
                                        ui.text_edit_singleline(&mut self.ssh_port);
                                        ui.end_row();

                                        ui.label("Username:");
                                        ui.text_edit_singleline(&mut self.ssh_username);
                                        ui.end_row();

                                        ui.label("Password:");
                                        ui.add(egui::TextEdit::singleline(&mut self.ssh_password).password(true));
                                        ui.end_row();
                                    });
                            }
                        }
                        });
                    });

                ui.separator();

                // Server output log
                ui.heading("Server output");
                egui::ScrollArea::vertical()
                    .id_salt(format!("logs_scroll_{}", idx))
                    .auto_shrink([false; 2])
                    .max_height(300.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for log in &logs {
                            let timestamp = log.timestamp.format("[%H:%M:%S%.3f]").to_string();
                            let color = match log.level {
                                LogLevel::Info => egui::Color32::LIGHT_GREEN,
                                LogLevel::Warning => egui::Color32::YELLOW,
                                LogLevel::Error => egui::Color32::LIGHT_RED,
                            };
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&timestamp).color(egui::Color32::GRAY));
                                ui.label(egui::RichText::new(&log.message).color(color));
                            });
                        }
                    });
            }
        });

        // Save config on close
        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_config();
        }
    }
}
