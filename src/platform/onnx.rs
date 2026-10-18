//! Finding people without Apple's Vision framework (Windows, and Linux
//! development builds): two small open models run by ONNX Runtime — on
//! the graphics card through DirectML on Windows.
//!
//! * Faces: YuNet (OpenCV Zoo, MIT).
//! * People's outlines: PP-HumanSeg (OpenCV Zoo / PaddleSeg, Apache 2.0).
//!
//! The models are built into the program (see `models/`). ONNX Runtime is
//! loaded when first needed: `onnxruntime.dll` next to Cutaway on Windows,
//! or the library named by ORT_DYLIB_PATH.

use super::vision::{Detection, Detector, Mask, Segmenter};
use crate::media::VideoFrame;
use anyhow::{anyhow, Result};
use ort::session::Session;
use ort::value::Tensor;
use std::path::PathBuf;
use std::sync::OnceLock;

const YUNET: &[u8] = include_bytes!("../../models/face_detection_yunet_2023mar.onnx");
const HUMANSEG: &[u8] = include_bytes!("../../models/human_segmentation_pphumanseg_2023mar.onnx");

/// Where ONNX Runtime is.
fn library() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ORT_DYLIB_PATH") {
        return Some(PathBuf::from(p));
    }
    let name = if cfg!(windows) { "onnxruntime.dll" } else if cfg!(target_os = "macos") { "libonnxruntime.dylib" } else { "libonnxruntime.so" };
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join(name), dir.join("onnxruntime").join(name)].into_iter().find(|p| p.exists()).or_else(|| (!cfg!(windows)).then(|| PathBuf::from(name)))
}

fn environment() -> Result<&'static ort::environment::Environment> {
    static ENV: OnceLock<std::result::Result<ort::environment::Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        let path = library().ok_or_else(|| "ONNX Runtime (onnxruntime.dll) isn't next to Cutaway".to_string())?;
        let builder = ort::init_from(&path).map_err(|e| format!("couldn't load ONNX Runtime from {}: {e}", path.display()))?;
        builder.with_name("cutaway").build().map_err(|e| format!("ONNX Runtime: {e}"))
    })
    .as_ref()
    .map_err(|e| anyhow!("{e}"))
}

/// Why people can't be found here, if they can't.
pub fn problem() -> Option<String> {
    environment().err().map(|e| format!("{e:#}"))
}

fn session(model: &[u8]) -> Result<Session> {
    let env = environment()?;
    let b = Session::builder(env)?;
    #[cfg(windows)]
    let b = b
        .with_memory_pattern(false)
        .map_err(|e| anyhow!("{e}"))?
        .with_parallel_execution(false)
        .map_err(|e| anyhow!("{e}"))?
        .with_execution_providers([ort::ep::DirectML::default().build()])
        .map_err(|e| anyhow!("{e}"))?;
    let mut b = b.with_intra_threads(2).map_err(|e| anyhow!("{e}"))?;
    b.commit_from_memory(model).map_err(|e| anyhow!("loading a model: {e}"))
}

/// RGB (0-255) of an NV12 picture's pixel (BT.709, video range).
fn rgb(y: u8, u: u8, v: u8) -> [f32; 3] {
    let yy = (y as f32 - 16.0) * (255.0 / 219.0);
    let (cb, cr) = ((u as f32 - 128.0) * (255.0 / 224.0), (v as f32 - 128.0) * (255.0 / 224.0));
    [
        (yy + 1.5748 * cr).clamp(0.0, 255.0),
        (yy - 0.1873 * cb - 0.4681 * cr).clamp(0.0, 255.0),
        (yy + 1.8556 * cb).clamp(0.0, 255.0),
    ]
}

/// A planar 3×`size`×`size` tensor from a picture already scaled to fit
/// (`w`×`h`, the rest left as padding). `map` turns RGB into the three
/// channel values.
fn planar(frame: &VideoFrame, size: usize, map: impl Fn([f32; 3]) -> [f32; 3], pad: f32) -> Vec<f32> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut t = vec![pad; 3 * size * size];
    frame.pixels.with_planes(|p| {
        for y in 0..h.min(size) {
            for x in 0..w.min(size) {
                let i = (y / 2) * p.uv_stride + (x & !1);
                let c = map(rgb(p.y[y * p.y_stride + x], p.uv[i], p.uv[i + 1]));
                for k in 0..3 {
                    t[k * size * size + y * size + x] = c[k];
                }
            }
        }
    });
    t
}

// ---- faces -------------------------------------------------------------------------------

