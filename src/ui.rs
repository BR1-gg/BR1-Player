use egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Rect, Response,
    Sense, Shape, Stroke, TextureHandle, Ui,
};

use crate::glass::GlassSlot;
use crate::theme::{self, with_alpha, Theme};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Prev,
    Play,
    Pause,
    Next,
    Search,
    Plus,
    Close,
    Sliders,
    Volume,
    Mute,
    Note,
}

/* ---------- covers ---------- */

pub fn paint_cover(
    ui: &Ui,
    rect: Rect,
    texture: Option<&TextureHandle>,
    radius: f32,
    theme: &Theme,
) {
    match texture {
        Some(texture) => {
            egui::Image::new(egui::load::SizedTexture::from_handle(texture))
                .corner_radius(radius)
                .uv(theme::texture_uv(texture.size_vec2(), rect.size()))
                .paint_at(ui, rect);
        }
        None => placeholder_cover(ui.painter(), rect, radius, theme),
    }
}

pub fn placeholder_cover(painter: &Painter, rect: Rect, radius: f32, theme: &Theme) {
    theme::gradient_rect(
        painter,
        rect,
        radius,
        theme::lerp_color(Color32::from_rgb(26, 24, 38), theme.accent_a, 0.22),
        theme::lerp_color(Color32::from_rgb(13, 13, 19), theme.accent_b, 0.12),
        theme.angle,
    );
    let size = rect.width().min(rect.height());
    painter.circle_filled(
        rect.center() - vec2(0.0, size * 0.05),
        size * 0.28,
        with_alpha(theme.accent_a, 34),
    );
    paint_glyph(
        painter,
        rect.center(),
        size * 0.46,
        Glyph::Note,
        with_alpha(theme.accent_b, 210),
    );
}

/* ---------- glyphs ---------- */

