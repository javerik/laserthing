//! Support for calibrating a camera that films the emulated line.
//!
//! - [`line_envelope`]: the area the line can cover over its whole motion, deformation
//!   and scan playback, so a camera region can be chosen that always holds it.
//! - [`calibration_params`] and [`pattern_params`]: a clean, static line (or nothing) at
//!   known positions, for measuring how view pixels map onto the camera's sensor.
//! - [`fit_into`]: shrinks and recentres the line so its envelope lies inside the part
//!   of the view a camera sees.
//! - [`constrain_into`]: changes the line as little as needed to keep its envelope inside an
//!   area, for a controller that limits where the line may go.
//! - [`fit_heights`]: moves the line across and lowers its heights (never its length or
//!   travel) until its envelope lies inside a band, e.g. the rows a camera's region sees.
//!
//! All coordinates are view pixels (the canvas' drawing buffer, y down).

use crate::model;
use crate::params::{
    ColorMode, DeformKind, DeformParams, HeightGain, LaserParams, LengthUnit, MotionKind,
    MotionParams, Orientation, ProfileKind, ScanScale,
};
use crate::scan::{self, Scan};
use serde::{Deserialize, Serialize};

/// Narrowest line drawn by [`calibration_params`], in px.
pub const MIN_CALIBRATION_WIDTH_PX: f32 = 4.0;

/// The area a line can cover: a rectangle in the line's own frame (along its direction,
/// across it), anchored at its resting center.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct Envelope {
    /// Corners in view px, in order: (along min, across min), (along max, across min),
    /// (along max, across max), (along min, across max).
    pub corners: [[f32; 2]; 4],
    /// Line center without motion, in view px.
    pub center_px: [f32; 2],
    /// Unit vector along the line.
    pub dir: [f32; 2],
    /// Unit vector across the line (the direction motion and deformation move it in).
    pub normal: [f32; 2],
    /// Extent along `dir` relative to `center_px`, including the soft end caps.
    pub along_px: [f32; 2],
    /// Extent along `normal` relative to `center_px`, including half the line width.
    pub across_px: [f32; 2],
}

impl Envelope {
    /// Whether every corner lies inside the convex `polygon`, at least `margin` px from
    /// its edges. The polygon may wind either way.
    pub fn inside(&self, polygon: &[[f32; 2]], margin: f32) -> bool {
        self.corners.iter().all(|&p| inset_distance(polygon, p) >= margin)
    }
}

/// The envelope of the line drawn with `params` in a view of `view_px`. `scan` is the
/// loaded scan; it shapes the line only while `params.scan.enabled`.
pub fn line_envelope(params: &LaserParams, view_px: [f32; 2], scan: Option<&Scan>) -> Envelope {
    let p = params;
    let angle = model::line_angle(&p.line);
    let dir = [angle.cos(), angle.sin()];
    let normal = [-dir[1], dir[0]];

    let mut length = model::line_length_px(&p.line, view_px);
    let active_scan = scan.filter(|_| p.scan.enabled);
    let shape = match active_scan {
        Some(s) => {
            let (mapping, l) = scan::display_mapping(s, &p.scan, length);
            length = l;
            scan_range(s, mapping.height_px_per_mm * if mapping.invert { 1.0 } else { -1.0 })
        }
        None => deform_range(&p.deform),
    };
    let motion = motion_range(&p.motion, model::extent_along(view_px, normal));
    let half_width = 0.5 * p.line.width_px.max(0.0);
    let across = [motion[0] + shape[0] - half_width, motion[1] + shape[1] + half_width];
    let half_length = 0.5 * (length + p.profile.end_softness_px.max(0.0));
    let along = [-half_length, half_length];

    let center = [p.line.position_pct[0] / 100.0 * view_px[0], p.line.position_pct[1] / 100.0 * view_px[1]];
    let at = |a: f32, c: f32| [center[0] + dir[0] * a + normal[0] * c, center[1] + dir[1] * a + normal[1] * c];
    Envelope {
        corners: [at(along[0], across[0]), at(along[1], across[0]), at(along[1], across[1]), at(along[0], across[1])],
        center_px: center,
        dir,
        normal,
        along_px: along,
        across_px: across,
    }
}

/// Range of [`model::motion_offset`] over all times.
fn motion_range(m: &MotionParams, extent_px: f32) -> [f32; 2] {
    match m.kind {
        MotionKind::Static => [0.0, 0.0],
        MotionKind::Sweep => {
            let half = 0.5 * (extent_px * m.span_pct / 100.0).max(0.0);
            [-half, half]
        }
        MotionKind::Sine => [-m.amplitude_px.abs(), m.amplitude_px.abs()],
    }
}

/// Range of [`model::deform_offset`] over all positions and times. Conservative: a
/// feature scrolling along the line is assumed to pass every point of it.
fn deform_range(d: &DeformParams) -> [f32; 2] {
    match d.kind {
        DeformKind::None => [0.0, 0.0],
        DeformKind::Step | DeformKind::Bump | DeformKind::Box => [d.height_px.min(0.0), d.height_px.max(0.0)],
        DeformKind::Sine => [-d.amplitude_px.abs(), d.amplitude_px.abs()],
    }
}

/// Range of the line displacement a scan produces: every seen height times `gain` (px per
/// mm, signed), and 0 where a profile starts without a seen height.
fn scan_range(s: &Scan, gain: f32) -> [f32; 2] {
    let mut range = [0.0f32, 0.0f32];
    for (h, v) in s.height.iter().zip(&s.visible) {
        if *v != 0 && h.is_finite() {
            let d = h * gain;
            range = [range[0].min(d), range[1].max(d)];
        }
    }
    range
}

