//! Real binaural spatial audio: HRTF convolution, distance/air absorption,
//! early reflections and a stereo room tail.
//!
//! The heavy lifting is done by the pure-Rust [`sofar`] crate:
//!   * `sofar::reader` parses SOFA files (HDF5) and interpolates between the
//!     measured HRIRs around the requested direction (inverse distance
//!     weighting over the nearest measurements).
//!   * `sofar::render` is a uniformly partitioned, overlap-save FFT
//!     convolution engine. Filters are swapped with a crossfade, so a moving
//!     source does not click or zip.
//!
//! Three synthetic HRTF sets are generated locally so the feature works with
//! no downloads at all. They model ITD (Woodworth), head shadow (ILD), pinna
//! reflections (elevation + front/back cues) and a torso reflection. Any SOFA
//! file dropped into `%APPDATA%\BR1 Player\hrtf` is offered as an extra set.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_4, PI, SQRT_2, TAU};
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::Source;
use sofar::filter::Filter;
use sofar::reader::{OpenOptions, Sofar};
use sofar::render::{FilterTransition, Renderer, RendererPlan};

/// FIR taps per ear. 512 taps is 10.7 ms at 48 kHz, which keeps the pinna,
/// head-shadow and first torso cues while staying cheap on the CPU.
pub const FILTER_LEN: usize = 512;
/// Uniform partition size for the FFT convolution. 256 frames is 5.3 ms at
/// 48 kHz: low latency, two partitions per 512-tap filter.
pub const PARTITION_LEN: usize = 256;
const MAX_DELAY_SECS: f32 = 0.003;
pub const MIN_DISTANCE: f32 = 0.5;
const SPEED_OF_SOUND: f32 = 343.0;
pub const DEFAULT_SET_ID: &str = "builtin:studio";
/// Measured sets downloaded by the first-run installer.
pub const KEMAR_NORMAL_ID: &str = "sofa:MIT_KEMAR_normal.sofa";

/* ------------------------------------------------------------------ */
/* HRTF sets                                                            */
/* ------------------------------------------------------------------ */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntheticKind {
    Studio,
    Compact,
    Broad,
}

impl SyntheticKind {
    pub const ALL: [Self; 3] = [Self::Studio, Self::Compact, Self::Broad];

    pub fn id(self) -> &'static str {
        match self {
            Self::Studio => "builtin:studio",
            Self::Compact => "builtin:compact",
            Self::Broad => "builtin:broad",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Studio => "Studio (synthetic)",
            Self::Compact => "Compact (synthetic)",
            Self::Broad => "Broad (synthetic)",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Studio => "STUDIO",
            Self::Compact => "COMPACT",
            Self::Broad => "BROAD",
        }
    }

    fn model(self) -> SyntheticModel {
        match self {
            Self::Studio => SyntheticModel {
                head_radius: 0.0875,
                shadow: 1.0,
                pinna: 1.0,
                rear: 1.0,
                torso: 1.0,
            },
            Self::Compact => SyntheticModel {
                head_radius: 0.0790,
                shadow: 0.92,
                pinna: 0.90,
                rear: 0.88,
                torso: 0.92,
            },
            Self::Broad => SyntheticModel {
                head_radius: 0.0965,
                shadow: 1.08,
                pinna: 1.10,
                rear: 1.12,
                torso: 1.08,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HrtfSetKind {
    Synthetic(SyntheticKind),
    Sofa(PathBuf),
}

#[derive(Clone, Debug)]
pub struct HrtfSetInfo {
    pub id: String,
    pub name: String,
    pub short: String,
    pub detail: String,
    pub kind: HrtfSetKind,
}

/// Folder scanned for user-supplied SOFA files.
pub fn hrtf_folder() -> PathBuf {
    crate::config::appdata_dir()
        .unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        })
        .join("hrtf")
}

fn short_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "SOFA".to_string());
    let lower = stem.to_ascii_lowercase();
    // The two KEMAR files share a long prefix; keep their chip labels distinct.
    if lower.contains("kemar") {
        if lower.contains("large") {
            return "KEMAR L".to_string();
        }
        if lower.contains("normal") {
            return "KEMAR N".to_string();
        }
        return "KEMAR".to_string();
    }
    let mut out: String = stem
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == ' ' || *ch == '-' || *ch == '_')
        .take(9)
        .collect();
    if out.trim().is_empty() {
        out = "SOFA".to_string();
    }
    out.trim().to_ascii_uppercase()
}

/// Built-in sets first, then every `.sofa` in the user folder.
pub fn available_hrtf_sets() -> Vec<HrtfSetInfo> {
    let mut sets = Vec::new();
    for kind in SyntheticKind::ALL {
        sets.push(HrtfSetInfo {
            id: kind.id().to_string(),
            name: kind.name().to_string(),
            short: kind.short().to_string(),
            detail: match kind {
                SyntheticKind::Studio => "Generic adult head · synthetic HRTF".into(),
                SyntheticKind::Compact => "Smaller head · tighter ITD".into(),
                SyntheticKind::Broad => "Larger head · wider ITD".into(),
            },
            kind: HrtfSetKind::Synthetic(kind),
        });
    }

    let folder = hrtf_folder();
    let _ = std::fs::create_dir_all(&folder);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.is_file()
                        && path
                            .extension()
                            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("sofa"))
                            .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase()
            .cmp(
                &b.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_ascii_lowercase(),
            )
    });

    for path in files {
        let file_name = path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown.sofa".to_string());
        let name = path
            .file_stem()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| file_name.clone());
        sets.push(HrtfSetInfo {
            id: format!("sofa:{file_name}"),
            name: name.clone(),
            short: short_name(&path),
            detail: path.display().to_string(),
            kind: HrtfSetKind::Sofa(path),
        });
    }
    sets
}

/// Load one set at the requested output sample rate.
pub fn load_provider(
    kind: &HrtfSetKind,
    sample_rate: u32,
) -> Result<Arc<dyn HrtfProvider>, String> {
    let rate = (sample_rate as f32).clamp(8_000.0, 192_000.0);
    match kind {
        HrtfSetKind::Synthetic(synthetic) => Ok(Arc::new(SyntheticProvider::new(*synthetic, rate))),
        HrtfSetKind::Sofa(path) => Ok(Arc::new(SofaProvider::new(path, rate)?)),
    }
}

/// A source of HRIRs for any direction, always producing [`FILTER_LEN`] taps.
pub trait HrtfProvider: Send + Sync {
    fn fill_filter(&self, direction: [f32; 3], out: &mut Filter);
}

/* ------------------------------------------------------------------ */
/* Synthetic HRTF model                                                 */
/* ------------------------------------------------------------------ */

#[derive(Clone, Copy)]
struct SyntheticModel {
    head_radius: f32,
    shadow: f32,
    pinna: f32,
    rear: f32,
    torso: f32,
}

struct SyntheticProvider {
    model: SyntheticModel,
    sample_rate: f32,
    gain: f32,
}

impl SyntheticProvider {
    fn new(kind: SyntheticKind, sample_rate: f32) -> Self {
        let model = kind.model();
        let mut probe = Filter::new(FILTER_LEN);
        synthesize(&model, sample_rate, [0.0, 0.0, 1.0], &mut probe);
        let energy = filter_energy(&probe).max(1e-9);
        Self {
            model,
            sample_rate,
            gain: (2.0 / energy).sqrt().clamp(0.0, 100.0),
        }
    }
}

impl HrtfProvider for SyntheticProvider {
    fn fill_filter(&self, direction: [f32; 3], out: &mut Filter) {
        synthesize(&self.model, self.sample_rate, direction, out);
        for sample in out.left.iter_mut().chain(out.right.iter_mut()) {
            *sample = if sample.is_finite() {
                (*sample * self.gain).clamp(-8.0, 8.0)
            } else {
                0.0
            };
        }
    }
}

fn filter_energy(filter: &Filter) -> f32 {
    filter
        .left
        .iter()
        .chain(filter.right.iter())
        .filter(|sample| sample.is_finite())
        .map(|sample| sample * sample)
        .sum()
}

fn normalize3(value: [f32; 3]) -> [f32; 3] {
    let length = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
    if length < 1e-6 {
        [0.0, 0.0, 1.0]
    } else {
        [value[0] / length, value[1] / length, value[2] / length]
    }
}

fn add_tap(buffer: &mut [f32], delay_seconds: f32, gain: f32, pole: f32, sample_rate: f32) {
    if gain.abs() < 1e-6 {
        return;
    }
    let start = (delay_seconds * sample_rate).max(0.0);
    let index = start.floor() as usize;
    if index >= buffer.len() {
        return;
    }
    let fraction = start - index as f32;
    let one_minus = 1.0 - pole;
    let mut envelope = 1.0f32;
    for offset in 0..(buffer.len() - index) {
        let value = gain * one_minus * envelope;
        if fraction > 0.0 {
            buffer[index + offset] += value * (1.0 - fraction);
            if index + offset + 1 < buffer.len() {
                buffer[index + offset + 1] += value * fraction;
            }
        } else {
            buffer[index + offset] += value;
        }
        envelope *= pole;
    }
}

