//! GPU rendering for Wyvern: owns the wgpu device and surface and draws text with glyphon.
//!
//! The renderer is deliberately "dumb": the app decides what to show and passes a list of
//! [`TextBlock`]s each frame. Shaped text lives in glyphon [`Buffer`]s owned by the caller, so
//! text is only re-shaped when it actually changes.

use std::sync::Arc;

pub use glyphon::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping};
/// Background clear color. Components are in *linear* space (the surface is sRGB).
pub use wgpu::Color as ClearColor;
use glyphon::{
    Cache, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use winit::window::Window;

/// A piece of already-shaped text to draw at a position (in physical pixels).
pub struct TextBlock<'a> {
    pub buffer: &'a Buffer,
    pub x: f32,
    pub y: f32,
    pub color: Color,
}

/// Information about the GPU, shown in the debug overlay.
#[derive(Debug, Clone)]
pub struct GpuInfo {
    pub name: String,
    pub backend: String,
    pub present_mode: String,
}

pub struct Renderer {
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
            // less RAM and battery. WYVERN_GPU=high picks the dedicated GPU instead.
            power_preference: if std::env::var("WYVERN_GPU").is_ok_and(|v| v == "high") {
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
                label: Some("wyvern"),
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

        Self {
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
            window,
        }
    }

    pub fn font_system(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    pub fn gpu_info(&self) -> &GpuInfo {
        &self.gpu_info
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

    /// Draws one frame. Returns `false` if the frame was skipped and should be retried.
    pub fn draw(&mut self, background: ClearColor, blocks: &[TextBlock<'_>]) -> bool {
        let (width, height) = self.size();
        self.viewport
            .update(&self.queue, Resolution { width, height });

        let areas = blocks.iter().map(|b| TextArea {
            buffer: b.buffer,
            left: b.x,
            top: b.y,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            },
            default_color: b.color,
            custom_glyphs: &[],
        });
        if let Err(e) = self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash_cache,
        ) {
            log::error!("text prepare failed: {e}");
            return false;
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return false;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .instance
                    .create_surface(self.window.clone())
                    .expect("failed to recreate GPU surface");
                self.surface.configure(&self.device, &self.config);
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
                        load: wgpu::LoadOp::Clear(background),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Err(e) = self
                .text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)
            {
                log::error!("text render failed: {e}");
            }
        }

        self.queue.submit(Some(encoder.finish()));
        self.window.pre_present_notify();
        self.queue.present(frame);
        self.atlas.trim();
        true
    }
}
