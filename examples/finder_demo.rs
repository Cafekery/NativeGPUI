//! A Finder front end. Real macOS chrome, entirely fictional files.
//!
//! Nothing here reads the filesystem: the tree in `DIRS` is invented, the sidebar's Locations are
//! props, and a row's icon comes from a *type* lookup — the system's own artwork for `.pdf`, for a
//! folder, for an application bundle — never from a file on disk. Dates and sizes are formatted by
//! `NSDateFormatter` and `NSByteCountFormatter`, so the columns read in the user's own locale,
//! timezone and unit conventions rather than in a format written down here. Relative date
//! formatting is what supplies Finder's `Today` and `Yesterday` tiers, and the switch happens on
//! the calendar day rather than after some number of hours: a file touched eight hours ago reads
//! `Yesterday` as soon as midnight has passed.
//!
//! What is native: the sidebar (`NSSplitViewItem.sidebar`, `NSOutlineView` rows), the unified
//! toolbar and its items, the back/forward and view-mode segmented controls, the search field and
//! the icon-size slider. What GPUI draws: the file list, its headers, the path bar, the status bar
//! and the info pane — all in system fonts and semantic colours so the two halves agree.
//!
//! `NG_QUIT=<frames>` exits after N frames, so a smoke run ends on its own.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::env;

use block2::{RcBlock, StackBlock};
use gpui::{
    AnyElement, App, Application, Bounds, ClickEvent, ClipboardItem, FocusHandle, FontWeight, Hsla,
    KeyBinding, KeyDownEvent, MouseButton, ScrollStrategy, SharedString, UniformListScrollHandle,
    Window, WindowBounds, WindowOptions, actions, div, point, prelude::*, px, size, uniform_list,
};
use native_gpui::sidebar::{
    NativeSidebar, SidebarItem, SidebarMetrics, SidebarOptions, sidebar_group, sidebar_item,
};
use native_gpui::tokens::{
    self, FileType, SymbolColor, SystemColor, SystemTheme, file_icon, sf_symbol_colored,
    system_color,
};
use native_gpui::{
    Material, MenuEntry, MenuView, ToolbarGroupWidth, ToolbarItem, ToolbarSegment,
    ToolbarSelection, ToolbarStyle, chrome_host, install_menus, native_root, native_slider,
    set_toolbar, window_options,
};
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{NSBezierPath, NSColor, NSImage, NSMenuToolbarItem, NSWindowTitleVisibility};
use objc2_foundation::{
    NSByteCountFormatter, NSByteCountFormatterCountStyle, NSCalendar, NSCalendarUnit, NSDate,
    NSDateComponents, NSDateFormatter, NSDateFormatterStyle, NSPoint, NSRect, NSSize,
};

use self::Face as F;

// ---------------------------------------------------------------------------------------------
// The invented filesystem
// ---------------------------------------------------------------------------------------------

