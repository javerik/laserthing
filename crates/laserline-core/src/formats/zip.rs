//! Just enough of a zip reader for 3MF packages: the central directory (with zip64
//! sizes and offsets) and stored or deflated entries.

/// A zip archive in memory.
pub(super) struct Zip<'a> {
    bytes: &'a [u8],
    entries: Vec<Entry>,
}

struct Entry {
    name: String,
    method: u16,
    compressed: usize,
    size: usize,
    header: usize,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

impl<'a> Zip<'a> {
    pub(super) fn open(bytes: &'a [u8]) -> Result<Self, String> {
        Self::read(bytes).ok_or_else(|| "not a valid zip package".into())
    }

    fn read(bytes: &'a [u8]) -> Option<Self> {
        // End of central directory: 22 bytes plus a comment of up to 64 KiB.
        let low = bytes.len().saturating_sub(22 + 0xFFFF);
        let eocd = (low..=bytes.len().checked_sub(22)?).rev().find(|&o| u32_at(bytes, o) == Some(0x0605_4b50))?;
        let mut count = u16_at(bytes, eocd + 10)? as u64;
        let mut dir = u32_at(bytes, eocd + 16)? as u64;
        // Zip64 end of central directory, found through its locator just before.
        if eocd >= 20 && u32_at(bytes, eocd - 20) == Some(0x0706_4b50) {
            let e64 = usize::try_from(u64_at(bytes, eocd - 12)?).ok()?;
            if u32_at(bytes, e64) == Some(0x0606_4b50) {
                count = u64_at(bytes, e64 + 32)?;
                dir = u64_at(bytes, e64 + 48)?;
            }
        }
        let mut o = usize::try_from(dir).ok()?;
        let mut entries = Vec::new();
        for _ in 0..count {
            if u32_at(bytes, o)? != 0x0201_4b50 {
                return None;
            }
            let method = u16_at(bytes, o + 10)?;
            let mut compressed = u32_at(bytes, o + 20)? as u64;
            let mut size = u32_at(bytes, o + 24)? as u64;
            let (name_len, extra_len, comment_len) = (u16_at(bytes, o + 28)? as usize, u16_at(bytes, o + 30)? as usize, u16_at(bytes, o + 32)? as usize);
            let mut header = u32_at(bytes, o + 42)? as u64;
            let name = String::from_utf8_lossy(bytes.get(o + 46..o + 46 + name_len)?).into_owned();
            // The zip64 extra field holds, in this order, the values saved as 0xFFFFFFFF.
            let extra = bytes.get(o + 46 + name_len..o + 46 + name_len + extra_len)?;
            let mut e = 0;
            while e + 4 <= extra.len() {
                let (id, len) = (u16_at(extra, e)?, u16_at(extra, e + 2)? as usize);
                if id == 1 {
                    let mut f = e + 4;
                    for v in [&mut size, &mut compressed, &mut header] {
                        if *v == 0xFFFF_FFFF {
                            *v = u64_at(extra, f)?;
                            f += 8;
                        }
                    }
                }
                e += 4 + len;
            }
            let usize_of = |v: u64| usize::try_from(v).ok();
            entries.push(Entry { name, method, compressed: usize_of(compressed)?, size: usize_of(size)?, header: usize_of(header)? });
            o += 46 + name_len + extra_len + comment_len;
        }
        Some(Zip { bytes, entries })
    }

    /// The content of entry `name` (case-insensitive, with or without a leading `/`),
    /// `Ok(None)` when there is none.
    pub(super) fn file(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let name = name.trim_start_matches('/');
        let Some(e) = self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name)) else {
            return Ok(None);
        };
        let bad = || format!("{}: damaged zip entry", e.name);
        let h = e.header;
        if u32_at(self.bytes, h) != Some(0x0403_4b50) {
            return Err(bad());
        }
        let start = h + 30 + u16_at(self.bytes, h + 26).ok_or_else(bad)? as usize + u16_at(self.bytes, h + 28).ok_or_else(bad)? as usize;
        let data = self.bytes.get(start..start.checked_add(e.compressed).ok_or_else(bad)?).ok_or_else(bad)?;
        match e.method {
            0 => Ok(Some(data.to_vec())),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, e.size).map(Some).map_err(|_| bad()),
            m => Err(format!("{}: unsupported zip compression method {m}", e.name)),
        }
    }
}

/// Writes a zip of `files`, each deflated when `deflate`, else stored (test helper).
#[cfg(test)]
pub(super) fn write(files: &[(&str, &[u8])], deflate: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut dir = Vec::new();
    for (name, data) in files {
        let packed = if deflate { miniz_oxide::deflate::compress_to_vec(data, 6) } else { data.to_vec() };
        let method: u16 = if deflate { 8 } else { 0 };
        let offset = out.len() as u32;
        let common = |v: &mut Vec<u8>| {
            v.extend(20u16.to_le_bytes());
            v.extend(0u16.to_le_bytes());
            v.extend(method.to_le_bytes());
            v.extend([0u8; 4]); // time, date
            v.extend(0u32.to_le_bytes()); // crc, not checked here
            v.extend((packed.len() as u32).to_le_bytes());
            v.extend((data.len() as u32).to_le_bytes());
            v.extend((name.len() as u16).to_le_bytes());
            v.extend(0u16.to_le_bytes());
        };
        out.extend(0x0403_4b50u32.to_le_bytes());
        common(&mut out);
        out.extend(name.as_bytes());
        out.extend(&packed);
        dir.extend(0x0201_4b50u32.to_le_bytes());
        dir.extend(20u16.to_le_bytes());
        common(&mut dir);
        dir.extend([0u8; 10]); // comment length, disk, internal and external attributes
        dir.extend(offset.to_le_bytes());
        dir.extend(name.as_bytes());
    }
    let dir_offset = out.len() as u32;
    out.extend(&dir);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((dir.len() as u32).to_le_bytes());
    out.extend(dir_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_and_deflated() {
        let text = "hello 3mf ".repeat(100);
        for deflate in [false, true] {
            let z = write(&[("a.txt", b"first"), ("3D/b.model", text.as_bytes())], deflate);
            let zip = Zip::open(&z).unwrap();
            assert_eq!(zip.file("/3d/B.model").unwrap().unwrap(), text.as_bytes());
            assert_eq!(zip.file("a.txt").unwrap().unwrap(), b"first");
            assert!(zip.file("missing").unwrap().is_none());
        }
        assert!(Zip::open(b"PK\x03\x04 nothing else").is_err());
    }
}
