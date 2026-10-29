//! glTF 2.0, as `.gltf` JSON with `data:` buffers or as a `.glb` container: the
//! triangle primitives of the scene's node tree, placed by the node transforms.
//! glTF is always in metres.

use crate::mesh::{Builder, Mesh};
use crate::scan::ModelUnit;
use base64::Engine;
use glam::{Mat4, Quat, Vec3};
use serde_json::Value;

/// Node trees deeper than this are taken for a reference cycle.
const MAX_DEPTH: usize = 64;

pub(super) fn parse_json(bytes: &[u8]) -> Result<Mesh, String> {
    let doc: Value = serde_json::from_slice(bytes).map_err(|e| format!("invalid JSON: {e}"))?;
    build(&doc, None)
}

pub(super) fn parse_glb(bytes: &[u8]) -> Result<Mesh, String> {
    let u32_at = |o: usize| bytes.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if !bytes.starts_with(b"glTF") {
        return Err("not a GLB file".into());
    }
    if u32_at(4) != Some(2) {
        return Err(format!("unsupported version {}", u32_at(4).unwrap_or(0)));
    }
    let (mut json, mut bin) = (None, None);
    let mut o = 12;
    while let (Some(len), Some(kind)) = (u32_at(o), u32_at(o + 4)) {
        let data = bytes.get(o + 8..o + 8 + len as usize).ok_or("truncated chunk")?;
        match kind {
            0x4E4F_534A => json = Some(data),
            0x004E_4942 => bin = Some(data),
            _ => {}
        }
        o += 8 + len as usize;
    }
    let doc: Value = serde_json::from_slice(json.ok_or("no JSON chunk")?).map_err(|e| format!("invalid JSON: {e}"))?;
    build(&doc, bin)
}

fn build(doc: &Value, bin: Option<&[u8]>) -> Result<Mesh, String> {
    let list = |key: &str| doc.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    if let Some(ext) = list("extensionsRequired").iter().filter_map(Value::as_str).next() {
        return Err(match ext {
            "KHR_draco_mesh_compression" | "EXT_meshopt_compression" => format!("compressed meshes ({ext}) are not supported"),
            "KHR_mesh_quantization" => "quantized meshes (KHR_mesh_quantization) are not supported".into(),
            _ => format!("required extension {ext} is not supported"),
        });
    }
    let buffers = list("buffers").iter().enumerate().map(|(i, b)| buffer(i, b, bin)).collect::<Result<Vec<_>, _>>()?;
    let doc = Doc { doc, buffers };
    let nodes = list("nodes");
    // The default scene's root nodes; without scenes every node that is no one's child.
    let roots: Vec<usize> = match list("scenes").get(index(doc.doc.get("scene")).unwrap_or(0)) {
        Some(scene) => scene.get("nodes").and_then(Value::as_array).into_iter().flatten().filter_map(|n| index(Some(n))).collect(),
        None => {
            let children: Vec<usize> = nodes.iter().flat_map(|n| n.get("children").and_then(Value::as_array).into_iter().flatten()).filter_map(|c| index(Some(c))).collect();
            (0..nodes.len()).filter(|i| !children.contains(i)).collect()
        }
    };
    let mut b = Builder::default();
    for root in roots {
        doc.node(root, Mat4::IDENTITY, 0, &mut b)?;
    }
    b.finish(Some(ModelUnit::M))
}

fn index(v: Option<&Value>) -> Option<usize> {
    v?.as_u64().and_then(|i| usize::try_from(i).ok())
}

/// The content of buffer `i`: the GLB's binary chunk or a `data:` URI.
fn buffer(i: usize, b: &Value, bin: Option<&[u8]>) -> Result<Vec<u8>, String> {
    match b.get("uri").and_then(Value::as_str) {
        None => bin.map(<[u8]>::to_vec).ok_or_else(|| format!("buffer {i} has no data")),
        Some(uri) if uri.starts_with("data:") => {
            let data = uri.split_once(";base64,").ok_or_else(|| format!("buffer {i}: data URI is not base64"))?.1;
            base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| format!("buffer {i}: {e}"))
        }
        Some(uri) => Err(format!("buffer {i} is a separate file ({uri}); export the model as .glb")),
    }
}

struct Doc<'a> {
    doc: &'a Value,
    buffers: Vec<Vec<u8>>,
}

