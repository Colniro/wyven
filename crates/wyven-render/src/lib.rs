//! GPU rendering for Wyven: owns the wgpu device and surface and draws text with glyphon.
//!
//! The renderer is deliberately "dumb": the app decides what to show and passes a list of
//! [`TextBlock`]s each frame. Shaped text lives in glyphon [`Buffer`]s owned by the caller, so
//! text is only re-shaped when it actually changes.

use std::sync::Arc;

mod quad;

pub use glyphon::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style, Weight, Wrap};
pub use quad::{Quad, Rgba, RoundRect};
use quad::Layer;
use quad::QuadPipeline;
use glyphon::{
    Cache, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use winit::window::Window;

/// A piece of already-shaped text to draw at a position (in physical pixels).
pub struct TextBlock<'a> {
    pub buffer: &'a Buffer,
    pub x: f32,
    pub y: f32,
    /// Color for spans without their own color.
    pub color: Rgba,
    /// Clip rectangle (left, top, right, bottom); `None` = whole window.
    pub clip: Option<[i32; 4]>,
}

/// Everything to draw in one frame, back to front.
pub struct Scene<'a> {
    pub background: Rgba,
    /// Rounded boxes behind everything (the cards panels and editors sit on).
    pub cards: &'a [RoundRect],
    /// Rectangles behind the text (panels, selection, current line).
    pub under: &'a [Quad],
    pub text: &'a [TextBlock<'a>],
    /// Rectangles in front of the text (cursors).
    pub over: &'a [Quad],
    /// Rounded shapes in front of the text: corner masks and outlines of the cards.
    pub masks: &'a [RoundRect],
    /// Rounded boxes and shadows of floating popups, below `overlay`.
    pub overlay_cards: &'a [RoundRect],
    /// Floating boxes (menus, popups): drawn last, above everything else.
    pub overlay: &'a [Quad],
    pub overlay_text: &'a [TextBlock<'a>],
}

/// Picks the first installed monospace font of a preference list.
pub fn pick_monospace_family(font_system: &FontSystem) -> Option<String> {
    const PREFERRED: &[&str] = &[
        "Cascadia Mono",
        "Cascadia Code",
        "JetBrains Mono",
        "Consolas",
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Noto Sans Mono",
    ];
    let db = font_system.db();
    PREFERRED
        .iter()
        .find(|name| {
            db.faces()
                .any(|f| f.families.iter().any(|(fam, _)| fam.eq_ignore_ascii_case(name)))
        })
        .map(|s| s.to_string())
}

/// Width of one character cell for a monospace font.
pub fn measure_cell_width(font_system: &mut FontSystem, metrics: Metrics, attrs: &Attrs) -> f32 {
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_size(None, None);
    buffer.set_text("MMMMMMMMMM", attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    buffer
        .layout_runs()
        .next()
        .map(|run| run.line_w / 10.0)
        .unwrap_or(metrics.font_size * 0.6)
}

/// Information about the GPU, shown in the debug overlay.
#[derive(Debug, Clone)]
pub struct GpuInfo {
    pub name: String,
    pub backend: String,
    pub present_mode: String,
}

/// A further window drawn with the same device, fonts and glyph atlas (floating panels).
struct Extra {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    window: Arc<Window>,
}

pub struct Renderer {
    extra: Vec<Extra>,
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu_info: GpuInfo,

    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    overlay_text_renderer: TextRenderer,
    quads: QuadPipeline,

    // Keep the window last so it is dropped after the surface.
    window: Arc<Window>,
}

/// Creates an instance limited to `backends` and finds an adapter that can draw to the window.
async fn connect(
    window: &Arc<Window>,
    display: &winit::event_loop::OwnedDisplayHandle,
    backends: wgpu::Backends,
) -> Option<(wgpu::Instance, wgpu::Surface<'static>, wgpu::Adapter)> {
    let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(display.clone()));
    desc.backends = backends;
    let instance = wgpu::Instance::new(desc);
    let surface = instance.create_surface(window.clone()).ok()?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            // An editor does not need the big GPU: the integrated one starts faster and uses
            // less RAM and battery. WYVEN_GPU=high picks the dedicated GPU instead.
            power_preference: if std::env::var("WYVEN_GPU").is_ok_and(|v| v == "high") {
                wgpu::PowerPreference::HighPerformance
            } else {
                wgpu::PowerPreference::LowPower
            },
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .ok()?;
    Some((instance, surface, adapter))
}

