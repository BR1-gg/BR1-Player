use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Layout, Rect, Sense,
    Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, UiBuilder,
};

use crate::audio::{AudioEngine, EQ_FREQS};
use crate::config::{self, Config, GlassPreset, GlassShape, UiConfig, EQ_BANDS};
use crate::download;
use crate::glass::GlassSlot;
use crate::library::{self, Track};
use crate::queue::{self, JobState};
use crate::theme::{self, Theme};
use crate::ui::{self, Glyph};
use crate::visualizer::{self, SPECTRUM_BINS};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Repeat {
    Off,
    All,
    One,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Setup,
    Dashboard,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PanelKind {
    CurrentPlaylist,
    Player,
    Downloader,
    Playlists,
    Equalizer,
}

const PANEL_KINDS: [PanelKind; 5] = [
    PanelKind::CurrentPlaylist,
    PanelKind::Player,
    PanelKind::Downloader,
    PanelKind::Playlists,
    PanelKind::Equalizer,
];

impl PanelKind {
    fn index(self) -> usize {
        match self {
            PanelKind::CurrentPlaylist => 0,
            PanelKind::Player => 1,
            PanelKind::Downloader => 2,
            PanelKind::Playlists => 3,
            PanelKind::Equalizer => 4,
        }
    }

    fn key(self) -> &'static str {
        match self {
            PanelKind::CurrentPlaylist => "current_playlist",
            PanelKind::Player => "player",
            PanelKind::Downloader => "downloader",
            PanelKind::Playlists => "playlists",
            PanelKind::Equalizer => "equalizer",
        }
    }

    fn title(self) -> &'static str {
        match self {
            PanelKind::CurrentPlaylist => "Current playlist",
            PanelKind::Player => "Player",
            PanelKind::Downloader => "Downloader",
            PanelKind::Playlists => "Playlists",
            PanelKind::Equalizer => "Equalizer / visualizer",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Appearance,
    Glass,
    Layout,
    Library,
    Downloads,
    About,
}

impl SettingsTab {
    fn key(self) -> &'static str {
        match self {
            Self::Appearance => "appearance",
            Self::Glass => "glass",
            Self::Layout => "layout",
            Self::Library => "library",
            Self::Downloads => "downloads",
            Self::About => "about",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EqMode {
    Equalizer,
    Visualizer,
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
    status_at: Option<Instant>,
    scroll_to_current: bool,
    autoplay: bool,
    download_url: String,
    download_focus: bool,
    download_log_open: bool,
    library_root: PathBuf,
    pending_url: Option<String>,
    open_downloads_after_setup: bool,
    manager: queue::Manager,
    download_status: Option<String>,
    download_failed: bool,

    config: Config,
    theme: Theme,
    add_menu_open: bool,
    add_menu_pos: egui::Pos2,
    add_menu_rect: Rect,
    search_query: String,
    search_active: bool,
    search_focus_request: bool,
    search_rect: Option<Rect>,
    search_popup_rect: Rect,
    settings_open: bool,
    settings_tab: SettingsTab,
    layout_edit: bool,
    eq_mode: EqMode,
    spectrum: Vec<f32>,
    background_tex: Option<TextureHandle>,
    background_loaded: Option<PathBuf>,
    drop_hovering: bool,
}

impl PlayerApp {
    pub fn new(
        egui_ctx: &egui::Context,
        initial_paths: Vec<PathBuf>,
        autoplay: bool,
        open_downloads: bool,
        initial_url: Option<String>,
        real_glass: bool,
    ) -> Self {
        let mut saved = config::load();
        let configured = saved.is_some();
        let config = saved
            .take()
            .unwrap_or_else(|| Config::with_root(config::default_library_root()));
        let mut theme = Theme::from_ui(&config.ui);
        theme.real_glass = real_glass;
        configure_style(egui_ctx, &theme);

        let (audio, status) = match AudioEngine::new() {
            Ok(engine) => {
                engine.set_eq(&config.ui.eq_gains, config.ui.eq_enabled);
                (Some(engine), None)
            }
            Err(err) => (None, Some(err)),
        };

        let manager = queue::Manager::new(queue::Settings {
            root: config.library_root.clone(),
            quality: config.quality,
            parallel: config.parallel,
        });

        let mut app = Self {
            view: if configured {
                View::Dashboard
            } else {
                View::Setup
            },
            tracks: Vec::new(),
            visible: Vec::new(),
            active_playlist: None,
            playlist_tex: HashMap::new(),
            current: None,
            paused: true,
            volume: config.ui.volume,
            muted: config.ui.muted,
            shuffle: false,
            repeat: Repeat::Off,
            audio,
            thumbs: Vec::new(),
            hero_tex: None,
            scan_rx: None,
            scanning: false,
            status_error: status.is_some(),
            status,
            status_at: Some(Instant::now()),
            scroll_to_current: false,
            autoplay,
            download_url: String::new(),
            download_focus: open_downloads,
            download_log_open: false,
            library_root: config.library_root.clone(),
            pending_url: initial_url,
            open_downloads_after_setup: open_downloads,
            manager,
            download_status: None,
            download_failed: false,
            config,
            theme,
            add_menu_open: false,
            add_menu_pos: egui::Pos2::ZERO,
            add_menu_rect: Rect::NOTHING,
            search_query: String::new(),
            search_active: false,
            search_focus_request: false,
            search_rect: None,
            search_popup_rect: Rect::NOTHING,
            settings_open: false,
            settings_tab: SettingsTab::Appearance,
            layout_edit: false,
            eq_mode: EqMode::Equalizer,
            spectrum: vec![0.0; SPECTRUM_BINS],
            background_tex: None,
            background_loaded: None,
            drop_hovering: false,
        };

        if configured {
            let mut scan_paths = vec![app.library_root.clone()];
            scan_paths.extend(initial_paths);
            app.start_scan(scan_paths);

            if let Some(url) = app.pending_url.take() {
                app.download_url = url;
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
        self.audio
            .as_ref()
            .map(|audio| audio.position())
            .unwrap_or(0.0)
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
            self.notice("No audio output device available.", true);
            return;
        };

        audio.set_volume(self.volume);
        audio.set_muted(self.muted);
        audio.set_eq(&self.config.ui.eq_gains, self.config.ui.eq_enabled);

        match audio.play(&path) {
            Ok(()) => {
                self.paused = false;
                self.status = None;
            }
            Err(err) => {
                self.paused = true;
                self.notice(err, true);
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
            return self
                .current
                .or_else(|| self.visible.first().copied())
                .unwrap_or(0);
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
        self.config.ui.volume = self.volume;
        if self.volume > 0.0 {
            self.muted = false;
            self.config.ui.muted = false;
        }
        if let Some(audio) = &mut self.audio {
            audio.set_volume(self.volume);
            audio.set_muted(self.muted);
        }
    }

    fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        self.config.ui.muted = self.muted;
        if let Some(audio) = &mut self.audio {
            audio.set_muted(self.muted);
        }
    }

    /* ---------- status ---------- */

    fn notice(&mut self, message: impl Into<String>, error: bool) {
        self.status = Some(message.into());
        self.status_error = error;
        self.status_at = Some(Instant::now());
    }

    /* ---------- library ---------- */

    fn playlists(&self) -> Vec<(String, usize)> {
        let mut counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
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
            .resize_to_fill(320, 320, image::imageops::FilterType::Triangle)
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

                if added == 0 && before == 0 {
                    self.notice("No audio files found in that folder yet.", false);
                } else {
                    self.notice(
                        format!("Added {added} track{}", if added == 1 { "" } else { "s" }),
                        false,
                    );
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
        self.config.quality = quality;
        self.save_config();
    }

    fn set_parallel(&mut self, parallel: usize) {
        if self.manager.settings.parallel == parallel {
            return;
        }
        self.manager.settings.parallel = parallel;
        self.config.parallel = parallel;
        self.save_config();
    }

    fn save_config(&self) {
        config::save(&Config {
            library_root: self.library_root.clone(),
            quality: self.manager.settings.quality,
            parallel: self.manager.settings.parallel,
            ui: self.config.ui.clone(),
        });
    }

    fn refresh_theme(&mut self, ctx: &egui::Context) {
        let real_glass = self.theme.real_glass;
        let foreground_only = self.theme.foreground_only;
        self.theme = Theme::from_ui(&self.config.ui);
        self.theme.real_glass = real_glass;
        self.theme.foreground_only = foreground_only;
        configure_style(ctx, &self.theme);
        if let Some(audio) = &self.audio {
            audio.set_eq(&self.config.ui.eq_gains, self.config.ui.eq_enabled);
        }
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
        self.view = View::Dashboard;
        self.start_scan(vec![root]);

        if let Some(url) = self.pending_url.take() {
            self.download_url = url;
            self.start_download();
        }
        if self.open_downloads_after_setup {
            self.download_focus = true;
        }
    }

    /* ---------- setup screen ---------- */

    fn setup_screen(&mut self, ui: &mut Ui, rect: Rect) {
        let theme = self.theme;
        let width = (rect.width() * 0.56).clamp(420.0, 760.0);
        let height = 342.0f32.min(rect.height() - 80.0).max(280.0);
        let card = Rect::from_center_size(rect.center(), vec2(width, height));
        theme::paint_card(
            ui.painter(),
            card,
            theme.radius + 6.0,
            &theme,
            false,
            false,
            0,
            false,
        );

        let inner = card.shrink2(vec2(30.0, 24.0));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min))
                .id_salt("setup"),
        );
        child.set_clip_rect(inner);
        child.spacing_mut().item_spacing = vec2(8.0, 8.0);

        child.label(
            egui::RichText::new("DEF PLAYER")
                .size(11.0)
                .color(theme.muted())
                .strong(),
        );
        child.add_space(4.0);
        child.label(
            egui::RichText::new("Your music, your workspace.")
                .size(27.0)
                .color(theme.text())
                .strong(),
        );
        child.add_space(3.0);
        child.label(
            egui::RichText::new(
                "Choose the main folder Def Player uses for downloads and playlists. The dashboard is built around it.",
            )
            .size(12.5)
            .color(theme.muted()),
        );
        child.add_space(20.0);

        let (path_rect, _) =
            child.allocate_exact_size(vec2(child.available_width(), 44.0), Sense::hover());
        theme::gradient_rect(
            child.painter(),
            path_rect,
            10.0,
            theme.panel_soft(),
            theme.panel_bottom(),
            theme.angle,
        );
        child.painter().rect_stroke(
            path_rect,
            10.0,
            Stroke::new(1.0_f32, theme.line()),
            StrokeKind::Inside,
        );
        child.painter().text(
            pos2(path_rect.left() + 14.0, path_rect.center().y),
            Align2::LEFT_CENTER,
            ui::ellipsize(
                child.painter(),
                &self.library_root.display().to_string(),
                &FontId::monospace(12.0),
                theme.text(),
                path_rect.width() - 28.0,
            ),
            FontId::monospace(12.0),
            theme.text(),
        );

        child.add_space(12.0);
        child.horizontal(|ui| {
            if ui::pill_button_sized(ui, 150.0, "Choose folder", true, &theme, GlassSlot::Setup)
                .clicked()
            {
                if let Some(folder) = rfd::FileDialog::new()
                    .set_title("Choose your music folder")
                    .pick_folder()
                {
                    self.library_root = folder;
                }
            }
            if ui::pill_button_sized(ui, 132.0, "Use default", false, &theme, GlassSlot::Setup)
                .clicked()
            {
                self.library_root = config::default_library_root();
            }
        });

        child.add_space(8.0);
        child.label(
            egui::RichText::new(
                "Every subfolder becomes a playlist. Files directly in it are shown under All music.",
            )
            .size(11.0)
            .color(theme.faint()),
        );
        child.add_space(8.0);
        if ui::pill_button_sized(
            &mut child,
            210.0,
            "Start using Def Player",
            true,
            &theme,
            GlassSlot::Setup,
        )
        .clicked()
        {
            self.finish_setup();
        }
        if let Some(status) = &self.download_status {
            child.label(egui::RichText::new(status).size(11.5).color(theme.danger()));
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
                        .resize_to_fill(720, 720, image::imageops::FilterType::Triangle)
                        .to_rgba8()
                });
                decoded.map(|image| {
                    let color = egui::ColorImage::from_rgba_unmultiplied(
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

    fn ensure_background(&mut self, ctx: &egui::Context) {
        if self.background_loaded == self.config.ui.background_image {
            return;
        }
        self.background_loaded = self.config.ui.background_image.clone();
        self.background_tex = match &self.config.ui.background_image {
            Some(path) => match image::open(path) {
                Ok(image) => {
                    let image = image
                        .resize_to_fill(1_920, 1_080, image::imageops::FilterType::Triangle)
                        .to_rgba8();
                    let color = egui::ColorImage::from_rgba_unmultiplied(
                        [image.width() as usize, image.height() as usize],
                        image.as_raw(),
                    );
                    Some(ctx.load_texture("background-photo", color, TextureOptions::LINEAR))
                }
                Err(error) => {
                    self.notice(format!("Background photo: {error}"), true);
                    None
                }
            },
            None => None,
        };
    }

    fn choose_background(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp", "gif"])
            .set_title("Choose a background photo")
            .pick_file()
        {
            self.config.ui.background_image = Some(path);
            self.background_loaded = None;
            self.save_config();
        }
    }

    fn update_spectrum(&mut self, ctx: &egui::Context) {
        if self.spectrum.len() != SPECTRUM_BINS {
            self.spectrum.resize(SPECTRUM_BINS, 0.0);
        }

        let target = if let Some(audio) = &self.audio {
            let (samples, rate) = audio.visualizer_samples(1_024);
            visualizer::spectrum(&samples, rate, SPECTRUM_BINS)
        } else {
            vec![0.0; SPECTRUM_BINS]
        };

        for (current, next) in self.spectrum.iter_mut().zip(target) {
            let speed = if next > *current { 0.55 } else { 0.13 };
            *current += (next - *current) * speed;
        }

        if self.is_playing() && self.eq_mode == EqMode::Visualizer {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    /* ---------- input ---------- */

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (escape, search, layout, settings) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::Escape),
                input.modifiers.command && input.key_pressed(egui::Key::F),
                input.modifiers.command && input.key_pressed(egui::Key::L),
                input.modifiers.command && input.key_pressed(egui::Key::Comma),
            )
        });

        if search {
            self.search_active = true;
            self.search_focus_request = true;
        }
        if layout {
            self.layout_edit = !self.layout_edit;
            self.settings_open = false;
            self.add_menu_open = false;
            self.save_config();
        }
        if settings {
            self.settings_open = !self.settings_open;
            self.add_menu_open = false;
        }

        if escape {
            if self.settings_open {
                self.settings_open = false;
            } else if self.add_menu_open {
                self.add_menu_open = false;
            } else if self.layout_edit {
                self.layout_edit = false;
                self.save_config();
            } else {
                self.search_active = false;
            }
        }

        if ctx.egui_wants_keyboard_input() || self.settings_open || self.layout_edit {
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
            self.save_config();
        }
    }

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
                .map(|file| file.path().to_path_buf())
                .collect()
        });

        if !dropped.is_empty() {
            self.start_scan(dropped);
        }

        let hovering = ctx.input(|input| !input.raw.hovered_files.is_empty());
        self.drop_hovering = hovering;
    }

    /* ---------- top bar ---------- */

    fn top_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let theme = self.theme;
        let painter = ui.painter().clone();
        let cy = rect.center().y;
        let side = (rect.height() * 0.68).clamp(36.0, 50.0);
        let margin = (rect.width() * 0.012).clamp(12.0, 20.0);

        let plus_rect = Rect::from_center_size(
            pos2(rect.left() + margin + side * 0.5, cy),
            vec2(side, side),
        );
        let settings_rect = Rect::from_center_size(
            pos2(rect.right() - margin - side * 0.5, cy),
            vec2(side, side),
        );

        let mini_w = (rect.width() * 0.36).clamp(250.0, 520.0);
        let mini_height = (side + 4.0).clamp(40.0, 56.0);
        let mini_rect = Rect::from_min_max(
            pos2(settings_rect.left() - 16.0 - mini_w, cy - mini_height * 0.5),
            pos2(settings_rect.left() - 16.0, cy + mini_height * 0.5),
        );

        let search_left = plus_rect.right() + 16.0;
        let search_right = (mini_rect.left() - 18.0)
            .min(search_left + 520.0)
            .max(search_left + 150.0);
        let search_height = (side * 0.84).clamp(34.0, 44.0);
        let search_rect = Rect::from_min_max(
            pos2(search_left, cy - search_height * 0.5),
            pos2(search_right, cy + search_height * 0.5),
        );
        self.search_rect = Some(search_rect);

        let plus_response = ui::circle_button(
            ui,
            plus_rect,
            "add-music",
            Glyph::Plus,
            &theme,
            GlassSlot::TopBar,
        )
        .on_hover_text(if self.layout_edit {
                "Add a hidden component"
            } else {
                "Add music or a link"
            });
        if plus_response.clicked() {
            self.add_menu_open = !self.add_menu_open;
            self.add_menu_pos = plus_rect.left_bottom() + vec2(0.0, 8.0);
            self.settings_open = false;
        }

        if self.layout_edit {
            theme::paint_pill(
                ui.painter(),
                search_rect,
                &theme,
                false,
                false,
                true,
                false,
                GlassSlot::TopBar,
            );
            let layout_text = "LAYOUT MODE — drag to move · corner to resize";
            painter.text(
                pos2(search_rect.left() + 18.0, search_rect.center().y),
                Align2::LEFT_CENTER,
                ui::ellipsize(
                    &painter,
                    layout_text,
                    &FontId::proportional(12.0),
                    theme.text(),
                    (search_rect.width() - 164.0).max(80.0),
                ),
                FontId::proportional(12.0),
                theme.text(),
            );
            let done = Rect::from_center_size(
                pos2(search_rect.right() - 44.0, search_rect.center().y),
                vec2(68.0, 26.0),
            );
            let reset = Rect::from_center_size(
                pos2(search_rect.right() - 116.0, search_rect.center().y),
                vec2(68.0, 26.0),
            );
            if ui::chip(
                ui,
                reset,
                "layout-reset-top",
                "RESET",
                false,
                &theme,
                GlassSlot::TopBar,
            )
            .clicked()
            {
                self.config.ui.panels = UiConfig::default().panels;
                self.save_config();
            }
            if ui::chip(
                ui,
                done,
                "layout-done",
                "DONE",
                true,
                &theme,
                GlassSlot::TopBar,
            )
            .clicked()
            {
                self.layout_edit = false;
                self.save_config();
            }
        } else {
            let search_hovered = ui
                .ctx()
                .pointer_hover_pos()
                .map(|pos| search_rect.contains(pos))
                .unwrap_or(false);
            theme::paint_pill(
                ui.painter(),
                search_rect,
                &theme,
                search_hovered,
                false,
                false,
                false,
                GlassSlot::TopBar,
            );
            ui::paint_glyph(
                ui.painter(),
                pos2(search_rect.left() + 17.0, search_rect.center().y),
                17.0,
                Glyph::Search,
                theme.muted(),
            );

            let text_rect = search_rect.shrink2(vec2(38.0, 3.0));
            let mut child = ui.new_child(
                UiBuilder::new()
                    .max_rect(text_rect)
                    .layout(Layout::left_to_right(Align::Center))
                    .id_salt("search-field"),
            );
            let response = child.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .frame(egui::Frame::NONE)
                    .hint_text("Search music, artists, playlists")
                    .font(FontId::proportional(13.0))
                    .text_color(theme.text())
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(0, 1))
                    .align(Align2::LEFT_CENTER),
            );
            if response.changed() {
                self.search_active = true;
            }
            if response.gained_focus() {
                self.search_active = true;
            }
            if self.search_focus_request {
                response.request_focus();
                self.search_focus_request = false;
                self.search_active = true;
            }
        }

        self.mini_player(ui, mini_rect, &theme);

        if ui::circle_button(
            ui,
            settings_rect,
            "settings-open",
            Glyph::Sliders,
            &theme,
            GlassSlot::TopBar,
        )
        .clicked()
        {
            self.settings_open = true;
            self.add_menu_open = false;
        }
    }

    fn mini_player(&mut self, ui: &mut Ui, rect: Rect, theme: &Theme) {
        let cover_size = (rect.height() - 6.0).clamp(30.0, 48.0);
        let play_size = (cover_size * 0.82).clamp(28.0, 40.0);
        let cover = Rect::from_min_size(
            pos2(rect.left(), rect.center().y - cover_size * 0.5),
            vec2(cover_size, cover_size),
        );
        let play_rect = Rect::from_center_size(
            pos2(rect.right() - play_size * 0.5, rect.center().y),
            vec2(play_size, play_size),
        );
        let text_left = cover.right() + 10.0;
        let text_right = play_rect.left() - 12.0;

        let thumb = self
            .current
            .and_then(|index| self.thumbs.get(index))
            .and_then(|texture| texture.as_ref());
        ui::paint_cover(ui, cover, thumb, 9.0, theme);

        let (title, artist) = match self.track() {
            Some(track) => (track.title.clone(), track.artist.clone()),
            None => ("Nothing playing".to_string(), "Pick a track".to_string()),
        };
        let title_font = FontId::proportional(12.5);
        let artist_font = FontId::proportional(10.5);
        let max_text = (text_right - text_left).max(20.0);
        let clip = Rect::from_min_max(pos2(text_left, rect.top()), pos2(text_right, rect.bottom()));
        let painter = ui.painter().with_clip_rect(clip);
        painter.text(
            pos2(text_left, rect.top() + rect.height() * 0.30),
            Align2::LEFT_CENTER,
            ui::ellipsize(&painter, &title, &title_font, theme.text(), max_text),
            title_font,
            theme.text(),
        );
        painter.text(
            pos2(text_left, rect.top() + rect.height() * 0.57),
            Align2::LEFT_CENTER,
            ui::ellipsize(
                &painter,
                if self.scanning {
                    "Scanning…"
                } else {
                    &artist
                },
                &artist_font,
                theme.muted(),
                max_text,
            ),
            artist_font,
            if self.scanning {
                theme.accent_b
            } else {
                theme.muted()
            },
        );

        let seek_rect = Rect::from_min_max(
            pos2(text_left, rect.bottom() - 8.0),
            pos2(text_right, rect.bottom() - 2.0),
        );
        let duration = self.duration();
        let fraction = if duration > 0.0 {
            (self.position() / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (_, requested) =
            ui::slider(ui, seek_rect, "mini-seek", fraction, duration > 0.0, theme);
        if let Some(value) = requested {
            self.seek_to(value);
        }

        let cover_resp = ui
            .interact(cover, ui.scope_id().with("mini-cover"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if cover_resp.clicked() {
            self.toggle_play();
        }

        let glyph = if self.is_playing() {
            Glyph::Pause
        } else {
            Glyph::Play
        };
        if ui::glyph_button(
            ui,
            play_rect,
            "mini-play",
            glyph,
            true,
            theme,
            GlassSlot::Transport,
        )
        .clicked()
        {
            self.toggle_play();
        }
    }

    /* ---------- dashboard ---------- */

    fn dashboard(&mut self, ui: &mut Ui, frame: Rect) {
        let content = Self::dashboard_content(frame);

        for kind in PANEL_KINDS {
            if self.config.ui.panels[kind.index()].visible {
                self.draw_panel(ui, content, kind);
            }
        }

        if self.layout_edit {
            self.layout_overlay(ui, frame, content);
        }

        self.status_toast(ui, frame);
    }

    fn panel_rect(&self, content: Rect, kind: PanelKind) -> Rect {
        let panel = self.config.ui.panels[kind.index()];
        Rect::from_min_size(
            pos2(
                content.left() + panel.x * content.width(),
                content.top() + panel.y * content.height(),
            ),
            vec2(panel.w * content.width(), panel.h * content.height()),
        )
    }

    fn draw_panel(&mut self, ui: &mut Ui, content: Rect, kind: PanelKind) {
        let theme = self.theme;
        let rect = self.panel_rect(content, kind);
        if rect.width() < 40.0 || rect.height() < 30.0 {
            return;
        }

        let hovered = ui
            .ctx()
            .pointer_hover_pos()
            .map(|pos| rect.contains(pos))
            .unwrap_or(false);
        theme::paint_card(ui.painter(), rect, theme.radius, &theme, hovered, false, 0, true);

        let border_pad = if theme.panel_border_visible {
            theme.panel_border_width.max(0.0)
        } else {
            0.0
        };
        let pad = if kind == PanelKind::Player {
            0.0
        } else {
            (rect.width().min(rect.height()) * 0.055).clamp(10.0, 18.0) + border_pad
        };
        let inner = rect.shrink(pad);
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min))
                .id_salt(("panel", kind.key())),
        );
        child.set_clip_rect(inner);
        child.spacing_mut().item_spacing = vec2(6.0, 6.0);

        match kind {
            PanelKind::CurrentPlaylist => self.current_playlist_panel(&mut child),
            PanelKind::Player => self.player_panel(&mut child),
            PanelKind::Downloader => self.downloader_panel(&mut child),
            PanelKind::Playlists => self.playlists_panel(&mut child),
            PanelKind::Equalizer => self.equalizer_panel(&mut child),
        }
    }

    fn settings_heading(ui: &mut Ui, theme: &Theme, title: &str, help: &str) {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(title)
                .size(10.5)
                .color(theme.muted())
                .strong(),
        );
        if !help.is_empty() {
            ui.label(egui::RichText::new(help).size(10.0).color(theme.faint()));
        }
    }

    fn panel_header(ui: &mut Ui, theme: &Theme, title: &str, meta: &str) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
        let painter = ui.painter();
        painter.text(
            pos2(rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            title,
            FontId::proportional(10.5),
            theme.muted(),
        );
        if !meta.is_empty() {
            painter.text(
                pos2(rect.right(), rect.center().y),
                Align2::RIGHT_CENTER,
                meta,
                FontId::proportional(10.5),
                theme.faint(),
            );
        }
    }

    fn current_playlist_panel(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        let name = self
            .active_playlist
            .clone()
            .unwrap_or_else(|| "All music".to_string());
        let count = self.visible.len();
        let duration: f32 = self
            .visible
            .iter()
            .map(|index| self.tracks[*index].duration)
            .sum();

        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
        let painter = ui.painter();
        painter.text(
            pos2(head.left(), head.top() + 9.0),
            Align2::LEFT_CENTER,
            "CURRENT PLAYLIST",
            FontId::proportional(10.0),
            theme.muted(),
        );
        painter.text(
            pos2(head.left(), head.top() + 28.0),
            Align2::LEFT_CENTER,
            ui::ellipsize(
                painter,
                &name,
                &FontId::proportional(15.0),
                theme.text(),
                head.width() - 10.0,
            ),
            FontId::proportional(15.0),
            theme.text(),
        );
        painter.text(
            pos2(head.right(), head.top() + 9.0),
            Align2::RIGHT_CENTER,
            if count == 0 {
                "—".to_string()
            } else {
                format!("{count} · {}", format_duration(duration))
            },
            FontId::proportional(10.0),
            theme.faint(),
        );

        if self.visible.is_empty() {
            let (empty, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 60.0), Sense::hover());
            ui.painter().text(
                empty.center(),
                Align2::CENTER_CENTER,
                "Nothing here yet.\nAdd music or pick a playlist.",
                FontId::proportional(11.5),
                theme.faint(),
            );
            return;
        }

        let visible = self.visible.clone();
        let current = self.current;
        let mut clicked = None;
        let mut scroll_to_current = self.scroll_to_current;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("current-playlist-list")
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for index in visible.iter().copied() {
                    let track = &self.tracks[index];
                    let active = current == Some(index);
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
                    let row = rect.shrink2(vec2(2.0, 1.0));
                    let painter = ui.painter();

                    let glass = crate::glass::paint_row(
                        painter,
                        row,
                        &theme,
                        response.hovered(),
                        active,
                    );
                    if !glass {
                        if active {
                            theme::gradient_rect(
                                painter,
                                row,
                                9.0,
                                theme::with_alpha(theme.accent_a, 54),
                                theme::with_alpha(theme.accent_b, 22),
                                theme.angle,
                            );
                        } else if response.hovered() {
                            painter.rect_filled(
                                row,
                                9.0,
                                Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                            );
                        }
                    }

                    let cover = Rect::from_min_size(
                        pos2(row.left() + 4.0, row.center().y - 18.0),
                        vec2(36.0, 36.0),
                    );
                    ui::paint_cover(ui, cover, self.thumbs[index].as_ref(), 7.0, &theme);

                    let text_left = cover.right() + 9.0;
                    let wide = row.width() > 215.0;
                    let time_width = if wide { 36.0 } else { 0.0 };
                    let text_width = (row.right() - 8.0 - time_width - text_left).max(20.0);
                    let title_font = FontId::proportional(12.0);
                    let artist_font = FontId::proportional(10.5);
                    let title_color = theme.text();
                    painter.text(
                        pos2(text_left, row.center().y - 8.0),
                        Align2::LEFT_CENTER,
                        ui::ellipsize(painter, &track.title, &title_font, title_color, text_width),
                        title_font,
                        title_color,
                    );
                    painter.text(
                        pos2(text_left, row.center().y + 8.0),
                        Align2::LEFT_CENTER,
                        ui::ellipsize(
                            painter,
                            &track.artist,
                            &artist_font,
                            theme.muted(),
                            text_width,
                        ),
                        artist_font,
                        theme.muted(),
                    );
                    if wide {
                        painter.text(
                            pos2(row.right() - 6.0, row.center().y),
                            Align2::RIGHT_CENTER,
                            format_duration(track.duration),
                            FontId::proportional(10.5),
                            theme.faint(),
                        );
                    }

                    if active && scroll_to_current {
                        ui.scroll_to_rect(rect, Some(Align::Center));
                        scroll_to_current = false;
                    }

                    if response.clicked() {
                        clicked = Some(index);
                    }
                }
            });

        self.scroll_to_current = scroll_to_current;
        if let Some(index) = clicked {
            if self.current == Some(index) {
                self.toggle_play();
            } else {
                self.play_index(index);
            }
        }
    }

    fn playlists_panel(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        let playlists = self.playlists();
        let active = self.active_playlist.clone();
        let mut entries: Vec<(Option<String>, String, usize, Option<TextureHandle>)> =
            Vec::with_capacity(playlists.len() + 1);
        entries.push((None, "All music".to_string(), self.tracks.len(), None));
        for (name, count) in playlists {
            let texture = self.playlist_texture(&name).cloned();
            entries.push((Some(name.clone()), name, count, texture));
        }

        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        let painter = ui.painter();
        painter.text(
            pos2(head.left(), head.center().y),
            Align2::LEFT_CENTER,
            "PLAYLISTS",
            FontId::proportional(10.5),
            theme.muted(),
        );
        painter.text(
            pos2(head.right(), head.center().y),
            Align2::RIGHT_CENTER,
            format!("{} collections", entries.len().saturating_sub(1)),
            FontId::proportional(10.5),
            theme.faint(),
        );

        let mut selected: Option<Option<String>> = None;
        let card_size = (ui.available_height() - 4.0).clamp(54.0, 190.0);
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .id_salt("playlist-covers")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for (playlist, name, count, texture) in &entries {
                        let is_active = active == *playlist;
                        let (rect, response) =
                            ui.allocate_exact_size(vec2(card_size, card_size), Sense::click());
                        if let Some(texture) = texture {
                            ui::paint_cover(ui, rect, Some(texture), 12.0, &theme);
                        } else {
                            theme::gradient_rect(
                                ui.painter(),
                                rect,
                                12.0,
                                theme::lerp_color(theme.panel_soft(), theme.accent_a, 0.25),
                                theme::lerp_color(theme.panel_bottom(), theme.accent_b, 0.18),
                                theme.angle,
                            );
                            ui::paint_glyph(
                                ui.painter(),
                                rect.center(),
                                rect.width() * 0.32,
                                Glyph::Note,
                                theme::with_alpha(theme.text(), 180),
                            );
                        }

                        // Bottom shade so the label stays readable over any cover.
                        let shade = Rect::from_min_max(
                            pos2(rect.left(), rect.bottom() - rect.height() * 0.42),
                            rect.max,
                        );
                        theme::gradient_rect(
                            ui.painter(),
                            shade,
                            12.0,
                            Color32::TRANSPARENT,
                            Color32::from_black_alpha(210),
                            90.0,
                        );

                        ui.painter().text(
                            pos2(rect.left() + 10.0, rect.bottom() - 10.0),
                            Align2::LEFT_BOTTOM,
                            ui::ellipsize(
                                ui.painter(),
                                name,
                                &FontId::proportional(12.0),
                                theme.text(),
                                rect.width() - 20.0,
                            ),
                            FontId::proportional(12.0),
                            theme.text(),
                        );
                        ui.painter().text(
                            pos2(rect.right() - 10.0, rect.top() + 10.0),
                            Align2::RIGHT_TOP,
                            format!("{count}"),
                            FontId::proportional(10.5),
                            theme::with_alpha(theme.text(), 210),
                        );
                        let glass_ring = crate::glass::paint_card_ring(
                            ui.painter(),
                            rect,
                            12.0,
                            &theme,
                            response.hovered(),
                            is_active,
                        );
                        if !glass_ring {
                            if is_active {
                                ui.painter().rect_stroke(
                                    rect,
                                    12.0,
                                    Stroke::new(2.0_f32, theme.accent_b),
                                    StrokeKind::Inside,
                                );
                            } else if response.hovered() {
                                ui.painter().rect_stroke(
                                    rect,
                                    12.0,
                                    Stroke::new(
                                        1.5_f32,
                                        theme::with_alpha(theme.accent_a, 120),
                                    ),
                                    StrokeKind::Inside,
                                );
                            }
                        }

                        if response.clicked() {
                            selected = Some(playlist.clone());
                        }
                    }
                });
            });

        if let Some(playlist) = selected {
            self.active_playlist = playlist;
            self.rebuild_visible();
        }
    }

    fn player_panel(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        let rect = ui.max_rect();
        let border_inset = if theme.panel_border_visible {
            theme.panel_border_width.max(0.0)
        } else {
            0.0
        };
        let art = rect.shrink(border_inset);

        let title = self
            .track()
            .map(|track| track.title.clone())
            .unwrap_or_else(|| "Nothing playing".to_string());
        let artist = self
            .track()
            .map(|track| {
                if track.album.is_empty() {
                    track.artist.clone()
                } else {
                    format!("{} — {}", track.artist, track.album)
                }
            })
            .unwrap_or_else(|| "Pick a track from your library".to_string());
        let duration = self.duration();
        let position = self.position();
        let fraction = if duration > 0.0 {
            (position / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let pad = (art.width().min(art.height()) * 0.055).clamp(14.0, 24.0) + border_inset;
        let left = rect.left() + pad;
        let right = rect.right() - pad;
        let bottom = rect.bottom() - pad;
        let title_size = (rect.height() * 0.052).clamp(17.0, 28.0);

        ui.painter().text(
            pos2(left, rect.top() + pad),
            Align2::LEFT_TOP,
            "NOW PLAYING",
            FontId::proportional(10.5),
            theme::with_alpha(theme.text(), 210),
        );
        if let Some(name) = &self.active_playlist {
            ui.painter().text(
                pos2(right, rect.top() + pad),
                Align2::RIGHT_TOP,
                ui::ellipsize(
                    ui.painter(),
                    name,
                    &FontId::proportional(11.0),
                    theme::with_alpha(theme.text(), 210),
                    (right - left) * 0.48,
                ),
                FontId::proportional(11.0),
                theme::with_alpha(theme.text(), 210),
            );
        }

        let controls_y = bottom - 22.0;
        let seek_y = controls_y - 34.0;
        let text_y = seek_y - 46.0;
        let clip = Rect::from_min_max(pos2(left, text_y - 40.0), pos2(right, seek_y + 10.0));
        let text_painter = ui.painter().with_clip_rect(clip);
        text_painter.text(
            pos2(left, text_y),
            Align2::LEFT_CENTER,
            ui::ellipsize(
                &text_painter,
                &title,
                &FontId::proportional(title_size),
                theme.text(),
                right - left,
            ),
            FontId::proportional(title_size),
            theme.text(),
        );
        text_painter.text(
            pos2(left, text_y + title_size * 0.86 + 8.0),
            Align2::LEFT_CENTER,
            ui::ellipsize(
                &text_painter,
                &artist,
                &FontId::proportional(12.5),
                theme.muted(),
                right - left,
            ),
            FontId::proportional(12.5),
            theme.muted(),
        );

        let time_width = 38.0;
        ui.painter().text(
            pos2(left, seek_y),
            Align2::LEFT_CENTER,
            format_duration(position),
            FontId::proportional(10.5),
            theme::with_alpha(theme.text(), 190),
        );
        ui.painter().text(
            pos2(right, seek_y),
            Align2::RIGHT_CENTER,
            format_duration(duration),
            FontId::proportional(10.5),
            theme::with_alpha(theme.text(), 190),
        );
        let seek_rect = Rect::from_min_max(
            pos2(left + time_width + 8.0, seek_y - 9.0),
            pos2(right - time_width - 8.0, seek_y + 9.0),
        );
        let (_, requested) = ui::slider(
            ui,
            seek_rect,
            "player-seek",
            fraction,
            duration > 0.0,
            &theme,
        );
        if let Some(value) = requested {
            self.seek_to(value);
        }

        let center_x = rect.center().x;
        let play_size = (rect.height() * 0.085).clamp(30.0, 42.0);
        let play_rect =
            Rect::from_center_size(pos2(center_x, controls_y), vec2(play_size, play_size));
        let prev_rect = Rect::from_center_size(
            pos2(center_x - play_size * 1.2, controls_y),
            vec2(play_size * 0.62, play_size * 0.62),
        );
        let next_rect = Rect::from_center_size(
            pos2(center_x + play_size * 1.2, controls_y),
            vec2(play_size * 0.62, play_size * 0.62),
        );
        let glyph = if self.is_playing() {
            Glyph::Pause
        } else {
            Glyph::Play
        };
        if ui::glyph_button(
            ui,
            prev_rect,
            "player-prev",
            Glyph::Prev,
            false,
            &theme,
            GlassSlot::Transport,
        )
        .clicked()
        {
            self.previous();
        }
        if ui::glyph_button(
            ui,
            play_rect,
            "player-play",
            glyph,
            true,
            &theme,
            GlassSlot::Transport,
        )
        .clicked()
        {
            self.toggle_play();
        }
        if ui::glyph_button(
            ui,
            next_rect,
            "player-next",
            Glyph::Next,
            false,
            &theme,
            GlassSlot::Transport,
        )
        .clicked()
        {
            self.advance(1, false);
        }

        if rect.width() > 390.0 {
            let shuffle_rect =
                Rect::from_center_size(pos2(left + 38.0, controls_y), vec2(76.0, 24.0));
            let repeat_rect =
                Rect::from_center_size(pos2(left + 122.0, controls_y), vec2(82.0, 24.0));
            if ui::chip(
                ui,
                shuffle_rect,
                "shuffle",
                "SHUFFLE",
                self.shuffle,
                &theme,
                GlassSlot::PlayerChip,
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
            if ui::chip(
                ui,
                repeat_rect,
                "repeat",
                repeat_label,
                self.repeat != Repeat::Off,
                &theme,
                GlassSlot::PlayerChip,
            )
            .clicked()
            {
                self.repeat = match self.repeat {
                    Repeat::Off => Repeat::All,
                    Repeat::All => Repeat::One,
                    Repeat::One => Repeat::Off,
                };
            }
        }

        if rect.width() > 510.0 {
            let mute = Rect::from_center_size(pos2(right - 12.0, controls_y), vec2(28.0, 28.0));
            let volume_rect = Rect::from_min_max(
                pos2(mute.left() - 106.0, controls_y - 9.0),
                pos2(mute.left() - 10.0, controls_y + 9.0),
            );
            if ui::glyph_button(
                ui,
                mute,
                "player-mute",
                if self.muted {
                    Glyph::Mute
                } else {
                    Glyph::Volume
                },
                false,
                &theme,
                GlassSlot::Transport,
            )
            .clicked()
            {
                self.toggle_mute();
            }
            let (_, requested) = ui::slider(
                ui,
                volume_rect,
                "player-volume",
                if self.muted { 0.0 } else { self.volume },
                true,
                &theme,
            );
            if let Some(value) = requested {
                self.set_volume(value);
                self.save_config();
            }
        }

        let cover_click = ui.interact(
            Rect::from_min_max(art.min, pos2(art.right(), seek_y - 18.0)),
            ui.scope_id().with("player-cover-click"),
            Sense::click(),
        );
        if cover_click.clicked() {
            self.toggle_play();
        }
    }

    fn downloader_panel(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        let active = self.manager.queued_or_running();
        Self::panel_header(
            ui,
            &theme,
            "DOWNLOADER",
            &if active > 0 {
                format!("{active} active")
            } else {
                "idle".to_string()
            },
        );

        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(ui.available_rect_before_wrap())
                .layout(Layout::top_down(Align::Min))
                .id_salt("downloader-body"),
        );
        child.spacing_mut().item_spacing = vec2(6.0, 6.0);

        let input_height = 34.0;
        let (input_rect, _) =
            child.allocate_exact_size(vec2(child.available_width(), input_height), Sense::hover());
        theme::gradient_rect(
            child.painter(),
            input_rect,
            9.0,
            theme.panel_soft(),
            theme.panel_bottom(),
            theme.angle,
        );
        child.painter().rect_stroke(
            input_rect,
            9.0,
            Stroke::new(1.0_f32, theme.line()),
            StrokeKind::Inside,
        );
        let text_rect = input_rect.shrink2(vec2(10.0, 3.0));
        let mut input_child = child.new_child(
            UiBuilder::new()
                .max_rect(text_rect)
                .layout(Layout::left_to_right(Align::Center))
                .id_salt("download-url"),
        );
        let response = input_child.add(
            egui::TextEdit::singleline(&mut self.download_url)
                .frame(egui::Frame::NONE)
                .hint_text("YouTube or Spotify link…")
                .font(FontId::proportional(12.0))
                .text_color(theme.text())
                .desired_width(f32::INFINITY)
                .margin(egui::Margin::symmetric(0, 1))
                .align(Align2::LEFT_CENTER),
        );
        if self.download_focus {
            response.request_focus();
            self.download_focus = false;
        }
        if response.lost_focus() && child.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.start_download();
        }

        child.horizontal(|ui| {
            if ui::chip_button(
                ui,
                "DOWNLOAD",
                true,
                96.0,
                &theme,
                GlassSlot::DownloaderAction,
            )
            .clicked()
            {
                self.start_download();
            }
            if active > 0
                && ui::chip_button(
                    ui,
                    "CANCEL ALL",
                    false,
                    92.0,
                    &theme,
                    GlassSlot::DownloaderAction,
                )
                .clicked()
            {
                self.manager.cancel_all();
            }
            if ui::chip_button(
                ui,
                "CHANGE FOLDER",
                false,
                110.0,
                &theme,
                GlassSlot::DownloaderAction,
            )
            .clicked()
            {
                self.choose_library_folder();
            }
        });

        let source = download::detect(&self.download_url);
        let source_label = if self.download_url.trim().is_empty() {
            "Waiting for a link".to_string()
        } else {
            format!("Detected: {}", source.label())
        };
        child.label(
            egui::RichText::new(source_label)
                .size(10.5)
                .color(theme.faint()),
        );

        let mut quality_clicked = None;
        let mut parallel_clicked = None;
        child.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                egui::RichText::new("QUALITY")
                    .size(9.5)
                    .color(theme.muted()),
            );
            for quality in [128u16, 192, 256, 320] {
                let current = self.manager.settings.quality == quality;
                if ui::chip_button(
                    ui,
                    &format!("{quality}"),
                    current,
                    40.0,
                    &theme,
                    GlassSlot::OptionChip,
                )
                .clicked()
                {
                    quality_clicked = Some(quality);
                }
            }
        });
        child.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                egui::RichText::new("AT ONCE")
                    .size(9.5)
                    .color(theme.muted()),
            );
            for parallel in [1usize, 2, 3, 4, 6] {
                let current = self.manager.settings.parallel == parallel;
                if ui::chip_button(
                    ui,
                    &parallel.to_string(),
                    current,
                    24.0,
                    &theme,
                    GlassSlot::OptionChip,
                )
                .clicked()
                {
                    parallel_clicked = Some(parallel);
                }
            }
        });
        if let Some(quality) = quality_clicked {
            self.set_quality(quality);
        }
        if let Some(parallel) = parallel_clicked {
            self.set_parallel(parallel);
        }

        let path_text = format!("Saving to  {}", self.library_root.display());
        child.label(
            egui::RichText::new(ui::ellipsize(
                child.painter(),
                &path_text,
                &FontId::proportional(10.0),
                theme.faint(),
                child.available_width(),
            ))
            .size(10.0)
            .color(theme.faint()),
        );

        if let Some(status) = &self.download_status {
            child.label(
                egui::RichText::new(status)
                    .size(10.5)
                    .color(if self.download_failed {
                        theme.danger()
                    } else {
                        theme.muted()
                    }),
            );
        }

        let jobs: Vec<(u64, String, String, JobState, Option<f32>)> = self
            .manager
            .jobs()
            .iter()
            .map(|job| {
                (
                    job.id,
                    job.title.clone(),
                    job.detail.clone(),
                    job.state,
                    job.progress,
                )
            })
            .collect();
        let finished = jobs.iter().filter(|job| !job.3.is_active()).count();

        child.horizontal(|ui| {
            ui.label(egui::RichText::new("QUEUE").size(10.0).color(theme.muted()));
            ui.label(
                egui::RichText::new(format!(
                    "{} waiting · {} running",
                    active.saturating_sub(self.manager.running_count()),
                    self.manager.running_count()
                ))
                .size(10.0)
                .color(theme.faint()),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if finished > 0
                    && ui::chip_button(
                        ui,
                        "CLEAR",
                        false,
                        54.0,
                        &theme,
                        GlassSlot::DownloaderAction,
                    )
                    .clicked()
                {
                    self.manager.clear_finished();
                }
                if ui::chip_button(
                    ui,
                    "LOG",
                    self.download_log_open,
                    44.0,
                    &theme,
                    GlassSlot::DownloaderAction,
                )
                .clicked()
                {
                    self.download_log_open = !self.download_log_open;
                }
            });
        });

        let log_height = if self.download_log_open { 62.0 } else { 0.0 };
        let queue_height = (child.available_height() - log_height - 4.0).max(46.0);
        let mut cancel_id = None;
        let job_time = child.input(|input| input.time);

        let mut list = child.new_child(
            UiBuilder::new()
                .max_rect(Rect::from_min_size(
                    child.cursor().min,
                    vec2(child.available_width(), queue_height),
                ))
                .layout(Layout::top_down(Align::Min))
                .id_salt("download-jobs"),
        );
        list.set_clip_rect(Rect::from_min_size(
            child.cursor().min,
            vec2(child.available_width(), queue_height),
        ));

        if jobs.is_empty() {
            list.painter().text(
                list.max_rect().center(),
                Align2::CENTER_CENTER,
                "Nothing queued yet.",
                FontId::proportional(11.0),
                theme.faint(),
            );
            child.advance_cursor_after_rect(Rect::from_min_size(
                child.cursor().min,
                vec2(child.available_width(), queue_height),
            ));
        } else {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .id_salt("download-jobs-scroll")
                .show(&mut list, |ui| {
                    for (id, title, detail, state, progress) in &jobs {
                        let (row, response) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
                        let painter = ui.painter();
                        crate::glass::paint_row(
                            painter,
                            row.shrink2(vec2(1.0, 1.0)),
                            &theme,
                            response.hovered(),
                            false,
                        );
                        let title_color = match state {
                            JobState::Failed => theme.danger(),
                            JobState::Done | JobState::Cancelled => theme.muted(),
                            _ => theme.text(),
                        };
                        painter.text(
                            pos2(row.left(), row.top() + 13.0),
                            Align2::LEFT_CENTER,
                            ui::ellipsize(
                                painter,
                                title,
                                &FontId::proportional(11.5),
                                title_color,
                                row.width() - 58.0,
                            ),
                            FontId::proportional(11.5),
                            title_color,
                        );
                        painter.text(
                            pos2(row.left(), row.top() + 27.0),
                            Align2::LEFT_CENTER,
                            ui::ellipsize(
                                painter,
                                detail,
                                &FontId::proportional(9.5),
                                theme.faint(),
                                row.width() - 58.0,
                            ),
                            FontId::proportional(9.5),
                            theme.faint(),
                        );

                        let state_text = match state {
                            JobState::Queued => "queued".to_string(),
                            JobState::Done => "done".to_string(),
                            JobState::Failed => "failed".to_string(),
                            JobState::Cancelled => "cancel".to_string(),
                            JobState::Running => match progress {
                                Some(value) => format!("{:.0}%", value * 100.0),
                                None => "…".to_string(),
                            },
                        };
                        painter.text(
                            pos2(row.right() - 18.0, row.top() + 13.0),
                            Align2::RIGHT_CENTER,
                            state_text,
                            FontId::proportional(9.5),
                            if *state == JobState::Failed {
                                theme.danger()
                            } else {
                                theme.muted()
                            },
                        );
                        let bar = Rect::from_min_max(
                            pos2(row.left(), row.bottom() - 6.0),
                            pos2(row.right() - 18.0, row.bottom() - 3.0),
                        );
                        let bar_color = if *state == JobState::Failed {
                            theme.danger()
                        } else if matches!(state, JobState::Done | JobState::Cancelled) {
                            theme::with_alpha(theme.accent_b, 120)
                        } else {
                            theme.accent_a
                        };
                        let fraction = if *state == JobState::Done {
                            Some(1.0)
                        } else {
                            *progress
                        };
                        ui::progress_bar(painter, bar, fraction, bar_color, job_time, &theme);

                        if state.is_active() {
                            let cancel_rect = Rect::from_center_size(
                                pos2(row.right() - 6.0, row.top() + 16.0),
                                vec2(20.0, 20.0),
                            );
                            let response = ui.interact(
                                cancel_rect,
                                Id::unique(("cancel-job", *id)),
                                Sense::click(),
                            );
                            if response.hovered() {
                                crate::glass::paint_button(
                                    ui.painter(),
                                    cancel_rect,
                                    &theme,
                                    true,
                                    response.is_pointer_button_down_on(),
                                    false,
                                    false,
                                    GlassSlot::DownloaderAction,
                                );
                            }
                            ui::paint_glyph(
                                ui.painter(),
                                cancel_rect.center(),
                                12.0,
                                Glyph::Close,
                                if response.hovered() {
                                    theme.danger()
                                } else {
                                    theme.faint()
                                },
                            );
                            if response.clicked() {
                                cancel_id = Some(*id);
                            }
                        }
                    }
                });
            child.advance_cursor_after_rect(Rect::from_min_size(
                child.cursor().min,
                vec2(child.available_width(), queue_height),
            ));
        }

        if let Some(id) = cancel_id {
            self.manager.cancel(id);
        }

        if self.download_log_open {
            child.add_space(4.0);
            let log_rect = Rect::from_min_size(
                child.cursor().min,
                vec2(child.available_width(), log_height),
            );
            theme::gradient_rect(
                child.painter(),
                log_rect,
                8.0,
                theme.panel_soft(),
                theme.panel_bottom(),
                theme.angle,
            );
            let start = self.manager.log.len().saturating_sub(4);
            for (index, line) in self.manager.log[start..].iter().enumerate() {
                child.painter().text(
                    pos2(
                        log_rect.left() + 8.0,
                        log_rect.top() + 10.0 + index as f32 * 12.0,
                    ),
                    Align2::LEFT_TOP,
                    ui::ellipsize(
                        child.painter(),
                        line,
                        &FontId::monospace(8.5),
                        theme.faint(),
                        log_rect.width() - 16.0,
                    ),
                    FontId::monospace(8.5),
                    theme.faint(),
                );
            }
        }
    }

    fn equalizer_panel(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        let painter = ui.painter();
        painter.text(
            pos2(head.left(), head.center().y),
            Align2::LEFT_CENTER,
            "EQUALIZER",
            FontId::proportional(10.5),
            theme.muted(),
        );
        let toggle_w = 58.0;
        let toggle_right = head.right() - 4.0;
        let eq_rect = Rect::from_center_size(
            pos2(toggle_right - toggle_w * 1.5 - 3.0, head.center().y),
            vec2(toggle_w, 22.0),
        );
        let bars_rect = Rect::from_center_size(
            pos2(toggle_right - toggle_w * 0.5, head.center().y),
            vec2(toggle_w, 22.0),
        );
        let power_rect = Rect::from_center_size(
            pos2(toggle_right - toggle_w * 2.0 - 28.0, head.center().y),
            vec2(42.0, 22.0),
        );
        if ui::chip(
            ui,
            power_rect,
            "eq-power",
            if self.config.ui.eq_enabled {
                "ON"
            } else {
                "OFF"
            },
            self.config.ui.eq_enabled,
            &theme,
            GlassSlot::OptionChip,
        )
        .clicked()
        {
            self.config.ui.eq_enabled = !self.config.ui.eq_enabled;
            if let Some(audio) = &self.audio {
                audio.set_eq(&self.config.ui.eq_gains, self.config.ui.eq_enabled);
            }
            self.save_config();
        }
        if ui::chip(
            ui,
            eq_rect,
            "eq-mode-eq",
            "EQ",
            self.eq_mode == EqMode::Equalizer,
            &theme,
            GlassSlot::OptionChip,
        )
        .clicked()
        {
            self.eq_mode = EqMode::Equalizer;
        }
        if ui::chip(
            ui,
            bars_rect,
            "eq-mode-bars",
            "BARS",
            self.eq_mode == EqMode::Visualizer,
            &theme,
            GlassSlot::OptionChip,
        )
        .clicked()
        {
            self.eq_mode = EqMode::Visualizer;
        }

        if self.eq_mode == EqMode::Visualizer {
            self.visualizer_view(ui);
            return;
        }

        if ui.available_height() > 170.0 {
            let mut preset: Option<[f32; EQ_BANDS]> = None;
            ui.horizontal(|ui| {
                for (label, gains) in [
                    ("FLAT", [0.0; EQ_BANDS]),
                    (
                        "BASS",
                        [7.0, 6.0, 4.0, 1.0, 0.0, -1.0, -2.0, -2.0, -1.0, 0.0],
                    ),
                    (
                        "VOCAL",
                        [-2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 3.0, 1.0, 0.0, -1.0],
                    ),
                    ("CLUB", [4.0, 4.0, 2.0, 0.0, 0.0, 1.0, 2.0, 3.0, 2.0, 1.0]),
                ] {
                    if ui::chip_button(
                        ui,
                        label,
                        false,
                        50.0,
                        &theme,
                        GlassSlot::OptionChip,
                    )
                    .clicked()
                    {
                        preset = Some(gains);
                    }
                }
            });
            if let Some(gains) = preset {
                self.config.ui.eq_gains = gains;
                if let Some(audio) = &self.audio {
                    audio.set_eq(&self.config.ui.eq_gains, self.config.ui.eq_enabled);
                }
                self.save_config();
            }
        }

        let mut gains = self.config.ui.eq_gains;
        let area = ui.available_rect_before_wrap();
        let band_width = area.width() / EQ_BANDS as f32;
        let top = area.top() + 14.0;
        let bottom = area.bottom() - 20.0;
        let mut changed = false;
        let mut dragging = false;
        let mut stopped = false;

        for (index, frequency) in EQ_FREQS.iter().enumerate() {
            let band_rect = Rect::from_min_size(
                pos2(area.left() + band_width * index as f32 + 2.0, area.top()),
                vec2((band_width - 4.0).max(8.0), area.height()),
            );
            let response = ui.interact(
                band_rect,
                Id::unique(("eq-band", index)),
                Sense::click_and_drag(),
            );
            let center_y = bottom - (bottom - top) * 0.5;
            if response.dragged() || response.clicked() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let t = (1.0 - (pointer.y - top) / (bottom - top).max(1.0)).clamp(0.0, 1.0);
                    gains[index] = -12.0 + t * 24.0;
                    changed = true;
                }
            }
            dragging |= response.dragged();
            stopped |= response.drag_stopped();

            let painter = ui.painter();
            let x = band_rect.center().x;
            painter.line_segment(
                [pos2(x, top), pos2(x, bottom)],
                Stroke::new(3.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 22)),
            );
            let normalized = ((gains[index] + 12.0) / 24.0).clamp(0.0, 1.0);
            let knob_y = bottom - (bottom - top) * normalized;
            let fill_from = knob_y.min(center_y);
            let fill_to = knob_y.max(center_y);
            theme::gradient_rect(
                painter,
                Rect::from_min_max(pos2(x - 1.5, fill_from), pos2(x + 1.5, fill_to)),
                1.5,
                if gains[index] >= 0.0 {
                    theme.accent_a
                } else {
                    theme.accent_b
                },
                if gains[index] >= 0.0 {
                    theme.accent_b
                } else {
                    theme.accent_a
                },
                theme.angle,
            );
            if !crate::glass::paint_orb(
                painter,
                pos2(x, knob_y),
                5.0,
                &theme,
                response.hovered() || response.dragged(),
                response.dragged(),
            ) {
                painter.circle_filled(pos2(x, knob_y), 4.5, theme.text());
            }
            if response.hovered() || response.dragged() {
                painter.circle_stroke(
                    pos2(x, knob_y),
                    7.0,
                    Stroke::new(1.0_f32, theme::with_alpha(theme.accent_b, 150)),
                );
            }
            let label = if *frequency >= 1000.0 {
                format!("{:.0}k", frequency / 1000.0)
            } else {
                format!("{frequency:.0}")
            };
            painter.text(
                pos2(x, bottom + 10.0),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(8.5),
                theme.faint(),
            );
            painter.text(
                pos2(x, top - 8.0),
                Align2::CENTER_CENTER,
                format!("{:+.0}", gains[index]),
                FontId::proportional(8.0),
                if gains[index].abs() > 0.5 {
                    theme.muted()
                } else {
                    theme.faint()
                },
            );
        }

        if changed {
            self.config.ui.eq_gains = gains;
            if let Some(audio) = &self.audio {
                audio.set_eq(&self.config.ui.eq_gains, self.config.ui.eq_enabled);
            }
        }
        if stopped {
            self.save_config();
        }
        if dragging {
            ui.ctx().request_repaint();
        }
    }

    fn visualizer_view(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        let rect = ui.available_rect_before_wrap();
        if rect.width() < 20.0 || rect.height() < 20.0 {
            return;
        }

        let bars = self.spectrum.len().max(1);
        let gap = 3.0;
        let bar_width = (rect.width() - gap * (bars - 1) as f32) / bars as f32;
        let baseline = rect.bottom() - 12.0;
        let max_height = rect.height() - 28.0;
        for (index, value) in self.spectrum.iter().enumerate() {
            let height = (value * max_height).max(2.0);
            let x = rect.left() + index as f32 * (bar_width + gap);
            let bar = Rect::from_min_max(pos2(x, baseline - height), pos2(x + bar_width, baseline));
            theme::gradient_rect(
                ui.painter(),
                bar,
                bar_width * 0.35,
                theme.accent_a,
                theme.accent_b,
                90.0,
            );
        }

        ui::status_dot(
            ui,
            pos2(rect.left() + 6.0, rect.top() + 8.0),
            if self.is_playing() { "LIVE" } else { "PAUSED" },
            self.is_playing(),
            &theme,
        );
        let peak = self
            .spectrum
            .iter()
            .fold(0.0f32, |value, sample| value.max(*sample));
        ui.painter().text(
            pos2(rect.right(), rect.top() + 8.0),
            Align2::RIGHT_CENTER,
            format!("{:.0}%", peak * 100.0),
            FontId::proportional(9.5),
            theme.faint(),
        );
    }

    /* ---------- layout editor ---------- */

    fn layout_overlay(&mut self, ui: &mut Ui, _frame: Rect, content: Rect) {
        let theme = self.theme;
        let mut save = false;
        let mut hide: Option<usize> = None;
        let mut guide_x: Option<f32> = None;
        let mut guide_y: Option<f32> = None;

        let all_rects: Vec<(usize, Rect)> = PANEL_KINDS
            .iter()
            .filter(|kind| self.config.ui.panels[kind.index()].visible)
            .map(|kind| (kind.index(), self.panel_rect(content, *kind)))
            .collect();

        for kind in PANEL_KINDS {
            if !self.config.ui.panels[kind.index()].visible {
                continue;
            }
            let rect = self.panel_rect(content, kind);
            theme::dashed_rect(
                ui.painter(),
                rect,
                theme::with_alpha(theme.accent_b, 220),
                7.0,
            );
            ui.painter().rect_filled(
                rect,
                theme.radius,
                Color32::from_rgba_unmultiplied(4, 4, 8, 36),
            );
            let label_width = (rect.width() - 46.0).clamp(72.0, 156.0);
            let label_height = 22.0;
            let above_y = rect.top() - label_height - 4.0;
            let label_pos = if above_y >= content.top() {
                pos2(rect.left(), above_y)
            } else {
                pos2(rect.left() + 8.0, rect.top() + 8.0)
            };
            let label = Rect::from_min_size(label_pos, vec2(label_width, label_height));
            theme::paint_pill(
                ui.painter(),
                label,
                &theme,
                false,
                false,
                false,
                false,
                GlassSlot::None,
            );
            let label_font = FontId::proportional(10.0);
            ui.painter().text(
                pos2(label.left() + 9.0, label.center().y),
                Align2::LEFT_CENTER,
                ui::ellipsize(
                    ui.painter(),
                    kind.title(),
                    &label_font,
                    theme.text(),
                    (label.width() - 46.0).max(20.0),
                ),
                label_font,
                theme.text(),
            );
            ui.painter().text(
                pos2(label.right() - 8.0, label.center().y),
                Align2::RIGHT_CENTER,
                format!("{:.0}×{:.0}", rect.width(), rect.height()),
                FontId::proportional(8.5),
                theme.muted(),
            );

            let others: Vec<Rect> = all_rects
                .iter()
                .filter(|(index, _)| *index != kind.index())
                .map(|(_, rect)| *rect)
                .collect();

            let move_response = ui
                .interact(
                    rect,
                    Id::unique(("layout-move", kind.index())),
                    Sense::click_and_drag(),
                )
                .on_hover_cursor(CursorIcon::Grab);
            if move_response.dragged() {
                let delta = move_response.drag_delta();
                let proposed = Rect::from_min_size(rect.min + delta, rect.size());
                let (snapped, gx, gy) = snap_layout_move(proposed, content, &others);
                let panel = &mut self.config.ui.panels[kind.index()];
                panel.x = (snapped.left() - content.left()) / content.width();
                panel.y = (snapped.top() - content.top()) / content.height();
                panel.w = snapped.width() / content.width();
                panel.h = snapped.height() / content.height();
                guide_x = gx;
                guide_y = gy;
            }
            if move_response.drag_stopped() {
                save = true;
            }

            let handle = Rect::from_center_size(
                pos2(rect.right() - 13.0, rect.bottom() - 13.0),
                vec2(26.0, 26.0),
            );
            let resize_response = ui
                .interact(
                    handle,
                    Id::unique(("layout-resize", kind.index())),
                    Sense::click_and_drag(),
                )
                .on_hover_cursor(CursorIcon::ResizeNwSe);
            ui.painter()
                .circle_filled(handle.center(), 8.0, theme.accent_b);
            ui.painter()
                .circle_stroke(handle.center(), 8.0, Stroke::new(1.0_f32, theme.text()));
            if resize_response.dragged() {
                let delta = resize_response.drag_delta();
                let proposed = Rect::from_min_size(
                    rect.min,
                    vec2(
                        (rect.width() + delta.x).max(80.0),
                        (rect.height() + delta.y).max(60.0),
                    ),
                );
                let (snapped, gx, gy) =
                    snap_layout_resize(proposed, content, &others, 150.0, 110.0);
                let panel = &mut self.config.ui.panels[kind.index()];
                panel.w = snapped.width() / content.width();
                panel.h = snapped.height() / content.height();
                guide_x = gx;
                guide_y = gy;
            }
            if resize_response.drag_stopped() {
                save = true;
            }

            let delete_rect = Rect::from_center_size(
                pos2(rect.right() - 16.0, rect.top() + 16.0),
                vec2(22.0, 22.0),
            );
            let delete_response = ui
                .interact(
                    delete_rect,
                    Id::unique(("layout-delete", kind.index())),
                    Sense::click(),
                )
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text("Hide this component");
            let delete_color = if delete_response.hovered() {
                theme::lerp_color(theme.danger(), Color32::WHITE, 0.18)
            } else {
                theme.danger()
            };
            ui.painter()
                .circle_filled(delete_rect.center(), 9.0, delete_color);
            ui::paint_glyph(
                ui.painter(),
                delete_rect.center(),
                11.0,
                Glyph::Close,
                Color32::WHITE,
            );
            if delete_response.clicked() {
                hide = Some(kind.index());
            }
        }

        if let Some(x) = guide_x {
            ui.painter().line_segment(
                [pos2(x, content.top()), pos2(x, content.bottom())],
                Stroke::new(1.0_f32, theme::with_alpha(theme.accent_b, 150)),
            );
        }
        if let Some(y) = guide_y {
            ui.painter().line_segment(
                [pos2(content.left(), y), pos2(content.right(), y)],
                Stroke::new(1.0_f32, theme::with_alpha(theme.accent_b, 150)),
            );
        }

        if let Some(index) = hide {
            self.config.ui.panels[index].visible = false;
            save = true;
        }
        if save {
            self.save_config();
        }
    }

    /* ---------- overlays ---------- */

    fn status_toast(&mut self, ui: &mut Ui, frame: Rect) {
        let Some(status) = self.status.clone() else {
            return;
        };
        let Some(started) = self.status_at else {
            return;
        };
        if started.elapsed() > Duration::from_secs(6) {
            self.status = None;
            return;
        }

        let theme = self.theme;
        let font = FontId::proportional(11.5);
        let text_width = ui
            .painter()
            .layout_no_wrap(status.clone(), font.clone(), theme.text())
            .size()
            .x;
        let width = (text_width + 34.0).min(frame.width() - 40.0);
        let rect = Rect::from_min_size(
            pos2(frame.right() - width - 18.0, frame.bottom() - 42.0),
            vec2(width, 28.0),
        );
        theme::paint_pill(
            ui.painter(),
            rect,
            &theme,
            false,
            false,
            false,
            false,
            GlassSlot::None,
        );
        let color = if self.status_error {
            theme.danger()
        } else {
            theme.text()
        };
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            ui::ellipsize(ui.painter(), &status, &font, color, rect.width() - 20.0),
            font,
            color,
        );
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }

    fn add_menu(&mut self, ctx: &egui::Context) {
        if !self.add_menu_open {
            return;
        }
        let theme = self.theme;
        let screen = ctx.content_rect();

        let mut action: Option<AddMenuAction> = None;
        let layout_edit = self.layout_edit;
        let hidden: Vec<PanelKind> = PANEL_KINDS
            .iter()
            .copied()
            .filter(|kind| !self.config.ui.panels[kind.index()].visible)
            .collect();
        egui::Area::new(Id::unique("add-menu-overlay"))
            .order(egui::Order::Foreground)
            .fade_in(false)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (backdrop, _) = ui.allocate_exact_size(screen.size(), Sense::hover());
                ui.painter().rect_filled(
                    backdrop,
                    0.0,
                    Color32::from_rgba_unmultiplied(0, 0, 0, 24),
                );

                let mut menu_ui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_size(self.add_menu_pos, vec2(260.0, 220.0)))
                        .id_salt("add-menu-card"),
                );
                let menu_glass = crate::glass::enabled(&theme, GlassSlot::Menu);
                let menu = egui::Frame::NONE
                    .fill(crate::glass::menu_fill(&theme).unwrap_or_else(|| theme.panel_top()))
                    .stroke(if menu_glass {
                        Stroke::NONE
                    } else {
                        Stroke::new(1.0_f32, theme.accent_line())
                    })
                    .corner_radius(CornerRadius::from(12.0))
                    .inner_margin(egui::Margin::symmetric(6, 6))
                    .show(&mut menu_ui, |ui| {
                        ui.set_min_width(228.0);
                        if layout_edit {
                            ui.label(
                                egui::RichText::new("ADD COMPONENT")
                                    .size(9.5)
                                    .color(theme.faint()),
                            );
                            if hidden.is_empty() {
                                ui.label(
                                    egui::RichText::new("All components are visible.")
                                        .size(11.0)
                                        .color(theme.muted()),
                                );
                                ui.label(
                                    egui::RichText::new(
                                        "Hide one with the red button, then it comes back here.",
                                    )
                                    .size(9.5)
                                    .color(theme.faint()),
                                );
                            } else {
                                for kind in &hidden {
                                    if ui::menu_item(
                                        ui,
                                        &format!("Add {}", kind.title().to_lowercase()),
                                        Glyph::Plus,
                                        &theme,
                                    )
                                    .clicked()
                                    {
                                        action = Some(AddMenuAction::Restore(*kind));
                                    }
                                }
                            }
                        } else {
                            if ui::menu_item(ui, "Add music files", Glyph::Music, &theme).clicked()
                            {
                                action = Some(AddMenuAction::Files);
                            }
                            if ui::menu_item(ui, "Add a folder", Glyph::Folder, &theme).clicked() {
                                action = Some(AddMenuAction::Folder);
                            }
                            if ui::menu_item(ui, "Download from a link", Glyph::Download, &theme)
                                .clicked()
                            {
                                action = Some(AddMenuAction::Link);
                            }
                            if ui::menu_item(ui, "Open downloader", Glyph::Sliders, &theme)
                                .clicked()
                            {
                                action = Some(AddMenuAction::Downloader);
                            }
                        }
                    });
                crate::glass::paint_menu_rim(
                    menu_ui.painter(),
                    menu.response.rect,
                    12.0,
                    &theme,
                );
                self.add_menu_rect = menu.response.rect;

                let outside_click = ui.ctx().input(|input| input.pointer.any_click())
                    && ui
                        .ctx()
                        .input(|input| input.pointer.interact_pos())
                        .map(|pos| !self.add_menu_rect.contains(pos))
                        .unwrap_or(false);
                if outside_click {
                    self.add_menu_open = false;
                }
            });

        match action {
            Some(AddMenuAction::Files) => {
                self.add_menu_open = false;
                self.pick_files();
            }
            Some(AddMenuAction::Folder) => {
                self.add_menu_open = false;
                self.pick_folder();
            }
            Some(AddMenuAction::Link) => {
                self.add_menu_open = false;
                self.download_focus = true;
                self.notice("Paste a YouTube or Spotify link in the downloader.", false);
            }
            Some(AddMenuAction::Downloader) => {
                self.add_menu_open = false;
                self.download_focus = true;
            }
            Some(AddMenuAction::Restore(kind)) => {
                self.add_menu_open = false;
                self.config.ui.panels[kind.index()].visible = true;
                self.save_config();
            }
            None => {}
        }
    }

    fn search_overlay(&mut self, ctx: &egui::Context) {
        if !self.search_active && self.search_query.trim().is_empty() {
            return;
        }
        let Some(search_rect) = self.search_rect else {
            return;
        };
        let query = self.search_query.trim().to_lowercase();
        if query.is_empty() {
            return;
        }

        let theme = self.theme;
        let track_hits: Vec<usize> = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| {
                track.title.to_lowercase().contains(&query)
                    || track.artist.to_lowercase().contains(&query)
                    || track.album.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .take(6)
            .collect();
        let playlist_hits: Vec<(String, usize)> = self
            .playlists()
            .into_iter()
            .filter(|(name, _)| name.to_lowercase().contains(&query))
            .take(5)
            .collect();

        let width = search_rect.width().max(360.0);
        let height = if track_hits.is_empty() && playlist_hits.is_empty() {
            70.0
        } else {
            (36.0
                + track_hits.len().min(6) as f32 * 38.0
                + if playlist_hits.is_empty() { 0.0 } else { 24.0 }
                + playlist_hits.len() as f32 * 34.0)
                .min(390.0)
        };
        let popup = Rect::from_min_size(
            search_rect.left_bottom() + vec2(0.0, 8.0),
            vec2(width, height),
        );
        self.search_popup_rect = popup;

        let mut action: Option<(bool, usize)> = None;
        let area = egui::Area::new(Id::unique("search-results"))
            .order(egui::Order::Foreground)
            .fade_in(false)
            .fixed_pos(popup.min)
            .show(ctx, |ui| {
                let popup_frame = egui::Frame::NONE
                    .fill(crate::glass::menu_fill(&theme).unwrap_or_else(|| theme.panel_top()))
                    .stroke(if crate::glass::enabled(&theme, GlassSlot::Menu) {
                        Stroke::NONE
                    } else {
                        Stroke::new(1.0_f32, theme.accent_line())
                    })
                    .corner_radius(CornerRadius::from(12.0))
                    .inner_margin(egui::Margin::symmetric(8, 8))
                    .show(ui, |ui| {
                        ui.set_min_size(vec2(width - 16.0, height - 16.0));
                        if track_hits.is_empty() && playlist_hits.is_empty() {
                            ui.label(
                                egui::RichText::new("No music or playlist matches that search.")
                                    .size(11.5)
                                    .color(theme.muted()),
                            );
                            return;
                        }
                        if !track_hits.is_empty() {
                            ui.label(egui::RichText::new("MUSIC").size(9.5).color(theme.faint()));
                            for index in &track_hits {
                                let track = &self.tracks[*index];
                                let (row, response) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), 36.0),
                                    Sense::click(),
                                );
                                if !crate::glass::paint_row(
                                    ui.painter(),
                                    row.shrink2(vec2(1.0, 1.0)),
                                    &theme,
                                    response.hovered(),
                                    false,
                                ) && response.hovered()
                                {
                                    ui.painter().rect_filled(
                                        row,
                                        8.0,
                                        Color32::from_rgba_unmultiplied(255, 255, 255, 14),
                                    );
                                }
                                ui::paint_cover(
                                    ui,
                                    Rect::from_min_size(
                                        pos2(row.left() + 4.0, row.center().y - 13.0),
                                        vec2(26.0, 26.0),
                                    ),
                                    self.thumbs[*index].as_ref(),
                                    6.0,
                                    &theme,
                                );
                                ui.painter().text(
                                    pos2(row.left() + 40.0, row.center().y),
                                    Align2::LEFT_CENTER,
                                    ui::ellipsize(
                                        ui.painter(),
                                        &track.title,
                                        &FontId::proportional(12.0),
                                        theme.text(),
                                        row.width() - 130.0,
                                    ),
                                    FontId::proportional(12.0),
                                    theme.text(),
                                );
                                ui.painter().text(
                                    pos2(row.right() - 10.0, row.center().y),
                                    Align2::RIGHT_CENTER,
                                    ui::ellipsize(
                                        ui.painter(),
                                        &track.artist,
                                        &FontId::proportional(10.0),
                                        theme.muted(),
                                        100.0,
                                    ),
                                    FontId::proportional(10.0),
                                    theme.muted(),
                                );
                                if response.clicked() {
                                    action = Some((true, *index));
                                }
                            }
                        }
                        if !playlist_hits.is_empty() {
                            ui.label(
                                egui::RichText::new("PLAYLISTS")
                                    .size(9.5)
                                    .color(theme.faint()),
                            );
                            for (position, (name, count)) in playlist_hits.iter().enumerate() {
                                let (row, response) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), 32.0),
                                    Sense::click(),
                                );
                                if !crate::glass::paint_row(
                                    ui.painter(),
                                    row.shrink2(vec2(1.0, 1.0)),
                                    &theme,
                                    response.hovered(),
                                    false,
                                ) && response.hovered()
                                {
                                    ui.painter().rect_filled(
                                        row,
                                        8.0,
                                        Color32::from_rgba_unmultiplied(255, 255, 255, 14),
                                    );
                                }
                                ui.painter().text(
                                    pos2(row.left() + 8.0, row.center().y),
                                    Align2::LEFT_CENTER,
                                    ui::ellipsize(
                                        ui.painter(),
                                        name,
                                        &FontId::proportional(12.0),
                                        theme.text(),
                                        row.width() - 70.0,
                                    ),
                                    FontId::proportional(12.0),
                                    theme.text(),
                                );
                                ui.painter().text(
                                    pos2(row.right() - 8.0, row.center().y),
                                    Align2::RIGHT_CENTER,
                                    format!("{count}"),
                                    FontId::proportional(10.0),
                                    theme.faint(),
                                );
                                if response.clicked() {
                                    action = Some((false, position));
                                }
                            }
                        }
                    });
                crate::glass::paint_menu_rim(
                    ui.painter(),
                    popup_frame.response.rect,
                    12.0,
                    &theme,
                );
            });
        self.search_popup_rect = area.response.rect;

        if action.is_none() && ctx.input(|input| input.key_pressed(egui::Key::Enter)) {
            if let Some(index) = track_hits.first() {
                action = Some((true, *index));
            }
        }

        if let Some((is_track, index)) = action {
            if is_track {
                self.active_playlist = self.tracks[index].playlist.clone();
                self.rebuild_visible();
                self.play_index(index);
            } else if let Some((name, _)) = playlist_hits.get(index) {
                self.active_playlist = Some(name.clone());
                self.rebuild_visible();
            }
            self.search_active = false;
            self.search_query.clear();
        }

        if ctx.input(|input| input.pointer.any_click()) {
            if let Some(pos) = ctx.input(|input| input.pointer.interact_pos()) {
                let inside_search = self.search_rect.map(|r| r.contains(pos)).unwrap_or(false);
                if !inside_search && !self.search_popup_rect.contains(pos) {
                    self.search_active = false;
                }
            }
        }
    }

    /* ---------- settings ---------- */

    fn settings_modal(&mut self, ctx: &egui::Context) {
        let theme = self.theme;
        let screen = ctx.content_rect();
        let width = (screen.width() * 0.82).min(780.0);
        let height = (screen.height() * 0.86).min(650.0);
        let modal = Rect::from_center_size(screen.center(), vec2(width, height));

        let mut close = false;
        let mut action: Option<SettingsAction> = None;

        egui::Area::new(Id::unique("settings-overlay"))
            .order(egui::Order::Foreground)
            .fade_in(false)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (_backdrop, _) = ui.allocate_exact_size(screen.size(), Sense::hover());

                let mut modal_ui =
                    ui.new_child(UiBuilder::new().max_rect(modal).id_salt("settings-card"));
                modal_ui.set_clip_rect(modal.expand(4.0));
                // A glass modal sits over a busy dashboard; keep its base dark
                // enough that the controls stay legible through the face. The
                // real wgpu pass supplies its own face instead.
                if !(theme.foreground_only && theme.real_glass) {
                    modal_ui.painter().rect_filled(
                        modal,
                        20.0,
                        Color32::from_rgba_unmultiplied(8, 8, 12, 240),
                    );
                }
                theme::paint_card(modal_ui.painter(), modal, 20.0, &theme, false, false, 4, true);
                let modal_border = if theme.panel_border_visible {
                    theme.panel_border_width.max(0.0)
                } else {
                    0.0
                };
                let inner = modal.shrink2(vec2(22.0 + modal_border, 16.0 + modal_border));
                let mut child = modal_ui.new_child(
                    UiBuilder::new()
                        .max_rect(inner)
                        .layout(Layout::top_down(Align::Min))
                        .id_salt("settings-content"),
                );
                child.set_clip_rect(inner);
                child.spacing_mut().item_spacing = vec2(8.0, 8.0);

                child.label(
                    egui::RichText::new("SETTINGS")
                        .size(11.0)
                        .color(theme.muted())
                        .strong(),
                );
                let close_rect = Rect::from_center_size(
                    pos2(inner.right() - 2.0, inner.top() + 6.0),
                    vec2(24.0, 24.0),
                );
                let close_response =
                    child.interact(close_rect, Id::unique("settings-close"), Sense::click());
                theme::paint_icon_button(
                    child.painter(),
                    close_rect,
                    &theme,
                    close_response.hovered(),
                    false,
                    GlassSlot::SettingsControl,
                );
                ui::paint_glyph(
                    child.painter(),
                    close_rect.center(),
                    12.0,
                    Glyph::Close,
                    theme.text(),
                );
                if close_response.clicked() {
                    close = true;
                }

                child.horizontal_wrapped(|ui| {
                    for (tab, label) in [
                        (SettingsTab::Appearance, "APPEARANCE"),
                        (SettingsTab::Glass, "GLASS"),
                        (SettingsTab::Layout, "LAYOUT"),
                        (SettingsTab::Library, "LIBRARY"),
                        (SettingsTab::Downloads, "DOWNLOADS"),
                        (SettingsTab::About, "ABOUT"),
                    ] {
                        if ui::chip_button(
                            ui,
                            label,
                            self.settings_tab == tab,
                            label.len() as f32 * 6.2 + 18.0,
                            &theme,
                            GlassSlot::SettingsControl,
                        )
                        .clicked()
                        {
                            self.settings_tab = tab;
                        }
                    }
                });

                child.add_space(2.0);
                let body_height = child.available_height();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(body_height)
                    .id_salt(("settings-body", self.settings_tab.key()))
                    .show(&mut child, |ui| {
                        ui.set_min_width(ui.available_width());
                        match self.settings_tab {
                            SettingsTab::Appearance => {
                                self.appearance_settings(ui, &theme);
                            }
                            SettingsTab::Glass => {
                                self.glass_settings(ui, &theme);
                            }
                            SettingsTab::Layout => {
                                self.layout_settings(ui, &theme, &mut action);
                            }
                            SettingsTab::Library => {
                                self.library_settings(ui, &theme, &mut action);
                            }
                            SettingsTab::Downloads => {
                                self.download_settings(ui, &theme);
                            }
                            SettingsTab::About => {
                                self.about_settings(ui, &theme);
                            }
                        }
                    });
            });

        if close {
            self.settings_open = false;
        }

        match action {
            Some(SettingsAction::ChooseFolder) => self.choose_library_folder(),
            Some(SettingsAction::AddFiles) => self.pick_files(),
            Some(SettingsAction::AddFolder) => self.pick_folder(),
            Some(SettingsAction::EditLayout) => {
                self.settings_open = false;
                self.layout_edit = true;
            }
            Some(SettingsAction::ResetLayout) => {
                self.config.ui.panels = UiConfig::default().panels;
                self.save_config();
            }
            None => {}
        }
    }

    fn appearance_settings(&mut self, ui: &mut Ui, theme: &Theme) {
        Self::settings_heading(
            ui,
            theme,
            "LIQUID GLASS",
            "Apple-style glass for the selected components. The GLASS tab exposes every material, tone, shape and accessibility setting.",
        );
        let mut glass_enabled = self.config.ui.liquid.enabled;
        let mut open_glass = false;
        ui.horizontal(|ui| {
            if ui.checkbox(&mut glass_enabled, "Enable Liquid Glass mode").changed() {
                self.config.ui.liquid.enabled = glass_enabled;
                self.refresh_theme(ui.ctx());
                self.save_config();
            }
            if ui::chip_button(
                ui,
                "OPEN GLASS SETTINGS",
                false,
                158.0,
                theme,
                GlassSlot::SettingsControl,
            )
            .clicked()
            {
                open_glass = true;
            }
        });
        if open_glass {
            self.settings_tab = SettingsTab::Glass;
        }

        Self::settings_heading(
            ui,
            theme,
            "THEME PRESETS",
            "Presets change the gradient and the frame colour. Your layout stays untouched.",
        );
        let mut preset = None;
        ui.horizontal_wrapped(|ui| {
            for (name, a, b, angle, frame, bg) in [
                (
                    "AURORA",
                    [124u8, 92, 255],
                    [34u8, 211, 238],
                    132.0,
                    [18u8, 16, 26],
                    [12u8, 12, 19],
                ),
                (
                    "SUNSET",
                    [255, 122, 89],
                    [177, 75, 255],
                    120.0,
                    [24, 13, 14],
                    [22, 12, 16],
                ),
                (
                    "OCEAN",
                    [46, 196, 255],
                    [54, 255, 196],
                    150.0,
                    [8, 18, 24],
                    [8, 18, 24],
                ),
                (
                    "EMBER",
                    [255, 176, 32],
                    [255, 70, 84],
                    115.0,
                    [24, 15, 9],
                    [22, 13, 8],
                ),
                (
                    "MONO",
                    [210, 214, 225],
                    [118, 124, 140],
                    135.0,
                    [16, 16, 18],
                    [15, 15, 18],
                ),
                (
                    "NEON",
                    [255, 46, 214],
                    [42, 255, 224],
                    118.0,
                    [13, 8, 22],
                    [13, 8, 20],
                ),
                (
                    "FOREST",
                    [70, 220, 132],
                    [30, 168, 220],
                    145.0,
                    [8, 19, 14],
                    [8, 16, 12],
                ),
                (
                    "ROYAL",
                    [92, 92, 255],
                    [196, 82, 255],
                    128.0,
                    [12, 10, 28],
                    [12, 10, 26],
                ),
            ] {
                if ui::chip_button(
                    ui,
                    name,
                    false,
                    64.0,
                    theme,
                    GlassSlot::SettingsControl,
                )
                .clicked()
                {
                    preset = Some((a, b, angle, frame, bg));
                }
            }
        });
        if let Some((a, b, angle, frame, bg)) = preset {
            self.config.ui.accent_a = a;
            self.config.ui.accent_b = b;
            self.config.ui.angle = angle;
            self.config.ui.frame_color = frame;
            self.config.ui.background_color = bg;
            self.refresh_theme(ui.ctx());
            self.save_config();
        }

        Self::settings_heading(
            ui,
            theme,
            "GRADIENT",
            "The black canvas stays as the base; panel glass and accents are mixed from these stops.",
        );
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Start").size(11.0).color(theme.muted()));
            let changed = ui
                .color_edit_button_srgb(&mut self.config.ui.accent_a)
                .changed();
            ui.label(egui::RichText::new("End").size(11.0).color(theme.muted()));
            let changed = ui
                .color_edit_button_srgb(&mut self.config.ui.accent_b)
                .changed()
                || changed;
            if changed {
                self.refresh_theme(ui.ctx());
                self.save_config();
            }
        });
        let mut angle = self.config.ui.angle;
        if ui
            .add(egui::Slider::new(&mut angle, 0.0..=360.0).text("Angle"))
            .changed()
        {
            self.config.ui.angle = angle;
            self.refresh_theme(ui.ctx());
            self.save_config();
        }
        let mut glass = self.config.ui.glass;
        if ui
            .add(egui::Slider::new(&mut glass, 0.0..=1.0).text("Accent strength"))
            .changed()
        {
            self.config.ui.glass = glass;
            self.refresh_theme(ui.ctx());
            self.save_config();
        }
        let mut glow = self.config.ui.glow;
        if ui
            .add(egui::Slider::new(&mut glow, 0.0..=1.0).text("Glow"))
            .changed()
        {
            self.config.ui.glow = glow;
            self.refresh_theme(ui.ctx());
            self.save_config();
        }
        let mut radius = self.config.ui.radius;
        if ui
            .add(egui::Slider::new(&mut radius, 8.0..=42.0).text("Corner radius"))
            .changed()
        {
            self.config.ui.radius = radius;
            self.refresh_theme(ui.ctx());
            self.save_config();
        }

        Self::settings_heading(
            ui,
            theme,
            "OUTER FRAME & INNER DASHBOARD",
            "The frame colour fills the area near the screen edge. The inner dashboard keeps its own surface and hosts the photo.",
        );
        let mut frame_changed = ui
            .checkbox(
                &mut self.config.ui.frame_visible,
                "Show the inner dashboard",
            )
            .changed();
        frame_changed |= ui
            .checkbox(
                &mut self.config.ui.frame_border_visible,
                "Show the inner frame border",
            )
            .changed();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Outer frame colour")
                    .size(11.0)
                    .color(theme.muted()),
            );
            frame_changed |= ui
                .color_edit_button_srgb(&mut self.config.ui.frame_color)
                .changed();
        });
        let mut frame_opacity = self.config.ui.frame_opacity;
        if ui
            .add(egui::Slider::new(&mut frame_opacity, 0.0..=1.0).text("Outer frame opacity"))
            .changed()
        {
            self.config.ui.frame_opacity = frame_opacity;
            frame_changed = true;
        }
        let mut frame_border = self.config.ui.frame_border_width;
        if ui
            .add(egui::Slider::new(&mut frame_border, 0.0..=6.0).text("Inner frame border width"))
            .changed()
        {
            self.config.ui.frame_border_width = frame_border;
            frame_changed = true;
        }
        frame_changed |= ui
            .checkbox(
                &mut self.config.ui.panel_border_visible,
                "Show component borders",
            )
            .changed();
        let mut panel_border = self.config.ui.panel_border_width;
        if ui
            .add(egui::Slider::new(&mut panel_border, 0.0..=6.0).text("Component border width"))
            .changed()
        {
            self.config.ui.panel_border_width = panel_border;
            frame_changed = true;
        }
        if frame_changed {
            self.refresh_theme(ui.ctx());
            self.save_config();
        }

        Self::settings_heading(
            ui,
            theme,
            "INNER BACKGROUND",
            "Pick a colour, an optional photo on top of it, or both. This fills the inner dashboard only.",
        );
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Background colour")
                    .size(11.0)
                    .color(theme.muted()),
            );
            if ui
                .color_edit_button_srgb(&mut self.config.ui.background_color)
                .changed()
            {
                self.refresh_theme(ui.ctx());
                self.save_config();
            }
        });
        ui.horizontal(|ui| {
            if ui::chip_button(
                ui,
                "CHOOSE PHOTO",
                false,
                112.0,
                theme,
                GlassSlot::SettingsControl,
            )
            .clicked()
            {
                self.choose_background();
            }
            if ui::chip_button(
                ui,
                "CLEAR",
                false,
                60.0,
                theme,
                GlassSlot::SettingsControl,
            )
            .clicked()
            {
                self.config.ui.background_image = None;
                self.background_loaded = None;
                self.background_tex = None;
                self.save_config();
            }
        });
        if self.config.ui.background_image.is_some() {
            ui.label(
                egui::RichText::new(
                    "The photo is drawn on top of the background colour. Clear it to use the colour alone.",
                )
                .size(9.5)
                .color(theme.faint()),
            );
        }
        let label = self
            .config
            .ui
            .background_image
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "No background photo selected".to_string());
        ui.add(
            egui::Label::new(egui::RichText::new(label).size(10.0).color(theme.faint())).truncate(),
        );
        if let Some(texture) = &self.background_tex {
            let (thumb, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::hover());
            egui::Image::new(egui::load::SizedTexture::from_handle(texture))
                .corner_radius(8.0)
                .uv(theme::texture_uv(texture.size_vec2(), thumb.size()))
                .paint_at(ui, thumb);
        }
        let mut dim = self.config.ui.background_dim;
        if ui
            .add(egui::Slider::new(&mut dim, 0.0..=1.0).text("Photo dim"))
            .changed()
        {
            self.config.ui.background_dim = dim;
            self.refresh_theme(ui.ctx());
            self.save_config();
        }

        ui.add_space(8.0);
        if ui::chip_button(
            ui,
            "RESTORE APPEARANCE DEFAULTS",
            false,
            210.0,
            theme,
            GlassSlot::SettingsControl,
        )
        .clicked()
        {
            let defaults = UiConfig::default();
            self.config.ui.accent_a = defaults.accent_a;
            self.config.ui.accent_b = defaults.accent_b;
            self.config.ui.angle = defaults.angle;
            self.config.ui.glass = defaults.glass;
            self.config.ui.glow = defaults.glow;
            self.config.ui.radius = defaults.radius;
            self.config.ui.frame_color = defaults.frame_color;
            self.config.ui.frame_visible = defaults.frame_visible;
            self.config.ui.frame_opacity = defaults.frame_opacity;
            self.config.ui.frame_border_visible = defaults.frame_border_visible;
            self.config.ui.frame_border_width = defaults.frame_border_width;
            self.config.ui.panel_border_visible = defaults.panel_border_visible;
            self.config.ui.panel_border_width = defaults.panel_border_width;
            self.config.ui.background_color = defaults.background_color;
            self.config.ui.background_image = None;
            self.config.ui.background_dim = defaults.background_dim;
            self.background_loaded = None;
            self.background_tex = None;
            self.refresh_theme(ui.ctx());
            self.save_config();
        }

        Self::settings_heading(
            ui,
            theme,
            "PREVIEW",
            "A quick look at the current component styling.",
        );
        let (preview, _) = ui.allocate_exact_size(vec2(ui.available_width(), 94.0), Sense::hover());
        theme::paint_card(
            ui.painter(),
            preview,
            self.theme.radius,
            &self.theme,
            false,
            false,
            5,
            true,
        );
        let inner = preview.shrink2(vec2(16.0, 14.0));
        theme::gradient_rect(
            ui.painter(),
            Rect::from_min_size(inner.min, vec2(52.0, 52.0)),
            10.0,
            self.theme.accent_a,
            self.theme.accent_b,
            self.theme.angle,
        );
        ui.painter().text(
            pos2(inner.left() + 66.0, inner.top() + 12.0),
            Align2::LEFT_TOP,
            "Sample track",
            FontId::proportional(14.0),
            self.theme.text(),
        );
        ui.painter().text(
            pos2(inner.left() + 66.0, inner.top() + 31.0),
            Align2::LEFT_TOP,
            "Sample artist — album",
            FontId::proportional(10.5),
            self.theme.muted(),
        );
        let pill = Rect::from_min_size(
            pos2(inner.left() + 66.0, inner.top() + 48.0),
            vec2(92.0, 24.0),
        );
        theme::paint_pill(
            ui.painter(),
            pill,
            &self.theme,
            false,
            false,
            false,
            true,
            GlassSlot::SettingsControl,
        );
        ui.painter().text(
            pill.center(),
            Align2::CENTER_CENTER,
            "PLAY",
            FontId::proportional(9.0),
            self.theme.on_accent(),
        );
    }

    fn glass_settings(&mut self, ui: &mut Ui, theme: &Theme) {
        let mut glass = self.config.ui.liquid;
        let mut changed = false;

        Self::settings_heading(
            ui,
            theme,
            "LIQUID GLASS",
            "All knobs from liquid-rust's Material, Tone, Shadow and Appearance models. Values resolve exactly like the library: size adaptation, the iOS 27 transparency slider, Reduce Transparency and Increase Contrast.",
        );
        changed |= ui
            .checkbox(&mut glass.enabled, "Enable Liquid Glass mode")
            .changed();

        ui.horizontal_wrapped(|ui| {
            for preset in [
                GlassPreset::Regular,
                GlassPreset::Clear,
                GlassPreset::ClearBar,
                GlassPreset::Tinted,
                GlassPreset::Identity,
            ] {
                if ui::chip_button(
                    ui,
                    preset.label(),
                    false,
                    76.0,
                    theme,
                    GlassSlot::SettingsControl,
                )
                .clicked()
                {
                    glass.apply_preset(preset);
                    changed = true;
                }
            }
        });
        ui.label(
            egui::RichText::new(
                "Presets change only the material. Targets, appearance, shape and scene stay as they are.",
            )
            .size(9.5)
            .color(theme.faint()),
        );

        egui::CollapsingHeader::new("TARGETS")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Choose which components receive the glass surface. Slider mode paints only the ball.",
                    )
                    .size(9.5)
                    .color(theme.faint()),
                );
                glass_check(ui, "Top bar (+, settings, search pill)", &mut glass.targets.top_bar, &mut changed);
                glass_check(ui, "Transport (prev / play / next / mute)", &mut glass.targets.transport, &mut changed);
                glass_check(ui, "Player chips (shuffle / repeat)", &mut glass.targets.player_chips, &mut changed);
                glass_check(ui, "Downloader actions", &mut glass.targets.downloader_actions, &mut changed);
                glass_check(ui, "Option chips (quality, parallel, EQ)", &mut glass.targets.option_chips, &mut changed);
                glass_check(ui, "Menus and popups", &mut glass.targets.menus, &mut changed);
                glass_check(ui, "Lists and cards", &mut glass.targets.lists, &mut changed);
                glass_check(ui, "Slider balls only", &mut glass.targets.sliders, &mut changed);
                glass_check(ui, "Panels and modals", &mut glass.targets.panels, &mut changed);
                glass_check(ui, "Settings controls", &mut glass.targets.settings_controls, &mut changed);
                glass_check(ui, "Setup screen", &mut glass.targets.setup, &mut changed);
            });

        egui::CollapsingHeader::new("MATERIAL")
            .default_open(true)
            .show(ui, |ui| {
                glass_slider(ui, "Refraction (bevel strength)", &mut glass.refraction, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Depth (bevel width, pt)", &mut glass.depth, 0.0..=80.0, &mut changed);
                glass_slider(ui, "Dispersion (chromatic split)", &mut glass.dispersion, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Frost (blur radius, pt)", &mut glass.frost, 0.0..=60.0, &mut changed);
                glass_slider(ui, "Splay (rim spread)", &mut glass.splay, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Specular (rim light)", &mut glass.specular, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Rim (bright inner edge)", &mut glass.rim, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Rim shade (soft inner shade)", &mut glass.rim_shade, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Hairline (1.25 pt chrome)", &mut glass.hairline, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Edge (dark outer line)", &mut glass.edge, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Dim (local backdrop dimming)", &mut glass.dim, 0.0..=1.0, &mut changed);
                glass_check(ui, "Adaptive (large glass gets thicker)", &mut glass.adaptive, &mut changed);
                ui.separator();
                glass_check(ui, "Tint (stained glass)", &mut glass.tint_enabled, &mut changed);
                if glass.tint_enabled {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Tint colour")
                                .size(11.0)
                                .color(theme.muted()),
                        );
                        changed |= ui
                            .color_edit_button_srgb(&mut glass.tint_color)
                            .changed();
                    });
                    glass_slider(ui, "Tint strength", &mut glass.tint_strength, 0.0..=1.0, &mut changed);
                }
            });

        egui::CollapsingHeader::new("SHADOW")
            .default_open(false)
            .show(ui, |ui| {
                glass_slider(ui, "Offset Y (pt)", &mut glass.shadow.offset_y, -30.0..=40.0, &mut changed);
                glass_slider(ui, "Blur (pt)", &mut glass.shadow.blur, 0.0..=100.0, &mut changed);
                glass_slider(ui, "Opacity", &mut glass.shadow.opacity, 0.0..=1.0, &mut changed);
            });

        egui::CollapsingHeader::new("LIGHT FACE TONE")
            .default_open(false)
            .show(ui, |ui| {
                glass_slider(ui, "Lift (LINEAR_DODGE)", &mut glass.light_tone.lift, 0.0..=0.5, &mut changed);
                glass_slider(ui, "Luminance target", &mut glass.light_tone.luminance, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Luminance mix", &mut glass.light_tone.luminance_mix, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Lighten", &mut glass.light_tone.lighten, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Darken to", &mut glass.light_tone.darken_to, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Darken", &mut glass.light_tone.darken, 0.0..=1.0, &mut changed);
            });

        egui::CollapsingHeader::new("DARK FACE TONE")
            .default_open(false)
            .show(ui, |ui| {
                glass_slider(ui, "Lift (LINEAR_DODGE)", &mut glass.dark_tone.lift, 0.0..=0.5, &mut changed);
                glass_slider(ui, "Luminance target", &mut glass.dark_tone.luminance, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Luminance mix", &mut glass.dark_tone.luminance_mix, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Lighten", &mut glass.dark_tone.lighten, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Darken to", &mut glass.dark_tone.darken_to, 0.0..=1.0, &mut changed);
                glass_slider(ui, "Darken", &mut glass.dark_tone.darken, 0.0..=1.0, &mut changed);
            });

        egui::CollapsingHeader::new("APPEARANCE")
            .default_open(true)
            .show(ui, |ui| {
                glass_check(ui, "Dark colour scheme", &mut glass.dark, &mut changed);
                glass_slider(ui, "Transparency (0 clear · 0.5 default · 1 tinted)", &mut glass.transparency, 0.0..=1.0, &mut changed);
                glass_check(ui, "Reduce Transparency", &mut glass.reduce_transparency, &mut changed);
                glass_check(ui, "Increase Contrast", &mut glass.increase_contrast, &mut changed);
                glass_check(ui, "Reduce Motion", &mut glass.reduce_motion, &mut changed);
                glass_check(ui, "Adapt to content", &mut glass.adapt_to_content, &mut changed);
            });

        egui::CollapsingHeader::new("SCENE & SHAPE")
            .default_open(false)
            .show(ui, |ui| {
                glass_check(ui, "Physics (hover and press motion)", &mut glass.physics, &mut changed);
                glass_check(ui, "Glass on glass", &mut glass.glass_on_glass, &mut changed);
                glass_slider(ui, "Light angle (0 = top, clockwise)", &mut glass.light_angle, 0.0..=360.0, &mut changed);
                glass_slider(ui, "Merge distance (reserved for grouped glass)", &mut glass.merge_spacing, 0.0..=60.0, &mut changed);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Button outline")
                            .size(11.0)
                            .color(theme.muted()),
                    );
                    let before = glass.shape;
                    egui::ComboBox::from_id_salt("glass-shape")
                        .selected_text(glass.shape.label())
                        .show_ui(ui, |ui| {
                            for shape in [
                                GlassShape::Capsule,
                                GlassShape::Rounded,
                                GlassShape::Circular,
                                GlassShape::Smooth,
                            ] {
                                ui.selectable_value(&mut glass.shape, shape, shape.label());
                            }
                        });
                    changed |= glass.shape != before;
                });
                glass_slider(ui, "Shape radius (Rounded / Smooth)", &mut glass.shape_radius, 0.0..=80.0, &mut changed);
                glass_slider(ui, "Smoothing (0 circular · 0.6 iOS)", &mut glass.smoothing, 0.0..=1.0, &mut changed);
                ui.label(
                    egui::RichText::new(
                        "The outline applies to buttons, chips and icon buttons. Panels follow the Appearance corner radius.",
                    )
                    .size(9.5)
                    .color(theme.faint()),
                );
            });

        ui.add_space(8.0);
        if ui::chip_button(
            ui,
            "RESTORE LIQUID GLASS DEFAULTS",
            false,
            232.0,
            theme,
            GlassSlot::SettingsControl,
        )
        .clicked()
        {
            glass = crate::config::GlassConfig::default();
            changed = true;
        }

        if changed {
            self.config.ui.liquid = glass;
            self.refresh_theme(ui.ctx());
            self.save_config();
        }
    }

    fn layout_settings(&mut self, ui: &mut Ui, theme: &Theme, action: &mut Option<SettingsAction>) {
        ui.label(
            egui::RichText::new("Components")
                .size(11.0)
                .color(theme.muted())
                .strong(),
        );
        ui.label(
            egui::RichText::new(
                "Panels snap to each other, to the frame and to the 8 px grid, and they stay inside the safe area. In layout mode the red corner buttons hide a panel; the top-left + brings hidden ones back.",
            )
            .size(10.5)
            .color(theme.faint()),
        );
        ui.add_space(4.0);

        for kind in PANEL_KINDS {
            let panel = &mut self.config.ui.panels[kind.index()];
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
            ui.painter().text(
                pos2(row.left(), row.center().y),
                Align2::LEFT_CENTER,
                kind.title(),
                FontId::proportional(12.0),
                theme.text(),
            );
            ui.painter().text(
                pos2(row.left() + row.width() * 0.48, row.center().y),
                Align2::LEFT_CENTER,
                format!("{:.0}% × {:.0}%", panel.w * 100.0, panel.h * 100.0),
                FontId::proportional(9.5),
                theme.faint(),
            );
            let toggle =
                Rect::from_center_size(pos2(row.right() - 20.0, row.center().y), vec2(34.0, 18.0));
            let response = ui.interact(
                toggle,
                Id::unique(("panel-toggle", kind.index())),
                Sense::click(),
            );
            let t = if panel.visible { 1.0 } else { 0.0 };
            let knob_x = toggle.left() + 8.0 + (toggle.width() - 16.0) * t;
            theme::paint_pill(
                ui.painter(),
                toggle,
                theme,
                response.hovered(),
                false,
                panel.visible,
                false,
                GlassSlot::SettingsControl,
            );
            ui.painter()
                .circle_filled(pos2(knob_x, toggle.center().y), 6.0, theme.text());
            if response.clicked() {
                panel.visible = !panel.visible;
                self.save_config();
            }
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui::pill_button_sized(
                ui,
                150.0,
                "Edit layout",
                true,
                theme,
                GlassSlot::SettingsControl,
            )
            .clicked()
            {
                *action = Some(SettingsAction::EditLayout);
            }
            if ui::pill_button_sized(
                ui,
                120.0,
                "Reset layout",
                false,
                theme,
                GlassSlot::SettingsControl,
            )
            .clicked()
            {
                *action = Some(SettingsAction::ResetLayout);
            }
        });
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "Drag a panel to move it. Pull the bright dot in its bottom-right corner to resize. Measurements snap to the grid, sibling edges and the frame so everything stays aligned. Esc leaves layout mode.",
            )
            .size(10.0)
            .color(theme.faint()),
        );
    }

    fn library_settings(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        action: &mut Option<SettingsAction>,
    ) {
        ui.label(
            egui::RichText::new("Music folder")
                .size(11.0)
                .color(theme.muted())
                .strong(),
        );
        let (path_rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
        theme::gradient_rect(
            ui.painter(),
            path_rect,
            8.0,
            theme.panel_soft(),
            theme.panel_bottom(),
            theme.angle,
        );
        ui.painter().rect_stroke(
            path_rect,
            8.0,
            Stroke::new(1.0_f32, theme.line()),
            StrokeKind::Inside,
        );
        ui.painter().text(
            pos2(path_rect.left() + 10.0, path_rect.center().y),
            Align2::LEFT_CENTER,
            ui::ellipsize(
                ui.painter(),
                &self.library_root.display().to_string(),
                &FontId::monospace(10.5),
                theme.text(),
                path_rect.width() - 20.0,
            ),
            FontId::monospace(10.5),
            theme.text(),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui::pill_button_sized(
                ui,
                130.0,
                "Change folder",
                true,
                theme,
                GlassSlot::SettingsControl,
            )
            .clicked()
            {
                *action = Some(SettingsAction::ChooseFolder);
            }
            if ui::pill_button_sized(
                ui,
                110.0,
                "Add files",
                false,
                theme,
                GlassSlot::SettingsControl,
            )
            .clicked()
            {
                *action = Some(SettingsAction::AddFiles);
            }
            if ui::pill_button_sized(
                ui,
                110.0,
                "Add folder",
                false,
                theme,
                GlassSlot::SettingsControl,
            )
            .clicked()
            {
                *action = Some(SettingsAction::AddFolder);
            }
        });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(format!(
                "{} tracks scanned. Every subfolder becomes a playlist; loose files are under All music.",
                self.tracks.len()
            ))
            .size(10.5)
            .color(theme.faint()),
        );
    }

    fn download_settings(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.label(
            egui::RichText::new("Download quality")
                .size(11.0)
                .color(theme.muted())
                .strong(),
        );
        ui.horizontal_wrapped(|ui| {
            for quality in [128u16, 192, 256, 320] {
                let active = self.manager.settings.quality == quality;
                if ui::chip_button(
                    ui,
                    &format!("{quality} kbps"),
                    active,
                    84.0,
                    theme,
                    GlassSlot::SettingsControl,
                )
                .clicked()
                {
                    self.set_quality(quality);
                }
            }
        });
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Downloads at once")
                .size(11.0)
                .color(theme.muted())
                .strong(),
        );
        ui.horizontal_wrapped(|ui| {
            for parallel in [1usize, 2, 3, 4, 6] {
                let active = self.manager.settings.parallel == parallel;
                if ui::chip_button(
                    ui,
                    &parallel.to_string(),
                    active,
                    34.0,
                    theme,
                    GlassSlot::SettingsControl,
                )
                .clicked()
                {
                    self.set_parallel(parallel);
                }
            }
        });
        ui.add_space(8.0);
        let tools = download::tools_dir();
        ui.label(
            egui::RichText::new(match tools {
                Some(ref path) => format!("Tools found in {}", path.display()),
                None => "Tools not found. Put yt-dlp, spotdl and ffmpeg in the bin folder next to the exe.".to_string(),
            })
            .size(10.5)
            .color(if tools.is_some() { theme.muted() } else { theme.danger() }),
        );
        ui.add_space(6.0);
        if ui::pill_button_sized(
            ui,
            130.0,
            "Change folder",
            false,
            theme,
            GlassSlot::SettingsControl,
        )
        .clicked()
        {
            self.choose_library_folder();
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "The downloader saves every track under the music folder; playlist downloads get their own subfolder and cover.",
            )
            .size(10.5)
            .color(theme.faint()),
        );
    }

    fn about_settings(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.label(
            egui::RichText::new("DEF PLAYER")
                .size(18.0)
                .color(theme.text())
                .strong(),
        );
        ui.label(
            egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                .size(10.5)
                .color(theme.faint()),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "A native desktop music player and playlist downloader built with egui + rodio. The dashboard, colours and equalizer are all adjustable here.",
            )
            .size(11.0)
            .color(theme.muted()),
        );
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("SHORTCUTS")
                .size(10.0)
                .color(theme.muted())
                .strong(),
        );
        for line in [
            "Space — play / pause",
            "Left / Right — seek 5 seconds",
            "Up / Down — volume",
            "M — mute",
            "Ctrl+F — search",
            "Ctrl+L — layout mode",
            "Ctrl+, — settings",
            "Esc — close panels and overlays",
        ] {
            ui.label(egui::RichText::new(line).size(10.5).color(theme.faint()));
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "Only download content you have the rights to. Respect the terms of service of the sites you use.",
            )
            .size(10.0)
            .color(theme.faint()),
        );
    }
}

