use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Layout, Rect, Sense,
    Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, UiBuilder, Vec2,
};

use crate::audio::{AudioEngine, EQ_FREQS};
use crate::config::{self, Config, GlassPreset, GlassShape, TrackSort, UiConfig, EQ_BANDS};
use crate::deps;
use crate::download;
use crate::glass::GlassSlot;
use crate::library::{self, Track};
use crate::queue::{self, JobState};
use crate::spatial::{self, TestMode};
use crate::theme::{self, Theme};
use crate::ui::{self, Glyph};
use crate::video_bg::VideoBackground;
use crate::visualizer::{self, SPECTRUM_BINS};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Repeat {
    Off,
    All,
    One,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Installing,
    Setup,
    Dashboard,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InstallRow {
    Done,
    Current,
    Waiting,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PanelKind {
    CurrentPlaylist,
    Player,
    Downloader,
    Playlists,
    Equalizer,
    Visualizer,
    SpatialAudio,
}

const PANEL_KINDS: [PanelKind; 7] = [
    PanelKind::CurrentPlaylist,
    PanelKind::Player,
    PanelKind::Downloader,
    PanelKind::Playlists,
    PanelKind::Equalizer,
    PanelKind::Visualizer,
    PanelKind::SpatialAudio,
];

impl PanelKind {
    fn index(self) -> usize {
        match self {
            PanelKind::CurrentPlaylist => 0,
            PanelKind::Player => 1,
            PanelKind::Downloader => 2,
            PanelKind::Playlists => 3,
            PanelKind::Equalizer => 4,
            PanelKind::Visualizer => 5,
            PanelKind::SpatialAudio => 6,
        }
    }

    fn key(self) -> &'static str {
        match self {
            PanelKind::CurrentPlaylist => "current_playlist",
            PanelKind::Player => "player",
            PanelKind::Downloader => "downloader",
            PanelKind::Playlists => "playlists",
            PanelKind::Equalizer => "equalizer",
            PanelKind::Visualizer => "visualizer",
            PanelKind::SpatialAudio => "spatial_audio",
        }
    }

    fn title(self) -> &'static str {
        match self {
            PanelKind::CurrentPlaylist => "Current playlist",
            PanelKind::Player => "Player",
            PanelKind::Downloader => "Downloader",
            PanelKind::Playlists => "Playlists",
            PanelKind::Equalizer => "Equalizer",
            PanelKind::Visualizer => "Audio visualizer",
            PanelKind::SpatialAudio => "Spatial audio",
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

/// Progress of the first-run tool installation shown on the loading screen.
struct InstallState {
    receiver: Option<Receiver<deps::Event>>,
    status: String,
    fraction: Option<f32>,
    tool: usize,
    error: Option<String>,
    first_run: bool,
}

impl Default for InstallState {
    fn default() -> Self {
        Self {
            receiver: None,
            status: "Connecting…".to_string(),
            fraction: Some(0.0),
            tool: 0,
            error: None,
            first_run: false,
        }
    }
}

pub struct PlayerApp {
    view: View,
    configured: bool,
    install: InstallState,
    initial_paths: Vec<PathBuf>,
    /// True while the spatial renderer is applied (panel visible and switched on).
    spatial_active: bool,
    /// Whether the equalizer currently processes playback; mirrors
    /// [`UiConfig::eq_active`] so adding or hiding the component takes
    /// effect immediately.
    eq_active: bool,
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
    /// True when a seek handle was dragged during the frame being built. The
    /// output stays muted until one full frame passes without a drag, so a
    /// seek in progress is never audible.
    seek_drag_seen: bool,
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
    /// Playlist folder new downloads land in; `None` = All music (root).
    download_target: Option<String>,
    download_target_menu_open: bool,
    download_target_menu_pos: egui::Pos2,
    download_target_menu_rect: Rect,
    download_target_menu_ignore_click: bool,
    /// Inline "new playlist" input of the downloader panel.
    download_new_open: bool,
    download_new_playlist: String,
    download_new_focus: bool,
    /// Update mode: songs already in the target folder are not downloaded again.
    download_skip_existing: bool,
    /// Right-click menu of one song in the current playlist.
    track_menu_open: bool,
    track_menu_pos: egui::Pos2,
    track_menu_rect: Rect,
    track_menu_ignore_click: bool,
    track_menu_index: Option<usize>,
    /// "New playlist" input inside the song menu.
    track_menu_new_open: bool,
    track_menu_new_name: String,
    track_menu_new_focus: bool,
    /// Playlist rows of the open song menu with their rects — like
    /// `plus_rect`, recorded so tests can click them precisely.
    track_menu_row_rects: Vec<(String, Rect)>,
    /// True while a scan should replace the whole track list (after file
    /// moves) instead of merging new files into it.
    scan_replace: bool,
    /// Old and new path of the last moved file, used to keep the currently
    /// playing song selected across a replacing rescan.
    scan_move: Option<(PathBuf, PathBuf)>,
    /// Status message to show once a replacing rescan finishes.
    scan_notice: Option<String>,

    config: Config,
    theme: Theme,
    add_menu_open: bool,
    add_menu_pos: egui::Pos2,
    add_menu_rect: Rect,
    add_menu_ignore_click: bool,
    plus_rect: Option<Rect>,
    search_query: String,
    search_active: bool,
    search_focus_request: bool,
    search_rect: Option<Rect>,
    search_popup_rect: Rect,
    settings_open: bool,
    settings_tab: SettingsTab,
    layout_edit: bool,
    spectrum: Vec<f32>,
    background_tex: Option<TextureHandle>,
    background_loaded: Option<PathBuf>,
    /// Decoded photo kept in memory so blur changes do not re-read the file.
    background_source: Option<image::RgbaImage>,
    /// Blur strength currently baked into `background_tex`.
    background_blur_applied: f32,
    video: VideoBackground,
    video_error: Option<String>,
    drop_hovering: bool,
    sort_menu_open: bool,
    sort_menu_pos: egui::Pos2,
    sort_menu_rect: Rect,
    sort_menu_ignore_click: bool,
    /// The component currently dragged in layout mode.
    layout_drag: Option<LayoutDrag>,
    /// The component gliding to its snapped position after a layout drag.
    layout_anim: Option<LayoutAnim>,
    /// Name being typed for a new layout preset.
    layout_preset_name: String,
    /// Snapshot of the UI config as last written to disk by the auto-save.
    saved_ui: UiConfig,
    /// When the config was last written to disk.
    saved_at: Instant,
}

/// Live drag state for one dashboard component in layout mode.
struct LayoutDrag {
    index: usize,
    /// Pointer offset inside the component at grab time.
    grab: Vec2,
    /// True when pulling the resize corner instead of the body.
    resizing: bool,
}

/// Smooth glide from a drag drop to the snapped grid position.
struct LayoutAnim {
    index: usize,
    from: Vec2,
    to: Vec2,
    /// True when animating size instead of position.
    resizing: bool,
    start: Instant,
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
        let (mut saved, load_note) = config::load_reported();
        let configured = saved.is_some();
        let mut config = saved
            .take()
            .unwrap_or_else(|| Config::with_root(config::default_library_root()));
        let mut theme = Theme::from_ui(&config.ui);
        theme.real_glass = real_glass;
        configure_style(egui_ctx, &theme);

        let (audio, status) = match AudioEngine::new() {
            Ok(mut engine) => {
                let warning = engine.configure(&mut config.ui);
                (Some(engine), warning)
            }
            Err(err) => (None, Some(err)),
        };
        // A recovered-or-lost settings note outranks nothing: show it once
        // at startup so a reset is never silent again.
        let status = status.or(load_note);

        let manager = queue::Manager::new(queue::Settings {
            root: config.library_root.clone(),
            quality: config.quality,
            parallel: config.parallel,
        });

        // Run the installer whenever anything is missing, not only on the
        // literal first run: an app update extracted to a fresh folder must
        // re-fetch the tools exactly once (into the stable per-user bin),
        // and never again on later updates.
        let installing = deps::needs_first_run() || !deps::assets_ready();
        let saved_ui = config.ui.clone();

        let mut app = Self {
            view: if installing {
                View::Installing
            } else if configured {
                View::Dashboard
            } else {
                View::Setup
            },
            configured,
            install: InstallState::default(),
            initial_paths,
            spatial_active: config.ui.spatial_active(),
            eq_active: config.ui.eq_active(),
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
            seek_drag_seen: false,
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
            download_target: config.download_target.clone(),
            download_target_menu_open: false,
            download_target_menu_pos: egui::Pos2::ZERO,
            download_target_menu_rect: Rect::NOTHING,
            download_target_menu_ignore_click: false,
            download_new_open: false,
            download_new_playlist: String::new(),
            download_new_focus: false,
            download_skip_existing: config.download_skip_existing,
            track_menu_open: false,
            track_menu_pos: egui::Pos2::ZERO,
            track_menu_rect: Rect::NOTHING,
            track_menu_ignore_click: false,
            track_menu_index: None,
            track_menu_new_open: false,
            track_menu_new_name: String::new(),
            track_menu_new_focus: false,
            track_menu_row_rects: Vec::new(),
            scan_replace: false,
            scan_move: None,
            scan_notice: None,
            config,
            theme,
            add_menu_open: false,
            add_menu_pos: egui::Pos2::ZERO,
            add_menu_rect: Rect::NOTHING,
            add_menu_ignore_click: false,
            plus_rect: None,
            search_query: String::new(),
            search_active: false,
            search_focus_request: false,
            search_rect: None,
            search_popup_rect: Rect::NOTHING,
            settings_open: false,
            settings_tab: SettingsTab::Appearance,
            layout_edit: false,
            spectrum: vec![0.0; SPECTRUM_BINS],
            background_tex: None,
            background_loaded: None,
            background_source: None,
            background_blur_applied: 0.0,
            video: VideoBackground::new(),
            video_error: None,
            drop_hovering: false,
            sort_menu_open: false,
            sort_menu_pos: egui::Pos2::ZERO,
            sort_menu_rect: Rect::NOTHING,
            sort_menu_ignore_click: false,
            layout_drag: None,
            layout_anim: None,
            layout_preset_name: String::new(),
            saved_ui,
            saved_at: Instant::now(),
        };

        if installing {
            app.start_install();
        } else if configured {
            let mut scan_paths = vec![app.library_root.clone()];
            scan_paths.extend(std::mem::take(&mut app.initial_paths));
            app.start_scan(scan_paths);

            if let Some(url) = app.pending_url.take() {
                app.download_url = url;
                app.start_download();
            }

            // A launch from a restricted shell shows up as "cannot write to
            // the music folder" much later; say it right away instead. The
            // downloader panel shows it, so a scan notice cannot hide it.
            let root = app.library_root.clone();
            if let Err(err) = Self::ensure_folder_writable(&root) {
                app.download_status = Some(err);
                app.download_failed = true;
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

        // Count the play for the most-used sort.
        let plays = self
            .config
            .ui
            .track_plays
            .entry(path.to_string_lossy().to_string())
            .or_insert(0);
        *plays += 1;
        self.save_config();

        let Some(audio) = &mut self.audio else {
            self.notice("No audio output device available.", true);
            return;
        };

        audio.set_volume(self.volume);
        audio.set_muted(self.muted);
        audio.set_eq(&self.config.ui.eq_gains, self.config.ui.eq_active());

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

    /// Silences the music while a seek handle is dragged: seeking plays a
    /// stuttering burst of every position the handle passes, which is the
    /// "fast-forward" noise. `update_logic` brings the volume back on the
    /// first frame without a drag.
    fn note_seek_drag(&mut self, dragging: bool) {
        if !dragging {
            return;
        }
        self.seek_drag_seen = true;
        if let Some(audio) = &mut self.audio {
            audio.set_seeking(true);
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

        // The chosen song order also drives the next/previous buttons.
        let sort = self.config.ui.track_sort;
        let tracks = &self.tracks;
        let plays = &self.config.ui.track_plays;
        match sort {
            TrackSort::Added => {}
            TrackSort::Newest => self
                .visible
                .sort_by(|a, b| tracks[*b].added.cmp(&tracks[*a].added)),
            TrackSort::Oldest => self
                .visible
                .sort_by(|a, b| tracks[*a].added.cmp(&tracks[*b].added)),
            TrackSort::Title => self.visible.sort_by(|a, b| {
                tracks[*a]
                    .title
                    .to_lowercase()
                    .cmp(&tracks[*b].title.to_lowercase())
            }),
            TrackSort::MostUsed => self.visible.sort_by(|a, b| {
                let a_plays = plays
                    .get(tracks[*a].path.to_string_lossy().as_ref())
                    .copied()
                    .unwrap_or(0);
                let b_plays = plays
                    .get(tracks[*b].path.to_string_lossy().as_ref())
                    .copied()
                    .unwrap_or(0);
                b_plays.cmp(&a_plays)
            }),
        }
    }

    /// Switches the active playlist.
    fn activate_playlist(&mut self, playlist: Option<String>) {
        self.active_playlist = playlist;
        self.rebuild_visible();
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
                if self.scan_replace {
                    self.finish_replace_scan(tracks);
                } else {
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

    /// Full rescan that replaces the track list instead of merging new files
    /// into it — used after songs were moved between playlist folders.
    fn refresh_library(&mut self, notice: Option<String>) {
        self.scanning = true;
        self.scan_replace = true;
        self.scan_notice = notice;
        let (sender, receiver) = channel();
        self.scan_rx = Some(receiver);
        let root = self.library_root.clone();

        std::thread::spawn(move || {
            let tracks = library::scan(std::slice::from_ref(&root), Some(&root));
            let _ = sender.send(tracks);
        });
    }

    /// Rebuilds the whole library after a replacing rescan, keeping the
    /// currently playing song selected (its file may have been renamed).
    fn finish_replace_scan(&mut self, tracks: Vec<Track>) {
        self.scan_replace = false;

        // Which path should stay "now playing" once the list is rebuilt?
        let keep = match (&self.current, &self.scan_move) {
            (Some(index), Some((old, new)))
                if self
                    .tracks
                    .get(*index)
                    .is_some_and(|track| track.path == *old) =>
            {
                Some(new.clone())
            }
            (Some(index), _) => self.tracks.get(*index).map(|track| track.path.clone()),
            _ => None,
        };
        self.scan_move = None;

        self.tracks = tracks;
        self.thumbs = vec![None; self.tracks.len()];
        self.playlist_tex.clear();
        self.hero_tex = None;
        self.current =
            keep.and_then(|path| self.tracks.iter().position(|track| track.path == path));

        if self.current.is_none() {
            // The selected file is gone from the library: stop gracefully.
            if let Some(audio) = &self.audio {
                audio.pause();
            }
            self.paused = true;
        }

        self.rebuild_visible();
        self.scanning = false;
        self.scan_rx = None;

        if let Some(message) = self.scan_notice.take() {
            self.notice(message, false);
        }
    }

    /// Real folder of a playlist display name; nested playlists show as
    /// "Parent / Child" and map back to a real path here.
    fn playlist_folder(root: &std::path::Path, name: &str) -> PathBuf {
        root.join(name.replace(" / ", std::path::MAIN_SEPARATOR_STR))
    }

    /// Moves a song's file into a playlist folder — `None` sends it back to
    /// All music (the library root) — then refreshes the library. The play
    /// count follows the file and playback is not interrupted.
    fn move_track_to_playlist(&mut self, index: usize, target: Option<&str>) {
        let Some(track) = self.tracks.get(index) else {
            return;
        };
        let source = track.path.clone();
        let title = track.title.clone();
        let root = self.library_root.clone();

        let folder = match target {
            Some(name) => Self::playlist_folder(&root, name),
            None => root.clone(),
        };

        if source.parent() == Some(folder.as_path()) {
            let label = target.unwrap_or("All music");
            self.notice(format!("“{title}” is already in {label}."), false);
            return;
        }

        let Some(file_name) = source.file_name().map(|name| name.to_os_string()) else {
            self.notice(format!("“{title}” has no file to move."), true);
            return;
        };

        if let Err(err) = std::fs::create_dir_all(&folder) {
            self.notice(format!("Cannot create that playlist folder: {err}"), true);
            return;
        }

        // Never overwrite: a clash becomes "Song (2).mp3", "Song (3).mp3"…
        let mut destination = folder.join(&file_name);
        let mut counter = 2;
        while destination.exists() {
            if counter > 500 {
                self.notice(
                    "That playlist already has too many copies of this song.",
                    true,
                );
                return;
            }
            let stem = std::path::Path::new(&file_name)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("Song");
            let ext = std::path::Path::new(&file_name)
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("mp3");
            destination = folder.join(format!("{stem} ({counter}).{ext}"));
            counter += 1;
        }

        // Rename first (instant, same drive); fall back to copy + remove
        // when the file sits on another drive.
        let moved = std::fs::rename(&source, &destination).or_else(|_| {
            std::fs::copy(&source, &destination)
                .map(|_| ())
                .and_then(|_| std::fs::remove_file(&source))
        });

        if let Err(err) = moved {
            let _ = std::fs::remove_file(&destination);
            self.notice(
                format!("Could not move “{title}”: {err}. Try again with the song paused."),
                true,
            );
            return;
        }

        // The play count follows the file to its new path. No save_config
        // here: the autosave persists it, so moves stay cheap and testable.
        let old_key = source.to_string_lossy().to_string();
        let new_key = destination.to_string_lossy().to_string();
        if let Some(count) = self.config.ui.track_plays.remove(&old_key) {
            self.config.ui.track_plays.insert(new_key, count);
        }

        self.scan_move = Some((source, destination));
        let label = match target {
            Some(name) => format!("“{name}”"),
            None => "All music".to_string(),
        };
        self.refresh_library(Some(format!("Moved “{title}” to {label}.")));
    }

    /* ---------- downloads ---------- */

    fn start_download(&mut self) {
        let url = self.download_url.trim().to_string();
        if url.is_empty() {
            self.download_status = Some("Paste a YouTube or Spotify link first.".to_string());
            self.download_failed = true;
            return;
        }

        // Fail before queueing anything when the destination cannot be
        // written — that is exactly what a restricted launch looks like.
        if let Err(err) = Self::ensure_folder_writable(&self.download_destination_folder()) {
            self.download_status = Some(err);
            self.download_failed = true;
            return;
        }

        self.download_status = None;
        self.download_failed = false;
        // The target and update mode are captured per job, so changing the
        // panel selector afterwards never reroutes links already queued.
        self.manager
            .submit(url, self.download_target.clone(), self.download_skip_existing);
    }

    /// Folder new downloads land in: the selected playlist, or the root.
    fn download_destination_folder(&self) -> PathBuf {
        self.download_target
            .as_deref()
            .map(|name| Self::playlist_folder(&self.library_root, name))
            .unwrap_or_else(|| self.library_root.clone())
    }

    /// What the user can do about a blocked save, worded for the real cause.
    /// A launch from a folder marked "low integrity" (or from a launcher that
    /// hands the app a restricted token) denies every write, including the
    /// player's own settings folder — then changing the music folder cannot
    /// help and must not be suggested.
    fn save_blocked_hint(settings_writable: bool) -> String {
        if settings_writable {
            "Start BR1 Player.exe by double-clicking it instead of launching it from a terminal, \
             a script or another app; if that does not help, allow BR1 Player in the folder's \
             Properties → Security and in any antivirus folder protection, or pick another \
             folder with CHANGE FOLDER."
                .to_string()
        } else {
            "Windows is running the player with restricted write permissions — it cannot even \
             write its own settings folder — so the music folder is not the problem. Copy \
             BR1 Player.exe together with the bin folder to a normal folder (for example \
             D:\\BR1 Player) and start it from there."
                .to_string()
        }
    }

    /// True when the player can write next to its own config. False only for a
    /// restricted launch, where no folder outside the app's own is writable.
    fn settings_folder_writable() -> bool {
        let Some(folder) = config::config_path().and_then(|path| {
            path.parent().map(|parent| parent.to_path_buf())
        }) else {
            return true;
        };
        let probe = folder.join(".br1-settings-check");
        if std::fs::write(&probe, b"ok").is_err() {
            return false;
        }
        let _ = std::fs::remove_file(&probe);
        true
    }

    /// Creates `folder` if needed and proves a file can be written there.
    /// The failure message tells the user what to do about it.
    ///
    /// Only the write is evidence: the probe file is deleted afterwards, but a
    /// refused delete is cleanup noise — antivirus and indexers routinely hold
    /// a fresh file for a moment and answer "access denied" — so it can never
    /// turn a writable folder into a permission error. The probe name carries
    /// the process id so a leftover probe from an earlier run (or a fixed name
    /// stuck open) cannot block every later download the way one shared name
    /// would.
    fn ensure_folder_writable(folder: &std::path::Path) -> Result<(), String> {
        if let Err(err) = std::fs::create_dir_all(folder) {
            return Err(format!(
                "Cannot create {}: {err}. {}",
                folder.display(),
                Self::save_blocked_hint(Self::settings_folder_writable())
            ));
        }

        // Probes written by older builds used one fixed name; clear it when it
        // is still around and writable so nobody mistakes it for a real file.
        let _ = std::fs::remove_file(folder.join(".br1-write-check"));

        let probe = folder.join(format!(".br1-write-check-{}", std::process::id()));
        let mut last: Option<std::io::Error> = None;
        for attempt in 0..3u64 {
            match std::fs::write(&probe, b"ok") {
                Ok(()) => {
                    // Cleanup only, best effort: a locked probe is harmless
                    // because the next run uses a different name.
                    for _ in 0..5 {
                        if std::fs::remove_file(&probe).is_ok() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    return Ok(());
                }
                Err(err) => {
                    // A scanner or a just-disconnected drive can lose a race
                    // once; retrying keeps a slow moment from blocking the
                    // download outright.
                    last = Some(err);
                    std::thread::sleep(Duration::from_millis(25 * (attempt + 1)));
                }
            }
        }

        let err = last.unwrap_or_else(|| std::io::Error::other("write check failed"));
        config::audit(&format!(
            "write check failed for {} ({err:?}, os error {:?})",
            folder.display(),
            err.raw_os_error()
        ));
        Err(format!(
            "No permission to save into {} ({err}). {}",
            folder.display(),
            Self::save_blocked_hint(Self::settings_folder_writable())
        ))
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
            download_target: self.download_target.clone(),
            download_skip_existing: self.download_skip_existing,
            ui: self.config.ui.clone(),
        });
    }

    /// Persists UI changes a couple of seconds after they happen, so state
    /// survives even when the app is killed, crashes or the session ends
    /// without a clean window close. Returns once the disk copy is current.
    fn autosave(&mut self, ctx: &egui::Context) {
        if self.config.ui == self.saved_ui {
            return;
        }
        // Give rapid changes (slider drags, layout moves) time to settle.
        if self.saved_at.elapsed() < Duration::from_secs(2) {
            ctx.request_repaint_after(Duration::from_millis(500));
            return;
        }
        self.save_config();
        self.saved_ui = self.config.ui.clone();
        self.saved_at = Instant::now();
    }

    fn refresh_theme(&mut self, ctx: &egui::Context) {
        let real_glass = self.theme.real_glass;
        let foreground_only = self.theme.foreground_only;
        self.theme = Theme::from_ui(&self.config.ui);
        self.theme.real_glass = real_glass;
        self.theme.foreground_only = foreground_only;
        configure_style(ctx, &self.theme);
        if let Some(audio) = &self.audio {
            audio.set_eq(&self.config.ui.eq_gains, self.config.ui.eq_active());
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

    /* ---------- first-run tool installation ---------- */

    fn start_install(&mut self) {
        self.install = InstallState {
            first_run: deps::needs_first_run(),
            ..InstallState::default()
        };
        self.install.receiver = Some(deps::start());
        self.status = None;
        self.view = View::Installing;
    }

    fn poll_install(&mut self, ctx: &egui::Context) {
        let Some(receiver) = self.install.receiver.take() else {
            return;
        };

        let mut finished = None;
        loop {
            match receiver.try_recv() {
                Ok(deps::Event::Progress {
                    tool,
                    status,
                    fraction,
                }) => {
                    self.install.tool = tool;
                    self.install.status = status;
                    self.install.fraction = fraction;
                }
                Ok(deps::Event::Finished(result)) => {
                    finished = Some(result);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = Some(Err("The setup stopped unexpectedly.".to_string()));
                    break;
                }
            }
        }

        match finished {
            Some(Ok(())) => {
                if let Err(err) = deps::mark_first_run_complete() {
                    self.notice(format!("Could not save first-run state: {err}"), true);
                }
                self.install.error = None;
                self.finish_install();
            }
            Some(Err(err)) => {
                self.install.error = Some(err);
                self.install.fraction = None;
                self.install.status = "Setup did not finish.".to_string();
            }
            None => {
                self.install.receiver = Some(receiver);
                ctx.request_repaint_after(Duration::from_millis(80));
            }
        }
    }

    /// Leaves the loading screen: the dashboard when the library is already
    /// configured, otherwise the folder picker.
    fn finish_install(&mut self) {
        let first_run = self.install.first_run;
        self.install = InstallState::default();

        if let Some(audio) = &mut self.audio {
            audio.rescan_hrtf_sets();
        }
        if first_run {
            // The engine scanned the HRTF folder before the measured sets were
            // downloaded, so refresh the list. A brand-new profile starts with
            // KEMAR; existing configs keep the set the user already chose.
            let available = self
                .audio
                .as_ref()
                .map(|audio| {
                    audio
                        .hrtf_sets()
                        .iter()
                        .any(|set| set.id == spatial::KEMAR_NORMAL_ID)
                })
                .unwrap_or(false);
            if !self.configured
                && available
                && self.config.ui.hrtf_set == spatial::DEFAULT_SET_ID
            {
                self.config.ui.hrtf_set = spatial::KEMAR_NORMAL_ID.to_string();
                if !self.apply_hrtf_set() {
                    self.config.ui.hrtf_set = spatial::DEFAULT_SET_ID.to_string();
                }
            }
        } else {
            // A repair install can restore a set that was missing at startup.
            let current = self.config.ui.hrtf_set.clone();
            let available = self
                .audio
                .as_ref()
                .map(|audio| audio.hrtf_sets().iter().any(|set| set.id == current))
                .unwrap_or(false);
            if available {
                self.apply_hrtf_set();
            }
        }

        if self.configured {
            self.view = View::Dashboard;
            let mut scan_paths = vec![self.library_root.clone()];
            scan_paths.extend(std::mem::take(&mut self.initial_paths));
            self.start_scan(scan_paths);

            if let Some(url) = self.pending_url.take() {
                self.download_url = url;
                self.start_download();
            }
        } else {
            self.view = View::Setup;
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
            egui::RichText::new("BR1 PLAYER")
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
                "Choose the main folder BR1 Player uses for downloads and playlists. The dashboard is built around it.",
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
            "Start using BR1 Player",
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

    /* ---------- loading screen ---------- */

    fn install_screen(&mut self, ui: &mut Ui, rect: Rect) {
        let theme = self.theme;
        let width = (rect.width() * 0.58).clamp(430.0, 780.0);
        let height = 440.0f32.min(rect.height() - 60.0).max(320.0);
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
                .id_salt("install"),
        );
        child.set_clip_rect(inner);
        child.spacing_mut().item_spacing = vec2(8.0, 8.0);

        let failed = self.install.error.is_some();
        let finished = self.install.receiver.is_none() && !failed;

        child.label(
            egui::RichText::new("BR1 PLAYER")
                .size(11.0)
                .color(theme.muted())
                .strong(),
        );
        child.add_space(4.0);
        child.label(
            egui::RichText::new(if failed {
                "Setup did not finish."
            } else {
                "Getting things ready."
            })
            .size(27.0)
            .color(theme.text())
            .strong(),
        );
        child.add_space(3.0);
        child.label(
            egui::RichText::new(
                "First run only: BR1 Player downloads the tools it uses to find, convert and tag music, plus two measured KEMAR HRTF sets for binaural headphone audio. They are stored in bin/ and %APPDATA%\\DefPlayer\\hrtf and reused every time after that.",
            )
            .size(12.5)
            .color(theme.muted()),
        );
        child.add_space(12.0);

        let time = child.input(|input| input.time);
        for index in 0..deps::count() {
            let state = if finished || index < self.install.tool {
                InstallRow::Done
            } else if index > self.install.tool {
                InstallRow::Waiting
            } else if failed {
                InstallRow::Failed
            } else {
                InstallRow::Current
            };
            Self::install_row(&mut child, index, &theme, state, time);
        }

        child.add_space(8.0);

        let (bar_rect, _) =
            child.allocate_exact_size(vec2(child.available_width(), 7.0), Sense::hover());
        ui::progress_bar(
            child.painter(),
            bar_rect,
            if finished {
                Some(1.0)
            } else {
                self.install.fraction
            },
            theme.accent_a,
            time,
            &theme,
        );
        child.add_space(4.0);

        let row_width = child.available_width();
        let (line_rect, _) = child.allocate_exact_size(vec2(row_width, 16.0), Sense::hover());
        let font = FontId::proportional(11.0);
        child.painter().text(
            pos2(line_rect.left(), line_rect.center().y),
            Align2::LEFT_CENTER,
            ui::ellipsize(
                child.painter(),
                &self.install.status,
                &font,
                if failed {
                    theme.danger()
                } else {
                    theme.muted()
                },
                row_width - 52.0,
            ),
            font.clone(),
            if failed {
                theme.danger()
            } else {
                theme.muted()
            },
        );
        if let Some(fraction) = self.install.fraction.filter(|_| !failed) {
            child.painter().text(
                pos2(line_rect.right(), line_rect.center().y),
                Align2::RIGHT_CENTER,
                format!("{:.0}%", fraction * 100.0),
                font,
                theme.faint(),
            );
        }

        if failed {
            child.add_space(2.0);
            child.label(
                egui::RichText::new(
                    self.install
                        .error
                        .clone()
                        .unwrap_or_else(|| "Unknown error".to_string()),
                )
                .size(11.0)
                .color(theme.danger()),
            );
        }

        child.add_space(10.0);
        let mut retry = false;
        let mut skip = false;
        if failed {
            child.horizontal(|ui| {
                if ui::pill_button_sized(
                    ui,
                    130.0,
                    "Try again",
                    true,
                    &theme,
                    GlassSlot::Setup,
                )
                .clicked()
                {
                    retry = true;
                }
                if ui::pill_button_sized(
                    ui,
                    230.0,
                    "Continue without downloads",
                    false,
                    &theme,
                    GlassSlot::Setup,
                )
                .clicked()
                {
                    skip = true;
                }
            });
        }

        if retry {
            self.start_install();
        } else if skip {
            if let Err(err) = deps::mark_first_run_complete() {
                self.notice(format!("Could not save first-run state: {err}"), true);
            }
            self.finish_install();
        }
    }

    fn install_row(ui: &mut Ui, index: usize, theme: &Theme, state: InstallRow, time: f64) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        let painter = ui.painter();
        let center = pos2(rect.left() + 7.0, rect.center().y);

        match state {
            InstallRow::Done => {
                let color = theme::with_alpha(theme.accent_b, 225);
                painter.circle_filled(center, 6.0, color);
                painter.line_segment(
                    [center + vec2(-2.6, 0.2), center + vec2(-0.6, 2.2)],
                    Stroke::new(1.5_f32, Color32::from_rgb(8, 10, 14)),
                );
                painter.line_segment(
                    [center + vec2(-0.6, 2.2), center + vec2(2.8, -2.4)],
                    Stroke::new(1.5_f32, Color32::from_rgb(8, 10, 14)),
                );
            }
            InstallRow::Current => {
                let pulse = 0.5 + 0.5 * (time * 3.0).sin() as f32;
                painter.circle_stroke(
                    center,
                    5.0 + pulse * 0.8,
                    Stroke::new(1.6_f32, theme.accent_a),
                );
            }
            InstallRow::Waiting => {
                painter.circle_stroke(
                    center,
                    5.0,
                    Stroke::new(1.0_f32, theme::with_alpha(theme.text(), 60)),
                );
            }
            InstallRow::Failed => {
                painter.circle_stroke(center, 5.2, Stroke::new(1.6_f32, theme.danger()));
                painter.line_segment(
                    [center + vec2(-2.0, -2.0), center + vec2(2.0, 2.0)],
                    Stroke::new(1.4_f32, theme.danger()),
                );
                painter.line_segment(
                    [center + vec2(2.0, -2.0), center + vec2(-2.0, 2.0)],
                    Stroke::new(1.4_f32, theme.danger()),
                );
            }
        }

        let label = deps::label(index);
        painter.text(
            pos2(rect.left() + 20.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.5),
            match state {
                InstallRow::Waiting => theme.faint(),
                _ => theme.text(),
            },
        );

        let (word, color) = match state {
            InstallRow::Done => ("installed", theme.faint()),
            InstallRow::Current => ("downloading", theme.muted()),
            InstallRow::Waiting => ("waiting", theme.faint()),
            InstallRow::Failed => ("failed", theme.danger()),
        };
        painter.text(
            pos2(rect.right(), rect.center().y),
            Align2::RIGHT_CENTER,
            word,
            FontId::proportional(11.0),
            color,
        );
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
        const SOURCE_W: u32 = 1_920;
        const SOURCE_H: u32 = 1_080;
        // Blur is computed on a small copy and then scaled back up by the GPU;
        // that keeps the slider responsive even for 4K photos.
        const BLUR_W: u32 = 240;
        const BLUR_H: u32 = 135;

        let path_changed = self.background_loaded != self.config.ui.background_image;
        let target_blur = self.config.ui.background_blur.clamp(0.0, 1.0);

        // No photo configured: drop the cached copy so a blur change can never
        // bring a cleared photo back on screen.
        if self.config.ui.background_image.is_none() {
            self.background_loaded = None;
            self.background_source = None;
            self.background_tex = None;
            self.background_blur_applied = target_blur;
            return;
        }

        if !path_changed && (target_blur - self.background_blur_applied).abs() <= 0.004 {
            return;
        }

        if path_changed {
            self.background_loaded = self.config.ui.background_image.clone();
            self.background_source = match &self.config.ui.background_image {
                Some(path) => match image::open(path) {
                    Ok(image) => Some(
                        image
                            .resize_to_fill(
                                SOURCE_W,
                                SOURCE_H,
                                image::imageops::FilterType::Triangle,
                            )
                            .to_rgba8(),
                    ),
                    Err(error) => {
                        self.notice(format!("Background photo: {error}"), true);
                        None
                    }
                },
                None => None,
            };
        }
        self.background_blur_applied = target_blur;

        self.background_tex = self.background_source.as_ref().map(|source| {
            let color = if target_blur <= 0.004 {
                egui::ColorImage::from_rgba_unmultiplied(
                    [source.width() as usize, source.height() as usize],
                    source.as_raw(),
                )
            } else {
                let mut small = image::imageops::resize(
                    source,
                    BLUR_W,
                    BLUR_H,
                    image::imageops::FilterType::Triangle,
                );
                // 8 px at the small scale is roughly 64 px on the dashboard.
                small = image::imageops::blur(&small, target_blur * 8.0);
                egui::ColorImage::from_rgba_unmultiplied(
                    [BLUR_W as usize, BLUR_H as usize],
                    small.as_raw(),
                )
            };
            ctx.load_texture("background-photo", color, TextureOptions::LINEAR)
        });
    }

    fn choose_background(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp", "gif"])
            .set_title("Choose a background photo")
            .pick_file()
        {
            self.config.ui.background_image = Some(path);
            self.config.ui.background_video = None;
            self.background_loaded = None;
            self.video_error = None;
            self.save_config();
        }
    }

    fn choose_background_video(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "webm", "mkv", "mov", "m4v", "avi"])
            .set_title("Choose a background video")
            .pick_file()
        {
            self.config.ui.background_video = Some(path);
            self.config.ui.background_image = None;
            self.background_loaded = None;
            self.video_error = None;
            if self.config.ui.background_dim > 0.60 {
                self.config.ui.background_dim = 0.35;
                self.notice(
                    "Background video added. DIM was lowered so the video is visible.",
                    false,
                );
            }
            self.save_config();
        }
    }

    /// Drives the looping video background (decoded by the bundled ffmpeg).
    fn sync_video(&mut self, ctx: &egui::Context) {
        let path = self.config.ui.background_video.clone();
        self.video
            .sync(path.as_ref(), self.config.ui.background_blur, ctx);
        self.video.update();
        if let Some(error) = self.video.take_error() {
            self.video_error = Some(error.clone());
            self.notice(error, true);
        }
        if self.video.is_running() && self.video.texture().is_some() {
            self.video_error = None;
        }
        if self.video.is_running() {
            ctx.request_repaint();
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

        let visualizer_visible = self.config.ui.panels[PanelKind::Visualizer.index()].visible;
        if self.is_playing() && visualizer_visible {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    /* ---------- input ---------- */

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.view == View::Installing {
            return;
        }
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
        self.plus_rect = Some(plus_rect);
        let settings_rect = Rect::from_center_size(
            pos2(rect.right() - margin - side * 0.5, cy),
            vec2(side, side),
        );

        // The mini player lives on the left, next to the add button, with the
        // search bar filling the gap up to the settings button. It only shows
        // when the full player panel is off screen; with both visible the
        // mini player is redundant.
        let mini_w = (rect.width() * 0.36).clamp(250.0, 520.0);
        let mini_height = (side + 4.0).clamp(40.0, 56.0);
        let mini_rect = if self.config.ui.panels[PanelKind::Player.index()].visible {
            None
        } else {
            Some(Rect::from_min_max(
                pos2(plus_rect.right() + 16.0, cy - mini_height * 0.5),
                pos2(plus_rect.right() + 16.0 + mini_w, cy + mini_height * 0.5),
            ))
        };

        let search_left = mini_rect
            .map(|mini| mini.right() + 18.0)
            .unwrap_or(plus_rect.right() + 16.0);
        let search_right = (settings_rect.left() - 16.0)
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
            "Components and layout"
        } else {
            "Edit layout / add components"
        });
        if plus_response.clicked() {
            self.add_menu_open = !self.add_menu_open;
            if self.add_menu_open {
                self.layout_edit = true;
                self.add_menu_pos = plus_rect.left_bottom() + vec2(0.0, 8.0);
                self.settings_open = false;
                // The click that opened the menu must not close it again on the
                // same frame, otherwise it only flashes on screen.
                self.add_menu_ignore_click = true;
            }
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

        if let Some(mini_rect) = mini_rect {
            self.mini_player(ui, mini_rect, &theme);
        }

        if ui::circle_button(
            ui,
            settings_rect,
            "settings-open",
            Glyph::Gear,
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
        let skip_size = (cover_size * 0.66).clamp(24.0, 34.0);
        let cover = Rect::from_min_size(
            pos2(rect.left(), rect.center().y - cover_size * 0.5),
            vec2(cover_size, cover_size),
        );
        // Transport cluster on the right edge: prev | play | next.
        let gap = 4.0;
        let next_rect = Rect::from_center_size(
            pos2(rect.right() - skip_size * 0.5, rect.center().y),
            vec2(skip_size, skip_size),
        );
        let play_rect = Rect::from_center_size(
            pos2(next_rect.left() - gap - play_size * 0.5, rect.center().y),
            vec2(play_size, play_size),
        );
        let prev_rect = Rect::from_center_size(
            pos2(play_rect.left() - gap - skip_size * 0.5, rect.center().y),
            vec2(skip_size, skip_size),
        );
        let text_left = cover.right() + 10.0;
        let text_right = prev_rect.left() - 10.0;

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
        let (response, requested) =
            ui::slider(ui, seek_rect, "mini-seek", fraction, duration > 0.0, theme);
        self.note_seek_drag(response.dragged());
        if let Some(value) = requested {
            self.seek_to(value);
        }

        let cover_resp = ui
            .interact(cover, ui.scope_id().with("mini-cover"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if cover_resp.clicked() {
            self.toggle_play();
        }

        if ui::glyph_button(
            ui,
            prev_rect,
            "mini-prev",
            Glyph::Prev,
            false,
            theme,
            GlassSlot::Transport,
        )
        .clicked()
        {
            self.previous();
        }
        if ui::glyph_button(
            ui,
            next_rect,
            "mini-next",
            Glyph::Next,
            false,
            theme,
            GlassSlot::Transport,
        )
        .clicked()
        {
            self.advance(1, false);
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
            PanelKind::Visualizer => self.visualizer_panel(&mut child),
            PanelKind::SpatialAudio => self.spatial_panel(&mut child),
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

        // Sort chip for the song list, with the track count beside it.
        let sort_label = format!("SORT · {}", self.config.ui.track_sort.label());
        let chip_width = sort_label.len() as f32 * 6.0 + 26.0;
        let show_chip = head.width() >= chip_width + 170.0;
        let count_right = if show_chip {
            head.right() - chip_width - 12.0
        } else {
            head.right()
        };
        painter.text(
            pos2(count_right, head.top() + 9.0),
            Align2::RIGHT_CENTER,
            if count == 0 {
                "—".to_string()
            } else {
                format!("{count} · {}", format_duration(duration))
            },
            FontId::proportional(10.0),
            theme.faint(),
        );
        if show_chip {
            let chip_rect = Rect::from_min_size(
                pos2(head.right() - chip_width, head.top() + 1.0),
                vec2(chip_width, 20.0),
            );
            let chip_response = ui::chip(
                ui,
                chip_rect,
                "song-sort",
                &sort_label,
                false,
                &theme,
                GlassSlot::OptionChip,
            )
            .on_hover_text("Change the song order");
            if chip_response.clicked() {
                self.sort_menu_open = true;
                self.sort_menu_pos = chip_rect.left_bottom() + vec2(0.0, 6.0);
                // The opening click must not close the menu again this frame.
                self.sort_menu_ignore_click = true;
            }
        }

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
        let mut right_clicked: Option<(usize, egui::Pos2)> = None;
        let mut scroll_to_current = self.scroll_to_current;

        // A landscape box scrolls the tracks sideways as a card strip; a
        // portrait box keeps the classic vertical list. Scrolling is
        // wheel-only: the bars are hidden.
        let avail = ui.available_size();
        let scroll_rect = ui.available_rect_before_wrap();
        if avail.x > avail.y {
            egui::ScrollArea::horizontal()
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .id_salt("current-playlist-strip")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
                        let strip_h = avail.y.max(36.0);
                        for index in visible.iter().copied() {
                            let track = &self.tracks[index];
                            let active = current == Some(index);
                            let card_w = (strip_h * 2.1).clamp(160.0, 300.0);
                            let (rect, response) =
                                ui.allocate_exact_size(vec2(card_w, strip_h), Sense::click());
                            let card = rect.shrink2(vec2(1.0, 2.0));
                            let painter = ui.painter();

                            let glass = crate::glass::paint_row(
                                painter,
                                card,
                                &theme,
                                response.hovered(),
                                active,
                            );
                            if !glass {
                                if active {
                                    theme::gradient_rect(
                                        painter,
                                        card,
                                        9.0,
                                        theme::with_alpha(theme.accent_a, 54),
                                        theme::with_alpha(theme.accent_b, 22),
                                        theme.angle,
                                    );
                                } else if response.hovered() {
                                    painter.rect_filled(
                                        card,
                                        9.0,
                                        Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                                    );
                                }
                            }
                            if active {
                                // Keep the running song obvious above the
                                // glass face too: a soft tint plus an accent
                                // marker on the left edge.
                                if glass {
                                    theme::gradient_rect(
                                        painter,
                                        card,
                                        9.0,
                                        theme::with_alpha(theme.accent_a, 62),
                                        theme::with_alpha(theme.accent_b, 26),
                                        theme.angle,
                                    );
                                }
                                painter.rect_filled(
                                    Rect::from_min_max(
                                        pos2(card.left() + 3.0, card.top() + 9.0),
                                        pos2(card.left() + 6.0, card.bottom() - 9.0),
                                    ),
                                    1.5,
                                    theme.accent_b,
                                );
                            }

                            let cover_side = (strip_h - 14.0).clamp(28.0, 64.0);
                            let cover = Rect::from_center_size(
                                pos2(card.left() + 5.0 + cover_side * 0.5, card.center().y),
                                vec2(cover_side, cover_side),
                            );
                            ui::paint_cover(ui, cover, self.thumbs[index].as_ref(), 7.0, &theme);

                            let text_left = cover.right() + 9.0;
                            let time_width = if card_w >= 190.0 { 36.0 } else { 0.0 };
                            let text_width = (card.right() - 6.0 - time_width - text_left)
                                .max(20.0);
                            let title_font = FontId::proportional(12.0);
                            let artist_font = FontId::proportional(10.5);
                            if strip_h >= 58.0 {
                                painter.text(
                                    pos2(text_left, card.center().y - 8.0),
                                    Align2::LEFT_CENTER,
                                    ui::ellipsize(
                                        painter,
                                        &track.title,
                                        &title_font,
                                        theme.text(),
                                        text_width,
                                    ),
                                    title_font,
                                    theme.text(),
                                );
                                painter.text(
                                    pos2(text_left, card.center().y + 8.0),
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
                            } else {
                                painter.text(
                                    pos2(text_left, card.center().y),
                                    Align2::LEFT_CENTER,
                                    ui::ellipsize(
                                        painter,
                                        &track.title,
                                        &title_font,
                                        theme.text(),
                                        text_width,
                                    ),
                                    title_font,
                                    theme.text(),
                                );
                            }
                            if time_width > 0.0 {
                                painter.text(
                                    pos2(card.right() - 6.0, card.center().y),
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
                            if response.secondary_clicked() {
                                right_clicked =
                                    Some((index, response.interact_pointer_pos().unwrap_or(rect.center())));
                            }
                        }
                    });
                });
        } else {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
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
                    if active {
                        // The running song stays obvious above the glass face
                        // too: a soft tint plus an accent edge marker.
                        if glass {
                            theme::gradient_rect(
                                painter,
                                row,
                                9.0,
                                theme::with_alpha(theme.accent_a, 62),
                                theme::with_alpha(theme.accent_b, 26),
                                theme.angle,
                            );
                        }
                        painter.rect_filled(
                            Rect::from_min_max(
                                pos2(row.left() + 3.0, row.top() + 9.0),
                                pos2(row.left() + 6.0, row.bottom() - 9.0),
                            ),
                            1.5,
                            theme.accent_b,
                        );
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
                    if response.secondary_clicked() {
                        right_clicked =
                            Some((index, response.interact_pointer_pos().unwrap_or(rect.center())));
                    }
                }
            });
        }

        // The songs blur out into shadow as they near the border instead of
        // a straight cut. Mostly very subtle; only the last few pixels
        // deepen, so almost no content is lost.
        if avail.x > avail.y {
            let fade = 26.0_f32;
            let left_band = Rect::from_min_size(
                scroll_rect.min,
                vec2(fade.min(scroll_rect.width()), scroll_rect.height()),
            );
            paint_edge_fade(ui.painter(), left_band, true, false);
            let right_band = Rect::from_min_max(
                pos2(
                    (scroll_rect.right() - fade).max(scroll_rect.left()),
                    scroll_rect.top(),
                ),
                scroll_rect.max,
            );
            paint_edge_fade(ui.painter(), right_band, true, true);
        } else {
            let fade = 22.0_f32;
            let top_band = Rect::from_min_size(
                scroll_rect.min,
                vec2(scroll_rect.width(), fade.min(scroll_rect.height())),
            );
            paint_edge_fade(ui.painter(), top_band, false, false);
            let bottom_band = Rect::from_min_max(
                pos2(
                    scroll_rect.left(),
                    (scroll_rect.bottom() - fade).max(scroll_rect.top()),
                ),
                scroll_rect.max,
            );
            paint_edge_fade(ui.painter(), bottom_band, false, true);
        }

        self.scroll_to_current = scroll_to_current;
        if let Some(index) = clicked {
            if self.current == Some(index) {
                self.toggle_play();
            } else {
                self.play_index(index);
            }
        }
        if let Some((index, pos)) = right_clicked {
            // Right-click opens the song's menu; the opening click must not
            // close it again this frame.
            self.track_menu_index = Some(index);
            self.track_menu_pos = pos;
            self.track_menu_open = true;
            self.track_menu_new_open = false;
            self.track_menu_ignore_click = true;
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
        let gap = 10.0;
        let available_w = ui.available_width();
        let available_h = ui.available_height();
        let scroll_rect = ui.available_rect_before_wrap();

        // A landscape box scrolls sideways as a single card strip; a portrait
        // box wraps the cards into a grid that scrolls with the wheel. The
        // bars stay hidden — the wheel does the scrolling.
        let wide = available_w > available_h;
        if wide {
            let card_size = (available_h - 4.0).clamp(54.0, 190.0);
            egui::ScrollArea::horizontal()
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .id_salt("playlist-covers-strip")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(gap, 0.0);
                        for (playlist, name, count, texture) in &entries {
                            if Self::playlist_card(
                                ui,
                                card_size,
                                playlist,
                                name,
                                *count,
                                texture.as_ref(),
                                &active,
                                &theme,
                            ) {
                                selected = Some(playlist.clone());
                            }
                        }
                    });
                });
        } else {
            let tall = available_h > available_w * 0.8;
            let card_size = if tall {
                let columns = (((available_w + gap) / (110.0 + gap)).floor()).max(1.0);
                ((available_w - gap * (columns - 1.0)) / columns).clamp(64.0, 180.0)
            } else {
                (available_h - 4.0).clamp(54.0, 190.0)
            };
            let per_row = (((available_w + gap) / (card_size + gap)).floor() as usize).max(1);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .id_salt("playlist-covers")
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(gap, gap);

                    for row in entries.chunks(per_row) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;

                            for (playlist, name, count, texture) in row {
                                if Self::playlist_card(
                                    ui,
                                    card_size,
                                    playlist,
                                    name,
                                    *count,
                                    texture.as_ref(),
                                    &active,
                                    &theme,
                                ) {
                                    selected = Some(playlist.clone());
                                }
                            }
                        });
                    }
                });
        }

        // Covers blur out into shadow as they near the border instead of a
        // straight cut. Kept subtle so most of the content stays visible.
        if wide {
            let fade = 26.0_f32;
            let left_band = Rect::from_min_size(
                scroll_rect.min,
                vec2(fade.min(scroll_rect.width()), scroll_rect.height()),
            );
            paint_edge_fade(ui.painter(), left_band, true, false);
            let right_band = Rect::from_min_max(
                pos2(
                    (scroll_rect.right() - fade).max(scroll_rect.left()),
                    scroll_rect.top(),
                ),
                scroll_rect.max,
            );
            paint_edge_fade(ui.painter(), right_band, true, true);
        } else {
            let fade = 22.0_f32;
            let top_band = Rect::from_min_size(
                scroll_rect.min,
                vec2(scroll_rect.width(), fade.min(scroll_rect.height())),
            );
            paint_edge_fade(ui.painter(), top_band, false, false);
            let bottom_band = Rect::from_min_max(
                pos2(
                    scroll_rect.left(),
                    (scroll_rect.bottom() - fade).max(scroll_rect.top()),
                ),
                scroll_rect.max,
            );
            paint_edge_fade(ui.painter(), bottom_band, false, true);
        }

        if let Some(playlist) = selected {
            self.activate_playlist(playlist);
        }
    }

    /// One playlist cover card. Returns true when clicked.
    fn playlist_card(
        ui: &mut Ui,
        card_size: f32,
        playlist: &Option<String>,
        name: &str,
        count: usize,
        texture: Option<&TextureHandle>,
        active: &Option<String>,
        theme: &Theme,
    ) -> bool {
        let is_active = *active == *playlist;
        let (rect, response) =
            ui.allocate_exact_size(vec2(card_size, card_size), Sense::click());
        if let Some(texture) = texture {
            ui::paint_cover(ui, rect, Some(texture), 12.0, theme);
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
            theme,
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
                    Stroke::new(1.5_f32, theme::with_alpha(theme.accent_a, 120)),
                    StrokeKind::Inside,
                );
            }
        }

        response.clicked()
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

        // The artwork is the panel's own background image, painted here on top
        // of the card (or the glass face) so nothing tints it: the theme
        // colours the chrome, the cover stays exactly as it was mastered.
        self.paint_player_art(ui.painter(), rect, &theme);

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

        // A blurred dark spot that gradually soaks into the artwork behind
        // the song text and the transport — no box, no hard edges. Stacked
        // radial ramps build a smooth falloff.
        let spot = pos2(rect.center().x, seek_y - 6.0);
        let reach = ((right - left) * 0.78).max(160.0);
        theme::radial_glow(ui.painter(), spot, reach, Color32::BLACK, 36);
        theme::radial_glow(ui.painter(), spot, reach * 0.62, Color32::BLACK, 32);
        theme::radial_glow(ui.painter(), spot, reach * 0.36, Color32::BLACK, 38);

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
        let (response, requested) = ui::slider(
            ui,
            seek_rect,
            "player-seek",
            fraction,
            duration > 0.0,
            &theme,
        );
        self.note_seek_drag(response.dragged());
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
        if ui::glyph_button_solid(
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
        if ui::glyph_button_solid(
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
        if ui::glyph_button_solid(
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
            if ui::chip_solid(
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
            if ui::chip_solid(
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
            if ui::glyph_button_solid(
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

        // Where new downloads land: All music (the root) or a playlist.
        let mut open_target_menu = false;
        let mut target_chip_rect = Rect::NOTHING;
        child.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(egui::RichText::new("SAVE TO").size(9.5).color(theme.muted()));
            let target_label = match &self.download_target {
                Some(name) => name.clone(),
                None => "ALL MUSIC".to_string(),
            };
            let avail = ui.available_width();
            let chip_width =
                (target_label.chars().count() as f32 * 6.6 + 34.0).clamp(64.0, (avail - 8.0).max(64.0));
            let shown = ui::ellipsize(
                ui.painter(),
                &target_label,
                &FontId::proportional(11.0),
                theme.text(),
                chip_width - 16.0,
            );
            let response = ui::chip_button(
                ui,
                &shown,
                self.download_target.is_some(),
                chip_width,
                &theme,
                GlassSlot::OptionChip,
            )
            .on_hover_text("Choose the playlist new downloads are added to");
            target_chip_rect = response.rect;
            if response.clicked() {
                open_target_menu = true;
            }
        });
        if open_target_menu {
            // Opening the playlist picker means "I'm not creating one right
            // now" — hide the inline creator so it never lingers.
            self.download_target_menu_open = true;
            self.download_target_menu_pos = target_chip_rect.left_bottom() + vec2(0.0, 6.0);
            self.download_target_menu_ignore_click = true;
            self.download_new_open = false;
            self.download_new_playlist.clear();
            self.download_status = None;
            self.download_failed = false;
        }

        // Inline creator shown only after "New playlist…" is picked in the menu.
        if self.download_new_open {
            // Escape backs out without creating anything.
            if child.input(|input| input.key_pressed(egui::Key::Escape)) {
                self.download_new_open = false;
                self.download_new_playlist.clear();
                self.download_status = None;
                self.download_failed = false;
            }
        }
        if self.download_new_open {
            child.horizontal(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.download_new_playlist)
                        .hint_text("New playlist name…")
                        .font(FontId::proportional(11.5))
                        .text_color(theme.text())
                        .desired_width((ui.available_width() - 132.0).max(40.0))
                        .margin(egui::Margin::symmetric(6, 4)),
                );
                if self.download_new_focus {
                    response.request_focus();
                    self.download_new_focus = false;
                }
                let mut create =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if ui::chip_button(ui, "CREATE", true, 66.0, &theme, GlassSlot::DownloaderAction)
                    .clicked()
                {
                    create = true;
                }
                if ui::chip_button(ui, "CANCEL", false, 58.0, &theme, GlassSlot::DownloaderAction)
                    .clicked()
                {
                    self.download_new_open = false;
                    self.download_new_playlist.clear();
                    self.download_status = None;
                    self.download_failed = false;
                }
                if create {
                    self.create_download_playlist();
                }
            });
        }

        // Update mode: only meaningful with a concrete playlist target.
        if self.download_target.is_some() {
            child.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(egui::RichText::new("UPDATE").size(9.5).color(theme.muted()));
                let response = ui::chip_button(
                    ui,
                    "SKIP EXISTING",
                    self.download_skip_existing,
                    106.0,
                    &theme,
                    GlassSlot::OptionChip,
                )
                .on_hover_text(
                    "Songs already in this playlist are not downloaded again — only the missing ones.",
                );
                if response.clicked() {
                    self.download_skip_existing = !self.download_skip_existing;
                    self.save_config();
                }
                ui.label(
                    egui::RichText::new(if self.download_skip_existing {
                        "Only songs missing from the playlist download"
                    } else {
                        "Everything downloads, even if already present"
                    })
                    .size(9.0)
                    .color(theme.faint()),
                );
            });
        }

        let destination = self.download_destination_folder();
        let path_text = format!("Saving to  {}", destination.display());
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

    /// Creates the folder typed into the downloader's "new playlist" input
    /// and selects it as the download target.
    fn create_download_playlist(&mut self) {
        let raw = self.download_new_playlist.trim().to_string();
        if raw.is_empty() {
            self.download_status = Some("Type a playlist name first.".to_string());
            self.download_failed = true;
            return;
        }
        let name = download::sanitize_folder(&raw);
        let folder = Self::playlist_folder(&self.library_root, &name);

        if let Err(err) = Self::ensure_folder_writable(&folder) {
            // Keep the input open so the name can be fixed or the folder
            // changed without starting over.
            self.download_status = Some(err);
            self.download_failed = true;
            return;
        }

        self.download_status = None;
        self.download_failed = false;
        self.download_target = Some(name);
        self.download_new_open = false;
        self.download_new_playlist.clear();
        self.save_config();
    }

    /// Menu listing every playlist (plus All music and a new-playlist
    /// option) that the SAVE TO chip in the downloader opens.
    fn download_target_menu(&mut self, ctx: &egui::Context) {
        if !self.download_target_menu_open {
            self.download_target_menu_ignore_click = false;
            return;
        }
        let theme = self.theme;
        let screen = ctx.content_rect();
        let playlists = self.playlists();
        let mut picked: Option<Option<String>> = None;
        let mut new_playlist = false;

        egui::Area::new(Id::unique("download-target-overlay"))
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

                let width = 268.0;
                let list_height = (playlists.len() as f32 * 30.0).min(210.0);
                let height = 26.0 + 34.0 + 30.0 + list_height + 46.0 + 12.0;
                let pos = pos2(
                    self.download_target_menu_pos
                        .x
                        .clamp(screen.left() + 4.0, (screen.right() - width - 4.0).max(screen.left() + 4.0)),
                    self.download_target_menu_pos
                        .y
                        .clamp(screen.top() + 4.0, (screen.bottom() - height - 4.0).max(screen.top() + 4.0)),
                );

                let mut menu_ui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_size(pos, vec2(width, height)))
                        .id_salt("download-target-card"),
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
                        ui.set_min_width(240.0);
                        ui.label(
                            egui::RichText::new("SAVE DOWNLOADS TO")
                                .size(9.5)
                                .color(theme.faint())
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new(
                                "New songs are added straight into the chosen playlist folder.",
                            )
                            .size(9.0)
                            .color(theme.faint()),
                        );
                        ui.add_space(2.0);

                        if ui::menu_toggle_item(
                            ui,
                            "All music",
                            self.download_target.is_none(),
                            &theme,
                        )
                        .clicked()
                        {
                            picked = Some(None);
                        }

                        if !playlists.is_empty() {
                            ui.label(
                                egui::RichText::new("YOUR PLAYLISTS")
                                    .size(9.5)
                                    .color(theme.faint())
                                    .strong(),
                            );
                            egui::ScrollArea::vertical()
                                .max_height(list_height)
                                .show(ui, |ui| {
                                    for (name, count) in &playlists {
                                        let selected =
                                            self.download_target.as_deref() == Some(name.as_str());
                                        if playlist_option_row(ui, name, *count, selected, &theme)
                                            .clicked()
                                        {
                                            picked = Some(Some(name.clone()));
                                        }
                                    }
                                });
                        }

                        if ui::menu_item(ui, "New playlist…", Glyph::Plus, &theme).clicked() {
                            new_playlist = true;
                        }
                    });
                crate::glass::paint_menu_rim(
                    menu_ui.painter(),
                    menu.response.rect,
                    12.0,
                    &theme,
                );
                self.download_target_menu_rect = menu.response.rect;

                let clicked = ui.ctx().input(|input| input.pointer.any_click());
                let pointer = ui.ctx().input(|input| input.pointer.interact_pos());
                if menu_should_close(
                    self.download_target_menu_ignore_click,
                    clicked,
                    pointer,
                    self.download_target_menu_rect,
                ) {
                    self.download_target_menu_open = false;
                }
                self.download_target_menu_ignore_click = false;
            });

        if new_playlist {
            self.download_target_menu_open = false;
            self.download_new_open = true;
            self.download_new_focus = true;
            self.download_new_playlist.clear();
        }
        if let Some(choice) = picked {
            self.download_target = choice;
            self.download_target_menu_open = false;
            // Picking an existing playlist ends the "new playlist" flow.
            self.download_new_open = false;
            self.download_new_playlist.clear();
            self.download_status = None;
            self.download_failed = false;
            self.save_config();
        }
    }

    /// Right-click menu of one song: play it, move it into any playlist
    /// (existing or brand new), or take it back out to All music.
    fn track_menu(&mut self, ctx: &egui::Context) {
        if !self.track_menu_open {
            self.track_menu_ignore_click = false;
            return;
        }
        let Some(index) = self.track_menu_index else {
            self.track_menu_open = false;
            return;
        };
        let Some(track) = self.tracks.get(index) else {
            self.track_menu_open = false;
            return;
        };

        let theme = self.theme;
        let screen = ctx.content_rect();
        let title = track.title.clone();
        let artist = track.artist.clone();
        let in_playlist = track.playlist.clone();
        let playlists: Vec<(String, usize)> = self
            .playlists()
            .into_iter()
            .filter(|(name, _)| in_playlist.as_deref() != Some(name.as_str()))
            .collect();

        let mut play = false;
        let mut move_to: Option<Option<String>> = None;
        let mut new_playlist = false;
        let mut create_and_move = false;
        self.track_menu_row_rects.clear();

        // A stable Area id keeps the text edit's focus while typing;
        // `Id::new` is egui's deterministic constructor for exactly that.
        #[allow(deprecated)]
        let menu_area_id = Id::new("br1-song-menu");
        egui::Area::new(menu_area_id)
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

                let width = 268.0;
                let list_height = (playlists.len() as f32 * 30.0).min(210.0);
                let height = if self.track_menu_new_open {
                    52.0 + 38.0 + 26.0 + 34.0 + 12.0
                } else {
                    52.0
                        + 38.0
                        + 24.0
                        + list_height
                        + 38.0
                        + if in_playlist.is_some() { 38.0 } else { 0.0 }
                        + 12.0
                };
                let pos = pos2(
                    self.track_menu_pos
                        .x
                        .clamp(screen.left() + 4.0, (screen.right() - width - 4.0).max(screen.left() + 4.0)),
                    self.track_menu_pos
                        .y
                        .clamp(screen.top() + 4.0, (screen.bottom() - height - 4.0).max(screen.top() + 4.0)),
                );

                let mut menu_ui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_size(pos, vec2(width, height)))
                        .id_salt("song-menu-card"),
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
                        ui.set_min_width(236.0);
                        ui.label(
                            egui::RichText::new("SONG")
                                .size(9.5)
                                .color(theme.faint())
                                .strong(),
                        );
                        let title_font = FontId::proportional(13.0);
                        let shown_title = ui::ellipsize(
                            ui.painter(),
                            &title,
                            &title_font,
                            theme.text(),
                            212.0,
                        );
                        ui.label(egui::RichText::new(shown_title).size(13.0).color(theme.text()));
                        if !artist.is_empty() && artist != "Unknown artist" {
                            let artist_font = FontId::proportional(9.5);
                            let shown_artist = ui::ellipsize(
                                ui.painter(),
                                &artist,
                                &artist_font,
                                theme.muted(),
                                212.0,
                            );
                            ui.label(
                                egui::RichText::new(shown_artist)
                                    .size(9.5)
                                    .color(theme.muted()),
                            );
                        }

                        if ui::menu_item(ui, "Play", Glyph::Play, &theme).clicked() {
                            play = true;
                        }

                        if self.track_menu_new_open {
                            ui.label(
                                egui::RichText::new("NEW PLAYLIST")
                                    .size(9.5)
                                    .color(theme.faint())
                                    .strong(),
                            );
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.track_menu_new_name)
                                    .hint_text("Playlist name…")
                                    .font(FontId::proportional(12.0))
                                    .text_color(theme.text())
                                    .desired_width((ui.available_width() - 74.0).max(40.0))
                                    .margin(egui::Margin::symmetric(6, 4)),
                            );
                            if self.track_menu_new_focus {
                                response.request_focus();
                                self.track_menu_new_focus = false;
                            }
                            let mut create = response.lost_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter));
                            if ui::chip_button(
                                ui,
                                "MOVE",
                                true,
                                64.0,
                                &theme,
                                GlassSlot::DownloaderAction,
                            )
                            .clicked()
                            {
                                create = true;
                            }
                            if create {
                                create_and_move = true;
                            }
                        } else {
                            ui.label(
                                egui::RichText::new(if in_playlist.is_some() {
                                    "MOVE TO PLAYLIST"
                                } else {
                                    "ADD TO PLAYLIST"
                                })
                                .size(9.5)
                                .color(theme.faint())
                                .strong(),
                            );
                            if playlists.is_empty() {
                                ui.label(
                                    egui::RichText::new("You have no other playlists yet.")
                                        .size(9.0)
                                        .color(theme.faint()),
                                );
                            }
                            egui::ScrollArea::vertical()
                                .max_height(list_height)
                                .show(ui, |ui| {
                                    for (name, count) in &playlists {
                                        let response =
                                            playlist_option_row(ui, name, *count, false, &theme);
                                        self.track_menu_row_rects
                                            .push((name.clone(), response.rect));
                                        if response.clicked() {
                                            move_to = Some(Some(name.clone()));
                                        }
                                    }
                                });
                            if ui::menu_item(ui, "New playlist…", Glyph::Plus, &theme).clicked() {
                                new_playlist = true;
                            }
                            // Nested on purpose: the menu item must always be
                            // laid out and painted before its click state is
                            // read, which a collapsed `&&` would skip.
                            #[allow(clippy::collapsible_if)]
                            if in_playlist.is_some() {
                                if ui::menu_item(ui, "Remove from playlist", Glyph::Close, &theme)
                                    .on_hover_text("Back to All music — the file stays in your library")
                                    .clicked()
                                {
                                    move_to = Some(None);
                                }
                            }
                        }
                    });
                crate::glass::paint_menu_rim(
                    menu_ui.painter(),
                    menu.response.rect,
                    12.0,
                    &theme,
                );
                self.track_menu_rect = menu.response.rect;

                let clicked = ui.ctx().input(|input| input.pointer.any_click());
                let pointer = ui.ctx().input(|input| input.pointer.interact_pos());
                if menu_should_close(
                    self.track_menu_ignore_click,
                    clicked,
                    pointer,
                    self.track_menu_rect,
                ) {
                    self.track_menu_open = false;
                    self.track_menu_new_open = false;
                }
                self.track_menu_ignore_click = false;
            });

        if new_playlist {
            self.track_menu_new_open = true;
            self.track_menu_new_focus = true;
            self.track_menu_new_name.clear();
        }
        if create_and_move {
            let raw = self.track_menu_new_name.trim().to_string();
            if !raw.is_empty() {
                let name = download::sanitize_folder(&raw);
                self.move_track_to_playlist(index, Some(&name));
                self.track_menu_open = false;
                self.track_menu_new_open = false;
            }
        }
        if let Some(choice) = move_to {
            match choice {
                Some(name) => self.move_track_to_playlist(index, Some(&name)),
                None => self.move_track_to_playlist(index, None),
            }
            self.track_menu_open = false;
            self.track_menu_new_open = false;
        }
        if play {
            self.play_index(index);
            self.track_menu_open = false;
            self.track_menu_new_open = false;
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
        let power_rect = Rect::from_center_size(
            pos2(toggle_right - 21.0, head.center().y),
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
                audio.set_eq(&self.config.ui.eq_gains, self.config.ui.eq_active());
            }
            self.save_config();
        }
        let _ = toggle_w;

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
                    audio.set_eq(&self.config.ui.eq_gains, self.config.ui.eq_active());
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
                audio.set_eq(&self.config.ui.eq_gains, self.config.ui.eq_active());
            }
        }
        if stopped {
            self.save_config();
        }
        if dragging {
            ui.ctx().request_repaint();
        }
    }

    fn visualizer_panel(&mut self, ui: &mut Ui) {
        self.visualizer_view(ui);
    }

    fn apply_spatial(&mut self) {
        let active = self.config.ui.spatial_active();
        self.spatial_active = active;
        if let Some(audio) = &mut self.audio {
            audio.set_spatial_enabled(active);
            if !active {
                audio.set_test_mode(TestMode::Off);
            }
            audio.set_source_position(spatial::position_from_pad(
                self.config.ui.spatial_x,
                self.config.ui.spatial_y,
                self.config.ui.spatial_height,
                self.config.ui.room_size,
            ));
            audio.set_room_size(self.config.ui.room_size);
            audio.set_reverb(self.config.ui.reverb);
            audio.set_head_yaw(self.config.ui.head_yaw);
            audio.set_hrtf_enabled(self.config.ui.hrtf_enabled);
        }
    }

    /// Keeps the renderer in sync with the component being added, hidden or
    /// switched off from the menu or the settings.
    fn sync_spatial(&mut self) {
        if self.config.ui.spatial_active() != self.spatial_active {
            self.apply_spatial();
        }
    }

    /// Keeps the equalizer in sync with the component being added, hidden or
    /// switched off: hidden panels never touch playback.
    fn sync_eq(&mut self) {
        let active = self.config.ui.eq_active();
        if active != self.eq_active {
            self.eq_active = active;
            if let Some(audio) = &self.audio {
                audio.set_eq(&self.config.ui.eq_gains, active);
            }
        }
    }

    fn apply_hrtf_set(&mut self) -> bool {
        let id = self.config.ui.hrtf_set.clone();
        let result = self.audio.as_mut().map(|audio| audio.set_hrtf_set(&id));
        match result {
            Some(Ok(())) => true,
            Some(Err(error)) => {
                self.notice(format!("HRTF: {error}"), true);
                false
            }
            None => false,
        }
    }

    fn cycle_hrtf_set(&mut self) {
        let sets = self
            .audio
            .as_ref()
            .map(|audio| audio.hrtf_sets().to_vec())
            .unwrap_or_default();
        if sets.is_empty() {
            return;
        }
        let current = self
            .audio
            .as_ref()
            .map(|audio| audio.hrtf_set_id().to_string())
            .unwrap_or_else(|| self.config.ui.hrtf_set.clone());
        let index = sets.iter().position(|set| set.id == current).unwrap_or(0);
        let next = &sets[(index + 1) % sets.len()];
        let previous = self.config.ui.hrtf_set.clone();
        self.config.ui.hrtf_set = next.id.clone();
        if self.apply_hrtf_set() {
            self.save_config();
        } else {
            self.config.ui.hrtf_set = previous;
        }
    }

    fn slider_row(
        ui: &Ui,
        rect: Rect,
        id: &str,
        label: &str,
        value_text: &str,
        fraction: f32,
        theme: &Theme,
    ) -> (egui::Response, Option<f32>) {
        let painter = ui.painter();
        painter.text(
            pos2(rect.left(), rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(9.5),
            theme.muted(),
        );
        painter.text(
            pos2(rect.right(), rect.center().y),
            Align2::RIGHT_CENTER,
            value_text,
            FontId::proportional(9.5),
            theme.faint(),
        );

        let slider = Rect::from_min_max(
            pos2(rect.left() + 48.0, rect.center().y - 9.0),
            pos2(rect.right() - 52.0, rect.center().y + 9.0),
        );
        ui::slider(ui, slider, id, fraction, true, theme)
    }

    /// Binaural speaker pad: drag to move the object, height shows elevation,
    /// the arrow shows head yaw. The chips switch HRTF/pan, the HRTF set and
    /// the built-in test patterns.
    fn spatial_panel(&mut self, ui: &mut Ui) {
        let theme = self.theme;
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
        let enabled = self.config.ui.spatial_enabled;
        let painter = ui.painter();
        painter.text(
            pos2(head.left(), head.center().y),
            Align2::LEFT_CENTER,
            "SPATIAL AUDIO",
            FontId::proportional(10.5),
            theme.muted(),
        );

        // The component's on/off switch. Off passes the music through untouched.
        let toggle = Rect::from_min_size(
            pos2(head.right() - 46.0, head.center().y - 10.0),
            vec2(46.0, 20.0),
        );
        let hrtf = self.config.ui.hrtf_enabled;
        if enabled {
            painter.text(
                pos2(toggle.left() - 10.0, head.center().y),
                Align2::RIGHT_CENTER,
                if hrtf { "HRTF · BINAURAL" } else { "STEREO · PAN" },
                FontId::proportional(10.0),
                theme.faint(),
            );
        }
        if ui::chip(
            ui,
            toggle,
            "spatial-enable",
            if enabled { "ON" } else { "OFF" },
            enabled,
            &theme,
            GlassSlot::SettingsControl,
        )
        .on_hover_text(
            "Turn the spatial component on or off. It is bypassed while it is off \
             or while the panel is not on the dashboard.",
        )
        .clicked()
        {
            self.config.ui.spatial_enabled = !enabled;
            self.apply_spatial();
            self.save_config();
        }

        let full = ui.available_rect_before_wrap();
        if full.width() < 210.0 || full.height() < 150.0 {
            return;
        }

        if !enabled {
            // Nothing here can move the sound while the component is off, so
            // show a quiet placeholder instead of an inert pad and sliders.
            theme::gradient_rect(
                ui.painter(),
                full,
                14.0,
                theme::with_alpha(theme.panel_soft(), 140),
                theme::with_alpha(theme.panel_bottom(), 190),
                theme.angle,
            );
            ui.painter().text(
                full.center() - vec2(0.0, 18.0),
                Align2::CENTER_CENTER,
                "SPATIAL AUDIO OFF",
                FontId::proportional(11.5),
                theme.muted(),
            );
            let font = FontId::proportional(9.5);
            for (index, text) in [
                "Playback is plain stereo.",
                "Use the ON chip above to bring it back.",
            ]
            .iter()
            .enumerate()
            {
                ui.painter().text(
                    full.center() + vec2(0.0, index as f32 * 14.0),
                    Align2::CENTER_CENTER,
                    ui::ellipsize(
                        ui.painter(),
                        text,
                        &font,
                        theme.faint(),
                        full.width() - 24.0,
                    ),
                    font.clone(),
                    theme.faint(),
                );
            }
            return;
        }

        let gap = 6.0;
        let chip_height = 26.0;
        let chip_width = ((full.width() - gap * 2.0) / 3.0).max(44.0);
        let chip_y = head.bottom() + 4.0;
        let mode_rect = Rect::from_min_size(pos2(full.left(), chip_y), vec2(chip_width, chip_height));
        let set_rect = Rect::from_min_size(
            pos2(mode_rect.right() + gap, chip_y),
            vec2(chip_width, chip_height),
        );
        let test_rect = Rect::from_min_size(
            pos2(set_rect.right() + gap, chip_y),
            vec2(chip_width, chip_height),
        );

        let test_mode = self
            .audio
            .as_ref()
            .map(|audio| audio.test_mode())
            .unwrap_or(TestMode::Off);
        let set_label = self
            .audio
            .as_ref()
            .and_then(|audio| {
                let id = audio.hrtf_set_id();
                audio
                    .hrtf_sets()
                    .iter()
                    .find(|set| set.id == id)
                    .map(|set| set.short.clone())
            })
            .unwrap_or_else(|| "SET".to_string());
        let set_detail = self
            .audio
            .as_ref()
            .and_then(|audio| {
                let id = audio.hrtf_set_id();
                audio
                    .hrtf_sets()
                    .iter()
                    .find(|set| set.id == id)
                    .map(|set| format!("{}\n{}", set.name, set.detail))
            })
            .unwrap_or_default();

        if ui::chip(
            ui,
            mode_rect,
            "spatial-mode",
            if hrtf { "HRTF" } else { "PAN" },
            hrtf,
            &theme,
            GlassSlot::SettingsControl,
        )
        .on_hover_text(
            "HRTF: real binaural rendering (ITD, ILD, pinna and torso cues).\n\
             PAN: the old amplitude panning, kept for A/B comparison.",
        )
        .clicked()
        {
            self.config.ui.hrtf_enabled = !hrtf;
            self.apply_spatial();
            self.save_config();
        }

        let set_response = ui::chip(
            ui,
            set_rect,
            "spatial-set",
            &set_label,
            false,
            &theme,
            GlassSlot::SettingsControl,
        )
        .on_hover_text(format!(
            "{set_detail}\n\nClick: next HRTF set.\nRight-click: rescan the folder.\n\nDrop SOFA files here:\n{}",
            spatial::hrtf_folder().display()
        ));
        if set_response.secondary_clicked() {
            let id = self.audio.as_mut().map(|audio| {
                audio.rescan_hrtf_sets();
                audio.hrtf_set_id().to_string()
            });
            if let Some(id) = id {
                if self.config.ui.hrtf_set != id {
                    self.config.ui.hrtf_set = id;
                    self.save_config();
                }
            }
        } else if set_response.clicked() {
            self.cycle_hrtf_set();
        }

        let test_response = ui::chip(
            ui,
            test_rect,
            "spatial-test",
            test_mode.label(),
            test_mode.active(),
            &theme,
            GlassSlot::SettingsControl,
        )
        .on_hover_text(
            "Test object: front, back, above, below, horizontal orbit, vertical orbit, \
             front/back sweep and left/right sweep.\nRight-click: stop.",
        );
        if test_response.secondary_clicked() {
            if let Some(audio) = &mut self.audio {
                audio.set_test_mode(TestMode::Off);
            }
        } else if test_response.clicked() {
            if let Some(audio) = &mut self.audio {
                audio.set_test_mode(test_mode.next());
            }
        }

        let area = Rect::from_min_max(
            pos2(full.left(), chip_y + chip_height + 6.0),
            full.max,
        );
        if area.width() < 70.0 || area.height() < 110.0 {
            return;
        }

        let row_height = 22.0;
        let row_gap = 2.0;
        let controls_height = row_height * 5.0 + row_gap * 4.0;
        let horizontal = area.width() > area.height() + 150.0;
        let (canvas, controls) = if horizontal {
            let side = (area.height() - 4.0).clamp(70.0, 460.0);
            let canvas = Rect::from_min_size(area.min, vec2(side, side));
            let controls = Rect::from_min_max(
                pos2(canvas.right() + 14.0, area.top()),
                area.max,
            );
            (canvas, controls)
        } else {
            if area.height() < controls_height + 76.0 {
                return;
            }
            let side = area
                .width()
                .min(area.height() - controls_height - 6.0)
                .clamp(70.0, 460.0);
            let canvas = Rect::from_min_size(
                pos2(area.center().x - side * 0.5, area.top()),
                vec2(side, side),
            );
            let controls = Rect::from_min_max(
                pos2(area.left(), canvas.bottom() + 6.0),
                area.max,
            );
            (canvas, controls)
        };

        let center = canvas.center();
        let radius = (canvas.width().min(canvas.height()) * 0.5 - 10.0).max(20.0);

        theme::gradient_rect(
            ui.painter(),
            canvas,
            14.0,
            theme::with_alpha(theme.panel_soft(), 200),
            theme::with_alpha(theme.panel_bottom(), 220),
            theme.angle,
        );
        for ring in 1..=3 {
            ui.painter().circle_stroke(
                center,
                radius * ring as f32 / 3.0,
                Stroke::new(
                    1.0_f32,
                    Color32::from_rgba_unmultiplied(255, 255, 255, 16),
                ),
            );
        }
        for axis in 0..2 {
            let (from, to) = if axis == 0 {
                (
                    pos2(canvas.left() + 8.0, center.y),
                    pos2(canvas.right() - 8.0, center.y),
                )
            } else {
                (
                    pos2(center.x, canvas.top() + 8.0),
                    pos2(center.x, canvas.bottom() - 8.0),
                )
            };
            ui.painter().line_segment(
                [from, to],
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 12)),
            );
        }

        // Head yaw arrow.
        let yaw = self.config.ui.head_yaw.to_radians();
        let forward = vec2(yaw.sin(), -yaw.cos());
        let arrow_length = (radius * 0.22).max(14.0);
        let arrow_end = center + forward * arrow_length;
        let back = -forward;
        let perp = vec2(-forward.y, forward.x);
        let arrow_color = theme::with_alpha(theme.text(), 150);
        ui.painter().line_segment(
            [center, arrow_end],
            Stroke::new(1.4_f32, arrow_color),
        );
        ui.painter().line_segment(
            [arrow_end, arrow_end + back * 6.0 + perp * 3.6],
            Stroke::new(1.3_f32, arrow_color),
        );
        ui.painter().line_segment(
            [arrow_end, arrow_end + back * 6.0 - perp * 3.6],
            Stroke::new(1.3_f32, arrow_color),
        );

        let fallback = spatial::position_from_pad(
            self.config.ui.spatial_x,
            self.config.ui.spatial_y,
            self.config.ui.spatial_height,
            self.config.ui.room_size,
        );
        let position = if test_mode.active() {
            self.audio
                .as_ref()
                .map(|audio| audio.spatial_position())
                .unwrap_or(fallback)
        } else {
            fallback
        };
        let scene = spatial::scene(position, self.config.ui.head_yaw, 0.0, 0.0);
        let (x, y) = spatial::pad_from_position(position, self.config.ui.room_size);
        let planar = pos2(center.x + x * radius, center.y - y * radius);
        let height_px = (position[1] / 2.5).clamp(-1.0, 1.0) * radius * 0.55;
        let speaker = pos2(
            planar.x,
            (planar.y - height_px)
                .clamp(canvas.top() + 16.0, canvas.bottom() - 28.0),
        );

        // Horizontal projection and height stick.
        ui.painter().line_segment(
            [center, planar],
            Stroke::new(1.2_f32, theme::with_alpha(theme.accent_b, 130)),
        );
        if height_px.abs() > 1.0 {
            ui.painter().line_segment(
                [planar, speaker],
                Stroke::new(1.6_f32, theme::with_alpha(theme.accent_a, 180)),
            );
        }
        ui.painter().circle_filled(
            planar,
            3.2,
            theme::with_alpha(theme.accent_a, 110),
        );
        ui.painter().circle_stroke(
            planar,
            6.0,
            Stroke::new(1.0_f32, theme::with_alpha(theme.accent_a, 90)),
        );

        // Listener.
        ui.painter()
            .circle_filled(center, 7.0, theme::with_alpha(theme.text(), 210));
        ui.painter().circle_stroke(
            center,
            11.0,
            Stroke::new(1.0_f32, theme::with_alpha(theme.text(), 90)),
        );
        ui::paint_glyph(
            ui.painter(),
            center,
            12.0,
            Glyph::Volume,
            theme.bg(),
        );

        // Source. A pulsing halo marks an active test pattern.
        if test_mode.active() {
            let time = ui.input(|input| input.time);
            let pulse = (time * 3.0).sin() as f32 * 0.5 + 0.5;
            ui.painter().circle_stroke(
                speaker,
                16.0 + pulse * 5.0,
                Stroke::new(
                    1.0_f32,
                    theme::with_alpha(theme.accent_b, (70.0 - pulse * 35.0) as u8),
                ),
            );
        }
        ui.painter()
            .circle_filled(speaker, 12.0, theme::with_alpha(theme.accent_a, 200));
        ui.painter().circle_stroke(
            speaker,
            14.0,
            Stroke::new(1.2_f32, theme::with_alpha(theme.accent_b, 200)),
        );
        ui::paint_glyph(ui.painter(), speaker, 14.0, Glyph::Volume, theme.text());

        let label = format!(
            "{}AZ {:+.0}° · EL {:+.0}° · {:.2} m · H {:+.2} m",
            if test_mode.active() { "TEST · " } else { "" },
            scene.azimuth_deg,
            scene.elevation_deg,
            scene.distance,
            position[1],
        );
        ui.painter().text(
            pos2(canvas.left() + 10.0, canvas.bottom() - 8.0),
            Align2::LEFT_BOTTOM,
            label,
            FontId::proportional(9.0),
            theme.faint(),
        );

        let response = ui.interact(
            canvas,
            Id::unique("spatial-room"),
            Sense::click_and_drag(),
        );
        if (response.drag_started() || response.clicked()) && test_mode.active() {
            if let Some(audio) = &mut self.audio {
                audio.set_test_mode(TestMode::Off);
            }
        }
        if response.dragged() || response.clicked() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let dx = (pointer.x - center.x) / radius;
                let dy = (center.y - pointer.y) / radius;
                let length = (dx * dx + dy * dy).sqrt();
                let (nx, ny) = if length > 1.0 {
                    (dx / length, dy / length)
                } else {
                    (dx, dy)
                };
                self.config.ui.spatial_x = nx.clamp(-1.0, 1.0);
                self.config.ui.spatial_y = ny.clamp(-1.0, 1.0);
                self.apply_spatial();
            }
        }
        if response.drag_stopped() || response.clicked() {
            self.save_config();
        }

        let block_top = if horizontal {
            (controls.top() + (controls.height() - controls_height) * 0.5)
                .max(controls.top())
        } else {
            controls.top()
        };
        let row_rect = |index: usize| {
            Rect::from_min_size(
                pos2(
                    controls.left(),
                    block_top + index as f32 * (row_height + row_gap),
                ),
                vec2(controls.width(), row_height),
            )
        };

        let height_fraction = ((self.config.ui.spatial_height + 2.5) / 5.0).clamp(0.0, 1.0);
        let (height_response, requested) = Self::slider_row(
            ui,
            row_rect(0),
            "spatial-height",
            "HEIGHT",
            &format!("{:+.2} m", self.config.ui.spatial_height),
            height_fraction,
            &theme,
        );
        if let Some(fraction) = requested {
            self.config.ui.spatial_height = -2.5 + fraction * 5.0;
            self.apply_spatial();
        }
        if height_response.drag_stopped() || height_response.clicked() {
            self.save_config();
        }

        let head_fraction = ((self.config.ui.head_yaw + 180.0) / 360.0).clamp(0.0, 1.0);
        let (head_response, requested) = Self::slider_row(
            ui,
            row_rect(1),
            "spatial-head",
            "HEAD",
            &format!("{:+.0}°", self.config.ui.head_yaw),
            head_fraction,
            &theme,
        );
        if let Some(fraction) = requested {
            self.config.ui.head_yaw = -180.0 + fraction * 360.0;
            self.apply_spatial();
        }
        if head_response.drag_stopped() || head_response.clicked() {
            self.save_config();
        }

        let manual_position = spatial::position_from_pad(
            self.config.ui.spatial_x,
            self.config.ui.spatial_y,
            self.config.ui.spatial_height,
            self.config.ui.room_size,
        );
        let horizontal = (manual_position[0] * manual_position[0]
            + manual_position[2] * manual_position[2])
            .sqrt();
        let height_abs = manual_position[1].abs();
        let max_horizontal = (self.config.ui.room_size * 0.5)
            .max(spatial::MIN_DISTANCE + 0.1);
        let min_distance = (spatial::MIN_DISTANCE * spatial::MIN_DISTANCE
            + height_abs * height_abs)
            .sqrt();
        let max_distance = (max_horizontal * max_horizontal + height_abs * height_abs)
            .sqrt()
            .max(min_distance + 0.001);
        let current_distance =
            (horizontal * horizontal + height_abs * height_abs).sqrt();
        let distance_fraction = ((current_distance - min_distance)
            / (max_distance - min_distance))
            .clamp(0.0, 1.0);
        let (distance_response, requested) = Self::slider_row(
            ui,
            row_rect(2),
            "spatial-distance",
            "DIST",
            &format!("{current_distance:.2} m"),
            distance_fraction,
            &theme,
        );
        if let Some(fraction) = requested {
            let target = min_distance + fraction * (max_distance - min_distance);
            let target_horizontal = (target * target - height_abs * height_abs)
                .max(0.0)
                .sqrt()
                .clamp(spatial::MIN_DISTANCE, max_horizontal);
            let radius = (target_horizontal / max_horizontal).clamp(0.0, 1.0);
            let length = (self.config.ui.spatial_x * self.config.ui.spatial_x
                + self.config.ui.spatial_y * self.config.ui.spatial_y)
                .sqrt();
            if length > 1e-3 {
                self.config.ui.spatial_x *= radius / length;
                self.config.ui.spatial_y *= radius / length;
            } else {
                self.config.ui.spatial_x = 0.0;
                self.config.ui.spatial_y = radius;
            }
            self.apply_spatial();
        }
        if distance_response.drag_stopped() || distance_response.clicked() {
            self.save_config();
        }

        let room_fraction = ((self.config.ui.room_size - 2.0) / 22.0).clamp(0.0, 1.0);
        let (room_response, requested) = Self::slider_row(
            ui,
            row_rect(3),
            "spatial-room-size",
            "ROOM",
            &format!("{:.1} m", self.config.ui.room_size),
            room_fraction,
            &theme,
        );
        if let Some(fraction) = requested {
            self.config.ui.room_size = 2.0 + fraction * 22.0;
            self.apply_spatial();
        }
        if room_response.drag_stopped() || room_response.clicked() {
            self.save_config();
        }

        let (reverb_response, requested) = Self::slider_row(
            ui,
            row_rect(4),
            "spatial-reverb",
            "REVERB",
            &format!("{:.0}%", self.config.ui.reverb * 100.0),
            self.config.ui.reverb,
            &theme,
        );
        if let Some(fraction) = requested {
            self.config.ui.reverb = fraction;
            self.apply_spatial();
        }
        if reverb_response.drag_stopped() || reverb_response.clicked() {
            self.save_config();
        }

        if test_mode.active() {
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
        // Components never come closer to each other than this.
        let gap = self.config.ui.component_gap.max(0.0);
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
            // Smooth Android-home-screen dragging: the component follows the
            // pointer exactly, slides around its siblings and never overlaps
            // them. The grid snap happens once, animated, on release.
            if move_response.drag_started() {
                let grab = move_response
                    .interact_pointer_pos()
                    .map(|pointer| pointer - rect.min)
                    .unwrap_or_else(|| rect.size() * 0.5);
                self.layout_drag = Some(LayoutDrag {
                    index: kind.index(),
                    grab,
                    resizing: false,
                });
                self.layout_anim = None;
            }
            if move_response.dragged() {
                let grab = self
                    .layout_drag
                    .as_ref()
                    .filter(|drag| !drag.resizing && drag.index == kind.index())
                    .map(|drag| drag.grab)
                    .unwrap_or_else(|| rect.size() * 0.5);
                let proposed = match move_response.interact_pointer_pos() {
                    Some(pointer) => Rect::from_min_size(pointer - grab, rect.size()),
                    None => Rect::from_min_size(rect.min + move_response.drag_delta(), rect.size()),
                };
                let resolved = resolve_layout_rect(proposed, &others, gap, content);
                let panel = &mut self.config.ui.panels[kind.index()];
                panel.x = (resolved.left() - content.left()) / content.width();
                panel.y = (resolved.top() - content.top()) / content.height();
                panel.w = resolved.width() / content.width();
                panel.h = resolved.height() / content.height();
                let (targets_x, targets_y) =
                    layout_snap_targets(&others, content, gap, resolved.size());
                guide_x = near_alignment(resolved.left(), &targets_x);
                guide_y = near_alignment(resolved.top(), &targets_y);
            }
            if move_response.drag_stopped() {
                let current = self.panel_rect(content, kind);
                let (targets_x, targets_y) =
                    layout_snap_targets(&others, content, gap, current.size());
                let snapped = resolve_layout_rect(
                    Rect::from_min_size(
                        pos2(
                            snap_or_grid(current.left(), &targets_x),
                            snap_or_grid(current.top(), &targets_y),
                        ),
                        current.size(),
                    ),
                    &others,
                    gap,
                    content,
                );
                self.layout_drag = None;
                self.layout_anim = Some(LayoutAnim {
                    index: kind.index(),
                    from: current.min.to_vec2(),
                    to: snapped.min.to_vec2(),
                    resizing: false,
                    start: Instant::now(),
                });
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
            if resize_response.drag_started() {
                self.layout_drag = Some(LayoutDrag {
                    index: kind.index(),
                    grab: Vec2::ZERO,
                    resizing: true,
                });
                self.layout_anim = None;
            }
            if resize_response.dragged() {
                let corner = resize_response
                    .interact_pointer_pos()
                    .unwrap_or(rect.right_bottom());
                let proposed = Rect::from_min_max(
                    rect.min,
                    pos2(
                        corner.x.max(rect.left() + 80.0),
                        corner.y.max(rect.top() + 60.0),
                    ),
                );
                let resized = clamp_layout_resize(proposed, &others, gap, content);
                let panel = &mut self.config.ui.panels[kind.index()];
                panel.w = resized.width() / content.width();
                panel.h = resized.height() / content.height();
                let (targets_x, targets_y) = layout_resize_targets(&others, content, gap);
                guide_x = near_alignment(resized.right(), &targets_x);
                guide_y = near_alignment(resized.bottom(), &targets_y);
            }
            if resize_response.drag_stopped() {
                let current = self.panel_rect(content, kind);
                let (targets_x, targets_y) = layout_resize_targets(&others, content, gap);
                let target = clamp_layout_resize(
                    Rect::from_min_max(
                        current.min,
                        pos2(
                            snap_or_grid(current.right(), &targets_x),
                            snap_or_grid(current.bottom(), &targets_y),
                        ),
                    ),
                    &others,
                    gap,
                    content,
                );
                self.layout_drag = None;
                self.layout_anim = Some(LayoutAnim {
                    index: kind.index(),
                    from: current.size(),
                    to: target.size(),
                    resizing: true,
                    start: Instant::now(),
                });
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

    /// Glides a just-dropped component from its drag position to the snapped
    /// one, then saves the layout. Runs every frame so the glide also
    /// finishes after the user leaves layout mode.
    fn advance_layout_anim(&mut self, ctx: &egui::Context) {
        let Some(anim) = self.layout_anim.as_ref() else {
            return;
        };
        let (index, from, to, resizing, start) =
            (anim.index, anim.from, anim.to, anim.resizing, anim.start);
        let screen = ctx.content_rect();
        let (_, frame_rect) = Self::layout_rects(screen);
        let content = Self::dashboard_content(frame_rect);
        if content.width() <= 0.0 || content.height() <= 0.0 {
            self.layout_anim = None;
            return;
        }

        let progress = (start.elapsed().as_secs_f32() / 0.24).clamp(0.0, 1.0);
        // Ease-out quartic: a very gentle landing on the snapped position.
        let rest = 1.0 - progress;
        let eased = 1.0 - rest * rest * rest * rest;
        let value = from + (to - from) * eased;
        let panel = &mut self.config.ui.panels[index];
        if resizing {
            panel.w = (value.x / content.width()).max(0.02);
            panel.h = (value.y / content.height()).max(0.02);
        } else {
            panel.x = (value.x - content.left()) / content.width();
            panel.y = (value.y - content.top()) / content.height();
        }

        if progress >= 1.0 {
            self.layout_anim = None;
            self.save_config();
        } else {
            ctx.request_repaint();
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

    /// Drops a component into the first free spot of the dashboard grid.
    fn place_panel(&mut self, kind: PanelKind) {
        let index = kind.index();
        let w = self.config.ui.panels[index].w.clamp(0.18, 0.62);
        let h = self.config.ui.panels[index].h.clamp(0.14, 0.58);
        let margin = 0.012;

        let others: Vec<(f32, f32, f32, f32)> = PANEL_KINDS
            .iter()
            .filter(|other| other.index() != index)
            .filter(|other| self.config.ui.panels[other.index()].visible)
            .map(|other| {
                let panel = self.config.ui.panels[other.index()];
                (panel.x, panel.y, panel.w, panel.h)
            })
            .collect();

        let overlaps = |x: f32, y: f32| {
            others.iter().any(|(ox, oy, ow, oh)| {
                x < ox + ow + margin
                    && x + w + margin > *ox
                    && y < oy + oh + margin
                    && y + h + margin > *oy
            })
        };

        let mut placement = None;
        let mut y = 0.01;
        while y + h <= 0.99 && placement.is_none() {
            let mut x = 0.01;
            while x + w <= 0.99 {
                if !overlaps(x, y) {
                    placement = Some((x, y));
                    break;
                }
                x += 0.02;
            }
            y += 0.02;
        }

        let (x, y) = placement.unwrap_or((0.34, 0.34));
        let panel = &mut self.config.ui.panels[index];
        panel.x = x;
        panel.y = y;
        panel.w = w;
        panel.h = h;
        panel.visible = true;
    }

    fn add_menu(&mut self, ctx: &egui::Context) {
        if !self.add_menu_open {
            self.add_menu_ignore_click = false;
            return;
        }
        let theme = self.theme;
        let screen = ctx.content_rect();

        let mut action: Option<AddMenuAction> = None;
        egui::Area::new(Id::unique("component-menu-overlay"))
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

                let height = 26.0 + PANEL_KINDS.len() as f32 * 34.0 + 46.0;
                let mut menu_ui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_size(
                            self.add_menu_pos,
                            vec2(268.0, height),
                        ))
                        .id_salt("component-menu-card"),
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
                        ui.set_min_width(236.0);
                        ui.label(
                            egui::RichText::new("COMPONENTS")
                                .size(9.5)
                                .color(theme.faint())
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new(
                                "Click to drop in or hide. Drag panels to move, corner dot to resize.",
                            )
                            .size(9.0)
                            .color(theme.faint()),
                        );
                        ui.add_space(2.0);

                        for kind in PANEL_KINDS {
                            let visible = self.config.ui.panels[kind.index()].visible;
                            if ui::menu_toggle_item(ui, kind.title(), visible, &theme).clicked() {
                                action = Some(AddMenuAction::Toggle(kind));
                            }
                        }

                        ui.add_space(4.0);
                        if ui::menu_item(ui, "Reset layout", Glyph::Sliders, &theme).clicked() {
                            action = Some(AddMenuAction::Reset);
                        }
                        ui.label(
                            egui::RichText::new("DONE in the top bar leaves layout mode.")
                                .size(9.0)
                                .color(theme.faint()),
                        );
                    });
                crate::glass::paint_menu_rim(
                    menu_ui.painter(),
                    menu.response.rect,
                    12.0,
                    &theme,
                );
                self.add_menu_rect = menu.response.rect;

                let clicked = ui.ctx().input(|input| input.pointer.any_click());
                let pointer = ui.ctx().input(|input| input.pointer.interact_pos());
                if menu_should_close(
                    self.add_menu_ignore_click,
                    clicked,
                    pointer,
                    self.add_menu_rect,
                ) {
                    self.add_menu_open = false;
                }
                self.add_menu_ignore_click = false;
            });

        match action {
            Some(AddMenuAction::Toggle(kind)) => {
                let was_visible = self.config.ui.panels[kind.index()].visible;
                if was_visible {
                    self.config.ui.panels[kind.index()].visible = false;
                } else {
                    self.place_panel(kind);
                }
                self.layout_edit = true;
                self.save_config();
            }
            Some(AddMenuAction::Reset) => {
                self.config.ui.panels = UiConfig::default().panels;
                self.layout_edit = true;
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
                                if response.secondary_clicked() {
                                    // Same right-click menu as the song list.
                                    self.track_menu_index = Some(*index);
                                    self.track_menu_pos = response
                                        .interact_pointer_pos()
                                        .unwrap_or(row.center());
                                    self.track_menu_open = true;
                                    self.track_menu_new_open = false;
                                    self.track_menu_ignore_click = true;
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
                self.activate_playlist(self.tracks[index].playlist.clone());
                self.play_index(index);
            } else if let Some((name, _)) = playlist_hits.get(index) {
                self.activate_playlist(Some(name.clone()));
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

    /// The song sort menu, opened from the chip in the current playlist.
    fn sort_menu(&mut self, ctx: &egui::Context) {
        if !self.sort_menu_open {
            self.sort_menu_ignore_click = false;
            return;
        }
        let theme = self.theme;
        let screen = ctx.content_rect();
        let mut picked: Option<TrackSort> = None;

        egui::Area::new(Id::unique("song-sort-overlay"))
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

                let height = 26.0 + 5.0 * 34.0 + 12.0;
                let mut menu_ui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(Rect::from_min_size(
                            self.sort_menu_pos,
                            vec2(224.0, height),
                        ))
                        .id_salt("song-sort-card"),
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
                        ui.set_min_width(200.0);
                        ui.label(
                            egui::RichText::new("SORT SONGS")
                                .size(9.5)
                                .color(theme.faint())
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new(
                                "Newest and oldest follow each file's date. Most used counts every play.",
                            )
                            .size(9.0)
                            .color(theme.faint()),
                        );
                        ui.add_space(2.0);

                        for sort in [
                            TrackSort::Added,
                            TrackSort::Newest,
                            TrackSort::Oldest,
                            TrackSort::Title,
                            TrackSort::MostUsed,
                        ] {
                            if ui::menu_toggle_item(
                                ui,
                                sort.label(),
                                self.config.ui.track_sort == sort,
                                &theme,
                            )
                            .clicked()
                            {
                                picked = Some(sort);
                            }
                        }
                    });
                crate::glass::paint_menu_rim(
                    menu_ui.painter(),
                    menu.response.rect,
                    12.0,
                    &theme,
                );
                self.sort_menu_rect = menu.response.rect;

                let clicked = ui.ctx().input(|input| input.pointer.any_click());
                let pointer = ui.ctx().input(|input| input.pointer.interact_pos());
                if menu_should_close(
                    self.sort_menu_ignore_click,
                    clicked,
                    pointer,
                    self.sort_menu_rect,
                ) {
                    self.sort_menu_open = false;
                }
                self.sort_menu_ignore_click = false;
            });

        if let Some(sort) = picked {
            self.config.ui.track_sort = sort;
            self.sort_menu_open = false;
            self.rebuild_visible();
            self.save_config();
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
                    pos2(inner.right() - 14.0, inner.top() + 14.0),
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
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui::chip_button(
                ui,
                "CHOOSE VIDEO",
                false,
                112.0,
                theme,
                GlassSlot::SettingsControl,
            )
            .clicked()
            {
                self.choose_background_video();
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
                self.config.ui.background_video = None;
                self.video_error = None;
                self.save_config();
            }
        });
        if self.config.ui.background_video.is_some() {
            ui.label(
                egui::RichText::new(
                    "A looping video replaces the photo. It is decoded at 1280x720 with the bundled ffmpeg and dimmed by the slider below. The blur slider applies to both photos and videos.",
                )
                .size(9.5)
                .color(theme.faint()),
            );
        }
        let video_label = self
            .config
            .ui
            .background_video
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "No background video selected".to_string());
        ui.add(
            egui::Label::new(
                egui::RichText::new(video_label)
                    .size(10.0)
                    .color(theme.faint()),
            )
            .truncate(),
        );
        if let Some(error) = &self.video_error {
            ui.add(
                egui::Label::new(egui::RichText::new(error).size(9.5).color(theme.danger())).wrap(),
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
            .add(egui::Slider::new(&mut dim, 0.0..=1.0).text("Background dim"))
            .changed()
        {
            self.config.ui.background_dim = dim;
            self.refresh_theme(ui.ctx());
            self.save_config();
        }
        let mut blur = self.config.ui.background_blur;
        if ui
            .add(egui::Slider::new(&mut blur, 0.0..=1.0).text("Background blur"))
            .changed()
        {
            self.config.ui.background_blur = blur;
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
            self.config.ui.background_blur = defaults.background_blur;
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
                "Drag a component and it follows your pointer smoothly, sliding around its siblings and gliding onto the 8 px grid when you let go. Components never overlap or come closer than the spacing below. The red corner buttons hide a panel; the top-left + brings hidden ones back.",
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
        ui.label(
            egui::RichText::new("Component spacing")
                .size(11.0)
                .color(theme.muted())
                .strong(),
        );
        ui.label(
            egui::RichText::new(
                "The minimum distance kept between components. While you drag or resize, a component stops this far from its neighbours instead of covering them.",
            )
            .size(10.5)
            .color(theme.faint()),
        );
        let mut gap = self.config.ui.component_gap;
        if ui
            .add(egui::Slider::new(&mut gap, 0.0..=60.0).text("minimum gap (px)"))
            .changed()
        {
            self.config.ui.component_gap = gap;
            self.save_config();
        }

        ui.add_space(8.0);
        Self::settings_heading(
            ui,
            theme,
            "LAYOUT PRESETS",
            "Save the whole look — arrangement, colours, glass and background — under a name, then switch between saved looks with one click.",
        );
        ui.horizontal(|ui| {
            let field_width = (ui.available_width() - 104.0).max(120.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.layout_preset_name)
                    .hint_text("Preset name")
                    .desired_width(field_width),
            );
            if ui::pill_button_sized(
                ui,
                92.0,
                "Save",
                true,
                theme,
                GlassSlot::SettingsControl,
            )
            .clicked()
            {
                let name = self.layout_preset_name.trim().to_string();
                if name.is_empty() {
                    self.notice("Give the preset a name first.", true);
                } else {
                    match config::save_layout(&name, &self.config) {
                        Ok(()) => self.notice(format!("Saved layout “{name}”."), false),
                        Err(err) => self.notice(err, true),
                    }
                    self.layout_preset_name.clear();
                }
            }
        });

        let presets = config::list_layouts();
        if presets.is_empty() {
            ui.label(
                egui::RichText::new("No saved layouts yet — arrange everything, then save.")
                    .size(10.0)
                    .color(theme.faint()),
            );
        } else {
            for name in presets {
                let (row, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
                let painter = ui.painter();
                painter.text(
                    pos2(row.left() + 2.0, row.center().y),
                    Align2::LEFT_CENTER,
                    ui::ellipsize(
                        painter,
                        &name,
                        &FontId::proportional(12.0),
                        theme.text(),
                        (row.width() - 180.0).max(40.0),
                    ),
                    FontId::proportional(12.0),
                    theme.text(),
                );
                let delete_rect = Rect::from_center_size(
                    pos2(row.right() - 41.0, row.center().y),
                    vec2(70.0, 22.0),
                );
                let apply_rect = Rect::from_center_size(
                    pos2(row.right() - 119.0, row.center().y),
                    vec2(70.0, 22.0),
                );
                if ui::chip(
                    ui,
                    apply_rect,
                    &format!("preset-apply-{name}"),
                    "APPLY",
                    false,
                    theme,
                    GlassSlot::SettingsControl,
                )
                .on_hover_text("Switch to this saved look")
                .clicked()
                {
                    match config::load_layout(&name) {
                        Some(preset) => {
                            config::apply_layout(&mut self.config.ui, &preset);
                            self.refresh_theme(ui.ctx());
                            // Re-pick the photo/video of the preset.
                            self.background_loaded = None;
                            self.save_config();
                            self.notice(format!("Layout “{name}” applied."), false);
                        }
                        None => self.notice("That preset could not be read.", true),
                    }
                }
                if ui::chip(
                    ui,
                    delete_rect,
                    &format!("preset-delete-{name}"),
                    "DELETE",
                    false,
                    theme,
                    GlassSlot::SettingsControl,
                )
                .on_hover_text("Remove this preset")
                .clicked()
                {
                    if config::delete_layout(&name) {
                        self.notice(format!("Layout “{name}” removed."), false);
                    }
                }
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
                "Drag a panel to move it; it glides onto the grid and its siblings' edges when dropped. Pull the bright dot in its bottom-right corner to resize. Esc leaves layout mode.",
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
        let tools_dir = download::tools_dir();
        let tools_ready = deps::tools_ready();
        let hrtf_ready = deps::hrtf_ready();
        ui.label(
            egui::RichText::new(match tools_dir {
                Some(ref path) if tools_ready => {
                    format!("Tools found in {}", path.display())
                }
                _ => "The downloader tools are not fully installed.".to_string(),
            })
            .size(10.5)
            .color(if tools_ready {
                theme.muted()
            } else {
                theme.danger()
            }),
        );
        if !hrtf_ready {
            ui.add_space(3.0);
            ui.label(
                egui::RichText::new("Measured KEMAR HRTF sets are not installed.")
                    .size(10.5)
                    .color(theme.muted()),
            );
        }
        if !deps::assets_ready() {
            ui.add_space(6.0);
            if ui::pill_button_sized(
                ui,
                190.0,
                "Download missing files",
                true,
                theme,
                GlassSlot::SettingsControl,
            )
            .clicked()
            {
                self.settings_open = false;
                self.start_install();
            }
        }
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
                "Every track lands under the music folder. SAVE TO in the downloader panel \
                 sends new songs straight into a playlist, and right-clicking a song moves \
                 it between playlists. Playlist links still fetch their own cover.",
            )
            .size(10.5)
            .color(theme.faint()),
        );
    }

    fn about_settings(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.label(
            egui::RichText::new("BR1 PLAYER")
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
    Toggle(PanelKind),
    Reset,
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
        self.poll_install(ctx);
        self.sync_spatial();
        self.sync_eq();
        self.tick(ctx);
        self.handle_keys(ctx);
        // A frame without a seek drag means the handle was released: restore
        // the music. Deliberately one frame late, so the last seek of a drag
        // is already silent.
        if !std::mem::take(&mut self.seek_drag_seen) {
            if let Some(audio) = &mut self.audio {
                audio.set_seeking(false);
            }
        }
        self.autosave(ctx);
        self.ensure_textures(ctx);
        self.ensure_playlist_textures(ctx);
        self.ensure_background(ctx);
        self.sync_video(ctx);
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

        // A light gradient only near the bottom, so the image stays clearly
        // visible while the lower text keeps some contrast.
        theme::gradient_rect(
            painter,
            Rect::from_min_max(
                pos2(art.left(), art.top() + art.height() * 0.55),
                art.max,
            ),
            art_radius,
            Color32::TRANSPARENT,
            Color32::from_rgba_unmultiplied(5, 5, 8, 120),
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
                self.video.texture().or(self.background_tex.as_ref()),
            );
            // The player artwork is painted by the player panel itself, on top
            // of the panel card, so the themed card colour can never wash over
            // the image.
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

        self.advance_layout_anim(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::TRANSPARENT))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                if self.view == View::Installing {
                    self.install_screen(ui, rect);
                    return;
                }
                if self.view == View::Setup {
                    self.setup_screen(ui, rect);
                    return;
                }

                let (top, frame_rect) = Self::layout_rects(rect);

                if real_capable {
                    // Glass stays active with a video background too; the
                    // decoded video is painted in the background pass, so the
                    // glass refracts it instead of covering it.
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
                    self.sort_menu(ctx);
                    self.download_target_menu(ctx);
                    self.track_menu(ctx);
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
/// How close an edge must come to a snap target before it magnetizes. Kept
/// small so landing feels gentle instead of sticky.
const LAYOUT_SNAP: f32 = 4.0;
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

/// Positions a dropped component may snap to: the frame edges, the centre and
/// the sides of its siblings, always kept `gap` apart.
fn layout_snap_targets(
    others: &[Rect],
    content: Rect,
    gap: f32,
    size: Vec2,
) -> (Vec<f32>, Vec<f32>) {
    let safe = content.shrink(LAYOUT_SAFE_PAD);
    let mut x = vec![
        safe.left(),
        safe.center().x - size.x * 0.5,
        safe.right() - size.x,
    ];
    let mut y = vec![
        safe.top(),
        safe.center().y - size.y * 0.5,
        safe.bottom() - size.y,
    ];
    for other in others {
        x.extend([other.left() - gap, other.right() + gap]);
        y.extend([other.top() - gap, other.bottom() + gap]);
    }
    (x, y)
}

/// Edges a resized component may snap its bottom-right corner to.
fn layout_resize_targets(others: &[Rect], content: Rect, gap: f32) -> (Vec<f32>, Vec<f32>) {
    let safe = content.shrink(LAYOUT_SAFE_PAD);
    let mut x = vec![safe.right()];
    let mut y = vec![safe.bottom()];
    for other in others {
        x.push(other.left() - gap);
        y.push(other.top() - gap);
    }
    (x, y)
}

/// Snaps to the closest target when one is within range, otherwise rounds to
/// the layout grid.
fn snap_or_grid(value: f32, targets: &[f32]) -> f32 {
    let mut best = None;
    let mut best_delta = LAYOUT_SNAP;
    for &target in targets {
        let delta = (target - value).abs();
        if delta <= best_delta {
            best = Some(target);
            best_delta = delta;
        }
    }
    best.unwrap_or((value / LAYOUT_GRID).round() * LAYOUT_GRID)
}

/// Returns the guide line position when an edge sits close to a snap target.
fn near_alignment(value: f32, targets: &[f32]) -> Option<f32> {
    targets
        .iter()
        .copied()
        .find(|target| (target - value).abs() <= 1.5)
}

/// Pushes `rect` out of every sibling (kept `gap` away) and back inside the
/// content area. The push always takes the shortest exit, so a dragged
/// component slides around its neighbours instead of covering them.
fn resolve_layout_rect(mut rect: Rect, others: &[Rect], gap: f32, bounds: Rect) -> Rect {
    for _ in 0..4 {
        let mut moved = false;
        for other in others {
            let zone = other.expand(gap);
            let overlap_x = rect.left() < zone.right() && rect.right() > zone.left();
            let overlap_y = rect.top() < zone.bottom() && rect.bottom() > zone.top();
            if !overlap_x || !overlap_y {
                continue;
            }
            let exit_right = zone.right() - rect.left();
            let exit_left = rect.right() - zone.left();
            let exit_down = zone.bottom() - rect.top();
            let exit_up = rect.bottom() - zone.top();
            let push_x = if exit_right <= exit_left {
                exit_right
            } else {
                -exit_left
            };
            let push_y = if exit_down <= exit_up {
                exit_down
            } else {
                -exit_up
            };
            if push_x.abs() <= push_y.abs() {
                rect = rect.translate(vec2(push_x, 0.0));
            } else {
                rect = rect.translate(vec2(0.0, push_y));
            }
            moved = true;
        }
        if !moved {
            break;
        }
    }
    let max_x = (bounds.right() - rect.width()).max(bounds.left());
    let max_y = (bounds.bottom() - rect.height()).max(bounds.top());
    Rect::from_min_size(
        pos2(
            rect.left().clamp(bounds.left(), max_x),
            rect.top().clamp(bounds.top(), max_y),
        ),
        rect.size(),
    )
}

/// Clamps a resized component (anchored at its top-left) so the bottom-right
/// corner never crosses into a sibling or out of the content area.
fn clamp_layout_resize(rect: Rect, others: &[Rect], gap: f32, bounds: Rect) -> Rect {
    let safe = bounds.shrink(LAYOUT_SAFE_PAD);
    let mut right = rect.right().min(safe.right()).max(rect.left());
    let mut bottom = rect.bottom().min(safe.bottom()).max(rect.top());
    for other in others {
        let zone = other.expand(gap);
        let overlaps_y = rect.top() < zone.bottom() && rect.bottom() > zone.top();
        let overlaps_x = rect.left() < zone.right() && rect.right() > zone.left();
        if overlaps_y && zone.left() > rect.left() {
            right = right.min(zone.left());
        }
        if overlaps_x && zone.top() > rect.top() {
            bottom = bottom.min(zone.top());
        }
    }
    Rect::from_min_max(rect.min, pos2(right, bottom))
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
    // One-direction scroll areas (the horizontal card strips) should react
    // to the mouse wheel without requiring a tilted wheel or Shift.
    style.always_scroll_the_only_direction = true;
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}

fn menu_should_close(
    ignore_click: bool,
    clicked: bool,
    pointer: Option<egui::Pos2>,
    rect: Rect,
) -> bool {
    if ignore_click || !clicked {
        return false;
    }
    match pointer {
        Some(pos) => !rect.contains(pos),
        None => false,
    }
}

/// One playlist row of the download-target and song menus: name on the
/// left, song count on the right, highlight when selected.
fn playlist_option_row(
    ui: &mut Ui,
    name: &str,
    count: usize,
    selected: bool,
    theme: &Theme,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let painter = ui.painter();
    if response.hovered() || selected {
        painter.rect_filled(
            rect,
            CornerRadius::from(9.0),
            Color32::from_rgba_unmultiplied(255, 255, 255, if selected { 30 } else { 18 }),
        );
    }

    let name_font = FontId::proportional(12.0);
    let name_color = if selected { theme.text() } else { theme.muted() };
    let label = ui::ellipsize(painter, name, &name_font, name_color, rect.width() - 66.0);
    painter.text(
        pos2(rect.left() + 14.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        name_font.clone(),
        name_color,
    );
    painter.text(
        pos2(rect.right() - 12.0, rect.center().y),
        Align2::RIGHT_CENTER,
        if count == 1 {
            "1 song".to_string()
        } else {
            format!("{count} songs")
        },
        FontId::proportional(9.0),
        theme.faint(),
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Content dims into shadow near a scroll border instead of getting a
/// straight cut. Two stacked near-black ramps keep the transition free of a
/// visible start line: the inner part is barely there, only the last few
/// pixels at the edge deepen enough to hide the clip. `edge_on_low_side`
/// picks whether the border sits at the band's low (left/top) or high
/// (right/bottom) end.
fn paint_edge_fade(painter: &egui::Painter, band: Rect, horizontal: bool, edge_on_low_side: bool) {
    let thickness = if horizontal {
        band.width()
    } else {
        band.height()
    };
    if thickness <= 1.0 {
        return;
    }

    let outer = (thickness * 0.4).min(9.0);
    let strong = Color32::from_black_alpha(105);
    let mid = Color32::from_black_alpha(40);

    let (edge_strip, inner_strip) = if horizontal {
        if edge_on_low_side {
            let split = band.right() - outer;
            (
                Rect::from_min_max(pos2(split, band.top()), band.max),
                Rect::from_min_max(band.min, pos2(split, band.bottom())),
            )
        } else {
            let split = band.left() + outer;
            (
                Rect::from_min_size(band.min, vec2(outer, band.height())),
                Rect::from_min_max(pos2(split, band.top()), band.max),
            )
        }
    } else if edge_on_low_side {
        let split = band.bottom() - outer;
        (
            Rect::from_min_max(pos2(band.left(), split), band.max),
            Rect::from_min_max(band.min, pos2(band.right(), split)),
        )
    } else {
        let split = band.top() + outer;
        (
            Rect::from_min_size(band.min, vec2(band.width(), outer)),
            Rect::from_min_max(pos2(band.left(), split), band.max),
        )
    };

    if horizontal {
        if edge_on_low_side {
            theme::gradient_rect(painter, edge_strip, 0.0, mid, strong, 0.0);
            theme::gradient_rect(painter, inner_strip, 0.0, Color32::TRANSPARENT, mid, 0.0);
        } else {
            theme::gradient_rect(painter, edge_strip, 0.0, strong, mid, 0.0);
            theme::gradient_rect(painter, inner_strip, 0.0, mid, Color32::TRANSPARENT, 0.0);
        }
    } else if edge_on_low_side {
        theme::gradient_rect(painter, edge_strip, 0.0, mid, strong, 90.0);
        theme::gradient_rect(painter, inner_strip, 0.0, Color32::TRANSPARENT, mid, 90.0);
    } else {
        theme::gradient_rect(painter, edge_strip, 0.0, strong, mid, 90.0);
        theme::gradient_rect(painter, inner_strip, 0.0, mid, Color32::TRANSPARENT, 90.0);
    }
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

    fn frame_input(screen: Rect, time: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            focused: true,
            events,
            ..Default::default()
        }
    }

    fn click(
        ctx: &egui::Context,
        app: &mut PlayerApp,
        screen: Rect,
        pos: egui::Pos2,
        time: f64,
    ) {
        let frames = [
            frame_input(screen, time, vec![egui::Event::PointerMoved(pos)]),
            frame_input(
                screen,
                time + 0.05,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }],
            ),
            frame_input(
                screen,
                time + 0.1,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            ),
        ];
        for frame in frames {
            run_frame(ctx, app, frame, screen);
        }
    }

    #[test]
    fn component_menu_stays_open_after_the_opening_click() {
        let ctx = egui::Context::default();
        let mut app = PlayerApp::new(&ctx, Vec::new(), false, false, None, false);
        app.view = View::Dashboard;
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(1180.0, 760.0));

        // Let the top bar lay out once so we know where the + button is.
        run_frame(&ctx, &mut app, frame_input(screen, 0.4, Vec::new()), screen);
        let plus = app.plus_rect.expect("plus button rect").center();

        click(&ctx, &mut app, screen, plus, 1.0);
        assert!(app.add_menu_open, "the + button should open the menu");
        assert!(app.layout_edit, "opening the menu enters layout mode");

        // Idle frames must not close it (this is the reported bug).
        for step in 0..5 {
            run_frame(
                &ctx,
                &mut app,
                frame_input(screen, 2.0 + step as f64 * 0.1, Vec::new()),
                screen,
            );
        }
        assert!(app.add_menu_open, "the menu closed by itself");

        // Clicking outside the menu closes it.
        click(&ctx, &mut app, screen, pos2(900.0, 700.0), 3.0);
        assert!(!app.add_menu_open, "outside click should close the menu");
    }
}

#[cfg(test)]
mod playlist_feature_tests {
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

    fn frame_input(screen: Rect, time: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            focused: true,
            events,
            ..Default::default()
        }
    }

    fn press(
        ctx: &egui::Context,
        app: &mut PlayerApp,
        screen: Rect,
        pos: egui::Pos2,
        time: f64,
        button: egui::PointerButton,
    ) {
        let frames = [
            frame_input(screen, time, vec![egui::Event::PointerMoved(pos)]),
            frame_input(
                screen,
                time + 0.05,
                vec![egui::Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    modifiers: Default::default(),
                }],
            ),
            frame_input(
                screen,
                time + 0.1,
                vec![egui::Event::PointerButton {
                    pos,
                    button,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            ),
        ];
        for frame in frames {
            run_frame(ctx, app, frame, screen);
        }
    }

    fn click(ctx: &egui::Context, app: &mut PlayerApp, screen: Rect, pos: egui::Pos2, time: f64) {
        press(ctx, app, screen, pos, time, egui::PointerButton::Primary);
    }

    fn right_click(
        ctx: &egui::Context,
        app: &mut PlayerApp,
        screen: Rect,
        pos: egui::Pos2,
        time: f64,
    ) {
        press(ctx, app, screen, pos, time, egui::PointerButton::Secondary);
    }

    fn test_track(path: PathBuf, playlist: Option<&str>) -> Track {
        let title = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Unknown title")
            .to_string();
        Track {
            path,
            title,
            artist: "Test artist".to_string(),
            album: String::new(),
            duration: 120.0,
            thumb: None,
            art: None,
            folder: None,
            playlist: playlist.map(str::to_string),
            added: 0,
        }
    }

    /// A hermetic app on a small temporary library, with the current
    /// playlist panel parked at its default spot on the dashboard.
    fn app_with_library(root: &std::path::Path) -> (egui::Context, PlayerApp) {
        let ctx = egui::Context::default();
        let mut app = PlayerApp::new(&ctx, Vec::new(), false, false, None, false);
        app.view = View::Dashboard;
        app.library_root = root.to_path_buf();
        app.manager.settings.root = root.to_path_buf();
        // Pin the layout and sort so the row positions are predictable
        // whatever the real saved settings of this machine say.
        app.config.ui.panels[PanelKind::CurrentPlaylist.index()] =
            UiConfig::default().panels[PanelKind::CurrentPlaylist.index()];
        app.config.ui.track_sort = TrackSort::Added;
        // The player loads the real user config, which may point at a
        // playlist; tests must start from "All music" to be reproducible.
        app.download_target = None;
        (ctx, app)
    }

    fn wait_for_scan(ctx: &egui::Context, app: &mut PlayerApp) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while app.scanning && Instant::now() < deadline {
            app.poll_scan(ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!app.scanning, "the library refresh should finish");
    }

    #[test]
    fn right_click_moves_a_song_into_a_playlist() {
        let root = std::env::temp_dir().join("br1-player-move-ui-test");
        let _ = std::fs::remove_dir_all(&root);
        let mix = root.join("Mix");
        std::fs::create_dir_all(&mix).unwrap();
        std::fs::write(root.join("Song One.mp3"), b"x").unwrap();
        std::fs::write(mix.join("Existing.mp3"), b"x").unwrap();

        let (ctx, mut app) = app_with_library(&root);
        app.tracks = vec![
            test_track(root.join("Song One.mp3"), None),
            test_track(mix.join("Existing.mp3"), Some("Mix")),
        ];
        app.thumbs = vec![None; 2];
        app.rebuild_visible();

        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(1180.0, 760.0));
        run_frame(&ctx, &mut app, frame_input(screen, 0.5, Vec::new()), screen);

        // The first song row: a 48px-tall interactive rect in the left panel.
        let row = ctx
            .interactive_rects_last_pass()
            .into_iter()
            .filter(|rect| rect.height() == 48.0 && rect.left() < 300.0 && rect.width() > 100.0)
            .min_by_key(|rect| rect.top() as i32)
            .expect("the song list should be interactive");

        right_click(&ctx, &mut app, screen, row.center(), 1.0);
        assert!(app.track_menu_open, "right-click should open the song menu");
        assert_eq!(app.track_menu_index, Some(0));

        // The menu lists every other playlist; its rows record their rects
        // (same pattern as plus_rect) so the click lands precisely.
        let playlist_row = app
            .track_menu_row_rects
            .iter()
            .find(|(name, _)| name == "Mix")
            .map(|(_, rect)| *rect)
            .expect("the Mix playlist should be listed in the menu");

        click(&ctx, &mut app, screen, playlist_row.center(), 2.0);
        assert!(!app.track_menu_open, "picking a playlist closes the menu");
        assert!(
            mix.join("Song One.mp3").is_file(),
            "the file should land in the playlist folder"
        );
        assert!(!root.join("Song One.mp3").exists());

        wait_for_scan(&ctx, &mut app);
        let moved = app
            .tracks
            .iter()
            .find(|track| track.path == mix.join("Song One.mp3"))
            .expect("the moved song should still be in the library");
        assert_eq!(moved.playlist.as_deref(), Some("Mix"));
    }

    #[test]
    fn song_menu_closes_on_an_outside_click() {
        let root = std::env::temp_dir().join("br1-player-song-menu-close");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Song One.mp3"), b"x").unwrap();

        let (ctx, mut app) = app_with_library(&root);
        app.tracks = vec![test_track(root.join("Song One.mp3"), None)];
        app.thumbs = vec![None; 1];
        app.rebuild_visible();

        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(1180.0, 760.0));
        run_frame(&ctx, &mut app, frame_input(screen, 0.5, Vec::new()), screen);

        let row = ctx
            .interactive_rects_last_pass()
            .into_iter()
            .filter(|rect| rect.height() == 48.0 && rect.left() < 300.0 && rect.width() > 100.0)
            .min_by_key(|rect| rect.top() as i32)
            .expect("the song list should be interactive");

        right_click(&ctx, &mut app, screen, row.center(), 1.0);
        assert!(app.track_menu_open);

        // Idle frames must not close it by themselves.
        for step in 0..3 {
            run_frame(
                &ctx,
                &mut app,
                frame_input(screen, 1.5 + step as f64 * 0.1, Vec::new()),
                screen,
            );
        }
        assert!(app.track_menu_open, "the menu closed by itself");

        // A click outside the card does.
        click(&ctx, &mut app, screen, pos2(900.0, 700.0), 2.0);
        assert!(!app.track_menu_open, "outside click should close the menu");
    }

    #[test]
    fn moving_a_song_keeps_its_play_count_and_refreshes_the_library() {
        let root = std::env::temp_dir().join("br1-player-move-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Mix")).unwrap();
        std::fs::write(root.join("Song.mp3"), b"x").unwrap();

        let (ctx, mut app) = app_with_library(&root);
        app.tracks = vec![test_track(root.join("Song.mp3"), None)];
        app.thumbs = vec![None; 1];
        app.rebuild_visible();
        let old_key = root.join("Song.mp3").to_string_lossy().to_string();
        app.config.ui.track_plays.insert(old_key.clone(), 7);

        app.move_track_to_playlist(0, Some("Mix"));

        let moved = root.join("Mix").join("Song.mp3");
        assert!(moved.is_file(), "the file should have moved");
        assert!(!root.join("Song.mp3").exists());
        let new_key = moved.to_string_lossy().to_string();
        assert_eq!(app.config.ui.track_plays.get(&new_key), Some(&7));
        assert!(app.config.ui.track_plays.get(&old_key).is_none());

        wait_for_scan(&ctx, &mut app);
        assert_eq!(app.tracks.len(), 1);
        assert_eq!(app.tracks[0].playlist.as_deref(), Some("Mix"));
        assert_eq!(
            app.status.as_deref(),
            Some("Moved “Song” to “Mix”.")
        );
    }

    #[test]
    fn removing_a_song_from_a_playlist_sends_it_back_to_all_music() {
        let root = std::env::temp_dir().join("br1-player-unmove-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Mix")).unwrap();
        std::fs::write(root.join("Mix").join("Song.mp3"), b"x").unwrap();

        let (ctx, mut app) = app_with_library(&root);
        let in_mix = root.join("Mix").join("Song.mp3");
        app.tracks = vec![test_track(in_mix.clone(), Some("Mix"))];
        app.thumbs = vec![None; 1];
        app.rebuild_visible();

        app.move_track_to_playlist(0, None);

        assert!(root.join("Song.mp3").is_file());
        assert!(!in_mix.exists());
        wait_for_scan(&ctx, &mut app);
        assert_eq!(app.tracks.len(), 1);
        assert_eq!(app.tracks[0].playlist, None);
    }

    #[test]
    fn download_target_menu_renders_and_closes() {
        let root = std::env::temp_dir().join("br1-player-target-menu-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let (ctx, mut app) = app_with_library(&root);
        app.download_target_menu_open = true;
        app.download_target_menu_pos = pos2(400.0, 200.0);

        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(1180.0, 760.0));
        run_frame(&ctx, &mut app, frame_input(screen, 0.5, Vec::new()), screen);
        assert!(
            app.download_target_menu_rect.width() > 0.0,
            "the target menu should be on screen"
        );

        // Idle frames keep it open; a click outside closes it.
        for step in 0..3 {
            run_frame(
                &ctx,
                &mut app,
                frame_input(screen, 0.8 + step as f64 * 0.1, Vec::new()),
                screen,
            );
        }
        assert!(app.download_target_menu_open, "the menu closed by itself");

        click(&ctx, &mut app, screen, pos2(40.0, 730.0), 1.2);
        assert!(
            !app.download_target_menu_open,
            "outside click should close the target menu"
        );
        // Nothing was picked, so no config write happened either way.
        assert_eq!(app.download_target, None);
    }

    #[test]
    fn writable_folders_pass_the_preflight_and_leave_no_probe_behind() {
        let root = std::env::temp_dir().join("br1-player-writable-preflight");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let playlist = root.join("Ambience");

        PlayerApp::ensure_folder_writable(&playlist).expect("creatable folder should pass");
        assert!(playlist.is_dir(), "the folder should have been created");
        assert!(
            probe_files(&playlist).is_empty(),
            "the probe file must be removed: {:?}",
            probe_files(&playlist)
        );
    }

    /// Lists every write probe left in `folder`, including the fixed name used
    /// by older builds.
    fn probe_files(folder: &std::path::Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".br1-write-check"))
            .collect()
    }

    #[test]
    fn a_leftover_read_only_probe_does_not_block_the_preflight() {
        let root = std::env::temp_dir().join("br1-player-probe-leftover");
        let _ = std::fs::remove_dir_all(&root);
        let folder = root.join("Ambience");
        std::fs::create_dir_all(&folder).unwrap();

        // Standing in for a probe an older build left behind and something
        // later locked down: writing to the old fixed name fails with
        // "access denied" even though the folder itself accepts files.
        let legacy = folder.join(".br1-write-check");
        std::fs::write(&legacy, b"leftover").unwrap();
        if let Ok(meta) = std::fs::metadata(&legacy) {
            let mut perms = meta.permissions();
            perms.set_readonly(true);
            let _ = std::fs::set_permissions(&legacy, perms);
        }

        PlayerApp::ensure_folder_writable(&folder)
            .expect("a stale probe must not be reported as a permission problem");
        // The stale file may survive (it may be locked), but the check must not
        // have added a probe of its own.
        let fresh: Vec<String> = probe_files(&folder)
            .into_iter()
            .filter(|name| name != ".br1-write-check")
            .collect();
        assert!(fresh.is_empty(), "a new probe was left behind: {fresh:?}");

        // Let the temporary folder be deleted again.
        if let Ok(meta) = std::fs::metadata(&legacy) {
            let mut perms = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(&legacy, perms);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn new_playlist_input_stays_open_and_explains_a_blocked_folder() {
        let root = std::env::temp_dir().join("br1-player-blocked-root");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // A file where the music folder should be makes every create fail,
        // standing in for the "Access denied" case.
        let blocked = root.join("blocked");
        std::fs::write(&blocked, b"x").unwrap();

        let (_ctx, mut app) = app_with_library(&blocked);
        app.download_new_open = true;
        app.download_new_playlist = "Ambience".to_string();
        app.create_download_playlist();

        assert!(app.download_failed);
        assert!(app.download_new_open, "the input should stay open for a retry");
        let message = app.download_status.clone().expect("an error message");
        assert!(
            message.contains(&blocked.display().to_string()),
            "the message should name the folder: {message}"
        );
        assert_eq!(app.download_target, None, "no broken target is selected");
    }

    #[test]
    fn a_blocked_folder_suggests_a_folder_fix_when_the_player_can_write_its_settings() {
        let hint = PlayerApp::save_blocked_hint(true);
        assert!(
            hint.contains("CHANGE FOLDER"),
            "changing the music folder is the fix here: {hint}"
        );
    }

    #[test]
    fn a_restricted_launch_is_not_blamed_on_the_music_folder() {
        let hint = PlayerApp::save_blocked_hint(false);
        assert!(
            hint.contains("restricted write permissions"),
            "the restriction itself must be named: {hint}"
        );
        assert!(
            hint.contains("bin folder"),
            "the fix is copying the app somewhere normal: {hint}"
        );
    }

    #[test]
    fn blocked_downloads_are_reported_before_anything_is_queued() {
        let root = std::env::temp_dir().join("br1-player-blocked-download");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let blocked = root.join("blocked");
        std::fs::write(&blocked, b"x").unwrap();

        let (_ctx, mut app) = app_with_library(&blocked);
        app.download_url = "https://youtu.be/abc".to_string();
        app.start_download();

        assert!(app.download_failed, "the click should report the problem");
        assert!(
            app.manager.jobs().is_empty(),
            "nothing should be queued that cannot be written"
        );
        assert!(app.download_status.is_some());
    }

    #[test]
    fn a_writable_destination_queues_the_link() {
        let root = std::env::temp_dir().join("br1-player-ok-download");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let (_ctx, mut app) = app_with_library(&root);
        app.download_target = Some("Mix".to_string());
        app.download_url = "https://youtu.be/abc".to_string();
        app.start_download();

        assert!(!app.download_failed);
        assert_eq!(app.manager.jobs().len(), 1);
        assert_eq!(app.manager.jobs()[0].detail, "Queued → Mix");
        // The playlist folder gets created up front by the preflight.
        assert!(root.join("Mix").is_dir());
    }
}
