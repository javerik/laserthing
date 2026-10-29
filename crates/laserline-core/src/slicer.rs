//! State of an interactive slicer: the loaded model, its settings, debounced automatic
//! re-slicing and the 3D preview geometry.
//!
//! Running a slice is left to the host (a thread natively, a Web Worker in the
//! browser): [`Slicer::start`] and [`Slicer::poll`] hand out a [`SliceRequest`] to run
//! with [`crate::scan::slice()`], and the host reports the outcome with
//! [`Slicer::finish`]. A newer request supersedes older ones; the host should cancel
//! those, and their late results are ignored.

use crate::camera::OrbitCamera;
use crate::gpu_types::GpuMesh;
use crate::mesh::{self, Mesh};
use crate::scan::{ModelUnit, Scan, SliceSettings};
use crate::viz;
use glam::Vec3;
use std::sync::Arc;

/// Settings must be stable this long before an automatic re-slice starts.
pub const DEBOUNCE_S: f64 = 0.3;

/// Error text of a slice stopped through its cancel flag.
pub const CANCELLED: &str = "cancelled";

/// A model loaded into the [`Slicer`].
#[derive(Clone, Debug)]
pub struct Model {
    /// The parsed mesh.
    pub mesh: Arc<Mesh>,
    /// Short name (file name), stored in the scans as their source.
    pub name: String,
    /// Where the model came from (full path or URL), for display.
    pub location: String,
    /// Raw bounds in model units.
    pub bounds: ([f32; 3], [f32; 3]),
    id: u64,
}

impl Model {
    /// Changes whenever another model is loaded; requests carry it as `model_id`.
    pub fn id(&self) -> u64 {
        self.id
    }
}

/// A slice the host should run: `scan::slice(&mesh, &settings, &name, ..)`.
#[derive(Clone, Debug)]
pub struct SliceRequest {
    /// Request id, passed back to [`Slicer::finish`].
    pub id: u64,
    /// [`Model::id`] of the model to slice.
    pub model_id: u64,
    /// Mesh to slice.
    pub mesh: Arc<Mesh>,
    /// Settings to slice with.
    pub settings: SliceSettings,
    /// Model name, stored as the scan's source.
    pub name: String,
}

/// What the 3D preview mesh was built from.
#[derive(Clone, Debug, PartialEq)]
enum PreviewKey {
    /// Model id and the transform part of the settings.
    Model(u64, SliceSettings),
    /// Heightmap surface of a scan loaded without its model.
    Scan(usize),
}

/// Geometry of the 3D preview: the model in the scan frame, or a scan's surface.
#[derive(Clone, Debug)]
pub struct Preview {
    key: PreviewKey,
    /// Triangles in scan-frame mm.
    pub mesh: Arc<GpuMesh>,
    /// Bounds of the preview geometry in scan-frame mm.
    pub bounds: (Vec3, Vec3),
}

/// Interactive slicer state, see the module docs.
#[derive(Clone, Debug)]
pub struct Slicer {
    /// Settings of the next slice.
    pub settings: SliceSettings,
    /// Re-slice automatically when settings change.
    pub auto: bool,
    /// The loaded model.
    pub model: Option<Model>,
    /// Id of the request currently running.
    running: Option<u64>,
    /// Settings of the most recently started slice.
    sliced: Option<SliceSettings>,
    /// Settings waiting for the debounce, and since when (host seconds).
    pending: Option<(SliceSettings, f64)>,
    /// Error of the last slice, if it failed (not set for cancelled slices).
    pub error: Option<String>,
    /// Seconds the last successful slice took.
    pub last_duration_s: Option<f64>,
    /// 3D view camera; reset when a new model is loaded.
    pub camera: Option<OrbitCamera>,
    preview: Option<Preview>,
    /// The unit a model's file set, and the unit the settings had before it.
    file_unit: Option<(ModelUnit, ModelUnit)>,
    next_id: u64,
}

