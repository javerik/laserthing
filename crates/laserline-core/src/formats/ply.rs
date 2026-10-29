//! Stanford PLY, ASCII and binary: the `vertex` element's x/y/z and the `face`
//! element's index list. Other elements and properties are skipped.

use crate::mesh::{Builder, Mesh};

#[derive(Clone, Copy, PartialEq)]
enum Encoding {
    Ascii,
    Little,
    Big,
}

#[derive(Clone, Copy)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "char" | "int8" => Scalar::I8,
            "uchar" | "uint8" => Scalar::U8,
            "short" | "int16" => Scalar::I16,
            "ushort" | "uint16" => Scalar::U16,
            "int" | "int32" => Scalar::I32,
            "uint" | "uint32" => Scalar::U32,
            "float" | "float32" => Scalar::F32,
            "double" | "float64" => Scalar::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }
}

enum Property {
    Scalar(String, Scalar),
    List(String, Scalar, Scalar),
}

struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

/// Reads values one at a time from the body, as text or binary.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
    encoding: Encoding,
}

impl Reader<'_> {
    fn read(&mut self, ty: Scalar) -> Result<f64, String> {
        if self.encoding == Encoding::Ascii {
            while self.bytes.get(self.pos).is_some_and(u8::is_ascii_whitespace) {
                self.pos += 1;
            }
            let start = self.pos;
            while self.bytes.get(self.pos).is_some_and(|b| !b.is_ascii_whitespace()) {
                self.pos += 1;
            }
            let s = std::str::from_utf8(&self.bytes[start..self.pos]).unwrap_or("");
            if s.is_empty() {
                return Err("unexpected end of file".into());
            }
            return s.parse().map_err(|_| format!("bad number “{s}”"));
        }
        let n = ty.size();
        let raw = self.bytes.get(self.pos..self.pos + n).ok_or("unexpected end of file")?;
        self.pos += n;
        let mut b = [0u8; 8];
        b[..n].copy_from_slice(raw);
        if self.encoding == Encoding::Big {
            b[..n].reverse();
        }
        Ok(match ty {
            Scalar::I8 => b[0] as i8 as f64,
            Scalar::U8 => b[0] as f64,
            Scalar::I16 => i16::from_le_bytes([b[0], b[1]]) as f64,
            Scalar::U16 => u16::from_le_bytes([b[0], b[1]]) as f64,
            Scalar::I32 => i32::from_le_bytes(b[..4].try_into().unwrap()) as f64,
            Scalar::U32 => u32::from_le_bytes(b[..4].try_into().unwrap()) as f64,
            Scalar::F32 => f32::from_le_bytes(b[..4].try_into().unwrap()) as f64,
            Scalar::F64 => f64::from_le_bytes(b),
        })
    }
}

pub(super) fn parse(bytes: &[u8]) -> Result<Mesh, String> {
    let (elements, encoding, body) = header(bytes)?;
    let mut r = Reader { bytes, pos: body, encoding };
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut builder = Builder::default();
    let mut corners = Vec::new();
    for e in &elements {
        let xyz = ["x", "y", "z"].map(|c| e.properties.iter().position(|p| matches!(p, Property::Scalar(n, _) if n == c)));
        let faces = e.name == "face";
        for _ in 0..e.count {
            let mut p = [0.0f32; 3];
            for (k, prop) in e.properties.iter().enumerate() {
                match prop {
                    Property::Scalar(_, ty) => {
                        let v = r.read(*ty)?;
                        if e.name == "vertex"
                            && let Some(c) = xyz.iter().position(|&i| i == Some(k))
                        {
                            p[c] = v as f32;
                        }
                    }
                    Property::List(name, len_ty, item_ty) => {
                        let len = r.read(*len_ty)? as usize;
                        let indices = faces && (name == "vertex_indices" || name == "vertex_index");
                        corners.clear();
                        for _ in 0..len {
                            let i = r.read(*item_ty)?;
                            if indices {
                                let p = positions.get(i as usize).filter(|_| i >= 0.0);
                                corners.push(*p.ok_or_else(|| format!("face index {i} out of range"))?);
                            }
                        }
                        if indices {
                            builder.push_polygon(&corners);
                        }
                    }
                }
            }
            if e.name == "vertex" {
                positions.push(p);
            }
        }
    }
    builder.finish(None)
}