/// What a row is, which decides its icon, its Kind column and — where no real artwork exists for
/// the type — the tint of the symbol standing in for it.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Face {
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
    fn symbol(self) -> &'static str {
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

    fn kind(self) -> &'static str {
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

    /// The type whose own artwork the system draws for this row, or `None` to fall back to
    /// [`Face::symbol`]. Keyed on the face and not only on the extension because Finder hides the
    /// `.app` of a bundle and a folder has no extension at all, yet both have real icons; the
    /// faces that answer `None` are props with no file type behind them.
    fn artwork(self, extension: Option<&'static str>) -> Option<FileType<'static>> {
        match self {
            F::Folder => Some(FileType::Folder),
            F::Program => Some(FileType::Extension("app")),
            F::Volume | F::Device => None,
            _ => extension.map(FileType::Extension),
        }
    }

    /// Hierarchical rather than one flat colour: a symbol with a secondary layer keeps it, which is
    /// what stops a tinted glyph looking like a silhouette.
    fn tint(self) -> SymbolColor {
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

/// `name`, face, modified as `yyyymmddhhmm` or an [`ago`] offset, size in bytes, the directory this
/// row opens (`""` when it opens nothing). Folder sizes are ignored: Finder shows `--`.
type Src = (&'static str, Face, u64, u64, &'static str);

/// Above every `yyyymmddhhmm` literal, which is twelve digits, so a relative stamp cannot collide
/// with a real one.
const RELATIVE: u64 = 1_000_000_000_000;

/// A stamp meaning `days` calendar days before launch at `hour:minute`. A frozen literal can never
/// be today, so without this the list would never show the `Today` and `Yesterday` tiers that are
/// two thirds of what Finder's date column does.
const fn ago(days: u64, hour: u64, minute: u64) -> u64 {
    RELATIVE + days * 10_000 + hour * 100 + minute
}

/// `id`, display name, parent id (`""` at the root), breadcrumbs, contents.
type Dir = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [Src],
);

const HOME: &[&str] = &["Macintosh HD", "Users", "avery"];

const DIRS: &[Dir] = &[
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
];

/// Applications are generated rather than written out: a long directory is what makes
/// `uniform_list` worth using, and forty hand-typed rows say nothing forty generated ones do not.
const APPS: &[&str] = &[
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

const AVAILABLE_BYTES: i64 = 218_420_000_000;

/// Sidebar group id, title, and its rows as (id, title, SF Symbol, badge — 0 for none).
type Group = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str, &'static str, i64)],
);

const SIDEBAR: &[Group] = &[
    (
        "g-favourites",
        "Favorites",
        &[
            ("airdrop", "AirDrop", "dot.radiowaves.right", 0),
            ("recents", "Recents", "clock", 0),
            ("applications", "Applications", "square.grid.2x2", 0),
            ("desktop", "Desktop", "menubar.dock.rectangle", 0),
            ("documents", "Documents", "doc", 0),
            ("downloads", "Downloads", "arrow.down.circle", 0),
            ("pictures", "Pictures", "photo", 0),
            ("movies", "Movies", "film", 0),
            ("music", "Music", "music.note", 0),
            ("home", "avery", "house", 0),
        ],
    ),
    (
        "g-icloud",
        "iCloud",
        &[
            ("icloud", "iCloud Drive", "icloud", 0),
            ("shared", "Shared", "person.2", 0),
        ],
    ),
    (
        "g-locations",
        "Locations",
        &[
            ("hd", "Macintosh HD", "internaldrive", 0),
            ("timemachine", "Time Machine", "clock.arrow.circlepath", 0),
            ("network", "Network", "globe", 0),
        ],
    ),
];

// ---------------------------------------------------------------------------------------------
// System formatting
// ---------------------------------------------------------------------------------------------

/// `(year, month, day, hour, minute)` for a stamp, an [`ago`] offset resolved against `today`.
///
/// A relative day is left deliberately out of range — `day - 1` on the 1st of a month comes out
/// zero, and three days before the 2nd of January comes out negative. `NSCalendar` normalises both
/// into the previous month or year, which is why this does no carrying of its own.
fn stamp_parts(stamp: u64, today: (i64, i64, i64)) -> (i64, i64, i64, i64, i64) {
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

/// The date a stamp names, in the user's own calendar and timezone.
///
/// Built from components rather than from an epoch offset: the relative stamps have to land on a
/// calendar day, and a day is not 86,400 seconds long twice a year.
fn stamp_date(
    calendar: &NSCalendar,
    today: (i64, i64, i64),
    stamp: u64,
) -> Option<objc2::rc::Retained<NSDate>> {
    let (year, month, day, hour, minute) = stamp_parts(stamp, today);
    let parts = NSDateComponents::new();
    parts.setYear(year as isize);
    parts.setMonth(month as isize);
    parts.setDay(day as isize);
    parts.setHour(hour as isize);
    parts.setMinute(minute as isize);
    parts.setSecond(0);
    calendar.dateFromComponents(&parts)
}

fn byte_string(bytes: u64) -> String {
    NSByteCountFormatter::stringFromByteCount_countStyle(
        bytes as i64,
        NSByteCountFormatterCountStyle::File,
    )
    .to_string()
}

// ---------------------------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
struct Row {
    name: SharedString,
    modified: SharedString,
    stamp: u64,
    size: SharedString,
    bytes: u64,
    face: Face,
    opens: Option<&'static str>,
    /// Resolved once at build time: a lookup plus raster is cached, but deciding *which* type to
    /// ask for from a name and a face is this file's own work and does not need repeating per frame.
    artwork: Option<FileType<'static>>,
}

struct Folder {
    title: &'static str,
    parent: Option<&'static str>,
    crumbs: &'static [&'static str],
    rows: Vec<Row>,
}

/// One visible line: a row plus where it sits in the disclosure tree.
#[derive(Clone)]
struct Line {
    key: SharedString,
    row: Row,
    depth: usize,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Column {
    Name,
    Modified,
    Size,
    Kind,
}

/// Column widths as Finder *draws* them, which are not the widths it stores — Name is stored 178
/// and drawn 197. Named rather than repeated, because the header and the rows are laid out in
/// different functions and a literal in only one of them is exactly how the two stop lining up.
/// Name takes whatever is left, so the pane can be any width and the rest still land together.
const COL_MODIFIED: f32 = 181.;
const COL_SIZE: f32 = 75.;
const COL_KIND: f32 = 121.;

const COLUMNS: &[(Column, &str, f32)] = &[
    (Column::Name, "Name", 0.),
    (Column::Modified, "Date Modified", COL_MODIFIED),
    (Column::Size, "Size", COL_SIZE),
    (Column::Kind, "Kind", COL_KIND),
];

/// Finder's Sort By / Group By list, read out of its own View menu. `None` for the four Finder
/// offers that this listing has no column for — the row is still drawn, because a menu missing four
/// of eight rows does not look like Finder's.
const GROUPINGS: &[(Option<Column>, &str)] = &[
    (Some(Column::Name), "Name"),
    (Some(Column::Kind), "Kind"),
    (None, "Date Last Opened"),
    (None, "Date Added"),
    (Some(Column::Modified), "Date Modified"),
    (None, "Date Created"),
    (Some(Column::Size), "Size"),
];

const ICON_VIEW: usize = 0;
const LIST_VIEW: usize = 1;
const COLUMN_VIEW: usize = 2;
const GALLERY_VIEW: usize = 3;

/// Keymap context for the file list. Scopes ⌘A and ⌘C to it, so they stay the menu's — and the
/// hosted search field's — whenever focus is anywhere else.
const LIST_CONTEXT: &str = "FinderList";

actions!(finder_demo, [SelectAll, CopyNames]);

/// Finder's toolbar band is 52pt and AppKit owns it: the height comes from the unified toolbar
/// style, and the GPUI tree below it is inset by `SidebarMetrics::top` rather than by a constant.
/// Every capsule in it is a stadium 36pt tall at y 8…44 with a radius of half its height — none of
/// which is written down here, because `NSToolbarItem` draws all of it.
///
/// What is written down are the two glyphs no symbol name reaches. Both are hand-drawn
/// approximations, measured off a window capture rather than guessed, and neither is claimed to be
/// the real artwork.
///
/// AirDrop: three concentric 1pt arcs about a common centre with a 2pt filled dot at it, each arc
/// interrupted by a gap centred on straight down.
const AIRDROP_RADII: [f64; 3] = [8.5, 6.0, 3.5];
const AIRDROP_DOT_R: f64 = 1.0;
/// Half the angular gap at the bottom of each arc, in degrees. The captured arcs run out at 29.5°,
/// 31° and 38.7° from straight down measured outermost first, which a single gap fits to within
/// the antialiased tail a threshold eats.
const AIRDROP_GAP: f64 = 32.0;
/// The image box the arcs are centred in. An `NSToolbarItem`'s width is its image's width plus a
/// constant 24pt of padding — measured, by building the item at 20 and reading back 44 — and
/// Finder's AirDrop item is 45, so the box is 21. The 1.5pt of slack round the 18pt of ink also
/// keeps the outer stroke's antialiasing off the image edge, which would read as a thin arc.
const AIRDROP_BOX: f64 = 21.;
/// The Group item's grid: an 18pt-wide stack of a rule, three 5pt square outlines, a rule, and
/// three more squares, with no rule under the bottom row. `y` is measured down from the ink top,
/// which is what the capture gives. The chevron beside it is `NSMenuToolbarItem`'s own indicator.
const GRID_W: f64 = 18.;
const GRID_H: f64 = 17.5;
const GRID_RULE_H: f64 = 1.5;
const GRID_CELL: f64 = 5.;
const GRID_GAP: f64 = 1.5;
/// Rule, gap, square row, gap, rule — the four y offsets the capture reads at 17.0, 20.0, 26.5
/// and 29.5 against an ink top of 17.0.
const GRID_ROWS: [f64; 4] = [0., 3., 9.5, 12.5];
/// The image box the 18pt grid is centred in. Finder's Group item is 58pt against 54 for an 18pt
/// image, and item width tracks image width one for one, so the box is 22.
const GRID_BOX_W: f64 = 22.;
/// Finder's column header band. Its own bottom hairline is the only line in the chrome above the
/// list: there is none under the toolbar, which shares one continuous background with the header.
const HEADER_H: f32 = 28.;
const ICON_SIZE: f32 = 32.;
/// Finder's list row at the view options the target window uses — large icons, text size 14. The
/// same rule holds at 16pt icons and 21pt rows, so the row follows the icon rather than the other
/// way round.
const ROW_H: f32 = ICON_SIZE + 4.;
/// Measured from the list pane's leading edge, not from the Name column: the icon sits 26 in, the
/// label a 6pt gap after it, and the disclosure triangle's 15pt hit area ends where the icon
/// begins. The label offset moves with the icon size; nothing else here does.
const ICON_X: f32 = 26.;
const LABEL_X: f32 = ICON_X + ICON_SIZE + 6.;
const TWIST_W: f32 = 15.;
/// The columns region is inset this far from both pane edges. Both the selection *and* the
/// alternating row band span exactly it, as one rounded shape — neither runs the full pane width.
const PANE_INSET: f32 = 10.;
/// Read off a real window with the area→radius conversion calibrated against AppKit's own
/// `NSBezierPath(roundedRect:)`: the uncalibrated number is about 1pt low, because the formula
/// assumes an ideal quarter-disc where a rendered corner spreads its coverage over the arc.
const ROW_PILL_RADIUS: f32 = 8.;
/// The header's label inset does not follow the icon size the way a row's does: at 16pt icons the
/// two coincide at 46, and at 32pt they do not.
const HEADER_LABEL_X: f32 = 46.;
/// A header separator is 1pt wide and 24 of the band's 28pt — inset from the top, flush to the
/// bottom, where it runs into the hairline.
const SEP_TOP: f32 = 4.;
/// Text inset inside a fixed column. Size measures it from the trailing edge instead, which is how
/// three different strings there come out sharing one right edge.
const CELL_PAD: f32 = 6.;
/// The sort indicator's inset from the sorted column's trailing edge.
const SORT_INSET: f32 = 7.5;
/// The list's font size is Finder's per-folder `text size` view option and not a system metric: a
/// fresh folder defaults to 13, and the window this is matched against is set to 14.
const ROW_FONT: f32 = 14.;
/// Finder's table starts this far below the header hairline, so the first row's band does not touch
/// the line.
const LIST_TOP_GAP: f32 = 5.;
const BAR_H: f32 = 26.;
/// The right pane including the 1pt divider on its leading edge, which is what leaves Finder 594pt
/// of list in a 1026pt window. Get it wrong and every column lands somewhere else.
const PREVIEW_W: f32 = 240.;
/// Chosen for where it leaves the *list*, not for its own width: the sidebar reports an 8pt gutter
/// between its trailing edge and the pane, and Finder's list starts at 192. Measured off a capture
/// — at 192 the pane began at 200 and every column sat 8pt right of Finder's.
const SIDEBAR_W: f32 = 184.;
const INDENT: f32 = 15.;
/// The browser (column) view keeps the compact row it always had: only the list view was measured.
const BROWSER_ROW_H: f32 = 21.;

/// Toolbar item identifiers. Named because two of them are matched again after `set_toolbar` to
/// write drawn artwork onto the items AppKit built.
const NAV_ID: &str = "ng.finder.nav";
const AIRDROP_ID: &str = "ng.finder.airdrop";
const VIEW_ID: &str = "ng.finder.view";
const GROUP_ID: &str = "ng.finder.group";
const SHARE_ID: &str = "ng.finder.share";
const TAG_ID: &str = "ng.finder.tag";
const ACTION_ID: &str = "ng.finder.action";
const SEARCH_ID: &str = "ng.finder.search";

/// What the view-mode capsule collapses to when the search field takes the room: Finder's popup
/// reads **as List**, so the segment labels are the View menu's own wording rather than the one-word
/// names a segmented control would carry.
const VIEW_LABELS: &[&str] = &["as Icons", "as List", "as Columns", "as Gallery"];
/// The glyph per view mode. Gallery is `squares.below.rectangle`, which beat every other candidate
/// 0.987 to under 0.775 — the obvious `square.grid.3x1.below.line.grid.1x2` did not place.
const VIEW_SYMBOLS: &[&str] = &[
    "square.grid.2x2",
    "list.bullet",
    "rectangle.split.3x1",
    "squares.below.rectangle",
];
/// What the search field widens to on focus. `NSSearchToolbarItem` adds 8pt of item padding to
/// this, and it is the *item* width that drives the reflow: at 242 the item came out 250 against
/// Finder's 248 and every group left of it moved 205pt instead of 203.
const SEARCH_W: f32 = 240.;

/// Icon-view tile: how much wider than the icon the cell is, and the gap between cells. The pitch
/// is the sum, and leaving the gap out of it is how the last column ends up clipped.
const TILE_MARGIN: f32 = 56.;
const TILE_GAP: f32 = 8.;

/// Finder's tag colours, in its own order. Seven, ending in Gray: it is the last dot in the strip
/// the Action menu shows, and a six-dot strip is visibly one short of Finder's.
const TAGS: &[(&str, SystemColor)] = &[
    ("Red", SystemColor::SystemRed),
    ("Orange", SystemColor::SystemOrange),
    ("Yellow", SystemColor::SystemYellow),
    ("Green", SystemColor::SystemGreen),
    ("Blue", SystemColor::SystemBlue),
    ("Purple", SystemColor::SystemPurple),
    ("Gray", SystemColor::SystemGray),
];

fn build_store() -> HashMap<&'static str, Folder> {
    // Short date + short time with relative formatting on, which is the combination that produces
    // Finder's own three tiers — `Today, 4:34 AM`, `Yesterday, 7:47 AM`, `8/26/26, 9:14 PM` —
    // including the narrow no-break space before the meridiem, which a hand-written format string
    // gets wrong by about 2pt a row. Medium date style would join with " at " instead of ", ".
    let formatter = NSDateFormatter::new();
    formatter.setDateStyle(NSDateFormatterStyle::ShortStyle);
    formatter.setTimeStyle(NSDateFormatterStyle::ShortStyle);
    formatter.setDoesRelativeDateFormatting(true);

    // The user's own calendar and timezone, not GMT: `Today` has to mean their today, and the
    // switch is a calendar-day boundary rather than a count of hours — a file touched eight hours
    // ago reads `Yesterday` once midnight has passed.
    let calendar = NSCalendar::currentCalendar();
    let now = NSDate::date();
    let fields = NSCalendarUnit::Year | NSCalendarUnit::Month | NSCalendarUnit::Day;
    let parts = calendar.components_fromDate(fields, &now);
    let today = (
        parts.year() as i64,
        parts.month() as i64,
        parts.day() as i64,
    );

    let row = |src: &Src| {
        let (name, face, stamp, bytes, opens) = *src;
        let modified = stamp_date(&calendar, today, stamp)
            .map(|date| formatter.stringFromDate(&date).to_string())
            .unwrap_or_default();
        Row {
            name: name.into(),
            modified: modified.into(),
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

// ---------------------------------------------------------------------------------------------
// The two toolbar glyphs that have no symbol name
// ---------------------------------------------------------------------------------------------

/// Finder's AirDrop glyph, hand-drawn. **Not a match, and not claimed as one.**
///
/// `airdrop`, `airdrop.circle` and `airdrop.square` are all in SF Hello's catalogue marked
/// *private*, and `NSImage(systemSymbolName:)` returns nil for every one of them; the closest
/// public name scores 0.75 by normalised cross-correlation, which is a different glyph. So this
/// draws the shape a window capture reads — three concentric arcs about one centre, gapped at the
/// bottom, over a filled dot — and leaves the fidelity claim to the measurement.
///
/// Template, so AppKit tints it exactly as it tints a symbol: enabled, disabled, and inside a
/// pressed capsule are all the system's decisions rather than a colour written down here.
fn airdrop_glyph() -> Retained<NSImage> {
    let box_size = AIRDROP_BOX;
    let handler = RcBlock::new(move |_: NSRect| -> Bool {
        let centre = NSPoint::new(box_size / 2., box_size / 2.);
        NSColor::blackColor().setStroke();
        NSColor::blackColor().setFill();
        for radius in AIRDROP_RADII {
            // One path per arc: appending an arc to a path that already has a current point draws
            // a line to the new arc's start, which would web the three rings together.
            let arc = NSBezierPath::new();
            arc.setLineWidth(1.);
            arc.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(
                centre,
                radius,
                AIRDROP_GAP - 90.,
                270. - AIRDROP_GAP,
            );
            arc.stroke();
        }
        NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            NSPoint::new(centre.x - AIRDROP_DOT_R, centre.y - AIRDROP_DOT_R),
            NSSize::new(AIRDROP_DOT_R * 2., AIRDROP_DOT_R * 2.),
        ))
        .fill();
        Bool::YES
    });
    // `RcBlock`, never `StackBlock`: the image keeps the handler and re-runs it on every appearance
    // change, so a block living on this frame is a use-after-free the first time dark mode flips.
    let image = NSImage::imageWithSize_flipped_drawingHandler(
        NSSize::new(box_size, box_size),
        false,
        &handler,
    );
    image.setTemplate(true);
    image
}

/// The Group item's grid glyph, hand-drawn. **Not a match, and not claimed as one.**
///
/// The closest public symbol scores 0.65, far too weak to substitute, so this reproduces the
/// captured geometry: an 18pt rule, a row of three 5pt square outlines, a second rule, a second
/// row, and no rule underneath.
///
/// Drawn in a flipped context so the offsets read top-down, the way the capture measures them.
fn group_glyph() -> Retained<NSImage> {
    let handler = RcBlock::new(move |_: NSRect| -> Bool {
        let inset = (GRID_BOX_W - GRID_W) / 2.;
        NSColor::blackColor().setStroke();
        NSColor::blackColor().setFill();
        for y in [GRID_ROWS[0], GRID_ROWS[2]] {
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(inset, y),
                NSSize::new(GRID_W, GRID_RULE_H),
            ));
        }
        for y in [GRID_ROWS[1], GRID_ROWS[3]] {
            for column in 0..3 {
                let x = inset + column as f64 * (GRID_CELL + GRID_GAP);
                // Inset by half the line width, so the *outer* edge of the stroke lands on the
                // 5pt cell the capture measures rather than 0.5pt outside it.
                let cell = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    NSRect::new(
                        NSPoint::new(x + 0.5, y + 0.5),
                        NSSize::new(GRID_CELL - 1., GRID_CELL - 1.),
                    ),
                    1.,
                    1.,
                );
                cell.setLineWidth(1.);
                cell.stroke();
            }
        }
        Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(
        NSSize::new(GRID_BOX_W, GRID_H),
        true,
        &handler,
    );
    image.setTemplate(true);
    image
}

/// Everything a `ToolbarItem` cannot say, said directly to the items AppKit has just built.
///
/// Four things live here, and each is a property of one `NSToolbarItem` rather than of the item
/// list, so none of them is expressible in a `ToolbarItem` spec:
///
/// **The drawn glyphs.** `ToolbarItem` names its image by SF Symbol, which is the right shape for
/// eleven of Finder's thirteen glyphs and cannot express the other two at all. Rather than give up
/// the real `NSToolbarItem` subclasses — and with them the capsule, the grey selection pill and the
/// search reflow, which is the entire point — those two items are built with a name that resolves to
/// nothing and the artwork is written on afterwards. A failed override therefore shows an empty
/// capsule, which reads as deliberate, instead of a confidently wrong glyph.
///
/// **`navigational`.** Without it AppKit lays the whole item run out *after* the window title,
/// which put back/forward at x 471 against Finder's 196 and left 279pt of empty band in front of
/// it. Marking the group navigational moves it into the leading section ahead of the title, which
/// is exactly Finder's order: back/forward, then the path, then everything else.
///
/// **`showsIndicator`.** Finder's Action item is 41pt — the same width as Share — and carries no
/// chevron; the Group item does. `NSMenuToolbarItem` shows one by default, and it cost 13pt.
///
/// **`autoenablesItems`.** Automatic validation asks the target for `validateMenuItem:`, and where
/// there is none it enables any row whose target merely responds to the action — which every row
/// here does. Leave it on and `MenuEntry::disabled` is a silent no-op, so Finder's greyed
/// *New Folder with Selection* comes up live. Nested menus do not inherit the flag either, but
/// `sync_menu` already turns it off on every submenu it builds.
fn finish_toolbar_items(window: &Window) -> usize {
    let Some(toolbar) = chrome_host(window).window().and_then(|w| w.toolbar()) else {
        return 0;
    };
    let mut done = 0;
    for item in toolbar.items().iter() {
        let id = item.itemIdentifier().to_string();
        match id.as_str() {
            NAV_ID => item.setNavigational(true),
            AIRDROP_ID => {
                item.setImage(Some(&airdrop_glyph()));
                done += 1;
            }
            GROUP_ID => {
                item.setImage(Some(&group_glyph()));
                done += 1;
            }
            _ => {}
        }
        if let Some(menu) = item.downcast_ref::<NSMenuToolbarItem>() {
            menu.menu().setAutoenablesItems(false);
            menu.setShowsIndicator(id == GROUP_ID);
        }
    }
    done
}

/// The Group item's menu: Finder's grouping choices, in the wording and order its own View menu
/// uses. Decorative — the demo does not group — so the checked row is the sort the list is already
/// on, which is the state the menu would be reporting if it worked.
fn group_menu(sort: Column, ascending: bool) -> Vec<MenuEntry> {
    let mut entries = vec![
        MenuEntry::item("None").checked(true),
        MenuEntry::separator(),
    ];
    for (column, title) in GROUPINGS {
        entries.push(MenuEntry::item(*title).checked(*column == Some(sort)));
    }
    entries.push(MenuEntry::separator());
    entries.push(MenuEntry::item("Ascending").checked(ascending));
    entries.push(MenuEntry::item("Descending").checked(!ascending));
    entries
}

/// The `…` (Action) menu. Rows, symbols and separator placement are Finder's shape; the wording is
/// read out of Finder's own File and View menus rather than invented.
///
/// **Its row list is not measured.** The metrics report never opened this menu, so the *order* and
/// the *sectioning* here are reconstructed rather than captured — unlike the toolbar geometry, which
/// is. Nothing in it does anything, which is the intent: this is a shape, not a feature.
fn action_menu() -> Vec<MenuEntry> {
    let swatches = MenuView::swatches(TAGS.iter().map(|(_, color)| system_color(*color)));
    vec![
        MenuEntry::item("New Folder").symbol("folder.badge.plus"),
        MenuEntry::item("New Folder with Selection")
            .symbol("folder.badge.plus")
            .disabled(),
        MenuEntry::item("Get Info").symbol("info.circle").disabled(),
        MenuEntry::separator(),
        MenuEntry::item("Rename").symbol("pencil"),
        MenuEntry::item("Compress").symbol("archivebox"),
        MenuEntry::item("Duplicate").symbol("plus.square.on.square"),
        MenuEntry::item("Make Alias").symbol("link"),
        MenuEntry::item("Quick Look").symbol("eye"),
        MenuEntry::separator(),
        MenuEntry::item("Copy").symbol("doc.on.doc"),
        MenuEntry::item("Share…").symbol("square.and.arrow.up"),
        MenuEntry::separator(),
        MenuEntry::item("Sort By")
            .symbol("arrow.up.arrow.down")
            .submenu(
                GROUPINGS
                    .iter()
                    .map(|(_, title)| MenuEntry::item(*title))
                    .chain([MenuEntry::separator(), MenuEntry::item("Tags")]),
            ),
        MenuEntry::item("Show View Options").symbol("slider.horizontal.3"),
        MenuEntry::separator(),
        MenuEntry::view(swatches),
        MenuEntry::item("Tags…").symbol("tag"),
        MenuEntry::separator(),
        MenuEntry::item("Import from iPhone or iPad")
            .symbol("iphone")
            .submenu([
                MenuEntry::item("Take Photo").symbol("camera"),
                MenuEntry::item("Scan Documents").symbol("doc.viewfinder"),
                MenuEntry::item("Add Sketch").symbol("pencil.tip.crop.circle"),
            ]),
        MenuEntry::item("Quick Actions").symbol("wand.and.stars"),
    ]
}

// ---------------------------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------------------------

struct FinderDemo {
    store: HashMap<&'static str, Folder>,
    sidebar: Option<NativeSidebar>,
    toolbar: bool,
    history: Vec<&'static str>,
    at: usize,
    view: usize,
    sort: Column,
    ascending: bool,
    query: String,
    expanded: HashSet<SharedString>,
    selected: HashSet<SharedString>,
    cursor: Option<SharedString>,
    anchor: Option<SharedString>,
    tags: HashMap<SharedString, usize>,
    preview: bool,
    /// Both default off: Finder ships them off and the reference window has them off.
    path_bar_shown: bool,
    status_bar_shown: bool,
    icon_size: f64,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    frames: u64,
    quit_at: Option<u64>,
}

impl FinderDemo {
    fn new(cx: &mut App) -> Self {
        Self {
            store: build_store(),
            sidebar: None,
            toolbar: false,
            history: vec!["documents"],
            at: 0,
            view: LIST_VIEW,
            sort: Column::Name,
            ascending: true,
            query: String::new(),
            expanded: HashSet::new(),
            selected: HashSet::new(),
            cursor: None,
            anchor: None,
            tags: HashMap::new(),
            // On, as Finder's own window is: the right pane is why the list is 594pt wide in a
            // 1026pt window, and the column geometry only lands where Finder puts it at that width.
            preview: true,
            path_bar_shown: false,
            status_bar_shown: false,
            icon_size: 64.,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            frames: 0,
            quit_at: env::var("NG_QUIT").ok().and_then(|v| v.parse().ok()),
        }
    }

    fn here(&self) -> &'static str {
        self.history[self.at]
    }

    fn folder(&self) -> Option<&Folder> {
        self.store.get(self.here())
    }

    fn sidebar_items(&self) -> Vec<SidebarItem> {
        SIDEBAR
            .iter()
            .map(|(id, title, rows)| {
                sidebar_group(*id, *title).children(rows.iter().map(
                    |(id, title, symbol, badge)| {
                        let item = sidebar_item(*id, *title).symbol(*symbol);
                        if *badge > 0 { item.badge(*badge) } else { item }
                    },
                ))
            })
            .collect()
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = NativeSidebar::install(
            window,
            cx,
            SidebarOptions {
                thickness: px(SIDEBAR_W),
                min_thickness: px(150.),
                max_thickness: px(320.),
                // The window's backdrop is what a transparent titlebar shows, so this is the
                // toolbar band's colour as much as the sidebar's. `WindowBackground` measured 67 in
                // dark against Finder's 29, which made the 52pt band the one part of the chrome
                // visibly not the table; `Card` is `ContentBackground`, the opaque one, and lands
                // on the table's own colour.
                backdrop: Some(Material::Card),
                ..SidebarOptions::default()
            },
        );
        // The listing is keyed by `&'static str`, so the row id has to come back out of the map's
        // own keys rather than being cloned out of the notification.
        sidebar.on_select(cx.listener(|this, id: &Option<SharedString>, window, cx| {
            let found = id
                .as_ref()
                .and_then(|id| this.store.get_key_value(id.as_ref()))
                .map(|(id, _)| *id);
            if let Some(id) = found {
                this.go(id, window, cx);
            }
        }));
        sidebar.update(&self.sidebar_items());
        sidebar.set_selected(Some(self.here()));
        self.sidebar = Some(sidebar);
        self.retitle(window);
        window.focus(&self.focus);
    }

    /// Finder's whole chrome, in one 52pt band: back/forward, the path title, AirDrop, the view
    /// picker, Group, Share + Tags + Action sharing a capsule, and the search field.
    ///
    /// Every one of them is a real `NSToolbarItem` subclass, which is what buys the glass capsules,
    /// the neutral grey selection pill and the reflow that shoves four groups 203pt left when the
    /// search field opens. None of that is drawn here; describing it in GPUI is what the two-row
    /// version of this demo did, and it is why it did not look like Finder.
    ///
    /// The 8pt gaps Finder shows between its trailing clusters are `ToolbarItem::space()`; the
    /// items *inside* a cluster are adjacent, and macOS 26 merges adjacent plain items into one
    /// capsule on its own — which is where Share, Tags and Action's shared 110pt capsule comes from.
    fn install_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.is_none() {
            return;
        }
        let this = cx.entity().downgrade();
        let nav = this.clone();
        let airdrop = this.clone();
        let view = this.clone();
        let tag = this.clone();
        let query = this.clone();

        set_toolbar(
            window,
            cx,
            Some(ToolbarStyle::Unified),
            &[
                ToolbarItem::group(
                    NAV_ID,
                    "Back/Forward",
                    [
                        // `chevron.backward`/`.forward` render identically — they are aliases.
                        ToolbarSegment::new("chevron.left", "Back"),
                        ToolbarSegment::new("chevron.right", "Forward"),
                    ],
                    0,
                )
                .selection(ToolbarSelection::Momentary)
                .group_width(ToolbarGroupWidth::Expanded)
                .on_select(move |index, window, cx| {
                    nav.update(cx, |this: &mut Self, cx| this.step(index == 1, window, cx))
                        .ok();
                }),
                // Where AppKit draws the window title. Finder's ink starts 8pt after the
                // back/forward item and does not move when the string changes, so it is
                // left-aligned in this region rather than centred in it.
                ToolbarItem::flexible_space(),
                // Wired, not decorative: without a handler `NSToolbarItem` validates itself
                // disabled and draws the glyph dimmed, which is not what Finder shows. Finder's
                // AirDrop shares the selection; this listing's AirDrop is a sidebar location, so
                // the item goes there — the same thing clicking that row does.
                ToolbarItem::symbol(AIRDROP_ID, "", "AirDrop").on_click(move |window, cx| {
                    airdrop
                        .update(cx, |this: &mut Self, cx| this.go("airdrop", window, cx))
                        .ok();
                }),
                ToolbarItem::group(
                    VIEW_ID,
                    "View",
                    VIEW_SYMBOLS
                        .iter()
                        .zip(VIEW_LABELS)
                        .map(|(symbol, label)| ToolbarSegment::new(*symbol, *label)),
                    self.view,
                )
                // Automatic, not Expanded: the capsule has to collapse into a labelled popup when
                // the search field opens, or the 99pt it should give up never appears and AppKit
                // sweeps Share, Tags and Action into an overflow menu instead of shifting them
                // 203pt left. That reflow is the whole reason these are real toolbar items.
                .group_width(ToolbarGroupWidth::Automatic)
                .on_select(move |index, _, cx| {
                    view.update(cx, |this: &mut Self, cx| {
                        this.view = index;
                        cx.notify();
                    })
                    .ok();
                }),
                ToolbarItem::space(),
                ToolbarItem::menu(GROUP_ID, "", "Group", group_menu(self.sort, self.ascending)),
                ToolbarItem::space(),
                ToolbarItem::share(SHARE_ID, "Share", self.share_items()),
                ToolbarItem::symbol(TAG_ID, "tag", "Edit Tags").on_click(move |_, cx| {
                    tag.update(cx, |this: &mut Self, cx| {
                        this.cycle_tag();
                        cx.notify();
                    })
                    .ok();
                }),
                ToolbarItem::menu(ACTION_ID, "ellipsis", "Action", action_menu()),
                ToolbarItem::space(),
                ToolbarItem::search(SEARCH_ID, "Search")
                    .placeholder("Search")
                    .width(px(SEARCH_W))
                    .on_change(move |text, _, cx| {
                        let text = text.to_string();
                        query
                            .update(cx, |this: &mut Self, cx| {
                                this.query = text;
                                cx.notify();
                            })
                            .ok();
                    }),
            ],
        );
        // Two of Finder's glyphs have no symbol name, and a menu row cannot be greyed while
        // automatic validation is on, so both are settled on the built items.
        finish_toolbar_items(window);
        if let Some(ns_window) = chrome_host(window).window() {
            // GPUI hides the title whenever the titlebar is transparent, and `window_options()`
            // always is. Finder puts the folder name in the toolbar, and with `Unified` that is what
            // a visible title *is*, so put it back.
            ns_window.setTitleVisibility(NSWindowTitleVisibility::Visible);
            // Undo `window_options()`'s transparent titlebar, keeping its full-size content view.
            // A transparent titlebar draws no background, so the 52pt band showed the window's
            // backdrop — 67 in dark against Finder's 29 — and, worse, **the capsules did not
            // render at all**: with an opaque backdrop behind a transparent titlebar the fill, the
            // stroke and the band all measured 29, so there was nothing to see. Opaque here, the
            // band is AppKit's own 29 and every capsule comes back. GPUI still draws under the
            // titlebar, which is fine because the tree is inset by `SidebarMetrics::top`.
            ns_window.setTitlebarAppearsTransparent(false);
        }
        self.toolbar = true;
    }

    /// What the share picker is offered. Text, because the listing is invented and there is no file
    /// behind a row to hand over.
    fn share_items(&self) -> Vec<String> {
        let names: Vec<String> = self
            .lines(false)
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .map(|line| line.row.name.to_string())
            .collect();
        if names.is_empty() {
            vec![self.folder().map_or("Finder", |folder| folder.title).into()]
        } else {
            names
        }
    }

    /// Finder's title is the full POSIX path, not the folder name, whenever
    /// `_FXShowPosixPathInTitle` is set — which is what the window this is matched against shows.
    /// The volume crumb is dropped, exactly as a real path drops it.
    fn retitle(&self, window: &mut Window) {
        let title = self.folder().map_or_else(
            || String::from("Finder"),
            |folder| match folder.crumbs.split_first() {
                Some((_volume, rest)) if !rest.is_empty() => format!("/{}", rest.join("/")),
                _ => folder.title.to_string(),
            },
        );
        window.set_window_title(&title);
    }

    // -- navigation ----------------------------------------------------------------------------

    fn go(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.here() == id {
            return;
        }
        self.history.truncate(self.at + 1);
        self.history.push(id);
        self.at = self.history.len() - 1;
        self.arrive(window, cx);
    }

    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let next = if forward {
            self.at + 1
        } else {
            self.at.wrapping_sub(1)
        };
        if next >= self.history.len() {
            return;
        }
        self.at = next;
        self.arrive(window, cx);
    }

    fn arrive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected.clear();
        self.cursor = None;
        self.anchor = None;
        self.query.clear();
        self.retitle(window);
        if let Some(sidebar) = &self.sidebar {
            let here = self.here();
            let known = SIDEBAR
                .iter()
                .any(|(_, _, rows)| rows.iter().any(|(id, _, _, _)| *id == here));
            sidebar.set_selected(known.then_some(here));
        }
        cx.notify();
    }

    fn open(&mut self, line: &Line, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = line.row.opens {
            self.go(id, window, cx);
        }
    }

    fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(parent) = self.folder().and_then(|folder| folder.parent) {
            self.go(parent, window, cx);
        }
    }

    // -- listing -------------------------------------------------------------------------------

    fn order(&self, a: &Row, b: &Row) -> Ordering {
        let by = match self.sort {
            Column::Name => Ordering::Equal,
            Column::Modified => a.stamp.cmp(&b.stamp),
            Column::Size => a.bytes.cmp(&b.bytes),
            Column::Kind => a.face.kind().cmp(b.face.kind()),
        };
        let by = by.then_with(|| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        });
        if self.ascending { by } else { by.reverse() }
    }

    /// `tree` follows open disclosure triangles; the other views are flat.
    fn lines(&self, tree: bool) -> Vec<Line> {
        let mut out = Vec::new();
        self.collect(self.here(), 0, tree, &mut out);
        out
    }

    fn collect(&self, id: &'static str, depth: usize, tree: bool, out: &mut Vec<Line>) {
        let Some(folder) = self.store.get(id) else {
            return;
        };
        let query = self.query.trim().to_lowercase();
        let mut rows: Vec<&Row> = folder
            .rows
            .iter()
            .filter(|row| query.is_empty() || row.name.to_lowercase().contains(&query))
            .collect();
        rows.sort_by(|a, b| self.order(a, b));

        for row in rows {
            let key = SharedString::from(format!("{id}/{}", row.name));
            let open = tree && self.expanded.contains(&key);
            let into = row.opens;
            out.push(Line {
                key,
                row: row.clone(),
                depth,
            });
            if open {
                if let Some(into) = into {
                    self.collect(into, depth + 1, tree, out);
                }
            }
        }
    }

    fn line_at(&self, lines: &[Line], key: &SharedString) -> Option<usize> {
        lines.iter().position(|line| &line.key == key)
    }

    /// Selection, AppKit's rules: plain replaces and re-anchors, ⌘ toggles, ⇧ extends from the
    /// anchor. The anchor is separate from the cursor because a run of ⇧↓ must grow one range from
    /// where the run started, not repeatedly select the last two rows.
    fn pick(&mut self, line: &Line, extend: bool, toggle: bool) {
        if toggle {
            if !self.selected.remove(&line.key) {
                self.selected.insert(line.key.clone());
            }
            self.anchor = Some(line.key.clone());
        } else if extend {
            let lines = self.lines(self.view == LIST_VIEW);
            let anchor = self
                .anchor
                .as_ref()
                .or(self.cursor.as_ref())
                .and_then(|key| self.line_at(&lines, key))
                .unwrap_or(0);
            if let Some(target) = self.line_at(&lines, &line.key) {
                let (lo, hi) = (anchor.min(target), anchor.max(target));
                self.selected = lines[lo..=hi].iter().map(|line| line.key.clone()).collect();
            }
        } else {
            self.selected.clear();
            self.selected.insert(line.key.clone());
            self.anchor = Some(line.key.clone());
        }
        self.cursor = Some(line.key.clone());
    }

    fn cycle_tag(&mut self) {
        for key in self.selected.clone() {
            match self.tags.get(&key).copied() {
                Some(index) if index + 1 >= TAGS.len() => {
                    self.tags.remove(&key);
                }
                Some(index) => {
                    self.tags.insert(key, index + 1);
                }
                None => {
                    self.tags.insert(key, 0);
                }
            }
        }
    }

    fn disclose(&mut self, key: &SharedString, open: bool) {
        if open {
            self.expanded.insert(key.clone());
        } else {
            self.expanded.remove(key);
        }
    }

    // -- keyboard ------------------------------------------------------------------------------

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        let tree = self.view == LIST_VIEW;
        let lines = self.lines(tree);
        if lines.is_empty() {
            return;
        }
        let current = self
            .cursor
            .as_ref()
            .and_then(|key| self.line_at(&lines, key));

        match event.keystroke.key.as_str() {
            "up" | "down" if modifiers.platform => {
                if event.keystroke.key == "up" {
                    self.up(window, cx);
                } else if let Some(line) = current.map(|index| lines[index].clone()) {
                    self.open(&line, window, cx);
                }
            }
            key @ ("up" | "down") => {
                let step: isize = if key == "up" { -1 } else { 1 };
                let next = match current {
                    Some(index) => (index as isize + step).clamp(0, lines.len() as isize - 1),
                    None if step > 0 => 0,
                    None => lines.len() as isize - 1,
                } as usize;
                let line = lines[next].clone();
                self.pick(&line, modifiers.shift, false);
                self.scroll.scroll_to_item(
                    if tree {
                        next
                    } else {
                        next / self.columns(window, cx)
                    },
                    ScrollStrategy::Center,
                );
                cx.notify();
            }
            "right" | "left" => {
                let open = event.keystroke.key == "right";
                if let Some(line) = current.map(|index| lines[index].clone()) {
                    if line.row.opens.is_some() {
                        self.disclose(&line.key, open);
                        cx.notify();
                    }
                }
            }
            "enter" => {
                if let Some(line) = current.map(|index| lines[index].clone()) {
                    self.open(&line, window, cx);
                }
            }
            _ => {}
        }
    }

    /// ⌘A and ⌘C arrive as actions rather than through [`Self::on_key`]. `install_menus` binds both
    /// chords so the Edit menu can carry their key equivalents, and a matched binding stops the
    /// key event before a bubble-phase listener sees it — so a plain `on_key_down` for ⌘A is dead
    /// code. Binding them again under [`LIST_CONTEXT`] wins only while this list has GPUI focus,
    /// which leaves the chords to the menu, and thus to the hosted search field, everywhere else.
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self
            .lines(self.view == LIST_VIEW)
            .iter()
            .map(|line| line.key.clone())
            .collect();
        cx.notify();
    }

    fn copy_names(&mut self, _: &CopyNames, _: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<String> = self
            .lines(self.view == LIST_VIEW)
            .iter()
            .filter(|line| self.selected.contains(&line.key))
            .map(|line| line.row.name.to_string())
            .collect();
        if !names.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(names.join("\n")));
        }
    }

    /// The sidebar's gutters, read off the handle rather than the `SidebarMetrics` global. The
    /// global is published from an AppKit layout pass through a spawned task, so it is unset for
    /// the first frames and each of those lays the tree out under the sidebar; the handle carries
    /// the same numbers synchronously.
    fn metrics(&self) -> SidebarMetrics {
        self.sidebar
            .as_ref()
            .map(NativeSidebar::metrics)
            .unwrap_or_default()
    }

    /// Tiles per band in icon view. The width available is the viewport less the gutters the
    /// sidebar publishes, so a collapse re-flows the grid.
    fn columns(&self, window: &Window, _cx: &App) -> usize {
        let metrics = self.metrics();
        let avail = f32::from(window.viewport_size().width)
            - f32::from(metrics.left)
            - f32::from(metrics.right)
            - if self.preview { PREVIEW_W } else { 0. }
            - 24.;
        let tile = self.icon_size as f32 + TILE_MARGIN + TILE_GAP;
        ((avail / tile).floor() as usize).max(1)
    }
}