/// A small RBJ biquad used only while building an HRIR (offline per update).
#[derive(Clone, Copy)]
struct IrBiquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl IrBiquad {
    fn lowpass(sample_rate: f32, frequency: f32, q: f32) -> Self {
        let w0 = TAU * frequency.clamp(20.0, sample_rate * 0.45) / sample_rate;
        let cos = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - cos) * 0.5 / a0,
            b1: (1.0 - cos) / a0,
            b2: (1.0 - cos) * 0.5 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            ..Self::default()
        }
    }

    fn peaking(sample_rate: f32, frequency: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = TAU * frequency.clamp(20.0, sample_rate * 0.45) / sample_rate;
        let cos = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: -2.0 * cos / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha / a) / a0,
            ..Self::default()
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn run(&mut self, buffer: &mut [f32]) {
        for sample in buffer.iter_mut() {
            *sample = self.process(*sample);
        }
    }
}

impl Default for IrBiquad {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }
}

/// Build one synthetic HRIR pair. `direction` is `[right, up, front]`.
fn synthesize(model: &SyntheticModel, sample_rate: f32, direction: [f32; 3], out: &mut Filter) {
    debug_assert_eq!(out.left.len(), FILTER_LEN);
    debug_assert_eq!(out.right.len(), FILTER_LEN);
    out.left.fill(0.0);
    out.right.fill(0.0);

    let [right, up, front] = normalize3(direction);
    let horizontal = (right * right + front * front).sqrt();
    let lateral = if horizontal > 1e-5 {
        (right / horizontal).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let elevation = up.clamp(-1.0, 1.0);
    let rear = if horizontal > 1e-5 {
        (-front / horizontal).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let elevation_abs = elevation.abs();

    // --- ITD: Woodworth's spherical head formula. --------------------
    let lateral_angle = lateral.abs().asin();
    let itd = (model.head_radius / SPEED_OF_SOUND)
        * (lateral_angle + lateral_angle.sin())
        * (1.0 - 0.35 * elevation_abs);
    if lateral >= 0.0 {
        // Source on the right: the left ear is the far ear.
        out.ldelay = itd.min(MAX_DELAY_SECS);
        out.rdelay = 0.0;
    } else {
        out.ldelay = 0.0;
        out.rdelay = itd.min(MAX_DELAY_SECS);
    }

    // --- Pinna / elevation / front-back reflection structure. ---------
    let elevation_scale = (1.0 - 0.32 * elevation * model.pinna).clamp(0.55, 1.45);
    let rear_scale = 1.0 + 0.28 * rear * model.rear;
    let pinna_delays = [0.00019f32, 0.00033, 0.00050];
    let pinna_gains = [0.42f32, -0.36, 0.24];
    let far_scale = 0.82;
    let near_extra_delay = 0.000015;

    let build_ear = |buffer: &mut [f32], near: bool| {
        let shadow = if near {
            0.0
        } else {
            lateral.abs().powf(1.1) * model.shadow
        };
        let cutoff = (20_000.0 * (1.0 - 0.92 * shadow)).clamp(1_200.0, 20_000.0);
        let pole = (-TAU * cutoff / sample_rate).exp();

        // Direct sound through the head-shadow low-pass.
        add_tap(buffer, 0.0, 1.0, pole, sample_rate);

        // Pinna reflections: their delays shrink with elevation, which moves
        // the spectral notches upwards; the rear side uses a different comb.
        let ear_gain = if near { 1.0 } else { far_scale };
        let ear_delay = if near { 0.0 } else { near_extra_delay };
        for (index, (delay, gain)) in pinna_delays.iter().zip(pinna_gains).enumerate() {
            let mut gain = gain;
            if rear > 0.5 && index == 1 {
                gain = 0.22;
            }
            let delay = ear_delay + delay * elevation_scale * rear_scale * model.pinna;
            add_tap(
                buffer,
                delay,
                gain * ear_gain * model.pinna,
                pole,
                sample_rate,
            );
        }

        // Shoulder / torso reflection, strongest at low elevations.
        let torso_delay = 0.00115 * (1.0 + 0.15 * rear) * (1.0 - 0.25 * elevation_abs);
        let torso_gain = 0.10 * model.torso * (1.0 - 0.55 * elevation_abs) * (0.7 + 0.3 * rear);
        add_tap(buffer, torso_delay, torso_gain, pole, sample_rate);
    };

    build_ear(&mut out.left, lateral <= 0.0);
    build_ear(&mut out.right, lateral >= 0.0);

    // --- Rear spectral shaping: rear sources are darker and get a broad
    // 3 kHz notch, which is one of the main front/back cues. -------------
    let rear_cut = 18_000.0 - 8_000.0 * rear * model.rear;
    let mut rear_filter = IrBiquad::lowpass(sample_rate, rear_cut, 0.707);
    rear_filter.run(&mut out.left);
    rear_filter.reset_state();
    rear_filter.run(&mut out.right);

    if rear > 0.05 {
        let mut notch = IrBiquad::peaking(sample_rate, 3_200.0, 1.1, -3.5 * rear * model.rear);
        notch.run(&mut out.left);
        notch.reset_state();
        notch.run(&mut out.right);
    }

    // Above-the-head sources get a gentle presence lift; below-the-head
    // sources lose a little high frequency. This supplements the pinna notch
    // shift and makes up/down easier to tell apart on headphones.
    if elevation.abs() > 0.15 {
        let mut tilt = IrBiquad::peaking(sample_rate, 7_000.0, 0.8, 1.8 * elevation * model.pinna);
        tilt.run(&mut out.left);
        tilt.reset_state();
        tilt.run(&mut out.right);
    }

    // Avoid a truncation edge at the end of the 512-tap window.
    let fade = 16.min(FILTER_LEN);
    for buffer in [&mut out.left, &mut out.right] {
        for index in 0..fade {
            let position = FILTER_LEN - fade + index;
            buffer[position] *= index as f32 / fade as f32;
        }
    }
}

impl IrBiquad {
    fn reset_state(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/* ------------------------------------------------------------------ */
/* SOFA provider                                                        */
/* ------------------------------------------------------------------ */

struct SofaProvider {
    sample_rate: f32,
    sofa: Sofar,
    temp: Mutex<Filter>,
    gain: f32,
}

impl SofaProvider {
    fn new(path: &Path, sample_rate: f32) -> Result<Self, String> {
        if !path.is_file() {
            return Err(format!("HRTF file not found: {}", path.display()));
        }
        let sofa = OpenOptions::new()
            .sample_rate(sample_rate)
            .normalized(true)
            .open(path)
            .map_err(|error| format!("Cannot read SOFA file {}: {error}", path.display()))?;
        let filter_len = sofa.filter_len();
        if filter_len == 0 {
            return Err(format!(
                "SOFA file {} contains no HRIR samples",
                path.display()
            ));
        }
        let temp = Filter::new(filter_len);
        let mut provider = Self {
            sample_rate,
            sofa,
            temp: Mutex::new(temp),
            gain: 1.0,
        };

        // Calibrate the set so a frontal source has the same total energy as
        // the synthetic sets. This keeps A/B against stereo panning fair.
        let mut probe = Filter::new(FILTER_LEN);
        provider.render([0.0, 0.0, 1.0], &mut probe, 1.0);
        let energy = filter_energy(&probe).max(1e-9);
        provider.gain = (2.0 / energy).sqrt().clamp(0.0, 100.0);
        Ok(provider)
    }

    fn render(&self, direction: [f32; 3], out: &mut Filter, gain: f32) {
        let [right, up, front] = direction;
        let mut temp = match self.temp.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        // sofar uses x=front, y=left, z=up.
        self.sofa.filter(front, -right, up, &mut temp);

        let peak = temp
            .left
            .iter()
            .chain(temp.right.iter())
            .fold(0.0f32, |value, sample| value.max(sample.abs()));
        let onset = if peak > 1e-9 {
            let threshold = peak * 0.02;
            let find = |buffer: &[f32]| {
                buffer
                    .iter()
                    .position(|sample| sample.abs() > threshold)
                    .unwrap_or(0)
            };
            find(&temp.left).min(find(&temp.right))
        } else {
            0
        };

        let available = temp.left.len().saturating_sub(onset).min(FILTER_LEN);
        out.left.fill(0.0);
        out.right.fill(0.0);
        if available > 0 {
            for index in 0..available {
                let left = temp.left[onset + index] * gain;
                let right = temp.right[onset + index] * gain;
                out.left[index] = if left.is_finite() {
                    left.clamp(-8.0, 8.0)
                } else {
                    0.0
                };
                out.right[index] = if right.is_finite() {
                    right.clamp(-8.0, 8.0)
                } else {
                    0.0
                };
            }
        }

        // Remove the common propagation delay; keep the interaural difference.
        let onset_seconds = onset as f32 / self.sample_rate;
        let mut ldelay = (temp.ldelay - onset_seconds).max(0.0);
        let mut rdelay = (temp.rdelay - onset_seconds).max(0.0);
        let common = ldelay.min(rdelay);
        ldelay -= common;
        rdelay -= common;
        out.ldelay = ldelay.min(MAX_DELAY_SECS);
        out.rdelay = rdelay.min(MAX_DELAY_SECS);

        // Smooth the tail so a measured IR cannot end on a discontinuity.
        let fade = if available > 32 { 16 } else { 0 };
        for buffer in [&mut out.left, &mut out.right] {
            for index in 0..fade {
                let position = available - fade + index;
                buffer[position] *= (index + 1) as f32 / fade as f32;
            }
        }
    }
}

impl HrtfProvider for SofaProvider {
    fn fill_filter(&self, direction: [f32; 3], out: &mut Filter) {
        self.render(direction, out, self.gain);
    }
}

/* ------------------------------------------------------------------ */
/* Listener / scene                                                     */
/* ------------------------------------------------------------------ */

/// Listener-relative scene of one source. All world coordinates use
/// `x = right`, `y = up`, `z = front`, in metres.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scene {
    pub right: f32,
    pub up: f32,
    pub front: f32,
    pub distance: f32,
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
}

impl Scene {
    pub fn direction(&self) -> [f32; 3] {
        [self.right, self.up, self.front]
    }

    pub fn horizontal(&self) -> f32 {
        (self.right * self.right + self.front * self.front).sqrt()
    }
}

fn rotate_y(value: [f32; 3], angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    [
        value[0] * cos + value[2] * sin,
        value[1],
        -value[0] * sin + value[2] * cos,
    ]
}

fn rotate_x(value: [f32; 3], angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    [
        value[0],
        value[1] * cos - value[2] * sin,
        value[1] * sin + value[2] * cos,
    ]
}

fn rotate_z(value: [f32; 3], angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    [
        value[0] * cos - value[1] * sin,
        value[0] * sin + value[1] * cos,
        value[2],
    ]
}

/// Convert a world-space source position into the listener's head frame.
/// `yaw` is positive to the right, `pitch` positive up, `roll` positive
/// clockwise from the listener's point of view.
pub fn scene(source: [f32; 3], yaw_deg: f32, pitch_deg: f32, roll_deg: f32) -> Scene {
    let distance = (source[0] * source[0] + source[1] * source[1] + source[2] * source[2]).sqrt();
    if distance < 1e-5 {
        return Scene {
            right: 0.0,
            up: 0.0,
            front: 1.0,
            distance: MIN_DISTANCE,
            azimuth_deg: 0.0,
            elevation_deg: 0.0,
        };
    }

    let mut value = rotate_y(source, -yaw_deg.to_radians());
    value = rotate_x(value, pitch_deg.to_radians());
    value = rotate_z(value, -roll_deg.to_radians());

    let horizontal = (value[0] * value[0] + value[2] * value[2]).sqrt();
    Scene {
        right: value[0] / distance,
        up: value[1] / distance,
        front: value[2] / distance,
        distance,
        azimuth_deg: value[0].atan2(value[2]).to_degrees(),
        elevation_deg: value[1].atan2(horizontal.max(1e-9)).to_degrees(),
    }
}

/// Equal-power stereo gains for the fallback panning path.
fn pan_gains(scene: &Scene) -> (f32, f32) {
    let pan = if scene.horizontal() > 0.1 {
        scene.right / scene.horizontal()
    } else {
        0.0
    }
    .clamp(-1.0, 1.0);
    let theta = (pan + 1.0) * FRAC_PI_4;
    (theta.cos() * SQRT_2, theta.sin() * SQRT_2)
}

/// Map the 2-D pad (`-1..1`, up = front) plus height in metres to a world
/// position. A source is never placed closer than 0.5 m, which keeps it
/// outside the listener's head and avoids the "inside the head" effect.
pub fn position_from_pad(x: f32, y: f32, height: f32, room: f32) -> [f32; 3] {
    let half = (room.clamp(2.0, 24.0) * 0.5).max(MIN_DISTANCE + 0.1);
    let mut right = x.clamp(-1.0, 1.0) * half;
    let mut front = y.clamp(-1.0, 1.0) * half;
    let horizontal = (right * right + front * front).sqrt();
    if horizontal < MIN_DISTANCE {
        if horizontal > 1e-4 {
            let scale = MIN_DISTANCE / horizontal;
            right *= scale;
            front *= scale;
        } else {
            right = 0.0;
            front = MIN_DISTANCE;
        }
    }
    [right, height.clamp(-2.5, 2.5), front]
}

/// Inverse of [`position_from_pad`], for drawing the speaker on the pad.
pub fn pad_from_position(position: [f32; 3], room: f32) -> (f32, f32) {
    let half = (room.clamp(2.0, 24.0) * 0.5).max(MIN_DISTANCE + 0.1);
    (
        (position[0] / half).clamp(-1.0, 1.0),
        (position[2] / half).clamp(-1.0, 1.0),
    )
}

/* ------------------------------------------------------------------ */
/* Test modes                                                           */
/* ------------------------------------------------------------------ */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestMode {
    Off,
    Front,
    Back,
    Above,
    Below,
    OrbitHorizontal,
    OrbitVertical,
    SweepFrontBack,
    SweepLeftRight,
}

impl TestMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "TEST OFF",
            Self::Front => "FRONT",
            Self::Back => "BACK",
            Self::Above => "ABOVE",
            Self::Below => "BELOW",
            Self::OrbitHorizontal => "ORBIT",
            Self::OrbitVertical => "VERT",
            Self::SweepFrontBack => "F/B",
            Self::SweepLeftRight => "L/R",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Front,
            Self::Front => Self::Back,
            Self::Back => Self::Above,
            Self::Above => Self::Below,
            Self::Below => Self::OrbitHorizontal,
            Self::OrbitHorizontal => Self::OrbitVertical,
            Self::OrbitVertical => Self::SweepFrontBack,
            Self::SweepFrontBack => Self::SweepLeftRight,
            Self::SweepLeftRight => Self::Off,
        }
    }

    pub fn active(self) -> bool {
        self != Self::Off
    }

    fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::Front,
            2 => Self::Back,
            3 => Self::Above,
            4 => Self::Below,
            5 => Self::OrbitHorizontal,
            6 => Self::OrbitVertical,
            7 => Self::SweepFrontBack,
            8 => Self::SweepLeftRight,
            _ => Self::Off,
        }
    }

    fn as_u32(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Front => 1,
            Self::Back => 2,
            Self::Above => 3,
            Self::Below => 4,
            Self::OrbitHorizontal => 5,
            Self::OrbitVertical => 6,
            Self::SweepFrontBack => 7,
            Self::SweepLeftRight => 8,
        }
    }
}

