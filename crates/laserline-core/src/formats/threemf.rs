//! 3MF: the mesh objects placed by the root model's build items, including
//! components and, from the production extension, components in other model files
//! of the package (`p:path`, as Bambu Studio and PrusaSlicer write them). Support
//! objects are left out.

use super::zip::Zip;
use crate::mesh::{Builder, Mesh};
use glam::{Affine3A, Vec3};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use std::collections::HashMap;

/// Components may nest; deeper than this is taken for a reference cycle.
const MAX_DEPTH: usize = 32;

#[derive(Default)]
struct ModelFile {
    /// The `unit` attribute of `<model>`.
    unit: Option<String>,
    objects: HashMap<u32, Object>,
    /// Build items: object id and transform.
    build: Vec<(u32, Affine3A)>,
}

#[derive(Default)]
struct Object {
    support: bool,
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    components: Vec<Component>,
}

struct Component {
    object: u32,
    /// Model file of the object (production extension); `None` for the same file.
    path: Option<String>,
    transform: Affine3A,
}

pub(super) fn parse(bytes: &[u8]) -> Result<Mesh, String> {
    let zip = Zip::open(bytes)?;
    let root = root_path(&zip)?;
    let mut files = HashMap::new();
    let model = read_model(&zip, &root)?;
    let mm = match model.unit.as_deref().unwrap_or("millimeter") {
        "micron" => 0.001,
        "millimeter" => 1.0,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        other => return Err(format!("unknown unit “{other}”")),
    };
    let (unit, scale) = super::unit_and_scale(mm);
    if model.build.is_empty() {
        return Err("no build items".into());
    }
    let build = model.build.clone();
    files.insert(root.clone(), model);
    let mut builder = Builder::default();
    for (object, transform) in build {
        emit(&zip, &mut files, &root, object, Affine3A::from_scale(Vec3::splat(scale)) * transform, 0, &mut builder)?;
    }
    builder.finish(Some(unit))
}

/// Adds object `id` of model file `path`, placed by `transform`, to `b`.
fn emit(zip: &Zip, files: &mut HashMap<String, ModelFile>, path: &str, id: u32, transform: Affine3A, depth: usize, b: &mut Builder) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("components nested too deeply".into());
    }
    if !files.contains_key(path) {
        files.insert(path.to_owned(), read_model(zip, path)?);
    }
    let object = files[path].objects.get(&id).ok_or_else(|| format!("{path}: no object {id}"))?;
    if object.support {
        return Ok(());
    }
    let place = |v: [f32; 3]| transform.transform_point3(Vec3::from(v)).to_array();
    for t in &object.triangles {
        let corner = |i: u32| object.vertices.get(i as usize).copied().ok_or_else(|| format!("{path}: object {id}: vertex {i} out of range"));
        b.push_triangle([place(corner(t[0])?), place(corner(t[1])?), place(corner(t[2])?)]);
    }
    let children: Vec<_> = object.components.iter().map(|c| (c.object, c.path.clone(), c.transform)).collect();
    for (child, child_path, child_transform) in children {
        let child_path = child_path.unwrap_or_else(|| path.to_owned());
        emit(zip, files, &child_path, child, transform * child_transform, depth + 1, b)?;
    }
    Ok(())
}

/// Path of the root model part, from the package relationships.
fn root_path(zip: &Zip) -> Result<String, String> {
    const DEFAULT: &str = "3D/3dmodel.model";
    let Some(rels) = zip.file("_rels/.rels")? else {
        return Ok(DEFAULT.into());
    };
    let text = String::from_utf8_lossy(&rels);
    let mut reader = quick_xml::Reader::from_str(&text);
    loop {
        match reader.read_event().map_err(|e| format!("_rels/.rels: {e}"))? {
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                let attrs = attributes(&e)?;
                if attrs.get("Type").is_some_and(|t| t.ends_with("/3dmodel"))
                    && let Some(target) = attrs.get("Target")
                {
                    return Ok(target.trim_start_matches('/').to_owned());
                }
            }
            Event::Eof => return Ok(DEFAULT.into()),
            _ => {}
        }
    }
}

/// The attributes of `e` by local name.
fn attributes(e: &BytesStart) -> Result<HashMap<String, String>, String> {
    let mut out = HashMap::new();
    for a in e.attributes() {
        let a = a.map_err(|e| e.to_string())?;
        let value = a.normalized_value(XmlVersion::Implicit1_0).map_err(|e| e.to_string())?;
        out.insert(String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned(), value.into_owned());
    }
    Ok(out)
}

fn number<T: std::str::FromStr>(attrs: &HashMap<String, String>, name: &str) -> Result<T, String> {
    let s = attrs.get(name).ok_or_else(|| format!("missing attribute {name}"))?;
    s.trim().parse().map_err(|_| format!("bad {name} “{s}”"))
}

/// A `transform` attribute (3×4, row vectors) or identity.
fn transform(attrs: &HashMap<String, String>) -> Result<Affine3A, String> {
    let Some(s) = attrs.get("transform") else {
        return Ok(Affine3A::IDENTITY);
    };
    let m: Vec<f32> = s.split_ascii_whitespace().map(str::parse).collect::<Result<_, _>>().map_err(|_| format!("bad transform “{s}”"))?;
    let m: [f32; 12] = m.try_into().map_err(|_| format!("transform needs 12 numbers: “{s}”"))?;
    // Rows m00 m01 m02 / m10 … / translation: each row is where a unit axis goes.
    Ok(Affine3A::from_cols_array(&m))
}

fn read_model(zip: &Zip, path: &str) -> Result<ModelFile, String> {
    let bytes = zip.file(path)?.ok_or_else(|| format!("{path} is missing"))?;
    let text = String::from_utf8_lossy(&bytes);
    read_model_xml(&text).map_err(|e| format!("{path}: {e}"))
}

