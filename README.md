# OServers

[![CI](https://github.com/wuooyun/OServers/actions/workflows/ci.yml/badge.svg)](https://github.com/wuooyun/OServers/actions/workflows/ci.yml)
[![Release](https://github.com/wuooyun/OServers/actions/workflows/release.yml/badge.svg)](https://github.com/wuooyun/OServers/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A multi-protocol server management application with a unified GUI interface, similar to MobaXterm's servers feature.

## ✨ Features

- **HTTP Server** - Simple static file server with directory listing
- **FTP Server** - Full-featured FTP server with user authentication
- **TFTP Server** - Lightweight TFTP server for network booting
- **SSH Server** - Basic SSH server implementation

All servers can be managed through a modern, native GUI built with [egui](https://github.com/emilk/egui).


## 📦 Installation

### Pre-built Binaries

Download the latest release from the [Releases](https://github.com/wuooyun/OServers/releases) page.

### Build from Source

```bash
# Clone the repository
git clone https://github.com/wuooyun/OServers.git
cd OServers

# Build in release mode
cargo build --release

# The binary will be at target/release/oservers(.exe)
```

## 🚀 Usage

Simply run the executable:

```bash
./oservers
```

The GUI will launch, allowing you to:
1. Configure each server's port and root directory
2. Start/stop servers individually
3. Monitor active connections
4. Demo
  <img width="794" height="623" alt="image" src="https://github.com/user-attachments/assets/358de501-1414-4042-996e-0306ab6731db" />

## 🛠️ Development

### Prerequisites

- Rust 1.75+ (stable)
- On Linux: `libgtk-3-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev`

### Build & Run

```bash
# Development build
cargo run

# Run with logging
RUST_LOG=debug cargo run

# Run tests
cargo test

# Check formatting & lints
cargo fmt --check
cargo clippy
```

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
