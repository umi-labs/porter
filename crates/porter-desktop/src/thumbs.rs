//! Media thumbnails: decoded off the UI thread (the bridge calls `decode` on a blocking
//! thread), at most `MAX_EDGE` px a side, kept in a bounded cache keyed by content hash.
//! A file that is missing, not an image or will not decode gets a placeholder that says
//! why; a hostile file never panics or allocates without bound.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, ErrorKind, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use gpui::RenderImage;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits};

/// Longest side of a thumbnail, in pixels.
pub const MAX_EDGE: u32 = 256;
/// Thumbnails kept.
pub const CAP: usize = 512;
/// Images whose header claims more pixels than this are not decoded.
const MAX_PIXELS: u64 = 64_000_000;
/// Decoder allocation ceiling.
const MAX_ALLOC: u64 = 256 * 1024 * 1024;
/// Files longer than this are not read (the media download cap).
const MAX_FILE: u64 = 200 * 1024 * 1024;

#[derive(Clone)]
pub enum Thumb {
    Ready(Arc<RenderImage>),
    /// What to show instead, and why.
    Placeholder(String),
}

/// Reads and shrinks the file at `path`. `mime` and `filename` say what it should be.
pub fn decode(path: &Path, mime: &str, filename: &str) -> Thumb {
    catch_unwind(AssertUnwindSafe(|| decode_inner(path, mime, filename)))
        .unwrap_or_else(|_| cant("the decoder crashed"))
}

fn cant(why: impl std::fmt::Display) -> Thumb {
    Thumb::Placeholder(format!("can't preview ({why})"))
}

/// "PDF", "DOCX"…: the extension, else the mime subtype.
fn type_label(mime: &str, filename: &str) -> Thumb {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e)
        .filter(|e| !e.is_empty() && e.len() <= 5);
    let label = match ext {
        Some(e) => e.to_uppercase(),
        None => match mime.split_once('/') {
            Some((_, sub)) if !sub.is_empty() && mime != "application/octet-stream" => {
                sub.to_uppercase()
            }
            _ => "FILE".into(),
        },
    };
    Thumb::Placeholder(label)
}