fn read_model_xml(text: &str) -> Result<ModelFile, String> {
    let mut reader = quick_xml::Reader::from_str(text);
    let mut model = ModelFile::default();
    let mut current: Option<(u32, Object)> = None;
    loop {
        let event = reader.read_event().map_err(|e| e.to_string())?;
        let (e, empty) = match &event {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(e) if e.local_name().as_ref() == b"object" => {
                if let Some((id, object)) = current.take() {
                    model.objects.insert(id, object);
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let a = || attributes(e);
        match e.local_name().as_ref() {
            b"model" => model.unit = a()?.get("unit").cloned(),
            b"object" => {
                let attrs = a()?;
                let support = matches!(attrs.get("type").map(String::as_str), Some("support" | "solidsupport"));
                let object = Object { support, ..Default::default() };
                if empty {
                    model.objects.insert(number(&attrs, "id")?, object);
                } else {
                    current = Some((number(&attrs, "id")?, object));
                }
            }
            b"vertex" => {
                if let Some((_, o)) = &mut current {
                    let attrs = a()?;
                    o.vertices.push([number(&attrs, "x")?, number(&attrs, "y")?, number(&attrs, "z")?]);
                }
            }
            b"triangle" => {
                if let Some((_, o)) = &mut current {
                    let attrs = a()?;
                    o.triangles.push([number(&attrs, "v1")?, number(&attrs, "v2")?, number(&attrs, "v3")?]);
                }
            }
            b"component" => {
                if let Some((_, o)) = &mut current {
                    let attrs = a()?;
                    let path = attrs.get("path").map(|p| p.trim_start_matches('/').to_owned());
                    o.components.push(Component { object: number(&attrs, "objectid")?, path, transform: transform(&attrs)? });
                }
            }
            b"item" => {
                let attrs = a()?;
                model.build.push((number(&attrs, "objectid")?, transform(&attrs)?));
            }
            _ => {}
        }
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::super::zip;
    use super::*;
    use crate::scan::ModelUnit;

    const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#;

    /// A unit cube object with id `id`.
    fn cube(id: u32) -> String {
        let mut s = format!(r#"<object id="{id}" type="model"><mesh><vertices>"#);
        for i in 0..8 {
            s += &format!(r#"<vertex x="{}" y="{}" z="{}"/>"#, i & 1, (i >> 1) & 1, (i >> 2) & 1);
        }
        s += "</vertices><triangles>";
        for [a, b, c, d] in [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]] {
            s += &format!(r#"<triangle v1="{a}" v2="{b}" v3="{c}"/><triangle v1="{a}" v2="{c}" v3="{d}"/>"#);
        }
        s + "</triangles></mesh></object>"
    }

    fn model(unit: &str, resources: &str, build: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="{unit}" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"
 xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
 <resources>{resources}</resources><build>{build}</build></model>"#
        )
    }

    #[test]
    fn placed_cube_in_centimetres() {
        let root = model("centimeter", &cube(1), r#"<item objectid="1" transform="2 0 0 0 3 0 0 0 4 10 20 30"/>"#);
        for deflate in [false, true] {
            let z = zip::write(&[("_rels/.rels", RELS.as_bytes()), ("3D/3dmodel.model", root.as_bytes())], deflate);
            let m = parse(&z).unwrap();
            assert_eq!(m.unit, Some(ModelUnit::Cm));
            assert_eq!(m.triangles.len(), 12);
            assert_eq!(m.bounds(), ([10.0, 20.0, 30.0], [12.0, 23.0, 34.0]));
        }
    }

    #[test]
    fn components_across_files_in_microns() {
        // Bambu-style: the root holds an assembly whose parts live in another model file.
        let parts = model("micron", &(cube(1) + &cube(2)), "");
        let assembly = r#"<object id="5" type="model"><components>
            <component p:path="/3D/Objects/parts.model" objectid="1"/>
            <component p:path="/3D/Objects/parts.model" objectid="2" transform="1 0 0 0 1 0 0 0 1 4 0 0"/>
            </components></object><object id="6" type="support"><components><component objectid="5"/></components></object>"#;
        let root = model("micron", assembly, r#"<item objectid="5" transform="1000 0 0 0 1000 0 0 0 1000 0 0 0"/><item objectid="6"/>"#);
        let z = zip::write(&[("3D/3dmodel.model", root.as_bytes()), ("3D/Objects/parts.model", parts.as_bytes())], true);
        let m = parse(&z).unwrap();
        assert_eq!(m.unit, Some(ModelUnit::Mm), "micron is converted to mm");
        assert_eq!(m.triangles.len(), 24, "the support object is left out");
        let (lo, hi) = m.bounds();
        assert_eq!(lo, [0.0; 3]);
        assert!((hi[0] - 5.0).abs() < 1e-5 && (hi[1] - 1.0).abs() < 1e-5 && (hi[2] - 1.0).abs() < 1e-5, "{hi:?}");
    }

    #[test]
    fn errors() {
        let cyclic = model("millimeter", r#"<object id="1"><components><component objectid="1"/></components></object>"#, r#"<item objectid="1"/>"#);
        let z = zip::write(&[("3D/3dmodel.model", cyclic.as_bytes())], false);
        assert!(parse(&z).unwrap_err().contains("nested"));
        let none = model("millimeter", &cube(1), "");
        assert!(parse(&zip::write(&[("3D/3dmodel.model", none.as_bytes())], false)).unwrap_err().contains("no build items"));
        assert!(parse(&zip::write(&[("x", b"")], false)).unwrap_err().contains("missing"));
    }
}