/// YuNet: faces in a 640×640 picture.
pub struct OnnxDetector {
    session: Session,
}

const YUNET_SIZE: usize = 640;

pub fn detector() -> Result<Box<dyn Detector>> {
    Ok(Box::new(OnnxDetector { session: session(YUNET)? }))
}

/// Overlap of two boxes (x, y, w, h) as intersection over union.
fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let (x1, y1) = (a[0].max(b[0]), a[1].max(b[1]));
    let (x2, y2) = ((a[0] + a[2]).min(b[0] + b[2]), (a[1] + a[3]).min(b[1] + b[3]));
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    inter / (a[2] * a[3] + b[2] * b[3] - inter).max(1e-6)
}

/// YuNet's outputs as faces (in model pixels): scores per anchor at
/// strides 8, 16 and 32, then overlapping boxes merged.
fn decode_yunet(get: impl Fn(&str) -> Option<Vec<f32>>, threshold: f32) -> Vec<(f32, [f32; 4])> {
    let mut found = Vec::new();
    for stride in [8usize, 16, 32] {
        let (Some(cls), Some(obj), Some(bbox)) = (get(&format!("cls_{stride}")), get(&format!("obj_{stride}")), get(&format!("bbox_{stride}"))) else { continue };
        let cols = YUNET_SIZE / stride;
        for i in 0..cls.len().min(obj.len()).min(bbox.len() / 4) {
            let score = (cls[i].clamp(0.0, 1.0) * obj[i].clamp(0.0, 1.0)).sqrt();
            if score < threshold {
                continue;
            }
            let (r, c) = (i / cols, i % cols);
            let s = stride as f32;
            let cx = (c as f32 + bbox[i * 4]) * s;
            let cy = (r as f32 + bbox[i * 4 + 1]) * s;
            let (w, h) = (bbox[i * 4 + 2].exp() * s, bbox[i * 4 + 3].exp() * s);
            found.push((score, [cx - w / 2.0, cy - h / 2.0, w, h]));
        }
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut keep: Vec<(f32, [f32; 4])> = Vec::new();
    for f in found {
        if keep.iter().all(|k| iou(&k.1, &f.1) < 0.3) {
            keep.push(f);
        }
        if keep.len() >= 50 {
            break;
        }
    }
    keep
}

impl Detector for OnnxDetector {
    fn detect(&mut self, frame: &VideoFrame) -> Result<Vec<Detection>> {
        // Fit inside 640×640 (padding below or to the right).
        let (fw, fh) = (frame.width.max(2) as f32, frame.height.max(2) as f32);
        let scale = YUNET_SIZE as f32 / fw.max(fh);
        let (sw, sh) = (((fw * scale) as u32).max(2) & !1, ((fh * scale) as u32).max(2) & !1);
        let small = frame.scaled(sw, sh);
        // YuNet takes BGR, 0-255.
        let data = planar(&small, YUNET_SIZE, |c| [c[2], c[1], c[0]], 0.0);
        let input = Tensor::from_array(([1usize, 3, YUNET_SIZE, YUNET_SIZE], data))?;
        let outputs = self.session.run(ort::inputs!["input" => input])?;
        let get = |name: &str| outputs.get(name).and_then(|v| v.try_extract_tensor::<f32>().ok()).map(|(_, d)| d.to_vec());
        let faces = decode_yunet(get, 0.6);
        Ok(faces
            .into_iter()
            .map(|(score, b)| Detection {
                rect: [b[0] / scale / fw, b[1] / scale / fh, b[2] / scale / fw, b[3] / scale / fh],
                face: true,
                confidence: score,
            })
            .collect())
    }
}

// ---- people's outlines ------------------------------------------------------------------

/// PP-HumanSeg: how likely each pixel of a 192×192 picture is a person.
pub struct OnnxSegmenter {
    session: Session,
    /// Last mask, to calm flicker between pictures.
    previous: Option<Vec<f32>>,
}

const SEG_SIZE: usize = 192;

pub fn segmenter() -> Result<Box<dyn Segmenter>> {
    Ok(Box::new(OnnxSegmenter { session: session(HUMANSEG)?, previous: None }))
}

impl Segmenter for OnnxSegmenter {
    fn mask(&mut self, frame: &VideoFrame) -> Result<Mask> {
        let small = frame.scaled(SEG_SIZE as u32, SEG_SIZE as u32);
        // RGB scaled to -1…1.
        let data = planar(&small, SEG_SIZE, |c| [c[0] / 127.5 - 1.0, c[1] / 127.5 - 1.0, c[2] / 127.5 - 1.0], 0.0);
        let input = Tensor::from_array(([1usize, 3, SEG_SIZE, SEG_SIZE], data))?;
        let outputs = self.session.run(ort::inputs![input])?;
        let (_, probs) = outputs[0].try_extract_tensor::<f32>()?;
        let n = SEG_SIZE * SEG_SIZE;
        if probs.len() < 2 * n {
            return Err(anyhow!("unexpected segmentation output"));
        }
        // Channel 1 is "person"; blend a little with the last mask.
        let mut p: Vec<f32> = probs[n..2 * n].to_vec();
        if let Some(prev) = &self.previous {
            for (a, b) in p.iter_mut().zip(prev) {
                *a = *a * 0.7 + b * 0.3;
            }
        }
        let data = p.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
        self.previous = Some(p);
        Ok(Mask { width: SEG_SIZE as u32, height: SEG_SIZE as u32, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_merges_faces() {
        // One strong anchor at stride 32, row 2, column 3, plus a weaker
        // overlapping one; boxes 2 units (64 px) wide.
        let get = |name: &str| -> Option<Vec<f32>> {
            let cols = 640 / name.rsplit('_').next()?.parse::<usize>().ok()?;
            let n = cols * cols;
            let mut v = vec![0.0f32; if name.starts_with("bbox") { n * 4 } else { n }];
            if name.ends_with("_32") {
                let i = 2 * cols + 3;
                let j = 2 * cols + 4;
                if name.starts_with("bbox") {
                    v[i * 4..i * 4 + 4].copy_from_slice(&[0.5, 0.5, 2f32.ln(), 2f32.ln()]);
                    v[j * 4..j * 4 + 4].copy_from_slice(&[-0.4, 0.5, 2f32.ln(), 2f32.ln()]);
                } else {
                    v[i] = 0.95;
                    v[j] = 0.8;
                }
            }
            Some(v)
        };
        let faces = decode_yunet(get, 0.6);
        assert_eq!(faces.len(), 1, "{faces:?}");
        let b = faces[0].1;
        assert!((b[0] - (3.5 * 32.0 - 32.0)).abs() < 0.01 && (b[2] - 64.0).abs() < 0.01, "{b:?}");
    }

    #[test]
    fn colour_conversion() {
        assert_eq!(rgb(235, 128, 128).map(|v| v.round()), [255.0, 255.0, 255.0]);
        assert_eq!(rgb(16, 128, 128).map(|v| v.round()), [0.0, 0.0, 0.0]);
    }

    /// With ONNX Runtime (ORT_DYLIB_PATH) and a test photo
    /// (CUTAWAY_TEST_PHOTO): finds the faces and the people.
    #[test]
    fn finds_people_in_a_photo() {
        let Some(photo) = std::env::var_os("CUTAWAY_TEST_PHOTO") else {
            eprintln!("skipping: set CUTAWAY_TEST_PHOTO and ORT_DYLIB_PATH");
            return;
        };
        let img = image::open(photo).unwrap().to_rgb8();
        let (w, h) = (img.width() & !1, img.height() & !1);
        let mut y = vec![0u8; (w * h) as usize];
        let mut uv = vec![0u8; (w * h / 2) as usize];
        for yy in 0..h {
            for xx in 0..w {
                let p = img.get_pixel(xx, yy).0.map(|c| c as f32);
                y[(yy * w + xx) as usize] = (16.0 + (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]) * 219.0 / 255.0) as u8;
                if yy % 2 == 0 && xx % 2 == 0 {
                    let i = ((yy / 2) * w + xx) as usize;
                    uv[i] = (128.0 + (-0.1146 * p[0] - 0.3854 * p[1] + 0.5 * p[2]) * 224.0 / 255.0) as u8;
                    uv[i + 1] = (128.0 + (0.5 * p[0] - 0.4542 * p[1] - 0.0458 * p[2]) * 224.0 / 255.0) as u8;
                }
            }
        }
        let frame = VideoFrame::new_nv12(w, h, 0, y, uv, 0);
        let mut d = detector().unwrap();
        let faces = d.detect(&frame).unwrap();
        eprintln!("faces: {faces:?}");
        assert!(!faces.is_empty());
        let mut s = segmenter().unwrap();
        let m = s.mask(&frame).unwrap();
        let person = m.data.iter().filter(|v| **v > 128).count() as f32 / m.data.len() as f32;
        eprintln!("person covers {:.0}% of the picture", person * 100.0);
        assert!(person > 0.02 && person < 0.98);
    }
}
