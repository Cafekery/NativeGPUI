use crate::{Error, Format, Kind, Outline, Probe, Result};

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Intent {
    Archive,
    VisuallyLossless,
    High,
    Web,
}

impl Intent {
    pub fn label(self) -> &'static str {
        match self {
            Self::Archive => "Archive",
            Self::VisuallyLossless => "Visually lossless",
            Self::High => "High",
            Self::Web => "Web",
        }
    }

    pub fn all() -> &'static [Intent] {
        &[Self::Archive, Self::VisuallyLossless, Self::High, Self::Web]
    }

    pub fn default_max_edge(self) -> Option<u32> {
        (self == Self::Web).then_some(1920)
    }

    pub fn jpeg_quality(self) -> u8 {
        match self {
            Self::Archive | Self::VisuallyLossless => 95,
            Self::High => 85,
            Self::Web => 70,
        }
    }

    pub fn webp_quality(self) -> Option<u8> {
        match self {
            Self::Archive => None,
            Self::VisuallyLossless => Some(92),
            Self::High => Some(80),
            Self::Web => Some(68),
        }
    }

    pub fn audio_bitrate_k(self, target: Format) -> u32 {
        let (opus, other) = match self {
            Self::Archive => (192, 320),
            Self::VisuallyLossless => (160, 256),
            Self::High => (128, 192),
            Self::Web => (96, 128),
        };
        if target == Format::Opus { opus } else { other }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PathTaken {
    Copy,
    Remux,
    Encode,
}

impl PathTaken {
    pub fn label(self) -> &'static str {
        match self {
            Self::Copy => "copied",
            Self::Remux => "remuxed",
            Self::Encode => "encoded",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Planned {
    pub path: PathTaken,
    pub notes: String,
}

impl Planned {
    fn new(path: PathTaken, notes: impl Into<String>) -> Self {
        Self {
            path,
            notes: notes.into(),
        }
    }
}

pub fn plan(probe: &Probe, target: Format, intent: Intent) -> Result<Planned> {
    if probe.kind != target.kind() {
        return Err(Error::new(format!(
            "cannot convert {} to {}",
            probe.format.label(),
            target.label()
        )));
    }
    match probe.kind {
        Kind::Image => Ok(plan_image(probe.format, target, intent)),
        Kind::Audio => Ok(plan_audio(probe, target, intent)),
        Kind::Font => plan_font(probe, target, intent),
    }
}

fn plan_image(source: Format, target: Format, intent: Intent) -> Planned {
    if source == target && intent == Intent::Archive {
        return Planned::new(
            PathTaken::Copy,
            if target == Format::Jpeg {
                "JPEG is lossy; archive keeps the original bytes"
            } else {
                "same format, archive copies"
            },
        );
    }
    let notes = if target == Format::Jpeg {
        "JPEG is lossy"
    } else if source == Format::Jpeg && target.is_lossless_image() {
        "lossless encode of already-decoded JPEG pixels"
    } else if intent == Intent::Archive && target.is_lossless_image() {
        "lossless encode"
    } else if intent == Intent::Archive && target.is_lossy_image() {
        "archive still encodes lossy formats"
    } else {
        ""
    };
    Planned::new(PathTaken::Encode, notes)
}

fn plan_audio(probe: &Probe, target: Format, intent: Intent) -> Planned {
    let codec = probe.audio_codec.as_deref().unwrap_or("");
    let lower = codec.to_ascii_lowercase();
    if container_accepts(target, &lower) {
        let lossless = lower == "flac" || lower == "alac" || lower.starts_with("pcm_");
        if intent == Intent::Web && !lossless {
            return Planned::new(PathTaken::Encode, "web re-encodes to a smaller bitrate");
        }
        if probe.format.ext() != target.ext() {
            return Planned::new(
                PathTaken::Remux,
                format!("{codec} stream copied into {}", target.label()),
            );
        }
        return Planned::new(
            PathTaken::Copy,
            if intent == Intent::Web {
                "already lossless in this container"
            } else {
                "compatible codec, copied"
            },
        );
    }
    let lossy_archive =
        intent == Intent::Archive && !matches!(target, Format::Wav | Format::Flac | Format::Alac);
    Planned::new(
        PathTaken::Encode,
        if lossy_archive {
            "archive of a lossy target still encodes"
        } else {
            ""
        },
    )
}

fn container_accepts(target: Format, codec: &str) -> bool {
    match target {
        Format::Wav => codec.starts_with("pcm_"),
        Format::Flac => codec == "flac",
        Format::Mp3 => codec == "mp3",
        Format::Opus => codec == "opus",
        Format::Aac | Format::M4a => codec == "aac" || codec == "alac",
        Format::Alac => codec == "alac",
        Format::Mp4 => matches!(codec, "aac" | "alac" | "mp3"),
        _ => false,
    }
}

fn plan_font(probe: &Probe, target: Format, intent: Intent) -> Result<Planned> {
    if target == Format::Ttf && probe.outline == Some(Outline::Cff) {
        return Err(Error::new(
            "This font is drawn as OpenType (CFF). Saving as TTF would redraw every letter. Save as OTF to install it.",
        ));
    }
    if probe.format == target {
        return Ok(Planned::new(
            PathTaken::Copy,
            if intent == Intent::Web && target.is_desktop_font() {
                "A website should use WOFF2 — this keeps the desktop font"
            } else {
                "same format, copied"
            },
        ));
    }
    if probe.format.is_desktop_font() && target.is_desktop_font() {
        return Ok(Planned::new(
            PathTaken::Copy,
            "same letters; only the file name changes",
        ));
    }
    let notes = match target {
        Format::Woff2 => "same letters, packed for the web",
        Format::Woff => "same letters, packed for older browsers",
        _ if target.is_desktop_font() => "unpacked so you can install it",
        _ => "",
    };
    Ok(Planned::new(PathTaken::Remux, notes))
}
