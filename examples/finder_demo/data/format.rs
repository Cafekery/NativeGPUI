use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;
use std::sync::{LazyLock, Mutex};
use std::thread::LocalKey;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{FontWeight, SharedString};
use native_gpui::tokens;
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAttributedStringNSStringDrawing, NSFont, NSFontAttributeName, NSFontWeightRegular,
};
use objc2_foundation::{
    NSAttributedString, NSBundle, NSByteCountFormatter, NSByteCountFormatterCountStyle,
    NSByteCountFormatterUnits, NSCalendar, NSCalendarOptions, NSCalendarUnit, NSDate,
    NSDateFormatter, NSDateFormatterStyle, NSDictionary, NSNumber, NSNumberFormatter,
    NSNumberFormatterStyle, NSString,
};

use finder_fs::words;

use super::face::Face;
use super::row::{Column, Row};

const UNIX_TO_REFERENCE: f64 = 978_307_200.;
const SEEN_MAX: usize = 4096;
const BYTES_HELD: usize = 16_384;

pub(super) fn reference_seconds(at: SystemTime) -> f64 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => since.as_secs_f64() - UNIX_TO_REFERENCE,
        Err(before) => -before.duration().as_secs_f64() - UNIX_TO_REFERENCE,
    }
}

/// The midnights starting the days 30, 7 and 1 before today, today and the two after it.
fn midnights() -> [f64; 6] {
    thread_local! {
        static DAYS: Cell<[f64; 6]> = const { Cell::new([0.; 6]) };
    }
    let now = reference_seconds(SystemTime::now());
    let days = DAYS.get();
    if (days[3]..days[4]).contains(&now) {
        return days;
    }
    let calendar = NSCalendar::currentCalendar();
    let today = calendar.startOfDayForDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(now));
    let days = [-30, -7, -1, 0, 1, 2].map(|offset| {
        calendar
            .dateByAddingUnit_value_toDate_options(
                NSCalendarUnit::Day,
                offset,
                &today,
                NSCalendarOptions::empty(),
            )
            .map_or(f64::NAN, |day| day.timeIntervalSinceReferenceDate())
    });
    DAYS.set(days);
    days
}

/// Today's midnight: the day relative dates are written for.
pub(crate) fn today() -> f64 {
    midnights()[3]
}

/// Texts for one day at most: relative dates name another day after midnight.
struct Seen<K> {
    day: f64,
    texts: HashMap<K, SharedString>,
}

type Cache<K> = LocalKey<RefCell<Seen<K>>>;

fn remembered<K: Hash + Eq>(
    seen: &'static Cache<K>,
    key: K,
    write: impl FnOnce() -> SharedString,
) -> SharedString {
    let day = today();
    seen.with_borrow_mut(|seen| {
        if seen.day != day || seen.texts.len() >= SEEN_MAX {
            seen.day = day;
            seen.texts.clear();
        }
        if let Some(text) = seen.texts.get(&key) {
            return text.clone();
        }
        let text = write();
        seen.texts.insert(key, text.clone());
        text
    })
}

fn seen<K>() -> RefCell<Seen<K>> {
    RefCell::new(Seen {
        day: f64::NAN,
        texts: HashMap::new(),
    })
}

fn formatted(formatter: &NSDateFormatter, seconds: f64) -> SharedString {
    SharedString::from(
        formatter
            .stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(seconds))
            .to_string(),
    )
}

pub(crate) fn date_text(seconds: f64) -> SharedString {
    thread_local! {
        static FORMATTER: Retained<NSDateFormatter> = {
            let formatter = NSDateFormatter::new();
            formatter.setDateStyle(NSDateFormatterStyle::ShortStyle);
            formatter.setTimeStyle(NSDateFormatterStyle::ShortStyle);
            formatter.setDoesRelativeDateFormatting(true);
            formatter
        };
        static SEEN: RefCell<Seen<i64>> = seen();
    }
    remembered(&SEEN, (seconds / 60.).floor() as i64, || {
        FORMATTER.with(|formatter| formatted(formatter, seconds))
    })
}