/* ------------------------------------------------------------------ */
/* Shared state                                                         */
/* ------------------------------------------------------------------ */

fn load_f32(value: &AtomicU32) -> f32 {
    f32::from_bits(value.load(Ordering::Relaxed))
}

fn store_f32(value: &AtomicU32, sample: f32) {
    value.store(sample.to_bits(), Ordering::Relaxed);
}

fn load_position(slot: &[AtomicU32; 3]) -> [f32; 3] {
    [load_f32(&slot[0]), load_f32(&slot[1]), load_f32(&slot[2])]
}

struct ProviderState {
    id: String,
    generation: u32,
    by_rate: HashMap<u32, Arc<dyn HrtfProvider>>,
}

/// State shared between the UI thread, the audio thread and the test source.
pub struct SpatialShared {
    position: [AtomicU32; 3],
    room: AtomicU32,
    reverb: AtomicU32,
    yaw: AtomicU32,
    pitch: AtomicU32,
    roll: AtomicU32,
    hrtf_enabled: AtomicBool,
    enabled: AtomicBool,
    test_mode: AtomicU32,
    sample_rate: AtomicU32,
    providers: Mutex<ProviderState>,
}

impl SpatialShared {
    pub fn new() -> Self {
        Self {
            position: [
                AtomicU32::new(0.0f32.to_bits()),
                AtomicU32::new(0.0f32.to_bits()),
                AtomicU32::new(MIN_DISTANCE.to_bits()),
            ],
            room: AtomicU32::new(8.0f32.to_bits()),
            reverb: AtomicU32::new(0.0f32.to_bits()),
            yaw: AtomicU32::new(0.0f32.to_bits()),
            pitch: AtomicU32::new(0.0f32.to_bits()),
            roll: AtomicU32::new(0.0f32.to_bits()),
            hrtf_enabled: AtomicBool::new(true),
            enabled: AtomicBool::new(true),
            test_mode: AtomicU32::new(0),
            sample_rate: AtomicU32::new(48_000),
            providers: Mutex::new(ProviderState {
                id: String::new(),
                generation: 0,
                by_rate: HashMap::new(),
            }),
        }
    }