// ---------------------------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------------------------

/// Every colour the list needs, resolved once per frame. `focused` picks between AppKit's two
/// selection colours, which is the rule a real table follows: the vivid one only while the view
/// that owns the selection has the keyboard.
struct Palette {
    label: Hsla,
    secondary: Hsla,
    tertiary: Hsla,
    surface: Hsla,
    stripe: Hsla,
    separator: Hsla,
    selection: Hsla,
    selection_text: Hsla,
    focused: bool,
}

impl Palette {
    fn new(focused: bool) -> Self {
        Self {
            label: system_color(SystemColor::Label),
            secondary: system_color(SystemColor::SecondaryLabel),
            tertiary: system_color(SystemColor::TertiaryLabel),
            surface: system_color(SystemColor::ControlBackground),
            stripe: alternating_row()
                .unwrap_or_else(|| system_color(SystemColor::ControlBackground)),
            separator: system_color(SystemColor::Separator),
            selection: if focused {
                system_color(SystemColor::SelectedContentBackground)
            } else {
                system_color(SystemColor::UnemphasizedSelectedContentBackground)
            },
            selection_text: if focused {
                system_color(SystemColor::AlternateSelectedControlText)
            } else {
                system_color(SystemColor::Label)
            },
            focused,
        }
    }
}

