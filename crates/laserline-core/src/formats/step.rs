//! STEP (ISO 10303-21): the B-rep shells read by truck-stepio and tessellated by
//! truck-meshalgo. The chord tolerance follows the model's size. truck reads no units
//! and no assembly placements, so the length unit is looked up in the file here, and
//! the shells of an assembly stay where their own coordinates put them.

use super::unit_and_scale;
use crate::mesh::{Builder, Mesh};
use truck_meshalgo::prelude::*;
use truck_stepio::r#in::Table;

/// Chord tolerance as a fraction of the model's bounding-box diagonal.
const TOLERANCE: f64 = 5e-4;

pub(super) fn parse(bytes: &[u8]) -> Result<Mesh, String> {
    let text = String::from_utf8_lossy(bytes);
    if !text.trim_start().starts_with("ISO-10303-21") {
        return Err("not a STEP file".into());
    }
    let (unit, scale) = length_unit(&text).map(unit_and_scale).unzip();
    let scale = scale.unwrap_or(1.0);
    // truck panics on some geometry it can't handle; natively that must not take the app down.
    let shells = std::panic::catch_unwind(|| tessellate(&text))
        .map_err(|_| "the geometry could not be tessellated; export the model as STL or 3MF".to_string())??;
    let mut b = Builder::default();
    for poly in &shells {
        let positions = poly.positions();
        for face in poly.face_iter() {
            let corners: Vec<[f32; 3]> = face.iter().map(|v| positions[v.pos]).map(|p| [p.x, p.y, p.z].map(|c| c as f32 * scale)).collect();
            b.push_polygon(&corners);
        }
    }
    b.finish(unit).map_err(|_| "no surfaces could be tessellated; export the model as STL or 3MF".into())
}

fn tessellate(text: &str) -> Result<Vec<PolygonMesh>, String> {
    let table = Table::from_step(text).ok_or("the file could not be read; export the model as STL or 3MF")?;
    if table.shell.is_empty() {
        return Err("no solid or surface model found (only shells are read)".into());
    }
    let shells = table.shell.values().map(|s| table.to_compressed_shell(s).map_err(|e| format!("{e:?}"))).collect::<Result<Vec<_>, _>>()?;
    let mut bounds = BoundingBox::new();
    for s in &shells {
        s.vertices.iter().for_each(|p| bounds.push(*p));
    }
    let tolerance = (bounds.diameter() * TOLERANCE).max(1e-6);
    Ok(shells.iter().map(|s| s.robust_triangulation(tolerance).to_polygon()).collect())
}

/// Millimetres per unit of the file's length unit, if it declares one this knows.
fn length_unit(text: &str) -> Option<f64> {
    let data = &text[text.find("DATA;")?..];
    let entity = data.split(';').map(|e| e.split_whitespace().collect::<String>().to_ascii_uppercase()).find(|e| e.contains("LENGTH_UNIT()"))?;
    if let Some(si) = entity.split_once("SI_UNIT(").map(|(_, rest)| rest) {
        let prefix = si.split(',').next()?;
        return Some(match prefix {
            ".MICRO." => 0.001,
            ".MILLI." => 1.0,
            ".CENTI." => 10.0,
            ".DECI." => 100.0,
            "$" => 1000.0,
            ".KILO." => 1e6,
            _ => return None,
        });
    }
    let name = entity.split_once("CONVERSION_BASED_UNIT('")?.1.split('\'').next()?;
    match name {
        "INCH" => Some(25.4),
        "FOOT" => Some(304.8),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::ModelUnit;
    use truck_modeling::{Point3, Rad, Solid, Vector3, builder};
    use truck_stepio::out;

    fn step(solid: &Solid) -> String {
        out::CompleteStepDisplay::new(out::StepModel::from(&solid.compress()), Default::default()).to_string()
    }

    fn block(lo: [f64; 3], size: [f64; 3]) -> Solid {
        let v = builder::vertex(Point3::from(lo));
        let e = builder::tsweep(&v, Vector3::new(size[0], 0.0, 0.0));
        let f = builder::tsweep(&e, Vector3::new(0.0, size[1], 0.0));
        builder::tsweep(&f, Vector3::new(0.0, 0.0, size[2]))
    }

    #[test]
    fn box_in_millimetres() {
        let m = parse(step(&block([5.0, 0.0, 0.0], [80.0, 15.0, 6.0])).as_bytes()).unwrap();
        assert_eq!(m.unit, Some(ModelUnit::Mm));
        assert_eq!(m.triangles.len(), 12);
        assert_eq!(m.bounds(), ([5.0, 0.0, 0.0], [85.0, 15.0, 6.0]));
    }

    #[test]
    fn cylinder_is_tessellated() {
        let v = builder::vertex(Point3::new(10.0, 0.0, 0.0));
        let circle = builder::rsweep(&v, Point3::new(0.0, 0.0, 0.0), Vector3::unit_z(), Rad(7.0));
        let disk = builder::try_attach_plane(&[circle]).unwrap();
        let m = parse(step(&builder::tsweep(&disk, Vector3::new(0.0, 0.0, 20.0))).as_bytes()).unwrap();
        assert!(m.triangles.len() > 100, "{} triangles", m.triangles.len());
        let (lo, hi) = m.bounds();
        assert!((hi[0] - lo[0] - 20.0).abs() < 0.05 && (hi[2] - lo[2] - 20.0).abs() < 1e-3, "{lo:?} {hi:?}");
    }

    #[test]
    fn units() {
        let mm = step(&block([0.0; 3], [1.0, 1.0, 1.0]));
        let inch = mm.replace("SI_UNIT(.MILLI.,.METRE.)", "CONVERSION_BASED_UNIT('INCH',#99)");
        assert_eq!(parse(inch.as_bytes()).unwrap().unit, Some(ModelUnit::Inch));
        let micro = mm.replace(".MILLI.", ".MICRO.");
        let m = parse(micro.as_bytes()).unwrap();
        assert_eq!((m.unit, m.bounds().1), (Some(ModelUnit::Mm), [0.001; 3]));
        assert!(mm.contains("SI_UNIT(.MILLI.,.METRE.)"), "the writer's unit line changed");
    }

    #[test]
    fn errors() {
        assert!(parse(b"solid x").unwrap_err().contains("not a STEP file"));
        assert!(parse(b"ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n").is_err());
    }
}
