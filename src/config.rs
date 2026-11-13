use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const DEFAULT_QUALITY: u16 = 192;
pub const DEFAULT_PARALLEL: usize = 3;
pub const PANEL_COUNT: usize = 7;
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

/// Outline used by the liquid-glass renderer. Mirrors `liquid_rust::Shape`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlassShape {
    Capsule,
    Rounded,
    Circular,
    Smooth,
}

impl GlassShape {
    pub fn label(self) -> &'static str {
        match self {
            Self::Capsule => "Capsule",
            Self::Rounded => "Rounded",
            Self::Circular => "Circular",
            Self::Smooth => "Smooth",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "capsule" => Some(Self::Capsule),
            "rounded" => Some(Self::Rounded),
            "circular" => Some(Self::Circular),
            "smooth" => Some(Self::Smooth),
            _ => None,
        }
    }
}

/// One `Tone` from liquid-rust: the fills applied to the backdrop under the
/// glass, in sRGB space, in this order: lift, LUMINOSITY, LIGHTEN, DARKEN.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassTone {
    pub lift: f32,
    pub luminance: f32,
    pub luminance_mix: f32,
    pub lighten: f32,
    pub darken_to: f32,
    pub darken: f32,
}

impl GlassTone {
    pub const NONE: Self = Self {
        lift: 0.0,
        luminance: 0.0,
        luminance_mix: 0.0,
        lighten: 0.0,
        darken_to: 1.0,
        darken: 0.0,
    };
}

/// Drop shadow under the glass, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassShadow {
    pub offset_y: f32,
    pub blur: f32,
    pub opacity: f32,
}

/// Which parts of the UI accept the liquid-glass surface. Every group can be
/// toggled independently in Settings, so a button can stay flat while its
/// neighbour is glass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlassTargets {
    pub top_bar: bool,
    pub transport: bool,
    pub player_chips: bool,
    pub downloader_actions: bool,
    pub option_chips: bool,
    pub menus: bool,
    pub lists: bool,
    pub sliders: bool,
    pub panels: bool,
    pub settings_controls: bool,
    pub setup: bool,
}

impl Default for GlassTargets {
    fn default() -> Self {
        // The set chosen for BR1 Player: transport, player chips, downloader
        // actions, menus, lists/cards, panels/modals and slider knobs.
        Self {
            top_bar: false,
            transport: true,
            player_chips: true,
            downloader_actions: true,
            option_chips: false,
            menus: true,
            lists: true,
            sliders: true,
            panels: true,
            settings_controls: false,
            setup: false,
        }
    }
}

/// Full liquid-glass configuration. Every public knob of liquid-rust's
/// `Material`, `Shadow`, `Tone` and `Appearance` is represented here; the
/// renderer in `glass.rs` resolves them the same way the library does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassConfig {
    pub enabled: bool,

    /* material */
    pub refraction: f32,
    pub depth: f32,
    pub dispersion: f32,
    pub frost: f32,
    pub splay: f32,
    pub specular: f32,
    pub rim: f32,
    pub rim_shade: f32,
    pub hairline: f32,
    pub edge: f32,
    pub dim: f32,
    pub adaptive: bool,
    pub tint_enabled: bool,
    pub tint_color: [u8; 3],
    pub tint_strength: f32,
    pub shadow: GlassShadow,
    pub light_tone: GlassTone,
    pub dark_tone: GlassTone,

    /* appearance */
    pub dark: bool,
    pub transparency: f32,
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
    pub reduce_motion: bool,
    pub adapt_to_content: bool,

    /* scene */
    pub physics: bool,
    pub glass_on_glass: bool,
    pub light_angle: f32,
    pub merge_spacing: f32,

    /* shape */
    pub shape: GlassShape,
    pub shape_radius: f32,
    pub smoothing: f32,

    /* targets */
    pub targets: GlassTargets,
}

