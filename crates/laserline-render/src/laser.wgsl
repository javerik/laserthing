// Laser line fragment shader (mirrored on the CPU in `laserline_core::cpu`).
// Every pixel is evaluated analytically:
// pixel -> line space (along/across) -> deformation (+ visibility) -> cross-section profile
// -> end caps -> speckle -> ambient + sensor noise.

struct Uniforms {
    center: vec2f,       // line center, framebuffer px
    dir: vec2f,          // unit direction along the line
    length: f32,
    width: f32,
    intensity: f32,      // 0..1
    end_soft: f32,
    color: vec4f,
    profile: u32,        // 0 gaussian, 1 top-hat, 2 hard
    edge_soft: f32,
    deform_start: f32,   // u of the first deformation sample
    deform_step: f32,
    ambient: f32,
    speckle: f32,
    grain: f32,
    noise: f32,
    seed: u32,
    deform_count: u32,   // 0 = no deformation
    srgb_out: u32,       // 1 if the target encodes sRGB itself
    _pad: u32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
// Per sample along the line: (displacement px, intensity 0..1), row-major in rows of
// DEFORM_WIDTH texels. A texture rather than a storage buffer so WebGL2 can read it.
@group(0) @binding(1) var deform: texture_2d<f32>;
const DEFORM_WIDTH: u32 = 1024u;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    // One oversized triangle covering the whole viewport.
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

fn hash(x: i32, y: i32, z: u32) -> f32 {
    return f32(pcg(bitcast<u32>(x) ^ pcg(bitcast<u32>(y) ^ pcg(z)))) / 4294967295.0;
}

fn value_noise(p: vec2f, s: u32) -> f32 {
    let i = vec2i(floor(p));
    let f = fract(p);
    let w = f * f * (3.0 - 2.0 * f);
    let a = hash(i.x, i.y, s);
    let b = hash(i.x + 1, i.y, s);
    let c = hash(i.x, i.y + 1, s);
    let d = hash(i.x + 1, i.y + 1, s);
    return mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
}

fn deform_at(x: f32) -> vec2f {
    if u.deform_count == 0u {
        return vec2f(0.0, 1.0);
    }
    let last = f32(u.deform_count - 1u);
    let fi = clamp((x - u.deform_start) / u.deform_step, 0.0, last);
    let i0 = u32(floor(fi));
    let i1 = min(i0 + 1u, u.deform_count - 1u);
    return mix(deform_sample(i0), deform_sample(i1), fract(fi));
}

fn deform_sample(i: u32) -> vec2f {
    return textureLoad(deform, vec2i(i32(i % DEFORM_WIDTH), i32(i / DEFORM_WIDTH)), 0).xy;
}

// Mirrors model::profile_intensity.
fn profile(v: f32) -> f32 {
    let a = abs(v);
    let hw = 0.5 * u.width;
    switch u.profile {
        case 0u: {
            let sigma = max(u.width / 2.35482, 1e-3);
            return exp(-0.5 * (a / sigma) * (a / sigma));
        }
        case 1u: {
            let s = max(u.edge_soft, 1e-3);
            return 1.0 - smoothstep(hw - 0.5 * s, hw + 0.5 * s, a);
        }
        default: {
            return clamp(min(a + 0.5, hw) - max(a - 0.5, -hw), 0.0, 1.0);
        }
    }
}

fn end_mask(along: f32) -> f32 {
    let d = abs(along) - 0.5 * u.length; // >0 outside the line
    if u.end_soft > 0.0 {
        return 1.0 - smoothstep(-0.5 * u.end_soft, 0.5 * u.end_soft, d);
    }
    return clamp(0.5 - d, 0.0, 1.0);
}

fn srgb_to_linear(c: vec3f) -> vec3f {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3f(2.4));
    return select(hi, lo, c <= vec3f(0.04045));
}

@fragment
fn fs_main(@builtin(position) frag: vec4f) -> @location(0) vec4f {
    let p = frag.xy - u.center;
    let along = dot(p, u.dir);
    let d = deform_at(along);
    let across = dot(p, vec2f(-u.dir.y, u.dir.x)) - d.x;

    var laser = u.intensity * d.y * profile(across) * end_mask(along);

    if u.speckle > 0.0 && laser > 0.0 {
        let n = value_noise(frag.xy / max(u.grain, 0.5), u.seed);
        // Exponential intensity statistics (mean 1), as in fully developed speckle.
        let s = -log(max(1.0 - n, 1e-4));
        laser *= mix(1.0, s, u.speckle);
    }

    var col = u.color.rgb * laser + vec3f(u.ambient);

    if u.noise > 0.0 {
        let px = vec2i(frag.xy);
        // Sum of two uniforms: triangular distribution with std 1/sqrt(6).
        let r = hash(px.x, px.y, u.seed ^ 0x9e3779b9u) + hash(px.x, px.y, u.seed ^ 0x85ebca6bu) - 1.0;
        col += vec3f(r * 2.449 * u.noise);
    }

    col = clamp(col, vec3f(0.0), vec3f(1.0));
    if u.srgb_out == 1u {
        // Target re-encodes to sRGB; pre-invert so pixel values equal `col`.
        col = srgb_to_linear(col);
    }
    return vec4f(col, 1.0);
}
