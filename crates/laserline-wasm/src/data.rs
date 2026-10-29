//! Meshes, scans and slicing.

use crate::{from_js, js_err, to_js};
use laserline_core::scan::{self, ModelUnit, SliceObserver};
use laserline_core::{formats, mesh, scan_file, viz};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

/// A parsed 3D model.
#[wasm_bindgen]
#[derive(Clone)]
pub struct Mesh {
    pub(crate) inner: Arc<mesh::Mesh>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Bounds {
    pub lo: [f32; 3],
    pub hi: [f32; 3],
}

#[wasm_bindgen]
impl Mesh {
    /// Parses a model file: STL, 3MF, OBJ, PLY, glTF (embedded buffers), GLB or STEP, by
    /// the extension of `fileName` or, failing that, by its content (see
    /// `constants().modelExtensions`). Errors start with the format's name.
    pub fn parse(bytes: &[u8], #[wasm_bindgen(js_name = fileName)] file_name: &str) -> Result<Mesh, JsError> {
        formats::parse_model(bytes, file_name).map(|m| Mesh { inner: Arc::new(m) }).map_err(js_err)
    }

    /// Parses a binary or ASCII STL file.
    #[wasm_bindgen(js_name = parseStl)]
    pub fn parse_stl(bytes: &[u8]) -> Result<Mesh, JsError> {
        mesh::parse_stl(bytes).map(|m| Mesh { inner: Arc::new(m) }).map_err(js_err)
    }

    /// A mesh from the arrays `vertices()` and `triangles()` return, e.g. of a mesh parsed
    /// in a worker, and the unit its file declared.
    #[wasm_bindgen(js_name = fromArrays)]
    pub fn from_arrays(vertices: &[f32], triangles: &[u32], #[wasm_bindgen(unchecked_param_type = "ModelUnit | undefined")] unit: JsValue) -> Result<Mesh, JsError> {
        if !vertices.len().is_multiple_of(3) || !triangles.len().is_multiple_of(3) {
            return Err(JsError::new("vertices and triangles must hold triples"));
        }
        if triangles.iter().any(|&i| i as usize >= vertices.len() / 3) {
            return Err(JsError::new("triangle index out of range"));
        }
        let unit: Option<ModelUnit> = from_js(unit)?;
        let inner = mesh::Mesh {
            vertices: vertices.chunks_exact(3).map(|v| [v[0], v[1], v[2]]).collect(),
            triangles: triangles.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect(),
            unit,
        };
        Ok(Mesh { inner: Arc::new(inner) })
    }

    /// Unit the model's file declares (3MF, glTF, STEP); loading the mesh into a
    /// `Slicer` makes it the settings' unit.
    #[wasm_bindgen(getter, unchecked_return_type = "ModelUnit | undefined")]
    pub fn unit(&self) -> JsValue {
        self.inner.unit.map_or(JsValue::UNDEFINED, |u| to_js(&u))
    }

    /// Number of triangles.
    #[wasm_bindgen(getter, js_name = triangleCount)]
    pub fn triangle_count(&self) -> usize {
        self.inner.triangles.len()
    }

    /// Number of distinct vertices.
    #[wasm_bindgen(getter, js_name = vertexCount)]
    pub fn vertex_count(&self) -> usize {
        self.inner.vertices.len()
    }

    /// Vertex positions as x, y, z triples in model units.
    pub fn vertices(&self) -> Vec<f32> {
        self.inner.vertices.iter().flatten().copied().collect()
    }

    /// Triangles as triples of indices into `vertices()`.
    pub fn triangles(&self) -> Vec<u32> {
        self.inner.triangles.iter().flatten().copied().collect()
    }

    /// Bounds in model units.
    #[wasm_bindgen(unchecked_return_type = "Bounds")]
    pub fn bounds(&self) -> JsValue {
        let (lo, hi) = self.inner.bounds();
        to_js(&Bounds { lo, hi })
    }
}

/// A sliced scan: one height profile per travel step.
#[wasm_bindgen]
#[derive(Clone)]
pub struct Scan {
    pub(crate) inner: Arc<scan::Scan>,
}

#[wasm_bindgen]
impl Scan {
    /// Reads an `.lscan` file.
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(bytes: &[u8]) -> Result<Scan, JsError> {
        scan_file::from_bytes(bytes).map(|s| Scan { inner: Arc::new(s) }).map_err(js_err)
    }