pub fn paint_glyph(
    painter: &Painter,
    center: egui::Pos2,
    scale: f32,
    glyph: Glyph,
    color: Color32,
) {
    let s = scale * 0.33;
    let fill = Stroke::NONE;

    match glyph {
        Glyph::Play => {
            painter.add(Shape::convex_polygon(
                vec![
                    center + vec2(-s * 0.5, -s * 0.95),
                    center + vec2(-s * 0.5, s * 0.95),
                    center + vec2(s * 1.0, 0.0),
                ],
                color,
                fill,
            ));
        }
        Glyph::Pause => {
            let bar = vec2(s * 0.42, s * 1.85);
            painter.rect_filled(
                Rect::from_center_size(center + vec2(-s * 0.42, 0.0), bar),
                s * 0.16,
                color,
            );
            painter.rect_filled(
                Rect::from_center_size(center + vec2(s * 0.42, 0.0), bar),
                s * 0.16,
                color,
            );
        }
        Glyph::Prev | Glyph::Next => {
            let dir = if glyph == Glyph::Prev { 1.0 } else { -1.0 };
            let bar_center = center - vec2(s * 0.78 * dir, 0.0);
            painter.rect_filled(
                Rect::from_center_size(bar_center, vec2(s * 0.28, s * 1.8)),
                s * 0.14,
                color,
            );
            painter.add(Shape::convex_polygon(
                vec![
                    center + vec2(s * 0.62 * dir, -s * 0.9),
                    center + vec2(s * 0.62 * dir, s * 0.9),
                    center + vec2(-s * 0.5 * dir, 0.0),
                ],
                color,
                fill,
            ));
        }
        Glyph::Search => {
            painter.circle_stroke(
                center - vec2(s * 0.16, s * 0.16),
                s * 0.72,
                Stroke::new(s * 0.22, color),
            );
            painter.line_segment(
                [
                    center + vec2(s * 0.36, s * 0.36),
                    center + vec2(s * 0.98, s * 0.98),
                ],
                Stroke::new(s * 0.24, color),
            );
        }
        Glyph::Plus => {
            painter.line_segment(
                [center - vec2(s * 0.75, 0.0), center + vec2(s * 0.75, 0.0)],
                Stroke::new(s * 0.24, color),
            );
            painter.line_segment(
                [center - vec2(0.0, s * 0.75), center + vec2(0.0, s * 0.75)],
                Stroke::new(s * 0.24, color),
            );
        }
        Glyph::Close => {
            painter.line_segment(
                [
                    center - vec2(s * 0.62, s * 0.62),
                    center + vec2(s * 0.62, s * 0.62),
                ],
                Stroke::new(s * 0.22, color),
            );
            painter.line_segment(
                [
                    center + vec2(s * 0.62, -s * 0.62),
                    center + vec2(-s * 0.62, s * 0.62),
                ],
                Stroke::new(s * 0.22, color),
            );
        }
        Glyph::Sliders => {
            for (index, offset) in [(-0.62f32, -0.28f32), (0.0, 0.30), (0.62, -0.05)] {
                let y = center.y + s * offset;
                painter.line_segment(
                    [pos2(center.x - s * 0.92, y), pos2(center.x + s * 0.92, y)],
                    Stroke::new(s * 0.16, color),
                );
                painter.circle_filled(pos2(center.x + s * index, y), s * 0.22, color);
            }
        }
        Glyph::Volume | Glyph::Mute => {
            painter.add(Shape::convex_polygon(
                vec![
                    center + vec2(-s * 0.9, -s * 0.3),
                    center + vec2(-s * 0.4, -s * 0.3),
                    center + vec2(s * 0.15, -s * 0.8),
                    center + vec2(s * 0.15, s * 0.8),
                    center + vec2(-s * 0.4, s * 0.3),
                    center + vec2(-s * 0.9, s * 0.3),
                ],
                color,
                fill,
            ));
            if glyph == Glyph::Volume {
                painter.circle_stroke(
                    center + vec2(s * 0.25, 0.0),
                    s * 0.55,
                    Stroke::new(s * 0.14, color),
                );
                painter.circle_stroke(
                    center + vec2(s * 0.25, 0.0),
                    s * 0.95,
                    Stroke::new(s * 0.14, color),
                );
            } else {
                painter.line_segment(
                    [
                        center + vec2(s * 0.35, -s * 0.45),
                        center + vec2(s * 0.95, s * 0.45),
                    ],
                    Stroke::new(s * 0.16, color),
                );
                painter.line_segment(
                    [
                        center + vec2(s * 0.95, -s * 0.45),
                        center + vec2(s * 0.35, s * 0.45),
                    ],
                    Stroke::new(s * 0.16, color),
                );
            }
        }
        Glyph::Note => {
            let head = s * 0.30;
            let left = center + vec2(-s * 0.5, s * 0.55);
            let right = center + vec2(s * 0.55, s * 0.20);
            painter.circle_filled(left, head, color);
            painter.circle_filled(right, head, color);
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(left.x + head * 0.7, center.y - s * 1.25),
                    vec2(s * 0.16, left.y - center.y + s * 1.25 - head * 0.4),
                ),
                0.0,
                color,
            );
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(right.x + head * 0.7, center.y - s * 0.85),
                    vec2(s * 0.16, right.y - center.y + s * 0.85 - head * 0.4),
                ),
                0.0,
                color,
            );
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(left.x + head * 0.7, center.y - s * 1.25),
                    vec2(right.x - left.x, s * 0.18),
                ),
                0.0,
                color,
            );
        }
    }
}

pub fn glyph_button(
    ui: &Ui,
    rect: Rect,
    id: &str,
    glyph: Glyph,
    filled: bool,
    theme: &Theme,
    slot: GlassSlot,
) -> Response {
    let response = ui
        .interact(rect, ui.scope_id().with(id), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let painter = ui.painter();

    let glass = crate::glass::paint_button(
        painter,
        rect,
        theme,
        response.hovered(),
        response.is_pointer_button_down_on(),
        false,
        filled,
        slot,
    );

    if !glass {
        let background = if filled {
            if response.hovered() {
                theme::lerp_color(theme.accent_a, Color32::WHITE, 0.14)
            } else {
                theme.accent_a
            }
        } else if response.hovered() {
            Color32::from_rgba_unmultiplied(255, 255, 255, 20)
        } else {
            Color32::TRANSPARENT
        };

        painter.circle_filled(rect.center(), rect.width() * 0.5, background);
        if filled {
            painter.circle_stroke(
                rect.center(),
                rect.width() * 0.5,
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 60)),
            );
        }
    }
    paint_glyph(
        painter,
        rect.center(),
        rect.width(),
        glyph,
        if filled {
            theme.on_accent()
        } else {
            theme.text()
        },
    );

    response
}

