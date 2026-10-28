//! Triangle meshes and the transform into the scan frame. The model file formats
//! are read in [`crate::formats`].

use crate::formats::{self, ModelFormat};
use crate::scan::{ModelUnit, SliceSettings, UpAxis};
use std::collections::HashMap;

/// Indexed triangle mesh in model units.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    /// Distinct vertex positions.
    pub vertices: Vec<[f32; 3]>,
    /// Triangles as indices into `vertices`.
    pub triangles: Vec<[u32; 3]>,
    /// Unit the file declares its coordinates in, if its format has one (3MF, glTF, STEP).
    pub unit: Option<ModelUnit>,
}

impl Mesh {
    /// Axis-aligned bounds `(min, max)`; zeros for an empty mesh.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        bounds(&self.vertices)
    }
}

/// Axis-aligned bounds `(min, max)` of `points`; zeros when empty.
pub fn bounds(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    if points.is_empty() {
        return ([0.0; 3], [0.0; 3]);
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

/// Parses a binary or ASCII STL file.
pub fn parse_stl(bytes: &[u8]) -> Result<Mesh, String> {
    formats::parse(bytes, ModelFormat::Stl)
}

/// Collects triangles while merging identical vertices.
#[derive(Default)]
pub(crate) struct Builder {
    mesh: Mesh,
    index: HashMap<[u32; 3], u32>,
}

impl Builder {
    /// Adds a triangle; degenerate ones and ones with non-finite corners are dropped.
    pub(crate) fn push_triangle(&mut self, corners: [[f32; 3]; 3]) {
        if corners.iter().flatten().any(|c| !c.is_finite()) {
            return;
        }
        let tri = corners.map(|v| {
            *self.index.entry(v.map(f32::to_bits)).or_insert_with(|| {
                self.mesh.vertices.push(v);
                (self.mesh.vertices.len() - 1) as u32
            })
        });
        if tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] {
            self.mesh.triangles.push(tri);
        }
    }

    /// Adds a polygon, fanned into triangles from its first corner.
    pub(crate) fn push_polygon(&mut self, corners: &[[f32; 3]]) {
        for i in 1..corners.len().saturating_sub(1) {
            self.push_triangle([corners[0], corners[i], corners[i + 1]]);
        }
    }

    /// The mesh, with the unit its file declares; fails when it has no triangles.
    pub(crate) fn finish(mut self, unit: Option<ModelUnit>) -> Result<Mesh, String> {
        if self.mesh.triangles.is_empty() {
            return Err("no triangles found".into());
        }
        self.mesh.unit = unit;
        Ok(self.mesh)
    }
}

/// Model vertices expressed in the scan frame, in millimetres:
/// `u` along the laser line, `v` along the travel direction, `z` up.
/// The object rests on z = 0, is centered in `u` and starts at `v = 0`.
pub fn to_scan_frame(mesh: &Mesh, s: &SliceSettings) -> Vec<[f32; 3]> {
    let (up, up_sign) = s.up_axis.index_sign();
    let travel = s.travel_axis_index();
    let along = 3 - up - travel;
    let scale = s.unit.mm() * s.scale;
    let (sin, cos) = s.yaw_deg.to_radians().sin_cos();
    let along_sign = if s.flip_along { -1.0 } else { 1.0 };

    let mut pts: Vec<[f32; 3]> = mesh
        .vertices
        .iter()
        .map(|p| {
            let u = p[along] * along_sign;
            let v = p[travel];
            let z = p[up] * up_sign;
            // Yaw rotates the object on the conveyor, about the up axis.
            [(u * cos - v * sin) * scale, (u * sin + v * cos) * scale, z * scale]
        })
        .collect();

    let (lo, hi) = bounds(&pts);
    let shift = [-0.5 * (lo[0] + hi[0]), -lo[1], -lo[2]];
    for p in &mut pts {
        for k in 0..3 {
            p[k] += shift[k];
        }
    }
    pts
}

/// Whether [`to_scan_frame`] turns the model into its mirror image: the determinant of its
/// axis mapping (model axes → u, v, z with their signs) is negative. Yaw is a rotation and
/// the scale is positive, so neither changes it.
pub fn scan_frame_mirrored(s: &SliceSettings) -> bool {
    let (up, up_sign) = s.up_axis.index_sign();
    let travel = s.travel_axis_index();
    let along = 3 - up - travel;
    // Sign of the permutation (along, travel, up) of (0, 1, 2).
    let even = matches!((along, travel, up), (0, 1, 2) | (1, 2, 0) | (2, 0, 1));
    let along_sign = if s.flip_along { -1.0 } else { 1.0 };
    let det = if even { 1.0 } else { -1.0 } * along_sign * up_sign;
    det < 0.0
}

impl UpAxis {
    /// Model axis index and sign that become scan-frame +z.
    pub fn index_sign(self) -> (usize, f32) {
        match self {
            UpAxis::PosX => (0, 1.0),
            UpAxis::NegX => (0, -1.0),
            UpAxis::PosY => (1, 1.0),
            UpAxis::NegY => (1, -1.0),
            UpAxis::PosZ => (2, 1.0),
            UpAxis::NegZ => (2, -1.0),
        }
    }
}

/// Axis-aligned box from `lo` to `hi` as 12 triangles (test helper).
#[cfg(any(test, feature = "test-utils"))]
pub fn box_mesh(lo: [f32; 3], hi: [f32; 3]) -> Mesh {
    let c = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
    let quads = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
    let mut b = Builder::default();
    for q in quads {
        b.push_triangle([c(q[0]), c(q[1]), c(q[2])]);
        b.push_triangle([c(q[0]), c(q[2]), c(q[3])]);
    }
    b.mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Axis, ModelUnit};

    #[test]
    fn scan_frame_mapping() {
        // 10 (x) × 20 (y) × 30 (z) box, Y up, travel along Z, in cm.
        let m = box_mesh([5.0, 0.0, 0.0], [15.0, 20.0, 30.0]);
        let s = SliceSettings { up_axis: UpAxis::PosY, travel_axis: Axis::Z, unit: ModelUnit::Cm, ..Default::default() };
        let (lo, hi) = bounds(&to_scan_frame(&m, &s));
        assert_eq!(lo, [-50.0, 0.0, 0.0]);
        assert_eq!(hi, [50.0, 300.0, 200.0]);

        // Upside down: still rests on z = 0.
        let s = SliceSettings { up_axis: UpAxis::NegY, travel_axis: Axis::Z, ..Default::default() };
        let (lo, hi) = bounds(&to_scan_frame(&m, &s));
        assert_eq!((lo[2], hi[2]), (0.0, 20.0));

        // 90° yaw swaps the along / travel extents.
        let s = SliceSettings { yaw_deg: 90.0, ..Default::default() };
        let (lo, hi) = bounds(&to_scan_frame(&m, &s));
        assert!((hi[0] - lo[0] - 20.0).abs() < 1e-3);
        assert!((hi[1] - lo[1] - 10.0).abs() < 1e-3);
    }

    /// The determinant of the transform `to_scan_frame` actually applies, from where it
    /// sends the unit vectors (differences cancel the shift).
    fn applied_det(s: &SliceSettings) -> f32 {
        let m = Mesh { vertices: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], ..Default::default() };
        let p = to_scan_frame(&m, s);
        let col = |k: usize| [p[k][0] - p[0][0], p[k][1] - p[0][1], p[k][2] - p[0][2]];
        let (a, b, c) = (col(1), col(2), col(3));
        a[0] * (b[1] * c[2] - b[2] * c[1]) - b[0] * (a[1] * c[2] - a[2] * c[1]) + c[0] * (a[1] * b[2] - a[2] * b[1])
    }

    #[test]
    fn mirrored_scan_frames() {
        let cases = [
            (SliceSettings::default(), false),
            (SliceSettings { travel_axis: Axis::X, ..Default::default() }, true),
            (SliceSettings { flip_along: true, ..Default::default() }, true),
            (SliceSettings { travel_axis: Axis::X, flip_along: true, ..Default::default() }, false),
            (SliceSettings { up_axis: UpAxis::NegZ, ..Default::default() }, true),
            (SliceSettings { up_axis: UpAxis::PosY, travel_axis: Axis::Z, ..Default::default() }, true),
            (SliceSettings { up_axis: UpAxis::PosY, travel_axis: Axis::X, yaw_deg: 30.0, ..Default::default() }, false),
        ];
        for (s, mirrored) in cases {
            assert_eq!(scan_frame_mirrored(&s), mirrored, "{s:?}");
            assert_eq!(applied_det(&s) < 0.0, mirrored, "{s:?}");
        }
    }
}
