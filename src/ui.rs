use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, FontId, Painter, Rect, Response,
    Sense, Shape, Stroke, TextureHandle, Ui, Vec2,
};

/* ---------- palette ---------- */

pub const BG: Color32 = Color32::from_rgb(11, 11, 15);
pub const PANEL: Color32 = Color32::from_rgb(17, 17, 24);
pub const PANEL_SOFT: Color32 = Color32::from_rgb(22, 22, 31);
pub const PANEL_HOVER: Color32 = Color32::from_rgb(29, 29, 40);
pub const LINE: Color32 = Color32::from_rgba_premultiplied(18, 18, 18, 18);
pub const TEXT: Color32 = Color32::from_rgb(236, 236, 243);
pub const MUTED: Color32 = Color32::from_rgb(139, 139, 158);
pub const ACCENT: Color32 = Color32::from_rgb(124, 92, 255);
pub const ACCENT_BRIGHT: Color32 = Color32::from_rgb(140, 111, 255);
pub const ACCENT_SOFT: Color32 = Color32::from_rgba_premultiplied(21, 16, 44, 44);
pub const NOTE: Color32 = Color32::from_rgb(167, 148, 255);
pub const DANGER: Color32 = Color32::from_rgb(255, 128, 128);
pub const ON_ACCENT: Color32 = Color32::WHITE;

pub const COVER_FALLBACK: Color32 = Color32::from_rgb(28, 24, 44);

/* ---------- covers ---------- */

pub fn paint_cover(ui: &Ui, rect: Rect, texture: Option<&TextureHandle>, radius: f32) {
    match texture {
        Some(texture) => {
            egui::Image::new(egui::load::SizedTexture::from_handle(texture))
                .corner_radius(radius)
                .uv(cover_uv(texture.size_vec2(), rect.size()))
                .paint_at(ui, rect);
        }
        None => placeholder_cover(ui.painter(), rect, radius),
    }
}

/// UV window that fills `target` while keeping the image's aspect ratio
/// (centre-cropped, never stretched).
fn cover_uv(texture: Vec2, target: Vec2) -> Rect {
    let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

    if texture.x <= 0.0 || texture.y <= 0.0 || target.x <= 0.0 || target.y <= 0.0 {
        return full;
    }

    let texture_aspect = texture.x / texture.y;
    let target_aspect = target.x / target.y;
    if (texture_aspect - target_aspect).abs() < 0.001 {
        return full;
    }

    let mut uv = full;
    if texture_aspect > target_aspect {
        // Wider than the box: keep the full height, crop the sides.
        let width = target_aspect / texture_aspect;
        let pad = (1.0 - width) * 0.5;
        uv.min.x = pad;
        uv.max.x = 1.0 - pad;
    } else {
        let height = texture_aspect / target_aspect;
        let pad = (1.0 - height) * 0.5;
        uv.min.y = pad;
        uv.max.y = 1.0 - pad;
    }

    uv
}

pub fn placeholder_cover(painter: &Painter, rect: Rect, radius: f32) {
    painter.rect_filled(rect, radius, COVER_FALLBACK);

    let size = rect.width().min(rect.height());
    painter.circle_filled(
        rect.center() - vec2(0.0, size * 0.06),
        size * 0.30,
        Color32::from_rgba_unmultiplied(124, 92, 255, 30),
    );

    paint_note(painter, rect, NOTE);
}