#[derive(Clone, Copy)]
enum SettingsAction {
    ChooseFolder,
    AddFiles,
    AddFolder,
    EditLayout,
    ResetLayout,
}

#[derive(Clone, Copy)]
enum AddMenuAction {
    Files,
    Folder,
    Link,
    Downloader,
    Restore(PanelKind),
}

impl PlayerApp {
    pub fn on_close(&mut self) {
        self.save_config();
        self.manager.kill_all_now();
    }

    pub fn update_logic(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().close_requested()) {
            self.save_config();
            self.manager.kill_all_now();
        }

        self.poll_scan(ctx);
        self.poll_download(ctx);
        self.tick(ctx);
        self.handle_keys(ctx);
        self.ensure_textures(ctx);
        self.ensure_playlist_textures(ctx);
        self.ensure_background(ctx);
        self.handle_drops(ctx);
        self.update_spectrum(ctx);

        if self
            .status_at
            .is_some_and(|started| started.elapsed() > Duration::from_secs(6))
        {
            self.status = None;
            self.status_at = None;
        }
    }

    fn layout_rects(rect: Rect) -> (Rect, Rect) {
        let bar_height = (rect.height() * 0.095).clamp(66.0, 78.0);
        let top = Rect::from_min_size(rect.min, vec2(rect.width(), bar_height));
        let margin = (rect.width() * 0.012).clamp(12.0, 20.0);
        let frame_rect = Rect::from_min_max(
            pos2(rect.left() + margin, rect.top() + bar_height + 8.0),
            pos2(rect.right() - margin, rect.bottom() - margin),
        );
        (top, frame_rect)
    }

    fn dashboard_content(frame: Rect) -> Rect {
        let pad_x = (frame.width() * 0.024).clamp(14.0, 30.0);
        let pad_y = (frame.height() * 0.047).clamp(14.0, 30.0);
        Rect::from_min_max(
            pos2(frame.left() + pad_x, frame.top() + pad_y),
            pos2(frame.right() - pad_x, frame.bottom() - pad_y),
        )
    }

    fn paint_player_art(&self, painter: &egui::Painter, rect: Rect, theme: &Theme) {
        let border_inset = if theme.panel_border_visible {
            theme.panel_border_width.max(0.0)
        } else {
            0.0
        };
        let art = rect.shrink(border_inset);
        let art_radius = (theme.radius - border_inset).max(0.0);
        let texture = if let Some(index) = self.current {
            self.hero_tex
                .as_ref()
                .filter(|(hero_index, _)| *hero_index == index)
                .map(|(_, texture)| texture.clone())
                .or_else(|| {
                    self.thumbs
                        .get(index)
                        .and_then(|texture| texture.as_ref())
                        .cloned()
                })
        } else if let Some(name) = &self.active_playlist {
            self.playlist_texture(name).cloned()
        } else {
            None
        };

        if let Some(texture) = &texture {
            theme::textured_rounded_rect(
                painter,
                art,
                art_radius,
                texture,
                theme::texture_uv(texture.size_vec2(), art.size()),
            );
        } else {
            theme::gradient_rect(
                painter,
                art,
                art_radius,
                theme::lerp_color(theme.panel_top(), theme.accent_a, 0.30),
                theme::lerp_color(theme.panel_bottom(), theme.accent_b, 0.17),
                theme.angle,
            );
        }

        theme::gradient_rect(
            painter,
            art,
            art_radius,
            Color32::from_rgba_unmultiplied(5, 5, 8, 40),
            Color32::from_rgba_unmultiplied(5, 5, 8, 240),
            90.0,
        );
    }

    /// Background content: the backdrop and dashboard frame/artwork that the
    /// real glass refracts. Painted into the root (Background) layer.
    pub fn draw_background(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme;
        let rect = ui.max_rect();
        let time = ui.input(|input| input.time);
        theme::paint_backdrop(ui.painter(), rect, &theme, time);

        if self.view == View::Dashboard {
            let (_, frame_rect) = Self::layout_rects(rect);
            theme::paint_dashboard_frame(
                ui.painter(),
                frame_rect,
                &theme,
                self.background_tex.as_ref(),
            );
            let content = Self::dashboard_content(frame_rect);
            if self.config.ui.panels[PanelKind::Player.index()].visible {
                let player = self.panel_rect(content, PanelKind::Player);
                if player.width() > 40.0 && player.height() > 30.0 {
                    self.paint_player_art(ui.painter(), player, &theme);
                }
            }
            if self.settings_open {
                ui.painter().rect_filled(
                    rect,
                    0.0,
                    Color32::from_rgba_unmultiplied(2, 2, 4, 205),
                );
                let modal = Rect::from_center_size(
                    rect.center(),
                    vec2(
                        (rect.width() * 0.82).min(780.0),
                        (rect.height() * 0.86).min(650.0),
                    ),
                );
                theme::gradient_rect(
                    ui.painter(),
                    modal,
                    20.0,
                    theme::lerp_color(theme.panel_top(), theme.accent_a, 0.18),
                    theme::lerp_color(theme.panel_bottom(), theme.accent_b, 0.12),
                    theme.angle,
                );
            }
        }
    }

    /// Foreground UI: labels, artwork, panel content and the non-glass controls.
    /// Painted after the background content in the same egui pass; glass-target
    /// backgrounds are skipped and recorded for the liquid-rust scene.
    pub fn draw_foreground(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let theme = self.theme;
        let real_capable = theme.real_glass;
        self.theme.foreground_only = true;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::TRANSPARENT))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                if self.view == View::Setup {
                    self.setup_screen(ui, rect);
                    return;
                }

                let (top, frame_rect) = Self::layout_rects(rect);

                if real_capable {
                    let show_dashboard = !self.settings_open;
                    self.theme.real_glass = show_dashboard;
                    self.theme.liquid.enabled =
                        self.config.ui.liquid.enabled && show_dashboard;
                }

                if !self.settings_open {
                    self.top_bar(ui, top);
                    self.dashboard(ui, frame_rect);
                }

                self.theme.real_glass = real_capable;
                self.theme.liquid.enabled = self.config.ui.liquid.enabled;

                if self.settings_open || self.add_menu_open {
                    let _ = ui.interact(
                        rect,
                        Id::unique("modal-input-blocker"),
                        Sense::click_and_drag(),
                    );
                }

                if self.view == View::Dashboard {
                    self.add_menu(ctx);
                    self.search_overlay(ctx);
                    if self.settings_open {
                        self.settings_modal(ctx);
                    }
                }

                if self.drop_hovering {
                    let screen = ui.max_rect();
                    let theme = self.theme;
                    let painter = ui.painter();
                    painter.rect_filled(
                        screen,
                        0.0,
                        Color32::from_rgba_unmultiplied(4, 4, 7, 220),
                    );
                    let card = Rect::from_center_size(screen.center(), vec2(440.0, 150.0));
                    theme::paint_card(painter, card, 20.0, &theme, true, false, 6, false);
                    painter.text(
                        card.center() - vec2(0.0, 16.0),
                        Align2::CENTER_CENTER,
                        "Drop to add to your library",
                        FontId::proportional(17.0),
                        theme.text(),
                    );
                    painter.text(
                        card.center() + vec2(0.0, 14.0),
                        Align2::CENTER_CENTER,
                        "Audio files and folders are supported.",
                        FontId::proportional(13.0),
                        theme.muted(),
                    );
                }

                self.theme.real_glass = real_capable;
                self.theme.liquid.enabled = self.config.ui.liquid.enabled;
            });

        self.theme.foreground_only = false;
    }
}

