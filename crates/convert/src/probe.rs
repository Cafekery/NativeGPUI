use std::path::Path;
use std::process::Command;

use crate::audio;
use crate::font;
use crate::{Error, Format, Kind, Outline, Result};

#[derive(Clone, Debug)]
pub struct Probe {
    pub path: std::path::PathBuf,
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
    let bytes = std::fs::metadata(path)
        .map_err(|e| Error::new(format!("{}: {e}", path.display())))?
        .len();
    match format.kind() {
        Kind::Image => probe_image(path, format, bytes),
        Kind::Audio => probe_audio(path, format, bytes),
        Kind::Font => probe_font(path, format, bytes),
    }
}

fn probe_image(path: &Path, format: Format, bytes: u64) -> Result<Probe> {
    let (width, height) = image_size(path, format);
    Ok(Probe {
        path: path.to_path_buf(),
        kind: Kind::Image,
        format,
        bytes,
        width,
        height,
        duration: None,
        audio_codec: None,
        outline: None,
    })
}

fn image_size(path: &Path, format: Format) -> (Option<u32>, Option<u32>) {
    if let Ok(reader) = image::ImageReader::open(path) {
        if let Ok(reader) = reader.with_guessed_format() {
            if let Ok((w, h)) = reader.into_dimensions() {
                return (Some(w), Some(h));
            }
        }
    }
    if matches!(format, Format::Heic | Format::Avif) {
        return sips_size(path);
    }
    (None, None)
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
    let text = String::from_utf8_lossy(&out.stdout);
    let mut width = None;
    let mut height = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some("pixelWidth:"), Some(v)) => width = v.parse().ok(),
            (Some("pixelHeight:"), Some(v)) => height = v.parse().ok(),
            _ => {}
        }
    }
    (width, height)
}

fn probe_audio(path: &Path, format: Format, bytes: u64) -> Result<Probe> {
    let (duration, codec) = audio::ffprobe(path).unwrap_or((None, None));
    Ok(Probe {
        path: path.to_path_buf(),
        kind: Kind::Audio,
        format,
        bytes,
        width: None,
        height: None,
        duration,
        audio_codec: codec,
        outline: None,
    })
}

fn probe_font(path: &Path, named: Format, bytes: u64) -> Result<Probe> {
    let data = std::fs::read(path).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
    if font::is_collection(path, &data) {
        return Err(font::collection_error());
    }
    let format = font::sniff(&data).unwrap_or(named);
    if format.kind() != Kind::Font {
        return Err(Error::new("not a font"));
    }
    let sfnt = font::to_sfnt(&data, format)?;
    let outline = font::outline_of(&sfnt)?;
    Ok(Probe {
        path: path.to_path_buf(),
        kind: Kind::Font,
        format,
        bytes,
        width: None,
        height: None,
        duration: None,
        audio_codec: None,
        outline: Some(outline),
    })
}
