use crate::format::{Outline, container_accepts, lossless_audio_codec};
use crate::{Error, Format, Kind, Probe, Result};

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
        match self {
            Self::Web => Some(1920),
            _ => None,
        }
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
        let opus = matches!(target, Format::Opus);
        match self {
            Self::Archive => {
                if opus {
                    192
                } else {
                    320
                }
            }
            Self::VisuallyLossless => {
                if opus {
                    160
                } else {
                    256
                }
            }
            Self::High => {
                if opus {
                    128
                } else {
                    192
                }
            }
            Self::Web => {
                if opus {
                    96
                } else {
                    128
                }
            }
        }
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

pub fn plan(probe: &Probe, target: Format, intent: Intent) -> Result<Planned> {
    if probe.kind != target.kind() {
        return Err(Error::new(format!(
            "cannot convert {} to {}",
            probe.format.label(),
            target.label()
        )));
    }
    match probe.kind {
        Kind::Image => Ok(plan_image(probe, target, intent)),
        Kind::Audio => Ok(plan_audio(probe, target, intent)),
        Kind::Font => plan_font(probe, target, intent),
    }
}

fn plan_image(probe: &Probe, target: Format, intent: Intent) -> Planned {
    if probe.format == target && intent == Intent::Archive {
        let notes = if target == Format::Jpeg {
            "JPEG is lossy; archive keeps the original bytes".into()
        } else {
            "same format, archive copies".into()
        };
        return Planned {
            path: PathTaken::Copy,
            notes,
        };
    }
    let mut notes = String::new();
    if target == Format::Jpeg {
        notes.push_str("JPEG is lossy");
    } else if probe.format == Format::Jpeg && target.is_lossless_image() {
        notes.push_str("lossless encode of already-decoded JPEG pixels");
    } else if intent == Intent::Archive && target.is_lossless_image() {
        notes.push_str("lossless encode");
    } else if intent == Intent::Archive && target.is_lossy_image() {
        notes.push_str("archive still encodes lossy formats");
    }
    Planned {
        path: PathTaken::Encode,
        notes,
    }
}

fn plan_audio(probe: &Probe, target: Format, intent: Intent) -> Planned {
    let codec = probe.audio_codec.as_deref().unwrap_or("");
    let same_ext = probe.format.ext() == target.ext();
    let accepts = !codec.is_empty() && container_accepts(target, codec);

    if accepts && intent != Intent::Web {
        if same_ext {
            return Planned {
                path: PathTaken::Copy,
                notes: "compatible codec, copied".into(),
            };
        }
        return Planned {
            path: PathTaken::Remux,
            notes: format!("{codec} stream copied into {}", target.label()),
        };
    }
    if accepts && intent == Intent::Web && !lossless_audio_codec(codec) {
        return Planned {
            path: PathTaken::Encode,
            notes: "web re-encodes to a smaller bitrate".into(),
        };
    }
    if accepts && intent == Intent::Web {
        if same_ext {
            return Planned {
                path: PathTaken::Copy,
                notes: "already lossless in this container".into(),
            };
        }
        return Planned {
            path: PathTaken::Remux,
            notes: format!("{codec} stream copied into {}", target.label()),
        };
    }
    Planned {
        path: PathTaken::Encode,
        notes: if intent == Intent::Archive
            && !matches!(target, Format::Wav | Format::Flac | Format::Alac)
        {
            "archive of a lossy target still encodes".into()
        } else {
            String::new()
        },
    }
}

fn plan_font(probe: &Probe, target: Format, intent: Intent) -> Result<Planned> {
    if target == Format::Ttf && probe.outline == Some(Outline::Cff) {
        return Err(Error::new(
            "This font is drawn as OpenType (CFF). Saving as TTF would redraw every letter. Save as OTF to install it.",
        ));
    }
    if probe.format == target {
        let mut notes = "same format, copied".to_string();
        if intent == Intent::Web && target.is_desktop_font() {
            notes = "A website should use WOFF2 — this keeps the desktop font".into();
        }
        return Ok(Planned {
            path: PathTaken::Copy,
            notes,
        });
    }
    if probe.format.is_desktop_font() && target.is_desktop_font() {
        return Ok(Planned {
            path: PathTaken::Copy,
            notes: "same letters; only the file name changes".into(),
        });
    }
    let notes = if target == Format::Woff2 {
        "same letters, packed for the web".into()
    } else if target == Format::Woff {
        "same letters, packed for older browsers".into()
    } else if target.is_desktop_font() {
        "unpacked so you can install it".into()
    } else {
        String::new()
    };
    Ok(Planned {
        path: PathTaken::Remux,
        notes,
    })
}
