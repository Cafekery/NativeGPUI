use std::path::Path;
use std::process::Command;

use crate::{Error, Format, Kind, Outline, Result, audio, font};

#[derive(Clone, Debug)]
pub struct Probe {
    pub kind: Kind,
    pub format: Format,
    pub bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration: Option<f64>,
    pub audio_codec: Option<String>,
    pub outline: Option<Outline>,
}

pub fn probe(path: &Path) -> Result<Probe> {
    if font::is_collection(path, &[]) {
        return Err(font::collection_error());
    }
    let format = Format::from_path(path).ok_or_else(|| {
        Error::new(format!(
            "unsupported type: {}",
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("unknown")
        ))
    })?;
    let located = |e: std::io::Error| Error::new(format!("{}: {e}", path.display()));
    let mut probe = Probe {
        kind: format.kind(),
        format,
        bytes: std::fs::metadata(path).map_err(located)?.len(),
        width: None,
        height: None,
        duration: None,
        audio_codec: None,
        outline: None,
    };
    match probe.kind {
        Kind::Image => (probe.width, probe.height) = image_size(path, format),
        Kind::Audio => {
            if let Some((duration, codec)) = audio::ffprobe(path) {
                probe.duration = duration;
                probe.audio_codec = codec;
            }
        }
        Kind::Font => {
            let data = std::fs::read(path).map_err(located)?;
            if font::is_collection(path, &data) {
                return Err(font::collection_error());
            }
            probe.format = font::sniff(&data).unwrap_or(format);
            probe.outline = Some(font::outline_of(&font::to_sfnt(data, probe.format)?)?);
        }
    }
    Ok(probe)
}

fn image_size(path: &Path, format: Format) -> (Option<u32>, Option<u32>) {
    let dimensions = image::ImageReader::open(path)
        .ok()
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.into_dimensions().ok());
    match dimensions {
        Some((w, h)) => (Some(w), Some(h)),
        None if matches!(format, Format::Heic | Format::Avif) => sips_size(path),
        None => (None, None),
    }
}

fn sips_size(path: &Path) -> (Option<u32>, Option<u32>) {
    let Ok(out) = Command::new("sips")
        .args(["-g", "pixelWidth", "-g", "pixelHeight"])
        .arg(path)
        .output()
    else {
        return (None, None);
    };
    if !out.status.success() {
        return (None, None);
    }
    let mut size = (None, None);
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some("pixelWidth:"), Some(v)) => size.0 = v.parse().ok(),
            (Some("pixelHeight:"), Some(v)) => size.1 = v.parse().ok(),
            _ => {}
        }
    }
    size
}
