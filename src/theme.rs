use eframe::egui::{
    self, pos2, vec2, Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, TextureHandle,
};

use crate::config::UiConfig;

/// The only fixed colour in the app: the near-black canvas the design asks for.
pub const BLACK: Color32 = Color32::from_rgb(6, 6, 9);

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub accent_a: Color32,
    pub accent_b: Color32,
    pub angle: f32,
    pub glass: f32,
    pub radius: f32,
    pub glow: f32,
    pub frame_color: Color32,
    pub frame_visible: bool,
    pub frame_opacity: f32,
    pub frame_border_visible: bool,
    pub frame_border_width: f32,
    pub panel_border_visible: bool,
    pub panel_border_width: f32,
    pub background_color: Color32,
    pub background_dim: f32,
}

impl Theme {
    pub fn from_ui(ui: &UiConfig) -> Self {
        Self {
            accent_a: Color32::from_rgb(ui.accent_a[0], ui.accent_a[1], ui.accent_a[2]),
            accent_b: Color32::from_rgb(ui.accent_b[0], ui.accent_b[1], ui.accent_b[2]),
            angle: ui.angle,
            glass: ui.glass,
            radius: ui.radius,
            glow: ui.glow,
            frame_color: Color32::from_rgb(ui.frame_color[0], ui.frame_color[1], ui.frame_color[2]),
            frame_visible: ui.frame_visible,
            frame_opacity: ui.frame_opacity,
            frame_border_visible: ui.frame_border_visible,
            frame_border_width: ui.frame_border_width,
            panel_border_visible: ui.panel_border_visible,
            panel_border_width: ui.panel_border_width,
            background_color: Color32::from_rgb(
                ui.background_color[0],
                ui.background_color[1],
                ui.background_color[2],
            ),
            background_dim: ui.background_dim,
        }
    }

    pub fn bg(&self) -> Color32 {
        BLACK
    }

    pub fn text(&self) -> Color32 {
        Color32::from_rgb(244, 245, 249)
    }

    pub fn muted(&self) -> Color32 {
        Color32::from_rgb(158, 162, 178)
    }

    pub fn faint(&self) -> Color32 {
        Color32::from_rgb(103, 106, 124)
    }

    pub fn accent(&self) -> Color32 {
        lerp_color(self.accent_a, self.accent_b, 0.5)
    }

    pub fn on_accent(&self) -> Color32 {
        let c = self.accent();
        let luma = 0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32;
        if luma > 160.0 {
            Color32::from_rgb(12, 12, 16)
        } else {
            Color32::WHITE
        }
    }

    pub fn panel_top(&self) -> Color32 {
        let base = Color32::from_rgb(23, 23, 31);
        with_alpha(lerp_color(base, self.accent_a, 0.13 * self.glass), 238)
    }

    pub fn panel_bottom(&self) -> Color32 {
        let base = Color32::from_rgb(12, 12, 17);
        with_alpha(lerp_color(base, self.accent_b, 0.07 * self.glass), 246)
    }

    pub fn panel_soft(&self) -> Color32 {
        with_alpha(
            lerp_color(
                Color32::from_rgb(26, 26, 35),
                self.accent_a,
                0.10 * self.glass,
            ),
            210,
        )
    }

    pub fn line(&self) -> Color32 {
        Color32::from_rgba_unmultiplied(255, 255, 255, 18)
    }

    pub fn accent_line(&self) -> Color32 {
        with_alpha(self.accent_a, (34.0 + 62.0 * self.glow) as u8)
    }

    pub fn danger(&self) -> Color32 {
        Color32::from_rgb(255, 121, 138)
    }
}

pub fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

pub fn darken(color: Color32, amount: f32) -> Color32 {
    let amount = amount.clamp(0.0, 1.0);
    Color32::from_rgba_unmultiplied(
        (color.r() as f32 * (1.0 - amount)) as u8,
        (color.g() as f32 * (1.0 - amount)) as u8,
        (color.b() as f32 * (1.0 - amount)) as u8,
        color.a(),
    )
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}