fn paint_note(painter: &Painter, rect: Rect, color: Color32) {
    let size = rect.width().min(rect.height());
    let center = rect.center();
    let head = size * 0.058;
    let stem = size * 0.024;
    let left = pos2(center.x - size * 0.095, center.y + size * 0.105);
    let right = pos2(center.x + size * 0.105, center.y + size * 0.035);
    let top = center.y - size * 0.255;

    painter.circle_filled(left, head, color);
    painter.circle_filled(right, head, color);

    let left_stem = Rect::from_min_size(
        pos2(left.x + head * 0.72, top),
        vec2(stem, left.y - top - head * 0.35),
    );
    let right_stem = Rect::from_min_size(
        pos2(right.x + head * 0.72, top + size * 0.035),
        vec2(stem, right.y - top - size * 0.035 - head * 0.35),
    );
    painter.rect_filled(left_stem, stem * 0.5, color);
    painter.rect_filled(right_stem, stem * 0.5, color);

    painter.rect_filled(
        Rect::from_min_size(
            pos2(left.x + head * 0.72, top),
            vec2(right.x - left.x, size * 0.042),
        ),
        size * 0.01,
        color,
    );
}

/* ---------- glyphs ---------- */

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Prev,
    Play,
    Pause,
    Next,
}

fn paint_glyph(painter: &Painter, center: egui::Pos2, scale: f32, glyph: Glyph, color: Color32) {
    let s = scale * 0.33;
    let fill = Stroke::NONE;

    match glyph {
        Glyph::Play => {
            painter.add(Shape::convex_polygon(
                vec![
                    center + vec2(-s * 0.55, -s * 0.95),
                    center + vec2(-s * 0.55, s * 0.95),
                    center + vec2(s * 0.95, 0.0),
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
            let dir = if glyph == Glyph::Prev { -1.0 } else { 1.0 };
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
    }
}

pub fn glyph_button(
    ui: &Ui,
    rect: Rect,
    id: &str,
    glyph: Glyph,
    filled: bool,
) -> Response {
    let response = ui
        .interact(rect, ui.id().with(id), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let painter = ui.painter();

    let background = if filled {
        if response.hovered() {
            Color32::from_rgb(245, 245, 250)
        } else {
            Color32::WHITE
        }
    } else if response.hovered() {
        Color32::from_rgba_unmultiplied(255, 255, 255, 18)
    } else {
        Color32::TRANSPARENT
    };

    painter.circle_filled(rect.center(), rect.width() * 0.5, background);
    paint_glyph(
        painter,
        rect.center(),
        rect.width(),
        glyph,
        if filled { BG } else { TEXT },
    );

    response
}

/* ---------- controls ---------- */

pub fn pill_button(ui: &mut Ui, label: &str, primary: bool) -> Response {
    let width = ui.available_width();
    pill_button_sized(ui, width, label, primary)
}

pub fn pill_button_sized(ui: &mut Ui, width: f32, label: &str, primary: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 38.0), Sense::click());
    let painter = ui.painter();

    let background = if primary {
        if response.hovered() {
            ACCENT_BRIGHT
        } else {
            ACCENT
        }
    } else if response.hovered() {
        PANEL_HOVER
    } else {
        PANEL_SOFT
    };

    painter.rect_filled(rect, CornerRadius::from(10.0), background);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(13.0),
        if primary {
            ON_ACCENT
        } else if response.hovered() {
            TEXT
        } else {
            MUTED
        },
    );

    response.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn nav_item(ui: &mut Ui, label: &str, active: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    let painter = ui.painter();

    if active || response.hovered() {
        painter.rect_filled(
            rect,
            CornerRadius::from(10.0),
            if active { PANEL_SOFT } else { PANEL_HOVER },
        );
    }

    painter.circle_filled(pos2(rect.left() + 18.0, rect.center().y), 3.0, ACCENT);
    painter.text(
        pos2(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.5),
        if active { TEXT } else { MUTED },
    );

    response.on_hover_cursor(CursorIcon::PointingHand)
}

pub fn chip(ui: &Ui, rect: Rect, id: &str, label: &str, active: bool) -> Response {
    let response = ui
        .interact(rect, ui.id().with(id), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let painter = ui.painter();

    let background = if active {
        ACCENT_SOFT
    } else if response.hovered() {
        PANEL_HOVER
    } else {
        Color32::TRANSPARENT
    };

    painter.rect_filled(rect, CornerRadius::from(rect.height() * 0.5), background);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.5),
        if active {
            NOTE
        } else if response.hovered() {
            TEXT
        } else {
            MUTED
        },
    );

    response
}

/// Flat progress/volume slider. Returns the requested fraction when the user clicks or drags.
pub fn slider(ui: &Ui, rect: Rect, id: &str, fraction: f32, enabled: bool) -> (Response, Option<f32>) {
    let response = if enabled {
        ui.interact(rect, ui.id().with(id), Sense::click_and_drag())
    } else {
        ui.interact(rect, ui.id().with(id), Sense::hover())
    };

    let painter = ui.painter();
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    let fraction = fraction.clamp(0.0, 1.0);

    painter.rect_filled(track, CornerRadius::from(2.0), Color32::from_rgba_unmultiplied(255, 255, 255, 30));

    let filled = Rect::from_min_max(track.min, pos2(track.left() + track.width() * fraction, track.max.y));
    painter.rect_filled(filled, CornerRadius::from(2.0), if enabled { ACCENT } else { MUTED });

    if enabled && (response.hovered() || response.dragged()) {
        painter.circle_filled(pos2(filled.right(), track.center().y), 5.5, Color32::WHITE);
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
        let mid = (low + high + 1) / 2;
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

/// Space-allocating chip button (settings toggles, small actions).
pub fn chip_button(ui: &mut Ui, label: &str, active: bool, width: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click());
    let painter = ui.painter();

    let background = if active {
        ACCENT_SOFT
    } else if response.hovered() {
        PANEL_HOVER
    } else {
        PANEL_SOFT
    };

    painter.rect_filled(rect, CornerRadius::from(rect.height() * 0.5), background);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        if active {
            NOTE
        } else if response.hovered() {
            TEXT
        } else {
            MUTED
        },
    );

    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Thin progress bar. `None` fraction draws a moving segment (unknown size).
pub fn progress_bar(
    painter: &Painter,
    rect: Rect,
    fraction: Option<f32>,
    color: Color32,
    time: f64,
) {
    let radius = CornerRadius::from(rect.height() * 0.5);
    painter.rect_filled(
        rect,
        radius,
        Color32::from_rgba_unmultiplied(255, 255, 255, 26),
    );

    match fraction {
        Some(value) => {
            let width = rect.width() * value.clamp(0.0, 1.0);
            if width > 0.5 {
                painter.rect_filled(
                    Rect::from_min_size(rect.min, vec2(width, rect.height())),
                    radius,
                    color,
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

/// A row in the sidebar playlist list.
pub fn playlist_row(
    ui: &mut Ui,
    title: &str,
    count: usize,
    texture: Option<&TextureHandle>,
    active: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
    let painter = ui.painter();

    if active || response.hovered() {
        painter.rect_filled(
            rect,
            CornerRadius::from(10.0),
            if active { PANEL_SOFT } else { PANEL_HOVER },
        );
    }

    let cover = Rect::from_min_size(
        pos2(rect.left() + 6.0, rect.center().y - 14.0),
        vec2(28.0, 28.0),
    );
    paint_cover(ui, cover, texture, 6.0);

    let text_left = cover.right() + 10.0;
    let font = FontId::proportional(12.5);
    let color = if active { TEXT } else { MUTED };
    let max_width = (rect.right() - 30.0 - text_left).max(20.0);

    painter.text(
        pos2(text_left, rect.center().y),
        Align2::LEFT_CENTER,
        ellipsize(painter, title, &font, color, max_width),
        font,
        color,
    );
    painter.text(
        pos2(rect.right() - 6.0, rect.center().y),
        Align2::RIGHT_CENTER,
        count.to_string(),
        FontId::proportional(11.0),
        MUTED,
    );

    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Draws a small pill (used for counters and hints).
pub fn badge(ui: &mut Ui, text: &str) {
    egui::Frame::NONE
        .fill(PANEL_SOFT)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(CornerRadius::from(13.0))
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(12.0).color(MUTED));
        });
}
