//! The face a row wears: the symbol and tint a uniform type identifier resolves to.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use native_gpui::tokens::{FileType, SymbolColor, SystemColor};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSString;

use self::Face as F;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Face {
    Folder,
    Volume,
    Pdf,
    Text,
    Code,
    Sheet,
    Pages,
    Slides,
    Image,
    Movie,
    Audio,
    Archive,
    Disk,
    Program,
    Web,
}

impl Face {
    pub(crate) fn symbol(self) -> &'static str {
        match self {
            F::Folder => "folder.fill",
            F::Volume => "internaldrive.fill",
            F::Pdf => "doc.richtext.fill",
            F::Text => "doc.text.fill",
            F::Code => "chevron.left.forwardslash.chevron.right",
            F::Sheet => "tablecells.fill",
            F::Pages => "doc.fill",
            F::Slides => "rectangle.on.rectangle.fill",
            F::Image => "photo.fill",
            F::Movie => "film.fill",
            F::Audio => "waveform",
            F::Archive => "shippingbox.fill",
            F::Disk => "externaldrive.fill",
            F::Program => "app.fill",
            F::Web => "safari.fill",
        }
    }

    /// The artwork a row wears while its own type's is still being drawn: the system's own
    /// generic folder or document, never the SF glyph, which is only what is left when even that
    /// has not landed.
    pub(crate) fn stand_in(self) -> FileType<'static> {
        match self {
            F::Folder => FileType::Folder,
            F::Volume | F::Disk => FileType::Path("/"),
            F::Program => FileType::Extension("app"),
            _ => FileType::Uti("public.data"),
        }
    }

    pub(crate) fn tint(self) -> SymbolColor {
        let color = match self {
            F::Folder => SystemColor::SystemBlue,
            F::Volume => SystemColor::SystemGray,
            F::Pdf => SystemColor::SystemRed,
            F::Text => SystemColor::SecondaryLabel,
            F::Code => SystemColor::SystemGreen,
            F::Sheet => SystemColor::SystemGreen,
            F::Pages => SystemColor::SystemOrange,
            F::Slides => SystemColor::SystemIndigo,
            F::Image => SystemColor::SystemTeal,
            F::Movie => SystemColor::SystemPurple,
            F::Audio => SystemColor::SystemPink,
            F::Archive => SystemColor::SystemBrown,
            F::Disk => SystemColor::SystemGray,
            F::Program => SystemColor::SystemBlue,
            F::Web => SystemColor::SystemCyan,
        };
        SymbolColor::Hierarchical(color)
    }
}

/// The face a type wears, by what its uniform type identifier conforms to, most specific first.
const FACES: &[(&str, Face)] = &[
    ("public.folder", F::Folder),
    ("public.volume", F::Volume),
    ("com.apple.application", F::Program),
    ("com.adobe.pdf", F::Pdf),
    ("public.image", F::Image),
    ("public.movie", F::Movie),
    ("public.audio", F::Audio),
    ("public.archive", F::Archive),
    ("public.disk-image", F::Disk),
    ("public.spreadsheet", F::Sheet),
    ("public.presentation", F::Slides),
    ("com.apple.iwork.pages.pages", F::Pages),
    ("public.source-code", F::Code),
    ("public.html", F::Web),
    ("public.text", F::Text),
];

// `NSWorkspace` is the only type conformance AppKit exposes; `UTType` is in a framework this
// example does not link. The lookup is LaunchServices', not AppKit state: fine off the main thread.
#[allow(deprecated)]
pub(super) fn face_of(uti: &str) -> Face {
    static SEEN: OnceLock<RwLock<HashMap<String, Face>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    if let Some(face) = seen.read().unwrap().get(uti) {
        return *face;
    }
    let workspace = NSWorkspace::sharedWorkspace();
    let mine = NSString::from_str(uti);
    let face = FACES
        .iter()
        .find(|(parent, _)| workspace.type_conformsToType(&mine, &NSString::from_str(parent)))
        .map_or(F::Text, |(_, face)| *face);
    seen.write().unwrap().insert(uti.to_owned(), face);
    face
}
