//! The emulator and slicer state machines, driven from JavaScript.

use crate::data::{Bounds, GridInfo, Mesh, Scan};
use crate::{from_js, to_js};
use laserline_core::{calibration, emulator, metric, scan, slicer};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// Simulation clock, scan playback and line parameters. Call `tick(dt)` once per
/// displayed frame and draw it with a `LaserView`.
#[wasm_bindgen]
pub struct Emulator {
    pub(crate) inner: emulator::Emulator,
    /// `Scan::object_bounds` of `inner.scan`, worked out once per scan for `metric`.
    object: Option<scan::ObjectBounds>,
}

#[wasm_bindgen]
impl Emulator {
    /// Starts with `params`, or the defaults.
    #[wasm_bindgen(constructor)]
    pub fn new(#[wasm_bindgen(unchecked_param_type = "LaserParams | undefined")] params: JsValue) -> Result<Emulator, JsError> {
        let params = if params.is_undefined() || params.is_null() { Default::default() } else { from_js(params)? };
        Ok(Emulator { inner: emulator::Emulator::new(params), object: None })
    }

    /// All line parameters (a copy; assign to change them).
    #[wasm_bindgen(getter, unchecked_return_type = "LaserParams")]
    pub fn params(&self) -> JsValue {
        to_js(&self.inner.params)
    }

    /// Replaces all parameters; missing fields take their defaults.
    #[wasm_bindgen(setter)]
    pub fn set_params(&mut self, #[wasm_bindgen(unchecked_param_type = "LaserParams")] params: JsValue) -> Result<(), JsError> {
        self.inner.params = from_js(params)?;
        Ok(())
    }

    /// Whether the clock and scan playback advance in `tick`.
    #[wasm_bindgen(getter)]
    pub fn playing(&self) -> bool {
        self.inner.playing
    }

    /// Pauses (false) or resumes (true) the simulation.
    #[wasm_bindgen(setter)]
    pub fn set_playing(&mut self, playing: bool) {
        self.inner.playing = playing;
    }

    /// Simulated seconds.
    #[wasm_bindgen(getter, js_name = simTime)]
    pub fn sim_time(&self) -> f64 {
        self.inner.sim_time
    }

    /// Jumps the simulation clock to `t` seconds.
    #[wasm_bindgen(setter, js_name = simTime)]
    pub fn set_sim_time(&mut self, t: f64) {
        self.inner.sim_time = t;
    }

    /// Smoothed frame rate measured by `tick`.
    #[wasm_bindgen(getter)]
    pub fn fps(&self) -> f32 {
        self.inner.fps
    }

    /// Advances by one displayed frame that took `dt` seconds.
    pub fn tick(&mut self, dt: f64) {
        self.inner.tick(dt);
    }

    /// Rewinds the clock and the scan playback.
    #[wasm_bindgen(js_name = resetTime)]
    pub fn reset_time(&mut self) {
        self.inner.reset_time();
    }

    /// Shows the profile `delta` rows away from the current one.
    #[wasm_bindgen(js_name = stepScan)]
    pub fn step_scan(&mut self, delta: i32) {
        self.inner.step_scan(delta as isize);
    }

    /// Jumps to profile `row`.
    #[wasm_bindgen(js_name = seekRow)]
    pub fn seek_row(&mut self, row: usize) {
        self.inner.seek_row(row);
    }

    /// Profile currently shown (0 without a scan).
    #[wasm_bindgen(getter, js_name = currentRow)]
    pub fn current_row(&self) -> usize {
        self.inner.current_row()
    }

    /// True when the scan shapes the line.
    #[wasm_bindgen(getter, js_name = scanDrivesLine)]
    pub fn scan_drives_line(&self) -> bool {
        self.inner.scan_drives_line()
    }

    /// Plays `scan`, keeping the playback position.
    #[wasm_bindgen(js_name = setScan)]
    pub fn set_scan(&mut self, scan: &Scan) {
        self.object = scan.inner.object_bounds();
        self.inner.set_scan(scan.inner.clone());
    }

