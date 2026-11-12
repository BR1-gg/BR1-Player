use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use eframe::egui;
use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Rect, Sense, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Ui, UiBuilder,
};

use crate::audio::AudioEngine;
use crate::config::{self, Config};
use crate::download;
use crate::library::{self, Track};
use crate::queue::{self, JobState};
use crate::ui::{self, Glyph};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Repeat {
    Off,
    All,
    One,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Setup,
    Library,
    Downloads,
}

pub struct PlayerApp {
    view: View,
    tracks: Vec<Track>,
    visible: Vec<usize>,
    active_playlist: Option<String>,
    playlist_tex: HashMap<String, Option<TextureHandle>>,
    current: Option<usize>,
    paused: bool,
    volume: f32,
    muted: bool,
    shuffle: bool,
    repeat: Repeat,
    audio: Option<AudioEngine>,
    thumbs: Vec<Option<TextureHandle>>,
    hero_tex: Option<(usize, TextureHandle)>,
    scan_rx: Option<Receiver<Vec<Track>>>,
    scanning: bool,
    status: Option<String>,
    status_error: bool,
    scroll_to_current: bool,
    autoplay: bool,
    download_url: String,
    library_root: PathBuf,
    pending_url: Option<String>,
    open_downloads_after_setup: bool,
    manager: queue::Manager,
    download_status: Option<String>,
    download_failed: bool,
}

impl PlayerApp {
    pub fn with_options(
        cc: &eframe::CreationContext<'_>,
        initial_paths: Vec<PathBuf>,
        autoplay: bool,
        open_downloads: bool,
        initial_url: Option<String>,
    ) -> Self {
        configure_style(&cc.egui_ctx);
        let (audio, status) = match AudioEngine::new() {
            Ok(engine) => (Some(engine), None),
            Err(err) => (None, Some(err)),
        };

        let saved = config::load();
        let configured = saved.is_some();
        let (library_root, quality, parallel) = match &saved {
            Some(saved) => (saved.library_root.clone(), saved.quality, saved.parallel),
            None => (
                config::default_library_root(),
                config::DEFAULT_QUALITY,
                config::DEFAULT_PARALLEL,
            ),
        };

        let manager = queue::Manager::new(queue::Settings {
            root: library_root.clone(),
            quality,
            parallel,
        });

        let mut app = Self {
            view: if !configured {
                View::Setup
            } else if open_downloads {
                View::Downloads
            } else {
                View::Library
            },
            tracks: Vec::new(),
            visible: Vec::new(),
            active_playlist: None,
            playlist_tex: HashMap::new(),
            current: None,
            paused: true,
            volume: 0.8,
            muted: false,
            shuffle: false,
            repeat: Repeat::Off,
            audio,
            thumbs: Vec::new(),
            hero_tex: None,
            scan_rx: None,
            scanning: false,
            status_error: status.is_some(),
            status,
            scroll_to_current: false,
            autoplay,
            download_url: String::new(),
            library_root,
            pending_url: initial_url,
            open_downloads_after_setup: open_downloads,
            manager,
            download_status: None,
            download_failed: false,
        };

        if configured {
            let mut scan_paths = vec![app.library_root.clone()];
            scan_paths.extend(initial_paths);
            app.start_scan(scan_paths);

            if let Some(url) = app.pending_url.take() {
                app.download_url = url;
                app.view = View::Downloads;
                app.start_download();
            }
        }

        app
    }

    /* ---------- playback state ---------- */

    fn track(&self) -> Option<&Track> {
        self.current.and_then(|index| self.tracks.get(index))
    }

    fn duration(&self) -> f32 {
        self.track().map(|track| track.duration).unwrap_or(0.0)
    }

    fn position(&self) -> f32 {
        self.audio.as_ref().map(|audio| audio.position()).unwrap_or(0.0)
    }

    fn is_playing(&self) -> bool {
        self.current.is_some() && !self.paused
    }

    fn play_index(&mut self, index: usize) {
        if index >= self.tracks.len() {
            return;
        }

        let path = self.tracks[index].path.clone();
        self.current = Some(index);
        self.hero_tex = None;
        self.scroll_to_current = true;

        let Some(audio) = &mut self.audio else {
            self.status = Some("No audio output device available.".to_string());
            return;
        };

        audio.set_volume(self.volume);
        audio.set_muted(self.muted);

        match audio.play(&path) {
            Ok(()) => {
                self.paused = false;
                self.status = None;
            }
            Err(err) => {
                self.paused = true;
                self.status = Some(err);
            }
        }
    }

    fn toggle_play(&mut self) {
        if self.current.is_none() {
            if let Some(index) = self.visible.first().copied() {
                self.play_index(index);
            }
            return;
        }

        if self.paused {
            let ended = self
                .audio
                .as_ref()
                .map(|audio| audio.finished())
                .unwrap_or(true);

            if ended {
                if let Some(index) = self.current {
                    self.play_index(index);
                }
            } else if let Some(audio) = &self.audio {
                audio.resume();
                self.paused = false;
            }
        } else if let Some(audio) = &self.audio {
            audio.pause();
            self.paused = true;
        }
    }

    fn advance(&mut self, direction: isize, automatic: bool) {
        let list = self.visible.clone();
        if list.is_empty() {
            return;
        }

        let Some(current) = self.current else {
            self.play_index(list[0]);
            return;
        };

        if automatic && self.repeat == Repeat::One {
            self.play_index(current);
            return;
        }

        if self.shuffle && direction > 0 {
            self.play_index(self.random_index());
            return;
        }

        let position = list.iter().position(|index| *index == current).unwrap_or(0) as isize;
        let mut next = position + direction;

        if next >= list.len() as isize {
            if self.repeat == Repeat::All || !automatic {
                next = 0;
            } else {
                self.paused = true;
                return;
            }
        }
        if next < 0 {
            next = if self.repeat == Repeat::All {
                list.len() as isize - 1
            } else {
                0
            };
        }

        self.play_index(list[next as usize]);
    }

    fn random_index(&self) -> usize {
        if self.visible.len() < 2 {
            return self.current.or_else(|| self.visible.first().copied()).unwrap_or(0);
        }

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.subsec_nanos() as usize)
            .unwrap_or(1);

