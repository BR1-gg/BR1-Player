//! Liquid glass painting for egui.
//!
//! The values and the resolution model come from `liquid-rust`
//! (https://github.com/al1h3n/liquid-rust, MIT OR Apache-2.0): `Material`,
//! `Tone`, `Shadow`, `Appearance`, `smooth_corner` and the transparency
//! slider logic. egui has no backdrop texture to refract, so the refraction
//! and frost stages are painted as bevel bands, a directional rim light and
//! calibrated tone fills. The material and appearance knobs all drive the
//! painter; `merge_spacing` is reserved for grouped glass.

use eframe::egui::{pos2, vec2, Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke};

use crate::config::{GlassConfig, GlassShape, GlassTone};
use crate::theme::{self, Theme};

/// Where a glass surface is being painted. Each slot maps to one target
/// checkbox in Settings.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GlassSlot {
    None,
    TopBar,
    Transport,
    PlayerChip,
    DownloaderAction,
    OptionChip,
    Menu,
    ListRow,
    Panel,
    Slider,
    SettingsControl,
    Setup,
}

pub fn enabled(theme: &Theme, slot: GlassSlot) -> bool {
    let glass = &theme.liquid;
    if !glass.enabled {
        return false;
    }
    match slot {
        GlassSlot::None => false,
        GlassSlot::TopBar => glass.targets.top_bar,
        GlassSlot::Transport => glass.targets.transport,
        GlassSlot::PlayerChip => glass.targets.player_chips,
        GlassSlot::DownloaderAction => glass.targets.downloader_actions,
        GlassSlot::OptionChip => glass.targets.option_chips,
        GlassSlot::Menu => glass.targets.menus,
        GlassSlot::ListRow => glass.targets.lists,
        GlassSlot::Panel => glass.targets.panels,
        GlassSlot::Slider => glass.targets.sliders,
        GlassSlot::SettingsControl => glass.targets.settings_controls,
        GlassSlot::Setup => glass.targets.setup,
    }
}

/* ---------- liquid-rust material resolution ---------- */

const IOS_SMOOTHING: f32 = 0.6;
const NEAR_BLACK: f32 = 0x1C as f32 / 255.0;

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn more_opaque(amount: f32, k: f32) -> f32 {
    if k >= 0.0 {
        amount + (1.0 - amount) * k
    } else {
        amount * (1.0 + k)
    }
}

fn thickness(min_side: f32) -> f32 {
    smoothstep(44.0, 320.0, min_side)
}

fn tint_faces(material: &mut GlassConfig, k: f32) {
    let dark = &mut material.dark_tone;
    dark.darken_to += (NEAR_BLACK - dark.darken_to) * k;
    dark.darken = more_opaque(dark.darken, 0.6 * k).min(0.88f32.max(dark.darken));
    let light = &mut material.light_tone;
    light.lighten = more_opaque(light.lighten, 0.6 * k).min(0.88f32.max(light.lighten));
}

/// Mirrors `Material::resolve` in liquid-rust: size adaptation, the iOS 27
/// transparency slider, Reduce Transparency and Increase Contrast.
fn resolve(style: &GlassConfig, min_side: f32) -> (GlassConfig, f32) {
    let mut material = *style;
    let mut adapt = 0.0;

    if material.adaptive {
        let tau = thickness(min_side);
        material.frost *= 1.0 + 0.6 * tau;
        material.depth *= 1.0 + 0.25 * tau;
        material.specular -= 0.05 * tau;
        material.shadow.blur += 33.0 * tau;
        material.shadow.opacity += 0.19 * tau;
        material.light_tone.lighten += 0.55 * tau;
        material.light_tone.luminance_mix += 0.15 * tau;
        material.dark_tone.darken += 0.27 * tau;
        if material.adapt_to_content {
            adapt = 1.0 - smoothstep(0.0, 0.5, tau);
        }
    }

    let k = (material.transparency.clamp(0.0, 1.0) - 0.5) * 2.0;
    if material.reduce_transparency {
        material.frost = (material.frost * 3.0).max(20.0) * 1.1f32.exp2();
        tint_faces(&mut material, 1.0);
    } else if k > 0.0 {
        material.frost *= (0.5 * k).exp2();
        tint_faces(&mut material, k);
        material.edge = (material.edge + 0.25 * k).min(1.0);
        material.rim += 0.08 * k;
        material.specular += 0.08 * k;
    } else {
        material.frost *= (1.1 * k).exp2();
        for tone in [&mut material.light_tone, &mut material.dark_tone] {
            tone.luminance_mix = more_opaque(tone.luminance_mix, 0.45 * k);
            tone.lighten = more_opaque(tone.lighten, 0.35 * k);
            tone.darken = more_opaque(tone.darken, 0.45 * k);
        }
    }

    if material.increase_contrast {
        material.edge = (material.edge * 2.5).min(1.0);
    }
    (material, adapt)
}