    /// Removes the scan; the line falls back to its parameters.
    #[wasm_bindgen(js_name = clearScan)]
    pub fn clear_scan(&mut self) {
        self.inner.scan = None;
        self.object = None;
    }

    /// The scan being played, if any.
    #[wasm_bindgen(getter)]
    pub fn scan(&self) -> Option<Scan> {
        self.inner.scan.clone().map(|inner| Scan { inner })
    }

    /// Area the line drawn with `params` covers in a `viewW`×`viewH` px view over its
    /// whole motion, deformation and the scan being played (if `params` enable it).
    #[wasm_bindgen(unchecked_return_type = "Envelope")]
    pub fn envelope(
        &self,
        #[wasm_bindgen(unchecked_param_type = "LaserParams")] params: JsValue,
        #[wasm_bindgen(js_name = viewW)] view_w: f32,
        #[wasm_bindgen(js_name = viewH)] view_h: f32,
    ) -> Result<JsValue, JsError> {
        let params = from_js(params)?;
        Ok(to_js(&calibration::line_envelope(&params, [view_w, view_h], self.inner.scan.as_deref())))
    }

    /// How the line drawn with `params` in a `viewW`×`viewH` px view maps the scan being
    /// played into view px, and how fast it travels; `null` without a scan or with
    /// `params.scan.enabled` off. `fps` is the measured frame rate (the row rate when a row
    /// is played per frame), `playing` whether the playback runs. Pass the user's parameters,
    /// not a calibration pattern's.
    #[wasm_bindgen(unchecked_return_type = "MetricMapping | null")]
    pub fn metric(
        &self,
        #[wasm_bindgen(unchecked_param_type = "LaserParams")] params: JsValue,
        #[wasm_bindgen(js_name = viewW)] view_w: f32,
        #[wasm_bindgen(js_name = viewH)] view_h: f32,
        fps: f32,
        playing: bool,
    ) -> Result<JsValue, JsError> {
        let params = from_js(params)?;
        let scan = self.inner.scan.as_deref().map(|s| (s, self.object.as_ref()));
        Ok(metric::metric_mapping(&params, [view_w, view_h], scan, fps, playing).map_or(JsValue::NULL, |m| to_js(&m)))
    }

    /// `params` moved into the middle of the convex `field` (view px corners) and, where
    /// needed, shrunk so the line's envelope stays `margin` px inside it.
    #[wasm_bindgen(js_name = fitInto, unchecked_return_type = "Fit")]
    pub fn fit_into(
        &self,
        #[wasm_bindgen(unchecked_param_type = "LaserParams")] params: JsValue,
        #[wasm_bindgen(unchecked_param_type = "[number, number][]")] field: JsValue,
        #[wasm_bindgen(js_name = viewW)] view_w: f32,
        #[wasm_bindgen(js_name = viewH)] view_h: f32,
        margin: f32,
    ) -> Result<JsValue, JsError> {
        let params = from_js(params)?;
        let field: Vec<[f32; 2]> = from_js(field)?;
        let fit = calibration::fit_into(&params, &field, [view_w, view_h], margin, self.inner.scan.as_deref());
        Ok(to_js(&fit))
    }

    /// `params` changed as little as needed so the line's envelope stays inside the convex
    /// `area` (view px corners): moved towards its middle only as far as needed, then its
    /// length and motion (and last its deformation and scan heights) shrunk. `clamped` names
    /// what changed; nothing does when the line is inside already.
    #[wasm_bindgen(js_name = constrainInto, unchecked_return_type = "Constrained")]
    pub fn constrain_into(
        &self,
        #[wasm_bindgen(unchecked_param_type = "LaserParams")] params: JsValue,
        #[wasm_bindgen(unchecked_param_type = "[number, number][]")] area: JsValue,
        #[wasm_bindgen(js_name = viewW)] view_w: f32,
        #[wasm_bindgen(js_name = viewH)] view_h: f32,
    ) -> Result<JsValue, JsError> {
        let params = from_js(params)?;
        let area: Vec<[f32; 2]> = from_js(area)?;
        Ok(to_js(&calibration::constrain_into(&params, &area, [view_w, view_h], self.inner.scan.as_deref())))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    id: u64,
    model_id: u64,
    settings: scan::SliceSettings,
    name: String,
}

fn request(r: Option<slicer::SliceRequest>) -> JsValue {
    r.map_or(JsValue::UNDEFINED, |r| to_js(&Request { id: r.id, model_id: r.model_id, settings: r.settings, name: r.name }))
}

#[derive(Serialize)]
struct ModelInfo<'a> {
    id: u64,
    name: &'a str,
    location: &'a str,
    triangles: usize,
    bounds: Bounds,
}

