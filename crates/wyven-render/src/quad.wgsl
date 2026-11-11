// Rectangles (cursor, selection, panels) and rounded rectangles with soft edges (cards,
// shadows). One instance per rectangle, 4 vertices each.

struct Globals {
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Instance {
    @location(0) rect: vec4<f32>,   // x, y, width, height in physical pixels
    @location(1) color: vec4<f32>,  // linear RGBA, premultiplied in the fragment shader
    @location(2) params: vec4<f32>, // corner radius, edge softness (0 = sharp), ring width (>0 = only an outline of that width, <0 = everything outside the rounded box: corner masks), unused
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) half_size: vec2<f32>,
    @location(3) params: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let p = inst.rect.xy + corner * inst.rect.zw;
    let ndc = vec2<f32>(p.x / globals.size.x * 2.0 - 1.0, 1.0 - p.y / globals.size.y * 2.0);
    var out: VsOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.color = inst.color;
    out.half_size = inst.rect.zw * 0.5;
    out.local = (corner - vec2<f32>(0.5, 0.5)) * inst.rect.zw;
    out.params = inst.params;
    return out;
}

// Signed distance to a rounded box centered at 0.
fn rounded_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius, radius);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let radius = in.params.x;
    let soft = in.params.y;
    let ring = in.params.z;
    var alpha = in.color.a;
    if (radius > 0.0 || soft > 0.0 || ring != 0.0) {
        // A soft shadow shrinks by its softness so it fades out inside the rectangle.
        let d = rounded_box(in.local, in.half_size - vec2<f32>(soft, soft), max(radius - soft, 0.0) + soft);
        let edge = max(soft, 0.5);
        var cover = clamp(0.5 - d / (2.0 * edge), 0.0, 1.0);
        if (ring > 0.0) {
            // Only the band of width `ring` along the edge.
            cover = cover * clamp(0.5 + (d + ring), 0.0, 1.0);
        } else if (ring < 0.0) {
            cover = 1.0 - cover;
        }
        alpha = alpha * cover;
    }
    return vec4<f32>(in.color.rgb * alpha, alpha);
}
