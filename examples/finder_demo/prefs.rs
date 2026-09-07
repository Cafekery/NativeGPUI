//! What the demo carries between launches: AppKit autosaves the frame, the sidebar sits in the
//! demo's store, the rest in user defaults.

use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSJSONReadingOptions, NSJSONSerialization, NSJSONWritingOptions,
    NSString, NSUserDefaults,
};

use crate::data::Column;

const VIEW: &str = "ng.finder.view";
const SIDEBAR: &str = "sidebar.json";
const SCOPE: &str = "ng.finder.search.scope";
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

/// The sidebar file, read whole: user defaults would follow the Mac's home, not the demo's.
fn sidebar_file() -> HashMap<String, Vec<String>> {
    let path = NSString::from_str(&crate::store().join(SIDEBAR).to_string_lossy());
    let Some(data) = NSData::dataWithContentsOfFile(&path) else {
        return HashMap::new();
    };
    let read =
        NSJSONSerialization::JSONObjectWithData_options_error(&data, NSJSONReadingOptions::empty());
    let Ok(object) = read else {
        return HashMap::new();
    };
    let Ok(map) = object.downcast::<NSDictionary>() else {
        return HashMap::new();
    };
    map.keys()
        .filter_map(|key| {
            let key = key.downcast_ref::<NSString>()?;
            let list = map.objectForKey(key)?.downcast::<NSArray>().ok()?;
            let values = list
                .iter()
                .filter_map(|value| Some(value.downcast_ref::<NSString>()?.to_string()))
                .collect();
            Some((key.to_string(), values))
        })
        .collect()
}

fn write_sidebar_file(kept: &HashMap<String, Vec<String>>) {
    let mut keys = Vec::new();
    let mut lists: Vec<Retained<AnyObject>> = Vec::new();
    for (key, values) in kept {
        keys.push(NSString::from_str(key));
        let strings: Vec<_> = values
            .iter()
            .map(|value| NSString::from_str(value))
            .collect();
        let list = NSArray::from_retained_slice(&strings);
        lists.push(unsafe { Retained::cast_unchecked(list) });
    }
    let keys: Vec<&NSString> = keys.iter().map(|key| &**key).collect();
    let values: Vec<_> = lists.iter().map(|list| &**list).collect();
    let map = NSDictionary::from_slices(&keys, &values);
    let written = unsafe {
        NSJSONSerialization::dataWithJSONObject_options_error(
            &map,
            NSJSONWritingOptions::PrettyPrinted,
        )
    };
    let Ok(data) = written else {
        return;
    };
    let path = NSString::from_str(&crate::store().join(SIDEBAR).to_string_lossy());
    data.writeToFile_atomically(&path, true);
}

fn read_list(key: &str) -> Vec<String> {
    sidebar_file().remove(key).unwrap_or_default()
}

fn write_list(key: &str, values: &[&str]) {
    let mut kept = sidebar_file();
    kept.insert(
        key.to_owned(),
        values.iter().map(|value| (*value).to_owned()).collect(),
    );
    write_sidebar_file(&kept);
}

pub(crate) fn sidebar_rows(group: &str) -> Vec<String> {
    read_list(&format!("rows.{group}"))
}

pub(crate) fn set_sidebar_rows(group: &str, rows: &[&str]) {
    write_list(&format!("rows.{group}"), rows);
}

/// The folders the user dragged into a sidebar group, which no listing of this Mac would give it.
pub(crate) fn pinned(group: &str) -> Vec<String> {
    read_list(&format!("pinned.{group}"))
}

pub(crate) fn set_pinned(group: &str, paths: &[&str]) {
    write_list(&format!("pinned.{group}"), paths);
}

pub(crate) fn view() -> Option<usize> {
    read(VIEW).map(|v| v as usize)
}

pub(crate) fn set_view(view: usize) {
    write(VIEW, view as f64);
}

pub(crate) fn search_scope() -> Option<usize> {
    read(SCOPE).map(|v| v as usize)
}

pub(crate) fn set_search_scope(scope: usize) {
    write(SCOPE, scope as f64);
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

pub(crate) fn column_view_width(folder: &str) -> Option<f32> {
    read(&format!("ng.finder.col.browser.{folder}")).map(|v| v as f32)
}

pub(crate) fn set_column_view_width(folder: &str, width: f32) {
    write(&format!("ng.finder.col.browser.{folder}"), width as f64);
}