/// Slicer state: the model, its settings, debounced re-slicing and the 3D view camera.
///
/// Slicing itself runs wherever the host likes (usually a Web Worker): `setModel`,
/// `start` and `poll` return a `SliceRequest` to run, `finish` / `fail` report the
/// outcome. Requests superseded by newer ones are ignored when they finish.
#[wasm_bindgen]
pub struct Slicer {
    pub(crate) inner: slicer::Slicer,
}

#[wasm_bindgen]
impl Slicer {
    /// Starts with `settings` (or the defaults) and automatic re-slicing on unless
    /// `auto` is false.
    #[wasm_bindgen(constructor)]
    pub fn new(#[wasm_bindgen(unchecked_param_type = "SliceSettings | undefined")] settings: JsValue, auto: Option<bool>) -> Result<Slicer, JsError> {
        let settings = if settings.is_undefined() || settings.is_null() { Default::default() } else { from_js(settings)? };
        Ok(Slicer { inner: slicer::Slicer::new(settings, auto.unwrap_or(true)) })
    }

    /// Settings of the next slice (a copy; assign to change them).
    #[wasm_bindgen(getter, unchecked_return_type = "SliceSettings")]
    pub fn settings(&self) -> JsValue {
        to_js(&self.inner.settings)
    }

    /// Replaces the settings; missing fields take their defaults. May schedule an
    /// automatic re-slice (see `poll`).
    #[wasm_bindgen(setter)]
    pub fn set_settings(&mut self, #[wasm_bindgen(unchecked_param_type = "SliceSettings")] settings: JsValue) -> Result<(), JsError> {
        self.inner.settings = from_js(settings)?;
        Ok(())
    }

    /// Re-slice automatically when the settings change.
    #[wasm_bindgen(getter)]
    pub fn auto(&self) -> bool {
        self.inner.auto
    }

    /// Turns automatic re-slicing on or off.
    #[wasm_bindgen(setter)]
    pub fn set_auto(&mut self, auto: bool) {
        self.inner.auto = auto;
    }

    /// Loads a model; a unit its file declares (`Mesh.unit`) replaces the settings' unit.
    /// Returns the slice to run when auto re-slicing is on.
    #[wasm_bindgen(js_name = setModel, unchecked_return_type = "SliceRequest | undefined")]
    pub fn set_model(&mut self, mesh: &Mesh, name: &str, location: &str) -> JsValue {
        request(self.inner.set_model(mesh.inner.clone(), name, location))
    }

    /// The loaded model, if any.
    #[wasm_bindgen(unchecked_return_type = "ModelInfo | undefined")]
    pub fn model(&self) -> JsValue {
        self.inner.model.as_ref().map_or(JsValue::UNDEFINED, |m| {
            let (lo, hi) = m.bounds;
            to_js(&ModelInfo { id: m.id(), name: &m.name, location: &m.location, triangles: m.mesh.triangles.len(), bounds: Bounds { lo, hi } })
        })
    }

    /// Plays a scan without its model: drops the model and shows the scan's settings.
    #[wasm_bindgen(js_name = adoptScan)]
    pub fn adopt_scan(&mut self, scan: &Scan) {
        self.inner.clear_model();
        self.inner.settings = scan.inner.meta.settings.clone();
    }

