# OServers 架构与开发设计文档 (Architecture & Design Guide)

本文档深入解析 **OServers** 的系统架构、设计理念、模块划分、数据流转机制及二次开发扩展指南。

---

## 1. 系统概述与设计理念

OServers 旨在为运维工程师、网络管理员及开发者提供类似 MobaXterm 的多协议便携服务套件。其核心设计目标包括：

1. **纯原生与零依赖**：利用 Rust 编译为单一静态独立二进制，无需安装 Python、Node.js 或 .NET 运行库。
2. **极速与低资源占用**：采用 Tokio 异步 I/O 驱动所有网络协议，空闲时内存占用仅数 MB。
3. **统一交互范式**：所有协议服务遵循统一的配置模型、生命周期管理接口及日志流规范。
4. **强健的硬件兼容性**：针对实际生产与网络调试场景（如老旧 Cisco/华为交换机通过 TFTP/SFTP 备份与升级配置），对协议握手、加密算法与端口分配做了深度兼容支持。

---

## 2. 总体系统架构

OServers 采用 **GUI 主线程 + Tokio 异步工作线程池** 的双层解耦架构：

```
┌─────────────────────────────────────────────────────────────┐
│                       OServers GUI                          │
│        (eframe / egui - 60 FPS 渲染 & 事件响应循环)           │
└──────────────┬───────────────────────────────▲──────────────┘
               │ 启动/停止指令 & 参数变更         │ 实时状态 & 日志轮询
               │ (mpsc::Sender<()>)            │ (Arc<RwLock<ServerState>>)
┌──────────────▼───────────────────────────────┴──────────────┐
│                    Tokio Async Runtime                      │
│                  (多线程异步任务调度池)                      │
├──────────────┬──────────────┬───────────────┬───────────────┤
│ HTTP Server  │  FTP Server  │  TFTP Server  │  SSH / SFTP   │
│ (Warp/Hyper) │ (libunftp)   │ (async-tftp)  │(russh & sftp) │
└──────────────┴──────────────┴───────────────┴───────────────┘
```

### 2.1 线程与并发模型
- **GUI 渲染主线程**：由 `eframe` 管理，负责组件布局、绘制、用户输入响应以及定时调用 `ctx.request_repaint()` 驱动日志与状态的平滑刷新。
- **Tokio 异步运行时 (`Arc<Runtime>`)**：由 `OServersApp` 实例持有。当用户点击“Start”时，GUI 主线程通过 `runtime.spawn(...)` 将对应的服务任务派发到异步工作池中。
- **线程安全与状态共享**：
  - 各服务状态统一封装为 `SharedState` (`Arc<RwLock<ServerState>>`)。
  - 主线程只读获取状态与日志，子服务在收到客户端连接或产生事件时写入日志。
  - 启停通过 `tokio::sync::mpsc::channel(1)` 发送关闭信号，实现优雅停机（Graceful Shutdown）。

---

## 3. 代码模块详细设计

```
src/
├── config.rs          # 应用全局配置加载与持久化
├── gui/
│   ├── mod.rs         # GUI 模块导出
│   └── app.rs         # egui 视图渲染、组件交互与中文字体初始化
├── main.rs            # 程序入口、日志订阅初始化与窗口参数配置
└── servers/
    ├── mod.rs         # 核心抽象类型 (ServerConfig, ServerState, LogMessage 等)
    ├── http.rs        # HTTP 静态文件服务与动态目录生成
    ├── ftp.rs         # FTP 服务 (主动/被动模式与认证支持)
    ├── tftp.rs        # TFTP 服务 (UDP 文件分块传输)
    └── ssh.rs         # SSH/SFTP 服务 (完整 SFTP v3 协议与旧算法兼容)
```

---

### 3.1 核心抽象 (`src/servers/mod.rs`)

定义了所有服务端通用的核心结构：

```rust
pub enum ServerStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error(String),
}

pub struct LogMessage {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub level: LogLevel,
    pub message: String,
}

pub struct ServerState {
    pub status: ServerStatus,
    pub logs: Vec<LogMessage>, // 自动维护最近 100 条日志
    pub config: ServerConfig,
}
```

每个服务启动函数均遵循统一的签名约定：
```rust
pub async fn start_server(
    config: SpecificConfig,
    state: SharedState,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> Result<(), ServerError>;
```