        let mut pick = nanos % self.visible.len();
        if Some(self.visible[pick]) == self.current {
            pick = (pick + 1) % self.visible.len();
        }
        self.visible[pick]
    }

    fn previous(&mut self) {
        if self.position() > 3.0 {
            if let Some(audio) = &self.audio {
                audio.seek(0.0);
            }
            return;
        }
        self.advance(-1, false);
    }

    fn seek_to(&mut self, fraction: f32) {
        let duration = self.duration();
        if duration <= 0.0 {
            return;
        }
        if let Some(audio) = &self.audio {
            audio.seek(fraction * duration);
        }
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if self.volume > 0.0 {
            self.muted = false;
        }
        if let Some(audio) = &mut self.audio {
            audio.set_volume(self.volume);
            audio.set_muted(self.muted);
        }
    }

    fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        if let Some(audio) = &mut self.audio {
            audio.set_muted(self.muted);
        }
    }

    /* ---------- library ---------- */

    fn playlists(&self) -> Vec<(String, usize)> {
        let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for track in &self.tracks {
            if let Some(name) = &track.playlist {
                *counts.entry(name.clone()).or_insert(0) += 1;
            }
        }
        counts.into_iter().collect()
    }

    fn rebuild_visible(&mut self) {
        let filter = self.active_playlist.clone();
        self.visible = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| match &filter {
                Some(name) => track.playlist.as_deref() == Some(name.as_str()),
                None => true,
            })
            .map(|(index, _)| index)
            .collect();
    }

    fn playlist_texture(&self, name: &str) -> Option<&TextureHandle> {
        if let Some(Some(texture)) = self.playlist_tex.get(name) {
            return Some(texture);
        }
        let index = self
            .tracks
            .iter()
            .position(|track| track.playlist.as_deref() == Some(name))?;
        self.thumbs.get(index).and_then(|texture| texture.as_ref())
    }

    fn ensure_playlist_textures(&mut self, ctx: &egui::Context) {
        let names: Vec<String> = self.playlists().into_iter().map(|(name, _)| name).collect();

        for name in names {
            if self.playlist_tex.contains_key(&name) {
                continue;
            }
            let texture = self.load_playlist_cover(&name, ctx);
            self.playlist_tex.insert(name, texture);
        }
    }

    fn load_playlist_cover(&self, name: &str, ctx: &egui::Context) -> Option<TextureHandle> {
        let folder = self
            .tracks
            .iter()
            .find(|track| track.playlist.as_deref() == Some(name))?
            .folder
            .clone()?;

        let entries = std::fs::read_dir(&folder).ok()?;
        let path = entries.flatten().find_map(|entry| {
            let file = entry.file_name().to_string_lossy().to_lowercase();
            matches!(
                file.as_str(),
                "cover.jpg"
                    | "cover.jpeg"
                    | "cover.png"
                    | "folder.jpg"
                    | "folder.png"
                    | "playlist.jpg"
                    | "playlist.png"
            )
            .then(|| entry.path())
        })?;

        let bytes = std::fs::read(path).ok()?;
        let image = image::load_from_memory(&bytes)
            .ok()?
            .resize_to_fill(256, 256, image::imageops::FilterType::Triangle)
            .to_rgba8();
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        );

        Some(ctx.load_texture(
            format!("playlist-cover-{name}"),
            color,
            TextureOptions::LINEAR,
        ))
    }

    fn pick_files(&mut self) {
        let extensions = [
            "mp3", "m4a", "mp4", "aac", "flac", "wav", "ogg", "oga", "opus", "wma", "aiff", "aif",
            "weba",
        ];
        if let Some(files) = rfd::FileDialog::new()
            .add_filter("Audio", &extensions)
            .set_title("Add music files")
            .pick_files()
        {
            self.start_scan(files);
        }
    }

    fn pick_folder(&mut self) {
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Add a music folder")
            .pick_folder()
        {
            self.start_scan(vec![folder]);
        }
    }

    fn start_scan(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        self.scanning = true;
        let (sender, receiver) = channel();
        self.scan_rx = Some(receiver);
        let root = self.library_root.clone();

        std::thread::spawn(move || {
            let tracks = library::scan(&paths, Some(&root));
            let _ = sender.send(tracks);
        });
    }

    fn poll_scan(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.scan_rx else {
            return;
        };

        match receiver.try_recv() {
            Ok(tracks) => {
                let before = self.tracks.len();
                for track in tracks {
                    if !self.tracks.iter().any(|known| known.path == track.path) {
                        self.tracks.push(track);
                    }
                }
                self.thumbs.resize(self.tracks.len(), None);
                self.playlist_tex.clear();
                self.rebuild_visible();
                let added = self.tracks.len() - before;
                self.scanning = false;
                self.scan_rx = None;

                self.status_error = false;
                if added == 0 && before == 0 {
                    self.status = Some("No audio files found in that folder yet.".to_string());
                } else {
                    self.status = Some(format!(
                        "Added {added} track{}",
                        if added == 1 { "" } else { "s" }
                    ));
                }

                if self.autoplay && self.current.is_none() && !self.visible.is_empty() {
                    self.autoplay = false;
                    self.play_index(self.visible[0]);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(120));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.scanning = false;
                self.scan_rx = None;
            }
        }
    }

    /* ---------- downloads ---------- */

    fn start_download(&mut self) {
        let url = self.download_url.trim().to_string();
        if url.is_empty() {
            self.download_status = Some("Paste a YouTube or Spotify link first.".to_string());
            self.download_failed = true;
            return;
        }

        self.download_status = None;
        self.download_failed = false;
        self.manager.submit(url);
    }

    fn poll_download(&mut self, ctx: &egui::Context) {
        self.manager.poll();

        if self.manager.queued_or_running() > 0 {
            ctx.request_repaint_after(Duration::from_millis(120));
        }

        if self.manager.take_rescan() {
            self.start_scan(vec![self.library_root.clone()]);
        }
    }

    fn set_quality(&mut self, quality: u16) {
        if self.manager.settings.quality == quality {
            return;
        }
        self.manager.settings.quality = quality;
        self.save_config();
    }

    fn set_parallel(&mut self, parallel: usize) {
        if self.manager.settings.parallel == parallel {
            return;
        }
        self.manager.settings.parallel = parallel;
        self.save_config();
    }

    fn save_config(&self) {
        config::save(&Config {
            library_root: self.library_root.clone(),
            quality: self.manager.settings.quality,
            parallel: self.manager.settings.parallel,
        });
    }

    fn choose_library_folder(&mut self) {
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Choose your music folder")
            .pick_folder()
        {
            self.library_root = folder;
            self.manager.settings.root = self.library_root.clone();
            self.save_config();
            self.start_scan(vec![self.library_root.clone()]);
        }
    }

    fn finish_setup(&mut self) {
        let root = self.library_root.clone();

        if let Err(err) = std::fs::create_dir_all(&root) {
            self.download_status = Some(format!("Cannot create that folder: {err}"));
            self.download_failed = true;
            return;
        }

        self.save_config();
        self.download_status = None;
        self.download_failed = false;
        self.manager.settings.root = root.clone();

        self.start_scan(vec![root]);
        self.view = if self.open_downloads_after_setup {
            View::Downloads
        } else {
            View::Library
        };

        if let Some(url) = self.pending_url.take() {
            self.download_url = url;
            self.view = View::Downloads;
            self.start_download();
        }
    }

    fn setup_area(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;

        ui.label(
            egui::RichText::new("SETUP")
                .size(11.0)
                .color(ui::MUTED)
                .strong(),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Where should your music live?")
                .size(26.0)
                .color(ui::TEXT)
                .strong(),
        );
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "Pick the main folder Def Player uses for downloads and playlists. Everything you download is saved there, and your library is built from it.",
            )
            .size(13.0)
            .color(ui::MUTED),
        );
        ui.add_space(22.0);

        let width = ui.available_width().min(760.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, 236.0), Sense::hover());
        let painter = ui.painter().clone();
        painter.rect_filled(rect, CornerRadius::from(18.0), ui::PANEL);
        painter.rect_stroke(
            rect,
            CornerRadius::from(18.0),
            Stroke::new(1.0_f32, ui::LINE),
            StrokeKind::Inside,
        );

        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink(22.0))
                .layout(Layout::top_down(Align::Min)),
        );
        child.set_clip_rect(rect.shrink(22.0));
        child.spacing_mut().item_spacing.y = 0.0;

        child.label(
            egui::RichText::new("MAIN FOLDER")
                .size(10.5)
                .color(ui::MUTED)
                .strong(),
        );
        child.add_space(10.0);

        let (path_rect, _) =
            child.allocate_exact_size(vec2(child.available_width(), 44.0), Sense::hover());
        let path_painter = child.painter();
        path_painter.rect_filled(path_rect, CornerRadius::from(10.0), ui::PANEL_SOFT);
        path_painter.rect_stroke(
            path_rect,
            CornerRadius::from(10.0),
            Stroke::new(1.0_f32, ui::LINE),
            StrokeKind::Inside,
        );
        let path_font = FontId::monospace(12.5);
        let path_text = self.library_root.display().to_string();
        path_painter.text(
            pos2(path_rect.left() + 14.0, path_rect.center().y),
            Align2::LEFT_CENTER,
            ui::ellipsize(
                path_painter,
                &path_text,
                &path_font,
                ui::TEXT,
                path_rect.width() - 28.0,
            ),
            path_font,
            ui::TEXT,
        );

        child.add_space(12.0);
        child.horizontal(|ui| {
            if ui::pill_button_sized(ui, 150.0, "Choose folder", true).clicked() {
                if let Some(folder) = rfd::FileDialog::new()
                    .set_title("Choose your music folder")
                    .pick_folder()
                {
                    self.library_root = folder;
                }
            }
            if ui::pill_button_sized(ui, 140.0, "Use default", false).clicked() {
                self.library_root = config::default_library_root();
            }
        });

        child.add_space(18.0);
        child.label(
            egui::RichText::new(
                "Playlists, albums and channels automatically get their own subfolder here.",
            )
            .size(11.5)
            .color(ui::MUTED),
        );

        child.add_space(18.0);
        if ui::pill_button_sized(&mut child, 200.0, "Start using Def Player", true).clicked() {
            self.finish_setup();
        }

        if self.view == View::Setup {
            if let Some(status) = &self.download_status {
                ui.add_space(10.0);
                ui.label(egui::RichText::new(status).size(12.0).color(ui::DANGER));
            }
        }
    }

    fn download_area(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;

        ui.label(
            egui::RichText::new("DOWNLOADS")
                .size(11.0)
                .color(ui::MUTED)
                .strong(),
        );
        ui.add_space(5.0);
        ui.label(
            egui::RichText::new("Add music from a link")
                .size(24.0)
                .color(ui::TEXT)
                .strong(),
        );
        ui.add_space(3.0);
        ui.label(
            egui::RichText::new(
                "Paste a YouTube or Spotify playlist, album or track. Tracks download in parallel — each one has its own progress bar.",
            )
            .size(13.0)
            .color(ui::MUTED),
        );
        ui.add_space(16.0);

        let input = egui::TextEdit::singleline(&mut self.download_url)
            .hint_text("https://www.youtube.com/playlist?list=…    or    https://open.spotify.com/playlist/…")
            .desired_width(f32::INFINITY)
            .font(FontId::proportional(13.0))
            .text_color(ui::TEXT)
            .margin(egui::Margin::symmetric(12, 9))
            .vertical_align(Align::Center);
        let response = ui.add(input);
        if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            self.start_download();
        }
        ui.add_space(10.0);

        let active = self.manager.queued_or_running();
        ui.horizontal(|ui| {
            if ui::pill_button_sized(ui, 140.0, "Download", true).clicked() {
                self.start_download();
            }
            if active > 0 && ui::pill_button_sized(ui, 112.0, "Cancel all", false).clicked() {
                self.manager.cancel_all();
            }

            let source = download::detect(&self.download_url);
            let label = if self.download_url.trim().is_empty() {
                "Waiting for a link".to_string()
            } else {
                format!("Detected: {}", source.label())
            };
            ui.label(egui::RichText::new(label).size(12.5).color(ui::MUTED));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui::pill_button_sized(ui, 132.0, "Change folder", false).clicked() {
                    self.choose_library_folder();
                }
            });
        });

        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("Saving to  {}", self.library_root.display()))
                    .size(12.0)
                    .color(ui::MUTED),
            )
            .truncate(),
        );

        ui.add_space(12.0);
        self.download_options(ui);

        ui.add_space(10.0);
        if let Some(status) = &self.download_status {
            let color = if self.download_failed {
                ui::DANGER
            } else {
                ui::MUTED
            };
            ui.label(egui::RichText::new(status).size(12.0).color(color));
            ui.add_space(6.0);
        }

        let finished = self
            .manager
            .jobs()
            .iter()
            .filter(|job| !job.state.is_active())
            .count();

        let running = self.manager.running_count();
        let waiting = active.saturating_sub(running);

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("QUEUE")
                    .size(10.5)
                    .color(ui::MUTED)
                    .strong(),
            );
            ui.label(
                egui::RichText::new(format!("{waiting} waiting · {running} running"))
                    .size(11.5)
                    .color(ui::MUTED),
            );

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if finished > 0
                    && ui::chip_button(ui, "Clear finished", false, 112.0).clicked()
                {
                    self.manager.clear_finished();
                }
            });
        });
        ui.add_space(8.0);

        let log_height = if self.manager.log.is_empty() { 0.0 } else { 92.0 };
        let bottom = ui.cursor().min.y + ui.available_height();
        let gap = if log_height > 0.0 { 8.0 } else { 0.0 };
        let list_height = (bottom - log_height - gap - ui.cursor().min.y).max(150.0);

        self.job_list(ui, list_height);

        if log_height > 0.0 {
            ui.add_space(8.0);
            self.download_log(ui, log_height);
        }
    }

    fn download_options(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("QUALITY")
                    .size(10.5)
                    .color(ui::MUTED)
                    .strong(),
            );
            for quality in [128u16, 192, 256, 320] {
                let active = self.manager.settings.quality == quality;
                if ui::chip_button(ui, &format!("{quality} kbps"), active, 76.0).clicked() {
                    self.set_quality(quality);
                }
            }

            ui.add_space(16.0);
            ui.label(
                egui::RichText::new("AT ONCE")
                    .size(10.5)
                    .color(ui::MUTED)
                    .strong(),
            );
            for parallel in [1usize, 2, 3, 4, 6] {
                let active = self.manager.settings.parallel == parallel;
                if ui::chip_button(ui, &parallel.to_string(), active, 32.0).clicked() {
                    self.set_parallel(parallel);
                }
            }

            ui.add_space(12.0);
            ui.label(
                egui::RichText::new("downloads running in parallel")
                    .size(11.5)
                    .color(ui::MUTED),
            );
        });
    }

    fn job_list(&mut self, ui: &mut Ui, height: f32) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter().clone();
        painter.rect_filled(rect, CornerRadius::from(16.0), ui::PANEL);
        painter.rect_stroke(
            rect,
            CornerRadius::from(16.0),
            Stroke::new(1.0_f32, ui::LINE),
            StrokeKind::Inside,
        );

        let inner = rect.shrink(1.0);
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min)),
        );
        child.set_clip_rect(inner);
        child.spacing_mut().item_spacing.y = 0.0;

        if self.manager.jobs().is_empty() {
            child.painter().text(
                inner.center(),
                Align2::CENTER_CENTER,
                "Nothing queued yet — paste a link above.",
                FontId::proportional(12.5),
                ui::MUTED,
            );
            return;
        }

        let mut cancel_id = None;
        let time = ui.input(|input| input.time);
        let row_width = inner.width();

        {
            let jobs = self.manager.jobs();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .id_salt("job-list")
                .show(&mut child, |list| {
                    list.spacing_mut().item_spacing.y = 0.0;

                    for job in jobs {
                        let (row, _) = list
                            .allocate_exact_size(vec2(row_width, 58.0), Sense::hover());
                        let row = row.shrink2(vec2(12.0, 0.0));
                        let painter = list.painter();

                        let title_color = match job.state {
                            JobState::Failed => ui::DANGER,
                            JobState::Done | JobState::Cancelled => ui::MUTED,
                            _ => ui::TEXT,
                        };
                        let title_font = FontId::proportional(13.0);
                        let detail_font = FontId::proportional(11.5);
                        let text_width = (row.width() - 110.0).max(60.0);

                        painter.text(
                            pos2(row.left(), row.top() + 18.0),
                            Align2::LEFT_CENTER,
                            ui::ellipsize(painter, &job.title, &title_font, title_color, text_width),
                            title_font,
                            title_color,
                        );
                        painter.text(
                            pos2(row.left(), row.top() + 33.0),
                            Align2::LEFT_CENTER,
                            ui::ellipsize(painter, &job.detail, &detail_font, ui::MUTED, text_width),
                            detail_font,
                            ui::MUTED,
                        );

                        let state_text = match job.state {
                            JobState::Queued => "queued".to_string(),
                            JobState::Done => "done".to_string(),
                            JobState::Failed => "failed".to_string(),
                            JobState::Cancelled => "cancelled".to_string(),
                            JobState::Running => match job.progress {
                                Some(fraction) => format!("{:.0}%", fraction * 100.0),
                                None => "…".to_string(),
                            },
                        };
                        painter.text(
                            pos2(row.right() - 22.0, row.top() + 18.0),
                            Align2::RIGHT_CENTER,
                            state_text,
                            FontId::proportional(11.5),
                            if job.state == JobState::Failed {
                                ui::DANGER
                            } else {
                                ui::MUTED
                            },
                        );

                        let bar = Rect::from_min_max(
                            pos2(row.left(), row.bottom() - 11.0),
                            pos2(row.right() - 22.0, row.bottom() - 8.0),
                        );
                        let bar_color = match job.state {
                            JobState::Failed => ui::DANGER,
                            JobState::Cancelled => ui::MUTED,
                            JobState::Queued => Color32::from_rgba_unmultiplied(255, 255, 255, 60),
                            JobState::Done => Color32::from_rgba_unmultiplied(124, 92, 255, 120),
                            _ => ui::ACCENT,
                        };
                        let fraction = match job.state {
                            JobState::Done => Some(1.0),
                            _ => job.progress,
                        };
                        ui::progress_bar(painter, bar, fraction, bar_color, time);

                        if job.state.is_active() {
                            let cancel_rect = Rect::from_center_size(
                                pos2(row.right() - 6.0, row.top() + 18.0),
                                vec2(22.0, 22.0),
                            );
                            let response = list.interact(
                                cancel_rect,
                                Id::new(("cancel-job", job.id)),
                                Sense::click(),
                            );
                            let color = if response.hovered() {
                                ui::DANGER
                            } else {
                                ui::MUTED
                            };
                            let center = cancel_rect.center();
                            painter.line_segment(
                                [center - vec2(4.0, 4.0), center + vec2(4.0, 4.0)],
                                Stroke::new(1.4_f32, color),
                            );
                            painter.line_segment(
                                [center + vec2(4.0, -4.0), center - vec2(4.0, -4.0)],
                                Stroke::new(1.4_f32, color),
                            );
                            if response.clicked() {
                                cancel_id = Some(job.id);
                            }
                        }
                    }
                });
        }

        if let Some(id) = cancel_id {
            self.manager.cancel(id);
        }
    }

    fn download_log(&mut self, ui: &mut Ui, height: f32) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter().clone();
        painter.rect_filled(rect, CornerRadius::from(14.0), ui::PANEL);
        painter.rect_stroke(
            rect,
            CornerRadius::from(14.0),
            Stroke::new(1.0_f32, ui::LINE),
            StrokeKind::Inside,
        );

        let inner = rect.shrink(12.0);
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min)),
        );
        child.set_clip_rect(inner);
        child.spacing_mut().item_spacing.y = 0.0;

        let start = self.manager.log.len().saturating_sub(6);
        for line in &self.manager.log[start..] {
            let color = if line.to_lowercase().contains("error") {
                ui::DANGER
            } else {
                ui::MUTED
            };
            child.add(
                egui::Label::new(
                    egui::RichText::new(line)
                        .monospace()
                        .size(10.5)
                        .color(color),
                )
                .truncate(),
            );
        }
    }

    /* ---------- textures ---------- */

    fn ensure_textures(&mut self, ctx: &egui::Context) {
        if self.thumbs.len() != self.tracks.len() {
            self.thumbs.resize(self.tracks.len(), None);
        }

        for index in 0..self.tracks.len() {
            if self.thumbs[index].is_some() {
                continue;
            }
            let Some(asset) = &self.tracks[index].thumb else {
                continue;
            };
            let image =
                egui::ColorImage::from_rgba_unmultiplied([asset.width, asset.height], &asset.rgba);
            self.thumbs[index] = Some(ctx.load_texture(
                format!("thumb-{}", self.tracks[index].path.display()),
                image,
                TextureOptions::LINEAR,
            ));
        }

        let Some(current) = self.current else {
            self.hero_tex = None;
            return;
        };
        if self.hero_tex.as_ref().map(|(index, _)| *index) == Some(current) {
            return;
        }

        self.hero_tex = match &self.tracks[current].art {
            Some(bytes) => {
                let decoded = image::load_from_memory(bytes).ok().map(|image| {
                    image
                        .resize_to_fill(480, 480, image::imageops::FilterType::Triangle)
                        .to_rgba8()
                });
                decoded.map(|image| {
                    let color =
                        egui::ColorImage::from_rgba_unmultiplied(
                            [image.width() as usize, image.height() as usize],
                            image.as_raw(),
                        );
                    let texture = ctx.load_texture(
                        format!("hero-{}", self.tracks[current].path.display()),
                        color,
                        TextureOptions::LINEAR,
                    );
                    (current, texture)
                })
            }
            None => None,
        };
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }

        let (space, left, right, up, down, mute) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::Space),
                input.key_pressed(egui::Key::ArrowLeft),
                input.key_pressed(egui::Key::ArrowRight),
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.key_pressed(egui::Key::M),
            )
        });

        if space {
            self.toggle_play();
        }
        if mute {
            self.toggle_mute();
        }
        if left || right {
            let duration = self.duration();
            if duration > 0.0 {
                let step = if right { 5.0 } else { -5.0 };
                let target = (self.position() + step).clamp(0.0, duration);
                if let Some(audio) = &self.audio {
                    audio.seek(target);
                }
            }
        }
        if up || down {
            let step = if up { 0.05 } else { -0.05 };
            self.set_volume(self.volume + step);
        }
    }

    /* ---------- frame ---------- */

    fn tick(&mut self, ctx: &egui::Context) {
        if self.audio.is_some() && self.is_playing() {
            let finished = self.audio.as_ref().map(|a| a.finished()).unwrap_or(false);
            if finished {
                self.advance(1, true);
            }
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });

        if !dropped.is_empty() {
            self.start_scan(dropped);
        }

        let hovering = ctx.input(|input| !input.raw.hovered_files.is_empty());
        if hovering {
            let screen = ctx.screen_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                Id::new("drop-overlay"),
            ));

            painter.rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(8, 8, 12, 210));
            let card = Rect::from_center_size(screen.center(), vec2(440.0, 150.0));
            painter.rect_filled(card, CornerRadius::from(20.0), Color32::from_rgb(24, 21, 40));
            painter.rect_stroke(
                card,
                CornerRadius::from(20.0),
                Stroke::new(2.0_f32, ui::ACCENT),
                StrokeKind::Inside,
            );
            painter.text(
                card.center() - vec2(0.0, 16.0),
                Align2::CENTER_CENTER,
                "Drop to add to your library",
                FontId::proportional(17.0),
                ui::TEXT,
            );
            painter.text(
                card.center() + vec2(0.0, 14.0),
                Align2::CENTER_CENTER,
                "Audio files and folders are supported.",
                FontId::proportional(13.0),
                ui::MUTED,
            );
        }
    }

    /* ---------- sidebar ---------- */

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;

        let (brand_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
        let painter = ui.painter();
        let mark = Rect::from_min_size(brand_rect.min, vec2(22.0, 22.0));
        painter.rect_filled(mark, CornerRadius::from(7.0), ui::ACCENT);
        painter.text(
            pos2(mark.right() + 10.0, mark.center().y),
            Align2::LEFT_CENTER,
            "Def Player",
            FontId::proportional(15.0),
            ui::TEXT,
        );

        ui.add_space(24.0);

        if self.view == View::Setup {
            ui.label(
                egui::RichText::new("Choose the folder where your music should live. You can add and download music once this is done.")
                    .size(11.5)
                    .color(ui::MUTED),
            );
            return;
        }

        if ui::nav_item(
            ui,
            "Library",
            self.view == View::Library && self.active_playlist.is_none(),
        )
        .clicked()
        {
            self.view = View::Library;
            self.active_playlist = None;
            self.rebuild_visible();
        }
        ui.add_space(4.0);
        if ui::nav_item(ui, "Downloads", self.view == View::Downloads).clicked() {
            self.view = View::Downloads;
        }
        ui.add_space(18.0);

        if ui::pill_button(ui, "Add music", true).clicked() {
            self.pick_files();
        }
        ui.add_space(8.0);
        if ui::pill_button(ui, "Add folder", false).clicked() {
            self.pick_folder();
        }

        ui.add_space(20.0);

        let playlists = self.playlists();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("PLAYLISTS")
                    .size(10.5)
                    .color(ui::MUTED)
                    .strong(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{}", playlists.len()))
                        .size(10.5)
                        .color(ui::MUTED),
                );
            });
        });
        ui.add_space(8.0);

        let mut selection: Option<Option<String>> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("playlist-list")
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;

                let all_active = self.active_playlist.is_none();
                if ui::playlist_row(ui, "All music", self.tracks.len(), None, all_active).clicked()
                {
                    selection = Some(None);
                }

                for (name, count) in &playlists {
                    let active = self.active_playlist.as_deref() == Some(name.as_str());
                    let texture = self.playlist_texture(name);
                    if ui::playlist_row(ui, name, *count, texture, active).clicked() {
                        selection = Some(Some(name.clone()));
                    }
                }
            });

        if let Some(playlist) = selection {
            self.view = View::Library;
            self.active_playlist = playlist;
            self.rebuild_visible();
        }
    }

    /* ---------- main area ---------- */

    fn main_area(&mut self, ui: &mut Ui) {
        // Anchor the playlist to the panel's real bottom edge, measured before
        // any child widget can grow the layout.
        let content_bottom = ui.cursor().min.y + ui.available_height();

        ui.spacing_mut().item_spacing.y = 0.0;
        self.header(ui);
        if let Some(status) = self.status.clone() {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(status)
                    .size(12.0)
                    .color(if self.status_error { ui::DANGER } else { ui::MUTED }),
            );
        }
        ui.add_space(18.0);
        self.hero(ui);
        ui.add_space(18.0);
        let height = (content_bottom - ui.cursor().min.y).max(140.0);
        self.playlist(ui, height);
    }

    fn header(&mut self, ui: &mut Ui) {
        let (eyebrow, title, sub, badge) = if let Some(name) = self.active_playlist.clone() {
            let count = self.visible.len();
            let duration: f32 = self
                .visible
                .iter()
                .map(|index| self.tracks[*index].duration)
                .sum();

            (
                "PLAYLIST",
                name,
                if count == 0 {
                    "No tracks in this playlist yet.".to_string()
                } else {
                    format!("{count} tracks · {}", format_duration(duration))
                },
                format!("{count} {}", if count == 1 { "track" } else { "tracks" }),
            )
        } else {
            let (title, sub) = match self.track() {
                Some(track) => (
                    track.title.clone(),
                    match (&track.artist, &track.album) {
                        (artist, album) if !album.is_empty() => format!("{artist} — {album}"),
                        (artist, _) => artist.clone(),
                    },
                ),
                None => (
                    "Nothing playing".to_string(),
                    "Pick a track from your library to start.".to_string(),
                ),
            };
            let mut sub = sub;
            if self.current.is_some() && self.paused {
                sub = format!("{sub}  ·  paused");
            }
            (
                "NOW PLAYING",
                title,
                sub,
                format!(
                    "{} {}",
                    self.tracks.len(),
                    if self.tracks.len() == 1 { "track" } else { "tracks" }
                ),
            )
        };

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(eyebrow)
                        .size(11.0)
                        .color(ui::MUTED)
                        .strong(),
                );
                ui.add_space(5.0);
                ui.label(
                    egui::RichText::new(title)
                        .size(24.0)
                        .color(ui::TEXT)
                        .strong(),
                );
                ui.add_space(3.0);
                ui.label(egui::RichText::new(sub).size(13.0).color(ui::MUTED));
            });

            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                let label = if self.scanning {
                    "Scanning…".to_string()
                } else {
                    badge
                };
                ui::badge(ui, &label);
            });
        });
    }

    fn hero(&mut self, ui: &mut Ui) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, 192.0), Sense::hover());
        let painter = ui.painter().clone();

        painter.rect_filled(rect, CornerRadius::from(18.0), Color32::from_rgb(20, 19, 30));

        let mut gradient = egui::Mesh::default();
        gradient.colored_vertex(rect.left_top(), Color32::from_rgba_unmultiplied(124, 92, 255, 46));
        gradient.colored_vertex(rect.right_top(), Color32::from_rgba_unmultiplied(124, 92, 255, 0));
        gradient.colored_vertex(rect.left_bottom(), Color32::from_rgba_unmultiplied(124, 92, 255, 12));
        gradient.colored_vertex(rect.right_bottom(), Color32::from_rgba_unmultiplied(124, 92, 255, 0));
        gradient.add_triangle(0, 1, 2);
        gradient.add_triangle(2, 1, 3);
        painter.add(egui::Shape::mesh(gradient));

        painter.rect_stroke(
            rect,
            CornerRadius::from(18.0),
            Stroke::new(1.0_f32, ui::LINE),
            StrokeKind::Inside,
        );

        let cover = Rect::from_min_size(rect.min + vec2(18.0, 18.0), vec2(156.0, 156.0));

        let active_playlist = self.active_playlist.clone();

        if let Some(name) = &active_playlist {
            let texture = self.playlist_texture(name);
            ui::paint_cover(ui, cover, texture, 14.0);
        } else {
            ui::paint_cover(
                ui,
                cover,
                self.hero_tex.as_ref().map(|(_, texture)| texture),
                14.0,
            );
        }

        let text_x = cover.right() + 22.0;
        let clip = Rect::from_min_max(
            pos2(text_x, rect.top()),
            pos2(rect.right() - 18.0, rect.bottom()),
        );
        let text_painter = painter.with_clip_rect(clip);

        let (label, heading, subtitle, hint) = if let Some(name) = &active_playlist {
            let count = self.visible.len();
            let duration: f32 = self
                .visible
                .iter()
                .map(|index| self.tracks[*index].duration)
                .sum();

            (
                "PLAYLIST",
                name.clone(),
                format!("{count} tracks · {}", format_duration(duration)),
                if self.playlist_texture(name).is_some() {
                    "Playlist cover loaded from its folder."
                } else {
                    "No playlist cover yet — using the first track's artwork."
                },
            )
        } else {
            match self.track() {
                Some(track) => (
                    "ALBUM",
                    if track.album.is_empty() {
                        track.title.clone()
                    } else {
                        track.album.clone()
                    },
                    track.artist.clone(),
                    if track.art.is_some() {
                        "Artwork loaded from this track's metadata."
                    } else {
                        "Artwork is not downloaded yet — showing the placeholder cover."
                    },
                ),
                None => (
                    "ALBUM",
                    "No cover available".to_string(),
                    "—".to_string(),
                    "Artwork is not downloaded yet — showing the placeholder cover.",
                ),
            }
        };

        text_painter.text(
            pos2(text_x, rect.top() + 52.0),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(11.0),
            ui::MUTED,
        );
        text_painter.text(
            pos2(text_x, rect.top() + 82.0),
            Align2::LEFT_CENTER,
            ui::ellipsize(&text_painter, &heading, &FontId::proportional(20.0), ui::TEXT, clip.width()),
            FontId::proportional(20.0),
            ui::TEXT,
        );
        text_painter.text(
            pos2(text_x, rect.top() + 110.0),
            Align2::LEFT_CENTER,
            ui::ellipsize(&text_painter, &subtitle, &FontId::proportional(13.0), ui::MUTED, clip.width()),
            FontId::proportional(13.0),
            ui::MUTED,
        );
        text_painter.text(
            pos2(text_x, rect.top() + 142.0),
            Align2::LEFT_CENTER,
            ui::ellipsize(&text_painter, hint, &FontId::proportional(11.5), Color32::from_rgb(110, 108, 130), clip.width()),
            FontId::proportional(11.5),
            Color32::from_rgb(110, 108, 130),
        );
    }

    fn playlist(&mut self, ui: &mut Ui, height: f32) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter().clone();

        painter.rect_filled(rect, CornerRadius::from(18.0), ui::PANEL);
        painter.rect_stroke(
            rect,
            CornerRadius::from(18.0),
            Stroke::new(1.0_f32, ui::LINE),
            StrokeKind::Inside,
        );

        let inner = rect.shrink(1.0);
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min)),
        );
        // Keep rows inside the card instead of painting over the player bar.
        child.set_clip_rect(inner);
        child.spacing_mut().item_spacing.y = 0.0;

        if self.tracks.is_empty() {
            self.empty_state(&mut child, inner);
            return;
        }

        self.playlist_header(&mut child);
        self.playlist_rows(&mut child);
    }

    fn empty_state(&self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter().clone();
        let center = rect.center();

        let (title, subtitle) = if let Some(name) = &self.active_playlist {
            (
                format!("“{name}” is empty"),
                "Download tracks into it, or add files from the sidebar.".to_string(),
            )
        } else {
            (
                "Your library is empty".to_string(),
                "Use “Add music” or “Add folder” in the sidebar, or drop files here.".to_string(),
            )
        };

        painter.text(
            center - vec2(0.0, 18.0),
            Align2::CENTER_CENTER,
            ui::ellipsize(&painter, &title, &FontId::proportional(16.0), ui::TEXT, rect.width() - 40.0),
            FontId::proportional(16.0),
            ui::TEXT,
        );
        painter.text(
            center + vec2(0.0, 8.0),
            Align2::CENTER_CENTER,
            subtitle,
            FontId::proportional(12.5),
            ui::MUTED,
        );
    }

    fn playlist_header(&self, ui: &mut Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        let painter = ui.painter();
        painter.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0_f32, ui::LINE));

        let font = FontId::proportional(10.5);
        let y = rect.center().y;
        painter.text(
            pos2(rect.left() + 24.0, y),
            Align2::LEFT_CENTER,
            "#",
            font.clone(),
            ui::MUTED,
        );
        painter.text(
            pos2(rect.left() + 96.0, y),
            Align2::LEFT_CENTER,
            "TITLE",
            font.clone(),
            ui::MUTED,
        );
        painter.text(
            pos2(rect.right() - 230.0, y),
            Align2::LEFT_CENTER,
            "ALBUM",
            font.clone(),
            ui::MUTED,
        );
        painter.text(
            pos2(rect.right() - 18.0, y),
            Align2::RIGHT_CENTER,
            "TIME",
            font,
            ui::MUTED,
        );
    }

    fn playlist_rows(&mut self, ui: &mut Ui) {
        let mut clicked = None;
        let visible = self.visible.clone();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("playlist")
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;

                for (position, index) in visible.iter().copied().enumerate() {
                    let track = &self.tracks[index];
                    let active = self.current == Some(index);

                    let (rect, response) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::click());
                    let row = rect.shrink2(vec2(8.0, 2.0));
                    let painter = ui.painter();

                    if active {
                        painter.rect_filled(row, CornerRadius::from(10.0), ui::ACCENT_SOFT);
                    } else if response.hovered() {
                        painter.rect_filled(
                            row,
                            CornerRadius::from(10.0),
                            Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                        );
                    }

                    let center_y = row.center().y;

                    painter.text(
                        pos2(row.left() + 16.0, center_y),
                        Align2::LEFT_CENTER,
                        format!("{}", position + 1),
                        FontId::proportional(12.0),
                        if active { ui::ACCENT } else { ui::MUTED },
                    );

                    let thumb = Rect::from_min_size(
                        pos2(row.left() + 40.0, center_y - 20.0),
                        vec2(40.0, 40.0),
                    );
                    ui::paint_cover(ui, thumb, self.thumbs[index].as_ref(), 8.0);

                    let title_x = row.left() + 92.0;
                    let album_x = row.right() - 230.0;
                    let time_right = row.right() - 16.0;

                    let title_clip = Rect::from_min_max(
                        pos2(title_x, row.top()),
                        pos2(album_x - 14.0, row.bottom()),
                    );
                    let title_painter = painter.with_clip_rect(title_clip);
                    let title_font = FontId::proportional(13.5);
                    let artist_font = FontId::proportional(12.0);
                    let title_color = if active { Color32::WHITE } else { ui::TEXT };

                    title_painter.text(
                        pos2(title_x, center_y - 9.0),
                        Align2::LEFT_CENTER,
                        ui::ellipsize(
                            &title_painter,
                            &track.title,
                            &title_font,
                            title_color,
                            title_clip.width(),
                        ),
                        title_font,
                        title_color,
                    );
                    title_painter.text(
                        pos2(title_x, center_y + 9.0),
                        Align2::LEFT_CENTER,
                        ui::ellipsize(
                            &title_painter,
                            &track.artist,
                            &artist_font,
                            ui::MUTED,
                            title_clip.width(),
                        ),
                        artist_font,
                        ui::MUTED,
                    );

                    let album_clip = Rect::from_min_max(
                        pos2(album_x, row.top()),
                        pos2(time_right - 60.0, row.bottom()),
                    );
                    let album_font = FontId::proportional(12.5);
                    let album_text = if track.album.is_empty() {
                        "—".to_string()
                    } else {
                        ui::ellipsize(
                            &painter,
                            &track.album,
                            &album_font,
                            ui::MUTED,
                            album_clip.width(),
                        )
                    };
                    painter.with_clip_rect(album_clip).text(
                        pos2(album_x, center_y),
                        Align2::LEFT_CENTER,
                        album_text,
                        album_font,
                        ui::MUTED,
                    );

                    painter.text(
                        pos2(time_right, center_y),
                        Align2::RIGHT_CENTER,
                        format_duration(track.duration),
                        FontId::proportional(12.5),
                        ui::MUTED,
                    );

                    if response.clicked() {
                        clicked = Some(index);
                    }

                    if active && self.scroll_to_current {
                        ui.scroll_to_rect(rect, Some(Align::Center));
                        self.scroll_to_current = false;
                    }
                }
            });

        if let Some(index) = clicked {
            if self.current == Some(index) {
                self.toggle_play();
            } else {
                self.play_index(index);
            }
        }
    }

    /* ---------- player bar ---------- */

    fn player_bar(&mut self, ui: &mut Ui) {
        let full = ui.max_rect();
        let painter = ui.painter().clone();
        painter.hline(full.x_range(), full.top(), Stroke::new(1.0_f32, ui::LINE));

        let inner = full.shrink2(vec2(22.0, 0.0));
        let left = Rect::from_min_max(inner.left_top(), pos2(inner.left() + 270.0, inner.bottom()));
        let right = Rect::from_min_max(pos2(inner.right() - 200.0, inner.top()), inner.right_bottom());
        let center = Rect::from_min_max(
            pos2(left.right() + 8.0, inner.top()),
            pos2(right.left() - 8.0, inner.bottom()),
        );

        self.now_playing(ui, left);
        self.transport(ui, center);
        self.volume_controls(ui, right);
    }

    fn now_playing(&self, ui: &mut Ui, area: Rect) {
        let cover = Rect::from_min_size(
            pos2(area.left(), area.center().y - 27.0),
            vec2(54.0, 54.0),
        );
        let thumb = self
            .current
            .and_then(|index| self.thumbs.get(index))
            .and_then(|texture| texture.as_ref());
        ui::paint_cover(ui, cover, thumb, 10.0);

        let (title, artist) = match self.track() {
            Some(track) => (track.title.clone(), track.artist.clone()),
            None => ("No track selected".to_string(), "—".to_string()),
        };

        let clip = Rect::from_min_max(
            pos2(cover.right() + 12.0, area.top()),
            pos2(area.right() - 4.0, area.bottom()),
        );
        let painter = ui.painter().with_clip_rect(clip);
        let max_width = clip.width();

        let title_font = FontId::proportional(13.0);
        let artist_font = FontId::proportional(12.0);

        painter.text(
            pos2(cover.right() + 12.0, area.center().y - 9.0),
            Align2::LEFT_CENTER,
            ui::ellipsize(&painter, &title, &title_font, ui::TEXT, max_width),
            title_font,
            ui::TEXT,
        );
        painter.text(
            pos2(cover.right() + 12.0, area.center().y + 10.0),
            Align2::LEFT_CENTER,
            ui::ellipsize(&painter, &artist, &artist_font, ui::MUTED, max_width),
            artist_font,
            ui::MUTED,
        );
    }

    fn transport(&mut self, ui: &mut Ui, area: Rect) {
        let controls_y = area.top() + 34.0;
        let center_x = area.center().x;
        let spread = (area.width() * 0.28).clamp(96.0, 186.0);

        let shuffle_rect =
            Rect::from_center_size(pos2(center_x - spread, controls_y), vec2(70.0, 26.0));
        if ui::chip(
            ui,
            shuffle_rect,
            "shuffle",
            "SHUFFLE",
            self.shuffle,
        )
        .clicked()
        {
            self.shuffle = !self.shuffle;
        }

        let repeat_label = match self.repeat {
            Repeat::Off => "REPEAT",
            Repeat::All => "REPEAT ALL",
            Repeat::One => "REPEAT 1",
        };
        let repeat_rect =
            Rect::from_center_size(pos2(center_x + spread, controls_y), vec2(82.0, 26.0));
        if ui::chip(ui, repeat_rect, "repeat", repeat_label, self.repeat != Repeat::Off).clicked()
        {
            self.repeat = match self.repeat {
                Repeat::Off => Repeat::All,
                Repeat::All => Repeat::One,
                Repeat::One => Repeat::Off,
            };
        }

        let prev_rect = Rect::from_center_size(pos2(center_x - 52.0, controls_y), vec2(36.0, 36.0));
        if ui::glyph_button(ui, prev_rect, "prev", Glyph::Prev, false).clicked() {
            self.previous();
        }

        let play_rect = Rect::from_center_size(pos2(center_x, controls_y), vec2(44.0, 44.0));
        let glyph = if self.is_playing() {
            Glyph::Pause
        } else {
            Glyph::Play
        };
        if ui::glyph_button(ui, play_rect, "play", glyph, true).clicked() {
            self.toggle_play();
        }

        let next_rect = Rect::from_center_size(pos2(center_x + 52.0, controls_y), vec2(36.0, 36.0));
        if ui::glyph_button(ui, next_rect, "next", Glyph::Next, false).clicked() {
            self.advance(1, false);
        }

        let seek_y = area.top() + 72.0;
        let duration = self.duration();
        let fraction = if duration > 0.0 {
            (self.position() / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let painter = ui.painter();
        painter.text(
            pos2(area.left() + 4.0, seek_y),
            Align2::LEFT_CENTER,
            format_duration(self.position()),
            FontId::proportional(11.0),
            ui::MUTED,
        );
        painter.text(
            pos2(area.right() - 4.0, seek_y),
            Align2::RIGHT_CENTER,
            format_duration(duration),
            FontId::proportional(11.0),
            ui::MUTED,
        );

        let slider_rect = Rect::from_min_max(
            pos2(area.left() + 46.0, seek_y - 9.0),
            pos2(area.right() - 46.0, seek_y + 9.0),
        );
        let (_, requested) = ui::slider(ui, slider_rect, "seek", fraction, duration > 0.0);
        if let Some(value) = requested {
            self.seek_to(value);
        }
    }

    fn volume_controls(&mut self, ui: &mut Ui, area: Rect) {
        let mute_rect = Rect::from_center_size(
            pos2(area.right() - 32.0, area.center().y),
            vec2(64.0, 26.0),
        );
        if ui::chip(
            ui,
            mute_rect,
            "mute",
            if self.muted { "MUTED" } else { "MUTE" },
            self.muted,
        )
        .clicked()
        {
            self.toggle_mute();
        }

        let slider_rect = Rect::from_min_max(
            pos2(area.left(), area.center().y - 9.0),
            pos2(mute_rect.left() - 14.0, area.center().y + 9.0),
        );
        let (_, requested) = ui::slider(
            ui,
            slider_rect,
            "volume",
            if self.muted { 0.0 } else { self.volume },
            true,
        );
        if let Some(value) = requested {
            self.set_volume(value);
        }
    }
}

impl eframe::App for PlayerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|input| input.viewport().close_requested()) {
            self.manager.kill_all_now();
        }
        self.poll_scan(ctx);
        self.poll_download(ctx);
        self.tick(ctx);
        self.handle_keys(ctx);
        self.ensure_textures(ctx);
        self.ensure_playlist_textures(ctx);
        self.handle_drops(ctx);

        egui::TopBottomPanel::bottom("player-bar")
            .exact_height(if self.view == View::Setup { 0.0 } else { 96.0 })
            .frame(egui::Frame::NONE.fill(ui::PANEL))
            .show(ctx, |ui| {
                if self.view != View::Setup {
                    self.player_bar(ui);
                }
            });

        egui::SidePanel::left("sidebar")
            .exact_width(264.0)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(ui::PANEL)
                    .inner_margin(egui::Margin::symmetric(18, 22)),
            )
            .show(ctx, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(ui::BG)
                    .inner_margin(egui::Margin::symmetric(30, 26)),
            )
            .show(ctx, |ui| match self.view {
                View::Setup => self.setup_area(ui),
                View::Library => self.main_area(ui),
                View::Downloads => self.download_area(ui),
            });
    }
}

fn format_duration(seconds: f32) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".to_string();
    }
    let total = seconds.round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let secs = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = ui::BG;
    visuals.window_fill = ui::PANEL;
    visuals.extreme_bg_color = ui::PANEL_SOFT;
    visuals.faint_bg_color = ui::PANEL_SOFT;
    visuals.override_text_color = Some(ui::TEXT);
    visuals.selection.bg_fill = ui::ACCENT;
    visuals.selection.stroke = Stroke::new(1.0_f32, ui::TEXT);
    visuals.hyperlink_color = ui::ACCENT;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, ui::LINE);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ui::ACCENT);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, ui::ACCENT);
    visuals.widgets.inactive.corner_radius = CornerRadius::from(10.0);
    visuals.widgets.hovered.corner_radius = CornerRadius::from(10.0);
    visuals.widgets.active.corner_radius = CornerRadius::from(10.0);
    style.visuals = visuals;
    style.spacing.item_spacing = vec2(8.0, 8.0);
    ctx.set_style(style);
}
