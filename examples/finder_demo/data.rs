//! The invented filesystem, and the system formatters that turn it into strings.

use std::collections::HashMap;

use gpui::SharedString;
use native_gpui::tokens::{FileType, SymbolColor, SystemColor};
use objc2_foundation::{
    NSByteCountFormatter, NSByteCountFormatterCountStyle, NSCalendar, NSCalendarUnit, NSDate,
    NSDateComponents, NSDateFormatter, NSDateFormatterStyle,
};

use self::Face as F;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Face {
    Folder,
    Volume,
    Device,
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
            F::Device => "laptopcomputer",
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

    pub(crate) fn kind(self) -> &'static str {
        match self {
            F::Folder => "Folder",
            F::Volume => "Volume",
            F::Device => "Computer",
            F::Pdf => "PDF Document",
            F::Text => "Plain Text",
            F::Code => "Source Text",
            F::Sheet => "Numbers Spreadsheet",
            F::Pages => "Pages Document",
            F::Slides => "Keynote Presentation",
            F::Image => "PNG image",
            F::Movie => "QuickTime Movie",
            F::Audio => "Audio File",
            F::Archive => "ZIP archive",
            F::Disk => "Disk Image",
            F::Program => "Application",
            F::Web => "Web Internet Location",
        }
    }

    fn artwork(self, extension: Option<&'static str>) -> Option<FileType<'static>> {
        match self {
            F::Folder => Some(FileType::Folder),
            F::Program => Some(FileType::Extension("app")),
            F::Volume | F::Device => None,
            // A name without an extension gets Finder's generic document icon, not a symbol.
            _ => Some(extension.map_or(FileType::Uti("public.data"), FileType::Extension)),
        }
    }

    pub(crate) fn tint(self) -> SymbolColor {
        let color = match self {
            F::Folder => SystemColor::SystemBlue,
            F::Volume | F::Device => SystemColor::SystemGray,
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

/// `name`, face, modified as `yyyymmddhhmm` or an [`ago`] offset, size in bytes, the folder this
/// row opens (`""` when it opens nothing).
pub(crate) type Src = (&'static str, Face, u64, u64, &'static str);

pub(crate) const RELATIVE: u64 = 1_000_000_000_000;

/// A stamp meaning `days` calendar days before launch at `hour:minute`, so rows keep landing on
/// Finder's `Today` and `Yesterday` tiers as a frozen literal never could.
pub(crate) const fn ago(days: u64, hour: u64, minute: u64) -> u64 {
    RELATIVE + days * 10_000 + hour * 100 + minute
}

/// `id`, display name, parent id (`""` at the root), breadcrumbs, contents.
pub(crate) type Dir = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [Src],
);

pub(crate) const HOME: &[&str] = &["Macintosh HD", "Users", "avery"];

pub(crate) const DIRS: &[Dir] = &[
    (
        "documents",
        "Documents",
        "home",
        &["Macintosh HD", "Users", "avery", "Documents"],
        &[
            ("Invoices", F::Folder, 202607281412, 0, "invoices"),
            ("Projects", F::Folder, ago(0, 4, 34), 0, "projects"),
            ("Tax 2025", F::Folder, 202601190904, 0, "tax"),
            ("Annual Report.pdf", F::Pdf, 202608141731, 8_412_770, ""),
            ("Cabin Lease.pdf", F::Pdf, 202605221459, 462_112, ""),
            (
                "Field Recording.mov",
                F::Movie,
                ago(1, 7, 47),
                148_220_410,
                "",
            ),
            ("Kickoff Notes.md", F::Text, ago(1, 22, 11), 11_402, ""),
            ("Onboarding.pdf", F::Pdf, 202604111005, 2_118_004, ""),
            ("Passport Scan.png", F::Image, 202602280811, 3_940_112, ""),
            ("Reading List.md", F::Text, 202608262114, 4_118, ""),
            ("Receipts.zip", F::Archive, 202608241819, 41_004_882, ""),
            ("Ventura.dmg", F::Disk, 202603160931, 812_004_118, ""),
        ],
    ),
    (
        "projects",
        "Projects",
        "documents",
        &["Macintosh HD", "Users", "avery", "Documents", "Projects"],
        &[
            ("src", F::Folder, 202608301041, 0, "src"),
            ("Cargo.toml", F::Text, 202608301037, 1_812, ""),
            ("LICENSE", F::Text, 202606010900, 1_074, ""),
            ("README.md", F::Text, 202608291955, 9_140, ""),
            ("architecture.png", F::Image, 202608221420, 1_240_991, ""),
            ("demo.mov", F::Movie, 202608280003, 148_220_410, ""),
            ("notes.txt", F::Text, 202608300822, 2_204, ""),
            ("release.zip", F::Archive, 202608241819, 41_004_882, ""),
        ],
    ),
    (
        "src",
        "src",
        "projects",
        &[
            "Macintosh HD",
            "Users",
            "avery",
            "Documents",
            "Projects",
            "src",
        ],
        &[
            ("bridge.rs", F::Code, 202608301041, 28_114, ""),
            ("chrome.rs", F::Code, 202608300940, 41_882, ""),
            ("controls.rs", F::Code, 202608291711, 33_004, ""),
            ("lib.rs", F::Code, 202608301040, 3_218, ""),
            ("materials.rs", F::Code, 202608261512, 19_774, ""),
            ("surfaces.rs", F::Code, 202608281033, 24_512, ""),
            ("tokens.rs", F::Code, 202608300911, 35_302, ""),
        ],
    ),
    (
        "invoices",
        "Invoices",
        "documents",
        &["Macintosh HD", "Users", "avery", "Documents", "Invoices"],
        &[
            ("2026-04 Northwind.pdf", F::Pdf, 202604301601, 184_220, ""),
            ("2026-05 Northwind.pdf", F::Pdf, 202605311559, 187_004, ""),
            ("2026-06 Aldermoor.pdf", F::Pdf, 202606301712, 201_882, ""),
            ("2026-07 Aldermoor.pdf", F::Pdf, 202607281412, 199_140, ""),
            ("Rates.numbers", F::Sheet, 202601121030, 88_004, ""),
        ],
    ),
    (
        "tax",
        "Tax 2025",
        "documents",
        &["Macintosh HD", "Users", "avery", "Documents", "Tax 2025"],
        &[
            ("Return 2025.pdf", F::Pdf, 202601190904, 1_412_004, ""),
            ("Receipts.zip", F::Archive, 202601180822, 62_004_118, ""),
            ("Summary.numbers", F::Sheet, 202601171540, 140_882, ""),
            ("Correspondence.pdf", F::Pdf, 202512201105, 604_112, ""),
        ],
    ),
    (
        "desktop",
        "Desktop",
        "home",
        &["Macintosh HD", "Users", "avery", "Desktop"],
        &[
            ("Screenshots", F::Folder, 202608301122, 0, "screenshots"),
            ("Agenda.pages", F::Pages, 202608300901, 214_004, ""),
            ("Colour Study.png", F::Image, 202608291402, 6_204_118, ""),
            ("Aldermoor Brief.webloc", F::Web, 202608241033, 412, ""),
            ("scratch.txt", F::Text, 202608301105, 812, ""),
            ("Send To Print.pdf", F::Pdf, 202608281455, 3_204_002, ""),
        ],
    ),
    (
        "screenshots",
        "Screenshots",
        "desktop",
        &["Macintosh HD", "Users", "avery", "Desktop", "Screenshots"],
        &[
            (
                "Screenshot 2026-08-30 at 11.22.04.png",
                F::Image,
                202608301122,
                2_884_112,
                "",
            ),
            (
                "Screenshot 2026-08-30 at 09.14.51.png",
                F::Image,
                202608300914,
                3_112_884,
                "",
            ),
            (
                "Screenshot 2026-08-29 at 18.02.19.png",
                F::Image,
                202608291802,
                1_998_004,
                "",
            ),
            (
                "Screenshot 2026-08-28 at 08.41.02.png",
                F::Image,
                202608280841,
                2_204_881,
                "",
            ),
            (
                "Screenshot 2026-08-27 at 22.10.44.png",
                F::Image,
                202608272210,
                4_118_002,
                "",
            ),
        ],
    ),
    (
        "downloads",
        "Downloads",
        "home",
        &["Macintosh HD", "Users", "avery", "Downloads"],
        &[
            ("Fonts", F::Folder, 202608261019, 0, "fonts"),
            (
                "Aperture Lab.dmg",
                F::Archive,
                202608301004,
                412_004_118,
                "",
            ),
            ("Bank Statement.pdf", F::Pdf, 202608291204, 288_004, ""),
            ("Cassette.mp3", F::Audio, 202608271911, 8_812_004, ""),
            (
                "Field Recording.wav",
                F::Audio,
                202608261441,
                148_220_884,
                "",
            ),
            (
                "Invoice Template.pages",
                F::Pages,
                202608251102,
                402_118,
                "",
            ),
            ("Iris Trailer.mov", F::Movie, 202608242202, 884_112_004, ""),
            ("Lecture Notes.pdf", F::Pdf, 202608231015, 1_204_882, ""),
            ("Mixdown.wav", F::Audio, 202608221844, 62_004_118, ""),
            ("Sample Set.zip", F::Archive, 202608211533, 214_882_004, ""),
            ("Slides.key", F::Slides, 202608201209, 41_004_112, ""),
            ("Theme.zip", F::Archive, 202608190940, 1_884_002, ""),
            ("Wallpaper 6K.png", F::Image, 202608182201, 38_112_004, ""),
            ("dataset.csv", F::Text, 202608171104, 24_884_112, ""),
            ("firmware.bin", F::Archive, 202608160812, 8_204_118, ""),
            ("receipt.pdf", F::Pdf, 202608151633, 94_004, ""),
        ],
    ),
    (
        "fonts",
        "Fonts",
        "downloads",
        &["Macintosh HD", "Users", "avery", "Downloads", "Fonts"],
        &[
            ("Chartwell.zip", F::Archive, 202608261019, 4_118_882, ""),
            ("Duplet.zip", F::Archive, 202608240944, 6_204_112, ""),
            ("Ginto Nord.zip", F::Archive, 202608221530, 3_884_004, ""),
            ("Signifier.zip", F::Archive, 202608201122, 5_112_118, ""),
        ],
    ),
    (
        "pictures",
        "Pictures",
        "home",
        &["Macintosh HD", "Users", "avery", "Pictures"],
        &[
            ("Wallpapers", F::Folder, 202608141200, 0, "wallpapers"),
            (
                "Photos Library.photoslibrary",
                F::Program,
                202608301201,
                84_112_004_118,
                "",
            ),
            ("Contact Sheet.pdf", F::Pdf, 202607190944, 12_204_882, ""),
            (
                "Harbour at Dusk.png",
                F::Image,
                202606221911,
                18_884_004,
                "",
            ),
            ("Studio Test 04.png", F::Image, 202605301433, 22_112_884, ""),
        ],
    ),
    (
        "wallpapers",
        "Wallpapers",
        "pictures",
        &["Macintosh HD", "Users", "avery", "Pictures", "Wallpapers"],
        &[
            ("Basalt.png", F::Image, 202608141200, 14_884_112, ""),
            ("Dune 6K.png", F::Image, 202607281033, 28_004_118, ""),
            ("Fjord.png", F::Image, 202606110912, 19_882_004, ""),
            ("Sodium.png", F::Image, 202605021455, 16_112_884, ""),
        ],
    ),
    (
        "home",
        "avery",
        "users",
        HOME,
        &[
            ("Applications", F::Folder, 202608120901, 0, "applications"),
            ("Desktop", F::Folder, 202608301122, 0, "desktop"),
            ("Documents", F::Folder, 202608301037, 0, "documents"),
            ("Downloads", F::Folder, 202608301004, 0, "downloads"),
            ("Movies", F::Folder, 202607141911, 0, "movies"),
            ("Music", F::Folder, 202606221044, 0, "music"),
            ("Pictures", F::Folder, 202608301201, 0, "pictures"),
            ("Public", F::Folder, 202601010900, 0, "public"),
        ],
    ),
    (
        "movies",
        "Movies",
        "home",
        &["Macintosh HD", "Users", "avery", "Movies"],
        &[
            ("Iris Trailer.mov", F::Movie, 202607141911, 884_112_004, ""),
            ("Kitchen Test.mov", F::Movie, 202606301422, 214_882_118, ""),
            (
                "Timelapse 01.mov",
                F::Movie,
                202605281033,
                1_204_882_004,
                "",
            ),
        ],
    ),
    (
        "music",
        "Music",
        "home",
        &["Macintosh HD", "Users", "avery", "Music"],
        &[
            ("Cassette.mp3", F::Audio, 202606221044, 8_812_004, ""),
            ("Mixdown.wav", F::Audio, 202605191833, 62_004_118, ""),
            ("Sketch 04.mp3", F::Audio, 202604220911, 4_118_882, ""),
        ],
    ),
    (
        "public",
        "Public",
        "home",
        &["Macintosh HD", "Users", "avery", "Public"],
        &[
            ("Drop Box", F::Folder, 202601010900, 0, ""),
            ("Shared Notes.txt", F::Text, 202602141422, 2_118, ""),
        ],
    ),
    (
        "users",
        "Users",
        "hd",
        &["Macintosh HD", "Users"],
        &[
            ("avery", F::Folder, 202608301037, 0, "home"),
            ("Shared", F::Folder, 202601010900, 0, ""),
        ],
    ),
    (
        "applications",
        "Applications",
        "hd",
        &["Macintosh HD", "Applications"],
        &[],
    ),
    (
        "hd",
        "Macintosh HD",
        "",
        &["Macintosh HD"],
        &[
            ("Applications", F::Folder, 202608120901, 0, "applications"),
            ("Library", F::Folder, 202608290844, 0, "library"),
            ("System", F::Folder, 202607010900, 0, "system"),
            ("Users", F::Folder, 202608301037, 0, "users"),
        ],
    ),
    (
        "library",
        "Library",
        "hd",
        &["Macintosh HD", "Library"],
        &[
            ("Application Support", F::Folder, 202608290844, 0, ""),
            ("Fonts", F::Folder, 202607221011, 0, ""),
            ("Preferences", F::Folder, 202608301102, 0, ""),
            ("Scripts", F::Folder, 202604180933, 0, ""),
        ],
    ),
    (
        "system",
        "System",
        "hd",
        &["Macintosh HD", "System"],
        &[
            ("Applications", F::Folder, 202607010900, 0, ""),
            ("Library", F::Folder, 202607010900, 0, ""),
            ("Volumes", F::Folder, 202607010900, 0, ""),
        ],
    ),
    (
        "recents",
        "Recents",
        "",
        &["Recents"],
        &[
            ("bridge.rs", F::Code, 202608301041, 28_114, ""),
            ("Cargo.toml", F::Text, 202608301037, 1_812, ""),
            ("scratch.txt", F::Text, 202608301105, 812, ""),
            (
                "Screenshot 2026-08-30 at 11.22.04.png",
                F::Image,
                202608301122,
                2_884_112,
                "",
            ),
            (
                "Aperture Lab.dmg",
                F::Archive,
                202608301004,
                412_004_118,
                "",
            ),
            ("Agenda.pages", F::Pages, 202608300901, 214_004, ""),
            ("README.md", F::Text, 202608291955, 9_140, ""),
            ("Meeting Notes.txt", F::Text, 202608291642, 11_402, ""),
        ],
    ),
    (
        "airdrop",
        "AirDrop",
        "",
        &["AirDrop"],
        &[
            ("Avery's iPhone", F::Device, 202608301200, 0, ""),
            ("Studio Display", F::Device, 202608301200, 0, ""),
            ("Wren's MacBook Air", F::Device, 202608301200, 0, ""),
        ],
    ),
    (
        "icloud",
        "iCloud Drive",
        "",
        &["iCloud Drive"],
        &[
            ("Documents", F::Folder, 202608301037, 0, "documents"),
            ("Desktop", F::Folder, 202608301122, 0, "desktop"),
            ("Keynote", F::Folder, 202607041122, 0, ""),
            ("Numbers", F::Folder, 202608070912, 0, ""),
            ("Pages", F::Folder, 202605221459, 0, ""),
            ("Shortcuts", F::Folder, 202603110844, 0, ""),
            (
                "Archive 2025.zip",
                F::Archive,
                202601020911,
                2_884_112_004,
                "",
            ),
        ],
    ),
    (
        "shared",
        "Shared",
        "",
        &["Shared"],
        &[
            ("Aldermoor Brief.pdf", F::Pdf, 202608221533, 4_118_882, ""),
            ("Northwind Assets", F::Folder, 202608191044, 0, ""),
            ("Rates.numbers", F::Sheet, 202601121030, 88_004, ""),
        ],
    ),
    (
        "timemachine",
        "Time Machine",
        "",
        &["Time Machine"],
        &[
            ("Backups 2026-08-30", F::Volume, 202608300400, 0, ""),
            ("Backups 2026-08-29", F::Volume, 202608290400, 0, ""),
            ("Backups 2026-08-28", F::Volume, 202608280400, 0, ""),
            ("Backups 2026-08-24", F::Volume, 202608240400, 0, ""),
        ],
    ),
    (
        "network",
        "Network",
        "",
        &["Network"],
        &[
            ("aldermoor-nas", F::Device, 202608301200, 0, ""),
            ("studio-mini", F::Device, 202608301200, 0, ""),
        ],
    ),
    (
        "developer",
        "Developer",
        "home",
        &["Macintosh HD", "Users", "avery", "Developer"],
        &[
            ("Aldermoor", F::Folder, ago(0, 9, 12), 0, "projects"),
            ("Northwind", F::Folder, 202608221420, 0, ""),
            ("Scratch", F::Folder, 202607091044, 0, ""),
            ("bootstrap.sh", F::Code, 202606141902, 3_118, ""),
            ("toolchain.toml", F::Text, 202605301155, 812, ""),
        ],
    ),
    (
        "bakery",
        "Bakery",
        "",
        &["Bakery"],
        &[
            ("Downloads", F::Folder, ago(1, 18, 3), 0, "downloads-bakery"),
            ("Masters", F::Folder, 202608180944, 0, ""),
            ("Renders", F::Folder, 202608291502, 0, ""),
            ("Iris Trailer.mov", F::Movie, 202608242202, 884_112_004, ""),
            ("Session 04.wav", F::Audio, 202608261441, 148_220_884, ""),
        ],
    ),
    (
        "downloads-bakery",
        "Downloads",
        "bakery",
        &["Bakery", "Downloads"],
        &[
            ("Chartwell.zip", F::Archive, 202608261019, 4_118_882, ""),
            ("Plate Set 12.zip", F::Archive, 202608201122, 88_204_118, ""),
            ("Reference.pdf", F::Pdf, 202608141731, 8_412_770, ""),
        ],
    ),
    (
        "trash",
        "Trash",
        "",
        &["Trash"],
        &[
            ("Old Mixdown.wav", F::Audio, 202608221844, 62_004_118, ""),
            (
                "Screenshot 2026-08-02 at 09.14.22.png",
                F::Image,
                202608020914,
                1_884_002,
                "",
            ),
            ("draft.md", F::Text, 202607301613, 2_204, ""),
        ],
    ),
];

pub(crate) const APPS: &[&str] = &[
    "App Store",
    "Automator",
    "Books",
    "Calculator",
    "Calendar",
    "Chess",
    "Clock",
    "Contacts",
    "Dictionary",
    "FaceTime",
    "Find My",
    "Font Book",
    "Freeform",
    "GarageBand",
    "Home",
    "Image Capture",
    "Journal",
    "Keynote",
    "Mail",
    "Maps",
    "Messages",
    "Mission Control",
    "Music",
    "News",
    "Notes",
    "Numbers",
    "Pages",
    "Passwords",
    "Photo Booth",
    "Photos",
    "Podcasts",
    "Preview",
    "QuickTime Player",
    "Reminders",
    "Safari",
    "Shortcuts",
    "Stickies",
    "Stocks",
    "TextEdit",
    "Time Machine",
    "TV",
    "Voice Memos",
    "Weather",
];

pub(crate) const AVAILABLE_BYTES: i64 = 218_420_000_000;

/// `(year, month, day, hour, minute)` for a stamp, an [`ago`] offset resolved against `today`.
pub(crate) fn stamp_parts(stamp: u64, today: (i64, i64, i64)) -> (i64, i64, i64, i64, i64) {
    if stamp >= RELATIVE {
        let offset = (stamp - RELATIVE) as i64;
        let (year, month, day) = today;
        return (
            year,
            month,
            day - offset / 10_000,
            offset / 100 % 100,
            offset % 100,
        );
    }
    let stamp = stamp as i64;
    (
        stamp / 100_000_000,
        stamp / 1_000_000 % 100,
        stamp / 10_000 % 100,
        stamp / 100 % 100,
        stamp % 100,
    )
}

/// The date a stamp names, in the user's own calendar and timezone, `back` calendar days earlier.
pub(crate) fn stamp_date(
    calendar: &NSCalendar,
    today: (i64, i64, i64),
    stamp: u64,
    back: i64,
) -> Option<objc2::rc::Retained<NSDate>> {
    let (year, month, day, hour, minute) = stamp_parts(stamp, today);
    // The day is left out of range on purpose: `NSCalendar` carries it into the right month.
    let parts = NSDateComponents::new();
    parts.setYear(year as isize);
    parts.setMonth(month as isize);
    parts.setDay((day - back) as isize);
    parts.setHour(hour as isize);
    parts.setMinute(minute as isize);
    parts.setSecond(0);
    calendar.dateFromComponents(&parts)
}

/// A day alone, `Mar 19, 2022`, as Finder dates a range over a selection.
pub(crate) fn day_string(seconds: f64) -> String {
    let formatter = NSDateFormatter::new();
    formatter.setDateStyle(NSDateFormatterStyle::MediumStyle);
    formatter.setTimeStyle(NSDateFormatterStyle::NoStyle);
    formatter
        .stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(seconds))
        .to_string()
}

pub(crate) fn byte_string(bytes: u64) -> String {
    NSByteCountFormatter::stringFromByteCount_countStyle(
        bytes as i64,
        NSByteCountFormatterCountStyle::File,
    )
    .to_string()
}

#[derive(Clone)]
pub(crate) struct Row {
    pub(crate) name: SharedString,
    pub(crate) modified: SharedString,
    pub(crate) stamp: u64,
    pub(crate) size: SharedString,
    pub(crate) bytes: u64,
    pub(crate) created: SharedString,
    /// The same two moments as seconds since the reference date, for ranges over a selection.
    pub(crate) modified_at: f64,
    pub(crate) created_at: f64,
    pub(crate) dimensions: Option<SharedString>,
    pub(crate) duration: Option<SharedString>,
    pub(crate) face: Face,
    pub(crate) opens: Option<&'static str>,
    pub(crate) artwork: Option<FileType<'static>>,
}

pub(crate) struct Folder {
    pub(crate) title: &'static str,
    pub(crate) parent: Option<&'static str>,
    pub(crate) crumbs: &'static [&'static str],
    pub(crate) rows: Vec<Row>,
}

