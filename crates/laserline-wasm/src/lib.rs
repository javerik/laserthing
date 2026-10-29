//! Browser bindings of laserline (`@javerik/laserline` on npm).
//!
//! - Pure functions and handles: [`Mesh`], [`Scan`], [`slice()`], presets, plot curves.
//! - [`Emulator`] and [`Slicer`]: the core state machines, driven from JavaScript.
//! - `LaserView` and `MeshView`: wgpu renderers bound to a `<canvas>`, using
//!   WebGPU where available and WebGL2 otherwise (wasm32 only).
//!
//! Plain data (parameters, settings, scan metadata) crosses the boundary as JS objects
//! whose TypeScript types are generated from the Rust structs.

#![warn(missing_docs)]

mod data;
mod state;
#[cfg(target_arch = "wasm32")]
mod view;

pub use data::{Mesh, Scan, grid_size, slice, to_scan_frame};
pub use state::{Emulator, Slicer};
#[cfg(target_arch = "wasm32")]
pub use view::{LaserView, MeshView};

use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::prelude::*;

// Runs on instantiation; not exported to JavaScript.
#[wasm_bindgen(start, skip_typescript)]
fn start() {
    console_error_panic_hook::set_once();
}

pub(crate) fn to_js<T: Serialize + ?Sized>(value: &T) -> JsValue {
    value.serialize(&serde_wasm_bindgen::Serializer::json_compatible()).unwrap_throw()
}

pub(crate) fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, JsError> {
    serde_wasm_bindgen::from_value(value).map_err(|e| JsError::new(&e.to_string()))
}

pub(crate) fn js_err(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

#[wasm_bindgen(typescript_custom_section)]
const TS_TYPES: &str = r#"
/** A built-in parameter set. */
export interface Preset { name: string; params: LaserParams }
/** Axis-aligned bounds in model units or mm. */
export interface Bounds { lo: [number, number, number]; hi: [number, number, number] }
/** A slice the host should run (in a worker), see `Slicer`. */
export interface SliceRequest { id: number; modelId: number; settings: SliceSettings; name: string }
/** The model loaded into a `Slicer`. */
export interface ModelInfo { id: number; name: string; location: string; triangles: number; bounds: Bounds }
/** Grid a slice would produce, or why the settings are unusable. */
export type GridInfo = { cols: number; rows: number; bytes: number } | { error: string };
/** Run of profile samples with the same visibility; `points` holds (column, height mm) pairs. */
export interface ProfileRun { visible: boolean; points: Float32Array }
/** Text to draw over the 3D view at (x, y), fractions of the view size from the top left; `color` is RGB 0..255. */
export interface ViewLabel { text: string; color: [number, number, number]; x: number; y: number }
/** Fixed limits and format constants, see `constants()`. */
export interface Constants {
  /** Most samples per profile a slice may produce. */
  maxCols: number;
  /** Most profiles a slice may produce. */
  maxRows: number;
  /** Error text of a cancelled slice. */
  cancelled: string;
  /** Seconds settings must be stable before an automatic re-slice. */
  debounceS: number;
  /** Ratio FWHM / standard deviation of a Gaussian profile. */
  fwhmPerSigma: number;
  /** File extension of scan files, without the dot. */
  scanExtension: string;
  /** `.lscan` format version this build reads and writes. */
  scanVersion: number;
  /** Extensions of the model files `Mesh.parse` reads (lower case, without the dot). */
  modelExtensions: string[];
  /** Longest side in px of `Scan.rangeImage()`. */
  rangeImageMax: number;
}
"#;

/// Library version.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Constants {
    max_cols: usize,
    max_rows: usize,
    cancelled: &'static str,
    debounce_s: f64,
    fwhm_per_sigma: f32,
    scan_extension: &'static str,
    scan_version: u16,
    model_extensions: &'static [&'static str],
    range_image_max: usize,
}

