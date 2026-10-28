//! Wavefront OBJ: `v` positions and `f` faces; everything else is ignored.

use crate::mesh::{Builder, Mesh};

pub(super) fn parse(bytes: &[u8]) -> Result<Mesh, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut builder = Builder::default();
    let mut corners = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let at = |e: String| format!("line {}: {e}", n + 1);
        let mut tokens = line.split_ascii_whitespace();
        match tokens.next() {
            Some("v") => {
                let mut p = [0.0f32; 3];
                for c in &mut p {
                    let s = tokens.next().ok_or_else(|| at("vertex needs three coordinates".into()))?;
                    *c = s.parse().map_err(|_| at(format!("bad number “{s}”")))?;
                }
                positions.push(p);
            }
            Some("f") => {
                corners.clear();
                for tok in tokens {
                    // `i`, `i/t`, `i//n` or `i/t/n`; negative indices count back from the end.
                    let s = tok.split('/').next().unwrap_or("");
                    let i: i64 = s.parse().map_err(|_| at(format!("bad face index “{tok}”")))?;
                    let index = if i < 0 { positions.len() as i64 + i } else { i - 1 };
                    let p = usize::try_from(index).ok().and_then(|i| positions.get(i));
                    corners.push(*p.ok_or_else(|| at(format!("face index {i} out of range")))?);
                }
                builder.push_polygon(&corners);
            }
            _ => {}
        }
    }
    builder.finish(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_of_every_kind() {
        let src = "# a unit cube's bottom and one side\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvn 0 0 1\n\
                   o side\nv 0 0 1 1.0\nv 1 0 1\n\
                   f 1/1/1 2/1/1 3/1/1 4/1/1\nf -6//1 -5//1 -1//1 -2//1\ns off\n";
        let m = parse(src.as_bytes()).unwrap();
        assert_eq!(m.triangles.len(), 4, "two quads, fanned");
        assert_eq!(m.vertices.len(), 6);
        assert_eq!(m.bounds(), ([0.0; 3], [1.0, 1.0, 1.0]));
        assert_eq!(m.unit, None);
    }

    #[test]
    fn bad_indices() {
        assert!(parse(b"v 0 0 0\nf 1 2 3\n").unwrap_err().contains("line 2"));
        assert!(parse(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n").is_err());
        assert!(parse(b"v 0 0 0\n").unwrap_err().contains("no triangles"));
    }
}