    /// Drops the model; a running slice is superseded.
    #[wasm_bindgen(js_name = clearModel)]
    pub fn clear_model(&mut self) {
        self.inner.clear_model();
    }

    /// Slices with the current settings, superseding a running slice.
    #[wasm_bindgen(unchecked_return_type = "SliceRequest | undefined")]
    pub fn start(&mut self) -> JsValue {
        request(self.inner.start())
    }

    /// Call every frame with a clock in seconds; returns a due automatic re-slice.
    #[wasm_bindgen(unchecked_return_type = "SliceRequest | undefined")]
    pub fn poll(&mut self, now_s: f64) -> JsValue {
        request(self.inner.poll(now_s))
    }

    /// Reports that request `id` produced a scan in `duration_s` seconds. Returns true
    /// if it is still current, i.e. the scan should be shown; false if superseded.
    pub fn finish(&mut self, id: f64, duration_s: f64) -> bool {
        self.inner.finish(id as u64, Ok(()), duration_s).is_some()
    }

    /// Reports a failed slice (`"cancelled"` is not shown as an error).
    pub fn fail(&mut self, id: f64, error: String) {
        self.inner.finish::<()>(id as u64, Err(error), 0.0);
    }

    /// A slice is running.
    #[wasm_bindgen(getter, js_name = isBusy)]
    pub fn is_busy(&self) -> bool {
        self.inner.is_busy()
    }

    /// Id of the running request.
    #[wasm_bindgen(getter)]
    pub fn running(&self) -> Option<f64> {
        self.inner.running().map(|id| id as f64)
    }

    /// The model has settings that were not sliced yet.
    #[wasm_bindgen(getter, js_name = isStale)]
    pub fn is_stale(&self) -> bool {
        self.inner.is_stale()
    }

    /// Error of the last slice, if it failed.
    #[wasm_bindgen(getter)]
    pub fn error(&self) -> Option<String> {
        self.inner.error.clone()
    }

    /// Seconds the last successful slice took.
    #[wasm_bindgen(getter, js_name = lastDurationS)]
    pub fn last_duration_s(&self) -> Option<f64> {
        self.inner.last_duration_s
    }

    /// Size (along line, travel, height) of the model in the scan frame in mm.
    #[wasm_bindgen(js_name = modelSizeMm)]
    pub fn model_size_mm(&mut self) -> Option<Vec<f32>> {
        self.inner.model.as_ref()?;
        self.inner.update_preview(None);
        self.inner.preview().map(|p| (p.bounds.1 - p.bounds.0).to_array().to_vec())
    }

    /// Grid the current settings produce for the model.
    #[wasm_bindgen(js_name = gridInfo, unchecked_return_type = "GridInfo | undefined")]
    pub fn grid_info(&mut self) -> JsValue {
        if self.inner.model.is_none() {
            return JsValue::UNDEFINED;
        }
        self.inner.update_preview(None);
        let Some((lo, hi)) = self.inner.preview().map(|p| p.bounds) else { return JsValue::UNDEFINED };
        to_js(&GridInfo::new(lo.into(), hi.into(), &self.inner.settings))
    }

    /// Rotates the 3D view by a pointer drag in CSS pixels.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        if let Some(c) = &mut self.inner.camera {
            c.orbit(dx, dy);
        }
    }

    /// Pans the 3D view by a pointer drag in CSS pixels.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        if let Some(c) = &mut self.inner.camera {
            c.pan(dx, dy);
        }
    }

    /// Zooms the 3D view: `scroll` in pixels (positive = closer), `pinch` factor (> 1 = closer).
    pub fn zoom(&mut self, scroll: f32, pinch: Option<f32>) {
        if let Some(c) = &mut self.inner.camera {
            c.zoom(scroll, pinch.unwrap_or(1.0));
        }
    }

    /// Reframes the 3D view on the next render.
    #[wasm_bindgen(js_name = resetCamera)]
    pub fn reset_camera(&mut self) {
        self.inner.camera = None;
    }
}