/// Adds a rounded rectangle filled with a linear gradient. The mesh is built as
/// a triangle fan from the centre so corners stay round and the colour ramp is
/// exact (the gradient is linear in x/y).
pub fn gradient_rect(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    a: Color32,
    b: Color32,
    angle: f32,
) {
    if rect.width() <= 0.5 || rect.height() <= 0.5 {
        return;
    }
    let radius = radius.clamp(0.0, rect.width().min(rect.height()) * 0.5);
    let dir = vec2(angle.to_radians().cos(), angle.to_radians().sin());
    let center = rect.center();
    let extent = (rect.width() * dir.x.abs() + rect.height() * dir.y.abs()).max(1.0);
    let color_at = |p: Pos2| {
        let t = ((p - center).dot(dir) / extent + 0.5).clamp(0.0, 1.0);
        lerp_color(a, b, t)
    };

    let points = rounded_rect_points(rect, radius, 7);
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, color_at(center));
    for point in &points {
        mesh.colored_vertex(*point, color_at(*point));
    }
    let count = points.len() as u32;
    for index in 0..count {
        mesh.add_triangle(0, 1 + index, 1 + (index + 1) % count);
    }
    painter.add(Shape::mesh(mesh));
}

pub fn rounded_rect_points(rect: Rect, radius: f32, segments: usize) -> Vec<Pos2> {
    let radius = radius.clamp(0.0, rect.width().min(rect.height()) * 0.5);
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

    for (center, from, to) in corners {
        for step in 0..=segments {
            let angle = from + (to - from) * step as f32 / segments as f32;
            points.push(center + vec2(angle.cos(), angle.sin()) * radius);
        }
    }
    points
}

/// Dark glass panel: gradient fill, faint accent sheen, thin border and shadow.
pub fn paint_card(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    theme: &Theme,
    hovered: bool,
    active: bool,
) {
    let radius = radius.clamp(0.0, rect.width().min(rect.height()) * 0.5);

    // Soft drop shadow, built from a few translucent rounded rects.
    for (grow, alpha) in [(8.0, 10u8), (5.0, 16), (2.0, 28)] {
        painter.rect_filled(
            rect.expand(grow).translate(vec2(0.0, 3.0)),
            radius + grow,
            Color32::from_black_alpha(alpha),
        );
    }

    gradient_rect(
        painter,
        rect,
        radius,
        theme.panel_top(),
        theme.panel_bottom(),
        theme.angle,
    );

    // Faint accent sheen from the top edge.
    gradient_rect(
        painter,
        rect.shrink(1.0),
        (radius - 1.0).max(0.0),
        with_alpha(theme.accent_a, (10.0 + 18.0 * theme.glow) as u8),
        Color32::TRANSPARENT,
        theme.angle + 90.0,
    );

    if theme.panel_border_visible && theme.panel_border_width > 0.01 {
        let border = if active {
            with_alpha(theme.accent(), 130)
        } else if hovered {
            with_alpha(theme.accent_a, 90)
        } else {
            with_alpha(theme.accent_a, (30.0 + 28.0 * theme.glow) as u8)
        };
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(theme.panel_border_width, border),
            StrokeKind::Inside,
        );
    }
}