const LAYOUT_SAFE_PAD: f32 = 6.0;
const LAYOUT_SNAP: f32 = 7.0;
const LAYOUT_GRID: f32 = 8.0;

fn glass_slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    changed: &mut bool,
) {
    if ui
        .add(egui::Slider::new(value, range).text(label))
        .changed()
    {
        *changed = true;
    }
}

fn glass_check(ui: &mut Ui, label: &str, value: &mut bool, changed: &mut bool) {
    if ui.checkbox(value, label).changed() {
        *changed = true;
    }
}

fn snap_targets(others: &[Rect], content: Rect) -> (Vec<f32>, Vec<f32>) {
    let safe = content.shrink(LAYOUT_SAFE_PAD);
    let mut x = vec![safe.left(), safe.center().x, safe.right()];
    let mut y = vec![safe.top(), safe.center().y, safe.bottom()];
    for rect in others {
        x.extend([rect.left(), rect.center().x, rect.right()]);
        y.extend([rect.top(), rect.center().y, rect.bottom()]);
    }
    (x, y)
}

fn snap_layout_move(
    proposed: Rect,
    content: Rect,
    others: &[Rect],
) -> (Rect, Option<f32>, Option<f32>) {
    let safe = content.shrink(LAYOUT_SAFE_PAD);
    let (targets_x, targets_y) = snap_targets(others, content);
    let width = proposed.width().min(safe.width());
    let height = proposed.height().min(safe.height());
    let mut x = proposed.left();
    let mut y = proposed.top();

    let mut best_x: Option<(f32, f32)> = None;
    for edge in [proposed.left(), proposed.center().x, proposed.right()] {
        for target in &targets_x {
            let delta = target - edge;
            if delta.abs() <= LAYOUT_SNAP
                && best_x
                    .map(|(_, best)| delta.abs() < best.abs())
                    .unwrap_or(true)
            {
                best_x = Some((*target, delta));
            }
        }
    }
    if let Some((_, delta)) = best_x {
        x += delta;
    } else {
        x = (x / LAYOUT_GRID).round() * LAYOUT_GRID;
    }

    let mut best_y: Option<(f32, f32)> = None;
    for edge in [proposed.top(), proposed.center().y, proposed.bottom()] {
        for target in &targets_y {
            let delta = target - edge;
            if delta.abs() <= LAYOUT_SNAP
                && best_y
                    .map(|(_, best)| delta.abs() < best.abs())
                    .unwrap_or(true)
            {
                best_y = Some((*target, delta));
            }
        }
    }
    if let Some((_, delta)) = best_y {
        y += delta;
    } else {
        y = (y / LAYOUT_GRID).round() * LAYOUT_GRID;
    }

    x = x.clamp(safe.left(), (safe.right() - width).max(safe.left()));
    y = y.clamp(safe.top(), (safe.bottom() - height).max(safe.top()));
    (
        Rect::from_min_size(pos2(x, y), vec2(width, height)),
        best_x.map(|(target, _)| target),
        best_y.map(|(target, _)| target),
    )
}

