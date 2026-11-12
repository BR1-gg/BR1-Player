use std::path::{Path, PathBuf};

pub const DEFAULT_QUALITY: u16 = 192;
pub const DEFAULT_PARALLEL: usize = 3;
pub const PANEL_COUNT: usize = 5;
pub const EQ_BANDS: usize = 10;

/// One dashboard panel, stored as a fraction of the dashboard's inner area so
/// the layout follows the window size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub visible: bool,
}

impl PanelRect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            x,
            y,
            w,
            h,
            visible: true,
        }
    }
}

/// Visual + layout options. The near-black canvas stays as the base; accents,
/// the frame, borders, panel glass and an optional background photo are all
/// configurable.
#[derive(Clone, Debug, PartialEq)]
pub struct UiConfig {
    pub accent_a: [u8; 3],
    pub accent_b: [u8; 3],
    /// Gradient direction in degrees.
    pub angle: f32,
    /// How strongly the accent shows through the glass panels (0..1).
    pub glass: f32,
    /// Panel corner radius in points.
    pub radius: f32,
    /// Accent glow strength (0..1).
    pub glow: f32,
    /// Colour of the outer frame, the area closest to the screen edge.
    pub frame_color: [u8; 3],
    /// Draw the inner rounded dashboard surface behind the components.
    pub frame_visible: bool,
    /// Outer frame colour opacity (0..1).
    pub frame_opacity: f32,
    /// Draw the inner dashboard outline.
    pub frame_border_visible: bool,
    /// Inner dashboard outline thickness in points.
    pub frame_border_width: f32,
    /// Draw borders on the individual components.
    pub panel_border_visible: bool,
    /// Component border thickness in points.
    pub panel_border_width: f32,
    /// Base colour of the inner dashboard background.
    pub background_color: [u8; 3],
    /// Optional photo on top of the inner dashboard background.
    pub background_image: Option<PathBuf>,
    /// How much the background photo is darkened (0..1).
    pub background_dim: f32,
    pub panels: [PanelRect; PANEL_COUNT],
    pub eq_enabled: bool,
    pub eq_gains: [f32; EQ_BANDS],
    pub volume: f32,
    pub muted: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        // Proportions taken from the 1920x1080 design: the dashboard frame is
        // 1861x957 at (32,94), and the five panels sit inside it.
        Self {
            accent_a: [124, 92, 255],
            accent_b: [34, 211, 238],
            angle: 132.0,
            glass: 0.62,
            radius: 24.0,
            glow: 0.62,
            frame_color: [18, 16, 26],
            frame_visible: true,
            frame_opacity: 0.92,
            frame_border_visible: true,
            frame_border_width: 1.5,
            panel_border_visible: true,
            panel_border_width: 1.0,
            background_color: [16, 16, 22],
            background_image: None,
            background_dim: 0.55,
            panels: [
                // Current playlist (left)
                PanelRect::new(0.02445, 0.04702, 0.20016, 0.58934),
                // Player (centre)
                PanelRect::new(0.23267, 0.04702, 0.43471, 0.58934),
                // Downloader (right)
                PanelRect::new(0.67598, 0.04702, 0.29876, 0.58934),
                // Playlist covers (bottom, spanning left + centre)
                PanelRect::new(0.02445, 0.65413, 0.64293, 0.29676),
                // Equalizer / visualizer (bottom right)
                PanelRect::new(0.67598, 0.65413, 0.29876, 0.29676),
            ],
            eq_enabled: true,
            eq_gains: [0.0; EQ_BANDS],
            volume: 0.8,
            muted: false,
        }
    }
}

/// User settings, stored in %APPDATA%\DefPlayer\config.txt
#[derive(Clone)]
pub struct Config {
    /// Main folder: every download goes here. Playlists get an automatic subfolder.
    pub library_root: PathBuf,
    /// MP3 bitrate in kbps.
    pub quality: u16,
    /// How many downloads may run at the same time.
    pub parallel: usize,
    pub ui: UiConfig,
}

impl Config {
    pub fn with_root(library_root: PathBuf) -> Self {
        Self {
            library_root,
            quality: DEFAULT_QUALITY,
            parallel: DEFAULT_PARALLEL,
            ui: UiConfig::default(),
        }
    }
}

pub fn default_library_root() -> PathBuf {
    let base = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Music").join("Def Player")
}

pub fn config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join("DefPlayer").join("config.txt"))
}

fn legacy_settings_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|base| PathBuf::from(base).join("DefPlayer").join("settings.txt"))
}

fn parse_bool(value: &str) -> bool {
    value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
}

fn parse_color(value: &str) -> Option<[u8; 3]> {
    let value = value.trim().trim_start_matches('#');
    if value.len() == 6 {
        let r = u8::from_str_radix(&value[0..2], 16).ok()?;
        let g = u8::from_str_radix(&value[2..4], 16).ok()?;
        let b = u8::from_str_radix(&value[4..6], 16).ok()?;
        return Some([r, g, b]);
    }
    let mut parts = value.split(',').map(|part| part.trim().parse::<u8>());
    Some([
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    ])
}