    pub fn position(&self) -> [f32; 3] {
        load_position(&self.position)
    }

    pub fn set_position(&self, position: [f32; 3]) {
        for (slot, value) in self.position.iter().zip(position) {
            store_f32(slot, value);
        }
    }

    pub fn room(&self) -> f32 {
        load_f32(&self.room).clamp(2.0, 24.0)
    }

    pub fn set_room(&self, meters: f32) {
        store_f32(&self.room, meters.clamp(2.0, 24.0));
    }

    pub fn reverb(&self) -> f32 {
        load_f32(&self.reverb).clamp(0.0, 1.0)
    }

    pub fn set_reverb(&self, wet: f32) {
        store_f32(&self.reverb, wet.clamp(0.0, 1.0));
    }

    pub fn head_yaw(&self) -> f32 {
        load_f32(&self.yaw)
    }

    pub fn set_head_yaw(&self, degrees: f32) {
        store_f32(&self.yaw, degrees.clamp(-180.0, 180.0));
    }

    pub fn head_pitch(&self) -> f32 {
        load_f32(&self.pitch)
    }

    /// Pitch/roll are part of the scene model; the compact panel exposes yaw
    /// only, but future head-tracking can drive these without touching the
    /// renderer.
    #[allow(dead_code)]
    pub fn set_head_pitch(&self, degrees: f32) {
        store_f32(&self.pitch, degrees.clamp(-89.0, 89.0));
    }

    pub fn head_roll(&self) -> f32 {
        load_f32(&self.roll)
    }

    #[allow(dead_code)]
    pub fn set_head_roll(&self, degrees: f32) {
        store_f32(&self.roll, degrees.clamp(-180.0, 180.0));
    }

    pub fn hrtf_enabled(&self) -> bool {
        self.hrtf_enabled.load(Ordering::Relaxed)
    }

    pub fn set_hrtf_enabled(&self, enabled: bool) {
        self.hrtf_enabled.store(enabled, Ordering::Relaxed);
    }

    /// True while the spatial chain is applied. When it is off the source
    /// passes the untouched stereo signal through (no pan, HRTF or reverb),
    /// which is what the UI uses while the component is hidden or switched off.
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn test_mode(&self) -> TestMode {
        TestMode::from_u32(self.test_mode.load(Ordering::Relaxed))
    }

    pub fn set_test_mode(&self, mode: TestMode) {
        self.test_mode.store(mode.as_u32(), Ordering::Relaxed);
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed).max(8_000)
    }

    pub fn set_sample_rate(&self, rate: u32) {
        self.sample_rate.store(rate.max(8_000), Ordering::Relaxed);
    }

    /// Replace the whole set. Used by the engine when the user picks a new
    /// HRTF set: every active source notices the generation bump and installs
    /// the new filter with a crossfade.
    pub fn install_set(&self, id: &str, providers: Vec<(u32, Arc<dyn HrtfProvider>)>) {
        let mut state = match self.providers.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.id = id.to_string();
        state.by_rate.clear();
        for (rate, provider) in providers {
            state.by_rate.insert(rate, provider);
        }
        state.generation = state.generation.wrapping_add(1);
    }

    /// Add (or replace) the provider for one sample rate without changing the
    /// set as a whole.
    pub fn ensure_provider(&self, id: &str, rate: u32, provider: Arc<dyn HrtfProvider>) {
        let mut state = match self.providers.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.id != id {
            state.id = id.to_string();
            state.by_rate.clear();
        }
        state.by_rate.insert(rate, provider);
        state.generation = state.generation.wrapping_add(1);
    }

    pub fn provider_for(&self, rate: u32) -> Option<Arc<dyn HrtfProvider>> {
        let state = match self.providers.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.by_rate.get(&rate).cloned()
    }

    pub fn generation(&self) -> u32 {
        let state = match self.providers.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.generation
    }
}

impl Default for SpatialShared {
    fn default() -> Self {
        Self::new()
    }
}

/* ------------------------------------------------------------------ */
/* HRTF renderer wrapper                                                */
/* ------------------------------------------------------------------ */

struct HrtfRenderer {
    renderer: Renderer,
    filter: Filter,
    last_direction: [f32; 3],
    blocks_since_update: u32,
    ready: bool,
}

impl HrtfRenderer {
    fn new(sample_rate: u32) -> Option<Self> {
        let sample_rate = sample_rate as f32;
        let plan = RendererPlan::builder(FILTER_LEN)
            .with_sample_rate(sample_rate)
            .with_partition_len(PARTITION_LEN)
            .build()
            .ok()?;
        let crossfade = ((sample_rate * 0.012).round() as usize).max(16);
        let renderer = Renderer::builder(&plan)
            .with_filter_transition(FilterTransition::crossfade(crossfade))
            .with_max_delay(MAX_DELAY_SECS)
            .build()
            .ok()?;
        Some(Self {
            renderer,
            filter: Filter::new(FILTER_LEN),
            last_direction: [0.0, 0.0, 1.0],
            blocks_since_update: 0,
            ready: false,
        })
    }

    fn ensure_filter(&mut self, provider: &dyn HrtfProvider, direction: [f32; 3], force: bool) {
        let direction = normalize3(direction);
        let dot = (self.last_direction[0] * direction[0]
            + self.last_direction[1] * direction[1]
            + self.last_direction[2] * direction[2])
            .clamp(-1.0, 1.0);
        let angle = dot.acos().to_degrees();
        let moving = angle > 0.05;
        let update =
            force || angle > 1.0 || (moving && self.blocks_since_update >= 4) || !self.ready;

        if update {
            provider.fill_filter(direction, &mut self.filter);
            if self.renderer.set_filter(&self.filter).is_ok() {
                self.last_direction = direction;
                self.blocks_since_update = 0;
                self.ready = true;
            }
        }
        self.blocks_since_update = self.blocks_since_update.saturating_add(1);
    }

    fn process(&mut self, input: &[f32], left: &mut [f32], right: &mut [f32]) -> bool {
        if !self.ready {
            return false;
        }
        self.renderer.process_block(input, left, right).is_ok()
    }

    fn reset(&mut self) {
        self.renderer.reset();
        self.blocks_since_update = 0;
    }
}

/* ------------------------------------------------------------------ */
/* Air absorption / early reflections / reverb                          */
/* ------------------------------------------------------------------ */

struct AirFilter {
    coefficient: f32,
    state: f32,
}

impl AirFilter {
    fn new() -> Self {
        Self {
            coefficient: 0.0,
            state: 0.0,
        }
    }

    fn set_cutoff(&mut self, sample_rate: f32, cutoff: f32) {
        self.coefficient = (-TAU * cutoff.clamp(200.0, sample_rate * 0.45) / sample_rate).exp();
    }

    fn process(&mut self, sample: f32) -> f32 {
        self.state = (1.0 - self.coefficient) * sample + self.coefficient * self.state;
        self.state
    }

    fn reset(&mut self) {
        self.state = 0.0;
    }
}

struct Comb {
    buffer: Vec<f32>,
    index: usize,
    store: f32,
    feedback: f32,
    damp: f32,
}

impl Comb {
    fn new(samples: usize) -> Self {
        Self {
            buffer: vec![0.0; samples.max(1)],
            index: 0,
            store: 0.0,
            feedback: 0.7,
            damp: 0.28,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.buffer[self.index];
        self.store = output * (1.0 - self.damp) + self.store * self.damp;
        self.buffer[self.index] = input + self.store * self.feedback;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.index = 0;
        self.store = 0.0;
    }
}

struct Allpass {
    buffer: Vec<f32>,
    index: usize,
    gain: f32,
}

impl Allpass {
    fn new(samples: usize) -> Self {
        Self {
            buffer: vec![0.0; samples.max(1)],
            index: 0,
            gain: 0.5,
        }
    }

    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.index];
        let output = buffered - input;
        self.buffer[self.index] = input + buffered * self.gain;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.index = 0;
    }
}

#[derive(Clone, Copy, Default)]
struct EarlyTap {
    delay: f32,
    gain_l: f32,
    gain_r: f32,
}

struct Space {
    line: Vec<f32>,
    write: usize,
    taps: [EarlyTap; 4],
    target: [EarlyTap; 4],
    combs_l: Vec<Comb>,
    combs_r: Vec<Comb>,
    allpass_l: Vec<Allpass>,
    allpass_r: Vec<Allpass>,
    sample_rate: f32,
}