/// Figma tone fill, ported from liquid-rust's `tone()` (sRGB space).
fn tone_color(color: Color32, tone: &GlassTone) -> Color32 {
    let input = [
        color.r() as f32 / 255.0,
        color.g() as f32 / 255.0,
        color.b() as f32 / 255.0,
    ];
    let blend_lum = |c: [f32; 3]| 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
    let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x = input.map(|value| (value + tone.lift).min(1.0));
    let delta = tone.luminance - blend_lum(x);
    let mut shifted = x.map(|value| value + delta);
    let (luma, low, high) = (
        blend_lum(shifted),
        shifted[0].min(shifted[1]).min(shifted[2]),
        shifted[0].max(shifted[1]).max(shifted[2]),
    );
    if low < 0.0 {
        shifted = shifted.map(|value| luma + (value - luma) * luma / (luma - low).max(1e-5));
    }
    if high > 1.0 {
        shifted =
            shifted.map(|value| luma + (value - luma) * (1.0 - luma) / (high - luma).max(1e-5));
    }
    let out = [0, 1, 2].map(|index| {
        let value = mix(x[index], shifted[index], tone.luminance_mix);
        let value = mix(value, 1.0, tone.lighten);
        mix(value, value.min(tone.darken_to), tone.darken)
    });
    Color32::from_rgb(
        (out[0] * 255.0).round().clamp(0.0, 255.0) as u8,
        (out[1] * 255.0).round().clamp(0.0, 255.0) as u8,
        (out[2] * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

/* ---------- geometry ---------- */

/// Figma smoothed corner: returns the corner extent along each edge and the
/// superellipse exponent (2 = circular).
fn smooth_corner(radius: f32, smoothing: f32, budget: f32) -> (f32, f32) {
    let radius = radius.min(budget).max(0.0);
    if radius <= 0.0 {
        return (0.0, 2.0);
    }
    let smoothing = smoothing
        .clamp(0.0, 1.0)
        .min((budget / radius - 1.0).max(0.0));
    (
        radius * (1.0 + 0.42 * smoothing + 0.18 * smoothing * smoothing),
        2.0 + 0.95 * smoothing + 0.6 * smoothing * smoothing,
    )
}

fn superellipse_points(
    rect: Rect,
    radius: f32,
    exponent: f32,
    segments: usize,
) -> Vec<Pos2> {
    let budget = rect.width().min(rect.height()) * 0.5;
    let radius = radius.clamp(0.0, budget);
    let exponent = exponent.max(2.0);
    let power = 2.0 / exponent;
    let mut points = Vec::with_capacity(segments * 4 + 4);
    let corners = [
        (
            pos2(rect.left() + radius, rect.top() + radius),
            std::f32::consts::PI,
            std::f32::consts::PI * 1.5,
        ),
        (
            pos2(rect.right() - radius, rect.top() + radius),
            std::f32::consts::PI * 1.5,
            std::f32::consts::TAU,
        ),
        (
            pos2(rect.right() - radius, rect.bottom() - radius),
            0.0,
            std::f32::consts::FRAC_PI_2,
        ),
        (
            pos2(rect.left() + radius, rect.bottom() - radius),
            std::f32::consts::FRAC_PI_2,
            std::f32::consts::PI,
        ),
    ];
    for (corner, from, to) in corners {
        for step in 0..=segments {
            let angle = from + (to - from) * step as f32 / segments as f32;
            let (sin, cos) = angle.sin_cos();
            points.push(pos2(
                corner.x + cos.signum() * radius * cos.abs().powf(power),
                corner.y + sin.signum() * radius * sin.abs().powf(power),
            ));
        }
    }
    points
}

fn button_points(
    rect: Rect,
    material: &GlassConfig,
    segments: usize,
) -> (Vec<Pos2>, f32, f32) {
    let budget = rect.width().min(rect.height()) * 0.5;
    let (radius, exponent) = match material.shape {
        GlassShape::Capsule => (budget, 2.0),
        GlassShape::Circular => (material.shape_radius.clamp(0.0, budget), 2.0),
        GlassShape::Rounded => smooth_corner(material.shape_radius, IOS_SMOOTHING, budget),
        GlassShape::Smooth => smooth_corner(material.shape_radius, material.smoothing, budget),
    };
    (
        superellipse_points(rect, radius.max(0.01), exponent, segments),
        radius,
        exponent,
    )
}

fn panel_points(rect: Rect, radius: f32, segments: usize) -> (Vec<Pos2>, f32, f32) {
    let budget = rect.width().min(rect.height()) * 0.5;
    let (extent, exponent) = smooth_corner(radius, IOS_SMOOTHING, budget);
    (
        superellipse_points(rect, extent.max(0.01), exponent, segments),
        extent,
        exponent,
    )
}

/* ---------- painting helpers ---------- */

fn fill_gradient(
    painter: &Painter,
    points: &[Pos2],
    center: Pos2,
    top: Color32,
    bottom: Color32,
    angle: f32,
) {
    if points.len() < 3 {
        return;
    }
    let direction = vec2(angle.to_radians().cos(), angle.to_radians().sin());
    let extent = points
        .iter()
        .map(|point| (*point - center).dot(direction).abs())
        .fold(0.0f32, f32::max)
        * 2.0;
    if extent <= 0.5 {
        return;
    }
    let color_at = |point: Pos2| {
        let t = ((point - center).dot(direction) / extent + 0.5).clamp(0.0, 1.0);
        theme::lerp_color(top, bottom, t)
    };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, color_at(center));
    for point in points {
        mesh.colored_vertex(*point, color_at(*point));
    }
    let count = points.len() as u32;
    for index in 0..count {
        mesh.add_triangle(0, 1 + index, 1 + (index + 1) % count);
    }
    painter.add(Shape::mesh(mesh));
}

fn paint_shadow(painter: &Painter, rect: Rect, radius: f32, material: &GlassConfig) {
    let opacity = (material.shadow.opacity * 255.0).clamp(0.0, 255.0);
    if opacity <= 0.5 {
        return;
    }
    let blur = material.shadow.blur.max(0.0);
    let steps = if blur > 20.0 { 5 } else { 3 };
    for index in 0..steps {
        let t = (index + 1) as f32 / steps as f32;
        let grow = blur * t;
        let alpha = (opacity * (0.26 - 0.05 * t)).clamp(0.0, 255.0) as u8;
        if alpha == 0 {
            continue;
        }
        painter.rect_filled(
            rect.expand(grow)
                .translate(vec2(0.0, material.shadow.offset_y)),
            radius + grow,
            Color32::from_black_alpha(alpha),
        );
    }
}

fn light_direction(angle: f32) -> egui::Vec2 {
    let radians = angle.to_radians();
    vec2(radians.sin(), -radians.cos())
}

fn painted_points<I>(points: I) -> Vec<Pos2>
where
    I: IntoIterator<Item = Pos2>,
{
    points.into_iter().collect()
}

fn paint_rim(
    painter: &Painter,
    points: &[Pos2],
    center: Pos2,
    rect: Rect,
    theme: &Theme,
    material: &GlassConfig,
    intensity: f32,
) {
    if points.len() < 3 {
        return;
    }
    let light = light_direction(material.light_angle);
    let tint = material
        .tint_enabled
        .then(|| Color32::from_rgb(material.tint_color[0], material.tint_color[1], material.tint_color[2]));
    let rim_color = theme::lerp_color(
        Color32::WHITE,
        tint.unwrap_or(theme.accent_a),
        if tint.is_some() { 0.5 } else { 0.24 },
    );
    let width = (1.0 + material.specular * 1.2).max(0.8);

    // A faint full outline keeps the silhouette readable on dark panels where
    // the directional rim only lights one side.
    painter.add(Shape::closed_line(
        points.to_vec(),
        Stroke::new(1.0_f32, theme::with_alpha(rim_color, (16.0 * intensity) as u8)),
    ));

    for index in 0..points.len() {
        let from = points[index];
        let to = points[(index + 1) % points.len()];
        let midpoint = (from.to_vec2() + to.to_vec2()) * 0.5;
        let normal = (midpoint - center.to_vec2()).normalized();
        if !normal.is_finite() || normal.length_sq() <= 0.0 {
            continue;
        }
        let facing = normal.dot(light).clamp(-1.0, 1.0);
        let lit = facing.max(0.0).powf(1.0 + material.splay * 2.0);
        let shade = (-facing).max(0.0);

        let dark_alpha =
            (material.rim_shade * 1.8 * 255.0 * shade * intensity).clamp(0.0, 255.0);
        if dark_alpha > 1.5 {
            painter.line_segment(
                [from, to],
                Stroke::new(
                    width * 0.9,
                    Color32::from_black_alpha(dark_alpha as u8),
                ),
            );
        }

        let light_alpha = ((material.rim * 0.8 + material.specular * 0.95)
            * 255.0
            * lit
            * intensity)
            .clamp(0.0, 235.0);
        if light_alpha > 1.5 {
            painter.line_segment(
                [from, to],
                Stroke::new(width, theme::with_alpha(rim_color, light_alpha as u8)),
            );
            let core_alpha = (material.specular * 0.5 * 255.0 * lit * intensity)
                .clamp(0.0, 200.0);
            if core_alpha > 1.5 {
                painter.line_segment(
                    [from, to],
                    Stroke::new(width * 0.55, Color32::from_white_alpha(core_alpha as u8)),
                );
            }
        }
    }

    // Chrome hairlines (dock style).
    if material.hairline > 0.01 {
        let alpha = (material.hairline * 105.0 * intensity).clamp(0.0, 160.0) as u8;
        painter.add(Shape::closed_line(
            points.to_vec(),
            Stroke::new(1.1_f32, Color32::from_white_alpha(alpha)),
        ));
    }

    // iOS 27 darkened outer edge.
    if material.edge > 0.01 {
        let alpha = (material.edge * 150.0 * intensity).clamp(0.0, 170.0) as u8;
        painter.add(Shape::closed_line(
            points.to_vec(),
            Stroke::new(1.0_f32, Color32::from_black_alpha(alpha)),
        ));
    }

    if material.increase_contrast {
        painter.add(Shape::closed_line(
            points.to_vec(),
            Stroke::new(1.0_f32, Color32::from_white_alpha(55)),
        ));
    }

    // Directional specular pool just inside the lit edge.
    let half = rect.width().min(rect.height()) * 0.5;
    let reach = (half - material.depth.min(half * 0.55) - 1.0).max(1.0);
    let glow_center = center + light * reach * 0.66;
    let glow_radius = (half * 0.62).clamp(4.0, 120.0);
    let size_scale = if rect.width().min(rect.height()) > 120.0 {
        0.0
    } else {
        (40.0 / rect.width().min(rect.height()).max(1.0)).min(1.0)
    };
    let glow_alpha =
        (material.specular * 52.0 * intensity * size_scale).clamp(0.0, 60.0) as u8;
    if glow_alpha > 1 {
        theme::radial_glow(
            painter,
            glow_center,
            glow_radius,
            Color32::WHITE,
            glow_alpha,
        );
    }
}

fn paint_bevel(
    painter: &Painter,
    rect: Rect,
    center: Pos2,
    _theme: &Theme,
    material: &GlassConfig,
    glass_shape: Option<(f32, f32)>,
    intensity: f32,
) {
    if material.refraction <= 0.01 && material.dispersion <= 0.01 {
        return;
    }
    let light = light_direction(material.light_angle);
    let half = rect.width().min(rect.height()) * 0.5;
    let depth = material.depth.clamp(0.0, half * 0.9);
    if depth <= 0.5 {
        return;
    }
    let (base_radius, exponent) = glass_shape.unwrap_or((half, 2.0));
    let large = half > 90.0;
    let fractions: &[f32] = if large {
        &[0.55]
    } else {
        &[0.30, 0.65]
    };
    for &fraction in fractions {
        let inset = depth * fraction;
        let inner = rect.shrink(inset);
        if inner.width() <= 2.0 || inner.height() <= 2.0 {
            continue;
        }
        let points = superellipse_points(
            inner,
            (base_radius - inset).max(0.0),
            exponent,
            3,
        );
        for index in 0..points.len() {
            let from = points[index];
            let to = points[(index + 1) % points.len()];
            let midpoint = (from.to_vec2() + to.to_vec2()) * 0.5;
            let normal = (midpoint - center.to_vec2()).normalized();
            if !normal.is_finite() || normal.length_sq() <= 0.0 {
                continue;
            }
            let facing = normal.dot(light).clamp(-1.0, 1.0);
            let lit = facing.max(0.0);
            let alpha = (material.refraction
                * if large { 18.0 } else { 38.0 }
                * (1.0 - fraction)
                * lit
                * intensity)
                .clamp(0.0, 55.0);
            if alpha > 1.5 {
                painter.line_segment(
                    [from, to],
                    Stroke::new(1.0_f32, Color32::from_white_alpha(alpha as u8)),
                );
            }
            let shade_alpha = (material.refraction
                * 18.0
                * (1.0 - fraction)
                * (-facing).max(0.0)
                * intensity)
                .clamp(0.0, 40.0);
            if shade_alpha > 1.5 {
                painter.line_segment(
                    [from, to],
                    Stroke::new(1.0_f32, Color32::from_black_alpha(shade_alpha as u8)),
                );
            }
        }
    }

    if material.dispersion > 0.01 {
        let inset = depth * 0.55;
        let inner = rect.shrink(inset);
        if inner.width() > 2.0 && inner.height() > 2.0 {
            let points = superellipse_points(
                inner,
                (base_radius - inset).max(0.0),
                exponent,
                3,
            );
            for index in 0..points.len() {
                let from = points[index];
                let to = points[(index + 1) % points.len()];
                let midpoint = (from.to_vec2() + to.to_vec2()) * 0.5;
                let normal = (midpoint - center.to_vec2()).normalized();
                if !normal.is_finite() || normal.length_sq() <= 0.0 {
                    continue;
                }
                let lit = normal.dot(light).max(0.0);
                let alpha = (material.dispersion * 16.0 * lit * intensity).clamp(0.0, 26.0) as u8;
                if alpha == 0 {
                    continue;
                }
                let fringe = if normal.x < 0.0 {
                    Color32::from_rgb(255, 96, 120)
                } else {
                    Color32::from_rgb(80, 205, 255)
                };
                let fringe = theme::lerp_color(fringe, Color32::WHITE, 0.35);
                painter.line_segment(
                    [from, to],
                    Stroke::new(1.0_f32, theme::with_alpha(fringe, alpha)),
                );
            }
        }
    }
}

fn face_colors(
    theme: &Theme,
    material: &GlassConfig,
    adapt: f32,
    primary: bool,
) -> (Color32, Color32, f32, bool) {
    let backdrop_top = theme.panel_top();
    let backdrop_bottom = theme.panel_bottom();
    let luma = 0.2126 * backdrop_top.r() as f32
        + 0.7152 * backdrop_top.g() as f32
        + 0.0722 * backdrop_top.b() as f32;
    let light = if material.adapt_to_content && material.adaptive && adapt > 0.5 {
        luma / 255.0 > 0.55
    } else {
        !material.dark
    };
    let tone = if light {
        material.light_tone
    } else {
        material.dark_tone
    };

    let mut top = tone_color(backdrop_top, &tone);
    let mut bottom = tone_color(backdrop_bottom, &tone);

    if primary {
        top = theme::lerp_color(top, theme.accent_a, 0.72);
        bottom = theme::lerp_color(bottom, theme.accent_b, 0.72);
    } else if material.tint_enabled {
        let tint = Color32::from_rgb(
            material.tint_color[0],
            material.tint_color[1],
            material.tint_color[2],
        );
        top = theme::lerp_color(top, tint, material.tint_strength * 0.68);
        bottom = theme::lerp_color(bottom, tint, material.tint_strength * 0.68);
    }

    top = theme::lerp_color(top, theme.accent_a, 0.035);
    bottom = theme::lerp_color(bottom, theme.accent_b, 0.025);

    let mut alpha = if light {
        0.30 + 0.55 * tone.lighten + 0.20 * tone.luminance_mix
    } else {
        0.20 + 0.45 * tone.darken
    };
    if material.reduce_transparency {
        alpha = (alpha + 0.22).min(0.95);
    }
    if primary {
        alpha = alpha.max(0.62);
    }
    (top, bottom, alpha.clamp(0.16, 0.94), light)
}

/* ---------- public surfaces ---------- */

#[allow(clippy::too_many_arguments)]
fn paint(
    painter: &Painter,
    rect: Rect,
    theme: &Theme,
    points: &[Pos2],
    radius: f32,
    corner: (f32, f32),
    material: &GlassConfig,
    adapt: f32,
    hovered: bool,
    pressed: bool,
    active: bool,
    primary: bool,
    intensity: f32,
    draw_face: bool,
    draw_glow: bool,
) {
    let interactive = material.physics && !material.reduce_motion;
    let hovered = hovered && interactive;
    let pressed = pressed && interactive;
    let active = active && interactive;
    let intensity = intensity.clamp(0.0, 1.5);
    let center = rect.center();

    let mut shadow_material = *material;
    shadow_material.shadow.opacity *= intensity;
    paint_shadow(painter, rect, radius, &shadow_material);

    if draw_face {
        let (mut top, mut bottom, mut alpha, light) =
            face_colors(theme, material, adapt, primary);
        if hovered {
            top = theme::lerp_color(top, Color32::WHITE, 0.06);
            bottom = theme::lerp_color(bottom, Color32::WHITE, 0.03);
            alpha = (alpha + 0.035).min(0.96);
        }
        if pressed {
            alpha = (alpha + 0.05).min(0.97);
        }
        if active {
            top = theme::lerp_color(top, theme.accent_a, 0.30);
            bottom = theme::lerp_color(bottom, theme.accent_b, 0.20);
        }
        alpha *= intensity;
        fill_gradient(
            painter,
            points,
            center,
            theme::with_alpha(top, (alpha * 255.0) as u8),
            theme::with_alpha(bottom, (alpha * 255.0) as u8),
            90.0,
        );

        // Frost: a soft white veil from the lit top edge.
        let frost_alpha = (material.frost * 0.42 * intensity).clamp(0.0, 30.0) as u8;
        if frost_alpha > 1 {
            fill_gradient(
                painter,
                points,
                center,
                Color32::from_white_alpha(frost_alpha),
                Color32::TRANSPARENT,
                90.0,
            );
        }

        if material.dim > 0.001 {
            let alpha = (material.dim * 190.0 * intensity).clamp(0.0, 200.0) as u8;
            painter.add(Shape::convex_polygon(
                painted_points(points.iter().copied()),
                Color32::from_black_alpha(alpha),
                Stroke::NONE,
            ));
        }

        // Light-glass face gets a little extra lift so dark text stays legible.
        if light {
            let lift = (0.10 * 255.0 * intensity) as u8;
            fill_gradient(
                painter,
                points,
                center,
                Color32::from_white_alpha(lift),
                Color32::from_white_alpha((lift / 3).max(1)),
                90.0,
            );
        }
    }

    let shape_info = Some(corner);
    paint_bevel(painter, rect, center, theme, material, shape_info, intensity);
    if draw_glow {
        paint_rim(painter, points, center, rect, theme, material, intensity);
    }
}

/// A button, chip, pill or icon. Returns `true` when glass was painted.
#[allow(clippy::too_many_arguments)]
pub fn paint_button(
    painter: &Painter,
    rect: Rect,
    theme: &Theme,
    hovered: bool,
    pressed: bool,
    active: bool,
    primary: bool,
    slot: GlassSlot,
) -> bool {
    if !enabled(theme, slot) || rect.width() < 2.0 || rect.height() < 2.0 {
        return false;
    }
    let shrink = if pressed && theme.liquid.physics {
        0.8
    } else {
        0.0
    };
    let rect = rect.shrink(shrink);
    let min_side = rect.width().min(rect.height());
    let (material, adapt) = resolve(&theme.liquid, min_side);
    let (points, radius, exponent) = button_points(rect, &material, 5);
    paint(
        painter,
        rect,
        theme,
        &points,
        radius,
        (radius, exponent),
        &material,
        adapt,
        hovered,
        pressed,
        active,
        primary,
        1.0,
        true,
        true,
    );
    true
}

/// A panel or modal card. The radius comes from the app theme, not from the
/// button shape setting.
pub fn paint_panel(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    theme: &Theme,
    hovered: bool,
) -> bool {
    if !enabled(theme, GlassSlot::Panel) || rect.width() < 8.0 || rect.height() < 8.0 {
        return false;
    }
    let min_side = rect.width().min(rect.height());
    let (material, adapt) = resolve(&theme.liquid, min_side);
    let (points, radius, exponent) = panel_points(rect, radius, 8);
    paint(
        painter,
        rect,
        theme,
        &points,
        radius,
        (radius, exponent),
        &material,
        adapt,
        hovered,
        false,
        false,
        false,
        0.85,
        true,
        theme.liquid.glass_on_glass,
    );
    true
}

/// A subtle glass highlight for a list row or card. `active` adds an accent
/// stain without hiding the row's own content.
pub fn paint_row(
    painter: &Painter,
    rect: Rect,
    theme: &Theme,
    hovered: bool,
    active: bool,
) -> bool {
    if !enabled(theme, GlassSlot::ListRow) || rect.width() < 4.0 || rect.height() < 4.0 {
        return false;
    }
    let min_side = rect.width().min(rect.height());
    let (material, adapt) = resolve(&theme.liquid, min_side);
    let (points, radius, exponent) = button_points(rect.shrink(0.5), &material, 4);
    paint(
        painter,
        rect,
        theme,
        &points,
        radius,
        (radius, exponent),
        &material,
        adapt,
        hovered,
        false,
        active,
        false,
        0.72,
        true,
        true,
    );
    true
}

/// A glass ring for a cover card whose artwork must stay visible.
pub fn paint_card_ring(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    theme: &Theme,
    hovered: bool,
    active: bool,
) -> bool {
    if !enabled(theme, GlassSlot::ListRow) || rect.width() < 4.0 {
        return false;
    }
    let (material, _) = resolve(&theme.liquid, rect.width().min(rect.height()));
    let (points, _radius, _exponent) = panel_points(rect.shrink(0.75), radius, 5);
    let intensity = if active {
        1.35
    } else if hovered {
        1.0
    } else {
        0.72
    };
    paint_rim(
        painter,
        &points,
        rect.center(),
        rect,
        theme,
        &material,
        intensity,
    );
    true
}

/// The slider ball, painted as a small glass bead.
pub fn paint_orb(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    theme: &Theme,
    hovered: bool,
    active: bool,
) -> bool {
    if !enabled(theme, GlassSlot::Slider) {
        return false;
    }
    let style = theme.liquid;
    let radius = radius + if hovered || active { 0.7 } else { 0.0 };
    let rect = Rect::from_center_size(center, vec2(radius * 2.0, radius * 2.0));
    let (material, _) = resolve(&style, radius * 2.0);

    painter.circle_filled(
        center + vec2(0.0, 1.2),
        radius + 0.4,
        Color32::from_black_alpha(70),
    );

    let tint = if style.tint_enabled {
        Color32::from_rgb(
            style.tint_color[0],
            style.tint_color[1],
            style.tint_color[2],
        )
    } else {
        theme.accent_b
    };
    let top = theme::lerp_color(Color32::WHITE, tint, 0.12);
    let bottom = theme::lerp_color(theme::with_alpha(tint, 235), theme.accent_a, 0.35);
    fill_radial_orb(painter, rect, center, top, bottom);

    let light = light_direction(material.light_angle);
    let spec = if hovered || active { 0.85 } else { 0.62 };
    painter.circle_filled(
        center + light * radius * 0.34,
        radius * 0.30,
        Color32::from_white_alpha((spec * 235.0) as u8),
    );
    painter.circle_stroke(
        center,
        radius,
        Stroke::new(
            1.0_f32,
            Color32::from_white_alpha((material.specular * 150.0).clamp(70.0, 190.0) as u8),
        ),
    );
    painter.circle_stroke(
        center + vec2(0.4, 0.7),
        radius,
        Stroke::new(1.0_f32, Color32::from_black_alpha(38)),
    );

    // A one-pixel dark outer edge reads as thickness on light backgrounds.
    painter.circle_stroke(
        center,
        radius + 0.5,
        Stroke::new(0.8_f32, Color32::from_black_alpha((material.edge * 80.0) as u8)),
    );
    true
}

fn fill_radial_orb(
    painter: &Painter,
    rect: Rect,
    center: Pos2,
    inner: Color32,
    outer: Color32,
) {
    let segments = 28usize;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, inner);
    let radius = rect.width().min(rect.height()) * 0.5;
    for index in 0..segments {
        let angle = std::f32::consts::TAU * index as f32 / segments as f32;
        mesh.colored_vertex(
            center + vec2(angle.cos(), angle.sin()) * radius,
            theme::with_alpha(outer, 210),
        );
    }
    for index in 0..segments as u32 {
        mesh.add_triangle(0, 1 + index, 1 + (index + 1) % segments as u32);
    }
    painter.add(Shape::mesh(mesh));
}

/// Opaque-enough fill for an `egui::Frame` menu card. `None` means the menu
/// target is off and the normal panel colour should be used.
pub fn menu_fill(theme: &Theme) -> Option<Color32> {
    if !enabled(theme, GlassSlot::Menu) {
        return None;
    }
    let style = theme.liquid;
    let (material, _) = resolve(&style, 220.0);
    let tone = if material.dark {
        material.dark_tone
    } else {
        material.light_tone
    };
    let base = tone_color(theme.panel_top(), &tone);
    let base = if material.tint_enabled {
        theme::lerp_color(
            base,
            Color32::from_rgb(
                material.tint_color[0],
                material.tint_color[1],
                material.tint_color[2],
            ),
            material.tint_strength * 0.35,
        )
    } else {
        base
    };
    Some(Color32::from_rgba_unmultiplied(
        base.r(),
        base.g(),
        base.b(),
        242,
    ))
}

/// Rim and edge only, for menus whose face is already filled by a Frame.
pub fn paint_menu_rim(painter: &Painter, rect: Rect, radius: f32, theme: &Theme) {
    if !enabled(theme, GlassSlot::Menu) {
        return;
    }
    let (material, _) = resolve(&theme.liquid, rect.width().min(rect.height()));
    let (points, _radius, _exponent) = panel_points(rect, radius, 6);
    paint_rim(
        painter,
        &points,
        rect.center(),
        rect,
        theme,
        &material,
        1.0,
    );
}