/// What [`pattern_params`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Pattern {
    /// Nothing: a black view, for the camera's background.
    Blank,
    /// A line (or, with `width_pct`, a band) across the whole view.
    Line {
        /// `Horizontal` lies along x at `position_pct` of the height; `Vertical` along y
        /// at `position_pct` of the width.
        orientation: Orientation,
        /// Position of its centre across the line, in percent of the view.
        position_pct: f32,
        /// Width in percent of the view across the line, for a band (100 at 50 % lights
        /// the whole view); the calibration line's width when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width_pct: Option<f32>,
        /// Rotation in degrees on top of the orientation, about the line's centre (as
        /// [`LineParams::tilt_deg`](crate::params::LineParams)); none when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tilt_deg: Option<f32>,
    },
}

/// `params` turned into a clean calibration line: static, undeformed, no scan or noise,
/// a hard profile without end caps, white at full intensity and at least
/// [`MIN_CALIBRATION_WIDTH_PX`] wide. White is the brightest a display shows, and holds
/// every laser colour a camera's filter may pass.
pub fn calibration_params(params: &LaserParams) -> LaserParams {
    let mut p = params.clone();
    p.line.intensity_pct = 100.0;
    p.line.width_px = p.line.width_px.max(MIN_CALIBRATION_WIDTH_PX);
    p.line.tilt_deg = 0.0;
    p.profile.kind = ProfileKind::Hard;
    p.profile.end_softness_px = 0.0;
    p.motion.kind = MotionKind::Static;
    p.deform.kind = DeformKind::None;
    p.scan.enabled = false;
    p.noise.ambient_pct = 0.0;
    p.noise.speckle_pct = 0.0;
    p.noise.noise_pct = 0.0;
    p.color.mode = ColorMode::Custom;
    p.color.custom_rgb = [1.0, 1.0, 1.0];
    p
}

/// `base` (see [`calibration_params`]) showing `pattern` in a view of `view_px`.
pub fn pattern_params(base: &LaserParams, pattern: Pattern, view_px: [f32; 2]) -> LaserParams {
    let mut p = base.clone();
    match pattern {
        Pattern::Blank => p.line.intensity_pct = 0.0,
        Pattern::Line { orientation, position_pct, width_pct, tilt_deg } => {
            if let Some(w) = width_pct {
                let across = match orientation {
                    Orientation::Horizontal => view_px[1],
                    Orientation::Vertical => view_px[0],
                };
                p.line.width_px = (w / 100.0 * across).max(1.0);
            }
            p.line.orientation = orientation;
            p.line.tilt_deg = tilt_deg.unwrap_or(0.0);
            p.line.length = 100.0;
            p.line.length_unit = LengthUnit::Percent;
            p.line.position_pct = match orientation {
                Orientation::Horizontal => [50.0, position_pct],
                Orientation::Vertical => [position_pct, 50.0],
            };
        }
    }
    p
}

/// Result of [`fit_into`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct Fit {
    /// The adjusted parameters.
    pub params: LaserParams,
    /// The envelope now lies inside the field. False when the deformation or scan heights
    /// alone are too large for it, or the field is empty.
    pub fits: bool,
}

/// `params` with the line moved to the middle of the convex `field` (view px) and, if
/// needed, its length and motion range shrunk so the whole envelope stays `margin` px
/// inside it. Nothing grows; deformation and scan heights are left alone.
pub fn fit_into(params: &LaserParams, field: &[[f32; 2]], view_px: [f32; 2], margin: f32, scan: Option<&Scan>) -> Fit {
    if field.len() < 3 || view_px[0] <= 0.0 || view_px[1] <= 0.0 {
        return Fit { params: params.clone(), fits: false };
    }
    let target = centroid(field);
    let at_scale = |s: f32| {
        let p = centered(&scaled(params, s), target, view_px, scan);
        let fits = line_envelope(&p, view_px, scan).inside(field, margin);
        (p, fits)
    };
    let (p, fits) = at_scale(1.0);
    if fits {
        return Fit { params: p, fits: true };
    }
    // The envelope shrinks monotonically with the scale: bisect for the largest that fits.
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    if !at_scale(0.0).1 {
        return Fit { params: at_scale(0.0).0, fits: false };
    }
    for _ in 0..30 {
        let mid = 0.5 * (lo + hi);
        if at_scale(mid).1 { lo = mid } else { hi = mid }
    }
    Fit { params: at_scale(lo).0, fits: true }
}

/// `p` with the line placed so its envelope's middle (not its resting center) is on `target`.
fn centered(p: &LaserParams, target: [f32; 2], view_px: [f32; 2], scan: Option<&Scan>) -> LaserParams {
    let mut p = p.clone();
    let e = line_envelope(&p, view_px, scan);
    let mid_along = 0.5 * (e.along_px[0] + e.along_px[1]);
    let mid_across = 0.5 * (e.across_px[0] + e.across_px[1]);
    let center = [
        target[0] - e.dir[0] * mid_along - e.normal[0] * mid_across,
        target[1] - e.dir[1] * mid_along - e.normal[1] * mid_across,
    ];
    p.line.position_pct = [center[0] / view_px[0] * 100.0, center[1] / view_px[1] * 100.0];
    p
}

/// Result of [`constrain_into`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct Constrained {
    /// The adjusted parameters; the given ones when the line already stays inside.
    pub params: LaserParams,
    /// The envelope now lies inside the area. False only when the line's width alone is
    /// too large for it, or the area is empty.
    pub fits: bool,
    /// What had to change, in words: `position`, `length`, `movement`, `deformation`,
    /// `scan scale`, `scan height`. Empty when nothing did.
    pub clamped: Vec<String>,
}

