//! Slicing a mesh into laser profiles as a triangulation sensor would see them.
//!
//! Scan frame: `u` runs along the laser line, `v` along the travel direction
//! (one profile per `v` step) and `z` is the height. The laser plane is vertical
//! and hits the topmost surface, so the scan is a heightmap whose rows are the
//! profiles. The camera looks at the laser plane from ±v, tilted by the
//! triangulation angle from vertical; surface between it and the laser line can
//! hide parts of the profile (camera occlusion).
//!
//! Pure and GUI-free so it can be unit-tested. Natively the work is split over
//! threads in bands of rows; on wasm32 the bands run one after another.

use crate::mesh::{self, Mesh};
use crate::params::{EndMode, HeightGain, ScanParams, ScanScale};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// One profile maps onto the line's deformation buffer, so wider grids add nothing.
pub const MAX_COLS: usize = crate::DEFORM_SAMPLES;
/// Most profiles a slice may produce.
pub const MAX_ROWS: usize = 20_000;

/// Model axis and direction that points up (towards the sensor).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub enum UpAxis {
    /// +x is up.
    PosX,
    /// −x is up.
    NegX,
    /// +y is up.
    PosY,
    /// −y is up.
    NegY,
    /// +z is up.
    PosZ,
    /// −z is up.
    NegZ,
}

/// A model axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub enum Axis {
    /// The x axis.
    X,
    /// The y axis.
    Y,
    /// The z axis.
    Z,
}

/// Unit the model coordinates are given in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub enum ModelUnit {
    /// Millimetres.
    Mm,
    /// Centimetres.
    Cm,
    /// Metres.
    M,
    /// Inches.
    Inch,
}

impl ModelUnit {
    /// Millimetres per unit.
    pub fn mm(self) -> f32 {
        match self {
            ModelUnit::Mm => 1.0,
            ModelUnit::Cm => 10.0,
            ModelUnit::M => 1000.0,
            ModelUnit::Inch => 25.4,
        }
    }
}

/// Side of the laser plane the camera looks from, relative to the travel direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub enum CameraSide {
    /// Camera on the +v side (towards the part of the object not yet scanned).
    Ahead,
    /// Camera on the −v side (towards the part already scanned).
    Behind,
}

impl CameraSide {
    /// +1 for `Ahead`, −1 for `Behind`.
    pub fn sign(self) -> isize {
        match self {
            CameraSide::Ahead => 1,
            CameraSide::Behind => -1,
        }
    }
}

/// How a mesh is placed on the conveyor and sampled into a scan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(default)]
pub struct SliceSettings {
    /// Model axis pointing up (towards the sensor).
    pub up_axis: UpAxis,
    /// Model axis the object travels along; the remaining axis runs along the line.
    pub travel_axis: Axis,
    /// Mirror the object along the line.
    pub flip_along: bool,
    /// Rotation of the object about the up axis, in degrees.
    pub yaw_deg: f32,
    /// Unit of the model coordinates.
    pub unit: ModelUnit,
    /// Extra uniform scale on top of the unit conversion.
    pub scale: f32,
    /// Sample spacing along the line in mm.
    pub lateral_res_mm: f32,
    /// Spacing between profiles in mm.
    pub travel_step_mm: f32,
    /// Empty space left and right of the object, in mm.
    pub margin_lateral_mm: f32,
    /// Empty profiles before and after the object, in mm.
    pub margin_travel_mm: f32,
    /// Conveyor surface at z = 0: the line stays visible where there is no object.
    pub base_plane: bool,
    /// Hide profile parts the camera cannot see.
    pub occlusion: bool,
    /// Angle between camera axis and laser plane in degrees.
    pub triangulation_deg: f32,
    /// Side of the laser plane the camera looks from.
    pub camera_side: CameraSide,
}