/// `alternatingContentBackgroundColors[1]`, the even row's fill, which this crate's palette does
/// not name.
///
/// Resolved inside an explicit drawing appearance rather than in whatever one happens to be
/// ambient: the dark value is white at 0.047 alpha over the table background while the light one is
/// an opaque (244,245,245), so an ambient read matches one appearance and silently contradicts the
/// other. Falls back to the plain surface, which shows up as banding simply gone rather than as
/// banding in the wrong colour.
fn alternating_row() -> Option<Hsla> {
    let slot: RefCell<Option<Hsla>> = RefCell::new(None);
    let block = StackBlock::new(|| {
        let colors = NSColor::alternatingContentBackgroundColors();
        *slot.borrow_mut() = colors.iter().nth(1).and_then(|color| tokens::srgb(&color));
    });
    tokens::current_appearance()
        .ns_appearance()
        .performAsCurrentDrawingAppearance(&block);
    slot.into_inner()
}

/// A list row's icon: the system's own artwork for the row's type, falling back to the face's SF
/// symbol for the props that have no file type behind them.
///
/// 32pt at a 2x backing scale lands on the 64px representation the artwork actually ships, so the
/// icon is crisp rather than resampled — asking for a size off that ladder is what makes one look
/// soft. The size is the same for every row on purpose: the cache is keyed by it, and varying it
/// per row would turn a hit into a 42µs raster.
fn artwork(row: &Row, size: f32) -> AnyElement {
    let symbol = row
        .artwork
        .and_then(|kind| file_icon(kind, px(size)))
        .map(|symbol| symbol.img().into_any_element());
    match symbol {
        Some(image) => div()
            .w(px(size))
            .h(px(size))
            .flex_none()
            .child(image)
            .into_any_element(),
        None => glyph(row.face, size),
    }
}

