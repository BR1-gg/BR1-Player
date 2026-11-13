#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod config;
mod deps;
mod download;
mod glass;
mod library;
mod queue;
mod theme;
mod ui;
mod visualizer;
mod video_bg;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use egui::ViewportId;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::app::PlayerApp;
use crate::config::GlassTone;
use crate::glass::{GlassShapeSpec, GlassSlot, GlassSpec};

const CONTENT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut autoplay = false;
    let mut open_downloads = false;
    let mut download_url: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--play" => autoplay = true,
            "--downloads" => open_downloads = true,
            "--download" => {
                if let Some(url) = args.next() {
                    download_url = Some(url);
                    open_downloads = true;
                }
            }
            "--help" | "-h" => {
                println!("BR1 Player");
                println!("Usage: br1_player [options] [file or folder ...]");
                println!();
                println!("  --play             start playing the first track after loading");
                println!("  --downloads        open the download page at startup");
                println!("  --download <url>   open the download page and start downloading a link");
                println!("  --help             show this help");
                return Ok(());
            }
            other if !other.starts_with("--") => paths.push(PathBuf::from(other)),
            _ => {}
        }
    }

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::new(paths, autoplay, open_downloads, download_url);
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct App {
    initial_paths: Vec<PathBuf>,
    autoplay: bool,
    open_downloads: bool,
    download_url: Option<String>,

    window: Option<Arc<Window>>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    gpu: Option<Gpu>,
    player: Option<PlayerApp>,
    liquid: Option<Liquid>,
    last_frame: Instant,
}

impl App {
    fn new(
        initial_paths: Vec<PathBuf>,
        autoplay: bool,
        open_downloads: bool,
        download_url: Option<String>,
    ) -> Self {
        Self {
            initial_paths,
            autoplay,
            open_downloads,
            download_url,
            window: None,
            egui_ctx: egui::Context::default(),
            egui_state: None,
            gpu: None,
            player: None,
            liquid: None,
            last_frame: Instant::now(),
        }
    }

    fn render(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        let Some(state) = self.egui_state.as_mut() else {
            return;
        };
        let Some(player) = self.player.as_mut() else {
            return;
        };
        let Some(liquid) = self.liquid.as_mut() else {
            return;
        };

        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        gpu.ensure_size(size.width, size.height);

        let raw_input = state.take_egui_input(&window);
        player.update_logic(&self.egui_ctx);
        glass::begin_frame();

        let mut content_shape_count = 0usize;
        let output = self.egui_ctx.run_ui(raw_input, |ui| {
            player.draw_background(ui);
            content_shape_count = self.egui_ctx.graphics(|graphics| {
                graphics
                    .get(egui::LayerId::background())
                    .map(|list| list.all_entries().len())
                    .unwrap_or(0)
            });
            player.draw_foreground(ui);
        });
        let specs = glass::take_specs();

        let egui::FullOutput {
            platform_output,
            mut textures_delta,
            mut shapes,
            pixels_per_point,
            ..
        } = output;

        state.handle_platform_output(&window, platform_output);
        let split = content_shape_count.min(shapes.len());
        let foreground_shapes = shapes.split_off(split);
        let background_shapes = shapes;
        let background_primitives =
            self.egui_ctx.tessellate(background_shapes, pixels_per_point);
        let foreground_primitives =
            self.egui_ctx.tessellate(foreground_shapes, pixels_per_point);
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point,
        };

