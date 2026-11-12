#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod config;
mod download;
mod library;
mod queue;
mod theme;
mod ui;
mod visualizer;

use std::path::PathBuf;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut autoplay = false;
    let mut open_downloads = false;
    let mut download_url: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--play" => autoplay = true,
            "--downloads" => open_downloads = true,
            "--download" => {
                if let Some(url) = args.next() {
                    download_url = Some(url);
                    open_downloads = true;
                }
            }
            "--help" | "-h" => {
                println!("Def Player");
                println!("Usage: def_player [options] [file or folder ...]");
                println!();
                println!("  --play             start playing the first track after loading");
                println!("  --downloads        open the download page at startup");
                println!(
                    "  --download <url>   open the download page and start downloading a link"
                );
                println!("  --help             show this help");
                return Ok(());
            }
            other if !other.starts_with("--") => paths.push(PathBuf::from(other)),
            _ => {}
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("Def Player"),
        ..Default::default()
    };

    eframe::run_native(
        "Def Player",
        options,
        Box::new(move |cc| {
            Ok(Box::new(app::PlayerApp::with_options(
                cc,
                paths,
                autoplay,
                open_downloads,
                download_url,
            )))
        }),
    )
}