/// `params` changed as little as needed so the line's whole envelope stays inside the
/// convex `area` (view px):
///
/// 1. unchanged when it already does;
/// 2. else the line keeps its size if it fits in the area's middle, and moves from where it
///    is towards there only as far as needed;
/// 3. else its length and motion shrink first, then (if even no length and motion don't fit)
///    its deformation and scan heights with them, to the largest size that fits; it then
///    moves as in 2.
///
/// Meant to run on every parameter change while a controller limits the line, so a
/// control pushed past a limit stops at it.
pub fn constrain_into(params: &LaserParams, area: &[[f32; 2]], view_px: [f32; 2], scan: Option<&Scan>) -> Constrained {
    let unchanged = |fits| Constrained { params: params.clone(), fits, clamped: Vec::new() };
    if area.len() < 3 || view_px[0] <= 0.0 || view_px[1] <= 0.0 {
        return unchanged(false);
    }
    let fits = |p: &LaserParams| line_envelope(p, view_px, scan).inside(area, 0.0);
    if fits(params) {
        return unchanged(true);
    }
    let target = centroid(area);
    let fits_centered = |p: &LaserParams| fits(&centered(p, target, view_px, scan));

    // The largest size that fits in the middle: the envelope shrinks monotonically with each scale.
    let largest = |make: &dyn Fn(f32) -> LaserParams| {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            if fits_centered(&make(mid)) { lo = mid } else { hi = mid }
        }
        make(lo)
    };
    let (sized, all_fit) = if fits_centered(params) {
        (params.clone(), true)
    } else if fits_centered(&scaled(params, 0.0)) {
        (largest(&|s| scaled(params, s)), true)
    } else if fits_centered(&scaled_shape(&scaled(params, 0.0), 0.0)) {
        (largest(&|s| scaled_shape(&scaled(params, s), s)), true)
    } else {
        (scaled_shape(&scaled(params, 0.0), 0.0), false)
    };

    // From where the line is towards the middle: the positions that fit form an interval
    // ending at the middle (the area is convex), so bisect for its start.
    let from = sized.line.position_pct;
    let to = centered(&sized, target, view_px, scan).line.position_pct;
    let at = |t: f32| {
        let mut p = sized.clone();
        p.line.position_pct = [from[0] + t * (to[0] - from[0]), from[1] + t * (to[1] - from[1])];
        p
    };
    let result = if !all_fit {
        at(1.0)
    } else if fits(&at(0.0)) {
        at(0.0)
    } else {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            if fits(&at(mid)) { hi = mid } else { lo = mid }
        }
        at(hi)
    };
    let fits = all_fit && fits(&result);
    let result = keep_inactive(params, result);
    Constrained { clamped: changes(params, &result), params: result, fits }
}

/// Result of [`fit_heights`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify::Tsify))]
pub struct HeightsFit {
    /// The adjusted parameters; the given ones when the line already lies inside the band or
    /// can't be made to.
    pub params: LaserParams,
    /// The envelope's across range now lies inside the band. False (and nothing changed) when
    /// the line's width and motion alone are wider than the band, the band is empty or the
    /// view has no size.
    pub fits: bool,
    /// What had to change, in words: `position` (across only), `deformation`, `scan height`.
    /// Empty when nothing did.
    pub clamped: Vec<String>,
}

/// `params` changed so the line's envelope lies inside the band `across` = `[min, max]`: view
/// px along the line's normal about its resting center now (as [`Envelope::across_px`], so
/// about the metric base line), e.g. the rows a camera's region sees.
///
/// 1. unchanged when the envelope's across range already lies inside;
/// 2. else, if the range is not wider than the band, the line moves across (along its normal,
///    never along it) only as far as needed;
/// 3. else the deformation and the scan's height gain shrink to the largest size whose range
///    fits, and the line then moves as in 2. A height gain derived from the scan scale
///    (`MatchScale`, `FromAngle`) becomes `Manual` at the lowered value;
/// 4. unchanged, with `fits` false, when even flat the line (its width and motion) is wider
///    than the band.
///
/// Length, motion, the scan scale and the position along the line stay as they are, so the
/// mapping along the line and the travel don't change; only the heights do.
pub fn fit_heights(params: &LaserParams, across: [f32; 2], view_px: [f32; 2], scan: Option<&Scan>) -> HeightsFit {
    let unchanged = |fits| HeightsFit { params: params.clone(), fits, clamped: Vec::new() };
    let [lo, hi] = across;
    if !(lo.is_finite() && hi.is_finite() && lo < hi) || view_px[0] <= 0.0 || view_px[1] <= 0.0 {
        return unchanged(false);
    }
    let range = |p: &LaserParams| line_envelope(p, view_px, scan).across_px;
    let r = range(params);
    if r[0] >= lo && r[1] <= hi {
        return unchanged(true);
    }
    let band = hi - lo;
    let width = |r: [f32; 2]| r[1] - r[0];
    let lowered = |s: f32| lowered_heights(params, s, view_px, scan);

    let sized = if width(r) <= band {
        params.clone()
    } else if width(range(&lowered(0.0))) > band {
        // The width and motion alone don't fit: change nothing.
        return unchanged(false);
    } else {
        // The range shrinks monotonically with the scale: bisect for the largest that fits.
        let (mut s_lo, mut s_hi) = (0.0f32, 1.0f32);
        for _ in 0..30 {
            let mid = 0.5 * (s_lo + s_hi);
            if width(range(&lowered(mid))) <= band { s_lo = mid } else { s_hi = mid }
        }
        lowered(s_lo)
    };

    // Move across only as far as needed.
    let r = range(&sized);
    let shift = if r[0] < lo {
        lo - r[0]
    } else if r[1] > hi {
        hi - r[1]
    } else {
        0.0
    };
    let mut result = sized;
    if shift != 0.0 {
        let e = line_envelope(&result, view_px, scan);
        let center = [e.center_px[0] + e.normal[0] * shift, e.center_px[1] + e.normal[1] * shift];
        result.line.position_pct = [center[0] / view_px[0] * 100.0, center[1] / view_px[1] * 100.0];
    }
    // The band is fixed in the view: measure the result about the original center.
    let e0 = line_envelope(params, view_px, scan);
    let e = line_envelope(&result, view_px, scan);
    let offset = (e.center_px[0] - e0.center_px[0]) * e0.normal[0] + (e.center_px[1] - e0.center_px[1]) * e0.normal[1];
    const TOLERANCE_PX: f32 = 1e-2;
    let fits = e.across_px[0] + offset >= lo - TOLERANCE_PX && e.across_px[1] + offset <= hi + TOLERANCE_PX;
    let result = keep_inactive(params, result);
    HeightsFit { clamped: changes(params, &result), params: result, fits }
}