fn decode_inner(path: &Path, mime: &str, filename: &str) -> Thumb {
    // Only a regular file, by what the path itself is (a symlink, FIFO or device is
    // refused unopened: opening a FIFO would block this thread for good).
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() && m.len() > MAX_FILE => {
            return Thumb::Placeholder("too large to preview".into());
        }
        Ok(m) if m.is_file() => {}
        Ok(_) => return cant("not a regular file"),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Thumb::Placeholder("missing file".into());
        }
        Err(e) => return cant(e),
    }
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Thumb::Placeholder("missing file".into());
        }
        Err(e) => return cant(e),
    };
    let image_mime = mime.starts_with("image/") && mime != "image/svg+xml";
    // The store records no type for most files, so those are sniffed.
    let unknown = mime.is_empty() || mime == "application/octet-stream";
    if !image_mime && !unknown {
        return type_label(mime, filename);
    }
    let reader = match ImageReader::new(BufReader::new(file.take(MAX_FILE))).with_guessed_format() {
        Ok(r) => r,
        Err(e) => return cant(e),
    };
    if reader.format().is_none() {
        return if image_mime {
            cant("unrecognised image format")
        } else {
            type_label(mime, filename)
        };
    }
    let mut reader = reader;
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let decoder = match reader.into_decoder() {
        Ok(d) => d,
        Err(e) => return cant(e),
    };
    let (w, h) = decoder.dimensions();
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Thumb::Placeholder("too large to preview".into());
    }
    let img = match DynamicImage::from_decoder(decoder) {
        Ok(i) => i,
        Err(e) => return cant(e),
    };
    let mut buf = img.thumbnail(MAX_EDGE, MAX_EDGE).into_rgba8();
    // GPUI draws BGRA.
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Thumb::Ready(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

#[derive(Default)]
struct Inner {
    /// Thumbnail and the tick it was last used.
    map: HashMap<String, (Thumb, u64)>,
    tick: u64,
    /// Being decoded right now.
    pending: HashSet<String>,
}

/// Least recently used, `CAP` entries, shared between the window and the decoding threads.
#[derive(Default)]
pub struct Cache {
    inner: Mutex<Inner>,
}

impl Cache {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The thumbnail, if it is there; counts as a use.
    pub fn get(&self, key: &str) -> Option<Thumb> {
        let mut g = self.lock();
        g.tick += 1;
        let tick = g.tick;
        let e = g.map.get_mut(key)?;
        e.1 = tick;
        Some(e.0.clone())
    }

    /// True when `key` is neither cached nor being decoded: the caller should decode it
    /// and `put` the result.
    pub fn begin(&self, key: &str) -> bool {
        let mut g = self.lock();
        !g.map.contains_key(key) && g.pending.insert(key.to_string())
    }

    pub fn put(&self, key: &str, t: Thumb) {
        let mut g = self.lock();
        g.pending.remove(key);
        g.tick += 1;
        let tick = g.tick;
        g.map.insert(key.to_string(), (t, tick));
        // ponytail: a scan for the oldest per eviction, fine at 512; a linked list if CAP grows.
        while g.map.len() > CAP {
            let Some(old) = g
                .map
                .iter()
                .min_by_key(|(_, v)| v.1)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            g.map.remove(&old);
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.lock().map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placeholder(t: Thumb) -> String {
        match t {
            Thumb::Placeholder(s) => s,
            Thumb::Ready(_) => panic!("expected a placeholder"),
        }
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 10, 10, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut c = !0u32;
        for b in bytes {
            c ^= u32::from(*b);
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    (c >> 1) ^ 0xEDB8_8320
                } else {
                    c >> 1
                };
            }
        }
        !c
    }

    #[test]
    fn thumb_missing_file_placeholder() {
        let d = tempfile::tempdir().unwrap();
        let t = decode(&d.path().join("nope"), "image/png", "a.png");
        assert_eq!(placeholder(t), "missing file");
    }

    #[test]
    fn thumb_corrupt_image_placeholder() {
        let d = tempfile::tempdir().unwrap();
        // Not an image at all, under an image type.
        let junk = d.path().join("junk");
        std::fs::write(&junk, b"this is not a picture").unwrap();
        let why = placeholder(decode(&junk, "image/png", "a.png"));
        assert!(why.starts_with("can't preview ("), "{why}");
        // A real PNG signature and header, cut off.
        let mut cut = png(40, 40);
        cut.truncate(60);
        let p = d.path().join("cut");
        std::fs::write(&p, cut).unwrap();
        let why = placeholder(decode(&p, "image/png", "a.png"));
        assert!(why.starts_with("can't preview ("), "{why}");
    }

    #[test]
    fn thumb_decodes_and_shrinks() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("big");
        std::fs::write(&p, png(1000, 500)).unwrap();
        // No recorded type: sniffed.
        let Thumb::Ready(img) = decode(&p, "application/octet-stream", "big") else {
            panic!("expected a thumbnail");
        };
        let s = img.size(0);
        assert_eq!((s.width.0, s.height.0), (256, 128));
        // BGRA: the red channel moved to the third byte.
        let px = &img.as_bytes(0).unwrap()[..4];
        assert_eq!(px, [10, 10, 200, 255]);
    }

    #[test]
    fn thumb_non_image_shows_its_type() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x");
        std::fs::write(&p, b"%PDF-1.7").unwrap();
        assert_eq!(
            placeholder(decode(&p, "application/pdf", "Menu.pdf")),
            "PDF"
        );
        // Unknown type and not an image: by extension.
        assert_eq!(
            placeholder(decode(&p, "application/octet-stream", "notes.docx")),
            "DOCX"
        );
        assert_eq!(placeholder(decode(&p, "image/svg+xml", "logo.svg")), "SVG");
    }

    #[test]
    fn huge_image_header_not_decoded() {
        // A valid 20,000 x 20,000 PNG header (400 MP) and almost no pixels: decoding it
        // would fail, but the size is refused before any pixel is read.
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(b"IHDR");
        ihdr.extend_from_slice(&20_000u32.to_be_bytes());
        ihdr.extend_from_slice(&20_000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(&ihdr);
        bytes.extend_from_slice(&crc32(&ihdr).to_be_bytes());
        // The PNG reader only reports the size once it reaches the pixel data.
        let idat = *b"IDAT\x78";
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&idat);
        bytes.extend_from_slice(&crc32(&idat).to_be_bytes());
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("huge.png");
        std::fs::write(&p, bytes).unwrap();
        assert_eq!(
            placeholder(decode(&p, "image/png", "huge.png")),
            "too large to preview"
        );
    }

    #[cfg(unix)]
    #[test]
    fn thumb_fifo_does_not_block() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&p)
                .status()
                .unwrap()
                .success()
        );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(decode(&p, "image/png", "pipe.png"));
        });
        let t = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("decode blocked on a FIFO");
        assert_eq!(placeholder(t), "can't preview (not a regular file)");
    }

    #[cfg(unix)]
    #[test]
    fn thumb_symlink_refused() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real.png");
        std::fs::write(&real, png(8, 8)).unwrap();
        let link = d.path().join("link.png");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            placeholder(decode(&link, "image/png", "link.png")),
            "can't preview (not a regular file)"
        );
    }

    #[test]
    fn thumb_oversized_file_not_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("big.png");
        // Sparse: no real disk use.
        let f = File::create(&p).unwrap();
        f.set_len(MAX_FILE + 1).unwrap();
        assert_eq!(
            placeholder(decode(&p, "image/png", "big.png")),
            "too large to preview"
        );
    }

    #[test]
    fn thumb_cache_bounded() {
        let c = Cache::default();
        for i in 0..CAP + 100 {
            assert!(c.begin(&format!("h{i}")));
            c.put(&format!("h{i}"), Thumb::Placeholder("x".into()));
        }
        assert_eq!(c.len(), CAP);
        // The oldest went; the newest stayed.
        assert!(c.get("h0").is_none());
        assert!(c.get(&format!("h{}", CAP + 99)).is_some());
    }

    #[test]
    fn thumb_cache_keeps_what_is_used() {
        let c = Cache::default();
        c.put("keep", Thumb::Placeholder("x".into()));
        for i in 0..CAP {
            c.put(&format!("h{i}"), Thumb::Placeholder("x".into()));
            // Touched all along, so never the oldest.
            assert!(c.get("keep").is_some());
        }
        assert!(c.get("keep").is_some());
        // One decode in flight per key.
        assert!(c.begin("new"));
        assert!(!c.begin("new"));
        assert!(!c.begin("keep"));
    }
}