fn parse_panel(value: &str) -> Option<PanelRect> {
    let mut parts = value.split(',').map(|part| part.trim().parse::<f32>());
    let x = parts.next()?.ok()?;
    let y = parts.next()?.ok()?;
    let w = parts.next()?.ok()?;
    let h = parts.next()?.ok()?;
    let visible = parts
        .next()
        .and_then(|part| part.ok())
        .map(|value| value >= 0.5)
        .unwrap_or(true);
    if !(0.0..=1.0).contains(&x)
        || !(0.0..=1.0).contains(&y)
        || !(0.0..=1.0).contains(&w)
        || !(0.0..=1.0).contains(&h)
        || w <= 0.0
        || h <= 0.0
    {
        return None;
    }
    Some(PanelRect {
        x,
        y,
        w,
        h,
        visible,
    })
}

fn parse_eq(value: &str) -> Option<[f32; EQ_BANDS]> {
    let mut gains = [0.0; EQ_BANDS];
    let mut count = 0;
    for (index, part) in value.split(',').enumerate().take(EQ_BANDS) {
        gains[index] = part.trim().parse::<f32>().ok()?.clamp(-12.0, 12.0);
        count += 1;
    }
    (count == EQ_BANDS).then_some(gains)
}

fn panel_key(index: usize) -> &'static str {
    match index {
        0 => "layout.current_playlist",
        1 => "layout.player",
        2 => "layout.downloader",
        3 => "layout.playlists",
        4 => "layout.equalizer",
        _ => "layout.unknown",
    }
}

pub fn load_from(path: &Path) -> Option<Config> {
    let text = std::fs::read_to_string(path).ok()?;

    let mut root = None;
    let mut quality = DEFAULT_QUALITY;
    let mut parallel = DEFAULT_PARALLEL;
    let mut ui = UiConfig::default();

    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "library_root" => {
                if !value.is_empty() {
                    root = Some(PathBuf::from(value));
                }
            }
            "quality" => {
                if let Ok(parsed) = value.parse::<u16>() {
                    if (64..=320).contains(&parsed) {
                        quality = parsed;
                    }
                }
            }
            "parallel" => {
                if let Ok(parsed) = value.parse::<usize>() {
                    if (1..=12).contains(&parsed) {
                        parallel = parsed;
                    }
                }
            }
            "ui_accent_a" => {
                if let Some(color) = parse_color(value) {
                    ui.accent_a = color;
                }
            }
            "ui_accent_b" => {
                if let Some(color) = parse_color(value) {
                    ui.accent_b = color;
                }
            }
            "ui_angle" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.angle = parsed.rem_euclid(360.0);
                }
            }
            "ui_glass" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.glass = parsed.clamp(0.0, 1.0);
                }
            }
            "ui_radius" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.radius = parsed.clamp(8.0, 42.0);
                }
            }
            "ui_glow" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.glow = parsed.clamp(0.0, 1.0);
                }
            }
            "ui_frame_color" => {
                if let Some(color) = parse_color(value) {
                    ui.frame_color = color;
                }
            }
            "ui_frame_visible" => ui.frame_visible = parse_bool(value),
            "ui_frame_opacity" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.frame_opacity = parsed.clamp(0.0, 1.0);
                }
            }
            "ui_frame_border_visible" => ui.frame_border_visible = parse_bool(value),
            "ui_frame_border_width" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.frame_border_width = parsed.clamp(0.0, 8.0);
                }
            }
            "ui_panel_border_visible" => ui.panel_border_visible = parse_bool(value),
            "ui_panel_border_width" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.panel_border_width = parsed.clamp(0.0, 8.0);
                }
            }
            "ui_background_color" => {
                if let Some(color) = parse_color(value) {
                    ui.background_color = color;
                }
            }
            "ui_background_image" => {
                ui.background_image = if value.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(value))
                };
            }
            "ui_background_dim" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.background_dim = parsed.clamp(0.0, 1.0);
                }
            }
            "eq_enabled" => ui.eq_enabled = parse_bool(value),
            "eq_gains" => {
                if let Some(gains) = parse_eq(value) {
                    ui.eq_gains = gains;
                }
            }
            "volume" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.volume = parsed.clamp(0.0, 1.0);
                }
            }
            "muted" => ui.muted = value == "1" || value.eq_ignore_ascii_case("true"),
            key => {
                for index in 0..PANEL_COUNT {
                    if key == panel_key(index) {
                        if let Some(panel) = parse_panel(value) {
                            ui.panels[index] = panel;
                        }
                    }
                }
            }
        }
    }

    root.map(|library_root| Config {
        library_root,
        quality,
        parallel,
        ui,
    })
}