impl Space {
    fn new(sample_rate: u32) -> Self {
        let fs = sample_rate as f32;
        let scale = fs / 44_100.0;
        let comb_delays_l = [1_557.0f32, 1_617.0, 1_491.0, 1_422.0];
        let comb_delays_r = [1_583.0f32, 1_641.0, 1_511.0, 1_447.0];
        let allpass_delays_l = [556.0f32, 225.0];
        let allpass_delays_r = [579.0f32, 248.0];
        let line_len = (0.08 * fs).ceil() as usize + 8;
        Self {
            line: vec![0.0; line_len],
            write: 0,
            taps: [EarlyTap::default(); 4],
            target: [EarlyTap::default(); 4],
            combs_l: comb_delays_l
                .iter()
                .map(|delay| Comb::new((delay * scale) as usize))
                .collect(),
            combs_r: comb_delays_r
                .iter()
                .map(|delay| Comb::new((delay * scale) as usize))
                .collect(),
            allpass_l: allpass_delays_l
                .iter()
                .map(|delay| Allpass::new((delay * scale) as usize))
                .collect(),
            allpass_r: allpass_delays_r
                .iter()
                .map(|delay| Allpass::new((delay * scale) as usize))
                .collect(),
            sample_rate: fs,
        }
    }

    fn read_delay(&self, delay: f32) -> f32 {
        let length = self.line.len() as f32;
        let position = (self.write as f32 - delay.max(0.0)).rem_euclid(length);
        let index = position.floor() as usize;
        let fraction = position - index as f32;
        let first = self.line[index % self.line.len()];
        let second = self.line[(index + 1) % self.line.len()];
        first + (second - first) * fraction
    }

    fn configure(&mut self, room: f32, yaw: f32, distance_ratio: f32) {
        let room_scale = (room / 8.0).clamp(0.55, 2.4);
        let directions = [
            [-0.80f32, 0.0, 0.60],
            [0.80, 0.0, 0.60],
            [-0.90, 0.0, -0.50],
            [0.90, 0.0, -0.50],
        ];
        let delays = [0.009f32, 0.013, 0.019, 0.025];
        let gains = [0.20f32, 0.20, 0.14, 0.14];
        for index in 0..4 {
            let rotated = rotate_y(directions[index], -yaw.to_radians());
            let pan = rotated[0].clamp(-1.0, 1.0);
            let theta = (pan + 1.0) * FRAC_PI_4;
            let gain = gains[index] * (0.55 + 0.45 * distance_ratio);
            self.target[index] = EarlyTap {
                delay: delays[index] * room_scale + 0.002,
                gain_l: theta.cos() * SQRT_2 * gain,
                gain_r: theta.sin() * SQRT_2 * gain,
            };
        }

        for tap in &mut self.taps {
            tap.delay = tap.delay.max(0.001);
        }

        let feedback = 0.68 + 0.24 * ((room - 2.0) / 22.0).clamp(0.0, 1.0);
        for comb in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            comb.feedback = feedback;
        }
    }

    /// Adds early reflections and the room tail to `left`/`right`.
    #[allow(clippy::too_many_arguments)]
    fn process_block(
        &mut self,
        mono: &[f32],
        reverb: f32,
        room: f32,
        yaw: f32,
        distance_ratio: f32,
        left: &mut [f32],
        right: &mut [f32],
    ) {
        if reverb <= 0.0001 {
            // Still advance the input line so enabling reverb later does not
            // replay stale history.
            for sample in mono {
                self.line[self.write] = *sample;
                self.write = (self.write + 1) % self.line.len();
            }
            return;
        }

        self.configure(room, yaw, distance_ratio);
        let smoothing = 0.28;
        for index in 0..4 {
            self.taps[index].delay +=
                (self.target[index].delay - self.taps[index].delay) * smoothing;
            self.taps[index].gain_l +=
                (self.target[index].gain_l - self.taps[index].gain_l) * smoothing;
            self.taps[index].gain_r +=
                (self.target[index].gain_r - self.taps[index].gain_r) * smoothing;
        }

        let send = 0.30 + 0.70 * distance_ratio;
        let tail_level = reverb * 0.9;
        let early_level = reverb;
        for (frame, sample) in mono.iter().enumerate() {
            self.line[self.write] = *sample;
            self.write = (self.write + 1) % self.line.len();

            let mut early_l = 0.0;
            let mut early_r = 0.0;
            for tap in &self.taps {
                let delayed = self.read_delay(tap.delay * self.sample_rate);
                early_l += delayed * tap.gain_l;
                early_r += delayed * tap.gain_r;
            }

            let feed = *sample * send;
            let mut wet_sum_l = 0.0;
            let mut wet_sum_r = 0.0;
            for comb in &mut self.combs_l {
                wet_sum_l += comb.process(feed);
            }
            for comb in &mut self.combs_r {
                wet_sum_r += comb.process(feed);
            }
            let mut tail_l = wet_sum_l * 0.25;
            let mut tail_r = wet_sum_r * 0.25;
            for allpass in &mut self.allpass_l {
                tail_l = allpass.process(tail_l);
            }
            for allpass in &mut self.allpass_r {
                tail_r = allpass.process(tail_r);
            }

            left[frame] += early_l * early_level + tail_l * tail_level;
            right[frame] += early_r * early_level + tail_r * tail_level;
        }
    }

    fn reset(&mut self) {
        self.line.fill(0.0);
        self.write = 0;
        self.taps = [EarlyTap::default(); 4];
        self.target = [EarlyTap::default(); 4];
        for comb in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            comb.reset();
        }
        for allpass in self.allpass_l.iter_mut().chain(self.allpass_r.iter_mut()) {
            allpass.reset();
        }
    }
}

/* ------------------------------------------------------------------ */
/* Spatial source                                                       */
/* ------------------------------------------------------------------ */

#[derive(Clone, Copy, PartialEq, Eq)]
enum RenderPath {
    Pan,
    Hrtf,
}

/// Wraps a decoder (or the test signal) and renders it as one binaural object.
pub struct SpatialSource<S> {
    inner: S,
    shared: Arc<SpatialShared>,
    in_channels: usize,
    sample_rate: u32,
    input_frame: [f32; 8],
    mono: Vec<f32>,
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
    dry_mix: f32,
    left: Vec<f32>,
    right: Vec<f32>,
    output: Vec<f32>,
    output_pos: usize,
    output_len: usize,
    finished: bool,
    hrtf: Option<HrtfRenderer>,
    provider: Option<Arc<dyn HrtfProvider>>,
    provider_generation: u32,
    active_path: RenderPath,
    fade: f32,
    air: AirFilter,
    space: Space,
    scene: Scene,
    distance_gain: f32,
    pan_l: f32,
    pan_r: f32,
}