    /// The scan as an `.lscan` file.
    #[wasm_bindgen(js_name = toBytes)]
    pub fn to_bytes(&self) -> Result<Vec<u8>, JsError> {
        scan_file::to_bytes(&self.inner).map_err(js_err)
    }

    /// Suggested file name for saving, e.g. `part.lscan`.
    #[wasm_bindgen(js_name = fileName)]
    pub fn file_name(&self) -> String {
        scan_file::file_name(&self.inner)
    }

    /// Grid geometry, settings and source of the scan.
    #[wasm_bindgen(unchecked_return_type = "ScanMeta")]
    pub fn meta(&self) -> JsValue {
        to_js(&self.inner.meta)
    }

    /// Number of profiles.
    #[wasm_bindgen(getter)]
    pub fn rows(&self) -> usize {
        self.inner.meta.rows
    }

    /// Samples per profile.
    #[wasm_bindgen(getter)]
    pub fn cols(&self) -> usize {
        self.inner.meta.cols
    }

    /// Distance in mm between the first and last sample along the line.
    #[wasm_bindgen(getter, js_name = widthMm)]
    pub fn width_mm(&self) -> f32 {
        self.inner.width_mm()
    }

    /// Distance in mm between the first and last profile.
    #[wasm_bindgen(getter, js_name = lengthMm)]
    pub fn length_mm(&self) -> f32 {
        self.inner.length_mm()
    }

    /// Highest surface point in mm.
    #[wasm_bindgen(getter, js_name = zMaxMm)]
    pub fn z_max_mm(&self) -> f32 {
        self.inner.meta.z_max_mm
    }

    /// Position in mm of the line center along the line.
    #[wasm_bindgen(getter, js_name = uCenterMm)]
    pub fn u_center_mm(&self) -> f32 {
        self.inner.u_center_mm()
    }

    /// Travel position of profile `row` in mm.
    #[wasm_bindgen(js_name = rowV)]
    pub fn row_v(&self, row: usize) -> f32 {
        self.inner.meta.v0_mm + row as f32 * self.inner.meta.dv_mm
    }

    /// Heights of profile `row` in mm (NaN where nothing was hit). `row` is clamped;
    /// empty for a scan without rows.
    #[wasm_bindgen(js_name = rowHeights)]
    pub fn row_heights(&self, row: usize) -> Vec<f32> {
        self.clamp_row(row).map_or_else(Vec::new, |r| self.inner.row_height(r).to_vec())
    }

    /// Visibility of profile `row`: 1 where the camera sees the laser, else 0. `row` is
    /// clamped; empty for a scan without rows.
    #[wasm_bindgen(js_name = rowVisible)]
    pub fn row_visible(&self, row: usize) -> Vec<u8> {
        self.clamp_row(row).map_or_else(Vec::new, |r| self.inner.row_visible(r).to_vec())
    }

    /// Height in mm of sample `col` in profile `row` if the camera sees it, else NaN.
    pub fn seen(&self, col: usize, row: usize) -> Result<f32, JsError> {
        if col >= self.inner.meta.cols || row >= self.inner.meta.rows {
            return Err(JsError::new("cell out of range"));
        }
        Ok(self.inner.seen(col, row))
    }

    /// All heights in mm, row-major (`rows × cols`, NaN where nothing was hit).
    pub fn heights(&self) -> Vec<f32> {
        self.inner.height.clone()
    }

    /// All visibility flags, row-major (`rows × cols`, 1 = seen by the camera).
    pub fn visibility(&self) -> Vec<u8> {
        self.inner.visible.clone()
    }

    /// Profile `row` split into seen and occluded polylines.
    #[wasm_bindgen(js_name = profileRuns, unchecked_return_type = "ProfileRun[]")]
    pub fn profile_runs(&self, row: usize) -> js_sys::Array {
        let Some(row) = self.clamp_row(row) else { return js_sys::Array::new() };
        viz::profile_runs(&self.inner, row)
            .into_iter()
            .map(|r| {
                let points: Vec<f32> = r.points.into_iter().flatten().collect();
                let run = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&run, &"visible".into(), &r.visible.into());
                let _ = js_sys::Reflect::set(&run, &"points".into(), &js_sys::Float32Array::from(&points[..]));
                JsValue::from(run)
            })
            .collect()
    }

