//! What the demo carries between launches: the window's frame is AppKit's own autosave, the rest
//! rides in user defaults.

use objc2_foundation::{NSString, NSUserDefaults};

use crate::data::Column;

const VIEW: &str = "ng.finder.view";
const PANE: &str = "ng.finder.pane";
const ICON: &str = "ng.finder.icon";
const LIST_ICON: &str = "ng.finder.list.icon";
const STRIP_ICON: &str = "ng.finder.strip.icon";

fn defaults() -> objc2::rc::Retained<NSUserDefaults> {
    NSUserDefaults::standardUserDefaults()
}

fn read(key: &str) -> Option<f64> {
    let defaults = defaults();
    let key = NSString::from_str(key);
    defaults.objectForKey(&key)?;
    Some(defaults.doubleForKey(&key))
}

fn write(key: &str, value: f64) {
    defaults().setDouble_forKey(value, &NSString::from_str(key));
}

pub(crate) fn view() -> Option<usize> {
    read(VIEW).map(|v| v as usize)
}

pub(crate) fn set_view(view: usize) {
    write(VIEW, view as f64);
}

pub(crate) fn pane_width() -> Option<f32> {
    read(PANE).map(|v| v as f32)
}

pub(crate) fn set_pane_width(width: f32) {
    write(PANE, width as f64);
}

pub(crate) fn icon_size() -> Option<f32> {
    read(ICON).map(|v| v as f32)
}

pub(crate) fn set_icon_size(size: f32) {
    write(ICON, size as f64);
}

pub(crate) fn list_icon() -> Option<f32> {
    read(LIST_ICON).map(|v| v as f32)
}

pub(crate) fn set_list_icon(size: f32) {
    write(LIST_ICON, size as f64);
}

pub(crate) fn strip_icon() -> Option<f32> {
    read(STRIP_ICON).map(|v| v as f32)
}

pub(crate) fn set_strip_icon(size: f32) {
    write(STRIP_ICON, size as f64);
}

fn column_key(column: Column) -> &'static str {
    match column {
        Column::Name => "ng.finder.col.name",
        Column::Modified => "ng.finder.col.modified",
        Column::Size => "ng.finder.col.size",
        Column::Kind => "ng.finder.col.kind",
    }
}

/// The list view's per-folder column width, `None` for a folder that has never been resized.
pub(crate) fn list_column_width(folder: &str, column: Column) -> Option<f32> {
    read(&format!("{}.{folder}", column_key(column))).map(|v| v as f32)
}

pub(crate) fn set_list_column_width(folder: &str, column: Column, width: f32) {
    write(&format!("{}.{folder}", column_key(column)), width as f64);
}

/// The column (browser) view's per-folder column width.
pub(crate) fn column_view_width(folder: &str) -> Option<f32> {
    read(&format!("ng.finder.col.browser.{folder}")).map(|v| v as f32)
}

pub(crate) fn set_column_view_width(folder: &str, width: f32) {
    write(&format!("ng.finder.col.browser.{folder}"), width as f64);
}
