//! M2: program icons for the main window. The page asks for the icons of
//! the names it shows; the host finds each program's file (a running
//! process, else the `path` of the entry), reads its icon as pixels
//! ([`crate::win32::icon`]) and hands it to the page as a PNG `data:` URL.
//! The page never gets file access; `img-src data:` in the CSP is all it
//! needs.
//!
//! Icons are kept per file and size, so a list that is drawn again costs
//! nothing. A missing icon is `None`, never an error: the page shows a
//! placeholder of the same size.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::process::{Access, Snapshot, Target};
use crate::win32::icon::Pixels;

/// Most bytes the cached `data:` URLs may take; beyond that the cache
/// starts over. One 32×32 icon is about 5 KiB, a 96×96 one about 49 KiB.
pub const CACHE_LIMIT: usize = 4 << 20;

/// One program the page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub exe: String,
    /// The `path` of the configuration entry, if any.
    pub path: Option<PathBuf>,
}

/// Icons already read, by lowercase file path and size.
#[derive(Default)]
pub struct Cache {
    icons: HashMap<(String, u32), Option<String>>,
    bytes: usize,
}

impl Cache {
    /// The icon of `file` at `size`, read by `read` only once.
    pub fn get(
        &mut self,
        file: &Path,
        size: u32,
        read: impl FnOnce(&Path, u32) -> Option<Pixels>,
    ) -> Option<String> {
        let key = (file.to_string_lossy().to_lowercase(), size);
        if let Some(known) = self.icons.get(&key) {
            return known.clone();
        }
        let url = read(file, size).map(|p| data_url(&png(&p)));
        let cost = key.0.len() + url.as_ref().map_or(0, String::len);
        if self.bytes + cost > CACHE_LIMIT {
            self.icons.clear();
            self.bytes = 0;
        }
        self.bytes += cost;
        self.icons.insert(key, url.clone());
        url
    }

    /// Bytes held by the cached URLs and their keys.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.icons.len()
    }

    pub fn is_empty(&self) -> bool {
        self.icons.is_empty()
    }
}

/// The file to read the icon of `wanted` from: the image of a running
/// process with that name (`running` maps lowercase names to paths), else
/// the configured path.
pub fn file_of(wanted: &Wanted, running: &HashMap<String, PathBuf>) -> Option<PathBuf> {
    running
        .get(&wanted.exe.to_lowercase())
        .cloned()
        .or_else(|| wanted.path.clone())
}

/// The image paths of running programs among `wanted`, by lowercase name:
/// one process of this session per name, opened with
/// `PROCESS_QUERY_LIMITED_INFORMATION` only (A7). Processes that cannot be
/// read are skipped.
pub fn running_paths(snapshot: &Snapshot, wanted: &[Wanted]) -> HashMap<String, PathBuf> {
    let session = snapshot.own_session();
    let mut paths = HashMap::new();
    for w in wanted {
        let name = w.exe.to_lowercase();
        if paths.contains_key(&name) {
            continue;
        }
        let path = snapshot
            .processes()
            .iter()
            .filter(|p| Some(p.session_id) == session && p.name.eq_ignore_ascii_case(&w.exe))
            .find_map(|p| match Target::open(p, Access::Query) {
                Ok(Some(target)) => target.image_path().ok(),
                _ => None,
            });
        if let Some(path) = path {
            paths.insert(name, path);
        }
    }
    paths
}

/// `data:image/png;base64,…`
pub fn data_url(png: &[u8]) -> String {
    let mut url = String::with_capacity(22 + png.len().div_ceil(3) * 4);
    url.push_str("data:image/png;base64,");
    base64(png, &mut url);
    url
}

fn base64(bytes: &[u8], out: &mut String) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
}