fn glyph(face: Face, size: f32) -> AnyElement {
    let symbol = sf_symbol_colored(face.symbol(), px(size), FontWeight::NORMAL, &face.tint());
    let mut slot = div()
        .w(px(size + 4.))
        .h(px(size))
        .flex()
        .items_center()
        .justify_center()
        .flex_none();
    if let Some(symbol) = symbol {
        slot = slot.child(symbol.img());
    }
    slot.into_any_element()
}

fn small_glyph(name: &str, size: f32, color: SystemColor) -> Option<AnyElement> {
    sf_symbol_colored(
        name,
        px(size),
        FontWeight::SEMIBOLD,
        &SymbolColor::Hierarchical(color),
    )
    .map(|symbol| symbol.img().into_any_element())
}

fn tag_dot(index: usize) -> AnyElement {
    div()
        .w(px(8.))
        .h(px(8.))
        .rounded_full()
        .flex_none()
        .bg(system_color(TAGS[index].1))
        .into_any_element()
}

impl FinderDemo {
    /// The column header. 28pt, one hairline along its own bottom edge and none under the toolbar,
    /// and a 1pt separator standing at each interior column boundary — never at the leading edge of
    /// Name or the trailing edge of Kind.
    ///
    /// The sorted column's label is both bolder and brighter than the rest: 11pt semibold in
    /// `labelColor` against 11pt regular in `secondaryLabelColor`.
    fn header_row(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .relative()
            .font_family(tokens::system_font_family())
            .text_size(tokens::small_system_font_size())
            .text_color(palette.secondary)
            // Finder needs no fill here: its toolbar band is already the table's colour, so the
            // header, the rows and the chrome above them read as one surface. This window's
            // backdrop is a material that measures 67 in dark against Finder's 29, so without
            // this the header would be the one part of the list that is visibly not the list.
            .bg(palette.surface);

        let last = COLUMNS.len() - 1;
        for (index, (column, title, width)) in COLUMNS.iter().enumerate() {
            let active = *column == self.sort;
            let column = *column;
            let mut cell = div()
                .id(SharedString::from(format!("header-{title}")))
                .h_full()
                .flex()
                .items_center()
                .relative()
                .overflow_hidden()
                .whitespace_nowrap()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if this.sort == column {
                        this.ascending = !this.ascending;
                    } else {
                        this.sort = column;
                        this.ascending = true;
                    }
                    cx.notify();
                }));
            cell = if *width > 0. {
                cell.w(px(*width))
            } else {
                cell.flex_1().min_w(px(120.))
            };
            // A leading spacer rather than padding: the separator and the sort indicator are
            // positioned from this cell's own edges, and padding would move what those edges mean.
            let inset = if index == 0 { HEADER_LABEL_X } else { CELL_PAD };
            cell = cell.child(div().w(px(inset)).flex_none());
            // Finder left-aligns the Size *header* while right-aligning its values, so this one
            // deliberately does not mirror the row. The label is not centred in the band either:
            // measured 1pt above centre, which a bottom margin buys without moving the cell's own
            // edges — the separator and the sort indicator are positioned from those.
            let label = div().mb(px(2.));
            cell = cell.child(if active {
                label
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.label)
                    .child(*title)
            } else {
                label.child(*title)
            });
            if active {
                let arrow = if self.ascending {
                    "chevron.up"
                } else {
                    "chevron.down"
                };
                if let Some(glyph) = small_glyph(arrow, 9., SystemColor::SecondaryLabel) {
                    cell = cell.child(
                        div()
                            .absolute()
                            .right(px(SORT_INSET))
                            .top_0()
                            .bottom_0()
                            .flex()
                            .items_center()
                            .child(glyph),
                    );
                }
            }
            if index < last {
                cell = cell.child(
                    div()
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .w(px(1.))
                        .h(px(HEADER_H - SEP_TOP))
                        .bg(palette.separator),
                );
            }
            row = row.child(cell);
        }
        row.child(div().w(px(PANE_INSET)).flex_none())
            // The hairline is drawn rather than bordered so the band keeps its full 28pt: a bottom
            // border would take the pt back out of the content box and shorten every separator.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(palette.separator),
            )
            .into_any_element()
    }

    /// One row is 36pt with a 32pt icon 26pt in from the pane's leading edge, the label 6pt after
    /// the icon, and the disclosure triangle's 15pt hit area in the gap before it. Every offset is
    /// measured from the pane rather than from the Name column, which begins 10pt further in.
    fn list_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lines = self.lines(true);
        let this = cx.entity().downgrade();
        let selected: HashSet<SharedString> = self.selected.clone();
        let tags = self.tags.clone();
        let expanded = self.expanded.clone();
        let label = palette.label;
        let secondary = palette.secondary;
        let stripe = palette.stripe;
        let selection = palette.selection;
        let focused = palette.focused;
        let selection_text = palette.selection_text;
        // Finder's date column is the same face, size and weight as the rest of the row with
        // tabular figures, so a column of times does not shift as its digits change. Resolved once
        // per frame: the feature list behind it allocates.
        let dates = tokens::system_font_tabular(FontWeight::NORMAL);

        let list = uniform_list("ng.finder.list", lines.len(), move |range, _, _| {
            range
                .map(|index| {
                    let line = lines[index].clone();
                    let is_selected = selected.contains(&line.key);
                    let text = if is_selected { selection_text } else { label };
                    let detail = if is_selected && focused {
                        selection_text
                    } else {
                        secondary
                    };
                    let this = this.clone();
                    let open = expanded.contains(&line.key);
                    let indent = line.depth as f32 * INDENT;

                    // `w_full` is load-bearing: a `uniform_list` item is laid out with its own
                    // width free, so without it the row shrinks to its contents and the flexible
                    // Name column collapses onto the Date column instead of pushing it right.
                    let mut row = div()
                        .id(("ng.finder.row", index))
                        .w_full()
                        .h(px(ROW_H))
                        .relative()
                        .flex()
                        .items_center()
                        .font_family(tokens::system_font_family())
                        .text_size(px(ROW_FONT))
                        .text_color(text);
                    // The banding and the selection are one shape: a pill inset from both pane
                    // edges, the full row height. Finder's alternating band is *not* a full-bleed
                    // rectangle — measured off a real window it stops 10pt short of each edge and
                    // carries the same rounded corners, so a selected row needs no band beneath it.
                    let fill = if is_selected {
                        Some(selection)
                    } else if index % 2 == 1 {
                        Some(stripe)
                    } else {
                        None
                    };
                    if let Some(fill) = fill {
                        row = row.child(
                            div()
                                .absolute()
                                .left(px(PANE_INSET))
                                .right(px(PANE_INSET))
                                .top_0()
                                .bottom_0()
                                .rounded(px(ROW_PILL_RADIUS))
                                .bg(fill),
                        );
                    }

                    let twist = {
                        let this = this.clone();
                        let key = line.key.clone();
                        let mut slot = div()
                            .id(("ng.finder.twist", index))
                            .w(px(TWIST_W))
                            .h_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center();
                        // Folders only. A bundle is a directory too, and Finder still gives it no
                        // triangle, so this follows what the row opens rather than what it is.
                        if line.row.opens.is_some() {
                            let arrow = if open {
                                "chevron.down"
                            } else {
                                "chevron.right"
                            };
                            let color = if is_selected && focused {
                                SystemColor::AlternateSelectedControlText
                            } else {
                                SystemColor::SecondaryLabel
                            };
                            if let Some(glyph) = small_glyph(arrow, 9., color) {
                                slot = slot.child(glyph);
                            }
                            slot = slot.on_click(move |_: &ClickEvent, _, cx| {
                                this.update(cx, |this: &mut FinderDemo, cx| {
                                    this.disclose(&key, !open);
                                    cx.notify();
                                })
                                .ok();
                            });
                        }
                        slot
                    };

                    // The gutter ends where the icon begins, so the triangle sitting at its
                    // trailing edge lands at ICON_X - TWIST_W without that number being written
                    // down twice.
                    let mut name = div()
                        .flex_1()
                        .min_w(px(120.))
                        .h_full()
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(
                            div()
                                .w(px(ICON_X + indent))
                                .h_full()
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_end()
                                .child(twist),
                        )
                        .child(artwork(&line.row, ICON_SIZE))
                        .child(
                            div()
                                .ml(px(LABEL_X - ICON_X - ICON_SIZE))
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(line.row.name.clone()),
                        );
                    if let Some(tag) = tags.get(&line.key) {
                        name = name.child(div().ml(px(6.)).child(tag_dot(*tag)));
                    }

                    let clicked = line.clone();
                    row.child(name)
                        .child(
                            div()
                                .w(px(COL_MODIFIED))
                                .flex_none()
                                .pl(px(CELL_PAD))
                                .font(dates.clone())
                                .text_color(detail)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(line.row.modified.clone()),
                        )
                        .child(
                            div()
                                .w(px(COL_SIZE))
                                .flex_none()
                                .pr(px(CELL_PAD))
                                .flex()
                                .justify_end()
                                .font(dates.clone())
                                .text_color(detail)
                                .child(line.row.size.clone()),
                        )
                        .child(
                            div()
                                .w(px(COL_KIND))
                                .flex_none()
                                .pl(px(CELL_PAD))
                                .text_color(detail)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(line.row.face.kind()),
                        )
                        .child(div().w(px(PANE_INSET)).flex_none())
                        .on_click(move |event: &ClickEvent, window, cx| {
                            let (extend, toggle, twice) = match event {
                                ClickEvent::Mouse(mouse) => (
                                    mouse.up.modifiers.shift,
                                    mouse.up.modifiers.platform,
                                    mouse.up.click_count >= 2,
                                ),
                                _ => (false, false, false),
                            };
                            let line = clicked.clone();
                            this.update(cx, |this: &mut FinderDemo, cx| {
                                this.pick(&line, extend, toggle);
                                if twice {
                                    this.open(&line, window, cx);
                                }
                                cx.notify();
                            })
                            .ok();
                        })
                })
                .collect::<Vec<_>>()
        })
        .flex_1()
        .track_scroll(self.scroll.clone());

        // Finder's table starts 5pt below the header hairline rather than flush against it, so the
        // first row's band does not touch the line.
        div()
            .flex_1()
            .flex()
            .flex_col()
            .pt(px(LIST_TOP_GAP))
            .child(list)
            .into_any_element()
    }

    fn icon_body(
        &mut self,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lines = self.lines(false);
        let columns = self.columns(window, cx);
        let rows = lines.len().div_ceil(columns).max(1);
        let this = cx.entity().downgrade();
        let selected = self.selected.clone();
        let tags = self.tags.clone();
        let size = self.icon_size as f32;
        let label = palette.label;
        let selection = palette.selection;
        let selection_text = palette.selection_text;

        uniform_list("ng.finder.icons", rows, move |range, _, _| {
            range
                .map(|band| {
                    let mut strip = div()
                        .h(px(size + 48.))
                        .flex()
                        .items_start()
                        .gap(px(TILE_GAP))
                        .px(px(12.))
                        .pt(px(8.));
                    for offset in 0..columns {
                        let index = band * columns + offset;
                        let Some(line) = lines.get(index) else { break };
                        let is_selected = selected.contains(&line.key);
                        let this = this.clone();
                        let clicked = line.clone();
                        let mut caption = div()
                            .px(px(5.))
                            .rounded(px(4.))
                            .max_w(px(size + TILE_MARGIN - 4.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(tokens::small_system_font_size())
                            .text_color(if is_selected { selection_text } else { label })
                            .child(line.row.name.clone());
                        if is_selected {
                            caption = caption.bg(selection);
                        }
                        let mut tile = div()
                            .id(("ng.finder.tile", index))
                            .w(px(size + TILE_MARGIN))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(4.))
                            .child(glyph(line.row.face, size))
                            .child(caption);
                        if let Some(tag) = tags.get(&line.key) {
                            tile = tile.child(tag_dot(*tag));
                        }
                        strip =
                            strip.child(tile.on_click(move |event: &ClickEvent, window, cx| {
                                let (extend, toggle, twice) = match event {
                                    ClickEvent::Mouse(mouse) => (
                                        mouse.up.modifiers.shift,
                                        mouse.up.modifiers.platform,
                                        mouse.up.click_count >= 2,
                                    ),
                                    _ => (false, false, false),
                                };
                                let line = clicked.clone();
                                this.update(cx, |this: &mut FinderDemo, cx| {
                                    this.pick(&line, extend, toggle);
                                    if twice {
                                        this.open(&line, window, cx);
                                    }
                                    cx.notify();
                                })
                                .ok();
                            }));
                    }
                    strip
                })
                .collect::<Vec<_>>()
        })
        .flex_1()
        .track_scroll(self.scroll.clone())
        .into_any_element()
    }

    /// One column of the browser view: a flat listing that reports clicks back through `pick`.
    fn browser_column(
        &self,
        id: &'static str,
        lines: Vec<Line>,
        palette: &Palette,
        width: Option<f32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity().downgrade();
        let selected = self.selected.clone();
        let label = palette.label;
        let selection = palette.selection;
        let selection_text = palette.selection_text;
        let mut column = div()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(palette.separator)
            .id(id)
            .overflow_y_scroll();
        column = match width {
            Some(width) => column.w(px(width)).flex_none(),
            None => column.flex_1(),
        };

        column
            .children(lines.into_iter().enumerate().map(|(index, line)| {
                let is_selected = selected.contains(&line.key);
                let this = this.clone();
                let clicked = line.clone();
                let mut row = div()
                    .id((id, index))
                    .h(px(BROWSER_ROW_H))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .px(px(8.))
                    .text_color(if is_selected { selection_text } else { label })
                    .child(glyph(line.row.face, 16.))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(line.row.name.clone()),
                    );
                if line.row.opens.is_some() {
                    if let Some(arrow) = small_glyph(
                        "chevron.right",
                        8.,
                        if is_selected {
                            SystemColor::AlternateSelectedControlText
                        } else {
                            SystemColor::TertiaryLabel
                        },
                    ) {
                        row = row.child(arrow);
                    }
                }
                if is_selected {
                    row = row.bg(selection);
                }
                row.on_click(move |event: &ClickEvent, window, cx| {
                    let twice =
                        matches!(event, ClickEvent::Mouse(mouse) if mouse.up.click_count >= 2);
                    let line = clicked.clone();
                    this.update(cx, |this: &mut FinderDemo, cx| {
                        this.pick(&line, false, false);
                        if twice {
                            this.open(&line, window, cx);
                        }
                        cx.notify();
                    })
                    .ok();
                })
            }))
            .into_any_element()
    }

    fn column_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lines = self.lines(false);
        let cursor = self
            .cursor
            .as_ref()
            .and_then(|key| lines.iter().find(|line| &line.key == key))
            .cloned();
        let child = cursor.as_ref().and_then(|line| line.row.opens);
        let child_lines: Vec<Line> = match child {
            Some(id) => {
                let mut out = Vec::new();
                self.collect(id, 0, false, &mut out);
                out
            }
            None => Vec::new(),
        };

        let mut body = div()
            .flex_1()
            .flex()
            .overflow_hidden()
            .child(self.browser_column("ng.finder.col1", lines, palette, Some(260.), cx));
        if child.is_some() {
            body = body.child(self.browser_column(
                "ng.finder.col2",
                child_lines,
                palette,
                Some(260.),
                cx,
            ));
        }
        // Finder's preview is the column after the last listing, not a panel floating in the
        // leftover width, and the remainder stays empty.
        body.child(
            div()
                .w(px(300.))
                .flex_none()
                .flex()
                .justify_center()
                .pt(px(28.))
                .px(px(20.))
                .border_r_1()
                .border_color(palette.separator)
                .child(self.info_card(cursor.as_ref(), palette, 96.)),
        )
        .child(div().flex_1())
        .into_any_element()
    }

    fn gallery_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lines = self.lines(false);
        let cursor = self
            .cursor
            .as_ref()
            .and_then(|key| lines.iter().find(|line| &line.key == key))
            .cloned()
            .or_else(|| lines.first().cloned());
        let this = cx.entity().downgrade();
        let selected = cursor.as_ref().map(|line| line.key.clone());
        let selection = palette.selection;

        div()
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(24.))
                    .child(self.info_card(cursor.as_ref(), palette, 144.)),
            )
            .child(
                div()
                    .id("ng.finder.filmstrip")
                    .h(px(96.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(14.))
                    .border_t_1()
                    .border_color(palette.separator)
                    .overflow_x_scroll()
                    .children(lines.into_iter().enumerate().map(|(index, line)| {
                        let is_selected = selected.as_ref() == Some(&line.key);
                        let this = this.clone();
                        let clicked = line.clone();
                        let mut cell = div()
                            .id(("ng.finder.frame", index))
                            .w(px(64.))
                            .h(px(64.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .child(glyph(line.row.face, 42.));
                        if is_selected {
                            cell = cell.bg(selection);
                        }
                        cell.on_click(move |_: &ClickEvent, _, cx| {
                            let line = clicked.clone();
                            this.update(cx, |this: &mut FinderDemo, cx| {
                                this.pick(&line, false, false);
                                cx.notify();
                            })
                            .ok();
                        })
                    })),
            )
            .into_any_element()
    }

    fn info_card(&self, line: Option<&Line>, palette: &Palette, size: f32) -> AnyElement {
        let mut card = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .text_color(palette.secondary);
        // Measured: with nothing selected Finder's preview pane is a uniform fill and carries no
        // label, so an empty div is the match rather than a "No selection" caption.
        let Some(line) = line else {
            return div().into_any_element();
        };

        card = card
            .child(glyph(line.row.face, size))
            .child(
                div()
                    .max_w(px(240.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.label)
                    .child(line.row.name.clone()),
            )
            .child(
                div()
                    .text_size(tokens::small_system_font_size())
                    .child(line.row.face.kind()),
            );

        for (caption, value) in [
            ("Modified", line.row.modified.clone()),
            ("Size", line.row.size.clone()),
        ] {
            card = card.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .text_size(tokens::small_system_font_size())
                    .child(div().text_color(palette.tertiary).child(caption))
                    .child(div().text_color(palette.secondary).child(value)),
            );
        }
        if let Some(tag) = self.tags.get(&line.key) {
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(tokens::small_system_font_size())
                    .child(tag_dot(*tag))
                    .child(div().text_color(palette.secondary).child(TAGS[*tag].0)),
            );
        }
        card.into_any_element()
    }

    fn path_bar(&self, palette: &Palette) -> AnyElement {
        let crumbs: &[&str] = self.folder().map_or(&[], |folder| folder.crumbs);
        let last = crumbs.len().saturating_sub(1);
        let mut bar = div()
            .h(px(BAR_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(14.))
            .border_t_1()
            .border_color(palette.separator)
            .text_size(tokens::small_system_font_size());

        for (index, crumb) in crumbs.iter().enumerate() {
            if index > 0 {
                if let Some(chevron) = small_glyph("chevron.right", 7., SystemColor::TertiaryLabel)
                {
                    bar = bar.child(chevron);
                }
            }
            let face = if index == 0 && crumbs.len() > 1 {
                F::Volume
            } else {
                F::Folder
            };
            bar = bar.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .text_color(if index == last {
                        palette.label
                    } else {
                        palette.secondary
                    })
                    .child(glyph(face, 13.))
                    .child(*crumb),
            );
        }
        bar.into_any_element()
    }

    fn status_bar(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let count = self.lines(false).len();
        let chosen = self.selected.len();
        let available = byte_string(AVAILABLE_BYTES as u64);
        let text = if chosen > 0 {
            format!("{chosen} of {count} selected, {available} available")
        } else {
            format!("{count} items, {available} available")
        };
        let slider = matches!(self.view, ICON_VIEW | GALLERY_VIEW);

        div()
            .h(px(BAR_H))
            .flex_none()
            .flex()
            .items_center()
            .px(px(12.))
            .border_t_1()
            .border_color(palette.separator)
            .text_size(tokens::small_system_font_size())
            .text_color(palette.secondary)
            .child(div().w(px(120.)).flex_none())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .whitespace_nowrap()
                    .child(SharedString::from(text)),
            )
            .child(
                div()
                    .w(px(120.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .when(slider, |row| {
                        row.child(
                            // Quantised in the handler rather than with `snap_to_ticks`: Finder's
                            // zoom slider draws no tick marks, and every distinct size rasterises
                            // its own set of symbols.
                            div().w(px(96.)).child(
                                native_slider("ng.finder.zoom", self.icon_size)
                                    .range(32., 128.)
                                    .on_change(cx.listener(|this, value: &f64, _, cx| {
                                        this.icon_size = (value / 8.).round() * 8.;
                                        cx.notify();
                                    })),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Render for FinderDemo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.sidebar.is_none() {
            self.install(window, cx);
        }
        if !self.toolbar {
            self.install_toolbar(window, cx);
        }
        self.frames += 1;
        if let Some(limit) = self.quit_at {
            window.request_animation_frame();
            if self.frames >= limit {
                std::process::exit(0);
            }
        }

        let metrics = self.metrics();
        let palette = Palette::new(self.focus.is_focused(window));
        let family = tokens::text_font_family(cx).unwrap_or_else(tokens::system_font_family);

        let path_bar = self.path_bar(&palette);
        let status_bar = self.status_bar(&palette, cx);
        let cursor = {
            let lines = self.lines(self.view == LIST_VIEW);
            self.cursor
                .as_ref()
                .and_then(|key| lines.iter().find(|line| &line.key == key))
                .cloned()
        };
        let body = match self.view {
            ICON_VIEW => self.icon_body(&palette, window, cx),
            COLUMN_VIEW => self.column_body(&palette, cx),
            GALLERY_VIEW => self.gallery_body(&palette, cx),
            _ => self.list_body(&palette, cx),
        };
        let headers = (self.view == LIST_VIEW).then(|| self.header_row(&palette, cx));
        let opaque = matches!(self.view, ICON_VIEW | LIST_VIEW | COLUMN_VIEW);

        // The gutters come from `SidebarMetrics` as padding, never as a background: a fill over
        // the sidebar strip would paint on top of its material and flatten it.
        native_root(
            div()
                .size_full()
                .font_family(family)
                .text_size(tokens::system_font_size())
                .flex()
                .flex_col()
                .pl(metrics.left)
                .pt(metrics.top)
                .pr(metrics.right)
                .pb(metrics.bottom)
                .child(
                    div()
                        .track_focus(&self.focus)
                        .key_context(LIST_CONTEXT)
                        .on_action(cx.listener(Self::select_all))
                        .on_action(cx.listener(Self::copy_names))
                        .on_key_down(cx.listener(Self::on_key))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, _| window.focus(&this.focus)),
                        )
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .overflow_hidden()
                                // No line under the toolbar when the column header is there: the
                                // header carries the only hairline, and the toolbar and header
                                // share one continuous background. The other views have no header,
                                // so the body needs the line drawn for them.
                                .when(headers.is_none(), |row| {
                                    row.border_t_1().border_color(palette.separator)
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .overflow_hidden()
                                        .children(headers)
                                        .child(
                                            div()
                                                .flex_1()
                                                .flex()
                                                .flex_col()
                                                .overflow_hidden()
                                                .when(opaque, |body| body.bg(palette.surface))
                                                .child(body),
                                        ),
                                )
                                .when(self.preview, |row| {
                                    row.child(
                                        div()
                                            .w(px(PREVIEW_W))
                                            .flex_none()
                                            .flex()
                                            .flex_col()
                                            .items_center()
                                            .justify_center()
                                            .p(px(18.))
                                            .border_l_1()
                                            .border_color(palette.separator)
                                            .child(self.info_card(cursor.as_ref(), &palette, 96.)),
                                    )
                                }),
                        )
                        .when(self.path_bar_shown, |c| c.child(path_bar))
                        .when(self.status_bar_shown, |c| c.child(status_bar)),
                ),
        )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        tokens::init(cx);
        install_menus(cx);
        // After `install_menus`, never before: it binds both chords with no context, and the more
        // specific binding has to be the later one.
        cx.bind_keys([
            KeyBinding::new("cmd-a", SelectAll, Some(LIST_CONTEXT)),
            KeyBinding::new("cmd-c", CopyNames, Some(LIST_CONTEXT)),
        ]);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(160.), px(120.)),
                    size: size(px(1026.), px(772.)),
                })),
                ..window_options()
            },
            |_, cx| {
                cx.new(|cx| {
                    cx.observe_global::<SystemTheme>(|_, cx| cx.notify())
                        .detach();
                    FinderDemo::new(cx)
                })
            },
        )
        .expect("the Finder window opens");
        cx.activate(true);
    });
}
