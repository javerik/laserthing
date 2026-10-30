//! CPU renderer of the emulated laser line.
//!
//! Evaluates the same analytical model as `laser.wgsl` (`laserline-render`) per pixel:
//! line-space projection, deformation with visibility, cross-section profile, soft end
//! caps, speckle, ambient light and sensor noise. Colour is reduced to its brightest
//! channel (`max(r, g, b)`), which is 1.0 for every wavelength colour and the default
//! white laser, so a monochrome camera sees full intensity regardless of wavelength.

use crate::emulator::Frame;
use crate::gpu_types::Uniforms;
use crate::model::FWHM_PER_SIGMA;

fn pcg(v: u32) -> u32 {
    let s = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let w = ((s >> ((s >> 28).wrapping_add(4))) ^ s).wrapping_mul(277_803_737);
    (w >> 22) ^ w
}

fn hash(x: i32, y: i32, z: u32) -> f32 {
    pcg((x as u32) ^ pcg((y as u32) ^ pcg(z))) as f32 / 4_294_967_295.0
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn fract_gl(x: f32) -> f32 {
    x - x.floor()
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn value_noise(px: f32, py: f32, s: u32) -> f32 {
    let ix = px.floor() as i32;
    let iy = py.floor() as i32;
    let fx = fract_gl(px);
    let fy = fract_gl(py);
    let wx = fx * fx * (3.0 - 2.0 * fx);
    let wy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash(ix, iy, s);
    let b = hash(ix.wrapping_add(1), iy, s);
    let c = hash(ix, iy.wrapping_add(1), s);
    let d = hash(ix.wrapping_add(1), iy.wrapping_add(1), s);
    mix(mix(a, b, wx), mix(c, d, wx), wy)
}

fn deform_at(u: &Uniforms, deform: Option<&[[f32; 2]]>, x: f32) -> [f32; 2] {
    let Some(samples) = deform else {
        return [0.0, 1.0];
    };
    let count = (u.deform_count as usize).min(samples.len());
    if count == 0 {
        return [0.0, 1.0];
    }
    let last = (count - 1) as f32;
    let fi = ((x - u.deform_start) / u.deform_step).clamp(0.0, last);
    let i0 = fi.floor() as usize;
    let i1 = (i0 + 1).min(count - 1);
    let t = fract_gl(fi);
    let a = samples[i0];
    let b = samples[i1];
    [mix(a[0], b[0], t), mix(a[1], b[1], t)]
}

/// Cross-section intensity at distance `v` px from the line center (`profile` in `laser.wgsl`).
fn profile(u: &Uniforms, v: f32) -> f32 {
    let a = v.abs();
    let hw = 0.5 * u.width;
    match u.profile {
        0 => {
            let sigma = (u.width / FWHM_PER_SIGMA).max(1e-3);
            let r = a / sigma;
            // exp(-0.5 * 36) < 1.6e-8, well below 16-bit quantization.
            if r >= 6.0 { 0.0 } else { (-0.5 * r * r).exp() }
        }
        1 => {
            let s = u.edge_soft.max(1e-3);
            1.0 - smoothstep(hw - 0.5 * s, hw + 0.5 * s, a)
        }
        _ => ((a + 0.5).min(hw) - (a - 0.5).max(-hw)).clamp(0.0, 1.0),
    }
}

fn end_mask(u: &Uniforms, along: f32) -> f32 {
    let d = along.abs() - 0.5 * u.length;
    if u.end_soft > 0.0 {
        1.0 - smoothstep(-0.5 * u.end_soft, 0.5 * u.end_soft, d)
    } else {
        (0.5 - d).clamp(0.0, 1.0)
    }
}

fn srgb_to_linear_ch(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Evaluates `laser.wgsl`'s fragment shader at fragment coordinates `(frag_x, frag_y)`
/// (pixel centre `(x + 0.5, y + 0.5)` for pixel `(x, y)`), returning linear/pre-inverted
/// RGB in `0.0..=1.0`.
pub fn eval_pixel_rgb(u: &Uniforms, deform: Option<&[[f32; 2]]>, frag_x: f32, frag_y: f32) -> [f32; 3] {
    let (laser, noise) = eval_terms(u, deform, frag_x, frag_y);
    let mut col = [
        (u.color[0] * laser + u.ambient + noise).clamp(0.0, 1.0),
        (u.color[1] * laser + u.ambient + noise).clamp(0.0, 1.0),
        (u.color[2] * laser + u.ambient + noise).clamp(0.0, 1.0),
    ];
    if u.srgb_out == 1 {
        col = col.map(srgb_to_linear_ch);
    }
    col
}

/// Precomputed bounds for culling rows/pixels far from the line.
#[derive(Clone, Copy)]
struct LineBounds {
    half_along: f32,
    /// Conservative max perpendicular distance `|px * -dir.y + py * dir.x|` where the
    /// laser can be non-zero, accounting for the profile radius and max deformation.
    max_across_unshifted: f32,
    color_gain: f32,
}

impl LineBounds {
    fn new(u: &Uniforms, deform: Option<&[[f32; 2]]>) -> Self {
        let half_along = 0.5 * (u.length + if u.end_soft > 0.0 { u.end_soft } else { 1.0 });
        let profile_radius = profile_cutoff_radius(u);
        let max_deform = match deform {
            Some(samples) if u.deform_count > 0 => samples
                .iter()
                .take(u.deform_count as usize)
                .filter(|s| s[1] > 0.0)
                .fold(0.0f32, |m, s| m.max(s[0].abs())),
            _ => 0.0,
        };
        Self {
            half_along,
            max_across_unshifted: profile_radius + max_deform,
            color_gain: u.color[0].max(u.color[1]).max(u.color[2]),
        }
    }

    /// Interval `[x_min, x_max]` of `frag_x` on row `frag_y` where both `|along| <= half_along`
    /// and `|across_unshifted| <= max_across_unshifted` can hold.
    #[inline]
    fn row_x_span(&self, u: &Uniforms, frag_y: f32, x_lo: u32, x_hi: u32) -> (u32, u32) {
        if u.intensity <= 0.0 || self.color_gain <= 0.0 || x_lo >= x_hi {
            return (0, 0);
        }
        let py = frag_y - u.center[1];
        // Condition 1: | (x - cx)*dx + py*dy | <= half_along
        // Condition 2: | -(x - cx)*dy + py*dx | <= max_across_unshifted
        let mut min_x = x_lo as f32;
        let mut max_x = (x_hi - 1) as f32 + 1.0;

        let clip = |coeff: f32, rhs_center: f32, half: f32, min_x: &mut f32, max_x: &mut f32| {
            if coeff.abs() > 1e-6 {
                let a = u.center[0] + (rhs_center - half) / coeff;
                let b = u.center[0] + (rhs_center + half) / coeff;
                *min_x = min_x.max(a.min(b) - 1.0);
                *max_x = max_x.min(a.max(b) + 1.0);
            } else if rhs_center.abs() > half {
                *min_x = 1.0;
                *max_x = 0.0;
            }
        };
        clip(u.dir[0], -py * u.dir[1], self.half_along, &mut min_x, &mut max_x);
        clip(-u.dir[1], -py * u.dir[0], self.max_across_unshifted, &mut min_x, &mut max_x);

        if min_x > max_x {
            return (0, 0);
        }
        let start = (min_x.floor().max(x_lo as f32) as u32).min(x_hi);
        let end = (max_x.ceil().max(x_lo as f32) as u32).min(x_hi);
        (start, end)
    }
}
/// Evaluates the monochrome intensity (`0.0..=1.0`) at fragment coordinates
/// `(frag_x, frag_y)`. Colour/wavelength reduces to `max(r, g, b)` (1.0 for every
/// wavelength colour and for white).
pub fn eval_pixel_gray(u: &Uniforms, deform: Option<&[[f32; 2]]>, frag_x: f32, frag_y: f32) -> f32 {
    let ix = frag_x as i32;
    let iy = frag_y as i32;
    let y_hash_a = pcg((iy as u32) ^ pcg(u.seed ^ 0x9e37_79b9));
    let y_hash_b = pcg((iy as u32) ^ pcg(u.seed ^ 0x85eb_ca6b));
    let laser = eval_laser(u, deform, frag_x, frag_y);
    let noise = eval_noise_row(u, ix, y_hash_a, y_hash_b);
    let color_gain = u.color[0].max(u.color[1]).max(u.color[2]);
    let col = (color_gain * laser + u.ambient + noise).clamp(0.0, 1.0);
    if u.srgb_out == 1 { srgb_to_linear_ch(col) } else { col }
}

#[inline]
fn eval_noise_row(u: &Uniforms, ix: i32, y_hash_a: u32, y_hash_b: u32) -> f32 {
    if u.noise > 0.0 {
        let ra = pcg((ix as u32) ^ y_hash_a) as f32 * (1.0 / 4_294_967_295.0);
        let rb = pcg((ix as u32) ^ y_hash_b) as f32 * (1.0 / 4_294_967_295.0);
        (ra + rb - 1.0) * 2.449 * u.noise
    } else {
        0.0
    }
}

#[inline]
fn finish_gray(u: &Uniforms, color_gain: f32, laser: f32, noise: f32) -> f32 {
    let col = (color_gain * laser + u.ambient + noise).clamp(0.0, 1.0);
    if u.srgb_out == 1 { srgb_to_linear_ch(col) } else { col }
}

#[inline]
fn profile_cutoff_radius(u: &Uniforms) -> f32 {
    let hw = 0.5 * u.width;
    match u.profile {
        0 => 6.0 * (u.width / FWHM_PER_SIGMA).max(1e-3),
        1 => hw + 0.5 * u.edge_soft.max(1e-3),
        _ => hw + 0.5,
    }
}

#[inline]
fn eval_laser(u: &Uniforms, deform: Option<&[[f32; 2]]>, frag_x: f32, frag_y: f32) -> f32 {
    if u.intensity <= 0.0 {
        return 0.0;
    }
    let px = frag_x - u.center[0];
    let py = frag_y - u.center[1];
    let along = px * u.dir[0] + py * u.dir[1];
    let half_along = 0.5 * (u.length + if u.end_soft > 0.0 { u.end_soft } else { 1.0 });
    if along.abs() >= half_along {
        return 0.0;
    }
    let d = deform_at(u, deform, along);
    if d[1] <= 0.0 {
        return 0.0;
    }
    let across = (px * -u.dir[1] + py * u.dir[0]) - d[0];
    if across.abs() >= profile_cutoff_radius(u) {
        return 0.0;
    }
    let pr = profile(u, across);
    if pr <= 0.0 {
        return 0.0;
    }
    let em = end_mask(u, along);
    let mut l = u.intensity * d[1] * pr * em;
    if u.speckle > 0.0 && l > 0.0 {
        let inv_grain = 1.0 / u.grain.max(0.5);
        let n = value_noise(frag_x * inv_grain, frag_y * inv_grain, u.seed);
        let s = -(1.0 - n).max(1e-4).ln();
        l *= mix(1.0, s, u.speckle);
    }
    l
}

#[inline]
fn eval_terms(u: &Uniforms, deform: Option<&[[f32; 2]]>, frag_x: f32, frag_y: f32) -> (f32, f32) {
    let laser = eval_laser(u, deform, frag_x, frag_y);
    let noise = if u.noise > 0.0 {
        let ix = frag_x as i32;
        let iy = frag_y as i32;
        let r = hash(ix, iy, u.seed ^ 0x9e37_79b9) + hash(ix, iy, u.seed ^ 0x85eb_ca6b) - 1.0;
        r * 2.449 * u.noise
    } else {
        0.0
    };
    (laser, noise)
}

#[cfg(not(target_arch = "wasm32"))]
fn threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).min(32)
}

/// Splits `out` into row bands and runs `f(y0, slice)` in parallel on native targets when
/// the image is large enough to amortize thread dispatch.
fn for_each_row_band<T: Send>(out: &mut [T], width: usize, height: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    if width == 0 || height == 0 {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    if width * height >= 64 * 1024 {
        let n_threads = threads().min(height);
        let band_rows = height.div_ceil(n_threads).max(1);
        std::thread::scope(|scope| {
            for (band, chunk) in out[..width * height].chunks_mut(band_rows * width).enumerate() {
                let f = &f;
                scope.spawn(move || f(band * band_rows, chunk));
            }
        });
        return;
    }
    f(0, &mut out[..width * height]);
}

/// Renders `frame` into a row-major `width × height` buffer of `bits`-bit grey pixels
/// (`1..=16`, scaled to `0..=(1 << bits) - 1`).
///
/// Pixel `(x, y)` is evaluated at its centre `(x + 0.5, y + 0.5)`, matching `wgpu` fragment
/// coordinates. When `frame` was built with a non-zero view `origin` (e.g. a camera ROI),
/// pass the same origin to [`Emulator::frame`](crate::emulator::Emulator::frame) with a negative
/// sign, or use [`render_gray_region`] to offset fragment coordinates directly.
///
/// # Panics
/// Panics if `out.len() < width as usize * height as usize` or if `bits` is not in `1..=16`.
pub fn render_gray(frame: &Frame, width: u32, height: u32, out: &mut [u16], bits: u32) {
    render_gray_region(frame, [0, 0], width, height, out, bits);
}

/// Like [`render_gray`], but evaluates pixels starting at `(offset[0], offset[1])` in the
/// frame's coordinate space (for rendering a sensor ROI of `width × height` at `offset`).
///
/// # Panics
/// Panics if `out.len() < width as usize * height as usize` or if `bits` is not in `1..=16`.
pub fn render_gray_region(frame: &Frame, offset: [u32; 2], width: u32, height: u32, out: &mut [u16], bits: u32) {
    assert!((1..=16).contains(&bits), "bits must be in 1..=16 (got {bits})");
    let (w, h) = (width as usize, height as usize);
    assert!(out.len() >= w * h, "output buffer too small: {} < {}", out.len(), w * h);
    let scale = ((1u32 << bits) - 1) as f32;
    let u = &frame.uniforms;
    let deform = frame.deform.as_deref();
    let bounds = LineBounds::new(u, deform);
    let bg = (finish_gray(u, bounds.color_gain, 0.0, 0.0) * scale).round() as u16;
    let [ox, oy] = offset;

    for_each_row_band(out, w, h, |y0, chunk| {
        for (dy, row) in chunk.chunks_exact_mut(w).enumerate() {
            let iy = (oy + (y0 + dy) as u32) as i32;
            let frag_y = iy as f32 + 0.5;
            let (xs, xe) = bounds.row_x_span(u, frag_y, ox, ox + width);
            let s = (xs - ox) as usize;
            let e = (xe - ox) as usize;
            if u.noise <= 0.0 {
                row[..s].fill(bg);
                for (dx, px) in row[s..e].iter_mut().enumerate() {
                    let frag_x = (xs + dx as u32) as f32 + 0.5;
                    let l = eval_laser(u, deform, frag_x, frag_y);
                    *px = (finish_gray(u, bounds.color_gain, l, 0.0) * scale).round() as u16;
                }
                row[e..].fill(bg);
            } else {
                let y_hash_a = pcg((iy as u32) ^ pcg(u.seed ^ 0x9e37_79b9));
                let y_hash_b = pcg((iy as u32) ^ pcg(u.seed ^ 0x85eb_ca6b));
                for (dx, px) in row[..s].iter_mut().enumerate() {
                    let ix = (ox + dx as u32) as i32;
                    let n = eval_noise_row(u, ix, y_hash_a, y_hash_b);
                    *px = (finish_gray(u, bounds.color_gain, 0.0, n) * scale).round() as u16;
                }
                for (dx, px) in row[s..e].iter_mut().enumerate() {
                    let ix = (xs + dx as u32) as i32;
                    let frag_x = ix as f32 + 0.5;
                    let l = eval_laser(u, deform, frag_x, frag_y);
                    let n = eval_noise_row(u, ix, y_hash_a, y_hash_b);
                    *px = (finish_gray(u, bounds.color_gain, l, n) * scale).round() as u16;
                }
                for (dx, px) in row[e..].iter_mut().enumerate() {
                    let ix = (xe + dx as u32) as i32;
                    let n = eval_noise_row(u, ix, y_hash_a, y_hash_b);
                    *px = (finish_gray(u, bounds.color_gain, 0.0, n) * scale).round() as u16;
                }
            }
        }
    });
}

/// Renders `frame` into a row-major `width × height` buffer of 8-bit grey (`Mono8`) pixels.
///
/// # Panics
/// Panics if `out.len() < width as usize * height as usize`.
pub fn render_gray8(frame: &Frame, width: u32, height: u32, out: &mut [u8]) {
    render_gray8_region(frame, [0, 0], width, height, out);
}

/// Like [`render_gray8`], but evaluates pixels starting at `(offset[0], offset[1])` in the
/// frame's coordinate space.
///
/// # Panics
/// Panics if `out.len() < width as usize * height as usize`.
pub fn render_gray8_region(frame: &Frame, offset: [u32; 2], width: u32, height: u32, out: &mut [u8]) {
    let (w, h) = (width as usize, height as usize);
    assert!(out.len() >= w * h, "output buffer too small: {} < {}", out.len(), w * h);
    let u = &frame.uniforms;
    let deform = frame.deform.as_deref();
    let bounds = LineBounds::new(u, deform);
    let bg = (finish_gray(u, bounds.color_gain, 0.0, 0.0) * 255.0).round() as u8;
    let [ox, oy] = offset;

    for_each_row_band(out, w, h, |y0, chunk| {
        for (dy, row) in chunk.chunks_exact_mut(w).enumerate() {
            let iy = (oy + (y0 + dy) as u32) as i32;
            let frag_y = iy as f32 + 0.5;
            let (xs, xe) = bounds.row_x_span(u, frag_y, ox, ox + width);
            let s = (xs - ox) as usize;
            let e = (xe - ox) as usize;
            if u.noise <= 0.0 {
                row[..s].fill(bg);
                for (dx, px) in row[s..e].iter_mut().enumerate() {
                    let frag_x = (xs + dx as u32) as f32 + 0.5;
                    let l = eval_laser(u, deform, frag_x, frag_y);
                    *px = (finish_gray(u, bounds.color_gain, l, 0.0) * 255.0).round() as u8;
                }
                row[e..].fill(bg);
            } else {
                let y_hash_a = pcg((iy as u32) ^ pcg(u.seed ^ 0x9e37_79b9));
                let y_hash_b = pcg((iy as u32) ^ pcg(u.seed ^ 0x85eb_ca6b));
                for (dx, px) in row[..s].iter_mut().enumerate() {
                    let ix = (ox + dx as u32) as i32;
                    let n = eval_noise_row(u, ix, y_hash_a, y_hash_b);
                    *px = (finish_gray(u, bounds.color_gain, 0.0, n) * 255.0).round() as u8;
                }
                for (dx, px) in row[s..e].iter_mut().enumerate() {
                    let ix = (xs + dx as u32) as i32;
                    let frag_x = ix as f32 + 0.5;
                    let l = eval_laser(u, deform, frag_x, frag_y);
                    let n = eval_noise_row(u, ix, y_hash_a, y_hash_b);
                    *px = (finish_gray(u, bounds.color_gain, l, n) * 255.0).round() as u8;
                }
                for (dx, px) in row[e..].iter_mut().enumerate() {
                    let ix = (xe + dx as u32) as i32;
                    let n = eval_noise_row(u, ix, y_hash_a, y_hash_b);
                    *px = (finish_gray(u, bounds.color_gain, 0.0, n) * 255.0).round() as u8;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator::Emulator;
    use crate::params::{ColorMode, DeformKind, ProfileKind};
    use crate::presets;

    #[test]
    fn straight_line_peaks_at_center() {
        let mut emu = Emulator::default();
        emu.params.line.width_px = 3.0;
        emu.params.profile.kind = ProfileKind::Hard;
        emu.params.profile.end_softness_px = 0.0;
        let (w, h) = (128u32, 64u32);
        let frame = emu.frame([0.0, 0.0], [w as f32, h as f32], false);

        let mut out8 = vec![0u8; (w * h) as usize];
        render_gray8(&frame, w, h, &mut out8);
        assert_eq!(out8[(h / 2 * w + w / 2) as usize], 255);
        assert_eq!(out8[(5 * w + w / 2) as usize], 0);

        let mut out12 = vec![0u16; (w * h) as usize];
        render_gray(&frame, w, h, &mut out12, 12);
        assert_eq!(out12[(h / 2 * w + w / 2) as usize], 4095);
        assert_eq!(out12[(5 * w + w / 2) as usize], 0);

        let mut out16 = vec![0u16; (w * h) as usize];
        render_gray(&frame, w, h, &mut out16, 16);
        assert_eq!(out16[(h / 2 * w + w / 2) as usize], 65535);
    }

    #[test]
    fn wavelength_reduces_to_intensity() {
        let (w, h) = (64u32, 32u32);
        let mut emu = Emulator::default();
        emu.params.color.mode = ColorMode::Wavelength;
        emu.params.color.wavelength_nm = 660.0;
        let f_red = emu.frame([0.0, 0.0], [w as f32, h as f32], false);
        emu.params.color.wavelength_nm = 405.0;
        let f_blue = emu.frame([0.0, 0.0], [w as f32, h as f32], false);

        let (mut a, mut b) = (vec![0u16; (w * h) as usize], vec![0u16; (w * h) as usize]);
        render_gray(&f_red, w, h, &mut a, 16);
        render_gray(&f_blue, w, h, &mut b, 16);
        assert_eq!(a, b);
    }

    #[test]
    fn deterministic_for_given_state_and_seed() {
        let (w, h) = (256u32, 128u32);
        for (name, params) in presets::builtin() {
            let mut emu = Emulator::new(params);
            emu.tick(0.1);
            let f1 = emu.frame([0.0, 0.0], [w as f32, h as f32], false);
            let f2 = emu.frame([0.0, 0.0], [w as f32, h as f32], false);
            let (mut a, mut b) = (vec![0u16; (w * h) as usize], vec![0u16; (w * h) as usize]);
            render_gray(&f1, w, h, &mut a, 16);
            render_gray(&f2, w, h, &mut b, 16);
            assert_eq!(a, b, "preset {name}");
        }
    }

    #[test]
    fn region_matches_cropped_full_frame() {
        let mut emu = Emulator::default();
        emu.params.deform.kind = DeformKind::Sine;
        emu.params.noise.speckle_pct = 50.0;
        emu.params.noise.noise_pct = 2.0;
        let (w, h) = (128u32, 64u32);
        let frame = emu.frame([0.0, 0.0], [w as f32, h as f32], false);
        let mut full = vec![0u8; (w * h) as usize];
        render_gray8(&frame, w, h, &mut full);

        let (ox, oy, rw, rh) = (20u32, 10u32, 40u32, 24u32);
        let mut roi = vec![0u8; (rw * rh) as usize];
        render_gray8_region(&frame, [ox, oy], rw, rh, &mut roi);
        for y in 0..rh {
            for x in 0..rw {
                assert_eq!(roi[(y * rw + x) as usize], full[((oy + y) * w + (ox + x)) as usize]);
            }
        }
    }
}