---

### 3.2 协议实现模块分析

#### 1. HTTP 服务 (`src/servers/http.rs`)
- **底层依赖**：`warp`
- **动态目录索引**：如果请求路径对应的是目录且没有 `index.html`，自动解析 `std::fs::read_dir` 并动态生成带图标、文件大小（B/KB/MB/GB 自动换算）、修改时间的美观响应式 HTML 页面。
- **结构化日志拦截**：通过 `warp::log::custom` 捕获客户端 IP 地址（`remote_addr`）、HTTP Method、Path、Status Code 以及耗时（毫秒）并实时推送到 GUI 日志控制台。
- **自动休眠机制**：支持配置 `auto_stop_seconds`，配合 Tokio 定时器实现空闲自动停机。

#### 2. FTP 服务 (`src/servers/ftp.rs`)
- **底层依赖**：`libunftp` + `unftp-sbe-fs`
- **传输模式**：
  - **主动模式 (PORT)**：由服务端向客户端指定端口发起数据连接。
  - **被动模式 (PASV)**：由客户端连接服务端开放的被动端口，支持通过 `passive_ports` 自定义范围（如 50000-50100），极大简化防火墙放行策略。
- **认证与连接日志**：
  - 自定义 `SimpleAuthenticator` 持有 `SharedState`，精准捕获客户端认证生命周期：匿名用户登录、指定用户名密码登录成功、登录失败等事件，并实时投递到对应服务的日志流中供 GUI 实时展示。

#### 3. TFTP 服务 (`src/servers/tftp.rs`)
- **底层依赖**：`async-tftp`
- **工作机制**：运行在 UDP 协议之上（默认 69 端口），采用简化的 Stop-and-Wait 确认机制，针对路由器/交换机小体积固件升级优化。
- **只读保护**：通过 `TftpServerBuilder::with_dir_ro` 提供安全只读模式。

#### 4. SSH / SFTP 服务 (`src/servers/ssh.rs`)
- **底层依赖**：`russh` (0.61) + `russh-sftp` (2.3)
- **连接与认证事件监听**：
  - 在 `MySshServer::new_client` 中捕获新客户端的 IP 与端口连接事件。
  - 在 `SshSession::auth_password` 中记录登录成功与失败日志（附带对端 IP 信息）。
  - 在 `subsystem_request` 与 `channel_eof` 中记录 SFTP 子系统会话启动与会话通道关闭事件。
- **子系统实现**：实现了 `russh_sftp::server::Handler`，覆盖 SFTP v3 的全部操作：
  - `opendir` / `readdir`：目录遍历与虚拟句柄管理。
  - `open` / `read` / `write` / `close`：流式异步文件读写与 offset seek。
  - `stat` / `lstat` / `fstat`：文件元数据与权限查询。
  - `remove` / `mkdir` / `rmdir` / `rename`：完整的文件系统操作。
- **路径安全隔离**：`resolve_path` 函数严格执行沙箱路径越界检查，拦截包含 `..` 的恶意逃逸请求。
- **多密钥与网络设备算法兼容**：
  - 自动生成并持久化 256 位 **Ed25519** 现代密钥 与 2048 位 **RSA** 兼容密钥。
  - 主动启用 Diffie-Hellman Group 1 SHA1、Group 14 SHA1、Group Exchange SHA1、AES-CBC (128/192/256) 及 HMAC-SHA1，完美支持老旧 Cisco IOS 交换机直接进行 `copy flash: sftp:`。

---

### 3.3 图形界面与用户体验 (`src/gui/app.rs`)

1. **字体回退渲染 (CJK Font Fallback)**：
   - 启动时自动从 Windows 系统字体目录扫描微软雅黑（`msyh.ttc`）、宋体（`simsun.ttc`）、黑体（`simhei.ttf`）。
   - 将中文字体追加至 `egui::FontFamily::Proportional` 与 `Monospace` 的 Fallback 链末尾，既保留了默认 emoji 符号渲染，又解决了中文字符乱码问题。
2. **状态指示器**：
   - 使用 `ui.painter().circle_filled(...)` 绘制状态光点，比纯 Emoji 状态展示更稳定统一。
3. **原生文件选择器**：
   - 集成 `rfd::FileDialog`，点击目录旁的 📁 按钮即可呼出系统原生目录选取框。

---