/// The elements, the encoding and where the body starts.
fn header(bytes: &[u8]) -> Result<(Vec<Element>, Encoding, usize), String> {
    const END: &[u8] = b"end_header";
    let end = bytes.windows(END.len()).position(|w| w == END).ok_or("no end_header")?;
    let body = bytes[end..].iter().position(|&b| b == b'\n').map(|i| end + i + 1).ok_or("no data after the header")?;
    let text = std::str::from_utf8(&bytes[..end]).map_err(|_| "header is not text")?;
    let mut lines = text.lines().map(str::trim);
    if lines.next() != Some("ply") {
        return Err("not a PLY file".into());
    }
    let mut encoding = None;
    let mut elements: Vec<Element> = Vec::new();
    for line in lines {
        let t: Vec<&str> = line.split_ascii_whitespace().collect();
        let scalar = |name: &str| Scalar::from_name(name).ok_or_else(|| format!("unknown property type “{name}”"));
        match t.as_slice() {
            ["format", "ascii", ..] => encoding = Some(Encoding::Ascii),
            ["format", "binary_little_endian", ..] => encoding = Some(Encoding::Little),
            ["format", "binary_big_endian", ..] => encoding = Some(Encoding::Big),
            ["format", other, ..] => return Err(format!("unknown format “{other}”")),
            ["element", name, count] => elements.push(Element {
                name: name.to_string(),
                count: count.parse().map_err(|_| format!("bad element count “{count}”"))?,
                properties: Vec::new(),
            }),
            ["property", "list", len, item, name] => {
                let e = elements.last_mut().ok_or("property before any element")?;
                e.properties.push(Property::List(name.to_string(), scalar(len)?, scalar(item)?));
            }
            ["property", ty, name] => {
                let e = elements.last_mut().ok_or("property before any element")?;
                e.properties.push(Property::Scalar(name.to_string(), scalar(ty)?));
            }
            _ => {} // comment, obj_info
        }
    }
    Ok((elements, encoding.ok_or("no format line")?, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "ply\nformat {fmt} 1.0\ncomment made by hand\nelement vertex 4\nproperty float x\nproperty float y\n\
                          property float z\nproperty uchar red\nelement face 1\nproperty list uchar int vertex_indices\n\
                          element extra 1\nproperty list uchar ushort stuff\nend_header\n";
    const QUAD: [[f32; 3]; 4] = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 3.0, 1.0], [0.0, 3.0, 1.0]];

    fn check(m: Mesh) {
        assert_eq!(m.triangles.len(), 2);
        assert_eq!(m.bounds(), ([0.0; 3], [2.0, 3.0, 1.0]));
    }

    #[test]
    fn ascii() {
        let mut src = HEADER.replace("{fmt}", "ascii");
        for p in QUAD {
            src += &format!("{} {} {} 255\n", p[0], p[1], p[2]);
        }
        src += "4 0 1 2 3\n2 7 7\n";
        check(parse(src.as_bytes()).unwrap());
    }

    #[test]
    fn binary_both_endians() {
        for (fmt, big) in [("binary_little_endian", false), ("binary_big_endian", true)] {
            let mut b = HEADER.replace("{fmt}", fmt).into_bytes();
            let mut put = |bytes: &[u8]| {
                let mut v = bytes.to_vec();
                if big {
                    v.reverse();
                }
                b.extend(v);
            };
            for p in QUAD {
                for c in p {
                    put(&c.to_le_bytes());
                }
                put(&[255]);
            }
            put(&[4]);
            for i in 0..4i32 {
                put(&i.to_le_bytes());
            }
            put(&[1]);
            put(&7u16.to_le_bytes());
            check(parse(&b).unwrap());
        }
    }

    #[test]
    fn errors() {
        assert!(parse(b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nend_header\n").unwrap_err().contains("end of file"));
        assert!(parse(b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\n").is_err());
        let src = "ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0\n3 0 0 5\n";
        assert!(parse(src.as_bytes()).unwrap_err().contains("out of range"));
    }
}