impl Doc<'_> {
    fn item(&self, key: &str, i: usize) -> Result<&Value, String> {
        self.doc.get(key).and_then(|l| l.get(i)).ok_or_else(|| format!("{key}[{i}] is missing"))
    }

    fn node(&self, i: usize, parent: Mat4, depth: usize, b: &mut Builder) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("nodes nested too deeply".into());
        }
        let node = self.item("nodes", i)?;
        let floats = |key: &str| node.get(key).and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect::<Vec<_>>());
        let local = match floats("matrix") {
            Some(m) => Mat4::from_cols_array(&m.try_into().map_err(|_| format!("nodes[{i}]: matrix needs 16 numbers"))?),
            None => {
                let t = floats("translation").map_or(Vec3::ZERO, |v| Vec3::new(v[0], v[1], v[2]));
                let r = floats("rotation").map_or(Quat::IDENTITY, |v| Quat::from_xyzw(v[0], v[1], v[2], v[3]));
                let s = floats("scale").map_or(Vec3::ONE, |v| Vec3::new(v[0], v[1], v[2]));
                Mat4::from_scale_rotation_translation(s, r, t)
            }
        };
        let world = parent * local;
        if let Some(mesh) = index(node.get("mesh")) {
            self.mesh(mesh, world, b)?;
        }
        for child in node.get("children").and_then(Value::as_array).into_iter().flatten() {
            self.node(index(Some(child)).ok_or("bad child index")?, world, depth + 1, b)?;
        }
        Ok(())
    }

    fn mesh(&self, i: usize, world: Mat4, b: &mut Builder) -> Result<(), String> {
        let primitives = self.item("meshes", i)?.get("primitives").and_then(Value::as_array).into_iter().flatten();
        for p in primitives {
            if p.get("extensions").is_some_and(|e| e.get("KHR_draco_mesh_compression").is_some()) {
                return Err("Draco-compressed meshes are not supported".into());
            }
            let mode = p.get("mode").and_then(Value::as_u64).unwrap_or(4);
            if !(4..=6).contains(&mode) {
                continue; // points and lines
            }
            let position = index(p.get("attributes").and_then(|a| a.get("POSITION"))).ok_or_else(|| format!("meshes[{i}]: primitive without POSITION"))?;
            let points: Vec<[f32; 3]> = self.positions(position)?.into_iter().map(|v| world.transform_point3(Vec3::from(v)).to_array()).collect();
            let indices = match index(p.get("indices")) {
                Some(a) => self.indices(a)?,
                None => (0..points.len() as u32).collect(),
            };
            let corner = |k: u32| points.get(k as usize).copied().ok_or_else(|| format!("meshes[{i}]: index {k} out of range"));
            let n = indices.len();
            let tris: Box<dyn Iterator<Item = [u32; 3]>> = match mode {
                4 => Box::new(indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]])),
                // Strips alternate their winding; fans share the first index.
                5 => Box::new((2..n).map(|k| if k % 2 == 0 { [indices[k - 2], indices[k - 1], indices[k]] } else { [indices[k - 1], indices[k - 2], indices[k]] })),
                _ => Box::new((2..n).map(|k| [indices[0], indices[k - 1], indices[k]])),
            };
            for t in tris {
                b.push_triangle([corner(t[0])?, corner(t[1])?, corner(t[2])?]);
            }
        }
        Ok(())
    }

    /// The bytes of accessor `i`'s elements, each `size` bytes, and their count.
    fn accessor(&self, i: usize, kind: &str, sizes: &[(u64, usize)]) -> Result<(Vec<&[u8]>, u64), String> {
        let a = self.item("accessors", i)?;
        let what = format!("accessors[{i}]");
        if a.get("sparse").is_some() {
            return Err(format!("{what}: sparse accessors are not supported"));
        }
        if a.get("type").and_then(Value::as_str) != Some(kind) {
            return Err(format!("{what}: expected type {kind}"));
        }
        let ty = a.get("componentType").and_then(Value::as_u64).unwrap_or(0);
        let &(_, size) = sizes.iter().find(|(t, _)| *t == ty).ok_or_else(|| format!("{what}: component type {ty} is not supported"))?;
        let count = a.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
        let view = self.item("bufferViews", index(a.get("bufferView")).ok_or_else(|| format!("{what}: no buffer view"))?)?;
        let buffer = self.buffers.get(index(view.get("buffer")).unwrap_or(usize::MAX)).ok_or_else(|| format!("{what}: bad buffer"))?;
        let start = index(view.get("byteOffset")).unwrap_or(0) + index(a.get("byteOffset")).unwrap_or(0);
        let stride = index(view.get("byteStride")).unwrap_or(size);
        let elements = (0..count)
            .map(|k| buffer.get(start + k * stride..start + k * stride + size))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| format!("{what}: past the end of its buffer"))?;
        Ok((elements, ty))
    }

    fn positions(&self, i: usize) -> Result<Vec<[f32; 3]>, String> {
        let (elements, _) = self.accessor(i, "VEC3", &[(5126, 12)])?;
        let f = |e: &[u8], k: usize| f32::from_le_bytes(e[k * 4..k * 4 + 4].try_into().unwrap());
        Ok(elements.into_iter().map(|e| [f(e, 0), f(e, 1), f(e, 2)]).collect())
    }

    fn indices(&self, i: usize) -> Result<Vec<u32>, String> {
        let (elements, ty) = self.accessor(i, "SCALAR", &[(5121, 1), (5123, 2), (5125, 4)])?;
        Ok(elements
            .into_iter()
            .map(|e| match ty {
                5121 => e[0] as u32,
                5123 => u16::from_le_bytes([e[0], e[1]]) as u32,
                _ => u32::from_le_bytes(e.try_into().unwrap()),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit square at z = 0 as 4 positions and 6 u16 indices, padded to 4 bytes.
    fn square_buffer() -> Vec<u8> {
        let mut b = Vec::new();
        for p in [[0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in p {
                b.extend(c.to_le_bytes());
            }
        }
        for i in [0u16, 1, 2, 0, 2, 3] {
            b.extend(i.to_le_bytes());
        }
        b
    }

    fn doc(uri: Option<&str>) -> String {
        let uri = uri.map_or(String::new(), |u| format!(r#""uri": "{u}","#));
        format!(
            r#"{{"asset": {{"version": "2.0"}}, "scene": 0, "scenes": [{{"nodes": [0]}}],
            "nodes": [{{"translation": [0, 0, 1], "children": [1]}}, {{"mesh": 0, "scale": [2, 3, 1], "rotation": [0, 0, 0, 1]}}],
            "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0}}, "indices": 1}}]}}],
            "accessors": [{{"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3"}},
                          {{"bufferView": 1, "componentType": 5123, "count": 6, "type": "SCALAR"}}],
            "bufferViews": [{{"buffer": 0, "byteLength": 48}}, {{"buffer": 0, "byteOffset": 48, "byteLength": 12}}],
            "buffers": [{{{uri} "byteLength": 60}}]}}"#
        )
    }

    fn check(m: Mesh) {
        assert_eq!(m.unit, Some(ModelUnit::M));
        assert_eq!(m.triangles.len(), 2);
        assert_eq!(m.bounds(), ([0.0, 0.0, 1.0], [2.0, 3.0, 1.0]));
    }

    #[test]
    fn embedded_buffer() {
        let uri = format!("data:application/octet-stream;base64,{}", base64::engine::general_purpose::STANDARD.encode(square_buffer()));
        check(parse_json(doc(Some(&uri)).as_bytes()).unwrap());
    }

    #[test]
    fn glb() {
        let mut json = doc(None).into_bytes();
        json.resize(json.len().div_ceil(4) * 4, b' ');
        let bin = square_buffer();
        let mut glb = b"glTF".to_vec();
        glb.extend(2u32.to_le_bytes());
        glb.extend(((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        glb.extend((json.len() as u32).to_le_bytes());
        glb.extend(0x4E4F_534Au32.to_le_bytes());
        glb.extend(&json);
        glb.extend((bin.len() as u32).to_le_bytes());
        glb.extend(0x004E_4942u32.to_le_bytes());
        glb.extend(&bin);
        check(parse_glb(&glb).unwrap());
    }

    #[test]
    fn errors() {
        assert!(parse_json(doc(Some("square.bin")).as_bytes()).unwrap_err().contains(".glb"));
        let draco = r#"{"extensionsRequired": ["KHR_draco_mesh_compression"]}"#;
        assert!(parse_json(draco.as_bytes()).unwrap_err().contains("compressed"));
        assert!(parse_glb(b"glTF\x01\0\0\0").unwrap_err().contains("version"));
    }
}