/// Fixed limits and format constants.
#[wasm_bindgen(unchecked_return_type = "Constants")]
pub fn constants() -> JsValue {
    use laserline_core::{formats, model, scan, scan_file, slicer, viz};
    to_js(&Constants {
        max_cols: scan::MAX_COLS,
        max_rows: scan::MAX_ROWS,
        cancelled: slicer::CANCELLED,
        debounce_s: slicer::DEBOUNCE_S,
        fwhm_per_sigma: model::FWHM_PER_SIGMA,
        scan_extension: scan_file::EXTENSION,
        scan_version: scan_file::VERSION,
        model_extensions: formats::EXTENSIONS,
        range_image_max: viz::RANGE_IMAGE_MAX,
    })
}

/// Default laser parameters.
#[wasm_bindgen(js_name = defaultParams, unchecked_return_type = "LaserParams")]
pub fn default_params() -> JsValue {
    to_js(&laserline_core::params::LaserParams::default())
}

/// Default slice settings.
#[wasm_bindgen(js_name = defaultSliceSettings, unchecked_return_type = "SliceSettings")]
pub fn default_slice_settings() -> JsValue {
    to_js(&laserline_core::scan::SliceSettings::default())
}

#[derive(Serialize)]
struct Preset {
    name: &'static str,
    params: laserline_core::params::LaserParams,
}

/// The built-in presets, in display order.
#[wasm_bindgen(js_name = builtinPresets, unchecked_return_type = "Preset[]")]
pub fn builtin_presets() -> JsValue {
    let presets: Vec<_> = laserline_core::presets::builtin().into_iter().map(|(name, params)| Preset { name, params }).collect();
    to_js(&presets)
}

/// Serializes parameters as a preset file (pretty JSON).
#[wasm_bindgen(js_name = presetToJson)]
pub fn preset_to_json(#[wasm_bindgen(unchecked_param_type = "LaserParams")] params: JsValue) -> Result<String, JsError> {
    laserline_core::presets::to_json(&from_js(params)?).map_err(js_err)
}

/// Parses a preset file; missing fields take their defaults.
#[wasm_bindgen(js_name = presetFromJson, unchecked_return_type = "LaserParams")]
pub fn preset_from_json(text: &str) -> Result<JsValue, JsError> {
    laserline_core::presets::from_json(text).map(|p| to_js(&p)).map_err(js_err)
}

/// Cross-section intensity (0..1) at `n` points from `x0` to `x1` px across the line.
#[wasm_bindgen(js_name = profileCurve)]
pub fn profile_curve(
    #[wasm_bindgen(unchecked_param_type = "ProfileParams")] profile: JsValue,
    #[wasm_bindgen(js_name = widthPx)] width_px: f32,
    x0: f32,
    x1: f32,
    n: usize,
) -> Result<Vec<f32>, JsError> {
    let p = from_js(profile)?;
    Ok(sample(x0, x1, n, |x| laserline_core::model::profile_intensity(&p, width_px, x)))
}

/// Deformation (px across the line) at `n` points from `x0` to `x1` px along the line,
/// at simulation time `t` seconds.
#[wasm_bindgen(js_name = deformCurve)]
pub fn deform_curve(
    #[wasm_bindgen(unchecked_param_type = "DeformParams")] deform: JsValue,
    t: f64,
    x0: f32,
    x1: f32,
    n: usize,
) -> Result<Vec<f32>, JsError> {
    let d = from_js(deform)?;
    Ok(sample(x0, x1, n, |x| laserline_core::model::deform_offset(&d, x, t)))
}

/// Deformation (px across the line) at `n` points `start + i·step` px along the line,
/// at simulation time `t` seconds.
#[wasm_bindgen(js_name = sampleDeformation)]
pub fn sample_deformation(
    #[wasm_bindgen(unchecked_param_type = "DeformParams")] deform: JsValue,
    t: f64,
    start: f32,
    step: f32,
    n: usize,
) -> Result<Vec<f32>, JsError> {
    let mut out = vec![0.0; n];
    laserline_core::model::sample_deformation(&from_js(deform)?, t, start, step, &mut out);
    Ok(out)
}

