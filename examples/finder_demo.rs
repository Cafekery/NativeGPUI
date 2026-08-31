//! A Finder front end. Real macOS chrome, entirely fictional files.
//!
//! Nothing here reads the filesystem: the tree in `DIRS` is invented and the sidebar's Locations
//! are props. Dates and sizes are still formatted by `NSDateFormatter` and `NSByteCountFormatter`,
//! so the columns read in the user's own locale and unit conventions rather than in a format
//! written down here — a list that says "14 Aug 2026 at 09:41" on one Mac and "Aug 14, 2026 at
//! 9:41 AM" on another is the difference between a recreation and a mock-up.
//!
//! What is native: the sidebar (`NSSplitViewItem.sidebar`, `NSOutlineView` rows), the unified
//! toolbar and its items, the back/forward and view-mode segmented controls, the search field and
//! the icon-size slider. What GPUI draws: the file list, its headers, the path bar, the status bar
//! and the info pane — all in system fonts and semantic colours so the two halves agree.
//!
//! `NG_QUIT=<frames>` exits after N frames, so a smoke run ends on its own.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::env;

use gpui::{
    AnyElement, App, Application, Bounds, ClickEvent, ClipboardItem, FocusHandle, FontWeight, Hsla,
    KeyBinding, KeyDownEvent, MouseButton, ScrollStrategy, SharedString, UniformListScrollHandle,
    Window, WindowBounds, WindowOptions, actions, div, point, prelude::*, px, size, uniform_list,
};
use native_gpui::sidebar::{
    NativeSidebar, SidebarItem, SidebarMetrics, SidebarOptions, sidebar_group, sidebar_item,
};
use native_gpui::tokens::{
    self, SymbolColor, SystemColor, SystemTheme, sf_symbol_colored, system_color,
};
use native_gpui::{
    MenuEntry, ToolbarItem, ToolbarStyle, TrackingMode, chrome_host, install_menus, native_popup,
    native_root, native_search_field, native_segmented, native_slider, set_toolbar, window_options,
};
use objc2_app_kit::NSWindowTitleVisibility;
use objc2_foundation::{
    NSByteCountFormatter, NSByteCountFormatterCountStyle, NSDate, NSDateFormatter,
    NSDateFormatterStyle, NSTimeZone,
};

use self::Face as F;

// ---------------------------------------------------------------------------------------------
// The invented filesystem
// ---------------------------------------------------------------------------------------------

/// What a row is, which decides its glyph, its Kind column and its icon tint.
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
            F::Text => "Plain Text Document",
            F::Code => "Source Text",
            F::Sheet => "Numbers Spreadsheet",
            F::Pages => "Pages Document",
            F::Slides => "Keynote Presentation",
            F::Image => "PNG Image",
            F::Movie => "QuickTime Movie",
            F::Audio => "Audio File",
            F::Archive => "ZIP Archive",
            F::Program => "Application",
            F::Web => "Web Internet Location",
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
            F::Program => SystemColor::SystemBlue,
            F::Web => SystemColor::SystemCyan,
        };
        SymbolColor::Hierarchical(color)
    }
}

