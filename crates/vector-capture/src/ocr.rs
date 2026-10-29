//! Text from a selection, read locally: PP-OCRv5 (Latin script, which covers Dutch
//! with its diacritics) through ONNX Runtime, via oar-ocr. No network use at run
//! time. The three model files (about 21 MiB) are downloaded once, on first use,
//! to ~/.local/share/vector-capture/models. Tesseract is the fallback when the
//! models are missing and cannot be fetched.

use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;

use gtk::gdk_pixbuf::Pixbuf;
use oar_ocr::oarocr::{OAROCR, OAROCRBuilder};

use crate::util;

const RELEASE: &str = "https://github.com/GreatV/oar-ocr/releases/download/v0.3.0";
const DET: &str = "pp-ocrv5_mobile_det.onnx";
const REC: &str = "latin_pp-ocrv5_mobile_rec.onnx";
const DICT: &str = "ppocrv5_latin_dict.txt";

thread_local! {
    static PIPELINE: RefCell<Option<Rc<OAROCR>>> = const { RefCell::new(None) };
}

/// `$XDG_DATA_HOME/vector-capture/models`.
pub fn models_dir() -> PathBuf {
    // VECTOR_CAPTURE_MODELS: a test seam for other model files
    if let Some(dir) = std::env::var_os("VECTOR_CAPTURE_MODELS").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| util::home().join(".local/share"))
        .join("vector-capture/models")
}

pub fn models_present() -> bool {
    let dir = models_dir();
    [DET, REC, DICT].iter().all(|f| dir.join(f).is_file())
}

/// Downloads any missing model file (once). Each file is written under a temporary name and
/// renamed when complete, so an interrupted download is never used.
pub fn ensure_models() -> Result<(), String> {
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for file in [DET, REC, DICT] {
        let target = dir.join(file);
        if target.is_file() {
            continue;
        }
        let url = format!("{RELEASE}/{file}");
        let mut resp = ureq::get(&url).call().map_err(|e| format!("download {file}: {e}"))?;
        let bytes = resp.body_mut().with_config().limit(64 << 20).read_to_vec().map_err(|e| format!("download {file}: {e}"))?;
        let part = dir.join(format!("{file}.part"));
        std::fs::write(&part, &bytes).map_err(|e| format!("{}: {e}", part.display()))?;
        std::fs::rename(&part, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    Ok(())
}

/// The resident pipeline; built on first use (about a second for the ONNX sessions).
fn pipeline() -> Result<Rc<OAROCR>, String> {
    if let Some(p) = PIPELINE.with(|p| p.borrow().clone()) {
        return Ok(p);
    }
    let dir = models_dir();
    let built = OAROCRBuilder::new(dir.join(DET), dir.join(REC), dir.join(DICT)).build().map_err(|e| format!("OCR models: {e}"))?;
    let rc = Rc::new(built);
    PIPELINE.with(|p| *p.borrow_mut() = Some(rc.clone()));
    Ok(rc)
}

/// Loads the pipeline now if its models are already on disk, so the first capture is fast.
pub fn prewarm() {
    if models_present() {
        let _ = pipeline();
    }
}

/// Reads the selection: PP-OCRv5 when it can run, else tesseract. Returns the text.
pub fn read(pb: &Pixbuf) -> Result<String, String> {
    if !models_present() {
        ensure_models()?;
    }
    match recognize(&rgb_of(pb)) {
        // accented letters come out of this model as escaped UTF-8 bytes ("\C3\AB"); such
        // output is not usable, so the selection is read again with tesseract
        Ok(text) if has_byte_escapes(&text) => {
            eprintln!("vector-capture: PP-OCRv5 returned escaped bytes, using tesseract");
            tesseract(pb)
        }
        Ok(text) => Ok(text),
        Err(e) => {
            eprintln!("vector-capture: PP-OCRv5 unavailable ({e}), using tesseract");
            tesseract(pb)
        }
    }
}

fn recognize(img: &image::RgbImage) -> Result<String, String> {
    let ocr = pipeline()?;
    let results = ocr.predict(vec![img.clone()]).map_err(|e| format!("OCR: {e}"))?;
    let lines: Vec<String> = results
        .iter()
        .flat_map(|r| r.text_regions.iter())
        .filter_map(|t| t.text_with_confidence().map(|(text, _)| text.to_string()))
        .filter(|t| !t.trim().is_empty())
        .collect();
    Ok(lines.join("\n"))
}

/// True for `\XX\YY`-style escapes of UTF-8 bytes (two hex digits after a backslash).
fn has_byte_escapes(text: &str) -> bool {
    let b = text.as_bytes();
    b.windows(4).any(|w| w[0] == b'\\' && w[1].is_ascii_hexdigit() && w[2].is_ascii_hexdigit() && w[3] == b'\\')
}

/// The fallback the Omarchy script uses, with the same options.
fn tesseract(pb: &Pixbuf) -> Result<String, String> {
    let png = pb.save_to_bufferv("png", &[]).map_err(|e| format!("png: {e}"))?;
    let mut child = Command::new("tesseract")
        .args(["stdin", "stdout", "--oem", "1", "--psm", "6", "-l", "eng", "--dpi", "300", "-c", "preserve_interword_spaces=1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("tesseract: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&png);
    }
    let out = child.wait_with_output().map_err(|e| format!("tesseract: {e}"))?;
    if !out.status.success() {
        return Err("tesseract failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
}

/// The pixbuf as an RGB image (alpha dropped).
fn rgb_of(pb: &Pixbuf) -> image::RgbImage {
    let (w, h) = (pb.width() as u32, pb.height() as u32);
    let (stride, ch) = (pb.rowstride() as usize, pb.n_channels() as usize);
    let bytes = pb.read_pixel_bytes();
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h as usize {
        let row = &bytes[y * stride..y * stride + w as usize * ch];
        for px in row.chunks_exact(ch) {
            out.extend_from_slice(&px[..3]);
        }
    }
    image::RgbImage::from_raw(w, h, out).expect("pixel buffer matches the size")
}

