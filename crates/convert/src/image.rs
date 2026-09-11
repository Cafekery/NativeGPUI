use std::fs;
use std::io::BufWriter;
use std::path::Path;
use std::process::Command;

use image::codecs::gif::{GifEncoder, Repeat};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ColorType, DynamicImage, ExtendedColorType, Frame, ImageEncoder, RgbaImage};

use crate::plan::{Intent, PathTaken, Planned};
use crate::report::Report;
use crate::{Error, Format, Job, Probe, Result};

pub fn run(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    match planned.path {
        PathTaken::Copy => copy_file(job, probe, planned),
        PathTaken::Remux => Err(Error::new("images are not remuxed")),
        PathTaken::Encode => encode(job, probe, planned),
    }
}

fn copy_file(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    fs::copy(&job.source, &job.dest).map_err(|e| Error::new(e.to_string()))?;
    let bytes_out = fs::metadata(&job.dest)
        .map_err(|e| Error::new(e.to_string()))?
        .len();
    Ok(Report {
        bytes_in: probe.bytes,
        bytes_out,
        width_in: probe.width,
        height_in: probe.height,
        width_out: probe.width,
        height_out: probe.height,
        duration_in: None,
        duration_out: None,
        path: PathTaken::Copy,
        notes: planned.notes.clone(),
    })
}

fn encode(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    let max_edge = job.max_edge.or(job.intent.default_max_edge());
    if matches!(job.target, Format::Heic) {
        return encode_sips(job, probe, planned, "heic", max_edge);
    }
    if matches!(job.target, Format::Avif) {
        return encode_avifenc(job, probe, planned, max_edge);
    }

    let mut rgba = load_rgba(&job.source, probe.format)?;
    rgba = maybe_resize(rgba, max_edge);
    let (width_out, height_out) = rgba.dimensions();
    write_pixels(&job.dest, job.target, job.intent, &rgba)?;
    let bytes_out = fs::metadata(&job.dest)
        .map_err(|e| Error::new(e.to_string()))?
        .len();
    let mut notes = planned.notes.clone();
    if !job.keep_metadata && !notes.is_empty() {
        notes.push_str("; ");
    }
    if !job.keep_metadata {
        notes.push_str("pixels only");
    }
    Ok(Report {
        bytes_in: probe.bytes,
        bytes_out,
        width_in: probe.width,
        height_in: probe.height,
        width_out: Some(width_out),
        height_out: Some(height_out),
        duration_in: None,
        duration_out: None,
        path: PathTaken::Encode,
        notes,
    })
}

fn load_rgba(path: &Path, format: Format) -> Result<RgbaImage> {
    if matches!(format, Format::Heic) {
        return load_heic_via_sips(path);
    }
    let img = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| Error::new(e.to_string()))?
        .decode()
        .map_err(|e| Error::new(e.to_string()))?;
    Ok(img.to_rgba8())
}

fn load_heic_via_sips(path: &Path) -> Result<RgbaImage> {
    let dir = tempfile_dir()?;
    let png = dir.join("decode.png");
    sips_convert(path, &png, "png", None)?;
    let img = image::open(&png).map_err(|e| Error::new(e.to_string()))?;
    let _ = fs::remove_dir_all(dir);
    Ok(img.to_rgba8())
}

fn write_pixels(dest: &Path, target: Format, intent: Intent, rgba: &RgbaImage) -> Result<()> {
    let file = fs::File::create(dest).map_err(|e| Error::new(e.to_string()))?;
    let mut out = BufWriter::new(file);
    match target {
        Format::Png => {
            let enc =
                PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive);
            enc.write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                ExtendedColorType::Rgba8,
            )
            .map_err(|e| Error::new(e.to_string()))?;
        }
        Format::Jpeg => {
            let rgb = DynamicImage::ImageRgba8(rgba.clone()).to_rgb8();
            let mut enc = JpegEncoder::new_with_quality(&mut out, intent.jpeg_quality());
            enc.encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                ColorType::Rgb8.into(),
            )
            .map_err(|e| Error::new(e.to_string()))?;
        }
        Format::Webp => {
            drop(out);
            write_webp(dest, rgba, intent.webp_quality())?;
        }
        Format::Tiff => {
            rgba.save_with_format(dest, image::ImageFormat::Tiff)
                .map_err(|e| Error::new(e.to_string()))?;
        }
        Format::Gif => {
            let frame = Frame::new(rgba.clone());
            let mut enc = GifEncoder::new(&mut out);
            enc.set_repeat(Repeat::Infinite)
                .map_err(|e| Error::new(e.to_string()))?;
            enc.encode_frame(frame)
                .map_err(|e| Error::new(e.to_string()))?;
        }
        Format::Heic
        | Format::Avif
        | Format::Wav
        | Format::Flac
        | Format::Mp3
        | Format::Aac
        | Format::M4a
        | Format::Mp4
        | Format::Opus
        | Format::Alac
        | Format::Ttf
        | Format::Otf
        | Format::Woff
        | Format::Woff2 => {
            return Err(Error::new("internal: wrong encoder for target"));
        }
    }
    Ok(())
}

