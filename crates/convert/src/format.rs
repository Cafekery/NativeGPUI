use std::path::Path;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Image,
    Audio,
    Font,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Outline {
    TrueType,
    Cff,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Heic,
    Avif,
    Tiff,
    Gif,
    Wav,
    Flac,
    Mp3,
    Aac,
    M4a,
    Mp4,
    Opus,
    Alac,
    Ttf,
    Otf,
    Woff,
    Woff2,
}

impl Format {
    pub fn kind(self) -> Kind {
        match self {
            Self::Png
            | Self::Jpeg
            | Self::Webp
            | Self::Heic
            | Self::Avif
            | Self::Tiff
            | Self::Gif => Kind::Image,
            Self::Wav
            | Self::Flac
            | Self::Mp3
            | Self::Aac
            | Self::M4a
            | Self::Mp4
            | Self::Opus
            | Self::Alac => Kind::Audio,
            Self::Ttf | Self::Otf | Self::Woff | Self::Woff2 => Kind::Font,
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Heic => "heic",
            Self::Avif => "avif",
            Self::Tiff => "tiff",
            Self::Gif => "gif",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Aac | Self::M4a | Self::Alac => "m4a",
            Self::Mp4 => "mp4",
            Self::Opus => "opus",
            Self::Ttf => "ttf",
            Self::Otf => "otf",
            Self::Woff => "woff",
            Self::Woff2 => "woff2",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Webp => "WebP",
            Self::Heic => "HEIC",
            Self::Avif => "AVIF",
            Self::Tiff => "TIFF",
            Self::Gif => "GIF",
            Self::Wav => "WAV",
            Self::Flac => "FLAC",
            Self::Mp3 => "MP3",
            Self::Aac => "AAC",
            Self::M4a => "M4A",
            Self::Mp4 => "MP4",
            Self::Opus => "Opus",
            Self::Alac => "ALAC",
            Self::Ttf => "TTF",
            Self::Otf => "OTF",
            Self::Woff => "WOFF",
            Self::Woff2 => "WOFF2",
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "png" => Self::Png,
            "jpg" | "jpeg" | "jpe" | "jfif" => Self::Jpeg,
            "webp" => Self::Webp,
            "heic" | "heif" => Self::Heic,
            "avif" => Self::Avif,
            "tif" | "tiff" => Self::Tiff,
            "gif" => Self::Gif,
            "wav" => Self::Wav,
            "flac" => Self::Flac,
            "mp3" => Self::Mp3,
            "aac" => Self::Aac,
            "m4a" | "m4b" => Self::M4a,
            "mp4" => Self::Mp4,
            "opus" | "ogg" | "oga" => Self::Opus,
            "caf" => Self::Alac,
            "ttf" | "tte" => Self::Ttf,
            "otf" => Self::Otf,
            "woff" => Self::Woff,
            "woff2" => Self::Woff2,
            _ => return None,
        })
    }

    pub fn is_lossless_image(self) -> bool {
        matches!(self, Self::Png | Self::Webp | Self::Tiff | Self::Gif)
    }

    pub fn is_lossy_image(self) -> bool {
        matches!(self, Self::Jpeg | Self::Heic | Self::Avif)
    }

    pub fn is_desktop_font(self) -> bool {
        matches!(self, Self::Ttf | Self::Otf)
    }

    pub fn advice(self) -> Option<&'static str> {
        match self {
            Self::Ttf => Some(
                "Best to install on this Mac and in Word. Don’t put it on a website — the file is huge.",
            ),
            Self::Otf => {
                Some("Best for design apps and print. Install this; don’t serve it on the web.")
            }
            Self::Woff2 => Some(
                "Best for websites. Same look as the desktop font, usually about half the size. Don’t install this in Font Book.",
            ),
            Self::Woff => {
                Some("Only if a site still has to work in very old browsers. Otherwise use WOFF2.")
            }
            _ => None,
        }
    }
}

pub fn relevant_targets(format: Format) -> &'static [Format] {
    match format.kind() {
        Kind::Image => &[
            Format::Png,
            Format::Jpeg,
            Format::Webp,
            Format::Heic,
            Format::Avif,
            Format::Tiff,
            Format::Gif,
        ],
        Kind::Audio => &[
            Format::Wav,
            Format::Flac,
            Format::Alac,
            Format::Aac,
            Format::M4a,
            Format::Mp4,
            Format::Mp3,
            Format::Opus,
        ],
        Kind::Font => &[Format::Woff2, Format::Ttf, Format::Otf, Format::Woff],
    }
}
