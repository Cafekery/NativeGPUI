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

pub(crate) fn reference_seconds(at: SystemTime) -> f64 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => since.as_secs_f64() - UNIX_TO_REFERENCE,
        Err(before) => -before.duration().as_secs_f64() - UNIX_TO_REFERENCE,
    }
}

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

pub(crate) fn today() -> f64 {
    midnights()[3]
}

struct Seen<K> {
    day: f64,
    texts: HashMap<K, SharedString>,
}

fn remembered<K: Hash + Eq>(
    seen: &'static LocalKey<RefCell<Seen<K>>>,
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

fn minute(seconds: f64) -> i64 {
    (seconds / 60.).floor() as i64
}

fn formatted(formatter: &NSDateFormatter, seconds: f64) -> String {
    formatter
        .stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(seconds))
        .to_string()
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
    remembered(&SEEN, minute(seconds), || {
        FORMATTER.with(|formatter| formatted(formatter, seconds).into())
    })
}

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
    remembered(&SEEN, (minute(seconds), (room * 2.) as i32), || {
        FORMATTERS.with(|all| {
            let mut texts = all
                .iter()
                .map(|formatter| formatted(formatter, seconds).into());
            let mut text: SharedString = texts.next().unwrap_or_default();
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

pub(crate) fn list_date(seconds: f64, style: usize, relative: bool) -> SharedString {
    thread_local! {
        static FORMATTERS: [Vec<Retained<NSDateFormatter>>; 2] = [false, true].map(|relative| {
            use NSDateFormatterStyle as Style;
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
        });
        static SEEN: RefCell<Seen<(i64, usize, bool)>> = seen();
    }
    let days = midnights();
    // Finder's date-only style shows today as its time.
    let today = style == 3 && relative && (days[3]..days[4]).contains(&seconds);
    let style = if today { 4 } else { style };
    remembered(&SEEN, (minute(seconds), style, relative), || {
        FORMATTERS.with(|all| formatted(&all[usize::from(relative)][style], seconds).into())
    })
}

// Finder's rule for every row: the fullest style whose samples leave 16 pt, else the date alone.
pub(crate) fn date_style(width: f32, size: f32, relative: bool) -> usize {
    let widest = date_samples(size, relative);
    (0..3)
        .find(|&style| widest[style] + 16. <= width)
        .unwrap_or(3)
}

pub(crate) fn date_fit(size: f32, relative: bool) -> f32 {
    (date_samples(size, relative)[0] + 20.).round()
}

fn date_samples(size: f32, relative: bool) -> [f32; 3] {
    thread_local! {
        static WIDEST: RefCell<HashMap<(u32, bool), (f64, [f32; 3])>> = RefCell::new(HashMap::new());
    }
    let day = today();
    let key = (size.to_bits(), relative);
    if let Some(widest) = WIDEST.with_borrow(|all| {
        all.get(&key)
            .filter(|(on, _)| *on == day)
            .map(|(_, widest)| *widest)
    }) {
        return widest;
    }
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
    let widest = [0, 1, 2].map(|style| {
        samples
            .iter()
            .map(|&sample| appkit_width(&list_date(sample, style, relative), size, style == 2))
            .fold(0., f32::max)
    });
    WIDEST.with_borrow_mut(|all| all.insert(key, (day, widest)));
    widest
}

struct KindNames {
    names: Vec<Vec<String>>,
    by_name: HashMap<String, (usize, usize)>,
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
        .flat_map(|(kind, all)| {
            (all.iter().enumerate()).map(move |(at, name)| (name.to_lowercase(), (kind, at)))
        })
        .collect();
    KindNames { names, by_name }
});

pub(crate) fn kind_text(kind: &SharedString, room: f32, size: f32) -> SharedString {
    type Names = Rc<[(SharedString, f32)]>;
    thread_local! {
        static MEASURED: RefCell<HashMap<SharedString, (u32, Names)>> = RefCell::new(HashMap::new());
    }
    let names = MEASURED.with_borrow_mut(|measured| {
        if let Some((_, names)) = measured.get(kind).filter(|(at, _)| *at == size.to_bits()) {
            return names.clone();
        }
        // Finder only shortens: "MPEG-4 movie" never widens to "Apple MPEG-4 movie".
        let names: Names = match KIND_NAMES.by_name.get(&kind.to_lowercase()) {
            Some(&(kind, at)) => KIND_NAMES.names[kind][at..]
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

const SIZE_EDGES: [u64; 5] = [1, 1_000_000, 10_000_000, 100_000_000, 1_000_000_000];

fn size_band(bytes: u64) -> usize {
    SIZE_EDGES.partition_point(|&edge| edge <= bytes)
}

pub(crate) struct Bands {
    group: Column,
    days: [f64; 6],
    months: Vec<(f64, f64, SharedString)>,
}

impl Bands {
    pub(crate) fn new(group: Column) -> Self {
        Self {
            group,
            days: match group.dated() {
                true => midnights(),
                false => [0.; 6],
            },
            months: Vec::new(),
        }
    }

    pub(crate) fn initial(name: &str) -> SharedString {
        match name.chars().next() {
            Some(first) if first.is_ascii_digit() => SharedString::new_static("0 \u{2013} 9"),
            Some(first) => first.to_uppercase().collect::<String>().into(),
            None => SharedString::new_static("\u{2014}"),
        }
    }

    /// Where a size band sorts: bigger items' above smaller ones', folders' below all.
    pub(crate) fn size_place(folder: bool, bytes: u64) -> f64 {
        match folder {
            true => -1.,
            false => size_band(bytes) as f64,
        }
    }

    /// Where the band of a date sorts: a later band above an earlier one.
    pub(crate) fn place(&mut self, at: f64) -> f64 {
        let days = self.days;
        match self.day(at) {
            Some(day) => days[day],
            None if at >= days[4] => self.month(at).0.max(days[4]),
            None => self.month(at).0,
        }
    }

    /// Which of Previous 30 Days, Previous 7 Days, Yesterday and Today holds `at`.
    fn day(&self, at: f64) -> Option<usize> {
        (0..4).find(|&day| (self.days[day]..self.days[day + 1]).contains(&at))
    }

    pub(crate) fn next(&mut self, row: &Row, band: Option<&SharedString>) -> Option<SharedString> {
        let named = |name: &str| band.is_some_and(|band| band.as_ref() == name);
        let fixed = |name: &'static str| (!named(name)).then(|| SharedString::new_static(name));
        let fresh = |name: String| (!named(&name)).then(|| SharedString::from(name));
        let finder = |key: &str, english: &str, args: &[&str]| {
            fresh(words::fill(&words::finder(key, english), args))
        };
        match self.group {
            Column::Name => {
                let initial = Self::initial(&row.name);
                (band != Some(&initial)).then_some(initial)
            }
            Column::Kind => {
                let kind = row.kind.as_ref();
                let plural = kind.ends_with('s');
                let same = band.is_some_and(|band| match plural {
                    true => band.as_ref() == kind,
                    false => band.strip_suffix('s') == Some(kind),
                });
                (!same).then(|| match plural {
                    true => row.kind.clone(),
                    false => format!("{kind}s").into(),
                })
            }
            Column::Size if row.face == Face::Folder => finder("GROUP_DIRECTORIES", "Folders", &[]),
            Column::Size => match size_band(row.bytes) {
                0 => fresh(band_bytes(0)),
                1 => finder("GV10", "Under ^0", &[&band_bytes(1_000_000)]),
                2 => fixed("1 MB to 10 MB"),
                3 => fixed("10 MB to 100 MB"),
                4 => fixed("100 MB to 1 GB"),
                _ => fixed("Over 1 GB"),
            },
            group if group.dated() => {
                let Some(at) = row.date(group) else {
                    return finder("GROUP_FUTURE", "No Date", &[]);
                };
                match self.day(at) {
                    Some(3) => finder("GROUP_TODAY", "Today", &[]),
                    Some(2) => finder("GROUP_YESTERDAY", "Yesterday", &[]),
                    Some(1) => finder("GROUP_PREVIOUS7DAYS", "Previous ^0 Days", &[&count(7)]),
                    Some(_) => finder("GROUP_PREVIOUS30DAYS", "Previous ^0 Days", &[&count(30)]),
                    None => {
                        let (_, month) = self.month(at);
                        (band != Some(&month)).then_some(month)
                    }
                }
            }
            group => fresh(match row.words(group).filter(|words| !words.is_empty()) {
                Some(words) => words.to_string(),
                None if group == Column::Tags => words::finder("GV12", "No Tags"),
                None => "--".to_string(),
            }),
        }
    }

    /// The start and name of the month holding `at`.
    fn month(&mut self, at: f64) -> (f64, SharedString) {
        thread_local! {
            static FORMATTER: Retained<NSDateFormatter> = {
                let formatter = NSDateFormatter::new();
                formatter.setLocalizedDateFormatFromTemplate(&NSString::from_str("MMMMyyyy"));
                formatter
            };
        }
        if let Some((from, _, name)) = self
            .months
            .iter()
            .find(|(from, to, _)| (*from..*to).contains(&at))
        {
            return (*from, name.clone());
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
        let name: SharedString = FORMATTER.with(|formatter| formatted(formatter, at).into());
        let Some(start) = start.filter(|_| found) else {
            return (at, name);
        };
        let from = start.timeIntervalSinceReferenceDate();
        self.months.push((from, from + length, name.clone()));
        (from, name)
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
    FORMATTER.with(|formatter| formatted(formatter, seconds))
}

// Default units: Finder's size bands read zero as "Zero KB".
fn band_bytes(bytes: u64) -> String {
    NSByteCountFormatter::stringFromByteCount_countStyle(
        bytes as i64,
        NSByteCountFormatterCountStyle::File,
    )
    .to_string()
}

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