        apply_textures(
            &mut gpu.renderer_content,
            &gpu.device,
            &gpu.queue,
            &textures_delta,
        );
        apply_textures(
            &mut gpu.renderer_surface,
            &gpu.device,
            &gpu.queue,
            &textures_delta,
        );

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("def-player encoder"),
            });
        let background_extra = gpu.renderer_content.update_buffers(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &background_primitives,
            &screen_descriptor,
        );
        let foreground_extra = gpu.renderer_surface.update_buffers(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &foreground_primitives,
            &screen_descriptor,
        );
        if !background_extra.is_empty() {
            gpu.queue.submit(background_extra);
        }
        if !foreground_extra.is_empty() {
            gpu.queue.submit(foreground_extra);
        }

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui background pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &gpu.content.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            gpu.renderer_content.render(
                &mut pass.forget_lifetime(),
                &background_primitives,
                &screen_descriptor,
            );
        }

        let dt = self.last_frame.elapsed().as_secs_f32().min(0.25);
        self.last_frame = Instant::now();
        liquid.sync(&specs, &self.egui_ctx, dt);

        let surface_texture = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => texture,
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gpu.configure();
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return,
        };
        let target_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let liquid_frame = liquid_rust::Frame {
            content: &gpu.content.view,
            target: &target_view,
            size: (size.width, size.height),
            scale: pixels_per_point,
            content_changed: true,
        };
        liquid
            .renderer
            .render(&gpu.device, &gpu.queue, &mut encoder, &liquid_frame, &liquid.scene);

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui foreground pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            gpu.renderer_surface.render(
                &mut pass.forget_lifetime(),
                &foreground_primitives,
                &screen_descriptor,
            );
        }

        gpu.queue.submit(std::iter::once(encoder.finish()));
        gpu.queue.present(surface_texture);

        for id in &textures_delta.free {
            gpu.renderer_content.free_texture(id);
            gpu.renderer_surface.free_texture(id);
        }
        textures_delta.clear();
    }
}

fn apply_textures(
    renderer: &mut egui_wgpu::Renderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    delta: &egui::TexturesDelta,
) {
    for (id, deltas) in &delta.set {
        for image in deltas {
            renderer.update_texture(device, queue, *id, image);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title("BR1 Player")
            .with_inner_size(LogicalSize::new(1180.0, 760.0))
            .with_min_inner_size(LogicalSize::new(900.0, 560.0));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("failed to create the BR1 Player window"),
        );

        let gpu = pollster::block_on(Gpu::new(window.clone())).expect("failed to start wgpu");
        let egui_state = egui_winit::State::new(
            self.egui_ctx.clone(),
            ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let player = PlayerApp::new(
            &self.egui_ctx,
            std::mem::take(&mut self.initial_paths),
            self.autoplay,
            self.open_downloads,
            self.download_url.take(),
            true,
        );
        let liquid = Liquid::new(&gpu.device, gpu.config.format);

        self.window = Some(window.clone());
        self.egui_state = Some(egui_state);
        self.gpu = Some(gpu);
        self.player = Some(player);
        self.liquid = Some(liquid);
        self.last_frame = Instant::now();

        window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if window.id() != window_id {
            return;
        }

        let response = self
            .egui_state
            .as_mut()
            .map(|state| state.on_window_event(&window, &event));
        let repaint = response.map(|response| response.repaint).unwrap_or(false);

        match event {
            WindowEvent::CloseRequested => {
                if let Some(player) = self.player.as_mut() {
                    player.on_close();
                }
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.ensure_size(size.width, size.height);
                }
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = window.inner_size();
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.ensure_size(size.width, size.height);
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }

        if repaint {
            window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let liquid_animating = self.liquid.as_ref().is_some_and(|liquid| liquid.animating);
        if self.egui_ctx.has_requested_repaint() || liquid_animating {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
}

/* ---------- wgpu surface + egui renderer ---------- */

struct Gpu {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer_content: egui_wgpu::Renderer,
    renderer_surface: egui_wgpu::Renderer,
    content: Offscreen,
}

impl Gpu {
    async fn new(window: Arc<Window>) -> Result<Self, Box<dyn std::error::Error>> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("BR1 Player"),
                ..Default::default()
            })
            .await?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| {
                matches!(
                    format,
                    wgpu::TextureFormat::Rgba8UnormSrgb | wgpu::TextureFormat::Bgra8UnormSrgb
                )
            })
            .unwrap_or(capabilities.formats[0]);

        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let mut config = surface
            .get_default_config(&adapter, width, height)
            .expect("the window surface is not supported by this adapter");
        config.format = format;
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&device, &config);

        let content = Offscreen::new(&device, width, height);
        let renderer_content = egui_wgpu::Renderer::new(
            &device,
            CONTENT_FORMAT,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: false,
                ..Default::default()
            },
        );
        let renderer_surface = egui_wgpu::Renderer::new(
            &device,
            format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: false,
                ..Default::default()
            },
        );

        Ok(Self {
            _instance: instance,
            surface,
            _adapter: adapter,
            device,
            queue,
            config,
            renderer_content,
            renderer_surface,
            content,
        })
    }

    fn ensure_size(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if self.config.width == width
            && self.config.height == height
            && self.content.width == width
            && self.content.height == height
        {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.content = Offscreen::new(&self.device, width, height);
    }

    fn configure(&mut self) {
        self.surface.configure(&self.device, &self.config);
    }
}