impl Default for SliceSettings {
    fn default() -> Self {
        Self {
            up_axis: UpAxis::PosZ,
            travel_axis: Axis::Y,
            flip_along: false,
            yaw_deg: 0.0,
            unit: ModelUnit::Mm,
            scale: 1.0,
            lateral_res_mm: 0.2,
            travel_step_mm: 0.5,
            margin_lateral_mm: 10.0,
            margin_travel_mm: 10.0,
            base_plane: true,
            occlusion: true,
            triangulation_deg: 30.0,
            camera_side: CameraSide::Ahead,
        }
    }
}

impl SliceSettings {
    /// Travel axis index, never equal to the up axis (falls back to the next axis).
    pub fn travel_axis_index(&self) -> usize {
        let up = self.up_axis.index_sign().0;
        let t = self.travel_axis as usize;
        if t != up { t } else { (up + 1) % 3 }
    }
}

/// Grid layout of a scan; cell `(i, j)` is centered at `(u0 + i·du, v0 + j·dv)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct ScanMeta {
    /// File name of the model the scan was sliced from.
    pub source: String,
    /// Settings the scan was sliced with.
    pub settings: SliceSettings,
    /// Samples per profile.
    pub cols: usize,
    /// Number of profiles.
    pub rows: usize,
    /// Position of the first sample along the line in mm.
    pub u0_mm: f32,
    /// Sample spacing along the line in mm.
    pub du_mm: f32,
    /// Travel position of the first profile in mm.
    pub v0_mm: f32,
    /// Spacing between profiles in mm.
    pub dv_mm: f32,
    /// Highest surface point in mm.
    pub z_max_mm: f32,
}

/// A sliced scan: a `rows × cols` height grid, one profile per travel step.
#[derive(Clone, Debug, PartialEq)]
pub struct Scan {
    /// Grid layout and origin.
    pub meta: ScanMeta,
    /// Row-major heights in mm; NaN where the laser hits nothing.
    pub height: Vec<f32>,
    /// Row-major: 1 if the camera sees the laser at this cell.
    pub visible: Vec<u8>,
}

impl Scan {
    /// Heights of profile `j` in mm (NaN = nothing hit). Panics if `j >= rows`.
    pub fn row_height(&self, j: usize) -> &[f32] {
        let c = self.meta.cols;
        &self.height[j * c..(j + 1) * c]
    }

    /// Visibility of profile `j` (1 = seen by the camera). Panics if `j >= rows`.
    pub fn row_visible(&self, j: usize) -> &[u8] {
        let c = self.meta.cols;
        &self.visible[j * c..(j + 1) * c]
    }

    /// Height in mm of cell (`i` along the line, `j` = row) where the camera sees the
    /// laser, NaN otherwise.
    pub fn seen(&self, i: usize, j: usize) -> f32 {
        let k = j * self.meta.cols + i;
        if self.visible[k] != 0 { self.height[k] } else { f32::NAN }
    }

    /// Distance between the first and last sample along the line.
    pub fn width_mm(&self) -> f32 {
        (self.meta.cols.max(2) - 1) as f32 * self.meta.du_mm
    }

    /// Distance between the first and last profile.
    pub fn length_mm(&self) -> f32 {
        (self.meta.rows.max(2) - 1) as f32 * self.meta.dv_mm
    }

    /// Position of the grid center along the line in mm.
    pub fn u_center_mm(&self) -> f32 {
        self.meta.u0_mm + 0.5 * self.width_mm()
    }
    /// Bounds of the object: cell centres with a finite height above
    /// [`OBJECT_MIN_HEIGHT_MM`], in scan-frame mm. `None` when no cell rises that high
    /// (only the base plane, or nothing hit).
    pub fn object_bounds(&self) -> Option<ObjectBounds> {
        let m = &self.meta;
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for (k, &h) in self.height.iter().enumerate() {
            if !(h.is_finite() && h > OBJECT_MIN_HEIGHT_MM) {
                continue;
            }
            let p = [m.u0_mm + (k % m.cols) as f32 * m.du_mm, m.v0_mm + (k / m.cols) as f32 * m.dv_mm, h];
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        lo[0].is_finite().then_some(ObjectBounds { min: lo, max: hi })
    }
}

/// Heights up to this (mm) count as the base, not the object, in [`Scan::object_bounds`].
pub const OBJECT_MIN_HEIGHT_MM: f32 = 1e-3;

/// An axis-aligned box in scan-frame mm: (u along the line, v along the travel, z up).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct ObjectBounds {
    /// Smallest (u, v, z).
    pub min: [f32; 3],
    /// Largest (u, v, z).
    pub max: [f32; 3],
}