#[derive(Clone)]
pub(crate) struct Line {
    pub(crate) key: SharedString,
    pub(crate) row: Row,
    pub(crate) depth: usize,
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) enum Column {
    Name,
    Modified,
    Size,
    Kind,
}

pub(crate) fn build_store() -> HashMap<&'static str, Folder> {
    let formatter = NSDateFormatter::new();
    formatter.setDateStyle(NSDateFormatterStyle::ShortStyle);
    formatter.setTimeStyle(NSDateFormatterStyle::ShortStyle);
    formatter.setDoesRelativeDateFormatting(true);

    let calendar = NSCalendar::currentCalendar();
    let now = NSDate::date();
    let fields = NSCalendarUnit::Year | NSCalendarUnit::Month | NSCalendarUnit::Day;
    let parts = calendar.components_fromDate(fields, &now);
    let today = (
        parts.year() as i64,
        parts.month() as i64,
        parts.day() as i64,
    );

    const FRAMES: [(u32, u32); 6] = [
        (1920, 1080),
        (3840, 2160),
        (1280, 720),
        (2560, 1440),
        (6016, 3384),
        (1170, 2532),
    ];
    let row = |src: &Src| {
        let (name, face, stamp, bytes, opens) = *src;
        let text = |back: i64| {
            stamp_date(&calendar, today, stamp, back)
                .map(|date| formatter.stringFromDate(&date).to_string())
                .unwrap_or_default()
        };
        let seconds = |back: i64| {
            stamp_date(&calendar, today, stamp, back)
                .map_or(0., |date| date.timeIntervalSinceReferenceDate())
        };
        let created_back = 1 + (bytes % 407) as i64;
        Row {
            name: name.into(),
            modified: text(0).into(),
            created: text(created_back).into(),
            modified_at: seconds(0),
            created_at: seconds(created_back),
            dimensions: matches!(face, F::Image | F::Movie).then(|| {
                let (w, h) = FRAMES[(bytes % FRAMES.len() as u64) as usize];
                SharedString::from(format!("{w} × {h}"))
            }),
            duration: matches!(face, F::Movie | F::Audio).then(|| {
                let seconds = 7 + bytes % 5_393;
                SharedString::from(format!("{}:{:02}", seconds / 60, seconds % 60))
            }),
            stamp,
            size: if face == F::Folder || face == F::Volume || face == F::Device {
                "--".into()
            } else {
                byte_string(bytes).into()
            },
            bytes,
            face,
            opens: (!opens.is_empty()).then_some(opens),
            artwork: face.artwork(name.rsplit_once('.').map(|(_, tail)| tail)),
        }
    };

    let mut store: HashMap<&'static str, Folder> = DIRS
        .iter()
        .map(|(id, title, parent, crumbs, items)| {
            (
                *id,
                Folder {
                    title,
                    parent: (!parent.is_empty()).then_some(*parent),
                    crumbs,
                    rows: items.iter().map(row).collect(),
                },
            )
        })
        .collect();

    if let Some(apps) = store.get_mut("applications") {
        apps.rows = APPS
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let index = index as u64;
                let stamp = 202601010000
                    + (index % 8 + 1) * 1_000_000
                    + (index % 27 + 1) * 10_000
                    + (index % 11 + 8) * 100
                    + index * 7 % 60;
                let bytes = 14_000_000 + index * 9_300_517 % 1_400_000_000;
                row(&(name, F::Program, stamp, bytes, ""))
            })
            .collect();
    }
    store
}
