//! STL, binary and ASCII.
//!
//! Parsed here rather than with a crate because many CAD exporters write binary
//! files whose 80-byte header starts with "solid", which naive format probes
//! mistake for ASCII. The binary size check is unambiguous.

use crate::mesh::{Builder, Mesh};

pub(super) fn parse(bytes: &[u8]) -> Result<Mesh, String> {
    let mut builder = Builder::default();
    if is_binary(bytes) {
        parse_binary(bytes, &mut builder)?;
    } else {
        parse_ascii(bytes, &mut builder)?;
    }
    builder.finish(None)
}

fn is_binary(bytes: &[u8]) -> bool {
    if bytes.len() < 84 {
        return false;
    }
    let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    let expected = 84usize.saturating_add(count.saturating_mul(50));
    if expected == bytes.len() {
        return true;
    }
    // Not an exact binary size: ASCII if it looks like text, else binary with trailing junk.
    let text_start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
    !bytes[text_start..].starts_with(b"solid") && expected < bytes.len()
}

fn parse_binary(bytes: &[u8], b: &mut Builder) -> Result<(), String> {
    let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    let f = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    for t in 0..count {
        let base = 84 + t * 50 + 12; // skip the stored normal
        if base + 36 > bytes.len() {
            return Err(format!("binary file truncated at triangle {t} of {count}"));
        }
        let v = |i: usize| [f(base + i * 12), f(base + i * 12 + 4), f(base + i * 12 + 8)];
        b.push_triangle([v(0), v(1), v(2)]);
    }
    Ok(())
}

fn parse_ascii(bytes: &[u8], b: &mut Builder) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "neither an ASCII nor a binary STL file")?;
    let mut tokens = text.split_ascii_whitespace();
    if tokens.next() != Some("solid") {
        return Err("neither an ASCII nor a binary STL file".into());
    }
    let mut corners = Vec::with_capacity(3);
    while let Some(tok) = tokens.next() {
        match tok {
            "vertex" => {
                let mut v = [0.0f32; 3];
                for c in &mut v {
                    let s = tokens.next().ok_or("unexpected end of file in vertex")?;
                    *c = s.parse().map_err(|_| format!("bad number “{s}”"))?;
                }
                corners.push(v);
            }
            "endloop" => {
                // Polygons with more than three corners are fanned into triangles.
                b.push_polygon(&corners);
                corners.clear();
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::mesh::{Mesh, box_mesh, parse_stl};

    fn to_binary(m: &Mesh) -> Vec<u8> {
        let mut out = b"solid but actually binary".to_vec();
        out.resize(80, 0);
        out.extend((m.triangles.len() as u32).to_le_bytes());
        for t in &m.triangles {
            out.extend([0u8; 12]);
            for &i in t {
                for c in m.vertices[i as usize] {
                    out.extend(c.to_le_bytes());
                }
            }
            out.extend([0u8; 2]);
        }
        out
    }

    #[test]
    fn ascii_stl() {
        let src = "solid test\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 1e0\n  endloop\n endfacet\n\
                   facet normal 0 0 1\n  outer loop\n   vertex 1 0 0\n   vertex 1 1 0\n   vertex 0 1 1\n  endloop\n endfacet\nendsolid test\n";
        let m = parse_stl(src.as_bytes()).unwrap();
        assert_eq!(m.triangles.len(), 2);
        assert_eq!(m.vertices.len(), 4, "shared vertices are merged");
        assert_eq!(m.bounds(), ([0.0; 3], [1.0, 1.0, 1.0]));
    }

    #[test]
    fn binary_stl_with_solid_header() {
        let m = box_mesh([0.0; 3], [1.0, 2.0, 3.0]);
        let parsed = parse_stl(&to_binary(&m)).unwrap();
        assert_eq!(parsed.triangles.len(), 12);
        assert_eq!(parsed.vertices.len(), 8);
        assert_eq!(parsed.bounds(), ([0.0; 3], [1.0, 2.0, 3.0]));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_stl(b"hello").is_err());
        assert!(parse_stl(b"solid x\nendsolid x\n").is_err());
    }
}
