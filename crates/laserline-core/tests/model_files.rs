//! The L-block in every model format (`models/`, see its README) reads as the same
//! object and slices like the STL.

use laserline_core::formats::{self, ModelFormat};
use laserline_core::scan::{self, ObjectBounds, Progress, SliceSettings};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn models() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models");
    dir.is_dir().then_some(dir).or_else(|| {
        eprintln!("models/ not found (outside the repository): skipped");
        None
    })
}

/// The object bounds of the file sliced with the unit it declares.
fn sliced(file: &str) -> (Option<ObjectBounds>, usize) {
    let bytes = std::fs::read(models().unwrap().join(file)).unwrap();
    let mesh = formats::parse_model(&bytes, file).unwrap_or_else(|e| panic!("{file}: {e}"));
    let unit = mesh.unit.unwrap_or(scan::ModelUnit::Mm);
    let (lo, hi) = mesh.bounds();
    let mm = |p: [f32; 3]| p.map(|c| c * unit.mm());
    for (got, want) in mm(lo).into_iter().zip([0.0, 0.0, 0.0]).chain(mm(hi).into_iter().zip([80.0, 60.0, 20.0])) {
        assert!((got - want).abs() < 1e-3, "{file}: bounds {:?}..{:?}", mm(lo), mm(hi));
    }
    let settings = SliceSettings { unit, lateral_res_mm: 1.0, travel_step_mm: 1.0, ..Default::default() };
    let scan = scan::slice(&mesh, &settings, file, &AtomicBool::new(false), &Progress::new(0)).unwrap();
    (scan.object_bounds(), mesh.triangles.len())
}

#[test]
fn l_block_in_every_format() {
    if models().is_none() {
        return;
    }
    let (stl, triangles) = sliced("l-block.stl");
    let mut files = vec!["l-block.obj", "l-block.ply", "l-block.glb", "l-block.3mf"];
    if cfg!(feature = "step") {
        files.push("l-block.step");
    }
    for file in files {
        assert!(ModelFormat::from_name(file).is_some());
        let (bounds, n) = sliced(file);
        assert_eq!(bounds, stl, "{file}");
        if !file.ends_with(".step") {
            assert_eq!(n, triangles, "{file}: same triangles as the STL");
        }
    }
}
