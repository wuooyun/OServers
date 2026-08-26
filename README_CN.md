# OServers - 多协议服务器集成管理工具

[![CI](https://github.com/wuooyun/OServers/actions/workflows/ci.yml/badge.svg)](https://github.com/wuooyun/OServers/actions/workflows/ci.yml)
[![Release](https://github.com/wuooyun/OServers/actions/workflows/release.yml/badge.svg)](https://github.com/wuooyun/OServers/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Language: Rust](https://img.shields.io/badge/Language-Rust%202024-orange.svg)](https://www.rust-lang.org/)

[English](./README.md) | [简体中文](./README_CN.md) | [架构设计文档](./PROJECT.md)

**OServers** 是一款基于 Rust 编写的高性能、轻量级、跨平台多协议服务器集成管理工具。界面体验类似于 MobaXterm 的内置服务器工具箱（Servers feature）及 SolarWinds 等网络工程师常用工具，提供开箱即用、一键启停的统一原生图形界面（GUI）。

---

## 🌟 核心特性

- ⚡ **原生高性能 & 低资源占用**：基于 Rust 与 Tokio 异步运行时构建，体积小巧、内存占用极低，无外部运行库依赖。
- 🖥️ **现代化统一 GUI**：基于 [egui](https://github.com/emilk/egui) / [eframe](https://github.com/emilk/egui/tree/master/crates/eframe) 打造，支持暗黑/明亮主题，内置中文字体回退渲染。通过 `wgpu + dx12` 驱动支持 **Windows 远程桌面 (RDP)** 与各类虚拟机无损流畅渲染，杜绝传统 OpenGL 1.1 崩溃。
- 🔄 **全独立服务生命周期**：每个服务器独立启停、互不干扰，支持图形化状态实时监测与最近 100 条滚动彩色日志流。
- 🛠️ **网络设备专属兼容优化**：SSH/SFTP 服务内置对传统网络设备（如 Cisco、华为等交换机和路由器）的算法兼容支持（Diffie-Hellman Group 1/14/GEX SHA1、AES-CBC 加密、HMAC-SHA1 校验以及 2048 位 RSA 密钥）。
- 💾 **自动配置持久化**：用户修改的端口、路径、认证凭据等配置将在应用退出时自动保存至系统标准配置目录，再次启动即恢复。

---

## 📦 支持的服务协议

| 服务协议 | 默认端口 | 核心功能与亮点 | 适用场景 |
| :--- | :--- | :--- | :--- |
| **HTTP Server** | `7777` | • 静态文件托管<br>• 精美 HTML 目录列表浏览（带文件类型图标/大小/修改时间）<br>• 请求方法、状态码与延迟日志记录<br>• 空闲超时自动停止（可选） | 局域网快速文件共享、Web 测试、脚本/固件下载 |
| **FTP Server** | `2121` | • 主动模式（PORT）与被动模式（PASV）<br>• 自定义被动端口范围（默认 50000-50100）<br>• 用户名/密码认证与匿名访问（Anonymous）开关 | 传统运维传输、工控设备数据交互、大文件备份传输 |
| **TFTP Server** | `69` | • 基于 UDP 的轻量级快速文件传输<br>• 支持只读（Read-Only）保护模式与读写模式 | 交换机/路由器系统固件升级、PXE 无盘网络引导、嵌入式开发 |
| **SSH / SFTP Server** | `2222` | • 完整的 SFTP 子系统（浏览、上传、下载、删除、重命名、属性查询）<br>• 自动生成并管理 Ed25519 和 2048 位 RSA 主机密钥<br>• 传统/遗留算法扩展支持（Legacy KEX/Ciphers/MACs） | 安全文件传输、自动化运维备份、旧款网络设备传输 |

---

## 🚀 快速上手

### 下载预编译版本

从 [Releases 页面](https://github.com/wuooyun/OServers/releases) 下载适合您操作系统的最新单文件可执行程序，无需安装即可直接运行。

### 源码编译安装

确保本地已安装 Rust 1.75+（推荐 1.85+ 及 Rust 2024 edition）：

```bash
# 1. 克隆代码仓库
git clone https://github.com/wuooyun/OServers.git
cd OServers

# 2. 编译发布版本（已开启 LTO 与二进制体积极致优化）
cargo build --release

# 3. 运行可执行文件
# Windows:
.\target\release\oservers.exe

# Linux / macOS:
./target/release/oservers
```

---

## 📖 使用指南

### 1. 主界面概览

1. **左侧服务列表**：
   - 绿色圆点表示服务正在运行（Running）。
   - 灰色圆点表示服务已停止（Stopped）。
   - 黄色/橙色表示启动中或停止中。
   - 红色表示启动异常或端口冲突报错。
2. **右侧服务配置与控制区**：
   - 顶部提供 **▶ Start（启动）** 与 **⏹ Stop（停止）** 按钮。
   - 状态显示栏实时反映服务运行状态。
   - 配置网格区支持配置根目录（带 📁 本地文件夹选择对话框）、监听端口及认证信息等。
3. **底部实时日志窗口**：
   - 记录客户端访问连接、请求详情、文件操作与报错信息。
   - 自动滚动至最新消息，按日志等级着色（绿色正常、黄色警告、红色错误）。

---

### 2. 各服务配置说明

#### 🌐 HTTP 服务器
- **Root directory**：要共享的本地文件夹绝对路径。
- **Listening port**：HTTP 监听端口（默认 `7777`）。
- **Directory listing**：勾选后，当访问无 `index.html` 的目录时，自动生成美观的文件列表网页。
- **Auto stop**：空闲指定秒数后自动停止服务（防止忘记关闭服务带来的安全暴露）。

#### 📁 FTP 服务器
- **Root directory**：FTP 共享根目录。
- **Listening port**：控制连接端口（默认 `2121`）。
- **Username / Password**：用于认证登录的用户名和密码。
- **Anonymous**：是否允许匿名用户（账号 `anonymous`）登录。
- **Transfer mode & Passive ports**：支持被动模式（PASV），可指定数据连接端口范围（默认 `50000-50100`），方便配置防火墙放行规则。

#### ⚡ TFTP 服务器
- **Root directory**：TFTP 存储根目录。
- **Listening port**：TFTP UDP 端口（默认 `69`，注意在 Linux/macOS 下 1024 以下端口通常需要 root 权限）。
- **Read-only mode**：开启只读模式可防止客户端意外覆盖或写入文件。

#### 🔒 SSH / SFTP 服务器
- **Root directory**：SFTP 文件访问根目录（自动做路径越界检查，禁止跳出根目录）。
- **Listening port**：SSH 监听端口（默认 `2222`）。
- **Username / Password**：SSH 登录认证账号与密码。
- **网络设备互通**：支持主流现代客户端（如 WinSCP、FileZilla、OpenSSH），同时兼容老旧网络设备（Cisco 2900 / 3750 等）的 SSH 客户端。

---

## ⚙️ 配置文件位置

OServers 将配置保存为标准 JSON 格式：

- **Windows**: `%APPDATA%\oservers\config\config.json`
- **Linux**: `~/.config/oservers/config.json`
- **macOS**: `~/Library/Application Support/com.oservers.oservers/config.json`

SSH 主机密钥文件（`ssh_key_ed25519.pem`、`ssh_key_rsa.pem`）也将保存在上述相同目录中。

---

## 🛠️ 本地开发与测试

### 系统依赖（Linux）
在 Linux 上进行开发编译前，需安装 GTK 与 X11/Wayland 相关依赖：
```bash
sudo apt-get update
sudo apt-get install -y libgtk-3-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev
```

### 开发命令
```bash
# 启动调试运行
cargo run

# 开启详细 Debug 日志运行
RUST_LOG=debug cargo run

# 运行自动化测试套件（含 FTP/SFTP 自动化通信测试）
cargo test

# 代码格式化与 Lint 检查
cargo fmt --check
cargo clippy
```

---

## 🗺️ 架构与设计

关于项目内部的多线程模型、Tokio 异步运行时设计、状态流转及如何扩展新协议，请参阅：[架构设计文档 (PROJECT.md)](./PROJECT.md)。

---

## 📄 开源许可证

本项目基于 [MIT 许可证](LICENSE) 开源。