impl Renderer {
    pub async fn new(window: Arc<Window>, display: winit::event_loop::OwnedDisplayHandle) -> Self {
        let size = window.inner_size();

        // Backend order, measured on Windows (Intel Arc + RTX 4050 laptop):
        //   OpenGL: ~32 MB private RAM, first frame ~0.85 s
        //   DX12:  ~285 MB private RAM, first frame ~1.6 s (DXGI loads every GPU driver)
        // So Windows tries GL first and falls back to DX12. Linux prefers Vulkan.
        // WGPU_BACKEND=dx12|vulkan|gl forces one backend.
        let order: &[wgpu::Backends] = if cfg!(windows) {
            &[wgpu::Backends::GL, wgpu::Backends::DX12]
        } else {
            &[wgpu::Backends::VULKAN, wgpu::Backends::GL]
        };
        let forced = wgpu::Backends::from_env();
        let order: Vec<wgpu::Backends> = match forced {
            Some(b) => vec![b],
            None => order.to_vec(),
        };
        let mut connected = None;
        for backends in order {
            if let Some(c) = connect(&window, &display, backends).await {
                connected = Some(c);
                break;
            }
            log::warn!("no usable GPU adapter for {backends:?}, trying next backend");
        }
        let (instance, surface, adapter) = connected.expect("no usable GPU backend found");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("wyven"),
                ..Default::default()
            })
            .await
            .expect("failed to create GPU device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        // Mailbox shows the newest frame without waiting for vsync: lowest input latency
        // without tearing. Fall back to Fifo (always supported) otherwise.
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let info = adapter.get_info();
        let gpu_info = GpuInfo {
            name: info.name,
            backend: format!("{:?}", info.backend),
            present_mode: format!("{present_mode:?}"),
        };
        log::info!("GPU: {gpu_info:?}, surface format {format:?}");

        let t = std::time::Instant::now();
        let font_system = FontSystem::new();
        log::info!(
            "font system: {} fonts in {:?}",
            font_system.db().len(),
            t.elapsed()
        );
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let overlay_text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        let quads = QuadPipeline::new(&device, format);

        Self {
            extra: Vec::new(),
            quads,
            instance,
            device,
            queue,
            surface,
            config,
            gpu_info,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            overlay_text_renderer,
            window,
        }
    }

    pub fn font_system(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    pub fn gpu_info(&self) -> &GpuInfo {
        &self.gpu_info
    }

    pub fn scale_factor(&self) -> f32 {
        self.window.scale_factor() as f32
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return; // minimized
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Adds a window to draw into (with [`Renderer::draw_window`]). `false` if the GPU cannot
    /// draw there.
    pub fn add_window(&mut self, window: Arc<Window>) -> bool {
        let Ok(surface) = self.instance.create_surface(window.clone()) else { return false };
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            width: size.width.max(1),
            height: size.height.max(1),
            ..self.config.clone()
        };
        surface.configure(&self.device, &config);
        self.extra.push(Extra { surface, config, window });
        true
    }

    pub fn remove_window(&mut self, id: winit::window::WindowId) {
        self.extra.retain(|e| e.window.id() != id);
    }

    pub fn has_window(&self, id: winit::window::WindowId) -> bool {
        self.extra.iter().any(|e| e.window.id() == id)
    }

    pub fn window_size(&self, id: winit::window::WindowId) -> Option<(u32, u32)> {
        self.extra.iter().find(|e| e.window.id() == id).map(|e| (e.config.width, e.config.height))
    }

    pub fn resize_window(&mut self, id: winit::window::WindowId, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if let Some(e) = self.extra.iter_mut().find(|e| e.window.id() == id) {
            e.config.width = width;
            e.config.height = height;
            e.surface.configure(&self.device, &e.config);
        }
    }

    /// Draws one frame. Returns `false` if the frame was skipped and should be retried.
    pub fn draw(&mut self, scene: &Scene<'_>) -> bool {
        self.draw_on(None, scene)
    }

    /// Draws one frame into an added window.
    pub fn draw_window(&mut self, id: winit::window::WindowId, scene: &Scene<'_>) -> bool {
        match self.extra.iter().position(|e| e.window.id() == id) {
            Some(i) => self.draw_on(Some(i), scene),
            None => true,
        }
    }

    fn draw_on(&mut self, target: Option<usize>, scene: &Scene<'_>) -> bool {
        let (width, height) = match target {
            Some(i) => (self.extra[i].config.width, self.extra[i].config.height),
            None => self.size(),
        };
        self.viewport
            .update(&self.queue, Resolution { width, height });
        let ranges = self.quads.prepare(
            &self.device,
            &self.queue,
            (width, height),
            &[
                Layer::Rounds(scene.cards),
                Layer::Quads(scene.under),
                Layer::Quads(scene.over),
                Layer::Rounds(scene.masks),
                Layer::Rounds(scene.overlay_cards),
                Layer::Quads(scene.overlay),
            ],
        );
        fn to_area<'a>(b: &TextBlock<'a>, width: u32, height: u32) -> TextArea<'a> {
            let [left, top, right, bottom] =
                b.clip.unwrap_or([0, 0, width as i32, height as i32]);
            TextArea {
                buffer: b.buffer,
                left: b.x,
                top: b.y,
                scale: 1.0,
                bounds: TextBounds { left, top, right, bottom },
                default_color: b.color.to_glyphon(),
                custom_glyphs: &[],
            }
        }
        for (renderer, blocks) in [
            (&mut self.text_renderer, scene.text),
            (&mut self.overlay_text_renderer, scene.overlay_text),
        ] {
            if let Err(e) = renderer.prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                blocks.iter().map(|b| to_area(b, width, height)),
                &mut self.swash_cache,
            ) {
                log::error!("text prepare failed: {e}");
                return false;
            }
        }

        let (surface, config, window) = match target {
            Some(i) => {
                let e = &mut self.extra[i];
                (&mut e.surface, &e.config, &e.window)
            }
            None => (&mut self.surface, &self.config, &self.window),
        };
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return false;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                surface.configure(&self.device, config);
                return false;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                *surface = self
                    .instance
                    .create_surface(window.clone())
                    .expect("failed to recreate GPU surface");
                surface.configure(&self.device, config);
                return false;
            }
            wgpu::CurrentSurfaceTexture::Validation => panic!("GPU validation error"),
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(scene.background.to_clear()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.quads.render(&mut pass, ranges[0].clone());
            self.quads.render(&mut pass, ranges[1].clone());
            if let Err(e) = self
                .text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
            {
                log::error!("text render failed: {e}");
            }
            self.quads.render(&mut pass, ranges[2].clone());
            self.quads.render(&mut pass, ranges[3].clone());
            self.quads.render(&mut pass, ranges[4].clone());
            self.quads.render(&mut pass, ranges[5].clone());
            if let Err(e) = self
                .overlay_text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
            {
                log::error!("overlay text render failed: {e}");
            }
        }

        self.queue.submit(Some(encoder.finish()));
        window.pre_present_notify();
        self.queue.present(frame);
        self.atlas.trim();
        true
    }
}