fn write_webp(dest: &Path, rgba: &RgbaImage, quality: Option<u8>) -> Result<()> {
    let enc = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height());
    let bytes = match quality {
        None => enc.encode_lossless(),
        Some(q) => enc.encode(q as f32),
    };
    fs::write(dest, &*bytes).map_err(|e| Error::new(e.to_string()))
}

fn maybe_resize(img: RgbaImage, max_edge: Option<u32>) -> RgbaImage {
    let Some(max) = max_edge else {
        return img;
    };
    let (w, h) = img.dimensions();
    let edge = w.max(h);
    if edge <= max {
        return img;
    }
    let scale = max as f32 / edge as f32;
    let nw = (w as f32 * scale).round().max(1.0) as u32;
    let nh = (h as f32 * scale).round().max(1.0) as u32;
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3)
}

fn encode_sips(
    job: &Job,
    probe: &Probe,
    planned: &Planned,
    format: &str,
    max_edge: Option<u32>,
) -> Result<Report> {
    if max_edge.is_some() {
        let mut rgba = load_rgba(&job.source, probe.format)?;
        rgba = maybe_resize(rgba, max_edge);
        let dir = tempfile_dir()?;
        let png = dir.join("resized.png");
        rgba.save(&png).map_err(|e| Error::new(e.to_string()))?;
        sips_convert(&png, &job.dest, format, Some(job.intent))?;
        let _ = fs::remove_dir_all(&dir);
    } else {
        sips_convert(&job.source, &job.dest, format, Some(job.intent))?;
    }
    finish_sips(job, probe, planned)
}

fn finish_sips(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    let bytes_out = fs::metadata(&job.dest)
        .map_err(|e| Error::new(e.to_string()))?
        .len();
    let (width_out, height_out) = crate::probe::probe(&job.dest)
        .ok()
        .map(|p| (p.width, p.height))
        .unwrap_or((probe.width, probe.height));
    Ok(Report {
        bytes_in: probe.bytes,
        bytes_out,
        width_in: probe.width,
        height_in: probe.height,
        width_out,
        height_out,
        duration_in: None,
        duration_out: None,
        path: PathTaken::Encode,
        notes: planned.notes.clone(),
    })
}

fn encode_avifenc(
    job: &Job,
    probe: &Probe,
    planned: &Planned,
    max_edge: Option<u32>,
) -> Result<Report> {
    if Command::new("avifenc").arg("--version").output().is_err() {
        return Err(Error::new("AVIF needs avifenc (libavif) on PATH"));
    }
    let mut rgba = load_rgba(&job.source, probe.format)?;
    rgba = maybe_resize(rgba, max_edge);
    let dir = tempfile_dir()?;
    let png = dir.join("in.png");
    rgba.save(&png).map_err(|e| Error::new(e.to_string()))?;
    let q = match job.intent {
        Intent::Archive | Intent::VisuallyLossless => 12,
        Intent::High => 28,
        Intent::Web => 40,
    };
    let status = Command::new("avifenc")
        .args(["-q", &q.to_string(), "--min", "0", "--max", "63"])
        .arg(&png)
        .arg(&job.dest)
        .status()
        .map_err(|e| Error::new(e.to_string()))?;
    let _ = fs::remove_dir_all(dir);
    if !status.success() {
        return Err(Error::new("avifenc failed"));
    }
    finish_sips(job, probe, planned)
}

fn sips_convert(src: &Path, dest: &Path, format: &str, intent: Option<Intent>) -> Result<()> {
    let mut cmd = Command::new("sips");
    cmd.args(["-s", "format", format]);
    if let Some(intent) = intent {
        let opt = match intent {
            Intent::Archive | Intent::VisuallyLossless => "high",
            Intent::High => "normal",
            Intent::Web => "low",
        };
        cmd.args(["-s", "formatOptions", opt]);
    }
    cmd.arg(src).args(["--out", &dest.to_string_lossy()]);
    let out = cmd.output().map_err(|e| Error::new(e.to_string()))?;
    if !out.status.success() {
        return Err(Error::new(format!(
            "sips: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

fn tempfile_dir() -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("ng-convert-{}", std::process::id()));
    let dir = dir.join(format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).map_err(|e| Error::new(e.to_string()))?;
    Ok(dir)
}