/// Grid dimensions `(cols, rows)` the settings would produce for an object with
/// scan-frame bounds `lo..hi`, or an explanation why they are unusable.
pub fn grid_size(lo: [f32; 3], hi: [f32; 3], s: &SliceSettings) -> Result<(usize, usize), String> {
    if !(s.lateral_res_mm > 0.0 && s.travel_step_mm > 0.0) {
        return Err("Resolution must be positive".into());
    }
    let width = hi[0] - lo[0] + 2.0 * s.margin_lateral_mm.max(0.0);
    let length = hi[1] - lo[1] + 2.0 * s.margin_travel_mm.max(0.0);
    let cols = (width / s.lateral_res_mm).floor() as usize + 1;
    let rows = (length / s.travel_step_mm).floor() as usize + 1;
    if cols > MAX_COLS {
        return Err(format!(
            "{cols} samples per line exceed the limit of {MAX_COLS}: increase the lateral resolution to ≥ {:.3} mm",
            width / (MAX_COLS - 1) as f32
        ));
    }
    if rows > MAX_ROWS {
        return Err(format!(
            "{rows} profiles exceed the limit of {MAX_ROWS}: increase the travel step to ≥ {:.3} mm",
            length / (MAX_ROWS - 1) as f32
        ));
    }
    Ok((cols, rows))
}

/// Progress of a running slice in 1/1000.
pub type Progress = AtomicU32;

/// Watches a running slice: reports progress and can stop it. Called from the slicing
/// threads, so it must be `Sync`.
pub trait SliceObserver: Sync {
    /// Checked regularly; returning true stops the slice with `Err("cancelled")`.
    fn cancelled(&self) -> bool;
    /// Progress in 1/1000.
    fn progress(&self, permille: u32);
}

/// Cancel flag and progress counter, for slices running on another thread.
struct Atomics<'a>(&'a AtomicBool, &'a Progress);

impl SliceObserver for Atomics<'_> {
    fn cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn progress(&self, permille: u32) {
        self.1.store(permille, Ordering::Relaxed);
    }
}

/// Slices `mesh` into a scan. Returns `Err("cancelled")` when `cancel` is set.
pub fn slice(mesh: &Mesh, s: &SliceSettings, source: &str, cancel: &AtomicBool, progress: &Progress) -> Result<Scan, String> {
    slice_observed(mesh, s, source, &Atomics(cancel, progress))
}

/// [`slice()`] with progress and cancellation going through `observer`.
pub fn slice_observed(mesh: &Mesh, s: &SliceSettings, source: &str, observer: &dyn SliceObserver) -> Result<Scan, String> {
    let pts = mesh::to_scan_frame(mesh, s);
    let (lo, hi) = mesh::bounds(&pts);
    let (cols, rows) = grid_size(lo, hi, s)?;
    let meta = ScanMeta {
        source: source.to_owned(),
        settings: s.clone(),
        cols,
        rows,
        u0_mm: lo[0] - s.margin_lateral_mm.max(0.0),
        du_mm: s.lateral_res_mm,
        v0_mm: lo[1] - s.margin_travel_mm.max(0.0),
        dv_mm: s.travel_step_mm,
        z_max_mm: 0.0,
    };

    let mut height = rasterize(&pts, &mesh.triangles, &meta, observer)?;
    let mut z_max = 0.0f32;
    for h in &mut height {
        if *h == f32::NEG_INFINITY {
            *h = if s.base_plane { 0.0 } else { f32::NAN };
        } else {
            z_max = z_max.max(*h);
        }
    }
    let meta = ScanMeta { z_max_mm: z_max, ..meta };

    let visible = if s.occlusion && s.triangulation_deg > 0.1 {
        occlusion(&height, &meta, observer)?
    } else {
        height.iter().map(|h| h.is_finite() as u8).collect()
    };
    observer.progress(1000);
    Ok(Scan { meta, height, visible })
}

