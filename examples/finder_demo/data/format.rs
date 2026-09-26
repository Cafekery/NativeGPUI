use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::LazyLock;
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
    NSByteCountFormatterUnits, NSCalendar, NSCalendarOptions, NSDate, NSDateFormatter,
    NSDateFormatterStyle, NSDictionary, NSString,
};

use super::face::Face;
use super::row::{Column, Row};

const UNIX_TO_REFERENCE: f64 = 978_307_200.;

pub(super) fn reference_seconds(at: SystemTime) -> f64 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => since.as_secs_f64() - UNIX_TO_REFERENCE,
        Err(before) => -before.duration().as_secs_f64() - UNIX_TO_REFERENCE,
    }
}

type Cache = LocalKey<RefCell<HashMap<i64, SharedString>>>;

fn per_minute(
    seconds: f64,
    seen: &'static Cache,
    write: impl FnOnce(f64) -> SharedString,
) -> SharedString {
    let minute = (seconds / 60.).floor() as i64;
    seen.with(|seen| {
        if let Some(text) = seen.borrow().get(&minute) {
            return text.clone();
        }
        let text = write(seconds);
        seen.borrow_mut().insert(minute, text.clone());
        text
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
        static SEEN: RefCell<HashMap<i64, SharedString>> = RefCell::new(HashMap::new());
    }
    per_minute(seconds, &SEEN, |seconds| {
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
        static SEEN: RefCell<HashMap<(i64, i32), SharedString>> = RefCell::new(HashMap::new());
    }
    if days_ago(seconds).abs() <= 1 {
        return date_text(seconds);
    }
    let key = ((seconds / 60.).floor() as i64, (room * 2.) as i32);
    if let Some(text) = SEEN.with_borrow(|seen| seen.get(&key).cloned()) {
        return text;
    }
    let text = FORMATTERS.with(|all| {
        let mut texts = all.iter().map(|formatter| formatted(formatter, seconds));
        let mut text = texts.next().unwrap_or_default();
        while small_text_width(&text) > room {
            let Some(shorter) = texts.next() else { break };
            text = shorter;
        }
        text
    });
    SEEN.with_borrow_mut(|seen| seen.insert(key, text.clone()));
    text
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
        static SEEN: RefCell<HashMap<(i64, usize), SharedString>> = RefCell::new(HashMap::new());
    }
    let today = style == 3 && crate::prefs::finder().relative_dates && days_ago(seconds) == 0;
    let style = if today { 4 } else { style };
    let key = ((seconds / 60.).floor() as i64, style);
    if let Some(text) = SEEN.with_borrow(|seen| seen.get(&key).cloned()) {
        return text;
    }
    let text = FORMATTERS.with(|all| formatted(&all[style], seconds));
    SEEN.with_borrow_mut(|seen| seen.insert(key, text.clone()));
    text
}

/// Finder's date column takes one style for every row, whatever they hold: the fullest whose
/// samples, yesterday at 10 PM and six days ago at 10 AM, leave 16 points of the column, the short
/// styles set in monospaced digits; else the date alone.
pub(crate) fn date_style(width: f32, size: f32) -> usize {
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
    (0..3)
        .find(|&style| {
            samples.iter().all(|&sample| {
                appkit_width(&list_date(sample, style), size, style == 2) + 16. <= width
            })
        })
        .unwrap_or(3)
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
    thread_local! {
        static SEEN: RefCell<HashMap<(SharedString, i32, i32), SharedString>> =
            RefCell::new(HashMap::new());
    }
    let key = (kind.clone(), room as i32, (size * 2.) as i32);
    if let Some(text) = SEEN.with_borrow(|seen| seen.get(&key).cloned()) {
        return text;
    }
    let text = match KIND_NAMES.by_name.get(&kind.to_lowercase()) {
        Some(&at) => {
            let names = &KIND_NAMES.names[at];
            let fits = |name: &&String| appkit_width(name, size, false) <= room;
            SharedString::from(
                names
                    .iter()
                    .find(fits)
                    .unwrap_or(&names[names.len() - 1])
                    .clone(),
            )
        }
        None => kind.clone(),
    };
    SEEN.with_borrow_mut(|seen| seen.insert(key, text.clone()));
    text
}

pub(crate) fn group_of(row: &Row, sort: Column) -> SharedString {
    match sort {
        Column::Name => match row.name.chars().next() {
            Some(first) if first.is_ascii_digit() => "0 \u{2013} 9".into(),
            Some(first) => SharedString::from(first.to_uppercase().to_string()),
            None => "\u{2014}".into(),
        },
        Column::Kind => plural(&row.kind),
        Column::Size if row.face == Face::Folder => "Folders".into(),
        Column::Size => match row.bytes {
            0 => "Zero KB".into(),
            bytes if bytes < 1_000_000 => "Under 1 MB".into(),
            bytes if bytes < 10_000_000 => "1 MB to 10 MB".into(),
            bytes if bytes < 100_000_000 => "10 MB to 100 MB".into(),
            bytes if bytes < 1_000_000_000 => "100 MB to 1 GB".into(),
            _ => "Over 1 GB".into(),
        },
        Column::Modified => match days_ago(row.modified_at) {
            0 => "Today".into(),
            1 => "Yesterday".into(),
            2..=7 => "Previous 7 Days".into(),
            8..=30 => "Previous 30 Days".into(),
            _ => month_string(row.modified_at),
        },
    }
}

fn plural(kind: &str) -> SharedString {
    match kind.ends_with('s') {
        true => SharedString::from(kind.to_string()),
        false => SharedString::from(format!("{kind}s")),
    }
}

fn days_ago(seconds: f64) -> i64 {
    let now = NSDate::date().timeIntervalSinceReferenceDate();
    let calendar = NSCalendar::currentCalendar();
    let then = NSDate::dateWithTimeIntervalSinceReferenceDate(seconds);
    let start_of = |date: &NSDate| {
        calendar
            .startOfDayForDate(date)
            .timeIntervalSinceReferenceDate()
    };
    let gap = start_of(&NSDate::dateWithTimeIntervalSinceReferenceDate(now)) - start_of(&then);
    (gap / 86_400.0).round() as i64
}

fn month_string(seconds: f64) -> SharedString {
    let formatter = NSDateFormatter::new();
    formatter.setDateFormat(Some(&NSString::from_str("LLLL yyyy")));
    SharedString::from(
        formatter
            .stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(seconds))
            .to_string(),
    )
}

pub(crate) fn day_string(seconds: f64) -> String {
    let formatter = NSDateFormatter::new();
    formatter.setDateStyle(NSDateFormatterStyle::MediumStyle);
    formatter.setTimeStyle(NSDateFormatterStyle::NoStyle);
    formatter
        .stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(seconds))
        .to_string()
}

pub(crate) fn byte_string(bytes: u64) -> String {
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
    FORMATTER.with(|formatter| formatter.stringFromByteCount(bytes as i64).to_string())
}