impl<S> SpatialSource<S>
where
    S: Source<Item = f32>,
{
    pub fn new(inner: S, shared: Arc<SpatialShared>) -> Self {
        let in_channels = inner.channels().get().max(1) as usize;
        let sample_rate = inner.sample_rate().get().max(8_000);
        shared.set_sample_rate(sample_rate);

        let provider = shared.provider_for(sample_rate);
        let mut hrtf = HrtfRenderer::new(sample_rate);
        let position = shared.position();
        let initial_scene = scene(
            position,
            shared.head_yaw(),
            shared.head_pitch(),
            shared.head_roll(),
        );

        if let (Some(renderer), Some(provider)) = (hrtf.as_mut(), provider.as_deref()) {
            renderer.ensure_filter(provider, initial_scene.direction(), true);
        }

        let path = if shared.hrtf_enabled() && hrtf.is_some() {
            RenderPath::Hrtf
        } else {
            RenderPath::Pan
        };
        let initial_distance = (MIN_DISTANCE / initial_scene.distance.max(MIN_DISTANCE)).min(1.0);
        let (initial_pan_l, initial_pan_r) = pan_gains(&initial_scene);

        Self {
            inner,
            shared: shared.clone(),
            in_channels,
            sample_rate,
            input_frame: [0.0; 8],
            mono: vec![0.0; PARTITION_LEN],
            dry_l: vec![0.0; PARTITION_LEN],
            dry_r: vec![0.0; PARTITION_LEN],
            dry_mix: if shared.enabled() { 0.0 } else { 1.0 },
            left: vec![0.0; PARTITION_LEN],
            right: vec![0.0; PARTITION_LEN],
            output: vec![0.0; PARTITION_LEN * 2],
            output_pos: 0,
            output_len: 0,
            finished: false,
            hrtf,
            provider,
            provider_generation: shared.generation(),
            active_path: path,
            fade: 1.0,
            air: AirFilter::new(),
            space: Space::new(sample_rate),
            scene: initial_scene,
            distance_gain: initial_distance,
            pan_l: initial_pan_l,
            pan_r: initial_pan_r,
        }
    }

    fn fill_block(&mut self) -> usize {
        let mut valid = 0usize;
        for index in 0..PARTITION_LEN {
            let mut ended = false;
            for channel in 0..self.in_channels {
                let sample = match self.inner.next() {
                    Some(sample) => sample,
                    None => {
                        ended = true;
                        break;
                    }
                };
                if channel < self.input_frame.len() {
                    self.input_frame[channel] = sample;
                }
            }
            if ended {
                break;
            }
            let used = self.in_channels.min(self.input_frame.len());
            let mut sum = 0.0;
            for channel in 0..used {
                sum += self.input_frame[channel];
            }
            self.mono[index] = (sum / used.max(1) as f32).clamp(-1.0, 1.0);
            // Untouched stereo, used while spatial rendering is switched off.
            let dry_left = self.input_frame[0];
            let dry_right = if used > 1 {
                self.input_frame[1]
            } else {
                dry_left
            };
            self.dry_l[index] = dry_left.clamp(-1.0, 1.0);
            self.dry_r[index] = dry_right.clamp(-1.0, 1.0);
            valid += 1;
        }
        if valid < PARTITION_LEN {
            self.mono[valid..].fill(0.0);
            self.dry_l[valid..].fill(0.0);
            self.dry_r[valid..].fill(0.0);
        }
        valid
    }

    fn process_block(&mut self) {
        let enabled = self.shared.enabled();
        if !enabled && self.dry_mix >= 0.999 {
            // Fully bypassed: hand the untouched stereo through and skip every
            // filter, so a hidden or switched-off component really is inert.
            for index in 0..PARTITION_LEN {
                self.output[index * 2] = self.dry_l[index];
                self.output[index * 2 + 1] = self.dry_r[index];
            }
            return;
        }

        let position = self.shared.position();
        let scene = scene(
            position,
            self.shared.head_yaw(),
            self.shared.head_pitch(),
            self.shared.head_roll(),
        );
        self.scene = scene;

        let distance = scene.distance.max(MIN_DISTANCE);
        let distance_gain = (MIN_DISTANCE / distance).min(1.0);
        let distance_ratio = (1.0 - distance_gain).clamp(0.0, 1.0);
        let air_cutoff = (20_000.0 * (-distance / 35.0).exp()).clamp(2_500.0, 20_000.0);
        self.air.set_cutoff(self.sample_rate as f32, air_cutoff);
        for sample in &mut self.mono {
            *sample = self.air.process(*sample);
        }

        let generation = self.shared.generation();
        if generation != self.provider_generation {
            self.provider = self.shared.provider_for(self.sample_rate);
            self.provider_generation = generation;
            if let Some(renderer) = &mut self.hrtf {
                renderer.ready = false;
            }
        }
        let provider = self.provider.clone();
        let hrtf_available = self.hrtf.is_some() && provider.is_some();
        let desired_path = if self.shared.hrtf_enabled() && hrtf_available {
            RenderPath::Hrtf
        } else {
            RenderPath::Pan
        };

        let mut switching = false;
        if !matches!(
            (&self.active_path, &desired_path),
            (RenderPath::Pan, RenderPath::Pan) | (RenderPath::Hrtf, RenderPath::Hrtf)
        ) {
            if self.fade <= 0.0001 {
                self.active_path = desired_path;
            } else {
                switching = true;
            }
        }
        let target_fade = if switching { 0.0 } else { 1.0 };

        let target_pan = pan_gains(&self.scene);
        let mut pan_output = false;

        self.left.fill(0.0);
        self.right.fill(0.0);

        let path = match (&self.active_path, hrtf_available, desired_path) {
            (RenderPath::Hrtf, true, _) => RenderPath::Hrtf,
            _ => RenderPath::Pan,
        };

        match path {
            RenderPath::Hrtf => {
                let mut rendered = false;
                if let (Some(renderer), Some(provider)) = (self.hrtf.as_mut(), provider.as_deref())
                {
                    renderer.ensure_filter(provider, self.scene.direction(), !renderer.ready);
                    rendered = renderer.process(&self.mono, &mut self.left, &mut self.right);
                }
                if !rendered {
                    self.fill_pan();
                    pan_output = true;
                }
            }
            RenderPath::Pan => {
                self.fill_pan();
                pan_output = true;
            }
        }

        // Smooth the dry gain per sample so fast drags cannot zipper.
        let count = PARTITION_LEN as f32;
        let distance_start = self.distance_gain;
        let distance_step = (distance_gain - distance_start) / count;
        let pan_l_start = self.pan_l;
        let pan_r_start = self.pan_r;
        let pan_l_step = (target_pan.0 - pan_l_start) / count;
        let pan_r_step = (target_pan.1 - pan_r_start) / count;
        for index in 0..PARTITION_LEN {
            let position = (index + 1) as f32;
            let distance = distance_start + distance_step * position;
            let pan_l = pan_l_start + pan_l_step * position;
            let pan_r = pan_r_start + pan_r_step * position;
            let (gain_l, gain_r) = if pan_output {
                (pan_l * distance, pan_r * distance)
            } else {
                (distance, distance)
            };
            self.left[index] *= gain_l;
            self.right[index] *= gain_r;
        }
        self.distance_gain = distance_gain;
        self.pan_l = target_pan.0;
        self.pan_r = target_pan.1;

        self.space.process_block(
            &self.mono,
            self.shared.reverb(),
            self.shared.room(),
            self.shared.head_yaw(),
            distance_ratio,
            &mut self.left,
            &mut self.right,
        );

        let step = 1.0 / PARTITION_LEN as f32;
        let mut fade = self.fade;
        let dry_target = if enabled { 0.0 } else { 1.0 };
        let mut dry_mix = self.dry_mix;
        for index in 0..PARTITION_LEN {
            if fade < target_fade {
                fade = (fade + step).min(target_fade);
            } else if fade > target_fade {
                fade = (fade - step).max(target_fade);
            }
            if dry_mix < dry_target {
                dry_mix = (dry_mix + step).min(dry_target);
            } else if dry_mix > dry_target {
                dry_mix = (dry_mix - step).max(dry_target);
            }
            let dry = dry_mix;
            let left = (self.left[index] * (1.0 - dry) + self.dry_l[index] * dry).clamp(-1.0, 1.0);
            let right =
                (self.right[index] * (1.0 - dry) + self.dry_r[index] * dry).clamp(-1.0, 1.0);
            self.output[index * 2] = left;
            self.output[index * 2 + 1] = right;
        }
        self.fade = fade;
        self.dry_mix = dry_mix;

        if switching && fade <= 0.0001 {
            self.active_path = desired_path;
        }
    }

    fn fill_pan(&mut self) {
        self.left.copy_from_slice(&self.mono);
        self.right.copy_from_slice(&self.mono);
    }

    fn reset(&mut self) {
        if let Some(renderer) = &mut self.hrtf {
            renderer.reset();
        }
        self.air.reset();
        self.space.reset();
        self.output_pos = 0;
        self.output_len = 0;
        self.finished = false;
    }
}

impl<S> Iterator for SpatialSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.output_pos < self.output_len {
            let sample = self.output[self.output_pos];
            self.output_pos += 1;
            return Some(sample);
        }
        if self.finished {
            return None;
        }

        let valid = self.fill_block();
        if valid == 0 {
            self.finished = true;
            return None;
        }
        self.process_block();
        self.output_pos = 0;
        self.output_len = valid * 2;
        if valid < PARTITION_LEN {
            self.finished = true;
        }
        let sample = self.output[0];
        self.output_pos = 1;
        Some(sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (low, high) = self.inner.size_hint();
        let factor = 2.0 / self.in_channels.max(1) as f32;
        (
            (low as f32 * factor) as usize,
            high.map(|value| (value as f32 * factor) as usize),
        )
    }
}

impl<S> Source for SpatialSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> NonZero<u16> {
        NonZero::new(2).unwrap_or(NonZero::<u16>::MIN)
    }

    fn sample_rate(&self) -> NonZero<u32> {
        NonZero::new(self.sample_rate).unwrap_or(NonZero::<u32>::MIN)
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(pos)?;
        self.reset();
        Ok(())
    }
}

/* ------------------------------------------------------------------ */
/* Pink-noise test signal                                               */
/* ------------------------------------------------------------------ */

/// Infinite pink-noise burst source that drives a source around the listener.
/// It writes the current position into [`SpatialShared`], so the music source
/// and the UI follow the same object.
pub struct TestSignal {
    shared: Arc<SpatialShared>,
    sample_rate: u32,
    samples: u64,
    mode: TestMode,
    rng: u32,
    pink: [f32; 7],
    highpass_x: f32,
    highpass_y: f32,
}