#[cfg(not(target_arch = "wasm32"))]
fn threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).min(32)
}

#[cfg(target_arch = "wasm32")]
fn threads() -> usize {
    1
}

/// Runs `f(band, chunk)` for each `band_len`-sized chunk of `data`, in parallel where
/// threads are available.
fn for_each_band<T: Send>(data: &mut [T], band_len: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::scope(|scope| {
        for (band, chunk) in data.chunks_mut(band_len).enumerate() {
            let f = &f;
            scope.spawn(move || f(band, chunk));
        }
    });
    #[cfg(target_arch = "wasm32")]
    for (band, chunk) in data.chunks_mut(band_len).enumerate() {
        f(band, chunk);
    }
}

/// Topmost surface per grid cell (z-buffer max); `-inf` where nothing is hit.
fn rasterize(pts: &[[f32; 3]], tris: &[[u32; 3]], m: &ScanMeta, observer: &dyn SliceObserver) -> Result<Vec<f32>, String> {
    let mut height = vec![f32::NEG_INFINITY; m.cols * m.rows];
    let n_threads = threads();
    let band_rows = m.rows.div_ceil(n_threads).max(1);
    let done = AtomicU32::new(0);
    let total = (tris.len() * m.rows.div_ceil(band_rows)).max(1) as f64;

    for_each_band(&mut height, band_rows * m.cols, |band, chunk| {
        let j0 = band * band_rows;
        let j1 = j0 + chunk.len() / m.cols; // exclusive
        for (n, t) in tris.iter().enumerate() {
            if n % 4096 == 0 {
                if observer.cancelled() {
                    return;
                }
                let d = done.fetch_add(4096, Ordering::Relaxed) as f64;
                observer.progress((d / total * 900.0).min(900.0) as u32);
            }
            let tri = t.map(|i| pts[i as usize]);
            raster_triangle(&tri, m, j0, j1, chunk);
        }
    });
    if observer.cancelled() {
        return Err("cancelled".into());
    }
    Ok(height)
}

/// Writes the max height of `tri` into the cells of rows `j0..j1` (`band` holds just those rows).
fn raster_triangle(tri: &[[f32; 3]; 3], m: &ScanMeta, j0: usize, j1: usize, band: &mut [f32]) {
    let [a, b, c] = *tri;
    // Plane z = a.z - (n.x (u - a.u) + n.y (v - a.v)) / n.z
    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
    let scale = n[0].abs() + n[1].abs() + n[2].abs();
    if n[2].abs() <= 1e-6 * scale || scale == 0.0 {
        return; // vertical in the top view: covered by its neighbours
    }
    let z_lo = a[2].min(b[2]).min(c[2]);
    let z_hi = a[2].max(b[2]).max(c[2]);

    let v_lo = a[1].min(b[1]).min(c[1]);
    let v_hi = a[1].max(b[1]).max(c[1]);
    let first = ((v_lo - m.v0_mm) / m.dv_mm).ceil().max(j0 as f32) as usize;
    let last = ((v_hi - m.v0_mm) / m.dv_mm).floor();
    if last < first as f32 {
        return;
    }
    let last = (last as usize).min(j1.saturating_sub(1));

    for j in first..=last {
        let v = m.v0_mm + j as f32 * m.dv_mm;
        // Span of the triangle on the horizontal line through v.
        let (mut u_min, mut u_max) = (f32::INFINITY, f32::NEG_INFINITY);
        for (p, q) in [(a, b), (b, c), (c, a)] {
            if (p[1] - v) * (q[1] - v) > 0.0 {
                continue;
            }
            if p[1] == q[1] {
                u_min = u_min.min(p[0]).min(q[0]);
                u_max = u_max.max(p[0]).max(q[0]);
            } else {
                let u = p[0] + (v - p[1]) * (q[0] - p[0]) / (q[1] - p[1]);
                u_min = u_min.min(u);
                u_max = u_max.max(u);
            }
        }
        if u_min > u_max {
            continue;
        }
        let i0 = ((u_min - m.u0_mm) / m.du_mm).ceil().max(0.0) as usize;
        let i1 = ((u_max - m.u0_mm) / m.du_mm).floor();
        if i1 < 0.0 {
            continue;
        }
        let i1 = (i1 as usize).min(m.cols - 1);
        let row = &mut band[(j - j0) * m.cols..(j - j0 + 1) * m.cols];
        for (i, cell) in row.iter_mut().enumerate().take(i1 + 1).skip(i0) {
            let u = m.u0_mm + i as f32 * m.du_mm;
            let z = (a[2] - (n[0] * (u - a[0]) + n[1] * (v - a[1])) / n[2]).clamp(z_lo, z_hi);
            if z > *cell {
                *cell = z;
            }
        }
    }
}