struct Offscreen {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl Offscreen {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("egui content"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CONTENT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            _texture: texture,
            view,
            width,
            height,
        }
    }
}

/* ---------- liquid-rust scene ---------- */

struct Liquid {
    renderer: liquid_rust::Renderer,
    scene: liquid_rust::Scene,
    ids: HashMap<GlassKey, liquid_rust::GlassId>,
    containers: HashMap<u64, liquid_rust::ContainerId>,
    pointer_down: bool,
    animating: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlassKey {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    slot: u8,
}

impl GlassKey {
    fn new(spec: &GlassSpec) -> Self {
        let rect = spec.rect;
        Self {
            x: rect.center().x.round() as i32,
            y: rect.center().y.round() as i32,
            w: rect.width().round() as i32,
            h: rect.height().round() as i32,
            slot: spec.slot as u8,
        }
    }
}

impl Liquid {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self {
            renderer: liquid_rust::Renderer::new(device, format),
            scene: liquid_rust::Scene::new(),
            ids: HashMap::new(),
            containers: HashMap::new(),
            pointer_down: false,
            animating: false,
        }
    }

    fn sync(&mut self, specs: &[GlassSpec], ctx: &egui::Context, dt: f32) {
        if let Some(first) = specs.first() {
            let style = first.style;
            self.scene.appearance.dark = style.dark;
            self.scene.appearance.transparency = style.transparency;
            self.scene.appearance.reduce_transparency = style.reduce_transparency;
            self.scene.appearance.increase_contrast = style.increase_contrast;
            self.scene.appearance.reduce_motion = style.reduce_motion;
            self.scene.appearance.adapt_to_content = style.adapt_to_content;
            self.scene.physics = style.physics;
            self.scene.glass_on_glass = style.glass_on_glass;
            self.scene.set_light_angle(style.light_angle);
        }

        let mut seen = HashSet::with_capacity(specs.len());
        for spec in specs {
            let key = GlassKey::new(spec);
            seen.insert(key);
            let material = material_for(spec);
            let shape = shape_for(spec.shape);
            if let Some(id) = self.ids.get(&key).copied() {
                self.scene.set_frame(id, rect(spec));
                self.scene
                    .set_material_with(id, material, liquid_rust::Spring::new(0.3, 0.0));
                self.scene.set_shape(id, shape);
            } else {
                let container = spec.container.map(|container| {
                    let scene = &mut self.scene;
                    let id = *self
                        .containers
                        .entry(container)
                        .or_insert_with(|| scene.add_container(spec.spacing, spec.z));
                    scene.set_container_spacing(id, spec.spacing);
                    id
                });
                let mut glass = liquid_rust::Glass::new(rect(spec))
                    .shape(shape)
                    .material(material)
                    .z(spec.z);
                if spec.interactive {
                    glass = glass.interactive();
                }
                if let Some(container) = container {
                    glass = glass.container(container);
                }
                let id = self.scene.add(glass);
                self.ids.insert(key, id);
            }
        }

        {
            let scene = &mut self.scene;
            self.ids.retain(|key, id| {
                if seen.contains(key) {
                    true
                } else {
                    scene.remove(*id);
                    false
                }
            });
        }

        let pointer = ctx.input(|input| input.pointer.hover_pos());
        let down = ctx.input(|input| input.pointer.primary_down());
        if let Some(position) = pointer {
            self.scene
                .pointer_move(liquid_rust::Vec2::new(position.x, position.y));
        }
        if down && !self.pointer_down {
            if let Some(position) = pointer {
                self.scene
                    .pointer_down(liquid_rust::Vec2::new(position.x, position.y));
            }
        } else if !down && self.pointer_down {
            self.scene.pointer_up();
        }
        self.pointer_down = down;
        self.animating = self.scene.update(dt);
    }
}