impl TestSignal {
    pub fn new(sample_rate: u32, shared: Arc<SpatialShared>) -> Self {
        Self {
            shared,
            sample_rate: sample_rate.max(8_000),
            samples: 0,
            mode: TestMode::Off,
            rng: 0x9E37_79B9,
            pink: [0.0; 7],
            highpass_x: 0.0,
            highpass_y: 0.0,
        }
    }

    fn random(&mut self) -> f32 {
        let mut value = self.rng;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.rng = value;
        (value as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn write_position(&mut self, seconds: f32) {
        let [x, y, z] = match self.mode {
            TestMode::Off => return,
            TestMode::Front => [0.0, 0.0, 1.25],
            TestMode::Back => [0.0, 0.0, -1.25],
            TestMode::Above => [0.0, 1.25, 0.35],
            TestMode::Below => [0.0, -1.25, 0.35],
            TestMode::OrbitHorizontal => {
                let angle = TAU * seconds / 8.0;
                [angle.sin() * 1.25, 0.0, angle.cos() * 1.25]
            }
            TestMode::OrbitVertical => {
                let angle = TAU * seconds / 8.0;
                [0.0, angle.sin() * 1.25, angle.cos() * 1.25]
            }
            TestMode::SweepFrontBack => {
                let phase = (seconds / 8.0).fract();
                let angle = if phase < 0.5 {
                    PI * (phase * 2.0)
                } else {
                    PI * (2.0 - phase * 2.0)
                };
                [angle.sin() * 1.25, 0.0, angle.cos() * 1.25]
            }
            TestMode::SweepLeftRight => {
                let angle = TAU * seconds / 6.0;
                [angle.sin() * 1.25, 0.0, angle.cos() * 1.25 * 0.25 + 0.35]
            }
        };
        self.shared.set_position([x, y, z]);
    }

    fn pink_sample(&mut self) -> f32 {
        let white = self.random();
        self.pink[0] = 0.99886 * self.pink[0] + white * 0.0555179;
        self.pink[1] = 0.99332 * self.pink[1] + white * 0.0750759;
        self.pink[2] = 0.96900 * self.pink[2] + white * 0.153852;
        self.pink[3] = 0.86650 * self.pink[3] + white * 0.3104856;
        self.pink[4] = 0.55000 * self.pink[4] + white * 0.5329522;
        self.pink[5] = -0.7616 * self.pink[5] - white * 0.0168980;
        let pink = self.pink[0]
            + self.pink[1]
            + self.pink[2]
            + self.pink[3]
            + self.pink[4]
            + self.pink[5]
            + self.pink[6]
            + white * 0.5362;
        self.pink[6] = white * 0.115926;
        pink * 0.11
    }
}

impl Iterator for TestSignal {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let mode = self.shared.test_mode();
        if mode != self.mode {
            self.mode = mode;
            self.samples = 0;
        }
        if self.mode == TestMode::Off {
            return Some(0.0);
        }

        if self.samples.is_multiple_of(PARTITION_LEN as u64) {
            let seconds = self.samples as f32 / self.sample_rate as f32;
            self.write_position(seconds);
        }

        let seconds = self.samples as f32 / self.sample_rate as f32;
        let cycle = seconds.rem_euclid(2.0);
        let gate = if cycle < 0.02 {
            cycle / 0.02
        } else if cycle < 1.6 {
            1.0
        } else if cycle < 1.62 {
            (1.62 - cycle) / 0.02
        } else {
            0.0
        };

        let mut sample = self.pink_sample() * gate * 0.55;

        // Remove any DC the pink filter leaves behind.
        let highpassed = sample - self.highpass_x + 0.995 * self.highpass_y;
        self.highpass_x = sample;
        self.highpass_y = highpassed;
        sample = highpassed;

        self.samples = self.samples.wrapping_add(1);
        Some(sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }
}

impl Source for TestSignal {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> NonZero<u16> {
        NonZero::new(1).unwrap_or(NonZero::<u16>::MIN)
    }

    fn sample_rate(&self) -> NonZero<u32> {
        NonZero::new(self.sample_rate).unwrap_or(NonZero::<u32>::MIN)
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }

    fn try_seek(&mut self, _pos: Duration) -> Result<(), SeekError> {
        self.samples = 0;
        Ok(())
    }
}

/* ------------------------------------------------------------------ */
/* Tests                                                                */
/* ------------------------------------------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> SyntheticProvider {
        SyntheticProvider::new(SyntheticKind::Studio, 48_000.0)
    }

    fn make_filter(provider: &SyntheticProvider, direction: [f32; 3]) -> Filter {
        let mut filter = Filter::new(FILTER_LEN);
        provider.fill_filter(direction, &mut filter);
        filter
    }

    fn high_frequency_energy(filter: &Filter) -> f32 {
        let difference = |buffer: &[f32]| {
            buffer
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).powi(2))
                .sum::<f32>()
        };
        difference(&filter.left) + difference(&filter.right)
    }

    fn band_energy(buffer: &[f32], frequency: f32, sample_rate: f32) -> f32 {
        let coefficient = TAU * frequency / sample_rate;
        let mut real = 0.0f32;
        let mut imag = 0.0f32;
        for (index, sample) in buffer.iter().enumerate() {
            real += sample * (coefficient * index as f32).cos();
            imag += sample * (coefficient * index as f32).sin();
        }
        (real * real + imag * imag).sqrt()
    }

    #[test]
    fn kemar_chip_labels_are_distinct() {
        assert_eq!(short_name(Path::new("MIT_KEMAR_normal.sofa")), "KEMAR N");
        assert_eq!(short_name(Path::new("MIT_KEMAR_large.sofa")), "KEMAR L");
        assert_eq!(short_name(Path::new("MIT_KEMAR_normal_pinna.sofa")), "KEMAR N");
    }

    #[test]
    fn right_source_delays_the_left_ear() {
        let provider = provider();
        let right = make_filter(&provider, [1.0, 0.0, 0.0]);
        assert!(
            right.ldelay > right.rdelay,
            "left ear must be farther for a right source"
        );
        assert!((right.rdelay).abs() < 1e-6);

        let left = make_filter(&provider, [-1.0, 0.0, 0.0]);
        assert!(left.rdelay > left.ldelay);
    }

    #[test]
    fn side_source_has_ild() {
        let provider = provider();
        let right = make_filter(&provider, [1.0, 0.0, 0.0]);
        let left_energy: f32 = right.left.iter().map(|sample| sample * sample).sum();
        let right_energy: f32 = right.right.iter().map(|sample| sample * sample).sum();
        assert!(
            right_energy > left_energy * 1.5,
            "near ear should be louder than the shadowed far ear"
        );
    }

    #[test]
    fn partitioned_renderer_produces_binaural_differences() {
        let provider = provider();
        let mut renderer = HrtfRenderer::new(48_000).expect("renderer");
        renderer.ensure_filter(&provider, [1.0, 0.0, 0.0], true);
        assert!(renderer.ready);

        let mut input = vec![0.0; PARTITION_LEN];
        input[0] = 1.0;
        let mut left = vec![0.0; PARTITION_LEN];
        let mut right = vec![0.0; PARTITION_LEN];
        let mut energy_left = 0.0f32;
        let mut energy_right = 0.0f32;
        for _ in 0..2 {
            assert!(renderer.process(&input, &mut left, &mut right));
            energy_left += left.iter().map(|sample| sample * sample).sum::<f32>();
            energy_right += right.iter().map(|sample| sample * sample).sum::<f32>();
            input.fill(0.0);
        }
        assert!(
            energy_right > energy_left * 2.0,
            "the near ear should receive much more energy (L {energy_left}, R {energy_right})"
        );
    }

    #[test]
    fn front_and_back_differ_spectrally() {
        let provider = provider();
        let front = make_filter(&provider, [0.0, 0.0, 1.0]);
        let back = make_filter(&provider, [0.0, 0.0, -1.0]);
        let front_hf = high_frequency_energy(&front);
        let back_hf = high_frequency_energy(&back);
        assert!(
            front_hf > back_hf * 1.15,
            "a rear source should be darker (front {front_hf}, back {back_hf})"
        );

        let front_3k = band_energy(&front.left, 3_200.0, 48_000.0);
        let back_3k = band_energy(&back.left, 3_200.0, 48_000.0);
        assert!(
            (front_3k - back_3k).abs() > 0.05,
            "front/back notch should differ at 3.2 kHz"
        );
    }

    #[test]
    fn above_and_below_differ_spectrally() {
        let provider = provider();
        let above = make_filter(&provider, [0.0, 1.0, 0.15]);
        let below = make_filter(&provider, [0.0, -1.0, 0.15]);
        let above_6k = band_energy(&above.left, 6_000.0, 48_000.0);
        let below_6k = band_energy(&below.left, 6_000.0, 48_000.0);
        assert!(
            (above_6k - below_6k).abs() > 0.02,
            "above and below must not have the same spectrum"
        );
    }

    #[test]
    fn scene_rotation_matches_head_yaw() {
        let front = scene([0.0, 0.0, 2.0], 0.0, 0.0, 0.0);
        assert!(front.azimuth_deg.abs() < 1e-4);
        assert!((front.elevation_deg).abs() < 1e-4);

        let right = scene([2.0, 0.0, 0.0], 0.0, 0.0, 0.0);
        assert!((right.azimuth_deg - 90.0).abs() < 1e-4);

        let turned = scene([0.0, 0.0, 2.0], 90.0, 0.0, 0.0);
        assert!(
            (turned.azimuth_deg + 90.0).abs() < 1e-4,
            "turning the head right should move a front source to the left"
        );

        let above = scene([0.0, 2.0, 0.0], 0.0, 0.0, 0.0);
        assert!((above.elevation_deg - 90.0).abs() < 1e-4);
    }

    #[test]
    fn pad_position_keeps_the_source_out_of_the_head() {
        let position = position_from_pad(0.0, 0.0, 0.0, 8.0);
        assert!((position[2] - MIN_DISTANCE).abs() < 1e-4);
        assert!(position[0].abs() < 1e-6);

        let position = position_from_pad(1.0, 0.0, 0.0, 8.0);
        assert!((position[0] - 4.0).abs() < 1e-4);

        let (x, y) = pad_from_position(position, 8.0);
        assert!((x - 1.0).abs() < 1e-4);
        assert!(y.abs() < 1e-4);

        let position = position_from_pad(0.0, 0.25, 0.0, 8.0);
        assert!((position[2] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn shared_state_round_trips() {
        let shared = SpatialShared::new();
        assert_eq!(shared.position(), [0.0, 0.0, MIN_DISTANCE]);
        shared.set_position([1.0, 2.0, 3.0]);
        assert_eq!(shared.position(), [1.0, 2.0, 3.0]);
        shared.set_room(99.0);
        assert_eq!(shared.room(), 24.0);
        shared.set_reverb(2.0);
        assert_eq!(shared.reverb(), 1.0);
        shared.set_head_yaw(45.0);
        assert_eq!(shared.head_yaw(), 45.0);
        shared.set_hrtf_enabled(false);
        assert!(!shared.hrtf_enabled());
        shared.set_enabled(false);
        assert!(!shared.enabled());
        shared.set_test_mode(TestMode::Above);
        assert_eq!(shared.test_mode(), TestMode::Above);
    }

    #[test]
    fn moving_object_renders_without_large_jumps() {
        use rodio::source::SineWave;

        let shared = Arc::new(SpatialShared::new());
        let provider: Arc<dyn HrtfProvider> =
            Arc::new(SyntheticProvider::new(SyntheticKind::Studio, 48_000.0));
        shared.install_set("test", vec![(48_000, provider)]);
        shared.set_hrtf_enabled(true);
        shared.set_reverb(0.0);
        shared.set_room(8.0);
        shared.set_position([1.5, 0.0, 0.0]);

        let mut source = SpatialSource::new(SineWave::new(220.0), shared.clone());
        let mut previous = 0.0f32;
        let mut max_delta = 0.0f32;
        let mut energy = 0.0f32;
        let blocks = 16;
        for index in 0..(PARTITION_LEN * blocks) {
            if index % (PARTITION_LEN * 2) == 0 {
                let seconds = index as f32 / 48_000.0;
                let angle = seconds * TAU;
                shared.set_position([angle.sin() * 1.5, 0.0, angle.cos() * 1.5]);
                // Alternate A/B every four updates to exercise the mode fade.
                if index % (PARTITION_LEN * 8) == 0 {
                    shared.set_hrtf_enabled(!shared.hrtf_enabled());
                }
            }
            let sample = source.next().expect("infinite source");
            assert!(sample.is_finite(), "output must be finite");
            if index > 0 {
                max_delta = max_delta.max((sample - previous).abs());
            }
            energy += sample * sample;
            previous = sample;
        }
        assert!(energy > 1.0, "renderer must produce sound");
        assert!(
            max_delta < 0.5,
            "moving/HRTF switching should not produce a click (max delta {max_delta})"
        );
    }

    #[test]
    fn disabled_spatial_source_passes_the_stereo_signal_through() {
        use rodio::buffer::SamplesBuffer;

        let shared = Arc::new(SpatialShared::new());
        shared.set_reverb(0.6);
        shared.set_position([1.5, 0.5, 0.0]);
        shared.set_enabled(false);

        let mut interleaved = Vec::new();
        for index in 0..PARTITION_LEN * 2 {
            let phase = index as f32 * 0.05;
            interleaved.push(phase.sin() * 0.4);
            interleaved.push((phase + 1.0).cos() * 0.3);
        }

        let source = SamplesBuffer::new(
            NonZero::new(2).unwrap(),
            NonZero::new(48_000).unwrap(),
            interleaved.clone(),
        );
        let mut spatial = SpatialSource::new(source, shared.clone());

        let mut output = Vec::new();
        while let Some(sample) = spatial.next() {
            output.push(sample);
        }

        // The first block crossfades from the spatial render to the dry signal;
        // afterwards the output has to match the input sample for sample.
        let settle = PARTITION_LEN * 2;
        assert!(output.len() > settle);
        for (index, expected) in interleaved[settle..].iter().enumerate() {
            let got = output[settle + index];
            assert!(
                (got - expected).abs() < 1e-5,
                "sample {index}: {got} != {expected}"
            );
        }
    }

    #[test]
    fn disabling_spatial_does_not_click() {
        use rodio::source::SineWave;

        let shared = Arc::new(SpatialShared::new());
        let provider: Arc<dyn HrtfProvider> =
            Arc::new(SyntheticProvider::new(SyntheticKind::Studio, 48_000.0));
        shared.install_set("test", vec![(48_000, provider)]);
        shared.set_position([1.2, 0.0, 0.8]);

        let mut source = SpatialSource::new(SineWave::new(220.0), shared.clone());
        let mut previous = 0.0f32;
        let mut max_delta = 0.0f32;
        for index in 0..(PARTITION_LEN * 8) {
            if index == PARTITION_LEN * 3 {
                shared.set_enabled(false);
            }
            let sample = source.next().expect("infinite source");
            if index > 0 {
                max_delta = max_delta.max((sample - previous).abs());
            }
            previous = sample;
        }
        assert!(max_delta < 0.5, "bypass should not click (max delta {max_delta})");
    }

    #[test]
    fn test_mode_cycles_through_all_patterns() {
        let mut mode = TestMode::Off;
        let mut seen = 0;
        for _ in 0..9 {
            mode = mode.next();
            if mode.active() {
                seen += 1;
            }
        }
        assert_eq!(mode, TestMode::Off);
        assert_eq!(seen, 8);
    }

    #[test]
    fn provider_installation_bumps_generation() {
        let shared = SpatialShared::new();
        let first = shared.generation();
        let provider: Arc<dyn HrtfProvider> =
            Arc::new(SyntheticProvider::new(SyntheticKind::Studio, 48_000.0));
        shared.install_set("test", vec![(48_000, provider.clone())]);
        assert_eq!(shared.generation(), first.wrapping_add(1));
        assert!(shared.provider_for(48_000).is_some());
        assert!(shared.provider_for(44_100).is_none());
        shared.ensure_provider("test", 44_100, provider);
        assert!(shared.provider_for(44_100).is_some());
    }

    #[test]
    fn test_signal_moves_the_shared_position() {
        let shared = Arc::new(SpatialShared::new());
        shared.set_test_mode(TestMode::OrbitHorizontal);
        let mut signal = TestSignal::new(48_000, shared.clone());
        for _ in 0..(PARTITION_LEN * 2) {
            let _ = signal.next();
        }
        let position = shared.position();
        assert!(
            (position[0].abs() + position[2].abs()) > 0.1,
            "the test orbit must move the object"
        );
    }

    #[test]
    fn missing_sofa_fails_cleanly() {
        let error = SofaProvider::new(Path::new("definitely-missing.sofa"), 48_000.0)
            .err()
            .expect("missing file must fail");
        assert!(error.contains("not found"), "{error}");
    }

    #[test]
    fn sofa_provider_reports_its_name() {
        // Regression guard for the error path used when a user drops a bad
        // file into the HRTF folder.
        let path = hrtf_folder().join("__br1_test_missing.sofa");
        let error = load_provider(&HrtfSetKind::Sofa(path), 48_000)
            .err()
            .expect("missing file must fail");
        assert!(error.contains("HRTF file not found"), "{error}");
    }
}