### 3.4 GUI 渲染引擎与 Windows 远程桌面 (RDP) 兼容性方案 (Review & Resolution)

在 Windows 远程桌面（RDP）或无独立显卡的虚拟机环境中运行基于 GPU 加速的 GUI 应用时，经常会遇到应用启动即崩溃或提示图形适配器初始化失败的问题。

#### 1. 问题复盘 (Root Cause Analysis)
- `egui / eframe` 默认或早期配置常用 `glow` (OpenGL) 后端。
- 在 Windows RDP 会话中，系统默认仅提供由 GDI 模拟的 **OpenGL 1.1** 驱动（`opengl32.dll`），缺失现代 OpenGL（如 3.3+）以及核心扩展函数（`wglCreateContextAttribsARB` 等），导致尝试创建 OpenGL 上下文时直接 Panic / 崩溃退出。
- 此外，若 `wgpu` 后端未启用平台相关的原生 API 特性支持，也会因找不到兼容的硬件适配器而初始化失败。

#### 2. 解决方案与实现细节 (Implementation Details)
项目在 `Cargo.toml` 与 `src/main.rs` 中进行了针对性优化与适配：

1. **启用 Direct3D 12 后端特性**（`Cargo.toml`）：
   ```toml
   [dependencies]
   eframe = { version = "0.30", features = ["wgpu", "glow"] }
   wgpu = { version = "23.0.1", features = ["dx12"] }
   ```
2. **指定 WGPU 原生渲染器**（`src/main.rs`）：
   ```rust
   let native_options = eframe::NativeOptions {
       viewport: egui::ViewportBuilder::default()
           .with_inner_size([800.0, 600.0])
           .with_min_inner_size([600.0, 400.0])
           .with_icon(load_icon()),
       renderer: eframe::Renderer::Wgpu,
       ..Default::default()
   };
   ```
3. **兼容性机制原理**：
   - 在 Windows RDP 环境下，DirectX 12 可直接调用系统内置的 **Microsoft Basic Render Driver (WARP 软件光栅化驱动)**。
   - `wgpu` 借助 `dx12` feature 能在无需物理显卡或专用驱动的情况下平滑降级运行，彻底摆脱 Windows RDP 对 OpenGL 1.1 的历史遗留限制，保证在远程桌面、无头测试机及各类云服务器虚拟机上均能 100% 稳定启动并流畅渲染。

---

## 4. 扩展开发指南：如何新增一个服务协议

假设你想在 OServers 中新增一个 **WebDAV 服务** 或 **Syslog 日志服务**，只需按照以下步骤进行扩展：

### 第一步：在 `src/servers/` 下创建新模块
创建 `src/servers/webdav.rs`：
```rust
use super::{LogMessage, ServerConfig, ServerError, ServerHandle, ServerStatus, SharedState};
use tokio::sync::mpsc;
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebDavConfig {
    pub root_dir: PathBuf,
    pub port: u16,
}

pub async fn start_server(
    config: WebDavConfig,
    state: SharedState,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> Result<(), ServerError> {
    // 1. 更新状态为 Starting
    // 2. 绑定端口并监听连接
    // 3. 使用 tokio::select! 监听服务主循环与 shutdown_rx.recv()
    // 4. 退出时清理状态
    Ok(())
}
```

### 第二步：在 `src/servers/mod.rs` 导出模块
```rust
pub mod webdav;
```

### 第三步：更新 `src/config.rs`
在 `AppConfig` 结构体中添加 `pub webdav: WebDavConfig`。

### 第四步：在 `src/gui/app.rs` 中注册 UI
1. 在 `ServerType` 枚举中添加 `WebDav`，并在 `ServerType::ALL` 中注册。
2. 在 `OServersApp::start_server` 中添加新服务的分发逻辑。
3. 在中央配置面板中添加新协议的 UI 设置网格。

---

## 5. 测试与持续集成

项目配置了完整的 CI 流程 (`.github/workflows/ci.yml`)，覆盖：
- **`cargo check --all-features`**：语法与特性依赖检查。
- **`cargo fmt --all -- --check`**：代码风格规范校验。
- **`cargo clippy --all-features -- -D warnings`**：静态分析与潜在隐患扫描。
- **`cargo test --all-features`**：单元测试与端到端协议通信集成测试（如 `test_ftp_put`）。