pub fn circle_button(
    ui: &Ui,
    rect: Rect,
    id: &str,
    glyph: Glyph,
    theme: &Theme,
    slot: GlassSlot,
) -> Response {
    let response = ui
        .interact(rect, ui.scope_id().with(id), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    theme::paint_icon_button(
        ui.painter(),
        rect,
        theme,
        response.hovered(),
        false,
        slot,
    );
    paint_glyph(
        ui.painter(),
        rect.center(),
        rect.width() * 0.9,
        glyph,
        theme.text(),
    );
    response
}

pub fn pill_button_sized(
    ui: &mut Ui,
    width: f32,
    label: &str,
    primary: bool,
    theme: &Theme,
    slot: GlassSlot,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 38.0), Sense::click());
    let painter = ui.painter();
    theme::paint_pill(
        painter,
        rect,
        theme,
        response.hovered(),
        response.is_pointer_button_down_on(),
        false,
        primary,
        slot,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(12.5),
        if primary {
            theme.on_accent()
        } else {
            theme.text()
        },
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn chip_button(
    ui: &mut Ui,
    label: &str,
    active: bool,
    width: f32,
    theme: &Theme,
    slot: GlassSlot,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click());
    let painter = ui.painter();
    theme::paint_pill(
        painter,
        rect,
        theme,
        response.hovered(),
        response.is_pointer_button_down_on(),
        active,
        false,
        slot,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        if active { theme.text() } else { theme.muted() },
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn chip(
    ui: &Ui,
    rect: Rect,
    id: &str,
    label: &str,
    active: bool,
    theme: &Theme,
    slot: GlassSlot,
) -> Response {
    let response = ui
        .interact(rect, ui.scope_id().with(id), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let painter = ui.painter();
    theme::paint_pill(
        painter,
        rect,
        theme,
        response.hovered(),
        response.is_pointer_button_down_on(),
        active,
        false,
        slot,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        if active { theme.text() } else { theme.muted() },
    );
    response
}

/// Flat progress/volume slider. Returns the requested fraction when the user clicks or drags.
pub fn slider(
    ui: &Ui,
    rect: Rect,
    id: &str,
    fraction: f32,
    enabled: bool,
    theme: &Theme,
) -> (Response, Option<f32>) {
    let response = if enabled {
        ui.interact(rect, ui.scope_id().with(id), Sense::click_and_drag())
    } else {
        ui.interact(rect, ui.scope_id().with(id), Sense::hover())
    };

    let painter = ui.painter();
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    let fraction = fraction.clamp(0.0, 1.0);

    painter.rect_filled(
        track,
        2.0,
        Color32::from_rgba_unmultiplied(255, 255, 255, 26),
    );

    let filled = Rect::from_min_max(
        track.min,
        pos2(track.left() + track.width() * fraction, track.max.y),
    );
    theme::gradient_rect(
        painter,
        filled,
        2.0,
        theme.accent_a,
        theme.accent_b,
        theme.angle,
    );

    if enabled && (response.hovered() || response.dragged()) {
        let knob = pos2(filled.right(), track.center().y);
        if !crate::glass::paint_orb(
            painter,
            knob,
            5.5,
            theme,
            response.hovered(),
            response.dragged(),
        ) {
            painter.circle_filled(knob, 5.5, Color32::WHITE);
        }
    }

    let mut requested = None;
    if enabled && (response.dragged() || response.clicked()) {
        if let Some(pointer) = response.interact_pointer_pos() {
            let value = ((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0);
            requested = Some(value);
        }
    }

    (response, requested)
}

/* ---------- text/feedback ---------- */

/// Shortens `text` with an ellipsis so it fits `max_width` points.
pub fn ellipsize(
    painter: &Painter,
    text: &str,
    font: &FontId,
    color: Color32,
    max_width: f32,
) -> String {
    if max_width <= 0.0 {
        return String::new();
    }
    if painter
        .layout_no_wrap(text.to_string(), font.clone(), color)
        .size()
        .x
        <= max_width
    {
        return text.to_string();
    }

    let chars: Vec<char> = text.chars().collect();
    let mut low = 0usize;
    let mut high = chars.len();

    while low < high {
        let mid = (low + high).div_ceil(2);
        let mut candidate: String = chars[..mid].iter().collect();
        candidate.push('…');

        if painter
            .layout_no_wrap(candidate, font.clone(), color)
            .size()
            .x
            <= max_width
        {
            low = mid;
        } else {
            high = mid - 1;
        }
    }

    let mut output: String = chars[..low].iter().collect();
    output.push('…');
    output
}

/// Thin progress bar. `None` fraction draws a moving segment (unknown size).
pub fn progress_bar(
    painter: &Painter,
    rect: Rect,
    fraction: Option<f32>,
    color: Color32,
    time: f64,
    theme: &Theme,
) {
    let radius = CornerRadius::from(rect.height() * 0.5);
    painter.rect_filled(
        rect,
        radius,
        Color32::from_rgba_unmultiplied(255, 255, 255, 24),
    );

    match fraction {
        Some(value) => {
            let width = rect.width() * value.clamp(0.0, 1.0);
            if width > 0.5 {
                theme::gradient_rect(
                    painter,
                    Rect::from_min_size(rect.min, vec2(width, rect.height())),
                    rect.height() * 0.5,
                    color,
                    theme.accent_b,
                    theme.angle,
                );
            }
        }
        None => {
            let segment = rect.width() * 0.3;
            let progress = ((time * 0.55) % 1.0) as f32;
            let start = rect.left() - segment + (rect.width() + segment) * progress;
            let from = start.max(rect.left());
            let to = (start + segment).min(rect.right());
            if to > from {
                painter.rect_filled(
                    Rect::from_min_max(pos2(from, rect.top()), pos2(to, rect.bottom())),
                    radius,
                    color,
                );
            }
        }
    }
}

/// Menu row with a tick indicator, used by the component menu.
pub fn menu_toggle_item(ui: &mut Ui, label: &str, enabled: bool, theme: &Theme) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    let painter = ui.painter();

    if response.hovered() {
        painter.rect_filled(
            rect,
            CornerRadius::from(9.0),
            Color32::from_rgba_unmultiplied(255, 255, 255, 18),
        );
    }

    let box_rect = Rect::from_center_size(
        pos2(rect.left() + 20.0, rect.center().y),
        vec2(15.0, 15.0),
    );
    if enabled {
        painter.rect_filled(box_rect, 4.0, theme.accent_b);
        let center = box_rect.center();
        let ink = Color32::from_rgb(12, 12, 18);
        painter.line_segment(
            [pos2(center.x - 3.6, center.y + 0.2), pos2(center.x - 0.9, center.y + 2.9)],
            Stroke::new(1.7_f32, ink),
        );
        painter.line_segment(
            [pos2(center.x - 0.9, center.y + 2.9), pos2(center.x + 3.8, center.y - 2.8)],
            Stroke::new(1.7_f32, ink),
        );
    } else {
        painter.rect_stroke(
            box_rect,
            4.0,
            Stroke::new(1.4_f32, theme.muted()),
            egui::StrokeKind::Inside,
        );
    }

    painter.text(
        pos2(rect.left() + 40.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.0),
        if enabled { theme.text() } else { theme.muted() },
    );

    response
}

pub fn menu_item(ui: &mut Ui, label: &str, glyph: Glyph, theme: &Theme) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        if theme.real_glass {
            painter.rect_filled(
                rect,
                CornerRadius::from(9.0),
                Color32::from_rgba_unmultiplied(255, 255, 255, 18),
            );
        } else {
            let glass = crate::glass::paint_button(
                painter,
                rect.shrink2(vec2(2.0, 1.0)),
                theme,
                true,
                false,
                false,
                false,
                GlassSlot::Menu,
            );
            if !glass {
                painter.rect_filled(
                    rect,
                    CornerRadius::from(9.0),
                    Color32::from_rgba_unmultiplied(255, 255, 255, 18),
                );
            }
        }
    }
    paint_glyph(
        painter,
        pos2(rect.left() + 20.0, rect.center().y),
        18.0,
        glyph,
        theme.accent_b,
    );
    painter.text(
        pos2(rect.left() + 40.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        theme.text(),
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Small text with an accent dot, used for "live" and scanning states.
pub fn status_dot(ui: &Ui, pos: egui::Pos2, label: &str, active: bool, theme: &Theme) {
    let painter = ui.painter();
    let color = if active {
        theme.accent_b
    } else {
        theme.muted()
    };
    painter.circle_filled(pos, 3.5, color);
    painter.text(
        pos + vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(11.0),
        color,
    );
}