fn snap_layout_resize(
    proposed: Rect,
    content: Rect,
    others: &[Rect],
    min_width: f32,
    min_height: f32,
) -> (Rect, Option<f32>, Option<f32>) {
    let safe = content.shrink(LAYOUT_SAFE_PAD);
    let (targets_x, targets_y) = snap_targets(others, content);
    let mut right = proposed.right();
    let mut bottom = proposed.bottom();

    let mut best_x: Option<(f32, f32)> = None;
    for target in &targets_x {
        let delta = target - proposed.right();
        if delta.abs() <= LAYOUT_SNAP
            && best_x
                .map(|(_, best)| delta.abs() < best.abs())
                .unwrap_or(true)
        {
            best_x = Some((*target, delta));
        }
    }
    if let Some((_, delta)) = best_x {
        right += delta;
    } else {
        right = (right / LAYOUT_GRID).round() * LAYOUT_GRID;
    }

    let mut best_y: Option<(f32, f32)> = None;
    for target in &targets_y {
        let delta = target - proposed.bottom();
        if delta.abs() <= LAYOUT_SNAP
            && best_y
                .map(|(_, best)| delta.abs() < best.abs())
                .unwrap_or(true)
        {
            best_y = Some((*target, delta));
        }
    }
    if let Some((_, delta)) = best_y {
        bottom += delta;
    } else {
        bottom = (bottom / LAYOUT_GRID).round() * LAYOUT_GRID;
    }

    right = right
        .max(proposed.left() + min_width.min(safe.width()))
        .min(safe.right());
    bottom = bottom
        .max(proposed.top() + min_height.min(safe.height()))
        .min(safe.bottom());

    (
        Rect::from_min_max(proposed.min, pos2(right, bottom)),
        best_x.map(|(target, _)| target),
        best_y.map(|(target, _)| target),
    )
}