/// The preview card's date: relative within a day, otherwise the longest of the full, long and
/// medium styles whose text is at most `room` points wide in the small system font.
pub(crate) fn stamp(seconds: f64, room: f32) -> SharedString {
    thread_local! {
        static FORMATTERS: [Retained<NSDateFormatter>; 3] = [
            NSDateFormatterStyle::FullStyle,
            NSDateFormatterStyle::LongStyle,
            NSDateFormatterStyle::MediumStyle,
        ]
        .map(|style| {
            let formatter = NSDateFormatter::new();
            formatter.setDateStyle(style);
            formatter.setTimeStyle(NSDateFormatterStyle::ShortStyle);
            formatter
        });
        static SEEN: RefCell<Seen<(i64, i32)>> = seen();
    }
    let days = midnights();
    if (days[2]..days[5]).contains(&seconds) {
        return date_text(seconds);
    }
    let key = ((seconds / 60.).floor() as i64, (room * 2.) as i32);
    remembered(&SEEN, key, || {
        FORMATTERS.with(|all| {
            let mut texts = all.iter().map(|formatter| formatted(formatter, seconds));
            let mut text = texts.next().unwrap_or_default();
            while small_text_width(&text) > room {
                let Some(shorter) = texts.next() else { break };
                text = shorter;
            }
            text
        })
    })
}

pub(crate) fn small_text_width(text: &str) -> f32 {
    tokens::appkit_string_width(
        &tokens::system_font_family(),
        tokens::small_system_font_size(),
        FontWeight::NORMAL,
        text,
    )
    .unwrap_or(f32::MAX)
}

fn appkit_width(text: &str, size: f32, tabular: bool) -> f32 {
    let font = match tabular {
        true => NSFont::monospacedDigitSystemFontOfSize_weight(size.into(), unsafe {
            NSFontWeightRegular
        }),
        false => NSFont::systemFontOfSize(size.into()),
    };
    let font: &AnyObject = font.as_ref();
    let attrs = NSDictionary::from_slices(&[unsafe { NSFontAttributeName }], &[font]);
    let text = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&attrs),
        )
    };
    text.size().width as f32
}

/// Finder's list styles, fullest first: long, medium and short date with time, then the date
/// alone, which for today is its time.
pub(crate) fn list_date(seconds: f64, style: usize) -> SharedString {
    thread_local! {
        static FORMATTERS: Vec<Retained<NSDateFormatter>> = {
            use NSDateFormatterStyle as Style;
            let relative = crate::prefs::finder().relative_dates;
            [
                (Style::LongStyle, Style::ShortStyle),
                (Style::MediumStyle, Style::ShortStyle),
                (Style::ShortStyle, Style::ShortStyle),
                (Style::ShortStyle, Style::NoStyle),
                (Style::NoStyle, Style::ShortStyle),
            ]
            .into_iter()
            .map(|(date, time)| {
                let formatter = NSDateFormatter::new();
                formatter.setDateStyle(date);
                formatter.setTimeStyle(time);
                formatter.setDoesRelativeDateFormatting(relative && date != Style::NoStyle);
                formatter
            })
            .collect()
        };
        static SEEN: RefCell<Seen<(i64, usize)>> = seen();
    }
    let days = midnights();
    let today = style == 3
        && crate::prefs::finder().relative_dates
        && (days[3]..days[4]).contains(&seconds);
    let style = if today { 4 } else { style };
    remembered(&SEEN, ((seconds / 60.).floor() as i64, style), || {
        FORMATTERS.with(|all| formatted(&all[style], seconds))
    })
}

/// Finder's date column takes one style for every row, whatever they hold: the fullest whose
/// samples, yesterday at 10 PM and six days ago at 10 AM, leave 16 points of the column, the short
/// styles set in monospaced digits; else the date alone.
pub(crate) fn date_style(width: f32, size: f32) -> usize {
    let widest = date_samples(size);
    (0..3)
        .find(|&style| widest[style] + 16. <= width)
        .unwrap_or(3)
}

/// Finder fits its date column to the long style's samples and 20 points, whatever the rows hold.
pub(crate) fn date_fit(size: f32) -> f32 {
    (date_samples(size)[0] + 20.).round()
}

fn date_samples(size: f32) -> [f32; 3] {
    thread_local! {
        static WIDEST: Cell<(u32, f64, [f32; 3])> = const { Cell::new((0, f64::NAN, [0.; 3])) };
    }
    let day = today();
    let (measured, on, mut widest) = WIDEST.get();
    if measured != size.to_bits() || on != day {
        let at = |days: f64, hour: isize| {
            let day = NSDate::dateWithTimeIntervalSinceNow(-days * 86_400.);
            NSCalendar::currentCalendar()
                .dateBySettingHour_minute_second_ofDate_options(
                    hour,
                    0,
                    0,
                    &day,
                    NSCalendarOptions::empty(),
                )
                .map_or(0., |date| date.timeIntervalSinceReferenceDate())
        };
        let samples = [at(1., 22), at(6., 10)];
        widest = [0, 1, 2].map(|style| {
            samples
                .iter()
                .map(|&sample| appkit_width(&list_date(sample, style), size, style == 2))
                .fold(0., f32::max)
        });
        WIDEST.set((size.to_bits(), day, widest));
    }
    widest
}