/// A PNG of `pixels` (RGBA, 8 bit). The image data is stored without
/// compression: icons are small and this keeps the encoder trivial.
pub fn png(pixels: &Pixels) -> Vec<u8> {
    let row = pixels.width as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * pixels.height as usize);
    for line in pixels.rgba.chunks_exact(row) {
        // Filter type 0 (none) before each row.
        raw.push(0);
        raw.extend_from_slice(line);
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&pixels.width.to_be_bytes());
    header.extend_from_slice(&pixels.height.to_be_bytes());
    // 8 bit, colour type 6 (RGBA), deflate, adaptive filtering, no interlace.
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A zlib stream of uncompressed deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = data.chunks(0xFFFF).collect();
    if blocks.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    for (i, block) in blocks.iter().enumerate() {
        out.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: u32) -> Pixels {
        Pixels {
            width: size,
            height: size,
            rgba: (0..size * size)
                .flat_map(|i| [i as u8, 0x80, 0xFF, if i % 2 == 0 { 255 } else { 0 }])
                .collect(),
        }
    }

    #[test]
    fn m2_checksums_match_known_values() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn m2_png_has_the_expected_structure() {
        let png = png(&square(2));
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..24], &[0, 0, 0, 2, 0, 0, 0, 2]);
        assert_eq!(&png[24..26], &[8, 6]);
        assert!(png.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
        // Rows: filter byte plus 2 × 4 bytes, twice; one stored block.
        let idat = &png[33..];
        assert_eq!(&idat[4..8], b"IDAT");
        let zlib = &idat[8..8 + u32::from_be_bytes(idat[0..4].try_into().unwrap()) as usize];
        assert_eq!(&zlib[..3], &[0x78, 0x01, 1]);
        assert_eq!(u16::from_le_bytes([zlib[3], zlib[4]]), 18);
        // A large icon needs several stored blocks; decoded, it gives the
        // pixels back.
        let pixels = square(256);
        assert_eq!(decode(&super::png(&pixels)), pixels);
        assert_eq!(decode(&super::png(&square(1))), square(1));
    }

    /// A small PNG reader for what `png` writes: checks every chunk's CRC,
    /// the stored deflate blocks and the Adler-32, and returns the pixels.
    fn decode(png: &[u8]) -> Pixels {
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let mut at = 8;
        let (mut width, mut height, mut idat) = (0, 0, Vec::new());
        loop {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let kind = &png[at + 4..at + 8];
            let data = &png[at + 8..at + 8 + len];
            let crc = u32::from_be_bytes(png[at + 8 + len..at + 12 + len].try_into().unwrap());
            assert_eq!(crc32(&png[at + 4..at + 8 + len]), crc, "CRC of {kind:?}");
            match kind {
                b"IHDR" => {
                    width = u32::from_be_bytes(data[0..4].try_into().unwrap());
                    height = u32::from_be_bytes(data[4..8].try_into().unwrap());
                    assert_eq!(&data[8..], &[8, 6, 0, 0, 0]);
                }
                b"IDAT" => idat.extend_from_slice(data),
                b"IEND" => break,
                other => panic!("unexpected chunk {other:?}"),
            }
            at += 12 + len;
        }
        assert_eq!(at + 12, png.len(), "nothing after IEND");
        // zlib: header, stored blocks, Adler-32 of the raw data.
        assert_eq!((u16::from(idat[0]) << 8 | u16::from(idat[1])) % 31, 0);
        let mut raw = Vec::new();
        let mut pos = 2;
        loop {
            let last = idat[pos] & 1 == 1;
            assert_eq!(idat[pos] & 0b110, 0, "stored blocks only");
            let len = u16::from_le_bytes([idat[pos + 1], idat[pos + 2]]);
            let nlen = u16::from_le_bytes([idat[pos + 3], idat[pos + 4]]);
            assert_eq!(len, !nlen);
            raw.extend_from_slice(&idat[pos + 5..pos + 5 + usize::from(len)]);
            pos += 5 + usize::from(len);
            if last {
                break;
            }
        }
        let adler = u32::from_be_bytes(idat[pos..pos + 4].try_into().unwrap());
        assert_eq!(adler32(&raw), adler);
        assert_eq!(pos + 4, idat.len());
        // Each row: filter 0, then RGBA.
        let row = width as usize * 4 + 1;
        assert_eq!(raw.len(), row * height as usize);
        let rgba = raw
            .chunks_exact(row)
            .flat_map(|r| {
                assert_eq!(r[0], 0);
                r[1..].to_vec()
            })
            .collect();
        Pixels {
            width,
            height,
            rgba,
        }
    }

    #[test]
    fn m2_base64_and_data_url() {
        let mut out = String::new();
        base64(b"Man", &mut out);
        base64(b"Ma", &mut out);
        base64(b"M", &mut out);
        assert_eq!(out, "TWFuTWE=TQ==");
        assert_eq!(data_url(b""), "data:image/png;base64,");
    }

    #[test]
    fn m2_cache_reads_each_file_and_size_once_and_keeps_misses() {
        let mut cache = Cache::default();
        let mut reads = 0;
        let mut read = |_: &Path, size: u32| {
            reads += 1;
            (size == 32).then(|| square(32))
        };
        let a = cache.get(Path::new(r"C:\A\a.exe"), 32, &mut read);
        assert!(a.unwrap().starts_with("data:image/png;base64,"));
        assert!(cache.get(Path::new(r"c:\a\A.EXE"), 32, &mut read).is_some());
        assert!(cache.get(Path::new(r"C:\A\a.exe"), 48, &mut read).is_none());
        assert!(cache.get(Path::new(r"C:\A\a.exe"), 48, &mut read).is_none());
        assert_eq!(reads, 2);
        assert_eq!(cache.len(), 2);
        assert!(cache.bytes() > 0);
    }

    #[test]
    fn m2_file_prefers_the_running_process() {
        let running = HashMap::from([("app.exe".to_owned(), PathBuf::from(r"C:\Run\app.exe"))]);
        let wanted = |exe: &str, path: Option<&str>| Wanted {
            exe: exe.to_owned(),
            path: path.map(PathBuf::from),
        };
        assert_eq!(
            file_of(&wanted("App.exe", Some(r"D:\Cfg\app.exe")), &running),
            Some(PathBuf::from(r"C:\Run\app.exe"))
        );
        assert_eq!(
            file_of(&wanted("other.exe", Some(r"D:\o.exe")), &running),
            Some(PathBuf::from(r"D:\o.exe"))
        );
        assert_eq!(file_of(&wanted("other.exe", None), &running), None);
    }
}