/// `p` with the heights that shape its line now (the scan's while it plays one, else the
/// deformation) times `s`. A height gain derived from the scan scale becomes `Manual` at its
/// derived value times `s`; at `s == 1` nothing changes.
fn lowered_heights(p: &LaserParams, s: f32, view_px: [f32; 2], scan: Option<&Scan>) -> LaserParams {
    let mut p = p.clone();
    if s == 1.0 {
        return p;
    }
    match scan.filter(|_| p.scan.enabled) {
        Some(sc) => {
            let (mapping, _) = scan::display_mapping(sc, &p.scan, model::line_length_px(&p.line, view_px));
            p.scan.height_gain = HeightGain::Manual;
            p.scan.height_px_per_mm = mapping.height_px_per_mm * s;
        }
        None => {
            p.deform.height_px *= s;
            p.deform.amplitude_px *= s;
        }
    }
    p
}

/// `result` with the settings the line doesn't use now (another motion or deformation kind,
/// a disabled scan) put back as in `original`: shrinking them limits nothing and would
/// surprise the user switching to them later.
fn keep_inactive(original: &LaserParams, mut result: LaserParams) -> LaserParams {
    if result.motion.kind != MotionKind::Sweep {
        result.motion.span_pct = original.motion.span_pct;
    }
    if result.motion.kind != MotionKind::Sine {
        result.motion.amplitude_px = original.motion.amplitude_px;
    }
    if !matches!(result.deform.kind, DeformKind::Step | DeformKind::Bump | DeformKind::Box) {
        result.deform.height_px = original.deform.height_px;
    }
    if result.deform.kind != DeformKind::Sine {
        result.deform.amplitude_px = original.deform.amplitude_px;
    }
    if !result.scan.enabled {
        result.scan.px_per_mm = original.scan.px_per_mm;
        result.scan.height_px_per_mm = original.scan.height_px_per_mm;
    }
    result
}

/// What differs between `a` and `b` among what [`constrain_into`] changes.
fn changes(a: &LaserParams, b: &LaserParams) -> Vec<String> {
    let mut out = Vec::new();
    let mut note = |changed: bool, what: &str| {
        if changed {
            out.push(what.to_string());
        }
    };
    note(a.line.position_pct != b.line.position_pct, "position");
    note(a.line.length != b.line.length, "length");
    note(a.motion.span_pct != b.motion.span_pct || a.motion.amplitude_px != b.motion.amplitude_px, "movement");
    note(a.deform.height_px != b.deform.height_px || a.deform.amplitude_px != b.deform.amplitude_px, "deformation");
    note(a.scan.px_per_mm != b.scan.px_per_mm, "scan scale");
    note(a.scan.height_px_per_mm != b.scan.height_px_per_mm || a.scan.height_gain != b.scan.height_gain, "scan height");
    out
}

/// `p` with the deformation and (where set by hand) the scan's height gain times `s`.
fn scaled_shape(p: &LaserParams, s: f32) -> LaserParams {
    let mut p = p.clone();
    p.deform.height_px *= s;
    p.deform.amplitude_px *= s;
    if p.scan.height_gain == HeightGain::Manual {
        p.scan.height_px_per_mm *= s;
    }
    p
}

/// `p` with the line length and motion range times `s`.
fn scaled(p: &LaserParams, s: f32) -> LaserParams {
    let mut p = p.clone();
    p.line.length *= s;
    p.motion.span_pct *= s;
    p.motion.amplitude_px *= s;
    if p.scan.scale == ScanScale::Fixed {
        p.scan.px_per_mm *= s;
    }
    p
}

fn centroid(polygon: &[[f32; 2]]) -> [f32; 2] {
    let n = polygon.len() as f32;
    let sum = polygon.iter().fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
    [sum[0] / n, sum[1] / n]
}

