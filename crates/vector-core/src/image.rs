//! Fast image reading for previews and thumbnails: dimensions from the file header, JPEGs
//! decoded at a reduced scale (the decoder skips most of the work), PNGs in-process, EXIF
//! orientation applied. Anything else returns `None` and callers fall back to gdk-pixbuf,
//! which on current systems runs every image through a sandboxed loader process and takes
//! half a second or more per photo.

use std::ffi::{c_int, c_void};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Decoded pixels, 4 bytes (RGBA, not premultiplied) per pixel, rows packed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Width, height and format name, read from the header only.
pub fn dimensions(path: &Path) -> Option<(u32, u32, &'static str)> {
    use std::io::Read;
    let size = imagesize::size(path).ok()?;
    let mut head = Vec::with_capacity(256);
    File::open(path)
        .ok()?
        .take(256)
        .read_to_end(&mut head)
        .ok()?;
    let format = imagesize::image_type(&head).map_or("", format_name);
    Some((size.width as u32, size.height as u32, format))
}

fn format_name(t: imagesize::ImageType) -> &'static str {
    use imagesize::ImageType as T;
    match t {
        T::Jpeg => "JPEG",
        T::Png => "PNG",
        T::Gif => "GIF",
        T::Webp => "WebP",
        T::Bmp => "BMP",
        T::Tiff => "TIFF",
        T::Ico => "ICO",
        T::Heif(_) => "HEIF",
        T::Jxl => "JPEG XL",
        T::Qoi => "QOI",
        _ => "",
    }
}

/// The image, at least `max` pixels on its long side when given (JPEGs are decoded at the
/// smallest of 1/8, 1/4, 1/2 or full size that still covers it; PNGs are reduced after). Only
/// JPEG and PNG; `None` for other formats or damaged files.
pub fn decode(path: &Path, max: Option<u32>) -> Option<Pixels> {
    let mut head = [0u8; 8];
    {
        use std::io::Read;
        File::open(path).ok()?.read_exact(&mut head).ok()?;
    }
    let px = if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        jpeg(path, max)?
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(path)?
    } else {
        return None;
    };
    Some(match max {
        Some(m) => shrink(px, m),
        None => px,
    })
}

