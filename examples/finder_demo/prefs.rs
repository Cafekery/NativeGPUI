use std::collections::HashMap;
use std::sync::LazyLock;

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSJSONReadingOptions, NSJSONSerialization, NSJSONWritingOptions,
    NSNumber, NSString, NSUserDefaults,
};

use crate::data::Column;
use crate::view::{COLUMN_VIEW, GALLERY_VIEW};

const VIEW: &str = "ng.finder.view";
const SIDEBAR: &str = "sidebar.json";
const SCOPE: &str = "ng.finder.search.scope";
const PANE: &str = "ng.finder.pane";
const ICON: &str = "ng.finder.icon";
const LIST_ICON: &str = "ng.finder.list.icon";
const STRIP_ICON: &str = "ng.finder.strip.icon";

fn defaults() -> Retained<NSUserDefaults> {
    NSUserDefaults::standardUserDefaults()
}

/// Finder's own settings for the windows it draws, read once.
pub(crate) struct Finder {
    pub(crate) posix_title: bool,
    pub(crate) text_size: f32,
    pub(crate) icon_size: f32,
    pub(crate) relative_dates: bool,
    pub(crate) airdrop: bool,
    pub(crate) folders_first: bool,
    pub(crate) sort: (Column, bool),
}

fn value(of: &NSDictionary, key: &str) -> Option<Retained<AnyObject>> {
    of.objectForKey(&NSString::from_str(key))
}

static FINDER: LazyLock<Finder> = LazyLock::new(|| {
    let suite = NSString::from_str("com.apple.finder");
    let defaults = NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&suite));
    let read = |key: &str| defaults.as_ref()?.objectForKey(&NSString::from_str(key));
    let dict = |key: &str| read(key)?.downcast::<NSDictionary>().ok();
    let list = dict("StandardViewSettings")
        .and_then(|all| value(&all, "ExtendedListViewSettingsV2")?.downcast().ok());
    let number = |key: &str| {
        let found = value(list.as_ref()?, key)?.downcast::<NSNumber>().ok()?;
        Some(found.doubleValue())
    };
    let toolbar = dict("NSToolbar Configuration Browser").and_then(|bar| {
        value(&bar, "TB Item Identifiers")?
            .downcast::<NSArray>()
            .ok()
    });
    let text = |of: &NSDictionary, key: &str| {
        Some(value(of, key)?.downcast::<NSString>().ok()?.to_string())
    };
    let sort_id = list.as_deref().and_then(|list| text(list, "sortColumn"));
    let ascending = list
        .as_deref()
        .zip(sort_id.as_deref())
        .and_then(|(list, id)| {
            let columns = value(list, "columns")?.downcast::<NSArray>().ok()?;
            columns.iter().find_map(|column| {
                let column = column.downcast::<NSDictionary>().ok()?;
                (text(&column, "identifier")? == id).then_some(())?;
                Some(
                    value(&column, "ascending")?
                        .downcast::<NSNumber>()
                        .ok()?
                        .boolValue(),
                )
            })
        });
    let sort = match sort_id.as_deref() {
        Some("name") => Some(Column::Name),
        Some("dateModified") => Some(Column::Modified),
        Some("size") => Some(Column::Size),
        Some("kind") => Some(Column::Kind),
        _ => None,
    };
    let flag = |key: &str| {
        read(key)
            .and_then(|on| on.downcast::<NSNumber>().ok())
            .is_some_and(|on| on.boolValue())
    };
    Finder {
        posix_title: flag("_FXShowPosixPathInTitle"),
        text_size: number("textSize").unwrap_or(13.) as f32,
        icon_size: number("iconSize").unwrap_or(16.) as f32,
        relative_dates: number("useRelativeDates").is_none_or(|on| on != 0.),
        airdrop: toolbar.is_some_and(|ids| {
            ids.iter().any(|id| {
                id.downcast_ref::<NSString>()
                    .is_some_and(|id| id.to_string() == "com.apple.finder.AirD")
            })
        }),
        folders_first: flag("_FXSortFoldersFirst"),
        sort: sort.map_or((Column::Name, true), |column| {
            (column, ascending.unwrap_or(true))
        }),
    }
});

pub(crate) fn finder() -> &'static Finder {
    &FINDER
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

pub(crate) fn pinned(group: &str) -> Vec<String> {
    read_list(&format!("pinned.{group}"))
}

pub(crate) fn set_pinned(group: &str, paths: &[&str]) {
    write_list(&format!("pinned.{group}"), paths);
}

pub(crate) fn removed(group: &str) -> Vec<String> {
    read_list(&format!("removed.{group}"))
}

pub(crate) fn set_removed(group: &str, ids: &[&str]) {
    write_list(&format!("removed.{group}"), ids);
}

pub(crate) fn go_history() -> Vec<String> {
    read_list("go.history")
}

pub(crate) fn set_go_history(paths: &[&str]) {
    write_list("go.history", paths);
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

/// Finder keeps one Show Preview for icon and list views, one for columns and one for gallery.
fn preview_key(view: usize) -> &'static str {
    match view {
        COLUMN_VIEW => "ng.finder.preview.column",
        GALLERY_VIEW => "ng.finder.preview.gallery",
        _ => "ng.finder.preview",
    }
}

pub(crate) fn preview(view: usize) -> bool {
    read(preview_key(view)).is_none_or(|shown| shown != 0.)
}

pub(crate) fn set_preview(view: usize, shown: bool) {
    write(preview_key(view), f64::from(u8::from(shown)));
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
