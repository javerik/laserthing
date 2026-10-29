//! Reading 3D model files into a [`Mesh`]: STL, 3MF, OBJ, PLY, glTF/GLB and STEP.
//!
//! Formats that declare a length unit (3MF, glTF, STEP) set [`Mesh::unit`]; units
//! [`ModelUnit`] has no variant for are converted to millimetres in the mesh.
//! STEP needs the `step` feature, which tessellates its surfaces with truck.

use crate::mesh::Mesh;
use crate::scan::ModelUnit;

mod gltf;
mod obj;
mod ply;
#[cfg(feature = "step")]
mod step;
mod stl;
mod threemf;
mod zip;

/// A model file format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    /// STL, binary or ASCII.
    Stl,
    /// 3D Manufacturing Format (`.3mf`).
    ThreeMf,
    /// Wavefront OBJ.
    Obj,
    /// Stanford PLY, ASCII or binary.
    Ply,
    /// glTF 2.0 as JSON with embedded buffers.
    Gltf,
    /// glTF 2.0 binary container.
    Glb,
    /// STEP (ISO 10303-21) B-rep, tessellated on import.
    Step,
}

/// File extensions of all model formats, lower case and without the dot.
pub const EXTENSIONS: &[&str] = &["stl", "3mf", "obj", "ply", "gltf", "glb", "step", "stp"];

impl ModelFormat {
    /// Short display name, e.g. `"3MF"`.
    pub fn name(self) -> &'static str {
        match self {
            ModelFormat::Stl => "STL",
            ModelFormat::ThreeMf => "3MF",
            ModelFormat::Obj => "OBJ",
            ModelFormat::Ply => "PLY",
            ModelFormat::Gltf => "glTF",
            ModelFormat::Glb => "GLB",
            ModelFormat::Step => "STEP",
        }
    }

    /// The format of a file named `file_name`, by its extension (any case).
    pub fn from_name(file_name: &str) -> Option<Self> {
        let ext = file_name.rsplit_once('.')?.1.to_ascii_lowercase();
        Some(match ext.as_str() {
            "stl" => ModelFormat::Stl,
            "3mf" => ModelFormat::ThreeMf,
            "obj" => ModelFormat::Obj,
            "ply" => ModelFormat::Ply,
            "gltf" => ModelFormat::Gltf,
            "glb" => ModelFormat::Glb,
            "step" | "stp" => ModelFormat::Step,
            _ => return None,
        })
    }

    /// Guesses the format from the content; STL when nothing else matches (it has no magic).
    pub fn sniff(bytes: &[u8]) -> Self {
        let text = &bytes[bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0)..];
        if bytes.starts_with(b"PK\x03\x04") {
            ModelFormat::ThreeMf
        } else if bytes.starts_with(b"glTF") {
            ModelFormat::Glb
        } else if text.starts_with(b"ISO-10303-21") {
            ModelFormat::Step
        } else if text.starts_with(b"ply") {
            ModelFormat::Ply
        } else if text.starts_with(b"{") {
            ModelFormat::Gltf
        } else {
            ModelFormat::Stl
        }
    }
}

/// Parses a model file, its format chosen by the extension of `file_name` or, failing
/// that, by its content.
pub fn parse_model(bytes: &[u8], file_name: &str) -> Result<Mesh, String> {
    parse(bytes, ModelFormat::from_name(file_name).unwrap_or_else(|| ModelFormat::sniff(bytes)))
}

/// Parses a model file of a known format. Errors start with the format's name.
pub fn parse(bytes: &[u8], format: ModelFormat) -> Result<Mesh, String> {
    let mesh = match format {
        ModelFormat::Stl => stl::parse(bytes),
        ModelFormat::ThreeMf => threemf::parse(bytes),
        ModelFormat::Obj => obj::parse(bytes),
        ModelFormat::Ply => ply::parse(bytes),
        ModelFormat::Gltf => gltf::parse_json(bytes),
        ModelFormat::Glb => gltf::parse_glb(bytes),
        #[cfg(feature = "step")]
        ModelFormat::Step => step::parse(bytes),
        #[cfg(not(feature = "step"))]
        ModelFormat::Step => Err("STEP support is not built in; export the model as STL or 3MF".into()),
    };
    mesh.map_err(|e| format!("{}: {e}", format.name()))
}

/// A declared length unit as a [`ModelUnit`] and the factor to apply to the coordinates
/// first: 1 for the units `ModelUnit` has, a conversion to millimetres for the others.
fn unit_and_scale(mm_per_unit: f64) -> (ModelUnit, f32) {
    const UNITS: [ModelUnit; 4] = [ModelUnit::Mm, ModelUnit::Cm, ModelUnit::M, ModelUnit::Inch];
    match UNITS.into_iter().find(|u| (u.mm() as f64 - mm_per_unit).abs() < 1e-6 * mm_per_unit) {
        Some(unit) => (unit, 1.0),
        None => (ModelUnit::Mm, mm_per_unit as f32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_by_name_and_content() {
        assert_eq!(ModelFormat::from_name("Part.STP"), Some(ModelFormat::Step));
        assert_eq!(ModelFormat::from_name("a.b.3mf"), Some(ModelFormat::ThreeMf));
        assert_eq!(ModelFormat::from_name("readme"), None);
        assert_eq!(ModelFormat::sniff(b"PK\x03\x04rest"), ModelFormat::ThreeMf);
        assert_eq!(ModelFormat::sniff(b"\nISO-10303-21;"), ModelFormat::Step);
        assert_eq!(ModelFormat::sniff(b"solid x"), ModelFormat::Stl);
        for ext in EXTENSIONS {
            assert!(ModelFormat::from_name(&format!("x.{ext}")).is_some(), "{ext}");
        }
    }

    #[test]
    fn units() {
        assert_eq!(unit_and_scale(25.4), (ModelUnit::Inch, 1.0));
        assert_eq!(unit_and_scale(1000.0), (ModelUnit::M, 1.0));
        assert_eq!(unit_and_scale(0.001), (ModelUnit::Mm, 0.001));
        assert_eq!(unit_and_scale(304.8), (ModelUnit::Mm, 304.8));
    }

    #[test]
    fn errors_name_the_format() {
        assert!(parse_model(b"v 1 2", "x.obj").unwrap_err().starts_with("OBJ: "));
        #[cfg(not(feature = "step"))]
        assert!(parse_model(b"", "x.step").unwrap_err().contains("not built in"));
    }
}