impl Default for GlassConfig {
    fn default() -> Self {
        // Values from liquid-rust `Material::regular()` and
        // `Appearance::default()`, with the dark scheme on because the app's
        // canvas is near black.
        Self {
            enabled: true,
            refraction: 0.7,
            depth: 30.0,
            dispersion: 0.2,
            frost: 10.0,
            splay: 0.2,
            specular: 0.3,
            rim: 0.157,
            rim_shade: 0.05,
            hairline: 0.0,
            edge: 0.2,
            dim: 0.0,
            adaptive: true,
            tint_enabled: false,
            tint_color: [0, 136, 255],
            tint_strength: 0.85,
            shadow: GlassShadow {
                offset_y: 8.0,
                blur: 15.0,
                opacity: 0.06,
            },
            light_tone: GlassTone {
                lift: 0.0,
                luminance: 0.92,
                luminance_mix: 0.3,
                lighten: 0.2,
                darken_to: 0.75,
                darken: 0.1,
            },
            dark_tone: GlassTone {
                lift: 0.03,
                luminance: 0.25,
                luminance_mix: 0.2,
                lighten: 0.0,
                darken_to: 0.0,
                darken: 0.25,
            },
            dark: true,
            transparency: 0.5,
            reduce_transparency: false,
            increase_contrast: false,
            reduce_motion: false,
            adapt_to_content: true,
            physics: true,
            glass_on_glass: true,
            light_angle: 0.0,
            merge_spacing: 0.0,
            shape: GlassShape::Capsule,
            shape_radius: 18.0,
            smoothing: 0.6,
            targets: GlassTargets::default(),
        }
    }
}

impl GlassConfig {
    /// Applies one of liquid-rust's material presets. Appearance, scene,
    /// shape and target settings are left untouched.
    pub fn apply_preset(&mut self, preset: GlassPreset) {
        let material = GlassConfig::default();
        match preset {
            GlassPreset::Regular => {
                self.copy_material_from(&material);
            }
            GlassPreset::Clear => {
                self.copy_material_from(&material);
                let tone = GlassTone {
                    lift: 0.063,
                    luminance: 1.0,
                    luminance_mix: 0.04,
                    lighten: 0.0,
                    darken_to: 1.0,
                    darken: 0.0,
                };
                self.refraction = 0.7;
                self.depth = 30.0;
                self.dispersion = 0.2;
                self.frost = 6.0;
                self.splay = 0.2;
                self.specular = 0.4;
                self.rim = 0.157;
                self.rim_shade = 0.098;
                self.hairline = 0.0;
                self.edge = 0.2;
                self.shadow = GlassShadow {
                    offset_y: 8.0,
                    blur: 15.0,
                    opacity: 0.02,
                };
                self.light_tone = tone;
                self.dark_tone = tone;
                self.adaptive = false;
            }
            GlassPreset::ClearBar => {
                self.apply_preset(GlassPreset::Clear);
                self.frost = 3.0;
                self.specular = 0.2;
                self.rim_shade = 0.0;
                self.hairline = 0.157;
                let tone = GlassTone {
                    lift: 0.0,
                    luminance: 0.6,
                    luminance_mix: 0.33,
                    lighten: 0.0,
                    darken_to: 1.0,
                    darken: 0.0,
                };
                self.light_tone = tone;
                self.dark_tone = tone;
            }
            GlassPreset::Tinted => {
                self.apply_preset(GlassPreset::Regular);
                self.tint_enabled = true;
                self.tint_color = [0, 136, 255];
                self.tint_strength = 1.0;
            }
            GlassPreset::Identity => {
                self.copy_material_from(&GlassConfig {
                    refraction: 0.0,
                    depth: 1.0,
                    dispersion: 0.0,
                    frost: 0.0,
                    splay: 0.0,
                    specular: 0.0,
                    rim: 0.0,
                    rim_shade: 0.0,
                    hairline: 0.0,
                    edge: 0.0,
                    dim: 0.0,
                    adaptive: false,
                    tint_enabled: false,
                    shadow: GlassShadow {
                        offset_y: 0.0,
                        blur: 0.0,
                        opacity: 0.0,
                    },
                    light_tone: GlassTone::NONE,
                    dark_tone: GlassTone::NONE,
                    ..GlassConfig::default()
                });
            }
        }
    }