fn rect(spec: &GlassSpec) -> liquid_rust::Rect {
    let rect = spec.rect;
    liquid_rust::Rect::new(rect.left(), rect.top(), rect.width(), rect.height())
}

fn shape_for(shape: GlassShapeSpec) -> liquid_rust::Shape {
    match shape {
        GlassShapeSpec::Capsule => liquid_rust::Shape::Capsule,
        GlassShapeSpec::Rounded(radius) => liquid_rust::Shape::Rounded(radius),
        GlassShapeSpec::Circular(radius) => liquid_rust::Shape::Circular(radius),
        GlassShapeSpec::Smooth { radius, smoothing } => {
            liquid_rust::Shape::Smooth { radius, smoothing }
        }
    }
}

fn color(color: egui::Color32, alpha: f32) -> liquid_rust::Color {
    liquid_rust::Color {
        r: color.r() as f32 / 255.0,
        g: color.g() as f32 / 255.0,
        b: color.b() as f32 / 255.0,
        a: alpha,
    }
}

fn tone(tone: GlassTone) -> liquid_rust::Tone {
    liquid_rust::Tone {
        lift: tone.lift,
        luminance: tone.luminance,
        luminance_mix: tone.luminance_mix,
        lighten: tone.lighten,
        darken_to: tone.darken_to,
        darken: tone.darken,
    }
}

fn material_for(spec: &GlassSpec) -> liquid_rust::Material {
    let style = spec.style;
    let tint = if spec.primary {
        Some(color(spec.accent_a, 0.88))
    } else if style.tint_enabled {
        Some(liquid_rust::Color {
            r: style.tint_color[0] as f32 / 255.0,
            g: style.tint_color[1] as f32 / 255.0,
            b: style.tint_color[2] as f32 / 255.0,
            a: style.tint_strength,
        })
    } else if spec.active {
        Some(color(spec.accent_b, 0.45))
    } else if spec.hovered && spec.slot != GlassSlot::Panel {
        Some(color(spec.accent_a, 0.18))
    } else {
        None
    };

    let mut material = liquid_rust::Material {
        refraction: style.refraction,
        depth: style.depth,
        dispersion: style.dispersion,
        frost: style.frost,
        splay: style.splay,
        specular: style.specular,
        rim: style.rim,
        rim_shade: style.rim_shade,
        hairline: style.hairline,
        edge: style.edge,
        shadow: liquid_rust::Shadow {
            offset_y: style.shadow.offset_y,
            blur: style.shadow.blur,
            opacity: style.shadow.opacity,
        },
        light_tone: tone(style.light_tone),
        dark_tone: tone(style.dark_tone),
        adaptive: style.adaptive,
        tint,
        dim: style.dim,
    };

    if spec.slot == GlassSlot::Panel && spec.z >= 4 {
        material.dark_tone.darken = material.dark_tone.darken.max(0.45);
        material.light_tone.lighten = material.light_tone.lighten.max(0.55);
        material.rim = material.rim.max(0.30);
        material.edge = material.edge.max(0.40);
        material.shadow.opacity = material.shadow.opacity.max(0.18);
        if material.tint.is_none() {
            material.tint = Some(color(spec.accent_a, 0.20));
        }
    }

    material
}