fn format_duration(seconds: f32) -> String {    if !seconds.is_finite() || seconds < 0.0 {
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

fn configure_style(ctx: &egui::Context, theme: &Theme) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = theme.bg();
    visuals.window_fill = theme.panel_top();
    visuals.extreme_bg_color = theme.panel_bottom();
    visuals.faint_bg_color = theme.panel_soft();
    visuals.override_text_color = Some(theme.text());
    visuals.selection.bg_fill = theme.accent_a;
    visuals.selection.stroke = Stroke::new(1.0_f32, theme.text());
    visuals.hyperlink_color = theme.accent_b;
    visuals.widgets.inactive.bg_fill = theme.panel_soft();
    visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, theme.line());
    visuals.widgets.hovered.bg_fill = theme::with_alpha(theme.accent_a, 92);
    visuals.widgets.hovered.weak_bg_fill = theme::with_alpha(theme.accent_a, 44);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, theme.accent_line());
    visuals.widgets.active.bg_fill = theme::with_alpha(theme.accent_a, 130);
    visuals.widgets.active.weak_bg_fill = theme::with_alpha(theme.accent_a, 70);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, theme.accent_line());
    visuals.widgets.inactive.corner_radius = CornerRadius::from(8.0);
    visuals.widgets.hovered.corner_radius = CornerRadius::from(8.0);
    visuals.widgets.active.corner_radius = CornerRadius::from(8.0);
    style.visuals = visuals;
    style.spacing.item_spacing = vec2(8.0, 8.0);
    style.spacing.button_padding = vec2(10.0, 6.0);
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}