pub fn save_to(path: &Path, config: &Config) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ui = &config.ui;
    let mut text = format!(
        "library_root={}\nquality={}\nparallel={}\n\
         ui_accent_a=#{:02X}{:02X}{:02X}\n\
         ui_accent_b=#{:02X}{:02X}{:02X}\n\
         ui_angle={:.1}\n\
         ui_glass={:.3}\n\
         ui_radius={:.1}\n\
         ui_glow={:.3}\n\
         ui_frame_color=#{:02X}{:02X}{:02X}\n\
         ui_frame_visible={}\n\
         ui_frame_opacity={:.3}\n\
         ui_frame_border_visible={}\n\
         ui_frame_border_width={:.2}\n\
         ui_panel_border_visible={}\n\
         ui_panel_border_width={:.2}\n\
         ui_background_color=#{:02X}{:02X}{:02X}\n\
         ui_background_image={}\n\
         ui_background_dim={:.3}\n\
         eq_enabled={}\n\
         eq_gains={}\n\
         volume={:.3}\n\
         muted={}\n",
        config.library_root.display(),
        config.quality,
        config.parallel,
        ui.accent_a[0],
        ui.accent_a[1],
        ui.accent_a[2],
        ui.accent_b[0],
        ui.accent_b[1],
        ui.accent_b[2],
        ui.angle,
        ui.glass,
        ui.radius,
        ui.glow,
        ui.frame_color[0],
        ui.frame_color[1],
        ui.frame_color[2],
        if ui.frame_visible { 1 } else { 0 },
        ui.frame_opacity,
        if ui.frame_border_visible { 1 } else { 0 },
        ui.frame_border_width,
        if ui.panel_border_visible { 1 } else { 0 },
        ui.panel_border_width,
        ui.background_color[0],
        ui.background_color[1],
        ui.background_color[2],
        ui.background_image
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        ui.background_dim,
        if ui.eq_enabled { 1 } else { 0 },
        ui.eq_gains
            .iter()
            .map(|gain| format!("{gain:.2}"))
            .collect::<Vec<_>>()
            .join(","),
        ui.volume,
        if ui.muted { 1 } else { 0 },
    );
    for (index, panel) in ui.panels.iter().enumerate() {
        text.push_str(&format!(
            "{}={:.5},{:.5},{:.5},{:.5},{}\n",
            panel_key(index),
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            if panel.visible { 1 } else { 0 }
        ));
    }
    let _ = std::fs::write(path, text);
}

/// Reads the config, migrating the old single-path settings file if needed.
pub fn load() -> Option<Config> {
    if let Some(path) = config_path() {
        if path.exists() {
            return load_from(&path);
        }
    }

    let legacy = legacy_settings_path()?;
    let text = std::fs::read_to_string(legacy).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(Config::with_root(PathBuf::from(text)))
}

pub fn save(config: &Config) {
    if let Some(path) = config_path() {
        save_to(&path, config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip() {
        let dir = std::env::temp_dir().join("defplayer_config_test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.txt");

        let mut original = Config {
            library_root: PathBuf::from(r"D:\Music Library"),
            quality: 320,
            parallel: 6,
            ui: UiConfig::default(),
        };
        original.ui.accent_a = [255, 122, 89];
        original.ui.accent_b = [177, 75, 255];
        original.ui.angle = 45.0;
        original.ui.frame_color = [12, 13, 14];
        original.ui.frame_opacity = 0.75;
        original.ui.frame_border_width = 3.5;
        original.ui.panel_border_width = 0.0;
        original.ui.background_color = [20, 30, 40];
        original.ui.background_image = Some(PathBuf::from(r"C:\Pictures\backdrop.png"));
        original.ui.background_dim = 0.4;
        original.ui.panels[1] = PanelRect {
            x: 0.21,
            y: 0.05,
            w: 0.45,
            h: 0.58,
            visible: false,
        };
        original.ui.eq_gains[3] = 4.5;
        save_to(&path, &original);

        let loaded = load_from(&path).expect("config should load");
        assert_eq!(loaded.library_root, original.library_root);
        assert_eq!(loaded.quality, 320);
        assert_eq!(loaded.parallel, 6);
        assert_eq!(loaded.ui.accent_a, [255, 122, 89]);
        assert_eq!(loaded.ui.accent_b, [177, 75, 255]);
        assert_eq!(loaded.ui.panels[1], original.ui.panels[1]);
        assert_eq!(loaded.ui.frame_color, [12, 13, 14]);
        assert_eq!(loaded.ui.frame_opacity, 0.75);
        assert_eq!(loaded.ui.frame_border_width, 3.5);
        assert_eq!(loaded.ui.panel_border_width, 0.0);
        assert_eq!(loaded.ui.background_color, [20, 30, 40]);
        assert_eq!(
            loaded.ui.background_image,
            Some(PathBuf::from(r"C:\Pictures\backdrop.png"))
        );
        assert_eq!(loaded.ui.background_dim, 0.4);
        assert_eq!(loaded.ui.eq_gains[3], 4.5);
    }

    #[test]
    fn old_config_without_new_keys_uses_defaults() {
        let dir = std::env::temp_dir().join("defplayer_config_legacy");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.txt");
        std::fs::write(&path, "library_root=C:\\Music\n").unwrap();

        let loaded = load_from(&path).expect("config should load");
        assert_eq!(loaded.quality, DEFAULT_QUALITY);
        assert_eq!(loaded.parallel, DEFAULT_PARALLEL);
        assert_eq!(loaded.ui, UiConfig::default());
    }
}