    fn copy_material_from(&mut self, source: &GlassConfig) {
        self.refraction = source.refraction;
        self.depth = source.depth;
        self.dispersion = source.dispersion;
        self.frost = source.frost;
        self.splay = source.splay;
        self.specular = source.specular;
        self.rim = source.rim;
        self.rim_shade = source.rim_shade;
        self.hairline = source.hairline;
        self.edge = source.edge;
        self.dim = source.dim;
        self.adaptive = source.adaptive;
        self.tint_enabled = source.tint_enabled;
        self.tint_color = source.tint_color;
        self.tint_strength = source.tint_strength;
        self.shadow = source.shadow;
        self.light_tone = source.light_tone;
        self.dark_tone = source.dark_tone;
    }
}

/// Material presets from liquid-rust's README.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlassPreset {
    Regular,
    Clear,
    ClearBar,
    Tinted,
    Identity,
}

impl GlassPreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::Regular => "REGULAR",
            Self::Clear => "CLEAR",
            Self::ClearBar => "CLEAR BAR",
            Self::Tinted => "TINTED",
            Self::Identity => "IDENTITY",
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
    /// Optional looping video on top of the inner dashboard background.
    pub background_video: Option<PathBuf>,
    /// How much the background photo/video is darkened (0..1).
    pub background_dim: f32,
    pub panels: [PanelRect; PANEL_COUNT],
    pub eq_enabled: bool,
    pub eq_gains: [f32; EQ_BANDS],
    /// Emitter position inside the spatial room, -1..1 from the listener.
    pub spatial_x: f32,
    pub spatial_y: f32,
    /// Room dimension in metres (scales distance and reverb length).
    pub room_size: f32,
    /// Reverb wet level, 0..1.
    pub reverb: f32,
    pub volume: f32,
    pub muted: bool,
    /// Apple-style liquid glass for the selected component groups.
    pub liquid: GlassConfig,
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
            background_video: None,
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
                // Equalizer (bottom right)
                PanelRect::new(0.67598, 0.65413, 0.29876, 0.29676),
                // Audio visualizer — hidden until added from the component menu
                PanelRect {
                    x: 0.67598,
                    y: 0.65413,
                    w: 0.29876,
                    h: 0.29676,
                    visible: false,
                },
                // Spatial audio — hidden until added from the component menu
                PanelRect {
                    x: 0.23267,
                    y: 0.65413,
                    w: 0.29876,
                    h: 0.29676,
                    visible: false,
                },
            ],
            eq_enabled: true,
            eq_gains: [0.0; EQ_BANDS],
            spatial_x: 0.0,
            spatial_y: 0.0,
            room_size: 8.0,
            reverb: 0.0,
            volume: 0.8,
            muted: false,
            liquid: GlassConfig::default(),
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
    base.join("Music").join("BR1 Player")
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

fn set_f32(target: &mut f32, value: &str, min: f32, max: f32) {
    if let Ok(parsed) = value.parse::<f32>() {
        *target = parsed.clamp(min, max);
    }
}