#[cfg(test)]
mod glass_click_tests {
    use super::*;

    fn run_frame(
        ctx: &egui::Context,
        app: &mut PlayerApp,
        input: egui::RawInput,
        _screen: Rect,
    ) {
        let mut output = ctx.run_ui(input, |ui| {
            app.draw_background(ui);
            app.draw_foreground(ui);
        });
        output.textures_delta.clear();
    }

    #[test]
    fn settings_button_opens_in_single_pass() {
        let ctx = egui::Context::default();
        let mut app = PlayerApp::new(&ctx, Vec::new(), false, false, None, false);
        app.view = View::Dashboard;
        app.settings_open = false;

        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(1180.0, 760.0));
        let pointer = pos2(1141.0, 36.0);
        let frames = [
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(0.5),
                focused: true,
                events: vec![egui::Event::PointerMoved(pointer)],
                ..Default::default()
            },
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(1.0),
                focused: true,
                events: vec![egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(1.2),
                focused: true,
                events: vec![egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
        ];

        for frame in frames {
            run_frame(&ctx, &mut app, frame, screen);
        }

        assert!(app.settings_open, "settings button did not receive the click");

        let close = pos2(955.0, 78.0);
        let close_frames = [
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(2.0),
                focused: true,
                events: vec![egui::Event::PointerMoved(close)],
                ..Default::default()
            },
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(2.5),
                focused: true,
                events: vec![egui::Event::PointerButton {
                    pos: close,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(2.7),
                focused: true,
                events: vec![egui::Event::PointerButton {
                    pos: close,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
        ];
        for frame in close_frames {
            run_frame(&ctx, &mut app, frame, screen);
        }
        assert!(!app.settings_open, "settings close button did not receive the click");
    }
}

