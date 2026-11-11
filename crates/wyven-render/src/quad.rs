//! Instanced solid rectangles.

use std::mem::size_of;

/// sRGB color with alpha, as written in themes (`#RRGGBBAA`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self(r, g, b, 255)
    }

    pub const fn with_alpha(self, a: u8) -> Self {
        Self(self.0, self.1, self.2, a)
    }

    /// Parses `#RGB`, `#RRGGBB` or `#RRGGBBAA`.
    pub fn parse(s: &str) -> Option<Self> {
        let h = s.strip_prefix('#')?;
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        match h.len() {
            3 => {
                let n = |i: usize| u8::from_str_radix(h.get(i..i + 1)?, 16).ok().map(|v| v * 17);
                Some(Self(n(0)?, n(1)?, n(2)?, 255))
            }
            6 => Some(Self(byte(0)?, byte(2)?, byte(4)?, 255)),
            8 => Some(Self(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    pub fn to_linear(self) -> [f32; 4] {
        fn lin(c: u8) -> f32 {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        }
        [lin(self.0), lin(self.1), lin(self.2), self.3 as f32 / 255.0]
    }

    pub fn to_glyphon(self) -> glyphon::Color {
        glyphon::Color::rgba(self.0, self.1, self.2, self.3)
    }

    pub fn to_clear(self) -> wgpu::Color {
        let [r, g, b, a] = self.to_linear();
        wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: a as f64 }
    }
}

/// A filled rectangle in physical pixels.
#[derive(Debug, Clone, Copy)]
pub struct Quad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: Rgba,
}

/// A rounded rectangle; with `soft` > 0 its edge fades out over that many pixels (shadows).
#[derive(Debug, Clone, Copy)]
pub struct RoundRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: Rgba,
    pub radius: f32,
    pub soft: f32,
    /// Greater than 0: only an outline this wide. Less than 0: everything inside the rectangle
    /// but outside the rounded box (paints the corners away). 0: filled.
    pub ring: f32,
}

impl RoundRect {
    pub fn new(x: f32, y: f32, w: f32, h: f32, color: Rgba, radius: f32) -> Self {
        Self { x, y, w, h, color, radius, soft: 0.0, ring: 0.0 }
    }

    /// A soft shadow around a box: `spread` pixels larger on every side, a bit lower.
    pub fn shadow(x: f32, y: f32, w: f32, h: f32, color: Rgba, radius: f32, spread: f32) -> Self {
        Self {
            x: x - spread,
            y: y - spread + spread * 0.4,
            w: w + 2.0 * spread,
            h: h + 2.0 * spread,
            color,
            radius: radius + spread,
            soft: spread,
            ring: 0.0,
        }
    }

    /// An outline of `width` pixels along the rounded edge.
    pub fn outline(x: f32, y: f32, w: f32, h: f32, color: Rgba, radius: f32, width: f32) -> Self {
        Self { x, y, w, h, color, radius, soft: 0.0, ring: width }
    }

    /// Paints the four corners outside the rounded box with `color` (the background), so square
    /// content inside looks rounded.
    pub fn corner_mask(x: f32, y: f32, w: f32, h: f32, color: Rgba, radius: f32) -> Self {
        Self { x, y, w, h, color, radius, soft: 0.0, ring: -1.0 }
    }
}

/// One layer of rectangles, drawn in order.
pub enum Layer<'a> {
    Quads(&'a [Quad]),
    Rounds(&'a [RoundRect]),
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Instance {
    rect: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
}

impl Instance {
    fn bytes(items: &[Instance]) -> &[u8] {
        // SAFETY: Instance is repr(C), made only of f32s, no padding.
        unsafe { std::slice::from_raw_parts(items.as_ptr().cast(), std::mem::size_of_val(items)) }
    }
}

pub(crate) struct QuadPipeline {
    pipeline: wgpu::RenderPipeline,
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: usize,
    scratch: Vec<Instance>,
}

impl QuadPipeline {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let capacity = 256;
        Self {
            pipeline,
            globals,
            bind_group,
            instances: create_instances(device, capacity),
            capacity,
            scratch: Vec::new(),
        }
    }

    /// Uploads all layers; returns one instance range per layer.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        layers: &[Layer<'_>],
    ) -> Vec<std::ops::Range<u32>> {
        let globals = [size.0 as f32, size.1 as f32, 0.0, 0.0];
        let bytes: Vec<u8> = globals.iter().flat_map(|f| f.to_ne_bytes()).collect();
        queue.write_buffer(&self.globals, 0, &bytes);

        self.scratch.clear();
        let mut ranges = Vec::with_capacity(layers.len());
        for layer in layers {
            let start = self.scratch.len() as u32;
            match layer {
                Layer::Quads(quads) => self.scratch.extend(quads.iter().map(|q| Instance {
                    rect: [q.x, q.y, q.w, q.h],
                    color: q.color.to_linear(),
                    params: [0.0; 4],
                })),
                Layer::Rounds(rounds) => self.scratch.extend(rounds.iter().map(|r| Instance {
                    rect: [r.x, r.y, r.w, r.h],
                    color: r.color.to_linear(),
                    params: [r.radius, r.soft, r.ring, 0.0],
                })),
            }
            ranges.push(start..self.scratch.len() as u32);
        }
        if self.scratch.len() > self.capacity {
            self.capacity = self.scratch.len().next_power_of_two();
            self.instances = create_instances(device, self.capacity);
        }
        if !self.scratch.is_empty() {
            queue.write_buffer(&self.instances, 0, Instance::bytes(&self.scratch));
        }
        ranges
    }

    pub(crate) fn render(&self, pass: &mut wgpu::RenderPass<'_>, range: std::ops::Range<u32>) {
        if range.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..4, range);
    }
}

fn create_instances(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("quad instances"),
        size: (capacity * size_of::<Instance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