fn panel_key(index: usize) -> &'static str {
    match index {
        0 => "layout.current_playlist",
        1 => "layout.player",
        2 => "layout.downloader",
        3 => "layout.playlists",
        4 => "layout.equalizer",
        5 => "layout.visualizer",
        6 => "layout.spatial_audio",
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
            "ui_background_video" => {
                ui.background_video = if value.is_empty() {
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
            "ui_spatial_x" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.spatial_x = parsed.clamp(-1.0, 1.0);
                }
            }
            "ui_spatial_y" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.spatial_y = parsed.clamp(-1.0, 1.0);
                }
            }
            "ui_room_size" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.room_size = parsed.clamp(2.0, 24.0);
                }
            }
            "ui_reverb" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.reverb = parsed.clamp(0.0, 1.0);
                }
            }
            "volume" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.volume = parsed.clamp(0.0, 1.0);
                }
            }
            "muted" => ui.muted = value == "1" || value.eq_ignore_ascii_case("true"),
            "ui_glass_enabled" => ui.liquid.enabled = parse_bool(value),
            "ui_glass_refraction" => set_f32(&mut ui.liquid.refraction, value, 0.0, 1.0),
            "ui_glass_depth" => set_f32(&mut ui.liquid.depth, value, 0.0, 120.0),
            "ui_glass_dispersion" => set_f32(&mut ui.liquid.dispersion, value, 0.0, 1.0),
            "ui_glass_frost" => set_f32(&mut ui.liquid.frost, value, 0.0, 80.0),
            "ui_glass_splay" => set_f32(&mut ui.liquid.splay, value, 0.0, 1.0),
            "ui_glass_specular" => set_f32(&mut ui.liquid.specular, value, 0.0, 1.0),
            "ui_glass_rim" => set_f32(&mut ui.liquid.rim, value, 0.0, 1.0),
            "ui_glass_rim_shade" => set_f32(&mut ui.liquid.rim_shade, value, 0.0, 1.0),
            "ui_glass_hairline" => set_f32(&mut ui.liquid.hairline, value, 0.0, 1.0),
            "ui_glass_edge" => set_f32(&mut ui.liquid.edge, value, 0.0, 1.0),
            "ui_glass_dim" => set_f32(&mut ui.liquid.dim, value, 0.0, 1.0),
            "ui_glass_adaptive" => ui.liquid.adaptive = parse_bool(value),
            "ui_glass_tint_enabled" => ui.liquid.tint_enabled = parse_bool(value),
            "ui_glass_tint_color" => {
                if let Some(color) = parse_color(value) {
                    ui.liquid.tint_color = color;
                }
            }
            "ui_glass_tint_strength" => set_f32(&mut ui.liquid.tint_strength, value, 0.0, 1.0),
            "ui_glass_shadow_offset" => set_f32(&mut ui.liquid.shadow.offset_y, value, -40.0, 40.0),
            "ui_glass_shadow_blur" => set_f32(&mut ui.liquid.shadow.blur, value, 0.0, 100.0),
            "ui_glass_shadow_opacity" => set_f32(&mut ui.liquid.shadow.opacity, value, 0.0, 1.0),
            "ui_glass_light_lift" => set_f32(&mut ui.liquid.light_tone.lift, value, 0.0, 0.5),
            "ui_glass_light_luminance" => {
                set_f32(&mut ui.liquid.light_tone.luminance, value, 0.0, 1.0)
            }
            "ui_glass_light_luminance_mix" => {
                set_f32(&mut ui.liquid.light_tone.luminance_mix, value, 0.0, 1.0)
            }
            "ui_glass_light_lighten" => {
                set_f32(&mut ui.liquid.light_tone.lighten, value, 0.0, 1.0)
            }
            "ui_glass_light_darken_to" => {
                set_f32(&mut ui.liquid.light_tone.darken_to, value, 0.0, 1.0)
            }
            "ui_glass_light_darken" => {
                set_f32(&mut ui.liquid.light_tone.darken, value, 0.0, 1.0)
            }
            "ui_glass_dark_lift" => set_f32(&mut ui.liquid.dark_tone.lift, value, 0.0, 0.5),
            "ui_glass_dark_luminance" => {
                set_f32(&mut ui.liquid.dark_tone.luminance, value, 0.0, 1.0)
            }
            "ui_glass_dark_luminance_mix" => {
                set_f32(&mut ui.liquid.dark_tone.luminance_mix, value, 0.0, 1.0)
            }
            "ui_glass_dark_lighten" => set_f32(&mut ui.liquid.dark_tone.lighten, value, 0.0, 1.0),
            "ui_glass_dark_darken_to" => {
                set_f32(&mut ui.liquid.dark_tone.darken_to, value, 0.0, 1.0)
            }
            "ui_glass_dark_darken" => set_f32(&mut ui.liquid.dark_tone.darken, value, 0.0, 1.0),
            "ui_glass_dark_scheme" => ui.liquid.dark = parse_bool(value),
            "ui_glass_transparency" => set_f32(&mut ui.liquid.transparency, value, 0.0, 1.0),
            "ui_glass_reduce_transparency" => ui.liquid.reduce_transparency = parse_bool(value),
            "ui_glass_increase_contrast" => ui.liquid.increase_contrast = parse_bool(value),
            "ui_glass_reduce_motion" => ui.liquid.reduce_motion = parse_bool(value),
            "ui_glass_adapt_to_content" => ui.liquid.adapt_to_content = parse_bool(value),
            "ui_glass_physics" => ui.liquid.physics = parse_bool(value),
            "ui_glass_on_glass" => ui.liquid.glass_on_glass = parse_bool(value),
            "ui_glass_light_angle" => {
                if let Ok(parsed) = value.parse::<f32>() {
                    ui.liquid.light_angle = parsed.rem_euclid(360.0);
                }
            }
            "ui_glass_merge_spacing" => set_f32(&mut ui.liquid.merge_spacing, value, 0.0, 80.0),
            "ui_glass_shape" => {
                if let Some(shape) = GlassShape::parse(value) {
                    ui.liquid.shape = shape;
                }
            }
            "ui_glass_shape_radius" => set_f32(&mut ui.liquid.shape_radius, value, 0.0, 120.0),
            "ui_glass_smoothing" => set_f32(&mut ui.liquid.smoothing, value, 0.0, 1.0),
            "ui_glass_target_top_bar" => ui.liquid.targets.top_bar = parse_bool(value),
            "ui_glass_target_transport" => ui.liquid.targets.transport = parse_bool(value),
            "ui_glass_target_player_chips" => ui.liquid.targets.player_chips = parse_bool(value),
            "ui_glass_target_downloader_actions" => {
                ui.liquid.targets.downloader_actions = parse_bool(value)
            }
            "ui_glass_target_option_chips" => ui.liquid.targets.option_chips = parse_bool(value),
            "ui_glass_target_menus" => ui.liquid.targets.menus = parse_bool(value),
            "ui_glass_target_lists" => ui.liquid.targets.lists = parse_bool(value),
            "ui_glass_target_sliders" => ui.liquid.targets.sliders = parse_bool(value),
            "ui_glass_target_panels" => ui.liquid.targets.panels = parse_bool(value),
            "ui_glass_target_settings_controls" => {
                ui.liquid.targets.settings_controls = parse_bool(value)
            }
            "ui_glass_target_setup" => ui.liquid.targets.setup = parse_bool(value),
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
         ui_background_video={}\n\
         ui_background_dim={:.3}\n\
         eq_enabled={}\n\
         eq_gains={}\n\
         ui_spatial_x={:.4}\n\
         ui_spatial_y={:.4}\n\
         ui_room_size={:.2}\n\
         ui_reverb={:.3}\n\
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
        ui.background_video
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
        ui.spatial_x,
        ui.spatial_y,
        ui.room_size,
        ui.reverb,
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
    push_glass(&mut text, &ui.liquid);
    let _ = std::fs::write(path, text);
}