impl Slicer {
    /// No model; `auto` turns on automatic re-slicing.
    pub fn new(settings: SliceSettings, auto: bool) -> Self {
        Self {
            settings,
            auto,
            model: None,
            running: None,
            sliced: None,
            pending: None,
            error: None,
            last_duration_s: None,
            camera: None,
            preview: None,
            file_unit: None,
            next_id: 1,
        }
    }

    /// A slice is running.
    pub fn is_busy(&self) -> bool {
        self.running.is_some()
    }

    /// Id of the request currently running, if any.
    pub fn running(&self) -> Option<u64> {
        self.running
    }

    /// True when the loaded model has settings that were not sliced yet.
    pub fn is_stale(&self) -> bool {
        self.model.is_some() && self.sliced.as_ref() != Some(&self.settings)
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Loads a model; returns the slice to run when auto re-slicing is on.
    ///
    /// A unit the model's file declares replaces the settings' unit. A later model without
    /// one gets the unit from before back, unless it was changed in the meantime.
    pub fn set_model(&mut self, mesh: impl Into<Arc<Mesh>>, name: impl Into<String>, location: impl Into<String>) -> Option<SliceRequest> {
        let mesh = mesh.into();
        // The user's own unit: the one before a file set it, while it is unchanged.
        let own = match self.file_unit.take() {
            Some((set, before)) if self.settings.unit == set => before,
            _ => self.settings.unit,
        };
        self.settings.unit = mesh.unit.unwrap_or(own);
        if let Some(unit) = mesh.unit {
            self.file_unit = Some((unit, own));
        }
        let bounds = mesh.bounds();
        let id = self.take_id();
        self.model = Some(Model { mesh, name: name.into(), location: location.into(), bounds, id });
        self.sliced = None;
        self.camera = None;
        self.error = None;
        if self.auto { self.start() } else { None }
    }

    /// Forgets the model, e.g. when a scan is played back without it. A running slice
    /// is abandoned.
    pub fn clear_model(&mut self) {
        self.model = None;
        self.running = None;
        self.pending = None;
    }

    /// Slices the model with the current settings, superseding a running slice.
    pub fn start(&mut self) -> Option<SliceRequest> {
        let model = self.model.as_ref()?;
        let (mesh, name, model_id) = (model.mesh.clone(), model.name.clone(), model.id);
        let id = self.take_id();
        self.running = Some(id);
        self.sliced = Some(self.settings.clone());
        self.pending = None;
        self.error = None;
        Some(SliceRequest { id, model_id, mesh, settings: self.settings.clone(), name })
    }

    /// Reports the outcome of request `id`, which took `duration_s` seconds. Returns
    /// the scan to show (a `Scan` or a handle to one), or `None` for errors and
    /// superseded requests.
    pub fn finish<S>(&mut self, id: u64, result: Result<S, String>, duration_s: f64) -> Option<S> {
        if self.running != Some(id) {
            return None;
        }
        self.running = None;
        match result {
            Ok(scan) => {
                self.last_duration_s = Some(duration_s);
                Some(scan)
            }
            Err(e) => {
                if e != CANCELLED {
                    self.error = Some(e);
                }
                None
            }
        }
    }

    /// Call every frame with the host's clock in seconds; returns an automatic
    /// re-slice once the settings have been stable for [`DEBOUNCE_S`].
    pub fn poll(&mut self, now_s: f64) -> Option<SliceRequest> {
        if !self.is_stale() {
            self.pending = None;
        } else if self.auto {
            if self.pending.as_ref().is_none_or(|(s, _)| *s != self.settings) {
                self.pending = Some((self.settings.clone(), now_s));
            }
            if self.pending.as_ref().is_some_and(|(_, t)| now_s - t >= DEBOUNCE_S) {
                return self.start();
            }
        }
        None
    }

    /// The preview built by the last [`Slicer::update_preview`].
    pub fn preview(&self) -> Option<&Preview> {
        self.preview.as_ref()
    }

    /// Rebuilds the 3D preview mesh when the model, its transform or the scan changed.
    pub fn update_preview(&mut self, scan: Option<&Arc<Scan>>) {
        let key = match (&self.model, scan) {
            (Some(m), _) => PreviewKey::Model(m.id, viz::transform_part(&self.settings)),
            (None, Some(s)) => PreviewKey::Scan(Arc::as_ptr(s) as usize),
            (None, None) => {
                self.preview = None;
                return;
            }
        };
        if self.preview.as_ref().is_some_and(|p| p.key == key) {
            return;
        }
        let id = self.take_id();
        let (positions, indices) = match (&self.model, scan) {
            (Some(m), _) => (mesh::to_scan_frame(&m.mesh, &self.settings), m.mesh.triangles.iter().flatten().copied().collect()),
            (None, Some(s)) => viz::heightmap_surface(s),
            (None, None) => unreachable!(),
        };
        let (lo, hi) = mesh::bounds(&positions);
        self.preview = Some(Preview {
            key,
            mesh: Arc::new(GpuMesh { id, positions, indices }),
            bounds: (lo.into(), hi.into()),
        });
    }

    /// Bounds of everything the 3D view shows: the preview geometry and the scan grid.
    pub fn view_bounds(&self, scan: Option<&Scan>) -> Option<(Vec3, Vec3)> {
        let mesh_bounds = self.preview.as_ref().map(|p| p.bounds);
        let scan_bounds = scan.map(viz::scan_bounds);
        [mesh_bounds, scan_bounds].into_iter().flatten().reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    }
}

/// Scene of the slicer's 3D view for one frame.
#[derive(Clone, Debug)]
pub struct View3d {
    /// Model or scan surface, if any.
    pub mesh: Option<Arc<GpuMesh>>,
    /// Conveyor, laser plane, camera and profile lines with their labels.
    pub overlay: viz::OverlayWithLabels,
    /// Camera to draw with.
    pub camera: OrbitCamera,
    /// Highest point of the scene, for the height tint.
    pub z_max: f32,
}

impl Slicer {
    /// Builds the 3D view scene, creating the camera on first use. Returns `None`
    /// when there is nothing to show. `reset_camera` reframes the scene.
    pub fn view_3d(&mut self, scan: Option<&Scan>, row: usize, reset_camera: bool) -> Option<View3d> {
        let (lo, mut hi) = self.view_bounds(scan)?;
        let z_max = hi.z;
        // Frame the laser source and camera marker above the object too.
        hi.z = lo.z + (hi.z - lo.z) * 1.6;
        let camera = self.camera.get_or_insert_with(|| OrbitCamera::framing(lo, hi));
        if reset_camera {
            *camera = OrbitCamera::framing(lo, hi);
        }
        let camera = *camera;
        let settings = scan.map_or(&self.settings, |s| &s.meta.settings);
        let overlay = viz::build_overlay(scan, row, settings, lo, hi.with_z(z_max));
        Some(View3d { mesh: self.preview.as_ref().map(|p| p.mesh.clone()), overlay, camera, z_max: z_max.max(1e-3) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::box_mesh;
    use crate::scan::{Progress, slice};
    use std::sync::atomic::AtomicBool;

    fn run(r: &SliceRequest) -> Result<Scan, String> {
        slice(&r.mesh, &r.settings, &r.name, &AtomicBool::new(false), &Progress::new(0))
    }

    fn coarse() -> SliceSettings {
        SliceSettings { lateral_res_mm: 1.0, travel_step_mm: 1.0, ..Default::default() }
    }

    #[test]
    fn auto_slice_and_debounce() {
        let mut sl = Slicer::new(coarse(), true);
        let req = sl.set_model(box_mesh([0.0; 3], [10.0, 5.0, 3.0]), "box.stl", "/tmp/box.stl").expect("auto slice");
        assert!(sl.is_busy());
        assert!(!sl.is_stale());
        let scan = sl.finish(req.id, run(&req), 0.5).expect("scan");
        assert_eq!(scan.meta.source, "box.stl");
        assert_eq!(sl.last_duration_s, Some(0.5));
        assert!(!sl.is_busy());

        sl.settings.yaw_deg = 10.0;
        assert!(sl.is_stale());
        assert!(sl.poll(1.0).is_none(), "debouncing");
        sl.settings.yaw_deg = 20.0;
        assert!(sl.poll(1.2).is_none(), "changed again: debounce restarts");
        assert!(sl.poll(1.4).is_none());
        let req = sl.poll(1.6).expect("stable for the debounce time");
        assert_eq!(req.settings.yaw_deg, 20.0);
    }

    #[test]
    fn unit_from_the_file() {
        let mut sl = Slicer::new(SliceSettings { unit: ModelUnit::Inch, ..coarse() }, true);
        sl.set_model(box_mesh([0.0; 3], [1.0, 1.0, 1.0]), "a.stl", "a.stl");
        assert_eq!(sl.settings.unit, ModelUnit::Inch, "an STL keeps the setting");
        let in_unit = |unit| Mesh { unit: Some(unit), ..box_mesh([0.0; 3], [1.0, 1.0, 1.0]) };
        let req = sl.set_model(in_unit(ModelUnit::Cm), "a.3mf", "a.3mf").unwrap();
        assert_eq!((sl.settings.unit, req.settings.unit), (ModelUnit::Cm, ModelUnit::Cm));
        sl.set_model(in_unit(ModelUnit::M), "a.glb", "a.glb");
        assert_eq!(sl.settings.unit, ModelUnit::M);
        sl.set_model(box_mesh([0.0; 3], [1.0, 1.0, 1.0]), "b.stl", "b.stl");
        assert_eq!(sl.settings.unit, ModelUnit::Inch, "back to the user's unit");

        // A unit chosen after a file set one is the user's.
        sl.set_model(in_unit(ModelUnit::M), "a.glb", "a.glb");
        sl.settings.unit = ModelUnit::Cm;
        sl.set_model(box_mesh([0.0; 3], [1.0, 1.0, 1.0]), "b.stl", "b.stl");
        assert_eq!(sl.settings.unit, ModelUnit::Cm);
    }

    #[test]
    fn superseded_and_failed_requests() {
        let mut sl = Slicer::new(coarse(), false);
        assert!(sl.set_model(box_mesh([0.0; 3], [10.0, 5.0, 3.0]), "a", "a").is_none(), "manual mode");
        let old = sl.start().unwrap();
        let new = sl.start().unwrap();
        assert!(sl.finish(old.id, run(&old), 1.0).is_none(), "superseded result is dropped");
        assert!(sl.is_busy());
        assert!(sl.finish::<Scan>(new.id, Err(CANCELLED.into()), 0.0).is_none());
        assert!(sl.error.is_none(), "cancelling is not an error");
        let r = sl.start().unwrap();
        sl.finish::<Scan>(r.id, Err("boom".into()), 0.0);
        assert_eq!(sl.error.as_deref(), Some("boom"));
    }

    #[test]
    fn preview_follows_model_and_scan() {
        let mut sl = Slicer::new(coarse(), false);
        sl.update_preview(None);
        assert!(sl.preview().is_none());
        assert!(sl.view_3d(None, 0, false).is_none());

        sl.set_model(box_mesh([0.0; 3], [10.0, 5.0, 3.0]), "a", "a");
        sl.update_preview(None);
        let id = sl.preview().unwrap().mesh.id;
        sl.update_preview(None);
        assert_eq!(sl.preview().unwrap().mesh.id, id, "unchanged key keeps the mesh");
        sl.settings.lateral_res_mm = 2.0;
        sl.update_preview(None);
        assert_eq!(sl.preview().unwrap().mesh.id, id, "resolution does not move the model");
        sl.settings.yaw_deg = 45.0;
        sl.update_preview(None);
        assert_ne!(sl.preview().unwrap().mesh.id, id);

        let r = sl.start().unwrap();
        let scan = Arc::new(run(&r).unwrap());
        sl.clear_model();
        sl.update_preview(Some(&scan));
        assert!(!sl.preview().unwrap().mesh.indices.is_empty(), "heightmap surface");
        let view = sl.view_3d(Some(&scan), 2, false).unwrap();
        assert!(!view.overlay.overlay.lines_on_top.is_empty(), "current profile drawn");
    }
}