    /// Range image: travel to the right, along the line downwards; grey by height where
    /// seen, magenta where occluded. Large scans are subsampled.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = rangeImage)]
    pub fn range_image(&self) -> Result<web_sys::ImageData, JsError> {
        let img = viz::range_image(&self.inner);
        web_sys::ImageData::new_with_u8_clamped_array_and_sh(wasm_bindgen::Clamped(&img.pixels), img.width as u32, img.height as u32)
            .map_err(|e| JsError::new(&format!("{e:?}")))
    }
}

impl Scan {
    fn clamp_row(&self, row: usize) -> Option<usize> {
        self.inner.meta.rows.checked_sub(1).map(|last| row.min(last))
    }
}

/// Model vertices in the scan frame as x, y, z triples in mm: x along the line
/// (centered), y along travel (from 0), z up (object resting on 0).
#[wasm_bindgen(js_name = toScanFrame)]
pub fn to_scan_frame(mesh: &Mesh, #[wasm_bindgen(unchecked_param_type = "SliceSettings")] settings: JsValue) -> Result<Vec<f32>, JsError> {
    Ok(mesh::to_scan_frame(&mesh.inner, &from_js(settings)?).into_iter().flatten().collect())
}

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum GridInfo {
    Ok { cols: usize, rows: usize, bytes: usize },
    Err { error: String },
}

impl GridInfo {
    pub(crate) fn new(lo: [f32; 3], hi: [f32; 3], settings: &scan::SliceSettings) -> GridInfo {
        match scan::grid_size(lo, hi, settings) {
            Ok((cols, rows)) => GridInfo::Ok { cols, rows, bytes: cols * rows * scan_file::BYTES_PER_CELL },
            Err(error) => GridInfo::Err { error },
        }
    }
}

/// Grid a slice with `settings` would produce for an object with scan-frame `bounds`
/// in mm (see `toScanFrame`); `bytes` is the size of the height and visibility data.
#[wasm_bindgen(js_name = gridSize, unchecked_return_type = "GridInfo")]
pub fn grid_size(
    #[wasm_bindgen(unchecked_param_type = "Bounds")] bounds: JsValue,
    #[wasm_bindgen(unchecked_param_type = "SliceSettings")] settings: JsValue,
) -> Result<JsValue, JsError> {
    let b: Bounds = from_js(bounds)?;
    Ok(to_js(&GridInfo::new(b.lo, b.hi, &from_js(settings)?)))
}

/// Reports progress to a JS callback. Slicing on wasm32 runs on the calling thread, so
/// the callback is never touched from another thread.
struct JsObserver {
    on_progress: Option<js_sys::Function>,
    is_cancelled: Option<js_sys::Function>,
    last: Cell<u32>,
}

// SAFETY: wasm32 without atomics has no threads; the slicer calls the observer only on
// the thread that created it.
unsafe impl Sync for JsObserver {}

impl SliceObserver for JsObserver {
    fn cancelled(&self) -> bool {
        self.is_cancelled.as_ref().is_some_and(|f| f.call0(&JsValue::NULL).is_ok_and(|v| v.is_truthy()))
    }

    fn progress(&self, permille: u32) {
        if permille / 10 != self.last.get() / 10
            && let Some(f) = &self.on_progress
        {
            self.last.set(permille);
            let _ = f.call1(&JsValue::NULL, &JsValue::from_f64(permille as f64 / 1000.0));
        }
    }
}

/// Slices `mesh` into a scan. `name` is stored as the scan's source. `onProgress`
/// receives the progress in 0..1. `isCancelled` is polled during slicing; returning
/// true fails the slice with `constants().cancelled`.
///
/// Runs synchronously: call it in a Web Worker to keep the page responsive. A worker
/// cannot receive messages while slicing, so `isCancelled` must read shared memory
/// (e.g. `Atomics.load` on a `SharedArrayBuffer`), or the worker is terminated instead.
#[wasm_bindgen]
pub fn slice(
    mesh: &Mesh,
    #[wasm_bindgen(unchecked_param_type = "SliceSettings")] settings: JsValue,
    name: &str,
    #[wasm_bindgen(js_name = onProgress)] on_progress: Option<js_sys::Function>,
    #[wasm_bindgen(js_name = isCancelled)] is_cancelled: Option<js_sys::Function>,
) -> Result<Scan, JsError> {
    let settings = from_js(settings)?;
    let observer = JsObserver { on_progress, is_cancelled, last: Cell::new(0) };
    scan::slice_observed(&mesh.inner, &settings, name, &observer).map(|s| Scan { inner: Arc::new(s) }).map_err(js_err)
}