/// `name`, face, modified as `yyyymmddhhmm`, size in bytes, the directory this row opens (`""`
/// when it opens nothing). Folder sizes are ignored: Finder shows `--`.
type Src = (&'static str, Face, u64, u64, &'static str);

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
            ("Projects", F::Folder, 202608301037, 0, "projects"),
            ("Tax 2025", F::Folder, 202601190904, 0, "tax"),
            ("Annual Report.pdf", F::Pdf, 202608141731, 8_412_770, ""),
            ("Budget 2026.numbers", F::Sheet, 202608070912, 1_204_881, ""),
            ("Cabin Lease.pages", F::Pages, 202605221459, 462_112, ""),
            ("Kickoff Deck.key", F::Slides, 202607041122, 24_880_401, ""),
            ("Meeting Notes.txt", F::Text, 202608291642, 11_402, ""),
            ("Onboarding.pdf", F::Pdf, 202604111005, 2_118_004, ""),
            ("Passport Scan.png", F::Image, 202602280811, 3_940_112, ""),
            ("Reading List.txt", F::Text, 202608120744, 4_118, ""),
            ("Recipes.pages", F::Pages, 202603160931, 812_004, ""),
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
        "Favourites",
        &[
            ("airdrop", "AirDrop", "dot.radiowaves.right", 0),
            ("recents", "Recents", "clock", 0),
            ("applications", "Applications", "square.grid.2x2", 0),
            ("desktop", "Desktop", "menubar.dock.rectangle", 0),
            ("documents", "Documents", "doc", 0),
            ("downloads", "Downloads", "arrow.down.circle", 3),
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
            ("shared", "Shared", "person.2", 2),
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

/// Days since 1970-01-01 from a proleptic Gregorian date. Needed because the invented dates are
/// written as readable `yyyymmddhhmm` literals and `NSDate` wants seconds.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn epoch_seconds(stamp: u64) -> f64 {
    let stamp = stamp as i64;
    let (year, month, day) = (
        stamp / 100_000_000,
        stamp / 1_000_000 % 100,
        stamp / 10_000 % 100,
    );
    let (hour, minute) = (stamp / 100 % 100, stamp % 100);
    (days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60) as f64
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

/// Width and trailing inset per fixed column. Named rather than repeated, because the header and
/// the rows are laid out in different functions and a literal in only one of them is exactly how
/// the two stop lining up.
const COL_MODIFIED: (f32, f32) = (172., 10.);
const COL_SIZE: (f32, f32) = (84., 16.);
const COL_KIND: (f32, f32) = (152., 10.);

const COLUMNS: &[(Column, &str, (f32, f32))] = &[
    (Column::Name, "Name", (0., 10.)),
    (Column::Modified, "Date Modified", COL_MODIFIED),
    (Column::Size, "Size", COL_SIZE),
    (Column::Kind, "Kind", COL_KIND),
];

const VIEWS: &[&str] = &["Icon", "List", "Column", "Gallery"];
const ICON_VIEW: usize = 0;
const LIST_VIEW: usize = 1;
const COLUMN_VIEW: usize = 2;
const GALLERY_VIEW: usize = 3;

/// Keymap context for the file list. Scopes ⌘A and ⌘C to it, so they stay the menu's — and the
/// hosted search field's — whenever focus is anywhere else.
const LIST_CONTEXT: &str = "FinderList";

actions!(finder_demo, [SelectAll, CopyNames]);

const TOOLBAR_H: f32 = 40.;
const HEADER_H: f32 = 23.;
/// Finder's own list row, measured off a real window: 21pt with a 16pt icon.
const ROW_H: f32 = 21.;
const BAR_H: f32 = 26.;
const PREVIEW_W: f32 = 248.;
const INDENT: f32 = 15.;

/// Icon-view tile: how much wider than the icon the cell is, and the gap between cells. The pitch
/// is the sum, and leaving the gap out of it is how the last column ends up clipped.
const TILE_MARGIN: f32 = 56.;
const TILE_GAP: f32 = 8.;

/// Finder's tag colours, in its own order.
const TAGS: &[(&str, SystemColor)] = &[
    ("Red", SystemColor::SystemRed),
    ("Orange", SystemColor::SystemOrange),
    ("Yellow", SystemColor::SystemYellow),
    ("Green", SystemColor::SystemGreen),
    ("Blue", SystemColor::SystemBlue),
    ("Purple", SystemColor::SystemPurple),
];

fn build_store() -> HashMap<&'static str, Folder> {
    // Medium date + short time is the pair Finder uses, and the locale's own joiner supplies the
    // "at" — the whole reason to ask Foundation instead of writing a format string.
    let formatter = NSDateFormatter::new();
    formatter.setDateStyle(NSDateFormatterStyle::MediumStyle);
    formatter.setTimeStyle(NSDateFormatterStyle::ShortStyle);
    // GMT, so an invented stamp renders as the literal that was typed. Local time would shift
    // every column by the host's offset and put the Screenshots folder's dates an hour off the
    // times in its own filenames.
    formatter.setTimeZone(Some(&NSTimeZone::timeZoneForSecondsFromGMT(0)));

    let row = |src: &Src| {
        let (name, face, stamp, bytes, opens) = *src;
        let date = NSDate::dateWithTimeIntervalSince1970(epoch_seconds(stamp));
        Row {
            name: name.into(),
            modified: formatter.stringFromDate(&date).to_string().into(),
            stamp,
            size: if face == F::Folder || face == F::Volume || face == F::Device {
                "--".into()
            } else {
                byte_string(bytes).into()
            },
            bytes,
            face,
            opens: (!opens.is_empty()).then_some(opens),
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
            preview: false,
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

    /// The current folder then each ancestor, nearest first.
    fn ancestry(&self) -> Vec<(&'static str, &'static str)> {
        let mut chain = Vec::new();
        let mut id = self.here();
        while let Some(folder) = self.store.get(id) {
            chain.push((id, folder.title));
            match folder.parent {
                Some(parent) => id = parent,
                None => break,
            }
        }
        chain
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
                thickness: px(190.),
                min_thickness: px(150.),
                max_thickness: px(320.),
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

    /// The unified toolbar carries the items that are actions rather than state: the sidebar
    /// toggle, the sort order, tags and the info pane. Back/forward, the view picker and the
    /// search field are hosted controls in the row below, because `ToolbarItem` has no view case
    /// and an `NSSearchField` cannot be an item without one.
    fn install_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sidebar) = self.sidebar.clone() else {
            return;
        };
        let this = cx.entity().downgrade();
        let sort = this.clone();
        let tag = this.clone();
        let info = this.clone();

        set_toolbar(
            window,
            cx,
            Some(ToolbarStyle::Unified),
            &[
                ToolbarItem::symbol("ng.finder.sidebar", "sidebar.left", "Sidebar")
                    .on_click(move |_, _| sidebar.toggle()),
                ToolbarItem::flexible_space(),
                ToolbarItem::symbol("ng.finder.sort", "arrow.up.arrow.down", "Sort").on_click(
                    move |_, cx| {
                        sort.update(cx, |this: &mut Self, cx| {
                            let next = COLUMNS
                                .iter()
                                .position(|(column, _, _)| *column == this.sort)
                                .map_or(0, |index| (index + 1) % COLUMNS.len());
                            this.sort = COLUMNS[next].0;
                            cx.notify();
                        })
                        .ok();
                    },
                ),
                ToolbarItem::symbol("ng.finder.tag", "tag", "Tags").on_click(move |_, cx| {
                    tag.update(cx, |this: &mut Self, cx| {
                        this.cycle_tag();
                        cx.notify();
                    })
                    .ok();
                }),
                ToolbarItem::symbol("ng.finder.info", "sidebar.right", "Info").on_click(
                    move |_, cx| {
                        info.update(cx, |this: &mut Self, cx| {
                            this.preview = !this.preview;
                            cx.notify();
                        })
                        .ok();
                    },
                ),
            ],
        );
        // GPUI hides the title whenever the titlebar is transparent, and `window_options()` always
        // is. Finder puts the folder name in the toolbar, and with `Unified` that is what a visible
        // title *is*, so put it back.
        if let Some(ns_window) = chrome_host(window).window() {
            ns_window.setTitleVisibility(NSWindowTitleVisibility::Visible);
        }
        self.toolbar = true;
    }

    fn retitle(&self, window: &mut Window) {
        let title = self.folder().map_or("Finder", |folder| folder.title);
        window.set_window_title(title);
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

    /// Tiles per band in icon view. The width available is the viewport less the gutters the
    /// sidebar publishes, so a collapse re-flows the grid.
    fn columns(&self, window: &Window, cx: &App) -> usize {
        let metrics = cx
            .try_global::<SidebarMetrics>()
            .copied()
            .unwrap_or_default();
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
            stripe: system_color(SystemColor::QuinarySystemFill),
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
    fn toolbar_row(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let back = self.at > 0;
        let forward = self.at + 1 < self.history.len();

        div()
            .h(px(TOOLBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .child(
                native_segmented("ng.finder.nav", usize::MAX)
                    .segments(["\u{2039}", "\u{203A}"])
                    .tracking_mode(TrackingMode::Momentary)
                    .enabled(back || forward)
                    .on_change(cx.listener(|this, index: &usize, window, cx| {
                        this.step(*index == 1, window, cx);
                    })),
            )
            .child(
                native_segmented("ng.finder.view", self.view)
                    .segments(VIEWS.iter().copied())
                    .on_change(cx.listener(|this, index: &usize, _, cx| {
                        this.view = *index;
                        cx.notify();
                    })),
            )
            .child(
                // Finder's title menu: the leaf is the button, the ancestors are its items. A
                // pull-down takes item 0 as its own label, which is exactly that shape.
                native_popup("ng.finder.path", 0)
                    .pull_down(true)
                    .entries(
                        self.ancestry()
                            .into_iter()
                            .map(|(_, title)| MenuEntry::item(title).symbol("folder")),
                    )
                    .on_change(cx.listener(|this, index: &usize, window, cx| {
                        if let Some((id, _)) = this.ancestry().get(*index).copied() {
                            this.go(id, window, cx);
                        }
                    })),
            )
            .child(div().flex_1())
            .child(
                div().w(px(196.)).flex_none().child(
                    native_search_field("ng.finder.search", &self.query)
                        .placeholder("Search")
                        .on_change(cx.listener(|this, text: &String, _, cx| {
                            this.query = text.clone();
                            cx.notify();
                        })),
                ),
            )
            .into_any_element()
    }

    fn header_row(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .px(px(12.))
            .text_size(tokens::small_system_font_size())
            .text_color(palette.secondary)
            .border_b_1()
            .border_color(palette.separator);

        for (column, title, (width, pad)) in COLUMNS {
            let active = *column == self.sort;
            let column = *column;
            let mut cell = div()
                .id(SharedString::from(format!("header-{title}")))
                .flex()
                .items_center()
                .gap(px(3.))
                .pr(px(*pad))
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
            // Finder left-aligns the Size *header* while right-aligning its values, so this one
            // deliberately does not mirror the row.
            cell = cell.child(*title);
            if active {
                let arrow = if self.ascending {
                    "chevron.up"
                } else {
                    "chevron.down"
                };
                if let Some(glyph) = small_glyph(arrow, 7., SystemColor::SecondaryLabel) {
                    cell = cell.child(glyph);
                }
            }
            row = row.child(cell);
        }
        row.into_any_element()
    }

    fn list_body(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let lines = self.lines(true);
        let this = cx.entity().downgrade();
        let selected: HashSet<SharedString> = self.selected.clone();
        let tags = self.tags.clone();
        let expanded = self.expanded.clone();
        let label = palette.label;
        let secondary = palette.secondary;
        let tertiary = palette.tertiary;
        let stripe = palette.stripe;
        let selection = palette.selection;
        let focused = palette.focused;
        let selection_text = palette.selection_text;

        uniform_list("ng.finder.list", lines.len(), move |range, _, _| {
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

                    // `w_full` is load-bearing: a `uniform_list` item is laid out with its own
                    // width free, so without it the row shrinks to its contents and the flexible
                    // Name column collapses onto the Date column instead of pushing it right.
                    let mut row = div()
                        .id(("ng.finder.row", index))
                        .w_full()
                        .h(px(ROW_H))
                        .flex()
                        .items_center()
                        .px(px(12.))
                        .text_color(text);
                    if is_selected {
                        row = row.bg(selection);
                    } else if index % 2 == 1 {
                        row = row.bg(stripe);
                    }

                    let mut name = div()
                        .flex_1()
                        .min_w(px(120.))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .pl(px(line.depth as f32 * INDENT));

                    let twist = {
                        let this = this.clone();
                        let key = line.key.clone();
                        let mut slot = div()
                            .id(("ng.finder.twist", index))
                            .w(px(13.))
                            .h(px(13.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center();
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
                            if let Some(glyph) = small_glyph(arrow, 8., color) {
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

                    name = name.child(twist).child(glyph(line.row.face, 16.)).child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(line.row.name.clone()),
                    );
                    if let Some(tag) = tags.get(&line.key) {
                        name = name.child(tag_dot(*tag));
                    }

                    let clicked = line.clone();
                    row.child(name)
                        .child(
                            div()
                                .w(px(COL_MODIFIED.0))
                                .flex_none()
                                .pr(px(COL_MODIFIED.1))
                                .text_size(tokens::small_system_font_size())
                                .text_color(detail)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(line.row.modified.clone()),
                        )
                        .child(
                            div()
                                .w(px(COL_SIZE.0))
                                .flex_none()
                                .pr(px(COL_SIZE.1))
                                .flex()
                                .justify_end()
                                .text_size(tokens::small_system_font_size())
                                .text_color(detail)
                                .child(line.row.size.clone()),
                        )
                        .child(
                            div()
                                .w(px(COL_KIND.0))
                                .flex_none()
                                .pr(px(COL_KIND.1))
                                .text_size(tokens::small_system_font_size())
                                .text_color(if is_selected && focused {
                                    selection_text
                                } else {
                                    tertiary
                                })
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(line.row.face.kind()),
                        )
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
        .track_scroll(self.scroll.clone())
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
                    .h(px(ROW_H))
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
        let Some(line) = line else {
            return card
                .child(
                    div()
                        .text_size(tokens::small_system_font_size())
                        .child("No selection"),
                )
                .into_any_element();
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

        let metrics = cx
            .try_global::<SidebarMetrics>()
            .copied()
            .unwrap_or_default();
        let palette = Palette::new(self.focus.is_focused(window));
        let family = tokens::text_font_family(cx).unwrap_or_else(tokens::system_font_family);

        let toolbar = self.toolbar_row(cx);
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
                        .child(toolbar)
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .overflow_hidden()
                                .border_t_1()
                                .border_color(palette.separator)
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
                        .child(path_bar)
                        .child(status_bar),
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
                    size: size(px(1180.), px(760.)),
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