/// The outer frame: the area between the window edge and the inner dashboard.
/// Its base colour is `frame_color`, independent from the inner dashboard.
pub fn paint_backdrop(painter: &Painter, rect: Rect, theme: &Theme, time: f64) {
    painter.rect_filled(rect, 0.0, theme.bg());

    let alpha = (theme.frame_opacity.clamp(0.0, 1.0) * 255.0) as u8;
    if alpha > 0 {
        let base = with_alpha(theme.frame_color, alpha);
        let top = lerp_color(base, theme.accent_a, 0.07 * theme.glass);
        let bottom = lerp_color(
            with_alpha(darken(theme.frame_color, 0.45), alpha),
            theme.accent_b,
            0.05 * theme.glass,
        );
        gradient_rect(painter, rect, 0.0, top, bottom, theme.angle);
    }

    // Slow, restrained ambient glows so the outer frame does not look flat.
    let drift = ((time * 0.15) as f32).sin() * 0.5 + 0.5;
    let left_center = pos2(
        rect.left() + rect.width() * (0.16 + 0.03 * drift),
        rect.top() + rect.height() * 0.20,
    );
    radial_glow(painter, left_center, 300.0, theme.accent_a, 10);
    radial_glow(
        painter,
        left_center + vec2(30.0, 20.0),
        170.0,
        theme.accent_a,
        8,
    );

    let right_center = pos2(
        rect.right() - rect.width() * 0.10,
        rect.bottom() - rect.height() * 0.16,
    );
    radial_glow(painter, right_center, 340.0, theme.accent_b, 9);
    radial_glow(
        painter,
        right_center - vec2(20.0, 30.0),
        190.0,
        theme.accent_b,
        7,
    );

    // Vignette toward the window edge.
    gradient_rect(
        painter,
        rect,
        rect.width().min(rect.height()) * 0.02,
        Color32::TRANSPARENT,
        Color32::from_black_alpha(80),
        90.0,
    );
}

/// Smooth radial fade from a colour to transparent.
pub fn radial_glow(painter: &Painter, center: Pos2, radius: f32, color: Color32, alpha: u8) {
    if radius <= 0.5 {
        return;
    }
    let segments = 56usize;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, with_alpha(color, alpha));
    for index in 0..segments {
        let angle = std::f32::consts::TAU * index as f32 / segments as f32;
        mesh.colored_vertex(
            center + vec2(angle.cos(), angle.sin()) * radius,
            Color32::TRANSPARENT,
        );
    }
    for index in 0..segments as u32 {
        mesh.add_triangle(0, 1 + index, 1 + (index + 1) % segments as u32);
    }
    painter.add(Shape::mesh(mesh));
}

/// The inner dashboard surface that holds every component. It uses its own
/// dark glass (or the background photo) rather than the outer frame colour.
pub fn paint_dashboard_frame(
    painter: &Painter,
    rect: Rect,
    theme: &Theme,
    background: Option<&TextureHandle>,
) {
    if !theme.frame_visible {
        return;
    }
    let radius = (theme.radius + 10.0).min(rect.width().min(rect.height()) * 0.12);

    for (grow, shadow_alpha) in [(18.0, 8u8), (10.0, 14), (4.0, 30)] {
        painter.rect_filled(
            rect.expand(grow).translate(vec2(0.0, 6.0)),
            radius + grow,
            Color32::from_black_alpha(shadow_alpha),
        );
    }

    // The colour is always the base, and doubles as the fallback when the
    // optional photo is cleared.
    let top = lerp_color(theme.background_color, theme.accent_a, 0.10 * theme.glass);
    let bottom = lerp_color(
        darken(theme.background_color, 0.55),
        theme.accent_b,
        0.06 * theme.glass,
    );
    gradient_rect(painter, rect, radius, top, bottom, theme.angle);

    if let Some(texture) = background {
        textured_rounded_rect(
            painter,
            rect,
            radius,
            texture,
            texture_uv(texture.size_vec2(), rect.size()),
        );
        painter.rect_filled(
            rect,
            radius,
            Color32::from_black_alpha((theme.background_dim.clamp(0.0, 1.0) * 255.0) as u8),
        );
        // Tie the photo to the current accent theme.
        gradient_rect(
            painter,
            rect,
            radius,
            with_alpha(theme.accent_a, 26),
            with_alpha(theme.accent_b, 18),
            theme.angle,
        );
    } else {
        // Inner top glow.
        gradient_rect(
            painter,
            rect.shrink(1.0),
            (radius - 1.0).max(0.0),
            with_alpha(theme.accent_a, (18.0 + 30.0 * theme.glow) as u8),
            Color32::TRANSPARENT,
            90.0,
        );
    }

    if theme.frame_border_visible && theme.frame_border_width > 0.01 {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(
                theme.frame_border_width,
                with_alpha(theme.accent_a, (70.0 + 70.0 * theme.glow) as u8),
            ),
            StrokeKind::Inside,
        );
    }
}

