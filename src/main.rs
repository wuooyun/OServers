//! OServers - Multi-server management application
//! 
//! A Rust implementation of server management similar to MobaXterm's servers feature.
//! Supports HTTP, FTP, TFTP, and SSH servers with a unified GUI interface.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod gui;
mod servers;

use gui::app::OServersApp;

fn main() -> eframe::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("Starting OServers application");

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([600.0, 400.0])
            .with_icon(load_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "OServers - Server Management",
        native_options,
        Box::new(|cc| Ok(Box::new(OServersApp::new(cc)))),
    )
}

fn load_icon() -> egui::IconData {
    // Simple default icon - could be replaced with a custom icon
    let size = 32;
    let mut pixels = vec![0u8; size * size * 4];
    
    // Create a simple blue circle icon
    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;
            let dx = x as f32 - size as f32 / 2.0;
            let dy = y as f32 - size as f32 / 2.0;
            let dist = (dx * dx + dy * dy).sqrt();
            
            if dist < size as f32 / 2.0 - 2.0 {
                // Inside circle - blue color
                pixels[idx] = 66;     // R
                pixels[idx + 1] = 133; // G
                pixels[idx + 2] = 244; // B
                pixels[idx + 3] = 255; // A
            } else if dist < size as f32 / 2.0 {
                // Border - slightly darker
                pixels[idx] = 50;
                pixels[idx + 1] = 100;
                pixels[idx + 2] = 200;
                pixels[idx + 3] = 255;
            }
        }
    }
    
    egui::IconData {
        rgba: pixels,
        width: size as u32,
        height: size as u32,
    }
}