fn push_glass(text: &mut String, glass: &GlassConfig) {
    let _ = writeln!(text, "ui_glass_enabled={}", u8::from(glass.enabled));
    let _ = writeln!(text, "ui_glass_refraction={:.4}", glass.refraction);
    let _ = writeln!(text, "ui_glass_depth={:.3}", glass.depth);
    let _ = writeln!(text, "ui_glass_dispersion={:.4}", glass.dispersion);
    let _ = writeln!(text, "ui_glass_frost={:.3}", glass.frost);
    let _ = writeln!(text, "ui_glass_splay={:.4}", glass.splay);
    let _ = writeln!(text, "ui_glass_specular={:.4}", glass.specular);
    let _ = writeln!(text, "ui_glass_rim={:.4}", glass.rim);
    let _ = writeln!(text, "ui_glass_rim_shade={:.4}", glass.rim_shade);
    let _ = writeln!(text, "ui_glass_hairline={:.4}", glass.hairline);
    let _ = writeln!(text, "ui_glass_edge={:.4}", glass.edge);
    let _ = writeln!(text, "ui_glass_dim={:.4}", glass.dim);
    let _ = writeln!(text, "ui_glass_adaptive={}", u8::from(glass.adaptive));
    let _ = writeln!(text, "ui_glass_tint_enabled={}", u8::from(glass.tint_enabled));
    let _ = writeln!(
        text,
        "ui_glass_tint_color=#{:02X}{:02X}{:02X}",
        glass.tint_color[0], glass.tint_color[1], glass.tint_color[2]
    );
    let _ = writeln!(text, "ui_glass_tint_strength={:.4}", glass.tint_strength);
    let _ = writeln!(text, "ui_glass_shadow_offset={:.3}", glass.shadow.offset_y);
    let _ = writeln!(text, "ui_glass_shadow_blur={:.3}", glass.shadow.blur);
    let _ = writeln!(text, "ui_glass_shadow_opacity={:.4}", glass.shadow.opacity);
    push_tone(text, "ui_glass_light", &glass.light_tone);
    push_tone(text, "ui_glass_dark", &glass.dark_tone);
    let _ = writeln!(text, "ui_glass_dark_scheme={}", u8::from(glass.dark));
    let _ = writeln!(text, "ui_glass_transparency={:.4}", glass.transparency);
    let _ = writeln!(
        text,
        "ui_glass_reduce_transparency={}",
        u8::from(glass.reduce_transparency)
    );
    let _ = writeln!(
        text,
        "ui_glass_increase_contrast={}",
        u8::from(glass.increase_contrast)
    );
    let _ = writeln!(text, "ui_glass_reduce_motion={}", u8::from(glass.reduce_motion));
    let _ = writeln!(
        text,
        "ui_glass_adapt_to_content={}",
        u8::from(glass.adapt_to_content)
    );
    let _ = writeln!(text, "ui_glass_physics={}", u8::from(glass.physics));
    let _ = writeln!(text, "ui_glass_on_glass={}", u8::from(glass.glass_on_glass));
    let _ = writeln!(text, "ui_glass_light_angle={:.2}", glass.light_angle);
    let _ = writeln!(text, "ui_glass_merge_spacing={:.3}", glass.merge_spacing);
    let _ = writeln!(text, "ui_glass_shape={}", glass.shape.label());
    let _ = writeln!(text, "ui_glass_shape_radius={:.3}", glass.shape_radius);
    let _ = writeln!(text, "ui_glass_smoothing={:.4}", glass.smoothing);
    let _ = writeln!(
        text,
        "ui_glass_target_top_bar={}",
        u8::from(glass.targets.top_bar)
    );
    let _ = writeln!(
        text,
        "ui_glass_target_transport={}",
        u8::from(glass.targets.transport)
    );
    let _ = writeln!(
        text,
        "ui_glass_target_player_chips={}",
        u8::from(glass.targets.player_chips)
    );
    let _ = writeln!(
        text,
        "ui_glass_target_downloader_actions={}",
        u8::from(glass.targets.downloader_actions)
    );
    let _ = writeln!(
        text,
        "ui_glass_target_option_chips={}",
        u8::from(glass.targets.option_chips)
    );
    let _ = writeln!(
        text,
        "ui_glass_target_menus={}",
        u8::from(glass.targets.menus)
    );
    let _ = writeln!(
        text,
        "ui_glass_target_lists={}",
        u8::from(glass.targets.lists)
    );
    let _ = writeln!(
        text,
        "ui_glass_target_sliders={}",
        u8::from(glass.targets.sliders)
    );
    let _ = writeln!(
        text,
        "ui_glass_target_panels={}",
        u8::from(glass.targets.panels)
    );
    let _ = writeln!(
        text,
        "ui_glass_target_settings_controls={}",
        u8::from(glass.targets.settings_controls)
    );
    let _ = writeln!(
        text,
        "ui_glass_target_setup={}",
        u8::from(glass.targets.setup)
    );
}