/// A texture clipped to a rounded rectangle, used for the inner background photo.
pub fn textured_rounded_rect(
    painter: &Painter,
    rect: Rect,
    radius: f32,
    texture: &TextureHandle,
    uv: Rect,
) {
    if rect.width() <= 1.0 || rect.height() <= 1.0 {
        return;
    }
    let radius = radius.clamp(0.0, rect.width().min(rect.height()) * 0.5);
    let points = rounded_rect_points(rect, radius, 10);
    let mut mesh = Mesh::default();
    mesh.texture_id = texture.id();

    let map_uv = |point: Pos2| {
        let tx = ((point.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let ty = ((point.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        pos2(
            uv.min.x + (uv.max.x - uv.min.x) * tx,
            uv.min.y + (uv.max.y - uv.min.y) * ty,
        )
    };
    let vertex = |pos: Pos2| egui::epaint::Vertex {
        pos,
        uv: map_uv(pos),
        color: Color32::WHITE,
    };

    mesh.vertices.push(vertex(rect.center()));
    for point in &points {
        mesh.vertices.push(vertex(*point));
    }
    let count = points.len() as u32;
    for index in 0..count {
        mesh.add_triangle(0, 1 + index, 1 + (index + 1) % count);
    }
    painter.add(Shape::mesh(mesh));
}

/// Used by the layout editor for its dashed outline.
pub fn dashed_rect(painter: &Painter, rect: Rect, color: Color32, dash: f32) {
    let mut x = rect.left();
    while x < rect.right() {
        let end = (x + dash).min(rect.right());
        painter.line_segment(
            [pos2(x, rect.top()), pos2(end, rect.top())],
            Stroke::new(1.0_f32, color),
        );
        painter.line_segment(
            [pos2(x, rect.bottom()), pos2(end, rect.bottom())],
            Stroke::new(1.0_f32, color),
        );
        x += dash * 2.0;
    }
    let mut y = rect.top();
    while y < rect.bottom() {
        let end = (y + dash).min(rect.bottom());
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.left(), end)],
            Stroke::new(1.0_f32, color),
        );
        painter.line_segment(
            [pos2(rect.right(), y), pos2(rect.right(), end)],
            Stroke::new(1.0_f32, color),
        );
        y += dash * 2.0;
    }
}

pub fn paint_icon_button(
    painter: &Painter,
    rect: Rect,
    theme: &Theme,
    hovered: bool,
    active: bool,
) {
    let radius = rect.width().min(rect.height()) * 0.5;
    let fill = if active {
        with_alpha(theme.accent(), 120)
    } else if hovered {
        with_alpha(theme.accent_a, 64)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 14)
    };
    painter.circle_filled(rect.center(), radius, fill);
    painter.circle_stroke(
        rect.center(),
        radius,
        Stroke::new(
            1.0_f32,
            with_alpha(theme.accent_a, if hovered { 110 } else { 52 }),
        ),
    );
}

pub fn paint_pill(
    painter: &Painter,
    rect: Rect,
    theme: &Theme,
    hovered: bool,
    active: bool,
    primary: bool,
) {
    let radius = rect.height() * 0.5;
    if primary {
        let a = if hovered {
            lerp_color(theme.accent_a, Color32::WHITE, 0.12)
        } else {
            theme.accent_a
        };
        gradient_rect(painter, rect, radius, a, theme.accent_b, theme.angle);
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 52)),
            StrokeKind::Inside,
        );
        return;
    }
    let fill = if active {
        with_alpha(theme.accent_a, 72)
    } else if hovered {
        Color32::from_rgba_unmultiplied(255, 255, 255, 26)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 13)
    };
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(
            1.0_f32,
            with_alpha(
                theme.accent_a,
                if active {
                    100
                } else if hovered {
                    60
                } else {
                    26
                },
            ),
        ),
        StrokeKind::Inside,
    );
}

pub fn texture_uv(texture: egui::Vec2, target: egui::Vec2) -> Rect {
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