/// Line direction in radians (screen coordinates, y down).
#[wasm_bindgen(js_name = lineAngle)]
pub fn line_angle(#[wasm_bindgen(unchecked_param_type = "LineParams")] line: JsValue) -> Result<f32, JsError> {
    Ok(laserline_core::model::line_angle(&from_js(line)?))
}

/// Extent of a `width`×`height` rectangle projected onto the unit vector (`axisX`, `axisY`).
#[wasm_bindgen(js_name = extentAlong)]
pub fn extent_along(width: f32, height: f32, #[wasm_bindgen(js_name = axisX)] axis_x: f32, #[wasm_bindgen(js_name = axisY)] axis_y: f32) -> f32 {
    laserline_core::model::extent_along([width, height], [axis_x, axis_y])
}

/// Displacement in px of the line perpendicular to itself at time `t` seconds, for a
/// view `extentPx` wide in that direction.
#[wasm_bindgen(js_name = motionOffset)]
pub fn motion_offset(
    #[wasm_bindgen(unchecked_param_type = "MotionParams")] motion: JsValue,
    t: f64,
    #[wasm_bindgen(js_name = extentPx)] extent_px: f32,
) -> Result<f32, JsError> {
    Ok(laserline_core::model::motion_offset(&from_js(motion)?, t, extent_px))
}

/// Grid spacing of 1, 2 or 5 × 10ⁿ giving about ten lines over `span`.
#[wasm_bindgen(js_name = niceStep)]
pub fn nice_step(span: f32) -> f32 {
    laserline_core::viz::nice_step(span)
}

fn sample(x0: f32, x1: f32, n: usize, f: impl Fn(f32) -> f32) -> Vec<f32> {
    let n = n.max(2);
    (0..n).map(|i| f(x0 + (x1 - x0) * i as f32 / (n - 1) as f32)).collect()
}

/// Linear RGB (0..1) the laser is drawn in.
#[wasm_bindgen(js_name = laserRgb)]
pub fn laser_rgb(#[wasm_bindgen(unchecked_param_type = "ColorParams")] color: JsValue) -> Result<Vec<f32>, JsError> {
    Ok(laserline_core::model::laser_rgb(&from_js(color)?).to_vec())
}

/// Approximate display colour (RGB 0..1) of light with the given wavelength.
#[wasm_bindgen(js_name = wavelengthToRgb)]
pub fn wavelength_to_rgb(nm: f32) -> Vec<f32> {
    laserline_core::model::wavelength_to_rgb(nm).to_vec()
}

/// `params` as a clean calibration line: static, undeformed, no scan or noise, a hard
/// profile without soft ends, full intensity; the colour is kept.
#[wasm_bindgen(js_name = calibrationParams, unchecked_return_type = "LaserParams")]
pub fn calibration_params(#[wasm_bindgen(unchecked_param_type = "LaserParams")] params: JsValue) -> Result<JsValue, JsError> {
    Ok(to_js(&laserline_core::calibration::calibration_params(&from_js(params)?)))
}

/// `base` (from `calibrationParams`) showing `pattern` in a `viewW`×`viewH` px view:
/// nothing, or a line or band across the whole view at a given position.
#[wasm_bindgen(js_name = patternParams, unchecked_return_type = "LaserParams")]
pub fn pattern_params(
    #[wasm_bindgen(unchecked_param_type = "LaserParams")] base: JsValue,
    #[wasm_bindgen(unchecked_param_type = "Pattern")] pattern: JsValue,
    #[wasm_bindgen(js_name = viewW)] view_w: f32,
    #[wasm_bindgen(js_name = viewH)] view_h: f32,
) -> Result<JsValue, JsError> {
    Ok(to_js(&laserline_core::calibration::pattern_params(&from_js(base)?, from_js(pattern)?, [view_w, view_h])))
}