/// Visibility of each cell from the camera: the ray from the surface point towards
/// the camera rises by `dv·cot α` per row and must stay above the heightmap.
fn occlusion(height: &[f32], m: &ScanMeta, observer: &dyn SliceObserver) -> Result<Vec<u8>, String> {
    let s = &m.settings;
    let rise = m.dv_mm / s.triangulation_deg.clamp(0.1, 89.9).to_radians().tan();
    let dir = s.camera_side.sign();
    let eps = 1e-3f32;
    let mut visible = vec![0u8; height.len()];
    let band_rows = m.rows.div_ceil(threads()).max(1);

    for_each_band(&mut visible, band_rows * m.cols, |band, chunk| {
        for (k, out) in chunk.iter_mut().enumerate() {
            if k % m.cols == 0 && observer.cancelled() {
                return;
            }
            let (i, j) = (k % m.cols, band * band_rows + k / m.cols);
            let h = height[j * m.cols + i];
            if !h.is_finite() {
                continue;
            }
            let mut seen = true;
            let mut step = 1;
            loop {
                let ray = h + step as f32 * rise;
                if ray > m.z_max_mm {
                    break;
                }
                let jj = j as isize + dir * step as isize;
                if jj < 0 || jj >= m.rows as isize {
                    break;
                }
                if height[jj as usize * m.cols + i] > ray + eps {
                    seen = false;
                    break;
                }
                step += 1;
            }
            *out = seen as u8;
        }
    });
    if observer.cancelled() {
        return Err("cancelled".into());
    }
    Ok(visible)
}

/// How scan millimetres land on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct DisplayMapping {
    /// Screen pixels per mm along the line.
    pub px_per_mm: f32,
    /// Line displacement in pixels per mm of height.
    pub height_px_per_mm: f32,
    /// Higher surfaces move the line down instead of up.
    pub invert: bool,
}

/// Screen mapping for `scan` and the line length to draw, given the configured
/// line length (used by `FitLine`, replaced in `Fixed` mode).
pub fn display_mapping(scan: &Scan, p: &ScanParams, line_length_px: f32) -> (DisplayMapping, f32) {
    let (px_per_mm, length_px) = match p.scale {
        ScanScale::FitLine => (line_length_px / scan.width_mm().max(1e-6), line_length_px),
        ScanScale::Fixed => {
            let k = p.px_per_mm.max(1e-6);
            (k, scan.width_mm() * k)
        }
    };
    let height_px_per_mm = match p.height_gain {
        HeightGain::MatchScale => px_per_mm,
        HeightGain::FromAngle => px_per_mm * scan.meta.settings.triangulation_deg.to_radians().sin(),
        HeightGain::Manual => p.height_px_per_mm,
    };
    (DisplayMapping { px_per_mm, height_px_per_mm, invert: p.invert_height }, length_px)
}