#[link(name = "turbojpeg")]
unsafe extern "C" {
    fn tj3Init(init_type: c_int) -> *mut c_void;
    fn tj3Destroy(handle: *mut c_void);
    fn tj3DecompressHeader(handle: *mut c_void, buf: *const u8, size: usize) -> c_int;
    fn tj3Get(handle: *mut c_void, param: c_int) -> c_int;
    fn tj3Set(handle: *mut c_void, param: c_int, value: c_int) -> c_int;
    fn tj3SetScalingFactor(handle: *mut c_void, factor: ScalingFactor) -> c_int;
    fn tj3Decompress8(
        handle: *mut c_void,
        buf: *const u8,
        size: usize,
        dst: *mut u8,
        pitch: c_int,
        pixel_format: c_int,
    ) -> c_int;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ScalingFactor {
    num: c_int,
    denom: c_int,
}

const TJINIT_DECOMPRESS: c_int = 1;
const TJPF_RGBA: c_int = 7;
const TJPARAM_JPEGWIDTH: c_int = 5;
const TJPARAM_JPEGHEIGHT: c_int = 6;
const TJPARAM_FASTUPSAMPLE: c_int = 9;
const TJPARAM_FASTDCT: c_int = 10;

/// A libjpeg-turbo decompressor, freed on drop.
struct Turbo(*mut c_void);

impl Drop for Turbo {
    fn drop(&mut self) {
        // SAFETY: a handle from tj3Init, freed once
        unsafe { tj3Destroy(self.0) };
    }
}

/// libjpeg-turbo, scaled by 1/8, 1/4, 1/2 or not at all: the smallest that still covers
/// `max` (scaling happens inside the inverse DCT, so most of the work is skipped).
fn jpeg(path: &Path, max: Option<u32>) -> Option<Pixels> {
    let data = std::fs::read(path).ok()?;
    // SAFETY: plain calls on a handle we own; buffers outlive each call and the output
    // buffer is sized from the scaled dimensions libjpeg-turbo reports
    unsafe {
        let tj = Turbo(tj3Init(TJINIT_DECOMPRESS));
        if tj.0.is_null() || tj3DecompressHeader(tj.0, data.as_ptr(), data.len()) != 0 {
            return None;
        }
        let (w, h) = (
            tj3Get(tj.0, TJPARAM_JPEGWIDTH),
            tj3Get(tj.0, TJPARAM_JPEGHEIGHT),
        );
        if w <= 0 || h <= 0 {
            return None;
        }
        let num = match max {
            Some(m) => [1, 2, 4, 8]
                .into_iter()
                .find(|n| (w.max(h) * n / 8) as u32 >= m)
                .unwrap_or(8),
            None => 8,
        };
        let factor = ScalingFactor { num, denom: 8 };
        if num != 8 {
            tj3SetScalingFactor(tj.0, factor);
            // previews: speed over the last bit of quality
            tj3Set(tj.0, TJPARAM_FASTDCT, 1);
            tj3Set(tj.0, TJPARAM_FASTUPSAMPLE, 1);
        }
        // TJSCALED: ceil(dimension * num / denom)
        let (sw, sh) = ((w * num + 7) / 8, (h * num + 7) / 8);
        let mut rgba = vec![0u8; sw as usize * sh as usize * 4];
        let r = tj3Decompress8(
            tj.0,
            data.as_ptr(),
            data.len(),
            rgba.as_mut_ptr(),
            sw * 4,
            TJPF_RGBA,
        );
        if r != 0 {
            return None;
        }
        let px = Pixels {
            width: sw as u32,
            height: sh as u32,
            rgba,
        };
        let orientation = jpeg_exif(&data).and_then(exif_orientation).unwrap_or(1);
        Some(orient(px, orientation))
    }
}

/// The EXIF block (APP1, `Exif\0\0…`) of a JPEG file's bytes.
fn jpeg_exif(data: &[u8]) -> Option<&[u8]> {
    let mut i = 2;
    while i + 4 <= data.len() && data[i] == 0xFF {
        let marker = data[i + 1];
        // start of scan: no more headers
        if marker == 0xDA {
            return None;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let body = data.get(i + 4..i + 2 + len)?;
        if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
            return Some(body);
        }
        i += 2 + len;
    }
    None
}

fn png(path: &Path) -> Option<Pixels> {
    let mut d = png::Decoder::new(BufReader::new(File::open(path).ok()?));
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = d.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let frame = r.next_frame(&mut buf).ok()?;
    let data = &buf[..frame.buffer_size()];
    let rgba = match frame.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => data
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(Pixels {
        width: frame.width,
        height: frame.height,
        rgba,
    })
}

/// The orientation tag (1–8) from EXIF data (with or without the `Exif\0\0` prefix).
pub fn exif_orientation(data: &[u8]) -> Option<u16> {
    let t = data.strip_prefix(b"Exif\0\0").unwrap_or(data);
    let le = match t.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |i: usize| -> Option<u16> {
        let b = t.get(i..i + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |i: usize| -> Option<u32> {
        let b = t.get(i..i + 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    };
    let ifd = u32_at(4)? as usize;
    let n = u16_at(ifd)? as usize;
    (0..n)
        .find_map(|k| {
            let e = ifd + 2 + k * 12;
            (u16_at(e)? == 0x0112).then(|| u16_at(e + 8)).flatten()
        })
        .filter(|o| (1..=8).contains(o))
}

/// Turns pixels stored with EXIF orientation `o` upright.
pub fn orient(px: Pixels, o: u16) -> Pixels {
    if o <= 1 || o > 8 {
        return px;
    }
    let (w, h) = (px.width as usize, px.height as usize);
    // orientations 5–8 swap the axes
    let (nw, nh) = if o >= 5 { (h, w) } else { (w, h) };
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match o {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (h - 1 - y, x),
                7 => (h - 1 - y, w - 1 - x),
                _ => (y, w - 1 - x),
            };
            let (s, d) = ((y * w + x) * 4, (ny * nw + nx) * 4);
            out[d..d + 4].copy_from_slice(&px.rgba[s..s + 4]);
        }
    }
    Pixels {
        width: nw as u32,
        height: nh as u32,
        rgba: out,
    }
}

/// Averages blocks of pixels until the long side is at most twice `max` (the display scales
/// the rest smoothly); keeps memory small for thumbnails of big PNGs.
fn shrink(px: Pixels, max: u32) -> Pixels {
    let k = (px.width.max(px.height) / max.max(1) / 2).max(1) as usize;
    if k == 1 {
        return px;
    }
    let (w, h) = (px.width as usize, px.height as usize);
    let (nw, nh) = (w / k, h / k);
    let mut out = Vec::with_capacity(nw * nh * 4);
    for by in 0..nh {
        for bx in 0..nw {
            let mut sum = [0u32; 4];
            for y in by * k..by * k + k {
                for x in bx * k..bx * k + k {
                    let i = (y * w + x) * 4;
                    for (s, v) in sum.iter_mut().zip(&px.rgba[i..i + 4]) {
                        *s += *v as u32;
                    }
                }
            }
            let n = (k * k) as u32;
            out.extend(sum.iter().map(|s| (s / n) as u8));
        }
    }
    Pixels {
        width: nw as u32,
        height: nh as u32,
        rgba: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(w: u32, h: u32) -> Pixels {
        // pixel value = its index, in the red channel
        Pixels {
            width: w,
            height: h,
            rgba: (0..w * h).flat_map(|i| [i as u8, 0, 0, 255]).collect(),
        }
    }

    fn reds(p: &Pixels) -> Vec<u8> {
        p.rgba.chunks(4).map(|c| c[0]).collect()
    }

    #[test]
    fn orientations() {
        // 0 1 2
        // 3 4 5
        let p = grid(3, 2);
        assert_eq!(reds(&orient(p.clone(), 1)), [0, 1, 2, 3, 4, 5]);
        assert_eq!(reds(&orient(p.clone(), 3)), [5, 4, 3, 2, 1, 0]);
        // 6: rotate 90° clockwise
        let r = orient(p.clone(), 6);
        assert_eq!((r.width, r.height), (2, 3));
        assert_eq!(reds(&r), [3, 0, 4, 1, 5, 2]);
        // 8: rotate 90° counter-clockwise
        assert_eq!(reds(&orient(p, 8)), [2, 5, 1, 4, 0, 3]);
    }

    #[test]
    fn exif_tag_both_byte_orders() {
        // II, 42, IFD at 8, 1 entry: 0x0112 SHORT count 1 value 6
        let le = [
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0,
        ];
        assert_eq!(exif_orientation(&le), Some(6));
        let mut prefixed = b"Exif\0\0".to_vec();
        prefixed.extend_from_slice(&le);
        assert_eq!(exif_orientation(&prefixed), Some(6));
        let be = [
            b'M', b'M', 0, 42, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 3, 0, 0,
        ];
        assert_eq!(exif_orientation(&be), Some(3));
        assert_eq!(exif_orientation(b"nonsense"), None);
    }

    #[test]
    fn finds_exif_in_jpeg_headers() {
        // SOI, APP0 (JFIF, 16 bytes), APP1 Exif with an orientation-6 TIFF block, SOS
        let tiff = [
            b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0,
        ];
        let mut f = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
        f.extend_from_slice(&[0; 14]);
        let app1_len = (2 + 6 + tiff.len()) as u16;
        f.extend_from_slice(&[0xFF, 0xE1]);
        f.extend_from_slice(&app1_len.to_be_bytes());
        f.extend_from_slice(b"Exif\0\0");
        f.extend_from_slice(&tiff);
        f.extend_from_slice(&[0xFF, 0xDA, 0, 2]);
        assert_eq!(jpeg_exif(&f).and_then(exif_orientation), Some(6));
        assert_eq!(jpeg_exif(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2]), None);
    }

    #[test]
    fn png_round_trip_and_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.png");
        {
            let f = File::create(&path).unwrap();
            let mut e = png::Encoder::new(f, 40, 20);
            e.set_color(png::ColorType::Rgb);
            e.set_depth(png::BitDepth::Eight);
            let mut w = e.write_header().unwrap();
            w.write_image_data(&vec![200u8; 40 * 20 * 3]).unwrap();
        }
        assert_eq!(dimensions(&path), Some((40, 20, "PNG")));
        let px = decode(&path, None).unwrap();
        assert_eq!((px.width, px.height), (40, 20));
        assert_eq!(&px.rgba[..4], &[200, 200, 200, 255]);
        // asked for 5 px: averaged down to at most twice that
        let small = decode(&path, Some(5)).unwrap();
        assert_eq!((small.width, small.height), (10, 5));
        std::fs::write(dir.path().join("x.txt"), "hello world").unwrap();
        assert_eq!(decode(&dir.path().join("x.txt"), None), None);
    }
}

#[cfg(test)]
mod bench {
    /// `VECTOR_BENCH_IMAGE=/path cargo test --release -p vector-core bench_decode -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_decode() {
        let Some(p) = std::env::var_os("VECTOR_BENCH_IMAGE") else {
            return;
        };
        let p = std::path::Path::new(&p);
        let t = std::time::Instant::now();
        let d = super::dimensions(p);
        eprintln!("dimensions {:?} in {:?}", d, t.elapsed());
        let t = std::time::Instant::now();
        let px = super::decode(p, Some(512)).map(|x| (x.width, x.height));
        eprintln!("decode 512 {:?} in {:?}", px, t.elapsed());
        let t = std::time::Instant::now();
        let px = super::decode(p, None).map(|x| (x.width, x.height));
        eprintln!("decode full {:?} in {:?}", px, t.elapsed());
    }
}