/// Distance of `p` from the nearest edge of the convex `polygon`, negative outside.
fn inset_distance(polygon: &[[f32; 2]], p: [f32; 2]) -> f32 {
    // Orientation from the signed area, so either winding works.
    let area: f32 = polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(a, b)| {
            let e = [b[0] - a[0], b[1] - a[1]];
            let len = (e[0] * e[0] + e[1] * e[1]).sqrt().max(1e-6);
            sign * (e[0] * (p[1] - a[1]) - e[1] * (p[0] - a[0])) / len
        })
        .fold(f32::INFINITY, f32::min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{ScanParams, SweepMode};

    const VIEW: [f32; 2] = [1000.0, 500.0];

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-2
    }

    fn approx2(a: [f32; 2], b: [f32; 2]) -> bool {
        approx(a[0], b[0]) && approx(a[1], b[1])
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 2]> {
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    #[test]
    fn static_line_is_its_length_by_its_width() {
        let mut p = LaserParams::default();
        p.profile.end_softness_px = 0.0;
        // 80 % of 1000 px, 6 px wide, centered.
        let e = line_envelope(&p, VIEW, None);
        assert!(approx2(e.center_px, [500.0, 250.0]));
        assert!(approx2(e.along_px, [-400.0, 400.0]));
        assert!(approx2(e.across_px, [-3.0, 3.0]));
        assert!(approx2(e.corners[0], [100.0, 247.0]));
        assert!(approx2(e.corners[2], [900.0, 253.0]));
    }

    #[test]
    fn soft_ends_extend_the_line_by_half_their_length() {
        let mut p = LaserParams::default();
        p.profile.end_softness_px = 20.0;
        assert!(approx2(line_envelope(&p, VIEW, None).along_px, [-410.0, 410.0]));
    }

    #[test]
    fn motion_widens_across() {
        let mut p = LaserParams::default();
        p.line.width_px = 0.0;
        p.motion = MotionParams { kind: MotionKind::Sweep, span_pct: 50.0, sweep_mode: SweepMode::Wrap, ..Default::default() };
        // Horizontal line: the sweep runs over 50 % of the 500 px height.
        assert!(approx2(line_envelope(&p, VIEW, None).across_px, [-125.0, 125.0]));
        p.motion = MotionParams { kind: MotionKind::Sine, amplitude_px: -40.0, ..Default::default() };
        assert!(approx2(line_envelope(&p, VIEW, None).across_px, [-40.0, 40.0]));
    }

    #[test]
    fn motion_range_covers_sampled_offsets() {
        let m = MotionParams { kind: MotionKind::Sweep, speed_px_s: 333.0, span_pct: 70.0, sweep_mode: SweepMode::Bounce, ..Default::default() };
        let r = motion_range(&m, 500.0);
        for i in 0..1000 {
            let o = model::motion_offset(&m, i as f64 * 0.013, 500.0);
            assert!(o >= r[0] - 1e-3 && o <= r[1] + 1e-3, "{o} outside {r:?}");
        }
    }

    #[test]
    fn deformation_adds_to_motion_on_its_side() {
        let mut p = LaserParams::default();
        p.line.width_px = 2.0;
        p.deform = DeformParams { kind: DeformKind::Step, height_px: -60.0, ..Default::default() };
        p.motion = MotionParams { kind: MotionKind::Sine, amplitude_px: 10.0, ..Default::default() };
        assert!(approx2(line_envelope(&p, VIEW, None).across_px, [-71.0, 11.0]));
        p.deform = DeformParams { kind: DeformKind::Sine, amplitude_px: 5.0, ..Default::default() };
        assert!(approx2(line_envelope(&p, VIEW, None).across_px, [-16.0, 16.0]));
    }

    #[test]
    fn deform_range_covers_sampled_offsets() {
        for kind in [DeformKind::Step, DeformKind::Bump, DeformKind::Box, DeformKind::Sine] {
            let d = DeformParams { kind, height_px: 35.0, amplitude_px: 12.0, ..Default::default() };
            let r = deform_range(&d);
            for i in 0..2000 {
                let o = model::deform_offset(&d, i as f32 - 1000.0, 0.0);
                assert!(o >= r[0] - 1e-3 && o <= r[1] + 1e-3, "{kind:?}: {o} outside {r:?}");
            }
        }
    }

    #[test]
    fn vertical_and_tilted_lines_rotate_the_envelope() {
        let mut p = LaserParams::default();
        p.profile.end_softness_px = 0.0;
        p.line.orientation = Orientation::Vertical;
        p.line.length = 100.0;
        let e = line_envelope(&p, VIEW, None);
        // Along y now: 100 % of the 500 px height.
        assert!(approx2(e.dir, [0.0, 1.0]));
        assert!(approx2(e.along_px, [-250.0, 250.0]));
        assert!(e.corners.iter().all(|c| approx((c[0] - 500.0).abs(), 3.0)), "{:?}", e.corners);

        p.line.orientation = Orientation::Horizontal;
        p.line.tilt_deg = 30.0;
        let e = line_envelope(&p, VIEW, None);
        assert!(approx2(e.dir, [30f32.to_radians().cos(), 30f32.to_radians().sin()]));
    }

    #[test]
    fn scan_heights_set_the_across_range() {
        use crate::mesh::box_mesh;
        use crate::scan::{Progress, SliceSettings, slice};
        use std::sync::atomic::AtomicBool;

        let m = box_mesh([0.0; 3], [10.0, 5.0, 3.0]);
        let s = SliceSettings { lateral_res_mm: 1.0, travel_step_mm: 1.0, ..Default::default() };
        let scan = slice(&m, &s, "", &AtomicBool::new(false), &Progress::new(0)).unwrap();
        let mut p = LaserParams::default();
        p.line.width_px = 0.0;
        p.scan = ScanParams { enabled: true, scale: ScanScale::Fixed, px_per_mm: 10.0, ..Default::default() };
        let e = line_envelope(&p, VIEW, Some(&scan));
        // Higher surfaces move the line up (negative y): 3 mm × 10 px/mm.
        assert!(approx(e.across_px[0], -30.0), "{:?}", e.across_px);
        assert!(approx(e.across_px[1], 0.0));
        // In fixed scale the scan width sets the length.
        assert!(approx(e.along_px[1] - e.along_px[0], scan.width_mm() * 10.0 + p.profile.end_softness_px));
        // A disabled scan does not shape the line.
        p.scan.enabled = false;
        assert!(approx2(line_envelope(&p, VIEW, Some(&scan)).across_px, [0.0, 0.0]));
    }

    #[test]
    fn calibration_line_is_clean_and_white() {
        let mut p = LaserParams::default();
        p.line.width_px = 1.0;
        p.line.intensity_pct = 30.0;
        p.color.wavelength_nm = 520.0;
        p.motion.kind = MotionKind::Sine;
        p.deform.kind = DeformKind::Bump;
        p.noise.noise_pct = 5.0;
        let c = calibration_params(&p);
        assert_eq!(c.line.width_px, MIN_CALIBRATION_WIDTH_PX);
        assert_eq!(c.line.intensity_pct, 100.0);
        assert_eq!(c.profile.kind, ProfileKind::Hard);
        assert_eq!(c.profile.end_softness_px, 0.0);
        assert_eq!((c.motion.kind, c.deform.kind, c.scan.enabled), (MotionKind::Static, DeformKind::None, false));
        assert_eq!(c.noise.noise_pct, 0.0);
        assert_eq!((c.color.mode, c.color.custom_rgb), (ColorMode::Custom, [1.0, 1.0, 1.0]));
    }

    #[test]
    fn patterns_place_full_lines() {
        let base = calibration_params(&LaserParams::default());
        let h = pattern_params(&base, Pattern::Line { orientation: Orientation::Horizontal, position_pct: 20.0, width_pct: None, tilt_deg: None }, VIEW);
        let e = line_envelope(&h, VIEW, None);
        assert!(approx2(e.center_px, [500.0, 100.0]));
        assert!(approx2(e.along_px, [-500.0, 500.0]));
        let v = pattern_params(&base, Pattern::Line { orientation: Orientation::Vertical, position_pct: 75.0, width_pct: None, tilt_deg: None }, VIEW);
        let e = line_envelope(&v, VIEW, None);
        assert!(approx2(e.center_px, [750.0, 250.0]));
        assert!(approx2(e.along_px, [-250.0, 250.0]));
        assert_eq!(pattern_params(&base, Pattern::Blank, VIEW).line.intensity_pct, 0.0);
        // A band: 10 % of the 1000 px width, and of the 500 px height.
        let band = |orientation, width_pct| pattern_params(&base, Pattern::Line { orientation, position_pct: 50.0, width_pct: Some(width_pct), tilt_deg: None }, VIEW);
        assert_eq!(band(Orientation::Vertical, 10.0).line.width_px, 100.0);
        assert_eq!(band(Orientation::Horizontal, 10.0).line.width_px, 50.0);
        assert_eq!(band(Orientation::Horizontal, 100.0).line.width_px, 500.0);
    }

    #[test]
    fn line_patterns_can_be_tilted() {
        let mut user = LaserParams::default();
        user.line.tilt_deg = 12.0;
        let base = calibration_params(&user);
        let flat = pattern_params(&base, Pattern::Line { orientation: Orientation::Horizontal, position_pct: 40.0, width_pct: None, tilt_deg: None }, VIEW);
        assert_eq!(flat.line.tilt_deg, 0.0);
        let t = pattern_params(&base, Pattern::Line { orientation: Orientation::Horizontal, position_pct: 40.0, width_pct: None, tilt_deg: Some(-2.5) }, VIEW);
        assert_eq!(t.line.tilt_deg, -2.5);
        let e = line_envelope(&t, VIEW, None);
        assert!(approx2(e.center_px, [500.0, 200.0]));
        assert!(approx2(e.dir, [(-2.5f32).to_radians().cos(), (-2.5f32).to_radians().sin()]));
        let p: Pattern = serde_json::from_str(r#"{"kind":"line","orientation":"Horizontal","position_pct":40,"tilt_deg":-2.5}"#).unwrap();
        assert_eq!(p, Pattern::Line { orientation: Orientation::Horizontal, position_pct: 40.0, width_pct: None, tilt_deg: Some(-2.5) });
    }

    #[test]
    fn pattern_json_is_tagged() {
        let p: Pattern = serde_json::from_str(r#"{"kind":"line","orientation":"Vertical","position_pct":30}"#).unwrap();
        assert_eq!(p, Pattern::Line { orientation: Orientation::Vertical, position_pct: 30.0, width_pct: None, tilt_deg: None });
        let b: Pattern = serde_json::from_str(r#"{"kind":"line","orientation":"Horizontal","position_pct":5,"width_pct":2.5}"#).unwrap();
        assert_eq!(b, Pattern::Line { orientation: Orientation::Horizontal, position_pct: 5.0, width_pct: Some(2.5), tilt_deg: None });
        let b: Pattern = serde_json::from_str(r#"{"kind":"blank"}"#).unwrap();
        assert_eq!(b, Pattern::Blank);
    }

    #[test]
    fn inside_respects_margin_and_winding() {
        let mut p = LaserParams::default();
        p.profile.end_softness_px = 0.0;
        let e = line_envelope(&p, VIEW, None); // x 100..900, y 247..253
        let field = rect(90.0, 200.0, 910.0, 300.0);
        assert!(e.inside(&field, 5.0));
        assert!(!e.inside(&field, 15.0));
        let reversed: Vec<_> = field.iter().rev().copied().collect();
        assert!(e.inside(&reversed, 5.0));
        assert!(!e.inside(&rect(200.0, 0.0, 1000.0, 500.0), 0.0));
    }

    #[test]
    fn fit_recentres_and_shrinks_into_the_field() {
        let mut p = LaserParams::default();
        p.profile.end_softness_px = 0.0;
        p.motion = MotionParams { kind: MotionKind::Sine, amplitude_px: 100.0, ..Default::default() };
        let field = rect(400.0, 100.0, 700.0, 300.0);
        let fit = fit_into(&p, &field, VIEW, 10.0, None);
        assert!(fit.fits);
        let e = line_envelope(&fit.params, VIEW, None);
        assert!(e.inside(&field, 10.0 - 1e-2));
        // Centered in the field, and as large as the field allows (within the bisection).
        assert!(approx2(e.center_px, [550.0, 200.0]));
        let length = e.along_px[1] - e.along_px[0];
        assert!((279.0..=280.01).contains(&length), "{length}");
        assert!(fit.params.motion.amplitude_px < 100.0);
    }

    #[test]
    fn fit_keeps_a_line_that_already_fits() {
        let mut p = LaserParams::default();
        p.profile.end_softness_px = 0.0;
        p.line.length = 10.0;
        let fit = fit_into(&p, &rect(0.0, 0.0, 1000.0, 500.0), VIEW, 0.0, None);
        assert!(fit.fits);
        assert_eq!(fit.params.line.length, 10.0);
    }

    #[test]
    fn fit_reports_a_deformation_too_large_for_the_field() {
        let mut p = LaserParams::default();
        p.deform = DeformParams { kind: DeformKind::Step, height_px: -300.0, ..Default::default() };
        let fit = fit_into(&p, &rect(0.0, 100.0, 1000.0, 300.0), VIEW, 0.0, None);
        assert!(!fit.fits);
        assert!(!fit_into(&p, &[[0.0, 0.0], [1.0, 1.0]], VIEW, 0.0, None).fits);
    }
    #[test]
    fn constrain_keeps_a_line_inside_the_area() {
        let p = LaserParams::default();
        let c = constrain_into(&p, &rect(0.0, 0.0, 1000.0, 500.0), VIEW, None);
        assert!(c.fits);
        assert!(c.clamped.is_empty());
        assert_eq!(c.params, p);
    }

    #[test]
    fn constrain_moves_the_line_only_as_far_as_needed() {
        let mut p = LaserParams::default();
        p.profile.end_softness_px = 0.0;
        p.line.length_unit = LengthUnit::Pixels;
        p.line.length = 200.0;
        p.line.position_pct = [30.0, 50.0];
        // The line spans x 200..400; the area starts at 350.
        let area = rect(350.0, 100.0, 900.0, 400.0);
        let c = constrain_into(&p, &area, VIEW, None);
        assert!(c.fits);
        assert_eq!(c.clamped, vec!["position".to_string()]);
        let e = line_envelope(&c.params, VIEW, None);
        assert!(e.inside(&area, -1e-2));
        assert!((e.center_px[0] - 450.0).abs() < 0.5, "{:?}", e.center_px);
        assert_eq!(c.params.line.length, 200.0);
        // A constrained line stays as it is.
        let again = constrain_into(&c.params, &area, VIEW, None);
        assert!(again.clamped.is_empty());
    }

    #[test]
    fn constrain_shrinks_length_and_motion_before_the_shape() {
        let mut p = LaserParams::default();
        p.profile.end_softness_px = 0.0;
        p.motion = MotionParams { kind: MotionKind::Sine, amplitude_px: 100.0, ..Default::default() };
        p.deform = DeformParams { kind: DeformKind::Step, height_px: -20.0, ..Default::default() };
        let area = rect(400.0, 100.0, 700.0, 300.0);
        let c = constrain_into(&p, &area, VIEW, None);
        assert!(c.fits);
        assert!(line_envelope(&c.params, VIEW, None).inside(&area, -1e-2));
        assert!(c.clamped.contains(&"length".to_string()));
        assert!(c.clamped.contains(&"movement".to_string()));
        assert!(!c.clamped.contains(&"deformation".to_string()));
        assert_eq!(c.params.deform.height_px, -20.0);
        // Settings the line doesn't use stay as they were.
        assert_eq!(c.params.motion.span_pct, p.motion.span_pct);
        let mut still = LaserParams::default();
        still.profile.end_softness_px = 0.0;
        let c = constrain_into(&still, &area, VIEW, None);
        assert_eq!(c.clamped, vec!["position".to_string(), "length".to_string()]);
        assert_eq!(c.params.motion, still.motion);
    }

    #[test]
    fn constrain_shrinks_a_deformation_too_large_for_the_area() {
        let mut p = LaserParams::default();
        p.deform = DeformParams { kind: DeformKind::Step, height_px: -300.0, ..Default::default() };
        let area = rect(0.0, 100.0, 1000.0, 300.0);
        let c = constrain_into(&p, &area, VIEW, None);
        assert!(c.fits);
        assert!(c.clamped.contains(&"deformation".to_string()));
        assert!(c.params.deform.height_px > -200.0);
        assert!(line_envelope(&c.params, VIEW, None).inside(&area, -1e-2));
    }

    #[test]
    fn constrain_reports_a_line_wider_than_the_area() {
        let mut p = LaserParams::default();
        p.line.width_px = 300.0;
        assert!(!constrain_into(&p, &rect(0.0, 100.0, 1000.0, 300.0), VIEW, None).fits);
        assert!(!constrain_into(&p, &[[0.0, 0.0], [1.0, 1.0]], VIEW, None).fits);
    }

    fn box_scan() -> Scan {
        use crate::mesh::box_mesh;
        use crate::scan::{Progress, SliceSettings, slice};
        use std::sync::atomic::AtomicBool;
        let m = box_mesh([0.0; 3], [10.0, 5.0, 3.0]);
        let s = SliceSettings { lateral_res_mm: 1.0, travel_step_mm: 1.0, ..Default::default() };
        slice(&m, &s, "", &AtomicBool::new(false), &Progress::new(0)).unwrap()
    }

    #[test]
    fn fit_heights_keeps_a_line_inside_the_band() {
        let p = LaserParams::default(); // 6 px wide: across -3..3
        let f = fit_heights(&p, [-10.0, 10.0], VIEW, None);
        assert!(f.fits);
        assert!(f.clamped.is_empty());
        assert_eq!(f.params, p);
    }

    #[test]
    fn fit_heights_moves_the_line_across_only_as_far_as_needed() {
        let mut p = LaserParams::default();
        p.line.width_px = 2.0;
        p.line.position_pct = [30.0, 50.0];
        p.deform = DeformParams { kind: DeformKind::Step, height_px: -40.0, ..Default::default() };
        // Across -41..1 (higher = up); the band -30..20 is wide enough: move by +11.
        let f = fit_heights(&p, [-30.0, 20.0], VIEW, None);
        assert!(f.fits);
        assert_eq!(f.clamped, vec!["position".to_string()]);
        assert_eq!(f.params.deform, p.deform);
        // Horizontal line: across is +y, along is x and stays.
        assert!(approx(f.params.line.position_pct[0], 30.0));
        assert!(approx(f.params.line.position_pct[1] / 100.0 * VIEW[1], 250.0 + 11.0), "{:?}", f.params.line.position_pct);
        assert_eq!(f.params.line.length, p.line.length);
    }

    #[test]
    fn fit_heights_lowers_the_deformation_but_not_the_length() {
        let mut p = LaserParams::default();
        p.profile.end_softness_px = 0.0;
        p.line.width_px = 2.0;
        p.deform = DeformParams { kind: DeformKind::Bump, height_px: -100.0, ..Default::default() };
        p.motion = MotionParams { kind: MotionKind::Sine, amplitude_px: 5.0, ..Default::default() };
        let before = line_envelope(&p, VIEW, None);
        let f = fit_heights(&p, [-40.0, 10.0], VIEW, None);
        assert!(f.fits);
        assert!(f.clamped.contains(&"deformation".to_string()));
        assert!(f.params.deform.height_px > -100.0 && f.params.deform.height_px < 0.0);
        assert_eq!(f.params.line.length, p.line.length);
        assert_eq!(f.params.motion, p.motion);
        let e = line_envelope(&f.params, VIEW, None);
        assert_eq!(e.along_px, before.along_px);
        // The band is fixed in the view: about the original center.
        let offset = e.center_px[1] - before.center_px[1];
        assert!(e.across_px[0] + offset >= -40.0 - 1e-2 && e.across_px[1] + offset <= 10.0 + 1e-2, "{:?} {offset}", e.across_px);
        // As large as the band allows: the range fills it.
        assert!((e.across_px[1] - e.across_px[0] - 50.0).abs() < 0.1, "{:?}", e.across_px);
    }

    #[test]
    fn fit_heights_lowers_a_scan_gain_and_switches_it_to_manual() {
        let scan = box_scan();
        let mut p = LaserParams::default();
        p.profile.end_softness_px = 0.0;
        p.line.width_px = 0.0;
        // MatchScale at 10 px/mm: 3 mm heights reach 30 px up.
        p.scan = ScanParams { enabled: true, scale: ScanScale::Fixed, px_per_mm: 10.0, ..Default::default() };
        assert_eq!(p.scan.height_gain, HeightGain::MatchScale);
        let before = line_envelope(&p, VIEW, Some(&scan));
        assert!(approx(before.across_px[0], -30.0));
        let f = fit_heights(&p, [-12.0, 0.0], VIEW, Some(&scan));
        assert!(f.fits);
        assert_eq!(f.params.scan.height_gain, HeightGain::Manual);
        assert!((f.params.scan.height_px_per_mm - 4.0).abs() < 1e-3, "{}", f.params.scan.height_px_per_mm);
        assert_eq!(f.clamped, vec!["scan height".to_string()]);
        // Length, scale and position stay.
        assert_eq!(f.params.scan.px_per_mm, 10.0);
        assert_eq!(f.params.scan.scale, ScanScale::Fixed);
        assert_eq!(f.params.line.position_pct, p.line.position_pct);
        assert_eq!(line_envelope(&f.params, VIEW, Some(&scan)).along_px, before.along_px);
        // FromAngle too, from its derived value.
        p.scan.height_gain = HeightGain::FromAngle;
        let f = fit_heights(&p, [-1.0, 0.0], VIEW, Some(&scan));
        assert!(f.fits);
        assert_eq!(f.params.scan.height_gain, HeightGain::Manual);
        assert!(line_envelope(&f.params, VIEW, Some(&scan)).across_px[0] >= -1.0 - 1e-2);
        // The deformation is not what shapes a playing scan: it stays.
        assert_eq!(f.params.deform, p.deform);
    }

    #[test]
    fn fit_heights_reports_a_line_wider_than_the_band() {
        let mut p = LaserParams::default();
        p.line.width_px = 30.0;
        p.deform = DeformParams { kind: DeformKind::Step, height_px: -100.0, ..Default::default() };
        let f = fit_heights(&p, [-10.0, 10.0], VIEW, None);
        assert!(!f.fits);
        assert!(f.clamped.is_empty());
        assert_eq!(f.params, p);
        // Motion counts as width: it is not lowered.
        let mut m = LaserParams::default();
        m.motion = MotionParams { kind: MotionKind::Sine, amplitude_px: 50.0, ..Default::default() };
        assert!(!fit_heights(&m, [-20.0, 20.0], VIEW, None).fits);
        // An empty band, or a view without size.
        assert!(!fit_heights(&LaserParams::default(), [5.0, 5.0], VIEW, None).fits);
        assert!(!fit_heights(&LaserParams::default(), [f32::NAN, 5.0], VIEW, None).fits);
        assert!(!fit_heights(&LaserParams::default(), [-5.0, 5.0], [0.0, 0.0], None).fits);
    }

    #[test]
    fn fit_heights_moves_a_tilted_line_along_its_normal() {
        let mut p = LaserParams::default();
        p.line.width_px = 2.0;
        p.line.tilt_deg = 20.0;
        p.deform = DeformParams { kind: DeformKind::Step, height_px: -40.0, ..Default::default() };
        let before = line_envelope(&p, VIEW, None);
        let f = fit_heights(&p, [-20.0, 30.0], VIEW, None);
        assert!(f.fits);
        let e = line_envelope(&f.params, VIEW, None);
        let d = [e.center_px[0] - before.center_px[0], e.center_px[1] - before.center_px[1]];
        // Nothing along the line, +21 px across it.
        assert!((d[0] * before.dir[0] + d[1] * before.dir[1]).abs() < 1e-2, "{d:?}");
        assert!(approx(d[0] * before.normal[0] + d[1] * before.normal[1], 21.0), "{d:?}");
    }
}