fn push_tone(text: &mut String, prefix: &str, tone: &GlassTone) {
    let _ = writeln!(text, "{prefix}_lift={:.4}", tone.lift);
    let _ = writeln!(text, "{prefix}_luminance={:.4}", tone.luminance);
    let _ = writeln!(text, "{prefix}_luminance_mix={:.4}", tone.luminance_mix);
    let _ = writeln!(text, "{prefix}_lighten={:.4}", tone.lighten);
    let _ = writeln!(text, "{prefix}_darken_to={:.4}", tone.darken_to);
    let _ = writeln!(text, "{prefix}_darken={:.4}", tone.darken);
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
        original.ui.liquid.transparency = 0.75;
        original.ui.liquid.dark = false;
        original.ui.liquid.tint_enabled = true;
        original.ui.liquid.tint_color = [10, 20, 30];
        original.ui.liquid.tint_strength = 0.5;
        original.ui.liquid.shape = GlassShape::Smooth;
        original.ui.liquid.shape_radius = 24.0;
        original.ui.liquid.smoothing = 0.6;
        original.ui.liquid.targets.top_bar = true;
        original.ui.liquid.targets.setup = true;
        original.ui.liquid.dark_tone.lift = 0.125;
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
        assert_eq!(loaded.ui.liquid.transparency, 0.75);
        assert_eq!(loaded.ui.liquid.dark, false);
        assert_eq!(loaded.ui.liquid.tint_enabled, true);
        assert_eq!(loaded.ui.liquid.tint_color, [10, 20, 30]);
        assert_eq!(loaded.ui.liquid.tint_strength, 0.5);
        assert_eq!(loaded.ui.liquid.shape, GlassShape::Smooth);
        assert_eq!(loaded.ui.liquid.shape_radius, 24.0);
        assert_eq!(loaded.ui.liquid.smoothing, 0.6);
        assert_eq!(loaded.ui.liquid.targets.top_bar, true);
        assert_eq!(loaded.ui.liquid.targets.setup, true);
        assert_eq!(loaded.ui.liquid.dark_tone.lift, 0.125);
        assert_eq!(loaded.ui.liquid.targets.transport, true);
        assert_eq!(loaded.ui.liquid.targets.settings_controls, false);
    }

    #[test]
    fn default_glass_targets_match_selection() {
        let targets = GlassTargets::default();
        assert!(targets.transport);
        assert!(targets.player_chips);
        assert!(targets.downloader_actions);
        assert!(targets.menus);
        assert!(targets.lists);
        assert!(targets.sliders);
        assert!(targets.panels);
        assert!(!targets.top_bar);
        assert!(!targets.option_chips);
        assert!(!targets.settings_controls);
        assert!(!targets.setup);
    }

    #[test]
    fn glass_presets_change_material_only() {
        let mut glass = GlassConfig::default();
        glass.targets.top_bar = true;
        glass.apply_preset(GlassPreset::ClearBar);
        assert_eq!(glass.frost, 3.0);
        assert_eq!(glass.hairline, 0.157);
        assert!(!glass.adaptive);
        assert!(glass.targets.top_bar);
        assert!(glass.targets.transport);

        glass.apply_preset(GlassPreset::Tinted);
        assert!(glass.tint_enabled);
        assert_eq!(glass.tint_color, [0, 136, 255]);
        assert!(glass.targets.top_bar);
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