/// Fills `out[k]` with `(offset_px, intensity)` for the line sample at
/// `start_px + k·step_px` (relative to the line center) from profile `row`.
///
/// Heights are interpolated between visible neighbours only, and the offset holds its
/// last value across gaps so the line never ramps into invisible regions.
pub fn fill_deform(scan: &Scan, row: usize, map: &DisplayMapping, start_px: f32, step_px: f32, out: &mut [[f32; 2]]) {
    let m = &scan.meta;
    let heights = scan.row_height(row);
    let vis = scan.row_visible(row);
    let valid = |i: usize| vis[i] != 0 && heights[i].is_finite();
    let gain = map.height_px_per_mm * if map.invert { 1.0 } else { -1.0 };
    let last_col = (m.cols - 1) as f32;
    let center = scan.u_center_mm();
    let px_per_mm = map.px_per_mm.max(1e-6);

    let mut held = 0.0;
    for (k, o) in out.iter_mut().enumerate() {
        let u_mm = center + (start_px + k as f32 * step_px) / px_per_mm;
        let f = ((u_mm - m.u0_mm) / m.du_mm).clamp(0.0, last_col);
        let i0 = f.floor() as usize;
        let i1 = (i0 + 1).min(m.cols - 1);
        let t = f - i0 as f32;
        let (a, b) = (valid(i0), valid(i1));
        let h = match (a, b) {
            (true, true) => Some(heights[i0] + (heights[i1] - heights[i0]) * t),
            (true, false) => Some(heights[i0]),
            (false, true) => Some(heights[i1]),
            (false, false) => None,
        };
        if let Some(h) = h {
            held = h * gain;
        }
        let intensity = a as u8 as f32 * (1.0 - t) + b as u8 as f32 * t;
        *o = [held, intensity];
    }
}

/// Profile shown for the playback position `pos` (a count of profiles since the start).
pub fn row_index(pos: f64, rows: usize, end: EndMode) -> usize {
    if rows <= 1 {
        return 0;
    }
    let p = pos.max(0.0).floor() as usize;
    match end {
        EndMode::Loop => p % rows,
        EndMode::Once => p.min(rows - 1),
        EndMode::Bounce => {
            let period = 2 * (rows - 1);
            let q = p % period;
            if q < rows { q } else { period - q }
        }
    }
}