/// Finder's own names for a kind, fullest first (`KIND_FORMATTER_<kind>_<n>` in its strings),
/// keyed by any of them in lower case.
struct KindNames {
    names: Vec<Vec<String>>,
    by_name: HashMap<String, usize>,
}

static KIND_NAMES: LazyLock<KindNames> = LazyLock::new(|| {
    let finder = NSString::from_str("/System/Library/CoreServices/Finder.app");
    let table = NSString::from_str("LocalizableMerged");
    let names: Vec<Vec<String>> = NSBundle::bundleWithPath(&finder).map_or(Vec::new(), |finder| {
        let name = |key: String| {
            let key = NSString::from_str(&key);
            let found = finder.localizedStringForKey_value_table(&key, None, Some(&table));
            (*found != *key).then(|| found.to_string())
        };
        (0..)
            .map(|kind| {
                (0..)
                    .map_while(|n| name(format!("KIND_FORMATTER_{kind}_{n}")))
                    .collect::<Vec<_>>()
            })
            .take_while(|names| !names.is_empty())
            .collect()
    });
    let by_name = (names.iter().enumerate())
        .flat_map(|(kind, all)| all.iter().map(move |name| (name.to_lowercase(), kind)))
        .collect();
    KindNames { names, by_name }
});

/// Finder's kind column: a kind Finder knows by several names shows the fullest of them that
/// fits `room`, else its shortest; any other kind shows as it is.
pub(crate) fn kind_text(kind: &SharedString, room: f32, size: f32) -> SharedString {
    type Names = Rc<[(SharedString, f32)]>;
    thread_local! {
        static MEASURED: RefCell<HashMap<SharedString, (u32, Names)>> = RefCell::new(HashMap::new());
    }
    let names = MEASURED.with_borrow_mut(|measured| {
        if let Some((_, names)) = measured.get(kind).filter(|(at, _)| *at == size.to_bits()) {
            return names.clone();
        }
        let names: Names = match KIND_NAMES.by_name.get(&kind.to_lowercase()) {
            Some(&at) => KIND_NAMES.names[at]
                .iter()
                .map(|name| (name.clone().into(), appkit_width(name, size, false)))
                .collect(),
            None => Rc::new([]),
        };
        if measured.len() >= SEEN_MAX {
            measured.clear();
        }
        measured.insert(kind.clone(), (size.to_bits(), names.clone()));
        names
    });
    match names.last() {
        Some(shortest) => names
            .iter()
            .find(|(_, width)| *width <= room)
            .unwrap_or(shortest)
            .0
            .clone(),
        None => kind.clone(),
    }
}

/// The bands of a grouped listing, named once per band rather than once per row.
pub(crate) struct Bands {
    sort: Column,
    days: [f64; 6],
    months: Vec<(f64, f64, SharedString)>,
}

impl Bands {
    pub(crate) fn new(sort: Column) -> Self {
        Self {
            sort,
            days: match sort {
                Column::Modified => midnights(),
                _ => [0.; 6],
            },
            months: Vec::new(),
        }
    }

    /// The band `row` falls in, when it is not `band`.
    pub(crate) fn next(&mut self, row: &Row, band: Option<&SharedString>) -> Option<SharedString> {
        let named = |name: &str| band.is_some_and(|band| band.as_ref() == name);
        let fixed = |name: &'static str| (!named(name)).then(|| SharedString::new_static(name));
        let finder = |key: &str, english: &str, args: &[&str]| {
            let name = words::fill(&words::finder(key, english), args);
            (!named(&name)).then(|| SharedString::from(name))
        };
        match self.sort {
            Column::Name => match row.name.chars().next() {
                Some(first) if first.is_ascii_digit() => fixed("0 \u{2013} 9"),
                Some(first) => band
                    .is_none_or(|band| !band.chars().eq(first.to_uppercase()))
                    .then(|| SharedString::from(first.to_uppercase().to_string())),
                None => fixed("\u{2014}"),
            },
            Column::Kind => {
                let kind = row.kind.as_ref();
                let same = band.is_some_and(|band| match kind.ends_with('s') {
                    true => band.as_ref() == kind,
                    false => band.strip_suffix('s') == Some(kind),
                });
                (!same).then(|| match kind.ends_with('s') {
                    true => row.kind.clone(),
                    false => SharedString::from(format!("{kind}s")),
                })
            }
            Column::Size if row.face == Face::Folder => finder("GROUP_DIRECTORIES", "Folders", &[]),
            Column::Size if row.bytes == 0 => {
                let zero = band_bytes(0);
                (!named(&zero)).then(|| zero.into())
            }
            Column::Size if row.bytes < 1_000_000 => {
                finder("GV10", "Under ^0", &[&band_bytes(1_000_000)])
            }
            Column::Size => fixed(match row.bytes {
                bytes if bytes < 10_000_000 => "1 MB to 10 MB",
                bytes if bytes < 100_000_000 => "10 MB to 100 MB",
                bytes if bytes < 1_000_000_000 => "100 MB to 1 GB",
                _ => "Over 1 GB",
            }),
            Column::Modified => {
                let (at, days) = (row.modified_at, self.days);
                match () {
                    _ if (days[3]..days[4]).contains(&at) => finder("GROUP_TODAY", "Today", &[]),
                    _ if (days[2]..days[3]).contains(&at) => {
                        finder("GROUP_YESTERDAY", "Yesterday", &[])
                    }
                    _ if (days[1]..days[2]).contains(&at) => {
                        finder("GROUP_PREVIOUS7DAYS", "Previous ^0 Days", &[&count(7)])
                    }
                    _ if (days[0]..days[1]).contains(&at) => {
                        finder("GROUP_PREVIOUS30DAYS", "Previous ^0 Days", &[&count(30)])
                    }
                    _ => {
                        let month = self.month(at);
                        (band != Some(&month)).then_some(month)
                    }
                }
            }
        }
    }

