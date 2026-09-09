use std::cell::RefCell;
use std::collections::HashMap;
use std::thread::LocalKey;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::SharedString;
use objc2::rc::Retained;
use objc2_foundation::{
    NSByteCountFormatter, NSByteCountFormatterCountStyle, NSCalendar, NSDate, NSDateFormatter,
    NSDateFormatterStyle, NSString,
};

use super::face::Face;
use super::row::{Column, Row};

const UNIX_TO_REFERENCE: f64 = 978_307_200.;

/// Seconds as `NSDate` counts them, from 2001; signed both ways, so a pre-1970 file keeps its date.
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

pub(crate) fn stamp(seconds: f64) -> SharedString {
    thread_local! {
        static FORMATTER: Retained<NSDateFormatter> = {
            let formatter = NSDateFormatter::new();
            formatter.setDateStyle(NSDateFormatterStyle::FullStyle);
            formatter.setTimeStyle(NSDateFormatterStyle::ShortStyle);
            formatter
        };
        static SEEN: RefCell<HashMap<i64, SharedString>> = RefCell::new(HashMap::new());
    }
    per_minute(seconds, &SEEN, |seconds| {
        match days_ago(seconds).abs() <= 1 {
            true => date_text(seconds),
            false => FORMATTER.with(|formatter| formatted(formatter, seconds)),
        }
    })
}

/// The band Finder files a row under while Use Groups is on.
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
            formatter
        };
    }
    FORMATTER.with(|formatter| formatter.stringFromByteCount(bytes as i64).to_string())
}
