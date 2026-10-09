use std::fs;
use std::io::BufWriter;
use std::path::Path;
use std::process::Command;

use image::buffer::ConvertBuffer;
use image::codecs::gif::{GifEncoder, Repeat};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, Frame, ImageEncoder, RgbImage, RgbaImage};

use crate::{Error, Format, Intent, Job, PathTaken, Planned, Probe, Report, Result};

pub fn run(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    match planned.path {
        PathTaken::Copy => crate::copy(job, probe, planned),
        PathTaken::Remux => Err(Error::new("images are not remuxed")),
        PathTaken::Encode => encode(job, probe, planned),
    }
}

fn encode(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    let max_edge = job.max_edge.or(job.intent.default_max_edge());
    match job.target {
        Format::Heic => return encode_heic(job, probe, planned, max_edge),
        Format::Avif => return encode_avif(job, probe, planned, max_edge),
        _ => {}
    }

    let rgba = pixels(job, probe, max_edge)?;
    let (width, height) = rgba.dimensions();
    write_pixels(&job.dest, job.target, job.intent, rgba)?;
    let mut notes = planned.notes.clone();
    if !job.keep_metadata {
        if !notes.is_empty() {
            notes.push_str("; ");
        }
        notes.push_str("pixels only");
    }
    let mut report = Report::new(
        probe,
        PathTaken::Encode,
        fs::metadata(&job.dest)?.len(),
        notes,
    );
    report.width_out = Some(width);
    report.height_out = Some(height);
    Ok(report)
}

fn pixels(job: &Job, probe: &Probe, max_edge: Option<u32>) -> Result<RgbaImage> {
    let rgba = if probe.format == Format::Heic {
        in_temp_dir(|dir| {
            let png = dir.join("decode.png");
            sips_convert(&job.source, &png, "png", None)?;
            Ok(image::open(&png)?.to_rgba8())
        })?
    } else {
        image::ImageReader::open(&job.source)?
            .with_guessed_format()?
            .decode()?
            .to_rgba8()
    };
    Ok(fit(rgba, max_edge))
}

fn fit(img: RgbaImage, max_edge: Option<u32>) -> RgbaImage {
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

fn write_pixels(dest: &Path, target: Format, intent: Intent, rgba: RgbaImage) -> Result<()> {
    let (w, h) = rgba.dimensions();
    let file = || -> Result<_> { Ok(BufWriter::new(fs::File::create(dest)?)) };
    match target {
        Format::Png => {
            PngEncoder::new_with_quality(file()?, CompressionType::Best, FilterType::Adaptive)
                .write_image(rgba.as_raw(), w, h, ExtendedColorType::Rgba8)?;
        }
        Format::Jpeg => {
            let rgb: RgbImage = rgba.convert();
            JpegEncoder::new_with_quality(file()?, intent.jpeg_quality()).encode(
                rgb.as_raw(),
                w,
                h,
                ExtendedColorType::Rgb8,
            )?;
        }
        Format::Webp => {
            let encoder = webp::Encoder::from_rgba(rgba.as_raw(), w, h);
            let bytes = match intent.webp_quality() {
                None => encoder.encode_lossless(),
                Some(q) => encoder.encode(q as f32),
            };
            fs::write(dest, &*bytes)?;
        }
        Format::Tiff => rgba.save_with_format(dest, image::ImageFormat::Tiff)?,
        Format::Gif => {
            let mut encoder = GifEncoder::new(file()?);
            encoder.set_repeat(Repeat::Infinite)?;
            encoder.encode_frame(Frame::new(rgba))?;
        }
        _ => return Err(Error::new("internal: wrong encoder for target")),
    }
    Ok(())
}

fn encode_heic(
    job: &Job,
    probe: &Probe,
    planned: &Planned,
    max_edge: Option<u32>,
) -> Result<Report> {
    if max_edge.is_some() {
        let rgba = pixels(job, probe, max_edge)?;
        in_temp_dir(|dir| {
            let png = dir.join("resized.png");
            rgba.save(&png)?;
            sips_convert(&png, &job.dest, "heic", Some(job.intent))
        })?;
    } else {
        sips_convert(&job.source, &job.dest, "heic", Some(job.intent))?;
    }
    reprobed(job, probe, planned)
}

fn encode_avif(
    job: &Job,
    probe: &Probe,
    planned: &Planned,
    max_edge: Option<u32>,
) -> Result<Report> {
    if Command::new("avifenc").arg("--version").output().is_err() {
        return Err(Error::new("AVIF needs avifenc (libavif) on PATH"));
    }
    let rgba = pixels(job, probe, max_edge)?;
    let q = match job.intent {
        Intent::Archive | Intent::VisuallyLossless => "12",
        Intent::High => "28",
        Intent::Web => "40",
    };
    let status = in_temp_dir(|dir| {
        let png = dir.join("in.png");
        rgba.save(&png)?;
        Ok(Command::new("avifenc")
            .args(["-q", q, "--min", "0", "--max", "63"])
            .arg(&png)
            .arg(&job.dest)
            .status()?)
    })?;
    if !status.success() {
        return Err(Error::new("avifenc failed"));
    }
    reprobed(job, probe, planned)
}

fn reprobed(job: &Job, probe: &Probe, planned: &Planned) -> Result<Report> {
    let mut report = Report::new(
        probe,
        PathTaken::Encode,
        fs::metadata(&job.dest)?.len(),
        planned.notes.clone(),
    );
    if let Ok(out) = crate::probe(&job.dest) {
        report.width_out = out.width;
        report.height_out = out.height;
    }
    Ok(report)
}

fn sips_convert(src: &Path, dest: &Path, format: &str, intent: Option<Intent>) -> Result<()> {
    let mut cmd = Command::new("sips");
    cmd.args(["-s", "format", format]);
    if let Some(intent) = intent {
        let quality = match intent {
            Intent::Archive | Intent::VisuallyLossless => "high",
            Intent::High => "normal",
            Intent::Web => "low",
        };
        cmd.args(["-s", "formatOptions", quality]);
    }
    let out = cmd.arg(src).arg("--out").arg(dest).output()?;
    if !out.status.success() {
        return Err(Error::new(format!(
            "sips: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

fn in_temp_dir<T>(work: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir()
        .join(format!("ng-convert-{}", std::process::id()))
        .join(nanos.to_string());
    fs::create_dir_all(&dir)?;
    let result = work(&dir);
    let _ = fs::remove_dir_all(&dir);
    result
}
