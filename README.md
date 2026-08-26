# OServers

[![CI](https://github.com/wuooyun/OServers/actions/workflows/ci.yml/badge.svg)](https://github.com/wuooyun/OServers/actions/workflows/ci.yml)
[![Release](https://github.com/wuooyun/OServers/actions/workflows/release.yml/badge.svg)](https://github.com/wuooyun/OServers/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Language: Rust](https://img.shields.io/badge/Language-Rust%202024-orange.svg)](https://www.rust-lang.org/)

[English](./README.md) | [简体中文](./README_CN.md) | [Architecture & Design Docs](./PROJECT.md)

**OServers** is a high-performance, lightweight, cross-platform multi-protocol server management desktop application built in pure Rust. It delivers a unified, native GUI experience similar to MobaXterm's built-in servers feature or SolarWinds network toolsets, providing one-click launch for all essential network servers.

---

## ✨ Features

- ⚡ **High Performance & Low Footprint**: Built with Rust and Tokio async I/O. Extremely lightweight with minimal memory usage and zero runtime dependencies.
- 🖥️ **Modern Unified GUI**: Powered by [egui](https://github.com/emilk/egui) / [eframe](https://github.com/emilk/egui/tree/master/crates/eframe), offering dark/light themes, automatic CJK font fallback, and smooth native rendering. Fully compatible with **Windows Remote Desktop (RDP)** and virtual machines via `wgpu + dx12` WARP software rasterization, avoiding OpenGL 1.1 crashes.
- 🔄 **Independent Server Lifecycle**: Start, stop, and configure each server independently. Features real-time status indicators (Running/Stopped/Error) and a 100-message color-coded rolling log console.
- 🛠️ **Legacy & Network Appliance Compatibility**: The SSH/SFTP server includes compatibility configurations for enterprise network equipment (e.g. Cisco/Huawei switches & routers) supporting Diffie-Hellman Group 1/14/GEX SHA1, AES-CBC ciphers, HMAC-SHA1 MACs, and 2048-bit RSA keys alongside modern Ed25519 keys.
- 💾 **Automatic Persistent Configuration**: All port numbers, directories, and credentials automatically persist across restarts in standard operating system configuration paths.

---

## 📦 Supported Server Protocols

| Server Protocol | Default Port | Key Capabilities & Highlights | Use Cases |
| :--- | :--- | :--- | :--- |
| **HTTP Server** | `7777` | • Static file serving<br>• Beautiful responsive HTML directory listing (icons, sizes, timestamps)<br>• Live HTTP request logging (method, path, status, latency)<br>• Optional inactivity auto-stop timer | Local LAN file sharing, web development testing, firmware/script distribution |
| **FTP Server** | `2121` | • Active (PORT) and Passive (PASV) data transfer modes<br>• Customizable passive port range (default `50000-50100`)<br>• Username/password authentication & anonymous login toggle | Network device backups, legacy system data exchange, fast file transfer |
| **TFTP Server** | `69` | • High-performance UDP-based file transfer engine<br>• Configurable Read-Only safety mode and Read-Write mode | Switch/router firmware updates, PXE netboot, embedded development |
| **SSH / SFTP Server** | `2222` | • Full SFTP subsystem (list, upload, download, delete, rename, file attributes)<br>• Auto-generated Ed25519 and 2048-bit RSA host keys<br>• Extended legacy cryptography suite for older network hardware | Secure remote file transfer, automated sysadmin pipelines, network switch backups |

---

## 🚀 Quick Start

### Pre-built Binaries

Download the latest release executable for Windows, Linux, or macOS from the [Releases](https://github.com/wuooyun/OServers/releases) page. No installation required.

### Build from Source

Make sure you have Rust 1.75+ (or 1.85+ with Rust 2024 edition) installed:

```bash
# 1. Clone the repository
git clone https://github.com/wuooyun/OServers.git
cd OServers

# 2. Build in release mode (LTO and size optimizations enabled)
cargo build --release

# 3. Run the executable
# On Windows:
.\target\release\oservers.exe

# On Linux / macOS:
./target/release/oservers
```

---

## 📖 User Guide

### 1. User Interface Overview

<p align="center">
  <img width="794" height="623" alt="OServers Interface Demo" src="https://github.com/user-attachments/assets/358de501-1414-4042-996e-0306ab6731db" />
</p>

1. **Left Sidebar (Server List)**:
   - 🟢 Green dot: Server is actively running.
   - ⚪ Gray dot: Server is stopped.
   - 🟡 Orange/Yellow dot: Server is starting or stopping.
   - 🔴 Red dot: Port conflict or startup error.
2. **Right Main Area (Configuration & Controls)**:
   - Top action bar with **▶ Start** and **⏹ Stop** buttons.
   - Live status display.
   - Parameter form for root folder (with native file picker 📁), listening port, credentials, and protocol switches.
3. **Bottom Log Console**:
   - Live streaming server events, client connections, transfer operations, and errors.
   - Auto-scrolls to the newest output. Color-coded (Green for Info, Yellow for Warning, Red for Error).

---

### 2. Protocol Details

#### 🌐 HTTP Server
- **Root directory**: Absolute path to the shared local folder.
- **Listening port**: HTTP port (default `7777`).
- **Directory listing**: When enabled, serves a directory index with interactive file browser if `index.html` is not present.
- **Auto stop**: Automatically shuts down the server after the configured duration of inactivity.

#### 📁 FTP Server
- **Root directory**: Local root path for FTP clients.
- **Listening port**: Control port (default `2121`).
- **Username / Password**: Credentials for authenticated logins.
- **Anonymous**: Toggle anonymous login (`anonymous` username).
- **Transfer mode & Passive ports**: Configures PASV mode port range (default `50000-50100`) for easy firewall traversal.

#### ⚡ TFTP Server
- **Root directory**: TFTP storage root.
- **Listening port**: UDP port (default `69`, note that ports < 1024 on Linux/macOS may require elevated privileges).
- **Read-only mode**: Protects files from client modification or deletion.

#### 🔒 SSH / SFTP Server
- **Root directory**: Chrooted root folder for SFTP sessions (includes path traversal protection).
- **Listening port**: SSH/SFTP port (default `2222`).
- **Username / Password**: Authentication credentials.
- **Appliance Compatibility**: Connect seamlessly from WinSCP, FileZilla, OpenSSH, or legacy network equipment (e.g. Cisco 2900 series).

---

## ⚙️ Configuration File Locations

Configuration is stored automatically as JSON upon exit:

- **Windows**: `%APPDATA%\oservers\config\config.json`
- **Linux**: `~/.config/oservers/config.json`
- **macOS**: `~/Library/Application Support/com.oservers.oservers/config.json`

SSH Host keys (`ssh_key_ed25519.pem`, `ssh_key_rsa.pem`) are saved in the same directory.

---

## 🛠️ Development

### Linux Prerequisites
Install GTK and X11/Wayland dependencies before compiling on Linux:
```bash
sudo apt-get update
sudo apt-get install -y libgtk-3-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev
```

### Useful Cargo Commands
```bash
# Run in development mode
cargo run

# Run with detailed debug logs
RUST_LOG=debug cargo run

# Run test suite (includes automated FTP/SFTP protocol tests)
cargo test

# Check formatting and linter
cargo fmt --check
cargo clippy
```

---

## 🗺️ Architecture & Design

For in-depth architectural details, threading model, Tokio async integration, and how to add new server protocols, please read the [Architecture & Design Guide (PROJECT.md)](./PROJECT.md).

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