/// Playback position after stepping `delta` profiles from the one currently shown.
pub fn step(pos: f64, rows: usize, end: EndMode, delta: isize) -> f64 {
    let r = row_index(pos, rows, end) as isize + delta;
    r.clamp(0, rows.saturating_sub(1) as isize) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::box_mesh;

    fn run(mesh: &Mesh, s: &SliceSettings) -> Scan {
        slice(mesh, s, "test", &AtomicBool::new(false), &Progress::new(0)).unwrap()
    }

    fn flat(s: SliceSettings) -> SliceSettings {
        SliceSettings { lateral_res_mm: 1.0, travel_step_mm: 1.0, margin_lateral_mm: 5.0, margin_travel_mm: 5.0, occlusion: false, ..s }
    }

    #[test]
    fn box_heights() {
        // 20 wide, 10 long, 8 high.
        let m = box_mesh([0.0; 3], [20.0, 10.0, 8.0]);
        let scan = run(&m, &flat(SliceSettings::default()));
        assert_eq!((scan.meta.cols, scan.meta.rows), (31, 21));
        assert_eq!(scan.meta.z_max_mm, 8.0);
        // Grid starts 5 mm before the object in both directions.
        assert_eq!(scan.seen(15, 10), 8.0);
        assert_eq!(scan.seen(5, 5), 8.0);
        assert_eq!(scan.seen(25, 15), 8.0);
        assert_eq!(scan.seen(4, 10), 0.0, "base plane outside");
        assert_eq!(scan.seen(15, 16), 0.0);

        let scan = run(&m, &flat(SliceSettings { base_plane: false, ..Default::default() }));
        assert!(scan.seen(4, 10).is_nan());
        assert_eq!(scan.seen(15, 10), 8.0);
    }

    #[test]
    fn ramp_interpolates() {
        // Two triangles rising from z = 0 at v = 0 to z = 10 at v = 10.
        let mesh = Mesh {
            vertices: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 10.0], [0.0, 10.0, 10.0]],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            unit: None,
        };
        let s = SliceSettings { margin_lateral_mm: 0.0, margin_travel_mm: 0.0, ..flat(SliceSettings::default()) };
        let scan = run(&mesh, &s);
        for j in 0..=10 {
            assert!((scan.seen(5, j) - j as f32).abs() < 1e-4, "row {j}");
        }
    }

    #[test]
    fn camera_occlusion() {
        // 10 mm high block starting at v = 20, camera looking from behind (−v) at 45°.
        let m = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let s = SliceSettings {
            occlusion: true,
            triangulation_deg: 45.0,
            camera_side: CameraSide::Ahead,
            margin_travel_mm: 20.0,
            ..flat(SliceSettings::default())
        };
        let scan = run(&m, &s);
        let i = 10;
        // Block occupies rows 20..=30. The camera ahead (+v) cannot see the floor
        // closer than h·tan 45° = 10 mm in front of the block.
        assert!(scan.seen(i, 9).is_finite(), "11 mm away: visible");
        assert!(scan.seen(i, 10).is_finite(), "10 mm away: grazing, visible");
        assert!(scan.seen(i, 11).is_nan(), "9 mm away: hidden");
        assert!(scan.seen(i, 19).is_nan());
        assert_eq!(scan.seen(i, 25), 10.0, "top visible");
        assert_eq!(scan.seen(i, 35), 0.0, "floor behind the block faces the camera");

        let scan = run(&m, &SliceSettings { camera_side: CameraSide::Behind, ..s });
        assert_eq!(scan.seen(i, 15), 0.0);
        assert!(scan.seen(i, 35).is_nan());
    }

    #[test]
    fn limits_and_cancel() {
        let m = box_mesh([0.0; 3], [10_000.0, 10.0, 1.0]);
        let s = flat(SliceSettings::default());
        assert!(slice(&m, &s, "", &AtomicBool::new(false), &Progress::new(0)).unwrap_err().contains("samples per line"));
        let m = box_mesh([0.0; 3], [10.0, 10.0, 1.0]);
        assert_eq!(slice(&m, &s, "", &AtomicBool::new(true), &Progress::new(0)).unwrap_err(), "cancelled");
    }

    fn row_scan(heights: &[f32], visible: &[u8]) -> Scan {
        let cols = heights.len();
        Scan {
            meta: ScanMeta {
                source: String::new(),
                settings: SliceSettings::default(),
                cols,
                rows: 1,
                u0_mm: 0.0,
                du_mm: 1.0,
                v0_mm: 0.0,
                dv_mm: 1.0,
                z_max_mm: 0.0,
            },
            height: heights.to_vec(),
            visible: visible.to_vec(),
        }
    }

    #[test]
    fn deform_mapping() {
        // Columns at u = 0..4 mm, center 2 mm.
        let scan = row_scan(&[0.0, 1.0, 2.0, 5.0, 5.0], &[1, 1, 1, 0, 1]);
        let map = DisplayMapping { px_per_mm: 10.0, height_px_per_mm: 4.0, invert: false };
        let mut out = [[0.0f32; 2]; 9];
        fill_deform(&scan, 0, &map, -20.0, 5.0, &mut out); // u = 0, 0.5, … 4 mm
        assert_eq!(out[0], [0.0, 1.0]);
        assert_eq!(out[1], [-2.0, 1.0], "interpolated, higher = up (negative)");
        assert_eq!(out[4], [-8.0, 1.0]);
        // Column 3 is hidden: intensity fades, offset holds instead of ramping to it.
        assert_eq!(out[5], [-8.0, 0.5]);
        assert_eq!(out[6], [-20.0, 0.0], "may jump where the line is invisible");
        assert_eq!(out[7], [-20.0, 0.5]);
        assert_eq!(out[8], [-20.0, 1.0]);

        let map = DisplayMapping { invert: true, ..map };
        fill_deform(&scan, 0, &map, -20.0, 5.0, &mut out);
        assert_eq!(out[4], [8.0, 1.0]);
    }

    #[test]
    fn mapping_modes() {
        let mut scan = row_scan(&[0.0; 101], &[1; 101]); // 100 mm wide
        scan.meta.settings.triangulation_deg = 30.0;
        let p = ScanParams::default();
        let (m, len) = display_mapping(&scan, &p, 500.0);
        assert_eq!((m.px_per_mm, len), (5.0, 500.0));
        assert_eq!(m.height_px_per_mm, 5.0, "defaults to true scale");

        let p = ScanParams { height_gain: HeightGain::FromAngle, ..ScanParams::default() };
        assert!((display_mapping(&scan, &p, 500.0).0.height_px_per_mm - 2.5).abs() < 1e-5);
        // A scan sliced at 0° (no occlusion) keeps a visible profile by default…
        scan.meta.settings.triangulation_deg = 0.0;
        assert_eq!(display_mapping(&scan, &ScanParams::default(), 500.0).0.height_px_per_mm, 5.0);
        // …but is flat when the gain follows the angle.
        assert_eq!(display_mapping(&scan, &p, 500.0).0.height_px_per_mm, 0.0);

        let p = ScanParams { scale: ScanScale::Fixed, px_per_mm: 2.0, height_gain: HeightGain::Manual, height_px_per_mm: 7.0, ..ScanParams::default() };
        let (m, len) = display_mapping(&scan, &p, 500.0);
        assert_eq!((m.px_per_mm, len, m.height_px_per_mm), (2.0, 200.0, 7.0));
    }

    #[test]
    fn object_bounds_of_a_box() {
        // 20 wide, 10 long, 8 high; 5 mm margins, 1 mm grid, centred in u.
        let m = box_mesh([0.0; 3], [20.0, 10.0, 8.0]);
        let scan = run(&m, &flat(SliceSettings::default()));
        let b = scan.object_bounds().unwrap();
        assert_eq!(b.min, [-10.0, 0.0, 8.0]);
        assert_eq!(b.max, [10.0, 10.0, 8.0]);
        // Without the base plane the empty cells are NaN: the same box.
        let scan = run(&m, &flat(SliceSettings { base_plane: false, ..Default::default() }));
        assert_eq!(scan.object_bounds(), Some(b));
        // A flat scan has no object.
        assert_eq!(row_scan(&[0.0, 0.0005, f32::NAN], &[1, 1, 1]).object_bounds(), None);
    }

    #[test]
    fn playback_indexing() {
        let loop_: Vec<_> = (0..7).map(|p| row_index(p as f64, 3, EndMode::Loop)).collect();
        assert_eq!(loop_, [0, 1, 2, 0, 1, 2, 0]);
        let bounce: Vec<_> = (0..7).map(|p| row_index(p as f64, 3, EndMode::Bounce)).collect();
        assert_eq!(bounce, [0, 1, 2, 1, 0, 1, 2]);
        let once: Vec<_> = (0..5).map(|p| row_index(p as f64 + 0.7, 3, EndMode::Once)).collect();
        assert_eq!(once, [0, 1, 2, 2, 2]);
        assert_eq!(row_index(5.0, 1, EndMode::Bounce), 0);

        assert_eq!(step(4.0, 3, EndMode::Bounce, 1), 1.0);
        assert_eq!(step(0.0, 3, EndMode::Loop, -1), 0.0);
        assert_eq!(step(2.0, 3, EndMode::Loop, 1), 2.0);
    }
}