    fn month(&mut self, at: f64) -> SharedString {
        thread_local! {
            static FORMATTER: Retained<NSDateFormatter> = {
                let formatter = NSDateFormatter::new();
                formatter.setLocalizedDateFormatFromTemplate(&NSString::from_str("MMMMyyyy"));
                formatter
            };
        }
        if let Some((_, _, name)) = self
            .months
            .iter()
            .find(|(from, to, _)| (*from..*to).contains(&at))
        {
            return name.clone();
        }
        let date = NSDate::dateWithTimeIntervalSinceReferenceDate(at);
        let mut start = None;
        let mut length = 0.;
        let found = unsafe {
            NSCalendar::currentCalendar().rangeOfUnit_startDate_interval_forDate(
                NSCalendarUnit::Month,
                Some(&mut start),
                &mut length,
                &date,
            )
        };
        let name = FORMATTER.with(|formatter| formatted(formatter, at));
        if let Some(start) = start.filter(|_| found) {
            let from = start.timeIntervalSinceReferenceDate();
            self.months.push((from, from + length, name.clone()));
        }
        name
    }
}

pub(crate) fn day_string(seconds: f64) -> String {
    thread_local! {
        static FORMATTER: Retained<NSDateFormatter> = {
            let formatter = NSDateFormatter::new();
            formatter.setDateStyle(NSDateFormatterStyle::MediumStyle);
            formatter.setTimeStyle(NSDateFormatterStyle::NoStyle);
            formatter
        };
    }
    FORMATTER.with(|formatter| {
        formatter
            .stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(seconds))
            .to_string()
    })
}

/// A size band's bound as Finder words it: the formatter's default units, so zero reads "Zero KB".
fn band_bytes(bytes: u64) -> String {
    NSByteCountFormatter::stringFromByteCount_countStyle(
        bytes as i64,
        NSByteCountFormatterCountStyle::File,
    )
    .to_string()
}

/// A count in the user's digits and grouping.
pub(crate) fn count(n: usize) -> String {
    NSNumberFormatter::localizedStringFromNumber_numberStyle(
        &NSNumber::new_usize(n),
        NSNumberFormatterStyle::DecimalStyle,
    )
    .to_string()
}

pub(crate) fn byte_string(bytes: u64) -> String {
    static SEEN: LazyLock<Mutex<HashMap<u64, String>>> = LazyLock::new(Mutex::default);
    if let Some(text) = SEEN.lock().unwrap().get(&bytes) {
        return text.clone();
    }
    thread_local! {
        static FORMATTER: Retained<NSByteCountFormatter> = {
            let formatter = NSByteCountFormatter::new();
            formatter.setCountStyle(NSByteCountFormatterCountStyle::File);
            // The default units read an empty file as "Zero KB"; Finder says "Zero bytes".
            formatter.setAllowedUnits(
                NSByteCountFormatterUnits::UseBytes
                    | NSByteCountFormatterUnits::UseKB
                    | NSByteCountFormatterUnits::UseMB
                    | NSByteCountFormatterUnits::UseGB
                    | NSByteCountFormatterUnits::UseTB
                    | NSByteCountFormatterUnits::UsePB,
            );
            formatter
        };
    }
    let text = FORMATTER.with(|formatter| formatter.stringFromByteCount(bytes as i64).to_string());
    let mut seen = SEEN.lock().unwrap();
    if seen.len() >= BYTES_HELD {
        seen.clear();
    }
    seen.insert(bytes, text.clone());
    text
}
